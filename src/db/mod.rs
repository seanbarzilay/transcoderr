use sqlx::{sqlite::{SqliteConnectOptions, SqlitePoolOptions}, SqlitePool};
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
        .connect_with(opts).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}

pub fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

pub mod flows;
pub mod jobs;
pub mod notifiers;
pub mod run_events;
pub mod checkpoints;
pub mod sources;

pub async fn snapshot_hw_caps(
    pool: &SqlitePool,
    caps: &crate::hw::HwCaps,
) -> anyhow::Result<()> {
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
            .fetch_one(&pool).await.unwrap();
        assert_eq!(count, 0);
    }
}
