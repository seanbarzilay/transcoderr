use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

pub mod logging;

/// `schema_with` helper for fields whose Rust type is `serde_json::Value`
/// but whose schema must be a typed object. schemars defaults `Value` to
/// `Schema::Bool(true)` ("any"), which Claude Code's Zod-based MCP tool
/// validator rejects with `Invalid input: expected "object"`, droppingdasdasdasds
/// the entire server's tool list.
pub fn json_object_schema(_g: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": "object",
        "additionalProperties": true
    })
}

/// As [`json_object_schema`] but for `Option<serde_json::Value>` fields.
pub fn optional_json_object_schema(_g: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": "object",
        "additionalProperties": true,
        "nullable": true
    })
}

/// Stable error wire format. The HTTP API returns this body on failures;
/// the MCP binary deserializes it and maps to ToolError.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ApiError {
    /// Machine-readable code, e.g. `flow.not_found`, `validation.bad_request`.
    pub code: String,
    /// Human-readable single-sentence description.
    pub message: String,
    /// Optional structured details.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl ApiError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self { code: code.into(), message: message.into(), details: None }
    }
}

// ─── Runs ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RunSummary {
    pub id: i64,
    pub flow_id: i64,
    pub status: String,
    pub created_at: i64,
    pub finished_at: Option<i64>,
    pub file_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RunEvent {
    pub id: i64,
    pub job_id: i64,
    pub ts: i64,
    pub step_id: Option<String>,
    pub kind: String,
    pub payload: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RunDetail {
    pub run: RunSummary,
    pub events: Vec<RunEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RerunResp {
    pub id: i64,
}

// ─── Flows ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FlowSummary {
    pub id: i64,
    pub name: String,
    pub enabled: bool,
    pub version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FlowDetail {
    pub id: i64,
    pub name: String,
    pub enabled: bool,
    pub version: i64,
    pub yaml_source: String,
    pub parsed_json: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateFlowReq {
    pub name: String,
    pub yaml: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UpdateFlowReq {
    pub yaml: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

// ─── Sources ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SourceSummary {
    pub id: i64,
    pub kind: String,
    pub name: String,
    pub config: serde_json::Value,
    /// `"***"` when the request was authenticated by API token.
    /// The cleartext token is returned only to UI session callers.
    pub secret_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateSourceReq {
    pub kind: String,
    pub name: String,
    #[schemars(schema_with = "json_object_schema")]
    pub config: serde_json::Value,
    pub secret_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UpdateSourceReq {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "optional_json_object_schema")]
    pub config: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_token: Option<String>,
}

// ─── Notifiers ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NotifierSummary {
    pub id: i64,
    pub name: String,
    pub kind: String,
    /// Secret-bearing keys (e.g. `bot_token`, `url`, `topic`) are replaced
    /// with `"***"` for token-authed callers.
    pub config: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NotifierReq {
    pub name: String,
    pub kind: String,
    #[schemars(schema_with = "json_object_schema")]
    pub config: serde_json::Value,
}

// ─── Tokens ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ApiTokenSummary {
    pub id: i64,
    pub name: String,
    pub prefix: String,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateTokenReq {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateTokenResp {
    pub id: i64,
    pub token: String,
}

// ─── Misc ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreatedIdResp {
    pub id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Health {
    pub healthy: bool,
    pub ready: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_error_round_trips_through_json() {
        let e = ApiError::new("flow.not_found", "flow 7 does not exist");
        let s = serde_json::to_string(&e).unwrap();
        let back: ApiError = serde_json::from_str(&s).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn api_error_omits_null_details() {
        let e = ApiError::new("x", "y");
        let s = serde_json::to_string(&e).unwrap();
        assert!(!s.contains("details"), "got {s}");
    }
}
