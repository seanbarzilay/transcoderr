use crate::db::now_unix;
use crate::flow::Flow;
use sqlx::SqlitePool;

#[derive(Debug, Clone)]
pub struct FlowRow {
    pub id: i64,
    pub name: String,
    pub enabled: bool,
    pub yaml_source: String,
    pub parsed_json: String,
    pub version: i64,
}

pub async fn insert(pool: &SqlitePool, name: &str, yaml: &str, parsed: &Flow) -> anyhow::Result<i64> {
    let parsed_json = serde_json::to_string(parsed)?;
    let now = now_unix();
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO flows (name, enabled, yaml_source, parsed_json, version, updated_at) \
         VALUES (?, 1, ?, ?, 1, ?) RETURNING id"
    )
    .bind(name).bind(yaml).bind(&parsed_json).bind(now)
    .fetch_one(pool).await?;
    sqlx::query("INSERT INTO flow_versions (flow_id, version, yaml_source, created_at) VALUES (?, 1, ?, ?)")
        .bind(id).bind(yaml).bind(now)
        .execute(pool).await?;
    Ok(id)
}

pub async fn get_by_name(pool: &SqlitePool, name: &str) -> anyhow::Result<Option<FlowRow>> {
    let row = sqlx::query_as::<_, (i64, String, i64, String, String, i64)>(
        "SELECT id, name, enabled, yaml_source, parsed_json, version FROM flows WHERE name = ?"
    ).bind(name).fetch_optional(pool).await?;
    Ok(row.map(|(id, name, enabled, yaml_source, parsed_json, version)| FlowRow {
        id, name, enabled: enabled != 0, yaml_source, parsed_json, version
    }))
}

pub async fn list_enabled_for_radarr(pool: &SqlitePool, event: &str) -> anyhow::Result<Vec<FlowRow>> {
    let all = sqlx::query_as::<_, (i64, String, i64, String, String, i64)>(
        "SELECT id, name, enabled, yaml_source, parsed_json, version FROM flows WHERE enabled = 1"
    ).fetch_all(pool).await?;
    let mut out = vec![];
    for (id, name, enabled, yaml_source, parsed_json, version) in all {
        let flow: Flow = serde_json::from_str(&parsed_json)?;
        let matches = flow.triggers.iter().any(|t| match t {
            crate::flow::Trigger::Radarr(events) => events.iter().any(|e| e == event),
            _ => false,
        });
        if matches {
            out.push(FlowRow { id, name, enabled: enabled != 0, yaml_source, parsed_json, version });
        }
    }
    Ok(out)
}
