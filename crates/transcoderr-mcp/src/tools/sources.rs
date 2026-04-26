use crate::Server;
use rmcp::{
    handler::server::{tool::Parameters, wrapper::Json},
    model::{ErrorCode, ErrorData},
    tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::future::Future; // pulled in by #[tool_router] macro expansion
use transcoderr_api_types::{CreateSourceReq, CreatedIdResp, SourceSummary, UpdateSourceReq};

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct IdArgs {
    pub id: i64,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct UpdateArgs {
    pub id: i64,
    #[serde(flatten)]
    pub patch: UpdateSourceReq,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct DeleteSourceArgs {
    pub id: i64,
    /// Required confirmation. Reject the call by setting this to false.
    pub confirm: bool,
}

#[tool_router(router = sources_router, vis = "pub")]
impl Server {
    #[tool(name = "list_sources", description = "List webhook sources (radarr/sonarr/lidarr/generic). Secret tokens are redacted to `***` in the response (token-authed callers can't recover them).")]
    pub async fn list_sources(&self, _: Parameters<super::NoArgs>) -> Result<Json<Vec<SourceSummary>>, ErrorData> {
        self.api
            .get::<Vec<SourceSummary>>("/api/sources")
            .await
            .map(Json)
            .map_err(|e| e.into_error_data())
    }

    #[tool(name = "get_source", description = "Get a source by id. Secret token is redacted in the response.")]
    pub async fn get_source(
        &self,
        Parameters(a): Parameters<IdArgs>,
    ) -> Result<Json<SourceSummary>, ErrorData> {
        self.api
            .get::<SourceSummary>(&format!("/api/sources/{}", a.id))
            .await
            .map(Json)
            .map_err(|e| e.into_error_data())
    }

    #[tool(name = "create_source", description = "Create a new webhook source. Side effect: registers a new endpoint that *arr instances can hit. `kind` is one of `radarr|sonarr|lidarr|generic`; `secret_token` is what the *arr will use for Bearer or Basic auth.")]
    pub async fn create_source(
        &self,
        Parameters(a): Parameters<CreateSourceReq>,
    ) -> Result<Json<CreatedIdResp>, ErrorData> {
        self.api
            .post::<CreatedIdResp, _>("/api/sources", &a)
            .await
            .map(Json)
            .map_err(|e| e.into_error_data())
    }

    #[tool(name = "update_source", description = "Patch fields on an existing source. Side effect: changes incoming-webhook auth or routing. Omitted fields are unchanged. Sending `\"***\"` for secret_token is treated as 'unchanged'.")]
    pub async fn update_source(
        &self,
        Parameters(a): Parameters<UpdateArgs>,
    ) -> Result<Json<serde_json::Value>, ErrorData> {
        self.api
            .put::<serde_json::Value, _>(&format!("/api/sources/{}", a.id), &a.patch)
            .await
            .map_err(|e| e.into_error_data())?;
        Ok(Json(serde_json::json!({"updated": true, "id": a.id})))
    }

    #[tool(name = "delete_source", description = "Destructive: permanently delete a source. Subsequent webhooks for this source will be rejected with 401. Cannot be undone. Requires confirm=true.")]
    pub async fn delete_source(
        &self,
        Parameters(a): Parameters<DeleteSourceArgs>,
    ) -> Result<Json<serde_json::Value>, ErrorData> {
        if !a.confirm {
            return Err(ErrorData {
                code: ErrorCode::INVALID_PARAMS,
                message: "delete_source requires `confirm: true`".into(),
                data: None,
            });
        }
        self.api
            .delete::<serde_json::Value>(&format!("/api/sources/{}", a.id))
            .await
            .map_err(|e| e.into_error_data())?;
        Ok(Json(serde_json::json!({"deleted": true, "id": a.id})))
    }
}
