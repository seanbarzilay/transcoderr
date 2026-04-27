use serde_json::Value;
use sqlx::SqlitePool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SourceRow {
    pub id: i64,
    pub kind: String,
    pub name: String,
    pub config_json: String,
    pub secret_token: String,
}

pub async fn insert(pool: &SqlitePool, kind: &str, name: &str, config: &Value, token: &str) -> anyhow::Result<i64> {
    let cj = serde_json::to_string(config)?;
    Ok(sqlx::query_scalar::<_, i64>(
        "INSERT INTO sources (kind, name, config_json, secret_token) VALUES (?, ?, ?, ?) RETURNING id"
    ).bind(kind).bind(name).bind(cj).bind(token).fetch_one(pool).await?)
}

pub async fn get_by_kind_and_token(pool: &SqlitePool, kind: &str, token: &str) -> anyhow::Result<Option<SourceRow>> {
    Ok(sqlx::query_as("SELECT id, kind, name, config_json, secret_token FROM sources WHERE kind = ? AND secret_token = ?")
        .bind(kind).bind(token).fetch_optional(pool).await?)
}

pub async fn get_webhook_by_name_and_token(pool: &SqlitePool, name: &str, token: &str) -> anyhow::Result<Option<SourceRow>> {
    Ok(sqlx::query_as("SELECT id, kind, name, config_json, secret_token FROM sources WHERE kind = 'webhook' AND name = ? AND secret_token = ?")
        .bind(name).bind(token).fetch_optional(pool).await?)
}

pub async fn list_all(pool: &SqlitePool) -> anyhow::Result<Vec<SourceRow>> {
    Ok(sqlx::query_as("SELECT id, kind, name, config_json, secret_token FROM sources ORDER BY id")
        .fetch_all(pool)
        .await?)
}

pub async fn get_by_id(pool: &SqlitePool, id: i64) -> anyhow::Result<Option<SourceRow>> {
    Ok(sqlx::query_as("SELECT id, kind, name, config_json, secret_token FROM sources WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

/// Update only the `arr_notification_id` field within an existing source's
/// `config_json`. Used by the boot reconciler when a webhook drifted and
/// got recreated under a new id.
pub async fn update_arr_notification_id(
    pool: &SqlitePool,
    source_id: i64,
    new_id: i64,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    let row: SourceRow = sqlx::query_as(
        "SELECT id, kind, name, config_json, secret_token FROM sources WHERE id = ?",
    )
    .bind(source_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| anyhow::anyhow!("source {source_id} not found"))?;

    let mut cfg: serde_json::Value = serde_json::from_str(&row.config_json)
        .map_err(|e| anyhow::anyhow!("invalid JSON in source {source_id} config: {e}"))?;
    let obj = cfg.as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("source {source_id} config is not a JSON object"))?;
    obj.insert("arr_notification_id".into(), serde_json::json!(new_id));
    let cfg_str = serde_json::to_string(&cfg)?;

    sqlx::query("UPDATE sources SET config_json = ? WHERE id = ?")
        .bind(cfg_str)
        .bind(source_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}
