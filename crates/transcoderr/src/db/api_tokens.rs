use crate::api::auth::hash_password;
use anyhow::Context;
use argon2::{password_hash::PasswordHash, Argon2, PasswordVerifier};
use rand::distributions::{Alphanumeric, DistString};
use sqlx::{Row, SqlitePool};
use transcoderr_api_types::ApiTokenSummary;

const TOKEN_PREFIX: &str = "tcr_";
const RANDOM_LEN: usize = 32;
const PREFIX_LEN: usize = TOKEN_PREFIX.len() + 8; // "tcr_" + 8 random chars

pub struct CreatedToken {
    pub id: i64,
    pub token: String, // shown to user once
}

pub fn mint_token() -> String {
    let body = Alphanumeric.sample_string(&mut rand::thread_rng(), RANDOM_LEN);
    format!("{TOKEN_PREFIX}{body}")
}

pub async fn create(pool: &SqlitePool, name: &str) -> anyhow::Result<CreatedToken> {
    let token = mint_token();
    let hash = hash_password(&token).context("argon2 hash failed")?;
    let prefix = &token[..PREFIX_LEN];
    let now = crate::db::now_unix();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO api_tokens (name, hash, prefix, created_at) VALUES (?, ?, ?, ?) RETURNING id",
    )
    .bind(name)
    .bind(&hash)
    .bind(prefix)
    .bind(now)
    .fetch_one(pool)
    .await?;
    Ok(CreatedToken { id, token })
}

pub async fn list(pool: &SqlitePool) -> anyhow::Result<Vec<ApiTokenSummary>> {
    let rows = sqlx::query(
        "SELECT id, name, prefix, created_at, last_used_at FROM api_tokens ORDER BY id DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| ApiTokenSummary {
            id: r.get(0),
            name: r.get(1),
            prefix: r.get(2),
            created_at: r.get(3),
            last_used_at: r.get(4),
        })
        .collect())
}

pub async fn delete(pool: &SqlitePool, id: i64) -> anyhow::Result<bool> {
    let n = sqlx::query("DELETE FROM api_tokens WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(n > 0)
}

/// Look up by prefix and verify with argon2. Returns the token id on success.
/// On success, kicks off a fire-and-forget update of `last_used_at`.
pub async fn verify(pool: &SqlitePool, presented: &str) -> Option<i64> {
    if !presented.starts_with(TOKEN_PREFIX) || presented.len() < PREFIX_LEN {
        return None;
    }
    let prefix = &presented[..PREFIX_LEN];
    let row: Option<(i64, String)> = sqlx::query_as(
        "SELECT id, hash FROM api_tokens WHERE prefix = ?",
    )
    .bind(prefix)
    .fetch_optional(pool)
    .await
    .ok()?;
    let (id, hash) = row?;
    let parsed = PasswordHash::new(&hash).ok()?;
    Argon2::default()
        .verify_password(presented.as_bytes(), &parsed)
        .ok()?;
    let pool2 = pool.clone();
    tokio::spawn(async move {
        let _ = sqlx::query("UPDATE api_tokens SET last_used_at = strftime('%s','now') WHERE id = ?")
            .bind(id)
            .execute(&pool2)
            .await;
    });
    Some(id)
}
