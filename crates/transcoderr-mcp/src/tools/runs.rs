use crate::Server;
use rmcp::{
    handler::server::{tool::Parameters, wrapper::Json},
    model::ErrorData,
    tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::future::Future;
use transcoderr_api_types::{RerunResp, RunDetail, RunEvent, RunSummary};

#[derive(Deserialize, Serialize, JsonSchema, Default)]
pub struct ListRunsArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<i64>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct IdArgs {
    pub id: i64,
}

#[derive(Deserialize, Serialize, JsonSchema, Default)]
pub struct EventsArgs {
    pub id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<i64>,
}

// All tool methods land on `Server` (the same struct) so they can be composed
// with `+` per rmcp's same-struct router rule. This impl is in a child module
// of the binary crate, so it must use `crate::Server` and access fields via
// `pub(crate)` visibility.
#[tool_router(router = runs_router, vis = "pub")]
impl Server {
    #[tool(
        name = "list_runs",
        description = "List job runs, newest first. Filter by status (`pending|running|completed|failed|cancelled`), flow_id; default limit 50, max 500."
    )]
    pub async fn list_runs(
        &self,
        Parameters(a): Parameters<ListRunsArgs>,
    ) -> Result<Json<Vec<RunSummary>>, ErrorData> {
        let mut q: Vec<String> = Vec::new();
        if let Some(s) = a.status {
            q.push(format!("status={s}"));
        }
        if let Some(f) = a.flow_id {
            q.push(format!("flow_id={f}"));
        }
        if let Some(l) = a.limit {
            q.push(format!("limit={l}"));
        }
        if let Some(o) = a.offset {
            q.push(format!("offset={o}"));
        }
        let path = if q.is_empty() {
            "/api/runs".into()
        } else {
            format!("/api/runs?{}", q.join("&"))
        };
        self.api
            .get::<Vec<RunSummary>>(&path)
            .await
            .map(Json)
            .map_err(|e| e.into_error_data())
    }

    #[tool(
        name = "get_run",
        description = "Get a run by id, including its full event timeline (last 200 events)."
    )]
    pub async fn get_run(
        &self,
        Parameters(a): Parameters<IdArgs>,
    ) -> Result<Json<RunDetail>, ErrorData> {
        self.api
            .get::<RunDetail>(&format!("/api/runs/{}", a.id))
            .await
            .map(Json)
            .map_err(|e| e.into_error_data())
    }

    #[tool(
        name = "get_run_events",
        description = "Get raw events for a run, oldest first; for tailing live timelines."
    )]
    pub async fn get_run_events(
        &self,
        Parameters(a): Parameters<EventsArgs>,
    ) -> Result<Json<Vec<RunEvent>>, ErrorData> {
        let mut q: Vec<String> = Vec::new();
        if let Some(l) = a.limit {
            q.push(format!("limit={l}"));
        }
        if let Some(o) = a.offset {
            q.push(format!("offset={o}"));
        }
        let path = if q.is_empty() {
            format!("/api/runs/{}/events", a.id)
        } else {
            format!("/api/runs/{}/events?{}", a.id, q.join("&"))
        };
        self.api
            .get::<Vec<RunEvent>>(&path)
            .await
            .map(Json)
            .map_err(|e| e.into_error_data())
    }

    #[tool(
        name = "cancel_run",
        description = "Destructive: terminates a running job mid-encode. Sends SIGKILL to ffmpeg if running. Cannot be undone — the run state becomes 'cancelled'."
    )]
    pub async fn cancel_run(
        &self,
        Parameters(a): Parameters<IdArgs>,
    ) -> Result<Json<serde_json::Value>, ErrorData> {
        self.api
            .post::<serde_json::Value, _>(
                &format!("/api/runs/{}/cancel", a.id),
                &serde_json::Value::Null,
            )
            .await
            .map_err(|e| e.into_error_data())?;
        Ok(Json(serde_json::json!({"cancelled": true, "id": a.id})))
    }

    #[tool(
        name = "rerun_run",
        description = "Enqueue a new pending job using the same flow + file as the given run. Side effect: starts a new transcode in the queue. Returns the new run id."
    )]
    pub async fn rerun_run(
        &self,
        Parameters(a): Parameters<IdArgs>,
    ) -> Result<Json<RerunResp>, ErrorData> {
        self.api
            .post::<RerunResp, _>(
                &format!("/api/runs/{}/rerun", a.id),
                &serde_json::Value::Null,
            )
            .await
            .map(Json)
            .map_err(|e| e.into_error_data())
    }
}
