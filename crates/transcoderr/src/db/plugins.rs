use crate::plugins::manifest::DiscoveredPlugin;
use sqlx::SqlitePool;

/// Sync discovered-on-disk plugins into the `plugins` table that the UI
/// reads from. Upserts every discovered plugin (preserving the existing
/// `enabled` value -- the user's toggle wins over a redeploy) and removes
/// rows for plugins no longer on disk so the UI list doesn't accumulate
/// dead entries.
///
/// Without this call the UI page is permanently empty even though the
/// in-memory step registry happily dispatches the discovered steps.
pub async fn sync_discovered(
    pool: &SqlitePool,
    discovered: &[DiscoveredPlugin],
) -> anyhow::Result<()> {
    for d in discovered {
        let schema_json = serde_json::to_string(&d.schema)?;
        let path_str = d.manifest_dir.to_string_lossy().to_string();
        sqlx::query(
            "INSERT INTO plugins (name, version, kind, path, schema_json, enabled)
             VALUES (?, ?, ?, ?, ?, 1)
             ON CONFLICT(name) DO UPDATE SET
               version     = excluded.version,
               kind        = excluded.kind,
               path        = excluded.path,
               schema_json = excluded.schema_json",
        )
        .bind(&d.manifest.name)
        .bind(&d.manifest.version)
        .bind(&d.manifest.kind)
        .bind(&path_str)
        .bind(&schema_json)
        .execute(pool)
        .await?;
    }

    // Drop rows for plugins that disappeared from disk. Avoids the
    // confusion where an operator removes a plugin directory but the UI
    // still lists (and pretends-to-toggle) the now-stale entry.
    if discovered.is_empty() {
        sqlx::query("DELETE FROM plugins").execute(pool).await?;
    } else {
        // Build "(?, ?, ?)" placeholders dynamically -- there's no fixed
        // upper bound, but in practice this is a handful per install.
        let placeholders = std::iter::repeat("?")
            .take(discovered.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!("DELETE FROM plugins WHERE name NOT IN ({placeholders})");
        let mut q = sqlx::query(&sql);
        for d in discovered {
            q = q.bind(&d.manifest.name);
        }
        q.execute(pool).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::manifest::Manifest;
    use sqlx::Row;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn discovered(name: &str, version: &str) -> DiscoveredPlugin {
        DiscoveredPlugin {
            manifest: Manifest {
                name: name.into(),
                version: version.into(),
                kind: "subprocess".into(),
                entrypoint: Some("bin/run".into()),
                provides_steps: vec![format!("{name}.step")],
                requires: serde_json::json!({}),
                capabilities: vec![],
            },
            manifest_dir: PathBuf::from(format!("/data/plugins/{name}")),
            schema: serde_json::json!({"type": "object"}),
        }
    }

    /// Keep the `TempDir` alive for the test's lifetime -- on Linux the
    /// directory disappears the moment the helper returns, and the pool
    /// can't open new connections for the second `sync_discovered` call.
    /// Caller binds both return values: `let (pool, _dir) = open_pool().await;`
    async fn open_pool() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let pool = crate::db::open(dir.path()).await.unwrap();
        (pool, dir)
    }

    #[tokio::test]
    async fn sync_inserts_new_plugins() {
        let (pool, _dir) = open_pool().await;
        sync_discovered(&pool, &[discovered("size-report", "0.1.0")])
            .await
            .unwrap();

        let row = sqlx::query("SELECT name, version, kind, enabled FROM plugins")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(row.get::<String, _>(0), "size-report");
        assert_eq!(row.get::<String, _>(1), "0.1.0");
        assert_eq!(row.get::<String, _>(2), "subprocess");
        assert_eq!(row.get::<i64, _>(3), 1);
    }

    #[tokio::test]
    async fn sync_preserves_enabled_on_redeploy() {
        // Operator disabled the plugin via the UI toggle, then redeployed.
        // The toggle state should win over the boot-time sync.
        let (pool, _dir) = open_pool().await;
        sync_discovered(&pool, &[discovered("size-report", "0.1.0")])
            .await.unwrap();
        sqlx::query("UPDATE plugins SET enabled = 0 WHERE name = 'size-report'")
            .execute(&pool).await.unwrap();

        // Sync again with a bumped version -- enabled must stay 0.
        sync_discovered(&pool, &[discovered("size-report", "0.2.0")])
            .await.unwrap();

        let row = sqlx::query("SELECT version, enabled FROM plugins WHERE name = 'size-report'")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(row.get::<String, _>(0), "0.2.0");
        assert_eq!(row.get::<i64, _>(1), 0, "enabled toggle was reset");
    }

    #[tokio::test]
    async fn sync_drops_plugins_no_longer_on_disk() {
        let (pool, _dir) = open_pool().await;
        sync_discovered(&pool, &[
            discovered("a", "0.1.0"),
            discovered("b", "0.1.0"),
        ]).await.unwrap();

        // 'b' deleted from the plugins dir.
        sync_discovered(&pool, &[discovered("a", "0.1.0")]).await.unwrap();

        let names: Vec<String> = sqlx::query_scalar("SELECT name FROM plugins ORDER BY name")
            .fetch_all(&pool).await.unwrap();
        assert_eq!(names, vec!["a"]);
    }

    #[tokio::test]
    async fn sync_with_empty_list_clears_table() {
        let (pool, _dir) = open_pool().await;
        sync_discovered(&pool, &[discovered("x", "0.1.0")]).await.unwrap();
        sync_discovered(&pool, &[]).await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM plugins")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(count, 0);
    }
}
