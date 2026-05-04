use crate::{
    db, flow::expr, flow::Context, http::auth_extract, http::dedup::DedupCache, http::AppState,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Extension, Json,
};
use serde_json::Value;
use std::sync::Arc;

pub async fn handle(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Extension(dedup): Extension<Arc<DedupCache>>,
    headers: HeaderMap,
    raw: Json<Value>,
) -> Result<StatusCode, StatusCode> {
    let token = auth_extract::extract_token(&headers);
    let source = db::sources::get_webhook_by_name_and_token(&state.pool, &name, &token)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let cfg: Value = serde_json::from_str(&source.config_json).unwrap_or(Value::Null);
    let path_expr = cfg["path_expr"].as_str().unwrap_or("steps.payload.path");

    // Bind payload under steps so CEL can access it as steps.payload.*
    let mut ctx = Context::for_file("");
    ctx.steps.insert("payload".into(), raw.0.clone());

    let path = expr::eval_string_template(&format!("{{{{ {path_expr} }}}}"), &ctx)
        .map_err(|_| StatusCode::BAD_REQUEST)?;

    let raw_str = serde_json::to_string(&raw.0).unwrap_or_default();
    if !dedup.observe(source.id, &path, &raw_str) {
        return Ok(StatusCode::ACCEPTED);
    }
    let flows = db::flows::list_enabled_for_webhook(&state.pool, &name)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    for flow in flows {
        let _ = db::jobs::insert_with_source(
            &state.pool,
            flow.id,
            flow.version,
            source.id,
            "webhook",
            &path,
            &raw_str,
        )
        .await;
    }
    Ok(StatusCode::ACCEPTED)
}
