use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
