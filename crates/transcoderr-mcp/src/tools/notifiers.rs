use crate::Server;
use rmcp::{
    handler::server::{tool::Parameters, wrapper::Json},
    model::{ErrorCode, ErrorData},
    tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::future::Future; // pulled in by #[tool_router] macro expansion
use transcoderr_api_types::{CreatedIdResp, NotifierReq, NotifierSummary};

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct IdArgs {
    pub id: i64,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct UpdateArgs {
    pub id: i64,
    #[serde(flatten)]
    pub body: NotifierReq,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct DeleteNotifierArgs {
    pub id: i64,
    /// Required confirmation. Reject the call by setting this to false.
    pub confirm: bool,
}

#[tool_router(router = notifiers_router, vis = "pub")]
impl Server {
    #[tool(name = "list_notifiers", description = "List notifier channels (discord/jellyfin/ntfy/telegram/webhook). Secret-bearing config keys are redacted to `***` for token-authed callers.")]
    pub async fn list_notifiers(&self, _: Parameters<super::NoArgs>) -> Result<Json<Vec<NotifierSummary>>, ErrorData> {
        self.api
            .get::<Vec<NotifierSummary>>("/api/notifiers")
            .await
            .map(Json)
            .map_err(|e| e.into_error_data())
    }

    #[tool(name = "get_notifier", description = "Get a notifier by id. Secret-bearing config keys are redacted in the response.")]
    pub async fn get_notifier(
        &self,
        Parameters(a): Parameters<IdArgs>,
    ) -> Result<Json<NotifierSummary>, ErrorData> {
        self.api
            .get::<NotifierSummary>(&format!("/api/notifiers/{}", a.id))
            .await
            .map(Json)
            .map_err(|e| e.into_error_data())
    }

    #[tool(name = "create_notifier", description = "Create a new notifier channel. Side effect: registers a new outbound notification target. `kind` is one of `discord|jellyfin|ntfy|telegram|webhook`; `config` shape depends on kind (e.g. telegram needs `bot_token` + `chat_id`; ntfy needs `server` + `topic`; jellyfin needs `url` + `api_key` and triggers a per-file rescan instead of sending a chat message).")]
    pub async fn create_notifier(
        &self,
        Parameters(a): Parameters<NotifierReq>,
    ) -> Result<Json<CreatedIdResp>, ErrorData> {
        self.api
            .post::<CreatedIdResp, _>("/api/notifiers", &a)
            .await
            .map(Json)
            .map_err(|e| e.into_error_data())
    }

    #[tool(name = "update_notifier", description = "Replace fields on an existing notifier. Side effect: changes outbound delivery for this channel. Sending `\"***\"` for any secret-bearing config key is treated as 'unchanged'.")]
    pub async fn update_notifier(
        &self,
        Parameters(a): Parameters<UpdateArgs>,
    ) -> Result<Json<serde_json::Value>, ErrorData> {
        self.api
            .put::<serde_json::Value, _>(&format!("/api/notifiers/{}", a.id), &a.body)
            .await
            .map_err(|e| e.into_error_data())?;
        Ok(Json(serde_json::json!({"updated": true, "id": a.id})))
    }

    #[tool(name = "delete_notifier", description = "Destructive: permanently delete a notifier channel. Flows that reference it will fail to deliver. Cannot be undone. Requires confirm=true.")]
    pub async fn delete_notifier(
        &self,
        Parameters(a): Parameters<DeleteNotifierArgs>,
    ) -> Result<Json<serde_json::Value>, ErrorData> {
        if !a.confirm {
            return Err(ErrorData {
                code: ErrorCode::INVALID_PARAMS,
                message: "delete_notifier requires `confirm: true`".into(),
                data: None,
            });
        }
        self.api
            .delete::<serde_json::Value>(&format!("/api/notifiers/{}", a.id))
            .await
            .map_err(|e| e.into_error_data())?;
        Ok(Json(serde_json::json!({"deleted": true, "id": a.id})))
    }

    #[tool(name = "test_notifier", description = "Send a test notification through this channel. Side effect: actually delivers a message via the configured kind (discord, telegram, etc.).")]
    pub async fn test_notifier(
        &self,
        Parameters(a): Parameters<IdArgs>,
    ) -> Result<Json<serde_json::Value>, ErrorData> {
        self.api
            .post::<serde_json::Value, _>(&format!("/api/notifiers/{}/test", a.id), &serde_json::Value::Null)
            .await
            .map_err(|e| e.into_error_data())?;
        Ok(Json(serde_json::json!({"sent": true, "id": a.id})))
    }
}
