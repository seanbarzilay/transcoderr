use crate::Server;
use rmcp::{
    handler::server::{tool::Parameters, wrapper::Json},
    model::{ErrorCode, ErrorData},
    tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::future::Future; // pulled in by #[tool_router] macro expansion
use transcoderr_api_types::{CreateFlowReq, FlowDetail, FlowSummary, UpdateFlowReq};

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct IdArgs {
    pub id: i64,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct UpdateFlowArgs {
    pub id: i64,
    pub yaml: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct DeleteFlowArgs {
    pub id: i64,
    /// Required confirmation. Reject the call by setting this to false.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct DryRunArgs {
    pub yaml: String,
    pub file_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "transcoderr_api_types::optional_json_object_schema")]
    pub probe: Option<serde_json::Value>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ValidateFlowArgs {
    pub yaml: String,
}

#[tool_router(router = flows_router, vis = "pub")]
impl Server {
    #[tool(
        name = "list_flows",
        description = "List all configured transcode flows."
    )]
    pub async fn list_flows(
        &self,
        _: Parameters<super::NoArgs>,
    ) -> Result<Json<Vec<FlowSummary>>, ErrorData> {
        self.api
            .get::<Vec<FlowSummary>>("/api/flows")
            .await
            .map(Json)
            .map_err(|e| e.into_error_data())
    }

    #[tool(
        name = "get_flow",
        description = "Get a flow by id with its YAML source and parsed AST."
    )]
    pub async fn get_flow(
        &self,
        Parameters(a): Parameters<IdArgs>,
    ) -> Result<Json<FlowDetail>, ErrorData> {
        self.api
            .get::<FlowDetail>(&format!("/api/flows/{}", a.id))
            .await
            .map(Json)
            .map_err(|e| e.into_error_data())
    }

    #[tool(
        name = "create_flow",
        description = "Create a new flow from YAML. Side effect: persists a new flow available for webhook dispatch. Name must be unique."
    )]
    pub async fn create_flow(
        &self,
        Parameters(a): Parameters<CreateFlowReq>,
    ) -> Result<Json<FlowSummary>, ErrorData> {
        self.api
            .post::<FlowSummary, _>("/api/flows", &a)
            .await
            .map(Json)
            .map_err(|e| e.into_error_data())
    }

    #[tool(
        name = "update_flow",
        description = "Replace the YAML for an existing flow. Side effect: bumps the flow version; future jobs will use the new YAML. Optionally toggles `enabled`."
    )]
    pub async fn update_flow(
        &self,
        Parameters(a): Parameters<UpdateFlowArgs>,
    ) -> Result<Json<serde_json::Value>, ErrorData> {
        let body = UpdateFlowReq {
            yaml: a.yaml,
            enabled: a.enabled,
        };
        self.api
            .put::<serde_json::Value, _>(&format!("/api/flows/{}", a.id), &body)
            .await
            .map_err(|e| e.into_error_data())?;
        Ok(Json(serde_json::json!({"updated": true, "id": a.id})))
    }

    #[tool(
        name = "delete_flow",
        description = "Destructive: permanently delete a flow. Cannot be undone. Requires confirm=true."
    )]
    pub async fn delete_flow(
        &self,
        Parameters(a): Parameters<DeleteFlowArgs>,
    ) -> Result<Json<serde_json::Value>, ErrorData> {
        if !a.confirm {
            return Err(ErrorData {
                code: ErrorCode::INVALID_PARAMS,
                message: "delete_flow requires `confirm: true`".into(),
                data: None,
            });
        }
        self.api
            .delete::<serde_json::Value>(&format!("/api/flows/{}", a.id))
            .await
            .map_err(|e| e.into_error_data())?;
        Ok(Json(serde_json::json!({"deleted": true, "id": a.id})))
    }

    #[tool(
        name = "dry_run_flow",
        description = "Walk a flow's AST against a synthetic file path to see which steps would execute. No side effects."
    )]
    pub async fn dry_run_flow(
        &self,
        Parameters(a): Parameters<DryRunArgs>,
    ) -> Result<Json<serde_json::Value>, ErrorData> {
        self.api
            .post::<serde_json::Value, _>("/api/dry-run", &a)
            .await
            .map(Json)
            .map_err(|e| e.into_error_data())
    }

    #[tool(
        name = "validate_flow",
        description = "Static check on a flow YAML — surfaces YAML parse errors AND every CEL compile error in `if:` conditions and `{{ ... }}` templates. Returns {ok, issues:[{path, kind, message}]}. The runtime evaluator silently treats CEL errors in `if:` as `false`, so a typo in a guard would otherwise disable the branch without warning. Run this before `update_flow` / `create_flow`. No side effects."
    )]
    pub async fn validate_flow(
        &self,
        Parameters(a): Parameters<ValidateFlowArgs>,
    ) -> Result<Json<serde_json::Value>, ErrorData> {
        self.api
            .post::<serde_json::Value, _>("/api/flows/validate", &a)
            .await
            .map(Json)
            .map_err(|e| e.into_error_data())
    }
}
