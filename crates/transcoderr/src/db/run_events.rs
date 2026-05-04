use crate::db::now_unix;
use serde_json::Value;
use sqlx::SqlitePool;
use std::path::Path;

pub struct RunEventInput<'a> {
    pub job_id: i64,
    pub step_id: Option<&'a str>,
    pub worker_id: Option<i64>,
    pub kind: &'a str,
    pub payload: Option<&'a Value>,
}

impl<'a> RunEventInput<'a> {
    pub fn new(job_id: i64, kind: &'a str) -> Self {
        Self {
            job_id,
            step_id: None,
            worker_id: None,
            kind,
            payload: None,
        }
    }

    pub fn step_id(mut self, step_id: &'a str) -> Self {
        self.step_id = Some(step_id);
        self
    }

    pub fn worker_id(mut self, worker_id: Option<i64>) -> Self {
        self.worker_id = worker_id;
        self
    }

    pub fn payload(mut self, payload: &'a Value) -> Self {
        self.payload = Some(payload);
        self
    }
}

pub async fn append(
    pool: &SqlitePool,
    job_id: i64,
    step_id: Option<&str>,
    kind: &str,
    payload: Option<&Value>,
) -> anyhow::Result<()> {
    let payload_json = payload.map(serde_json::to_string).transpose()?;
    sqlx::query(
        "INSERT INTO run_events (job_id, ts, step_id, kind, payload_json) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(job_id)
    .bind(now_unix())
    .bind(step_id)
    .bind(kind)
    .bind(payload_json)
    .execute(pool)
    .await?;
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
        worker_id: None,
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
    worker_id: Option<i64>,
    kind: &str,
    payload: Option<&Value>,
) -> anyhow::Result<()> {
    let payload_json = payload.map(serde_json::to_string).transpose()?;
    let event_id: i64 = sqlx::query_scalar(
        "INSERT INTO run_events (job_id, ts, step_id, worker_id, kind) VALUES (?, ?, ?, ?, ?) RETURNING id"
    ).bind(job_id).bind(now_unix()).bind(step_id).bind(worker_id).bind(kind).fetch_one(pool).await?;
    if let Some(p) = payload_json {
        if let Some(path) =
            crate::log_spill::maybe_spill(data_dir, job_id, step_id, event_id, &p).await?
        {
            sqlx::query("UPDATE run_events SET payload_path = ? WHERE id = ?")
                .bind(path.to_string_lossy().as_ref())
                .bind(event_id)
                .execute(pool)
                .await?;
        } else {
            sqlx::query("UPDATE run_events SET payload_json = ? WHERE id = ?")
                .bind(&p)
                .bind(event_id)
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

pub async fn append_with_bus_and_spill(
    pool: &SqlitePool,
    bus: &crate::bus::Bus,
    data_dir: &Path,
    event: RunEventInput<'_>,
) -> anyhow::Result<()> {
    append_with_spill(
        pool,
        data_dir,
        event.job_id,
        event.step_id,
        event.worker_id,
        event.kind,
        event.payload,
    )
    .await?;
    bus.send(crate::bus::Event::RunEvent {
        job_id: event.job_id,
        step_id: event.step_id.map(|s| s.to_string()),
        worker_id: event.worker_id,
        kind: event.kind.to_string(),
        payload: event.payload.cloned().unwrap_or(Value::Null),
    });
    Ok(())
}
