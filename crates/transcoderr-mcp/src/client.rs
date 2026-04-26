use anyhow::{Context, Result};
use reqwest::{Client, Method, StatusCode};
use serde::{de::DeserializeOwned, Serialize};
use transcoderr_api_types::ApiError;

/// Thin reqwest wrapper that always sets the bearer header, deserializes
/// `ApiError` on failure, and returns it as a richly-mapped `McpHttpError`.
#[derive(Clone)]
pub struct ApiClient {
    base: String,
    token: String,
    http: Client,
}

#[derive(Debug, thiserror::Error)]
pub enum McpHttpError {
    #[error("could not connect to {0}: {1}")]
    Unreachable(String, String),
    #[error("auth failed — check TRANSCODERR_TOKEN")]
    AuthFailed,
    #[error("forbidden")]
    Forbidden,
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("invalid params: {0}")]
    InvalidParams(String),
    #[error("server error: {0}")]
    Internal(String),
    #[error("unexpected: {0}")]
    Other(String),
}

impl McpHttpError {
    pub fn into_error_data(self) -> rmcp::model::ErrorData {
        use rmcp::model::{ErrorCode, ErrorData};
        let code = match self {
            McpHttpError::Unreachable(..) | McpHttpError::Internal(_) => ErrorCode::INTERNAL_ERROR,
            McpHttpError::AuthFailed | McpHttpError::Forbidden => ErrorCode(-32001),
            McpHttpError::NotFound(_) => ErrorCode(-32004),
            McpHttpError::InvalidParams(_) => ErrorCode::INVALID_PARAMS,
            McpHttpError::Conflict(_) => ErrorCode(-32009),
            McpHttpError::Other(_) => ErrorCode::INTERNAL_ERROR,
        };
        ErrorData { code, message: self.to_string().into(), data: None }
    }
}

impl ApiClient {
    pub fn new(base: String, token: String, timeout_secs: u64) -> Result<Self> {
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .build()
            .context("build reqwest client")?;
        Ok(Self { base: base.trim_end_matches('/').to_string(), token, http })
    }

    pub async fn request<R, B>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<R, McpHttpError>
    where
        R: DeserializeOwned,
        B: Serialize + ?Sized,
    {
        let url = format!("{}{}", self.base, path);
        let mut req = self.http.request(method, &url).bearer_auth(&self.token);
        if let Some(b) = body { req = req.json(b); }
        let resp = req.send().await
            .map_err(|e| McpHttpError::Unreachable(url.clone(), e.to_string()))?;
        let status = resp.status();
        if status.is_success() {
            // 204 → empty body. Deserialize via serde_json::Value::Null fallback.
            if status == StatusCode::NO_CONTENT {
                return serde_json::from_value(serde_json::Value::Null)
                    .map_err(|e| McpHttpError::Other(format!("decode 204: {e}")));
            }
            let txt = resp.text().await.map_err(|e| McpHttpError::Other(e.to_string()))?;
            return serde_json::from_str(&txt)
                .map_err(|e| McpHttpError::Other(format!("decode response: {e} (body: {txt})")));
        }
        let body_txt = resp.text().await.unwrap_or_default();
        let parsed: Option<ApiError> = serde_json::from_str(&body_txt).ok();
        let msg = parsed.map(|p| p.message).unwrap_or_else(|| body_txt.clone());
        Err(match status {
            StatusCode::UNAUTHORIZED => McpHttpError::AuthFailed,
            StatusCode::FORBIDDEN => McpHttpError::Forbidden,
            StatusCode::NOT_FOUND => McpHttpError::NotFound(msg),
            StatusCode::CONFLICT => McpHttpError::Conflict(msg),
            StatusCode::BAD_REQUEST => McpHttpError::InvalidParams(msg),
            s if s.is_server_error() => McpHttpError::Internal(msg),
            s => McpHttpError::Other(format!("{s}: {msg}")),
        })
    }

    pub async fn get<R: DeserializeOwned>(&self, path: &str) -> Result<R, McpHttpError> {
        self.request::<R, ()>(Method::GET, path, None).await
    }
    pub async fn post<R: DeserializeOwned, B: Serialize + ?Sized>(
        &self, path: &str, body: &B,
    ) -> Result<R, McpHttpError> {
        self.request::<R, B>(Method::POST, path, Some(body)).await
    }
    pub async fn put<R: DeserializeOwned, B: Serialize + ?Sized>(
        &self, path: &str, body: &B,
    ) -> Result<R, McpHttpError> {
        self.request::<R, B>(Method::PUT, path, Some(body)).await
    }
    pub async fn delete<R: DeserializeOwned>(&self, path: &str) -> Result<R, McpHttpError> {
        self.request::<R, ()>(Method::DELETE, path, None).await
    }

    /// Pass-through used by `get_metrics` — server returns Prometheus text, not JSON.
    pub async fn get_text(&self, path: &str) -> Result<String, McpHttpError> {
        let url = format!("{}{}", self.base, path);
        let resp = self.http.get(&url).bearer_auth(&self.token).send().await
            .map_err(|e| McpHttpError::Unreachable(url, e.to_string()))?;
        if !resp.status().is_success() {
            return Err(McpHttpError::Other(format!("{}: {}", resp.status(), resp.text().await.unwrap_or_default())));
        }
        resp.text().await.map_err(|e| McpHttpError::Other(e.to_string()))
    }

    /// Validate URL reachability AND token authority by hitting an
    /// auth-gated endpoint. Used at startup to fail-fast on misconfig.
    pub async fn probe(&self) -> Result<(), McpHttpError> {
        // /api/settings is auth-gated, side-effect-free, and cheap.
        let _: serde_json::Value = self.get("/api/settings").await?;
        Ok(())
    }
}
