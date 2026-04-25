use crate::db::now_unix;
use serde_json::Value;
use sqlx::SqlitePool;
use std::path::Path;

pub async fn append(
    pool: &SqlitePool,
    job_id: i64,
    step_id: Option<&str>,
    kind: &str,
    payload: Option<&Value>,
) -> anyhow::Result<()> {
    let payload_json = payload.map(|v| serde_json::to_string(v)).transpose()?;
    sqlx::query("INSERT INTO run_events (job_id, ts, step_id, kind, payload_json) VALUES (?, ?, ?, ?, ?)")
        .bind(job_id).bind(now_unix()).bind(step_id).bind(kind).bind(payload_json)
        .execute(pool).await?;
    Ok(())
}

pub async fn append_with_bus(
    pool: &SqlitePool,
    bus: &crate::bus::Bus,
    job_id: i64,
    step_id: Option<&str>,
    kind: &str,
    payload: Option<&Value>,
) -> anyhow::Result<()> {
    append(pool, job_id, step_id, kind, payload).await?;
    bus.send(crate::bus::Event::RunEvent {
        job_id,
        step_id: step_id.map(|s| s.to_string()),
        kind: kind.to_string(),
        payload: payload.cloned().unwrap_or(Value::Null),
    });
    Ok(())
}

pub async fn append_with_spill(
    pool: &SqlitePool,
    data_dir: &Path,
    job_id: i64,
    step_id: Option<&str>,
    kind: &str,
    payload: Option<&Value>,
) -> anyhow::Result<()> {
    let payload_json = payload.map(|v| serde_json::to_string(v)).transpose()?;
    let event_id: i64 = sqlx::query_scalar(
        "INSERT INTO run_events (job_id, ts, step_id, kind) VALUES (?, ?, ?, ?) RETURNING id"
    ).bind(job_id).bind(now_unix()).bind(step_id).bind(kind).fetch_one(pool).await?;
    if let Some(p) = payload_json {
        if let Some(path) = crate::log_spill::maybe_spill(data_dir, job_id, step_id, event_id, &p).await? {
            sqlx::query("UPDATE run_events SET payload_path = ? WHERE id = ?")
                .bind(path.to_string_lossy().as_ref()).bind(event_id).execute(pool).await?;
        } else {
            sqlx::query("UPDATE run_events SET payload_json = ? WHERE id = ?")
                .bind(&p).bind(event_id).execute(pool).await?;
        }
    }
    Ok(())
}

pub async fn append_with_bus_and_spill(
    pool: &SqlitePool,
    bus: &crate::bus::Bus,
    data_dir: &Path,
    job_id: i64,
    step_id: Option<&str>,
    kind: &str,
    payload: Option<&Value>,
) -> anyhow::Result<()> {
    append_with_spill(pool, data_dir, job_id, step_id, kind, payload).await?;
    bus.send(crate::bus::Event::RunEvent {
        job_id,
        step_id: step_id.map(|s| s.to_string()),
        kind: kind.to_string(),
        payload: payload.cloned().unwrap_or(Value::Null),
    });
    Ok(())
}
