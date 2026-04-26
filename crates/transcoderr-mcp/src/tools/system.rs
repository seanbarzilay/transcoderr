use crate::Server;
use rmcp::{
    handler::server::{tool::Parameters, wrapper::Json},
    model::ErrorData,
    tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::future::Future; // pulled in by #[tool_router] macro expansion
use transcoderr_api_types::{Health, RunSummary};

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct QueueResp {
    pub pending: Vec<RunSummary>,
    pub running: Vec<RunSummary>,
}

#[tool_router(router = system_router, vis = "pub")]
impl Server {
    #[tool(name = "get_health", description = "Server health snapshot — probes /healthz and /readyz. Read-only, no side effects.")]
    pub async fn get_health(&self, _: Parameters<super::NoArgs>) -> Result<Json<Health>, ErrorData> {
        let healthy = self.api.get_text("/healthz").await.is_ok();
        let ready = self.api.get_text("/readyz").await.is_ok();
        Ok(Json(Health { healthy, ready }))
    }

    #[tool(name = "get_queue", description = "Pending and currently-running jobs. Read-only.")]
    pub async fn get_queue(&self, _: Parameters<super::NoArgs>) -> Result<Json<QueueResp>, ErrorData> {
        let pending = self.api.get::<Vec<RunSummary>>("/api/runs?status=pending&limit=500").await
            .map_err(|e| e.into_error_data())?;
        let running = self.api.get::<Vec<RunSummary>>("/api/runs?status=running&limit=500").await
            .map_err(|e| e.into_error_data())?;
        Ok(Json(QueueResp { pending, running }))
    }

    #[tool(name = "get_hw_caps", description = "Hardware-encode capability snapshot — NVENC/QSV/VAAPI/VideoToolbox detection results from boot probe. Read-only.")]
    pub async fn get_hw_caps(&self, _: Parameters<super::NoArgs>) -> Result<Json<serde_json::Value>, ErrorData> {
        self.api.get::<serde_json::Value>("/api/hw").await.map(Json).map_err(|e| e.into_error_data())
    }

    #[tool(name = "get_metrics", description = "Prometheus metrics text exposition (passthrough from /metrics). Read-only.")]
    pub async fn get_metrics(&self, _: Parameters<super::NoArgs>) -> Result<Json<serde_json::Value>, ErrorData> {
        let txt = self.api.get_text("/metrics").await.map_err(|e| e.into_error_data())?;
        Ok(Json(serde_json::Value::String(txt)))
    }
}
