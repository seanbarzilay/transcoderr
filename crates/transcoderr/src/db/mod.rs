use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    SqlitePool,
};
use std::{path::Path, str::FromStr, time::Duration};

pub async fn open(data_dir: &Path) -> anyhow::Result<SqlitePool> {
    std::fs::create_dir_all(data_dir)?;
    let db_path = data_dir.join("data.db");
    let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", db_path.display()))?
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5))
        .pragma("foreign_keys", "ON");
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(opts)
        .await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    check_migrations_compatible(&pool).await?;
    Ok(pool)
}

/// Open an in-memory SQLite pool with all migrations applied. Used
/// by the worker daemon process — the worker's steps may consult
/// settings or scratch tables, but the worker doesn't share state
/// with the coordinator's DB. Each worker process gets its own
/// in-memory schema-only pool.
pub async fn open_in_memory() -> anyhow::Result<SqlitePool> {
    let pool = SqlitePool::connect("sqlite::memory:").await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}

pub async fn check_migrations_compatible(pool: &SqlitePool) -> anyhow::Result<()> {
    let migrator = sqlx::migrate!("./migrations");
    let known: std::collections::HashSet<i64> = migrator.iter().map(|m| m.version).collect();
    let applied: Vec<(i64,)> = sqlx::query_as("SELECT version FROM _sqlx_migrations")
        .fetch_all(pool)
        .await
        .unwrap_or_default();
    for (v,) in applied {
        if !known.contains(&v) {
            anyhow::bail!("DB has migration {v} unknown to this binary — refusing to start");
        }
    }
    Ok(())
}

pub fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

pub mod api_tokens;
pub mod checkpoints;
pub mod flows;
pub mod jobs;
pub mod notifiers;
pub mod plugin_catalogs;
pub mod plugins;
pub mod run_events;
pub mod settings;
pub mod sources;
pub mod workers;

pub async fn snapshot_hw_caps(pool: &SqlitePool, caps: &crate::hw::HwCaps) -> anyhow::Result<()> {
    let json = serde_json::to_string(caps)?;
    sqlx::query(
        "INSERT INTO hw_capabilities (id, probed_at, devices_json) VALUES (1, ?, ?)
         ON CONFLICT (id) DO UPDATE SET probed_at = excluded.probed_at, devices_json = excluded.devices_json",
    )
    .bind(caps.probed_at)
    .bind(json)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn opens_and_migrates() {
        let dir = tempdir().unwrap();
        let pool = open(dir.path()).await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM flows")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn migration_seeds_official_plugin_catalog() {
        let dir = tempdir().unwrap();
        let pool = open(dir.path()).await.unwrap();
        let row = sqlx::query(
            "SELECT name, priority FROM plugin_catalogs WHERE name = 'transcoderr official'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        use sqlx::Row;
        assert_eq!(row.get::<String, _>(0), "transcoderr official");
        assert_eq!(row.get::<i64, _>(1), 0);
    }
}
