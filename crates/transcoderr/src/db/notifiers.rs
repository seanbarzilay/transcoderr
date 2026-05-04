use serde_json::Value;
use sqlx::SqlitePool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct NotifierRow {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub config_json: String,
}

pub async fn upsert(
    pool: &SqlitePool,
    name: &str,
    kind: &str,
    config: &Value,
) -> anyhow::Result<i64> {
    let cj = serde_json::to_string(config)?;
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO notifiers (name, kind, config_json) VALUES (?, ?, ?)
         ON CONFLICT (name) DO UPDATE SET kind = excluded.kind, config_json = excluded.config_json
         RETURNING id",
    )
    .bind(name)
    .bind(kind)
    .bind(cj)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

pub async fn get_by_name(pool: &SqlitePool, name: &str) -> anyhow::Result<Option<NotifierRow>> {
    Ok(
        sqlx::query_as("SELECT id, name, kind, config_json FROM notifiers WHERE name = ?")
            .bind(name)
            .fetch_optional(pool)
            .await?,
    )
}
