//! CRUD for the `workers` table.
//!
//! Tokens are stored verbatim (matching the existing sources/notifiers
//! pattern at `db/sources.rs`) — they're random 32-byte hex strings,
//! not user-chosen, so the hashed-bcrypt path used by `db/api_tokens.rs`
//! buys nothing here.

use serde::Serialize;
use sqlx::{FromRow, SqlitePool};

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct WorkerRow {
    pub id: i64,
    pub name: String,
    pub kind: String, // 'local' | 'remote'
    #[sqlx(default)]
    pub secret_token: Option<String>,
    #[sqlx(default)]
    pub hw_caps_json: Option<String>,
    #[sqlx(default)]
    pub plugin_manifest_json: Option<String>,
    pub enabled: i64,
    #[sqlx(default)]
    pub last_seen_at: Option<i64>,
    pub created_at: i64,
}

/// Insert a new remote worker. Returns its id.
pub async fn insert_remote(
    pool: &SqlitePool,
    name: &str,
    secret_token: &str,
) -> anyhow::Result<i64> {
    let id: (i64,) = sqlx::query_as(
        "INSERT INTO workers (name, kind, secret_token, enabled, created_at)
         VALUES (?, 'remote', ?, 1, strftime('%s','now'))
         RETURNING id",
    )
    .bind(name)
    .bind(secret_token)
    .fetch_one(pool)
    .await?;
    Ok(id.0)
}

pub async fn list_all(pool: &SqlitePool) -> anyhow::Result<Vec<WorkerRow>> {
    Ok(sqlx::query_as(
        "SELECT id, name, kind, secret_token, hw_caps_json, plugin_manifest_json,
                enabled, last_seen_at, created_at
           FROM workers
          ORDER BY id",
    )
    .fetch_all(pool)
    .await?)
}

pub async fn get_by_id(pool: &SqlitePool, id: i64) -> anyhow::Result<Option<WorkerRow>> {
    Ok(sqlx::query_as(
        "SELECT id, name, kind, secret_token, hw_caps_json, plugin_manifest_json,
                enabled, last_seen_at, created_at
           FROM workers WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

/// Find a worker by its (verbatim) secret token. Used by the auth path
/// and by the WS upgrade handler.
pub async fn get_by_token(
    pool: &SqlitePool,
    token: &str,
) -> anyhow::Result<Option<WorkerRow>> {
    Ok(sqlx::query_as(
        "SELECT id, name, kind, secret_token, hw_caps_json, plugin_manifest_json,
                enabled, last_seen_at, created_at
           FROM workers WHERE secret_token = ?",
    )
    .bind(token)
    .fetch_optional(pool)
    .await?)
}

/// Delete a remote worker by id. Refuses to touch the local row.
pub async fn delete_remote(pool: &SqlitePool, id: i64) -> anyhow::Result<u64> {
    let res = sqlx::query("DELETE FROM workers WHERE id = ? AND kind = 'remote'")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// Stamp the worker's last-seen timestamp + last register payload after a
/// successful register frame.
pub async fn record_register(
    pool: &SqlitePool,
    id: i64,
    hw_caps_json: &str,
    plugin_manifest_json: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE workers
            SET hw_caps_json         = ?,
                plugin_manifest_json = ?,
                last_seen_at         = strftime('%s','now')
          WHERE id = ?",
    )
    .bind(hw_caps_json)
    .bind(plugin_manifest_json)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Stamp last_seen_at on a heartbeat or any other live frame.
pub async fn record_heartbeat(pool: &SqlitePool, id: i64) -> anyhow::Result<()> {
    sqlx::query("UPDATE workers SET last_seen_at = strftime('%s','now') WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
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
    async fn local_row_is_seeded_by_migration() {
        let (pool, _dir) = pool().await;
        let rows = list_all(&pool).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "local");
        assert_eq!(rows[0].kind, "local");
        assert!(rows[0].secret_token.is_none());
        assert_eq!(rows[0].enabled, 1);
    }

    #[tokio::test]
    async fn insert_remote_returns_id_and_appears_in_list() {
        let (pool, _dir) = pool().await;
        let id = insert_remote(&pool, "gpu-box-1", "wkr_abcdef").await.unwrap();
        let rows = list_all(&pool).await.unwrap();
        assert_eq!(rows.len(), 2); // local + new remote
        assert!(rows.iter().any(|r| r.id == id && r.kind == "remote"));
    }

    #[tokio::test]
    async fn get_by_token_finds_remote_only() {
        let (pool, _dir) = pool().await;
        insert_remote(&pool, "gpu-box-1", "wkr_secret_xyz").await.unwrap();
        let found = get_by_token(&pool, "wkr_secret_xyz").await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "gpu-box-1");
        assert!(get_by_token(&pool, "nope").await.unwrap().is_none());
        // The local row has NULL secret_token so it's not findable by any value.
        assert!(get_by_token(&pool, "").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn delete_remote_refuses_local_row() {
        let (pool, _dir) = pool().await;
        let removed = delete_remote(&pool, 1).await.unwrap(); // id=1 is the seeded local row
        assert_eq!(removed, 0);
        let rows = list_all(&pool).await.unwrap();
        assert_eq!(rows.len(), 1); // local row still there
    }

    #[tokio::test]
    async fn record_register_stamps_payload_and_last_seen() {
        let (pool, _dir) = pool().await;
        let id = insert_remote(&pool, "w", "tok").await.unwrap();
        record_register(&pool, id, r#"{"encoders":[]}"#, r#"[]"#).await.unwrap();
        let row = get_by_id(&pool, id).await.unwrap().unwrap();
        assert_eq!(row.hw_caps_json.as_deref(), Some(r#"{"encoders":[]}"#));
        assert_eq!(row.plugin_manifest_json.as_deref(), Some(r#"[]"#));
        assert!(row.last_seen_at.is_some());
    }
}
