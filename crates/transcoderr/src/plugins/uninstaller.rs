use sqlx::SqlitePool;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum UninstallError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("db: {0}")]
    Db(#[from] sqlx::Error),
    #[error("plugin {0:?} not found in DB")]
    NotFound(String),
}

/// Remove the plugin directory from disk and drop the row from the DB.
/// Caller is responsible for the registry rebuild afterwards.
///
/// **Concurrency:** if a subprocess plugin is currently mid-run when
/// uninstall fires, the in-flight `Arc<dyn Step>` snapshot held by the
/// caller keeps working. On Linux/macOS, removing the entrypoint file
/// while the subprocess is executing it is safe — the kernel keeps the
/// inode alive while the file descriptor is open, so the running step
/// completes against the deleted-on-disk binary. Windows does NOT have
/// this semantic; transcoderr is documented as Linux/macOS-only.
pub async fn uninstall(
    pool: &SqlitePool,
    plugins_dir: &Path,
    plugin_id: i64,
) -> Result<String, UninstallError> {
    use sqlx::Row;
    let row = sqlx::query("SELECT name FROM plugins WHERE id = ?")
        .bind(plugin_id).fetch_optional(pool).await?;
    let row = match row {
        Some(r) => r,
        None => return Err(UninstallError::NotFound(plugin_id.to_string())),
    };
    let name: String = row.get(0);
    let dir = plugins_dir.join(&name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    sqlx::query("DELETE FROM plugins WHERE id = ?").bind(plugin_id).execute(pool).await?;

    // Best-effort: clear any cached source tarballs for this name.
    // Coordinator-side only — the worker has no cache dir, so the
    // glob simply finds nothing.
    clear_tarball_cache(plugins_dir, &name);

    Ok(name)
}

/// Worker-side uninstall: just remove the plugin directory. No DB,
/// no cache (workers don't keep tarballs). Best-effort — missing
/// directory is not an error.
pub fn uninstall_by_name(plugins_dir: &Path, name: &str) -> Result<(), UninstallError> {
    let dir = plugins_dir.join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(())
}

/// Best-effort glob deletion of `<plugins_dir>/.tarball-cache/<name>-*.tar.gz`.
/// Failures are logged but never returned — uninstall is already
/// well underway by the time we get here, and a stuck cache file
/// just wastes disk.
fn clear_tarball_cache(plugins_dir: &Path, name: &str) {
    let cache_dir = plugins_dir.join(".tarball-cache");
    let prefix = format!("{name}-");
    let suffix = ".tar.gz";
    let entries = match std::fs::read_dir(&cache_dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let fname = entry.file_name();
        let fname = fname.to_string_lossy();
        if fname.starts_with(&prefix) && fname.ends_with(suffix) {
            if let Err(e) = std::fs::remove_file(entry.path()) {
                tracing::warn!(error = ?e, file = %fname, "failed to remove cache file");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    async fn open_pool() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let pool = crate::db::open(dir.path()).await.unwrap();
        (pool, dir)
    }

    #[tokio::test]
    async fn uninstall_removes_dir_and_db_row() {
        let (pool, _data) = open_pool().await;
        let plugins_dir = tempdir().unwrap();
        let plugin_dir = plugins_dir.path().join("foo");
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::write(plugin_dir.join("manifest.toml"), "name = \"foo\"\nversion = \"0.1.0\"\nkind = \"subprocess\"\nentrypoint = \"bin/run\"\nprovides_steps = []\n").unwrap();

        let id: i64 = sqlx::query_scalar(
            "INSERT INTO plugins (name, version, kind, path, schema_json, enabled) \
             VALUES ('foo', '0.1.0', 'subprocess', ?, '{}', 1) RETURNING id"
        ).bind(plugin_dir.to_string_lossy().to_string())
         .fetch_one(&pool).await.unwrap();

        uninstall(&pool, plugins_dir.path(), id).await.unwrap();
        assert!(!plugin_dir.exists());
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM plugins")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn uninstall_returns_not_found_for_missing_id() {
        let (pool, _data) = open_pool().await;
        let plugins_dir = tempdir().unwrap();
        let err = uninstall(&pool, plugins_dir.path(), 9999).await.unwrap_err();
        assert!(matches!(err, UninstallError::NotFound(_)));
    }

    #[test]
    fn uninstall_by_name_removes_plugin_dir() {
        let dir = tempdir().unwrap();
        let plugins_dir = dir.path();
        let name = "foo";
        std::fs::create_dir_all(plugins_dir.join(name).join("bin")).unwrap();
        assert!(plugins_dir.join(name).exists());

        uninstall_by_name(plugins_dir, name).unwrap();
        assert!(!plugins_dir.join(name).exists());
    }

    #[test]
    fn uninstall_by_name_missing_dir_is_ok() {
        let dir = tempdir().unwrap();
        // No plugins/foo/ ever created — uninstall should succeed.
        uninstall_by_name(dir.path(), "foo").unwrap();
    }

    #[test]
    fn clear_tarball_cache_removes_matching_files() {
        let dir = tempdir().unwrap();
        let plugins_dir = dir.path();
        let cache = plugins_dir.join(".tarball-cache");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("foo-abc.tar.gz"), b"x").unwrap();
        std::fs::write(cache.join("foo-def.tar.gz"), b"y").unwrap();
        std::fs::write(cache.join("bar-xyz.tar.gz"), b"z").unwrap();

        clear_tarball_cache(plugins_dir, "foo");

        assert!(!cache.join("foo-abc.tar.gz").exists());
        assert!(!cache.join("foo-def.tar.gz").exists());
        assert!(cache.join("bar-xyz.tar.gz").exists(), "bar should be untouched");
    }
}
