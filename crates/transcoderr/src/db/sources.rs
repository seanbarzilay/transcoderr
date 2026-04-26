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
