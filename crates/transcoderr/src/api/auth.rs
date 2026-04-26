use crate::{db, http::AppState};
use argon2::{password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString}, Argon2};
use axum::{extract::{Path, State}, http::StatusCode, Json};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use tower_cookies::{Cookie, Cookies};
use transcoderr_api_types::{ApiTokenSummary, CreateTokenReq, CreateTokenResp};

#[derive(Deserialize)]
pub struct LoginReq { pub password: String }

#[derive(Serialize)]
pub struct MeResp { pub auth_required: bool, pub authed: bool }

pub async fn login(
    State(state): State<AppState>,
    cookies: Cookies,
    Json(req): Json<LoginReq>,
) -> Result<StatusCode, StatusCode> {
    let enabled = db::settings::get(&state.pool, "auth.enabled").await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .unwrap_or_default() == "true";
    if !enabled { return Ok(StatusCode::NO_CONTENT); }
    let stored = db::settings::get(&state.pool, "auth.password_hash").await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?.unwrap_or_default();
    if stored.is_empty() { return Err(StatusCode::UNAUTHORIZED); }
    let parsed = PasswordHash::new(&stored).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Argon2::default().verify_password(req.password.as_bytes(), &parsed)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp();
    let expires = now + 60*60*24*30;
    sqlx::query("INSERT INTO sessions (id, created_at, expires_at) VALUES (?, ?, ?)")
        .bind(&id).bind(now).bind(expires)
        .execute(&state.pool).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let cookie = Cookie::build(("transcoderr_sid", id))
        .http_only(true).path("/").max_age(time::Duration::days(30)).build();
    cookies.add(cookie);
    Ok(StatusCode::NO_CONTENT)
}

pub async fn logout(State(state): State<AppState>, cookies: Cookies) -> StatusCode {
    if let Some(c) = cookies.get("transcoderr_sid") {
        let _ = sqlx::query("DELETE FROM sessions WHERE id = ?").bind(c.value()).execute(&state.pool).await;
        cookies.remove(Cookie::from("transcoderr_sid"));
    }
    StatusCode::NO_CONTENT
}

pub async fn me(State(state): State<AppState>, cookies: Cookies) -> Json<MeResp> {
    let enabled = db::settings::get(&state.pool, "auth.enabled").await
        .ok().flatten().unwrap_or_default() == "true";
    let authed = if !enabled { true } else {
        match cookies.get("transcoderr_sid") {
            Some(c) => session_valid(&state.pool, c.value()).await.unwrap_or(false),
            None => false,
        }
    };
    Json(MeResp { auth_required: enabled, authed })
}

async fn session_valid(pool: &sqlx::SqlitePool, sid: &str) -> anyhow::Result<bool> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT expires_at FROM sessions WHERE id = ?")
        .bind(sid).fetch_optional(pool).await?;
    Ok(matches!(row, Some((e,)) if e > chrono::Utc::now().timestamp()))
}

pub async fn require_auth(
    State(state): State<AppState>,
    cookies: Cookies,
    request: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, StatusCode> {
    let enabled = db::settings::get(&state.pool, "auth.enabled").await
        .ok().flatten().unwrap_or_default() == "true";
    if !enabled { return Ok(next.run(request).await); }
    let sid = cookies.get("transcoderr_sid").ok_or(StatusCode::UNAUTHORIZED)?;
    if !session_valid(&state.pool, sid.value()).await.unwrap_or(false) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(next.run(request).await)
}

pub fn hash_password(p: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Ok(Argon2::default().hash_password(p.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("hash: {e}"))?
        .to_string())
}

pub async fn list_tokens(
    State(state): State<AppState>,
) -> Result<Json<Vec<ApiTokenSummary>>, StatusCode> {
    db::api_tokens::list(&state.pool)
        .await
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub async fn create_token(
    State(state): State<AppState>,
    Json(req): Json<CreateTokenReq>,
) -> Result<Json<CreateTokenResp>, StatusCode> {
    if req.name.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let made = db::api_tokens::create(&state.pool, req.name.trim())
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(CreateTokenResp { id: made.id, token: made.token }))
}

pub async fn delete_token(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    let removed = db::api_tokens::delete(&state.pool, id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if removed { Ok(StatusCode::NO_CONTENT) } else { Err(StatusCode::NOT_FOUND) }
}
