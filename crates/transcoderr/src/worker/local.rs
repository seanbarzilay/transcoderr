//! Local-worker registration. The seeded `local` row in the `workers`
//! table (id=1) gets stamped with hw_caps + plugin_manifest at boot,
//! and a background heartbeat task keeps `last_seen_at` fresh every 30s
//! regardless of whether the row is enabled.
//!
//! `is_enabled` is consulted by `pool::Worker::run_loop` before each
//! claim — toggling `workers.enabled` from the UI is the per-worker
//! kill switch (graceful drain: the in-flight job finishes; the next
//! claim short-circuits).

use crate::db;
use crate::ffmpeg_caps::FfmpegCaps;
use crate::plugins::manifest::DiscoveredPlugin;
use crate::worker::protocol::PluginManifestEntry;
use sqlx::SqlitePool;
use std::time::Duration;

/// Pinned to the migration's `INSERT INTO workers (...) VALUES ('local',
/// 'local', 1, ...)` which gets `rowid=1` on a fresh database. If the
/// migration ever reorders that insert, this constant moves with it.
pub const LOCAL_WORKER_ID: i64 = 1;

/// 30s — matches the remote worker `HEARTBEAT_INTERVAL` in
/// `worker/connection.rs`. Keeping the cadence identical means the UI's
/// "stale after 90s" logic is uniform across local and remote rows.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

/// Stamp the seeded `local` row with the coordinator's current hw_caps
/// and plugin manifest. Failure logs a warning and returns `Ok(())` —
/// boot must not block on this. The pool keeps working; only the
/// Workers UI shows stale data until next register.
pub async fn register_local_worker(
    pool: &SqlitePool,
    ffmpeg_caps: &FfmpegCaps,
    plugins: &[DiscoveredPlugin],
) -> anyhow::Result<()> {
    let hw_caps = serde_json::json!({
        "has_libplacebo": ffmpeg_caps.has_libplacebo,
    });
    let hw_caps_json = serde_json::to_string(&hw_caps).unwrap_or_else(|_| "null".into());

    let manifest: Vec<PluginManifestEntry> = plugins
        .iter()
        .map(|p| PluginManifestEntry {
            name: p.manifest.name.clone(),
            version: p.manifest.version.clone(),
            sha256: None,
        })
        .collect();
    let plugin_manifest_json =
        serde_json::to_string(&manifest).unwrap_or_else(|_| "[]".into());

    if let Err(e) = db::workers::record_register(
        pool,
        LOCAL_WORKER_ID,
        &hw_caps_json,
        &plugin_manifest_json,
    )
    .await
    {
        tracing::warn!(error = ?e, "local worker register failed; UI may show stale row");
    }
    Ok(())
}

/// Spawn the local heartbeat task. Stamps `last_seen_at` every 30s on
/// the seeded `local` row regardless of `enabled`. This is what makes
/// the UI distinguish "operator turned it off" (enabled=false, fresh
/// last_seen) from "the daemon is dead" (enabled=true, stale last_seen).
pub fn spawn_local_heartbeat(pool: SqlitePool) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(HEARTBEAT_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // First tick fires immediately; skip it because we already
        // stamped `last_seen_at` via record_register at boot.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if let Err(e) = db::workers::record_heartbeat(&pool, LOCAL_WORKER_ID).await {
                tracing::warn!(error = ?e, "local heartbeat failed");
            }
        }
    });
}

/// True if the local worker row is enabled. Defaults to `true` on DB
/// error so transient sqlite hiccups don't stall the pool.
pub async fn is_enabled(pool: &SqlitePool) -> bool {
    let row: Result<(i64,), _> =
        sqlx::query_as("SELECT enabled FROM workers WHERE id = ?")
            .bind(LOCAL_WORKER_ID)
            .fetch_one(pool)
            .await;
    match row {
        Ok((flag,)) => flag != 0,
        Err(e) => {
            tracing::warn!(error = ?e, "is_enabled query failed; defaulting to true");
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    async fn pool() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let pool = crate::db::open(dir.path()).await.unwrap();
        (pool, dir)
    }

    #[tokio::test]
    async fn is_enabled_returns_column_value() {
        let (pool, _dir) = pool().await;
        // Seeded enabled=1.
        assert!(is_enabled(&pool).await);

        db::workers::set_enabled(&pool, LOCAL_WORKER_ID, false).await.unwrap();
        assert!(!is_enabled(&pool).await);

        db::workers::set_enabled(&pool, LOCAL_WORKER_ID, true).await.unwrap();
        assert!(is_enabled(&pool).await);
    }

    #[tokio::test]
    async fn is_enabled_defaults_true_when_row_missing() {
        // We can't easily fabricate a "closed pool" so cover the
        // not-found path instead (also routes through fetch_one's
        // RowNotFound error → the warn path).
        let (pool, _dir) = pool().await;
        // Drop the seeded row.
        sqlx::query("DELETE FROM workers WHERE id = ?")
            .bind(LOCAL_WORKER_ID)
            .execute(&pool)
            .await
            .unwrap();
        assert!(is_enabled(&pool).await, "missing row must default to true");
    }
}
