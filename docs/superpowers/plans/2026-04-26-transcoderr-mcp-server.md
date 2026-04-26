# transcoderr MCP server — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a stdio MCP server (`transcoderr-mcp`) that lets AI clients drive transcoderr's read & write surface, plus the API-token system the server side needs to authenticate it.

**Architecture:** Convert the repo to a Cargo workspace with three crates: `transcoderr` (existing server), `transcoderr-api-types` (shared serde + JsonSchema types), and `transcoderr-mcp` (stdio binary that proxies tool calls over HTTPS to the server). The MCP binary is a stateless proxy; the server is authoritative.

**Tech Stack:** Rust workspace; `rmcp` SDK with `transport-io` + `server` + `macros` features; `schemars` for JSON schemas; `reqwest` (already in deps) for HTTP; existing `argon2` for token hashing.

---

## File map

**Created:**
- `crates/transcoderr-api-types/Cargo.toml`
- `crates/transcoderr-api-types/src/lib.rs`
- `crates/transcoderr-mcp/Cargo.toml`
- `crates/transcoderr-mcp/src/main.rs`
- `crates/transcoderr-mcp/src/client.rs`
- `crates/transcoderr-mcp/src/tools/mod.rs`
- `crates/transcoderr-mcp/src/tools/{runs,flows,sources,notifiers,system}.rs`
- `crates/transcoderr-mcp/tests/mcp_stdio_e2e.rs`
- `crates/transcoderr/migrations/20260426000001_api_tokens.sql`
- `crates/transcoderr/src/db/api_tokens.rs`
- `web/src/components/api-tokens-card.tsx`
- `docs/mcp.md`

**Moved (Task 1):**
- `Cargo.toml` → `crates/transcoderr/Cargo.toml` (workspace root gets a new `Cargo.toml`)
- `src/`, `tests/`, `migrations/` → `crates/transcoderr/{src,tests,migrations}`

**Modified:**
- `Cargo.toml` (new workspace root)
- `crates/transcoderr/src/static_assets.rs` (include_dir path)
- `crates/transcoderr/src/api/{auth,sources,notifiers,runs,flows,mod}.rs`
- `crates/transcoderr/src/db/mod.rs`
- `crates/transcoderr/src/lib.rs`
- `web/src/pages/settings.tsx`
- `web/src/api/client.ts`
- `web/src/types.ts`
- `.github/workflows/release.yml`
- `README.md`

---

## Task 1: Convert to Cargo workspace

**Files:**
- Create: `Cargo.toml` (new root)
- Move: `Cargo.toml` → `crates/transcoderr/Cargo.toml`
- Move: `src/`, `tests/`, `migrations/` → `crates/transcoderr/{src,tests,migrations}`
- Modify: `crates/transcoderr/src/static_assets.rs:8`

- [ ] **Step 1: Create the directory and move the crate**

```bash
mkdir -p crates/transcoderr
git mv Cargo.toml crates/transcoderr/Cargo.toml
git mv src crates/transcoderr/src
git mv tests crates/transcoderr/tests
git mv migrations crates/transcoderr/migrations
```

- [ ] **Step 2: Write the new workspace root `Cargo.toml`**

Create `Cargo.toml` (at repo root):

```toml
[workspace]
resolver = "2"
members = [
  "crates/transcoderr",
  "crates/transcoderr-api-types",
  "crates/transcoderr-mcp",
]

[workspace.package]
version = "0.7.0"
edition = "2021"
rust-version = "1.78"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
schemars = "0.8"
chrono = { version = "0.4", features = ["serde"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread", "io-util"] }
anyhow = "1"
thiserror = "1"
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

[profile.release]
lto = "thin"
strip = true
codegen-units = 1
```

- [ ] **Step 3: Update the existing `transcoderr` crate manifest to use workspace settings**

Edit `crates/transcoderr/Cargo.toml`:

```toml
[package]
name = "transcoderr"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[dependencies]
tokio = { version = "1", features = ["macros", "rt-multi-thread", "process", "signal", "fs", "time", "sync"] }
axum = { version = "0.7", features = ["macros"] }
tower = "0.5"
tower-http = { version = "0.6", features = ["trace"] }
sqlx = { version = "0.8", default-features = false, features = ["runtime-tokio", "sqlite", "migrate", "chrono", "macros"] }
libsqlite3-sys = { version = "0.30", features = ["bundled"] }
serde = { workspace = true }
serde_json = { workspace = true }
serde_yaml = "0.9"
toml = "0.8"
thiserror = { workspace = true }
anyhow = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true, features = ["env-filter", "json"] }
clap = { version = "4", features = ["derive"] }
chrono = { workspace = true }
async-trait = "0.1"
cel-interpreter = "0.10"
reqwest = { workspace = true, features = ["json", "rustls-tls", "cookies", "stream"] }
tokio-stream = { version = "0.1", features = ["sync"] }
tokio-util = { version = "0.7", default-features = false }
futures = "0.3"
argon2 = "0.5"
base64 = "0.22"
rand = "0.8"
tower-cookies = "0.10"
time = "0.3"
uuid = { version = "1", features = ["v4"] }
include_dir = "0.7"
mime_guess = "2"
metrics = "0.23"
metrics-exporter-prometheus = "0.15"

[dev-dependencies]
tempfile = "3"
pretty_assertions = "1"
serial_test = "3"

[lib]
name = "transcoderr"
path = "src/lib.rs"

[[bin]]
name = "transcoderr"
path = "src/main.rs"
```

- [ ] **Step 4: Fix the embedded SPA path now that the manifest moved**

Edit `crates/transcoderr/src/static_assets.rs:8` from:

```rust
static DIST: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/web/dist");
```

to:

```rust
static DIST: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../web/dist");
```

- [ ] **Step 5: Build the SPA so include_dir has something to embed**

Run: `npm --prefix web ci && npm --prefix web run build`
Expected: `web/dist/` is populated with `index.html` and assets.

- [ ] **Step 6: Verify the workspace builds**

Run: `cargo build --workspace`
Expected: success. (Only the `transcoderr` crate exists in the workspace at this point — the other two are added in later tasks. If cargo complains about missing members, comment them out of `members` for now and re-add in Task 2 / Task 9.)

To keep cargo happy: temporarily change root `Cargo.toml`'s `members` to `["crates/transcoderr"]` for this task. Restore the full list in Task 2.

- [ ] **Step 7: Run the existing test suite to confirm no regression**

Run: `cargo test --workspace`
Expected: all tests pass (same count as before the move).

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "refactor: convert to Cargo workspace, move crate under crates/transcoderr"
```

---

## Task 2: Create the `transcoderr-api-types` crate with `ApiError`

**Files:**
- Create: `crates/transcoderr-api-types/Cargo.toml`
- Create: `crates/transcoderr-api-types/src/lib.rs`
- Modify: `Cargo.toml` (workspace `members`)

- [ ] **Step 1: Re-add the api-types member in the workspace root `Cargo.toml`**

Update `members` in root `Cargo.toml`:

```toml
members = [
  "crates/transcoderr",
  "crates/transcoderr-api-types",
]
```

- [ ] **Step 2: Write the api-types `Cargo.toml`**

Create `crates/transcoderr-api-types/Cargo.toml`:

```toml
[package]
name = "transcoderr-api-types"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
schemars = { workspace = true }
chrono = { workspace = true }
```

- [ ] **Step 3: Write the failing test**

Create `crates/transcoderr-api-types/src/lib.rs`:

```rust
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
```

- [ ] **Step 4: Run the test**

Run: `cargo test -p transcoderr-api-types`
Expected: 2 tests pass.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/transcoderr-api-types
git commit -m "feat(api-types): new crate with shared ApiError"
```

---

## Task 3: Move shared response/request types to `transcoderr-api-types`

**Why:** Both the server (serializing) and the MCP binary (deserializing + JsonSchema) need the same struct definitions. Single source of truth.

**Files:**
- Modify: `crates/transcoderr-api-types/src/lib.rs`
- Modify: `crates/transcoderr/Cargo.toml` (add dep)
- Modify: `crates/transcoderr/src/api/{runs,flows,sources,notifiers}.rs`

- [ ] **Step 1: Add dep on api-types in the server crate**

Append to `crates/transcoderr/Cargo.toml` `[dependencies]`:

```toml
transcoderr-api-types = { path = "../transcoderr-api-types" }
```

- [ ] **Step 2: Add the response/request types to api-types**

Append to `crates/transcoderr-api-types/src/lib.rs` (after `ApiError`):

```rust
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
    pub config: serde_json::Value,
    pub secret_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UpdateSourceReq {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    /// Secret-bearing keys (e.g. `bot_token`, `webhook_url`) are replaced
    /// with `"***"` for token-authed callers.
    pub config: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NotifierReq {
    pub name: String,
    pub kind: String,
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
```

- [ ] **Step 3: Update server handlers to use the shared types**

In `crates/transcoderr/src/api/runs.rs`, delete the local `RunSummary`, `RunEvent`, `RunDetail`, `RerunResp` struct definitions (lines ~10–56) and add at the top:

```rust
use transcoderr_api_types::{RunDetail, RunEvent, RunSummary, RerunResp};
```

Leave `ListParams` and `EventsParams` as local — they are query-string-only, never deserialized client-side from MCP.

- [ ] **Step 4: Same for flows.rs**

In `crates/transcoderr/src/api/flows.rs` delete `FlowSummary`, `FlowDetail`, `CreateFlowReq`, `UpdateFlowReq` definitions and add:

```rust
use transcoderr_api_types::{CreateFlowReq, FlowDetail, FlowSummary, UpdateFlowReq};
```

- [ ] **Step 5: Same for sources.rs**

In `crates/transcoderr/src/api/sources.rs` delete `SourceSummary`, `CreateSourceReq`, `UpdateSourceReq`, `CreateResp` and add:

```rust
use transcoderr_api_types::{CreatedIdResp as CreateResp, CreateSourceReq, SourceSummary, UpdateSourceReq};
```

Adjust `list` and `get` to populate the new `secret_token` field. Replace the body of `list` with:

```rust
pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<SourceSummary>>, StatusCode> {
    let rows = sqlx::query("SELECT id, kind, name, config_json, secret_token FROM sources ORDER BY name")
        .fetch_all(&state.pool).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let out = rows.into_iter().map(|r| {
        let config_str: String = r.get(3);
        SourceSummary {
            id: r.get(0),
            kind: r.get(1),
            name: r.get(2),
            config: serde_json::from_str(&config_str).unwrap_or_default(),
            secret_token: r.get(4),
        }
    }).collect();
    Ok(Json(out))
}
```

And `get`:

```rust
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<SourceSummary>, StatusCode> {
    let row = sqlx::query("SELECT id, kind, name, config_json, secret_token FROM sources WHERE id = ?")
        .bind(id).fetch_optional(&state.pool).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let config_str: String = row.get(3);
    Ok(Json(SourceSummary {
        id: row.get(0),
        kind: row.get(1),
        name: row.get(2),
        config: serde_json::from_str(&config_str).unwrap_or_default(),
        secret_token: row.get(4),
    }))
}
```

(Redaction is wired in Task 7.)

- [ ] **Step 6: Same for notifiers.rs**

In `crates/transcoderr/src/api/notifiers.rs` delete `NotifierSummary`, `NotifierReq`, `CreateResp` and add:

```rust
use transcoderr_api_types::{CreatedIdResp as CreateResp, NotifierReq, NotifierSummary};
```

- [ ] **Step 7: Confirm everything compiles and tests still pass**

Run: `cargo test --workspace`
Expected: all existing tests still pass; no warnings about unused imports.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "refactor: move shared API types to transcoderr-api-types crate"
```

---

## Task 4: Add the `api_tokens` migration and DB module

**Files:**
- Create: `crates/transcoderr/migrations/20260426000001_api_tokens.sql`
- Create: `crates/transcoderr/src/db/api_tokens.rs`
- Modify: `crates/transcoderr/src/db/mod.rs`

- [ ] **Step 1: Write the migration**

Create `crates/transcoderr/migrations/20260426000001_api_tokens.sql`:

```sql
CREATE TABLE api_tokens (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  name         TEXT NOT NULL,
  hash         TEXT NOT NULL,
  prefix       TEXT NOT NULL,
  created_at   INTEGER NOT NULL,
  last_used_at INTEGER
);
CREATE UNIQUE INDEX api_tokens_prefix_idx ON api_tokens(prefix);
```

- [ ] **Step 2: Write the failing test**

Append to `crates/transcoderr/tests/api_auth.rs`:

```rust
#[tokio::test]
async fn api_tokens_table_round_trips() {
    let app = boot().await;
    sqlx::query("INSERT INTO api_tokens (name, hash, prefix, created_at) VALUES (?, ?, ?, ?)")
        .bind("claude-desktop")
        .bind("$argon2id$dummy")
        .bind("tcr_a1b2c3d4")
        .bind(123_i64)
        .execute(&app.pool).await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM api_tokens")
        .fetch_one(&app.pool).await.unwrap();
    assert_eq!(n, 1);
}
```

- [ ] **Step 3: Run the test**

Run: `cargo test -p transcoderr --test api_auth api_tokens_table_round_trips`
Expected: PASS (the migration auto-applies via `db::open`).

- [ ] **Step 4: Write the db module**

Create `crates/transcoderr/src/db/api_tokens.rs`:

```rust
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
```

- [ ] **Step 5: Register the new module**

Append to `crates/transcoderr/src/db/mod.rs` (with the other `pub mod` lines):

```rust
pub mod api_tokens;
```

- [ ] **Step 6: Write a unit test for create + verify + delete**

Append to `crates/transcoderr/tests/api_auth.rs`:

```rust
#[tokio::test]
async fn api_token_create_verify_delete_round_trip() {
    use transcoderr::db::api_tokens;
    let app = boot().await;

    let made = api_tokens::create(&app.pool, "claude-desktop").await.unwrap();
    assert!(made.token.starts_with("tcr_"));
    assert_eq!(made.token.len(), 4 + 32);

    let id = api_tokens::verify(&app.pool, &made.token).await.expect("verify");
    assert_eq!(id, made.id);

    let bad = api_tokens::verify(&app.pool, "tcr_NOTREAL000000000000000000000000").await;
    assert!(bad.is_none());

    let listed = api_tokens::list(&app.pool).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].prefix.len(), 12);

    let removed = api_tokens::delete(&app.pool, made.id).await.unwrap();
    assert!(removed);

    let after = api_tokens::verify(&app.pool, &made.token).await;
    assert!(after.is_none());
}
```

- [ ] **Step 7: Run**

Run: `cargo test -p transcoderr --test api_auth`
Expected: 3 tests pass (the existing `login_with_correct_password_succeeds` + 2 new).

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(server): api_tokens table + db module with create/verify/list/delete"
```

---

## Task 5: Add token CRUD endpoints

**Files:**
- Modify: `crates/transcoderr/src/api/auth.rs`
- Modify: `crates/transcoderr/src/api/mod.rs`

- [ ] **Step 1: Write a failing integration test**

Append to `crates/transcoderr/tests/api_auth.rs`:

```rust
#[tokio::test]
async fn token_endpoints_create_list_delete() {
    let app = boot().await;
    let h = hash_password("hunter2").unwrap();
    db::settings::set(&app.pool, "auth.enabled", "true").await.unwrap();
    db::settings::set(&app.pool, "auth.password_hash", &h).await.unwrap();

    let client = reqwest::Client::builder().cookie_store(true).build().unwrap();
    let _ = client.post(format!("{}/api/auth/login", app.url))
        .json(&json!({"password":"hunter2"})).send().await.unwrap();

    // Create
    let made: serde_json::Value = client.post(format!("{}/api/auth/tokens", app.url))
        .json(&json!({"name":"claude-desktop"}))
        .send().await.unwrap()
        .json().await.unwrap();
    let token = made["token"].as_str().unwrap().to_string();
    assert!(token.starts_with("tcr_"));

    // List
    let listed: Vec<serde_json::Value> = client.get(format!("{}/api/auth/tokens", app.url))
        .send().await.unwrap()
        .json().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert!(!listed[0].get("token").is_some(), "list must NOT include the secret");

    // Delete
    let id = made["id"].as_i64().unwrap();
    let del = client.delete(format!("{}/api/auth/tokens/{id}", app.url))
        .send().await.unwrap();
    assert!(del.status().is_success());

    let listed2: Vec<serde_json::Value> = client.get(format!("{}/api/auth/tokens", app.url))
        .send().await.unwrap()
        .json().await.unwrap();
    assert!(listed2.is_empty());
}
```

- [ ] **Step 2: Run to confirm it fails**

Run: `cargo test -p transcoderr --test api_auth token_endpoints_create_list_delete`
Expected: FAIL with 404 (the `/api/auth/tokens` route doesn't exist yet).

- [ ] **Step 3: Add handlers**

Append to `crates/transcoderr/src/api/auth.rs`:

```rust
use crate::db::api_tokens;
use axum::extract::Path;
use transcoderr_api_types::{ApiTokenSummary, CreateTokenReq, CreateTokenResp};

pub async fn list_tokens(
    State(state): State<AppState>,
) -> Result<Json<Vec<ApiTokenSummary>>, StatusCode> {
    api_tokens::list(&state.pool)
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
    let made = api_tokens::create(&state.pool, req.name.trim())
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(CreateTokenResp { id: made.id, token: made.token }))
}

pub async fn delete_token(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    let removed = api_tokens::delete(&state.pool, id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if removed { Ok(StatusCode::NO_CONTENT) } else { Err(StatusCode::NOT_FOUND) }
}
```

- [ ] **Step 4: Wire the routes**

In `crates/transcoderr/src/api/mod.rs`, add to the `protected` router (between the existing `auth/me` line and the rest):

```rust
        .route("/auth/tokens",      get(auth::list_tokens).post(auth::create_token))
        .route("/auth/tokens/:id",  axum::routing::delete(auth::delete_token))
```

(They live in `protected` because token *management* requires session auth — bearer-tokens-managing-bearer-tokens isn't a v1 use case.)

- [ ] **Step 5: Re-run the test**

Run: `cargo test -p transcoderr --test api_auth token_endpoints_create_list_delete`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(server): /api/auth/tokens CRUD endpoints"
```

---

## Task 6: Bearer-token auth in `require_auth` + AuthSource extension

**Files:**
- Modify: `crates/transcoderr/src/api/auth.rs:69-83`

- [ ] **Step 1: Write the failing test**

Append to `crates/transcoderr/tests/api_auth.rs`:

```rust
#[tokio::test]
async fn bearer_token_authenticates_to_protected_endpoint() {
    let app = boot().await;
    let h = hash_password("hunter2").unwrap();
    db::settings::set(&app.pool, "auth.enabled", "true").await.unwrap();
    db::settings::set(&app.pool, "auth.password_hash", &h).await.unwrap();

    let made = transcoderr::db::api_tokens::create(&app.pool, "test").await.unwrap();

    // No auth → 401
    let r0 = reqwest::Client::new().get(format!("{}/api/flows", app.url)).send().await.unwrap();
    assert_eq!(r0.status(), 401);

    // Wrong token → 401
    let r1 = reqwest::Client::new().get(format!("{}/api/flows", app.url))
        .bearer_auth("tcr_definitelynotreal000000000000000")
        .send().await.unwrap();
    assert_eq!(r1.status(), 401);

    // Correct token → 200
    let r2 = reqwest::Client::new().get(format!("{}/api/flows", app.url))
        .bearer_auth(&made.token)
        .send().await.unwrap();
    assert!(r2.status().is_success(), "got {}", r2.status());
}
```

- [ ] **Step 2: Run, confirm it fails**

Run: `cargo test -p transcoderr --test api_auth bearer_token_authenticates_to_protected_endpoint`
Expected: FAIL — bearer header is currently ignored, so the request gets 401.

- [ ] **Step 3: Define `AuthSource` and extend `require_auth`**

Replace the entire `pub async fn require_auth(...)` block in `crates/transcoderr/src/api/auth.rs:69-83` with:

```rust
/// Marker placed on the request via Extension when auth was satisfied.
/// Downstream handlers consult this to decide whether to redact secrets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthSource {
    /// Auth disabled globally — treat as session-equivalent (no redaction).
    Disabled,
    /// Authenticated via session cookie (UI).
    Session,
    /// Authenticated via Bearer API token (e.g. MCP).
    Token,
}

pub async fn require_auth(
    State(state): State<AppState>,
    cookies: Cookies,
    mut request: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, StatusCode> {
    let enabled = db::settings::get(&state.pool, "auth.enabled").await
        .ok().flatten().unwrap_or_default() == "true";
    if !enabled {
        request.extensions_mut().insert(AuthSource::Disabled);
        return Ok(next.run(request).await);
    }

    // Bearer first (cheap header read, no DB if absent).
    if let Some(h) = request.headers().get(axum::http::header::AUTHORIZATION) {
        if let Ok(s) = h.to_str() {
            if let Some(token) = s.strip_prefix("Bearer ") {
                if crate::db::api_tokens::verify(&state.pool, token).await.is_some() {
                    request.extensions_mut().insert(AuthSource::Token);
                    return Ok(next.run(request).await);
                }
                return Err(StatusCode::UNAUTHORIZED);
            }
        }
    }

    // Fall back to session cookie.
    let sid = cookies.get("transcoderr_sid").ok_or(StatusCode::UNAUTHORIZED)?;
    if !session_valid(&state.pool, sid.value()).await.unwrap_or(false) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    request.extensions_mut().insert(AuthSource::Session);
    Ok(next.run(request).await)
}
```

- [ ] **Step 4: Run all auth tests**

Run: `cargo test -p transcoderr --test api_auth`
Expected: all 4 tests pass (3 from earlier tasks + the new bearer test).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(server): bearer-token auth path + AuthSource request extension"
```

---

## Task 7: Server-side secret redaction for sources & notifiers

**Files:**
- Modify: `crates/transcoderr/src/api/sources.rs`
- Modify: `crates/transcoderr/src/api/notifiers.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/transcoderr/tests/api_redaction.rs`:

```rust
mod common;
use common::boot;
use serde_json::json;
use transcoderr::{api::auth::hash_password, db};

#[tokio::test]
async fn token_authed_caller_sees_redacted_source_secret() {
    let app = boot().await;
    let h = hash_password("hunter2").unwrap();
    db::settings::set(&app.pool, "auth.enabled", "true").await.unwrap();
    db::settings::set(&app.pool, "auth.password_hash", &h).await.unwrap();

    let made = transcoderr::db::api_tokens::create(&app.pool, "mcp").await.unwrap();

    // Seed a source via SQL so we don't depend on the create endpoint here.
    sqlx::query("INSERT INTO sources (kind, name, config_json, secret_token) VALUES ('radarr','x','{}','sekrit')")
        .execute(&app.pool).await.unwrap();

    let listed: Vec<serde_json::Value> = reqwest::Client::new()
        .get(format!("{}/api/sources", app.url))
        .bearer_auth(&made.token).send().await.unwrap().json().await.unwrap();
    assert_eq!(listed[0]["secret_token"], json!("***"));

    // Same call via session cookie returns the cleartext.
    let session = reqwest::Client::builder().cookie_store(true).build().unwrap();
    session.post(format!("{}/api/auth/login", app.url))
        .json(&json!({"password":"hunter2"})).send().await.unwrap();
    let listed2: Vec<serde_json::Value> = session
        .get(format!("{}/api/sources", app.url))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(listed2[0]["secret_token"], json!("sekrit"));
}

#[tokio::test]
async fn token_authed_caller_sees_redacted_notifier_secret() {
    let app = boot().await;
    let h = hash_password("hunter2").unwrap();
    db::settings::set(&app.pool, "auth.enabled", "true").await.unwrap();
    db::settings::set(&app.pool, "auth.password_hash", &h).await.unwrap();
    let made = transcoderr::db::api_tokens::create(&app.pool, "mcp").await.unwrap();

    db::notifiers::upsert(
        &app.pool, "tg", "telegram",
        &json!({"bot_token": "1234:secret", "chat_id": "42"})
    ).await.unwrap();

    let listed: Vec<serde_json::Value> = reqwest::Client::new()
        .get(format!("{}/api/notifiers", app.url))
        .bearer_auth(&made.token).send().await.unwrap().json().await.unwrap();
    assert_eq!(listed[0]["config"]["bot_token"], json!("***"));
    assert_eq!(listed[0]["config"]["chat_id"], json!("42"));
}
```

- [ ] **Step 2: Run, confirm both fail**

Run: `cargo test -p transcoderr --test api_redaction`
Expected: 2 FAIL — server still returns cleartext secrets to all callers.

- [ ] **Step 3: Add redaction helper**

Append to `crates/transcoderr/src/api/auth.rs`:

```rust
/// Replaces secret-bearing JSON fields in-place with `"***"`. Used in
/// notifier `config` blobs where the schema varies by `kind`.
pub fn redact_notifier_config(config: &mut serde_json::Value) {
    const SECRET_KEYS: &[&str] = &[
        "bot_token", "token", "secret", "password", "api_key", "webhook_url",
        "url", "auth_token",
    ];
    if let Some(obj) = config.as_object_mut() {
        for k in SECRET_KEYS {
            if obj.contains_key(*k) {
                obj.insert((*k).into(), serde_json::Value::String("***".into()));
            }
        }
    }
}
```

- [ ] **Step 4: Apply redaction in sources handlers**

Edit `crates/transcoderr/src/api/sources.rs`. Add at top:

```rust
use crate::api::auth::AuthSource;
use axum::Extension;
```

Update `list` to take an `Extension<AuthSource>` and redact:

```rust
pub async fn list(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthSource>,
) -> Result<Json<Vec<SourceSummary>>, StatusCode> {
    let rows = sqlx::query("SELECT id, kind, name, config_json, secret_token FROM sources ORDER BY name")
        .fetch_all(&state.pool).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let out = rows.into_iter().map(|r| {
        let config_str: String = r.get(3);
        let secret: String = r.get(4);
        SourceSummary {
            id: r.get(0),
            kind: r.get(1),
            name: r.get(2),
            config: serde_json::from_str(&config_str).unwrap_or_default(),
            secret_token: if auth == AuthSource::Token { "***".into() } else { secret },
        }
    }).collect();
    Ok(Json(out))
}
```

Update `get` similarly:

```rust
pub async fn get(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthSource>,
    Path(id): Path<i64>,
) -> Result<Json<SourceSummary>, StatusCode> {
    let row = sqlx::query("SELECT id, kind, name, config_json, secret_token FROM sources WHERE id = ?")
        .bind(id).fetch_optional(&state.pool).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let config_str: String = row.get(3);
    let secret: String = row.get(4);
    Ok(Json(SourceSummary {
        id: row.get(0),
        kind: row.get(1),
        name: row.get(2),
        config: serde_json::from_str(&config_str).unwrap_or_default(),
        secret_token: if auth == AuthSource::Token { "***".into() } else { secret },
    }))
}
```

- [ ] **Step 5: Apply redaction in notifiers handlers**

Edit `crates/transcoderr/src/api/notifiers.rs`. Add at top:

```rust
use crate::api::auth::{redact_notifier_config, AuthSource};
use axum::Extension;
```

Update `list`:

```rust
pub async fn list(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthSource>,
) -> Result<Json<Vec<NotifierSummary>>, StatusCode> {
    let rows = sqlx::query("SELECT id, name, kind, config_json FROM notifiers ORDER BY name")
        .fetch_all(&state.pool).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let out = rows.into_iter().map(|r| {
        let config_str: String = r.get(3);
        let mut config: serde_json::Value = serde_json::from_str(&config_str).unwrap_or_default();
        if auth == AuthSource::Token { redact_notifier_config(&mut config); }
        NotifierSummary { id: r.get(0), name: r.get(1), kind: r.get(2), config }
    }).collect();
    Ok(Json(out))
}
```

Update `get` similarly:

```rust
pub async fn get(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthSource>,
    Path(id): Path<i64>,
) -> Result<Json<NotifierSummary>, StatusCode> {
    let row = sqlx::query("SELECT id, name, kind, config_json FROM notifiers WHERE id = ?")
        .bind(id).fetch_optional(&state.pool).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let config_str: String = row.get(3);
    let mut config: serde_json::Value = serde_json::from_str(&config_str).unwrap_or_default();
    if auth == AuthSource::Token { redact_notifier_config(&mut config); }
    Ok(Json(NotifierSummary { id: row.get(0), name: row.get(1), kind: row.get(2), config }))
}
```

- [ ] **Step 6: Re-run**

Run: `cargo test -p transcoderr --test api_redaction`
Expected: both PASS.

Run: `cargo test --workspace`
Expected: full suite still green.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(server): redact source/notifier secrets for token-authed callers"
```

---

## Task 8: Settings UI — API tokens card

**Files:**
- Create: `web/src/components/api-tokens-card.tsx`
- Modify: `web/src/api/client.ts`
- Modify: `web/src/types.ts`
- Modify: `web/src/pages/settings.tsx`

- [ ] **Step 1: Add types**

Append to `web/src/types.ts`:

```ts
export type ApiTokenSummary = {
  id: number;
  name: string;
  prefix: string;
  created_at: number;
  last_used_at: number | null;
};
```

- [ ] **Step 2: Add API client methods**

Append a new section to `web/src/api/client.ts` inside the `auth:` object, replacing it with:

```ts
  auth: {
    me:     () => req<{ auth_required: boolean; authed: boolean }>("/auth/me"),
    login:  (password: string) => req<void>("/auth/login", { method: "POST", body: JSON.stringify({ password }) }),
    logout: () => req<void>("/auth/logout", { method: "POST" }),
    tokens: {
      list:   () => req<import("../types").ApiTokenSummary[]>("/auth/tokens"),
      create: (name: string) => req<{ id: number; token: string }>("/auth/tokens", { method: "POST", body: JSON.stringify({ name }) }),
      remove: (id: number) => req<void>(`/auth/tokens/${id}`, { method: "DELETE" }),
    },
  },
```

- [ ] **Step 3: Build the card component**

Create `web/src/components/api-tokens-card.tsx`:

```tsx
import { useState } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../api/client";

export default function ApiTokensCard() {
  const qc = useQueryClient();
  const list = useQuery({ queryKey: ["api-tokens"], queryFn: api.auth.tokens.list });
  const [name, setName] = useState("");
  const [revealed, setRevealed] = useState<string | null>(null);

  const create = useMutation({
    mutationFn: (n: string) => api.auth.tokens.create(n),
    onSuccess: (resp) => {
      setRevealed(resp.token);
      setName("");
      qc.invalidateQueries({ queryKey: ["api-tokens"] });
    },
  });

  const remove = useMutation({
    mutationFn: (id: number) => api.auth.tokens.remove(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["api-tokens"] }),
  });

  return (
    <div className="surface" style={{ padding: 16, marginTop: 16 }}>
      <div className="page-header" style={{ marginBottom: 12 }}>
        <h3 style={{ margin: 0 }}>API tokens</h3>
      </div>

      <table style={{ width: "100%", marginBottom: 12 }}>
        <thead>
          <tr>
            <th style={{ textAlign: "left" }}>Name</th>
            <th style={{ textAlign: "left" }}>Prefix</th>
            <th style={{ textAlign: "left" }}>Created</th>
            <th style={{ textAlign: "left" }}>Last used</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {(list.data ?? []).map((t) => (
            <tr key={t.id}>
              <td>{t.name}</td>
              <td className="mono dim">{t.prefix}…</td>
              <td className="dim tnum">{new Date(t.created_at * 1000).toLocaleString()}</td>
              <td className="dim tnum">
                {t.last_used_at ? new Date(t.last_used_at * 1000).toLocaleString() : "—"}
              </td>
              <td>
                <button
                  className="btn-danger"
                  onClick={() => {
                    if (confirm(`Revoke token "${t.name}"?`)) remove.mutate(t.id);
                  }}
                >
                  Revoke
                </button>
              </td>
            </tr>
          ))}
          {(list.data ?? []).length === 0 && (
            <tr><td colSpan={5} className="muted">No tokens.</td></tr>
          )}
        </tbody>
      </table>

      <div style={{ display: "flex", gap: 8 }}>
        <input
          placeholder="token name (e.g. claude-desktop)"
          value={name}
          onChange={(e) => setName(e.target.value)}
          style={{ flex: 1 }}
        />
        <button
          onClick={() => create.mutate(name)}
          disabled={!name.trim() || create.isPending}
        >
          Create token
        </button>
      </div>

      {revealed && (
        <div className="surface" style={{ padding: 12, marginTop: 12, borderColor: "var(--ok)" }}>
          <div className="label" style={{ marginBottom: 6 }}>
            New token — copy it now, this is the only time it will be shown
          </div>
          <code className="mono" style={{ wordBreak: "break-all" }}>{revealed}</code>
          <div style={{ marginTop: 8, display: "flex", gap: 8 }}>
            <button onClick={() => navigator.clipboard.writeText(revealed)}>Copy</button>
            <button className="btn-ghost" onClick={() => setRevealed(null)}>I've saved it</button>
          </div>
        </div>
      )}
    </div>
  );
}
```

- [ ] **Step 4: Mount it on the Settings page**

Edit `web/src/pages/settings.tsx`. Add at the top:

```tsx
import ApiTokensCard from "../components/api-tokens-card";
```

Add `<ApiTokensCard />` just before the closing `</div>` of the page:

```tsx
      <ApiTokensCard />
    </div>
  );
}
```

- [ ] **Step 5: Build the SPA and confirm no TS errors**

Run: `npm --prefix web run build`
Expected: clean build, `web/dist/` updated.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(web): API tokens card on Settings page"
```

---

## Task 9: `transcoderr-mcp` crate skeleton

**Files:**
- Modify: `Cargo.toml` (add member)
- Create: `crates/transcoderr-mcp/Cargo.toml`
- Create: `crates/transcoderr-mcp/src/main.rs`

- [ ] **Step 1: Add the member**

In root `Cargo.toml`, update `members`:

```toml
members = [
  "crates/transcoderr",
  "crates/transcoderr-api-types",
  "crates/transcoderr-mcp",
]
```

- [ ] **Step 2: Write the manifest**

Create `crates/transcoderr-mcp/Cargo.toml`:

```toml
[package]
name = "transcoderr-mcp"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[[bin]]
name = "transcoderr-mcp"
path = "src/main.rs"

[dependencies]
transcoderr-api-types = { path = "../transcoderr-api-types" }
rmcp = { version = "0.3", features = ["server", "macros", "transport-io"] }
schemars = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
tokio = { workspace = true }
reqwest = { workspace = true }
anyhow = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true, features = ["env-filter"] }
clap = { version = "4", features = ["derive", "env"] }

[dev-dependencies]
tempfile = "3"
serial_test = "3"
```

- [ ] **Step 3: Write the failing test**

Create `crates/transcoderr-mcp/tests/smoke.rs`:

```rust
#[test]
fn binary_builds_and_help_works() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_transcoderr-mcp"))
        .arg("--help").output().expect("run --help");
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("--url"), "got: {s}");
    assert!(s.contains("--token"), "got: {s}");
}
```

- [ ] **Step 4: Write `main.rs` with arg parsing + healthz check + a stub rmcp server**

Create `crates/transcoderr-mcp/src/main.rs`:

```rust
use anyhow::{anyhow, Context, Result};
use clap::Parser;
use rmcp::{
    handler::server::router::tool::ToolRouter,
    model::{Implementation, ProtocolVersion, ServerCapabilities, ServerInfo},
    transport::io::stdio,
    tool_router, ServerHandler, ServiceExt,
};

#[derive(Parser, Debug, Clone)]
#[command(name = "transcoderr-mcp", version)]
struct Cli {
    /// transcoderr server base URL.
    #[arg(long, env = "TRANSCODERR_URL")]
    url: String,
    /// API token from Settings → API tokens.
    #[arg(long, env = "TRANSCODERR_TOKEN")]
    token: String,
    /// Per-call HTTP timeout, seconds.
    #[arg(long, env = "TRANSCODERR_TIMEOUT_SECS", default_value_t = 30)]
    timeout_secs: u64,
}

#[derive(Clone)]
struct Server {
    tool_router: ToolRouter<Self>,
}

#[tool_router(server_handler)]
impl Server {
    pub fn new() -> Self {
        Self { tool_router: Self::tool_router() }
    }
}

impl ServerHandler for Server {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2025_03_26,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation::from_build_env(),
            instructions: Some("transcoderr MCP proxy — drives runs, flows, sources, notifiers.".into()),
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "info".into()))
        .with_writer(std::io::stderr)
        .init();

    // Probe healthz before announcing capabilities — fail-fast on misconfig.
    let probe = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(cli.timeout_secs))
        .build().context("build reqwest client")?;
    let url = format!("{}/healthz", cli.url.trim_end_matches('/'));
    let resp = probe.get(&url).bearer_auth(&cli.token).send().await
        .with_context(|| format!("connect to {url}"))?;
    if !resp.status().is_success() {
        return Err(anyhow!("health probe to {url} returned {}", resp.status()));
    }
    tracing::info!(url = %cli.url, "transcoderr-mcp starting");

    let server = Server::new();
    let (stdin, stdout) = stdio();
    server.serve((stdin, stdout)).await?.waiting().await?;
    Ok(())
}
```

- [ ] **Step 5: Build**

Run: `cargo build -p transcoderr-mcp`
Expected: success.

- [ ] **Step 6: Run the smoke test**

Run: `cargo test -p transcoderr-mcp --test smoke`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(mcp): transcoderr-mcp crate skeleton with stdio rmcp server"
```

---

## Task 10: HTTP client wrapper

**Files:**
- Create: `crates/transcoderr-mcp/src/client.rs`
- Modify: `crates/transcoderr-mcp/src/main.rs`

- [ ] **Step 1: Write the client**

Create `crates/transcoderr-mcp/src/client.rs`:

```rust
use anyhow::{anyhow, Context, Result};
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
}
```

- [ ] **Step 2: Wire into main.rs**

Update `crates/transcoderr-mcp/src/main.rs`:

Add at top:

```rust
mod client;
use client::ApiClient;
```

Update the `Server` struct:

```rust
#[derive(Clone)]
struct Server {
    api: ApiClient,
    tool_router: ToolRouter<Self>,
}

#[tool_router(server_handler)]
impl Server {
    pub fn new(api: ApiClient) -> Self {
        Self { api, tool_router: Self::tool_router() }
    }
}
```

Replace the body after the healthz probe:

```rust
    let api = ApiClient::new(cli.url.clone(), cli.token.clone(), cli.timeout_secs)?;
    let server = Server::new(api);
    let (stdin, stdout) = stdio();
    server.serve((stdin, stdout)).await?.waiting().await?;
    Ok(())
```

- [ ] **Step 3: Confirm it builds**

Run: `cargo build -p transcoderr-mcp`
Expected: success.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(mcp): HTTP client wrapper with bearer auth + ApiError mapping"
```

---

## Task 11: MCP tools — runs

**Files:**
- Create: `crates/transcoderr-mcp/src/tools/mod.rs`
- Create: `crates/transcoderr-mcp/src/tools/runs.rs`
- Modify: `crates/transcoderr-mcp/src/main.rs`

- [ ] **Step 1: Create the tools module**

Create `crates/transcoderr-mcp/src/tools/mod.rs`:

```rust
pub mod runs;
```

- [ ] **Step 2: Implement run tools**

Create `crates/transcoderr-mcp/src/tools/runs.rs`:

```rust
use crate::client::ApiClient;
use rmcp::{
    handler::server::wrapper::{Json, Parameters},
    model::ErrorData,
    tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use transcoderr_api_types::{RerunResp, RunDetail, RunEvent, RunSummary};

#[derive(Clone)]
pub struct RunsTools { pub api: ApiClient }

#[derive(Deserialize, Serialize, JsonSchema, Default)]
pub struct ListRunsArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<i64>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct IdArgs { pub id: i64 }

#[derive(Deserialize, Serialize, JsonSchema, Default)]
pub struct EventsArgs {
    pub id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<i64>,
}

#[tool_router(router = runs_router)]
impl RunsTools {
    #[tool(name = "list_runs", description = "List job runs, newest first. Filter by status (`pending|running|completed|failed|cancelled`), flow_id; default limit 50, max 500.")]
    pub async fn list_runs(&self, Parameters(a): Parameters<ListRunsArgs>) -> Result<Json<Vec<RunSummary>>, ErrorData> {
        let mut q: Vec<String> = Vec::new();
        if let Some(s) = a.status { q.push(format!("status={s}")); }
        if let Some(f) = a.flow_id { q.push(format!("flow_id={f}")); }
        if let Some(l) = a.limit { q.push(format!("limit={l}")); }
        if let Some(o) = a.offset { q.push(format!("offset={o}")); }
        let path = if q.is_empty() { "/api/runs".into() } else { format!("/api/runs?{}", q.join("&")) };
        self.api.get::<Vec<RunSummary>>(&path).await.map(Json).map_err(|e| e.into_error_data())
    }

    #[tool(name = "get_run", description = "Get a run by id, including its full event timeline (last 200 events).")]
    pub async fn get_run(&self, Parameters(a): Parameters<IdArgs>) -> Result<Json<RunDetail>, ErrorData> {
        self.api.get::<RunDetail>(&format!("/api/runs/{}", a.id)).await
            .map(Json).map_err(|e| e.into_error_data())
    }

    #[tool(name = "get_run_events", description = "Get raw events for a run, oldest first; for tailing live timelines.")]
    pub async fn get_run_events(&self, Parameters(a): Parameters<EventsArgs>) -> Result<Json<Vec<RunEvent>>, ErrorData> {
        let mut q: Vec<String> = Vec::new();
        if let Some(l) = a.limit { q.push(format!("limit={l}")); }
        if let Some(o) = a.offset { q.push(format!("offset={o}")); }
        let path = if q.is_empty() {
            format!("/api/runs/{}/events", a.id)
        } else {
            format!("/api/runs/{}/events?{}", a.id, q.join("&"))
        };
        self.api.get::<Vec<RunEvent>>(&path).await.map(Json).map_err(|e| e.into_error_data())
    }

    #[tool(name = "cancel_run", description = "Cancel a pending or running job. Sends SIGKILL to ffmpeg if running.")]
    pub async fn cancel_run(&self, Parameters(a): Parameters<IdArgs>) -> Result<Json<serde_json::Value>, ErrorData> {
        self.api.post::<serde_json::Value, _>(&format!("/api/runs/{}/cancel", a.id), &serde_json::Value::Null).await
            .map(Json).map_err(|e| e.into_error_data())
    }

    #[tool(name = "rerun_run", description = "Create a new pending job from this one's flow + file. Returns the new run id.")]
    pub async fn rerun_run(&self, Parameters(a): Parameters<IdArgs>) -> Result<Json<RerunResp>, ErrorData> {
        self.api.post::<RerunResp, _>(&format!("/api/runs/{}/rerun", a.id), &serde_json::Value::Null).await
            .map(Json).map_err(|e| e.into_error_data())
    }
}
```

- [ ] **Step 3: Wire RunsTools into the server**

In `crates/transcoderr-mcp/src/main.rs`, replace the `Server` impl block to compose the runs router:

```rust
mod client;
mod tools;

use client::ApiClient;
use rmcp::{
    handler::server::router::tool::ToolRouter,
    model::{Implementation, ProtocolVersion, ServerCapabilities, ServerInfo},
    transport::io::stdio,
    tool_router, ServerHandler, ServiceExt,
};

#[derive(Clone)]
struct Server {
    runs: tools::runs::RunsTools,
    tool_router: ToolRouter<Self>,
}

#[tool_router(server_handler)]
impl Server {
    pub fn new(api: ApiClient) -> Self {
        let runs = tools::runs::RunsTools { api: api.clone() };
        let tool_router = Self::tool_router() + tools::runs::RunsTools::runs_router();
        Self { runs, tool_router }
    }
}

impl ServerHandler for Server {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2025_03_26,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation::from_build_env(),
            instructions: Some("transcoderr MCP proxy — drives runs, flows, sources, notifiers.".into()),
        }
    }
}
```

The rmcp `ToolRouter` composition pattern uses `Self::tool_router() + Other::other_router()`. Each sub-router carries closures over its own state, so the `RunsTools` instance held in the `Server` struct is what those closures dispatch into.

- [ ] **Step 4: Build**

Run: `cargo build -p transcoderr-mcp`
Expected: success.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(mcp): runs tools (list/get/events/cancel/rerun)"
```

---

## Task 12: MCP tools — flows

**Files:**
- Create: `crates/transcoderr-mcp/src/tools/flows.rs`
- Modify: `crates/transcoderr-mcp/src/tools/mod.rs`
- Modify: `crates/transcoderr-mcp/src/main.rs`

- [ ] **Step 1: Implement flow tools**

Create `crates/transcoderr-mcp/src/tools/flows.rs`:

```rust
use crate::client::ApiClient;
use rmcp::{
    handler::server::wrapper::{Json, Parameters},
    model::ErrorData,
    tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use transcoderr_api_types::{CreateFlowReq, CreatedIdResp, FlowDetail, FlowSummary, UpdateFlowReq};

#[derive(Clone)]
pub struct FlowsTools { pub api: ApiClient }

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct IdArgs { pub id: i64 }

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct UpdateFlowArgs {
    pub id: i64,
    pub yaml: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct DeleteArgs {
    pub id: i64,
    /// Required confirmation. Reject the call by setting this to false.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct DryRunArgs {
    pub yaml: String,
    pub file_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe: Option<serde_json::Value>,
}

#[tool_router(router = flows_router)]
impl FlowsTools {
    #[tool(name = "list_flows", description = "List all configured flows.")]
    pub async fn list_flows(&self, _: Parameters<()>) -> Result<Json<Vec<FlowSummary>>, ErrorData> {
        self.api.get::<Vec<FlowSummary>>("/api/flows").await.map(Json).map_err(|e| e.into_error_data())
    }

    #[tool(name = "get_flow", description = "Get a flow with its YAML source and parsed AST.")]
    pub async fn get_flow(&self, Parameters(a): Parameters<IdArgs>) -> Result<Json<FlowDetail>, ErrorData> {
        self.api.get::<FlowDetail>(&format!("/api/flows/{}", a.id)).await.map(Json).map_err(|e| e.into_error_data())
    }

    #[tool(name = "create_flow", description = "Create a new flow from YAML. Name must be unique.")]
    pub async fn create_flow(&self, Parameters(a): Parameters<CreateFlowReq>) -> Result<Json<FlowSummary>, ErrorData> {
        self.api.post::<FlowSummary, _>("/api/flows", &a).await.map(Json).map_err(|e| e.into_error_data())
    }

    #[tool(name = "update_flow", description = "Replace the YAML for an existing flow. Bumps the version.")]
    pub async fn update_flow(&self, Parameters(a): Parameters<UpdateFlowArgs>) -> Result<Json<serde_json::Value>, ErrorData> {
        let body = UpdateFlowReq { yaml: a.yaml, enabled: a.enabled };
        self.api.put::<serde_json::Value, _>(&format!("/api/flows/{}", a.id), &body).await.map(Json).map_err(|e| e.into_error_data())
    }

    #[tool(name = "delete_flow", description = "Delete a flow. Requires confirm=true.")]
    pub async fn delete_flow(&self, Parameters(a): Parameters<DeleteArgs>) -> Result<Json<serde_json::Value>, ErrorData> {
        if !a.confirm {
            return Err(ErrorData {
                code: rmcp::model::ErrorCode::INVALID_PARAMS,
                message: "delete_flow requires `confirm: true`".into(),
                data: None,
            });
        }
        self.api.delete::<serde_json::Value>(&format!("/api/flows/{}", a.id)).await.map(Json).map_err(|e| e.into_error_data())
    }

    #[tool(name = "dry_run_flow", description = "Walk a flow's AST against a synthetic file path; returns which steps would execute.")]
    pub async fn dry_run_flow(&self, Parameters(a): Parameters<DryRunArgs>) -> Result<Json<serde_json::Value>, ErrorData> {
        self.api.post::<serde_json::Value, _>("/api/dry-run", &a).await.map(Json).map_err(|e| e.into_error_data())
    }
}
```

Note: `CreatedIdResp` is imported via `use transcoderr_api_types::CreatedIdResp;` even though only `FlowSummary` is the create-flow response in this codebase. The import is forward-looking — leave it in if the type is used in `update_flow` returns; otherwise drop the import to keep the file warning-free.

- [ ] **Step 2: Register the module**

Edit `crates/transcoderr-mcp/src/tools/mod.rs`:

```rust
pub mod flows;
pub mod runs;
```

- [ ] **Step 3: Compose into the server**

In `crates/transcoderr-mcp/src/main.rs`, update `Server::new`:

```rust
    pub fn new(api: ApiClient) -> Self {
        let runs = tools::runs::RunsTools { api: api.clone() };
        let flows = tools::flows::FlowsTools { api: api.clone() };
        let tool_router = Self::tool_router()
            + tools::runs::RunsTools::runs_router()
            + tools::flows::FlowsTools::flows_router();
        Self { runs, flows, tool_router }
    }
```

Add `flows: tools::flows::FlowsTools,` to the struct.

- [ ] **Step 4: Build**

Run: `cargo build -p transcoderr-mcp`
Expected: success.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(mcp): flows tools (list/get/create/update/delete/dry-run)"
```

---

## Task 13: MCP tools — sources

**Files:**
- Create: `crates/transcoderr-mcp/src/tools/sources.rs`
- Modify: `crates/transcoderr-mcp/src/tools/mod.rs`
- Modify: `crates/transcoderr-mcp/src/main.rs`

- [ ] **Step 1: Implement source tools**

Create `crates/transcoderr-mcp/src/tools/sources.rs`:

```rust
use crate::client::ApiClient;
use rmcp::{
    handler::server::wrapper::{Json, Parameters},
    model::{ErrorCode, ErrorData},
    tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use transcoderr_api_types::{CreateSourceReq, CreatedIdResp, SourceSummary, UpdateSourceReq};

#[derive(Clone)]
pub struct SourcesTools { pub api: ApiClient }

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct IdArgs { pub id: i64 }

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct UpdateArgs {
    pub id: i64,
    #[serde(flatten)]
    pub patch: UpdateSourceReq,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct DeleteArgs {
    pub id: i64,
    pub confirm: bool,
}

#[tool_router(router = sources_router)]
impl SourcesTools {
    #[tool(name = "list_sources", description = "List webhook sources (radarr/sonarr/lidarr/generic). Secret tokens are redacted to `***`.")]
    pub async fn list_sources(&self, _: Parameters<()>) -> Result<Json<Vec<SourceSummary>>, ErrorData> {
        self.api.get::<Vec<SourceSummary>>("/api/sources").await.map(Json).map_err(|e| e.into_error_data())
    }

    #[tool(name = "get_source", description = "Get a source by id. Secret tokens are redacted.")]
    pub async fn get_source(&self, Parameters(a): Parameters<IdArgs>) -> Result<Json<SourceSummary>, ErrorData> {
        self.api.get::<SourceSummary>(&format!("/api/sources/{}", a.id)).await.map(Json).map_err(|e| e.into_error_data())
    }

    #[tool(name = "create_source", description = "Create a webhook source. `kind` is one of `radarr|sonarr|lidarr|generic`; `secret_token` is what your *arr instance uses for Bearer or Basic auth.")]
    pub async fn create_source(&self, Parameters(a): Parameters<CreateSourceReq>) -> Result<Json<CreatedIdResp>, ErrorData> {
        self.api.post::<CreatedIdResp, _>("/api/sources", &a).await.map(Json).map_err(|e| e.into_error_data())
    }

    #[tool(name = "update_source", description = "Patch fields on an existing source. Omitted fields are unchanged.")]
    pub async fn update_source(&self, Parameters(a): Parameters<UpdateArgs>) -> Result<Json<serde_json::Value>, ErrorData> {
        self.api.put::<serde_json::Value, _>(&format!("/api/sources/{}", a.id), &a.patch).await.map(Json).map_err(|e| e.into_error_data())
    }

    #[tool(name = "delete_source", description = "Delete a source. Requires confirm=true.")]
    pub async fn delete_source(&self, Parameters(a): Parameters<DeleteArgs>) -> Result<Json<serde_json::Value>, ErrorData> {
        if !a.confirm {
            return Err(ErrorData { code: ErrorCode::INVALID_PARAMS, message: "delete_source requires `confirm: true`".into(), data: None });
        }
        self.api.delete::<serde_json::Value>(&format!("/api/sources/{}", a.id)).await.map(Json).map_err(|e| e.into_error_data())
    }
}
```

- [ ] **Step 2: Register the module**

Add `pub mod sources;` to `crates/transcoderr-mcp/src/tools/mod.rs`.

- [ ] **Step 3: Compose into the server**

In `crates/transcoderr-mcp/src/main.rs` `Server` struct add `sources: tools::sources::SourcesTools,` and update `new`:

```rust
        let sources = tools::sources::SourcesTools { api: api.clone() };
        let tool_router = Self::tool_router()
            + tools::runs::RunsTools::runs_router()
            + tools::flows::FlowsTools::flows_router()
            + tools::sources::SourcesTools::sources_router();
        Self { runs, flows, sources, tool_router }
```

- [ ] **Step 4: Build**

Run: `cargo build -p transcoderr-mcp`
Expected: success.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(mcp): sources tools (list/get/create/update/delete)"
```

---

## Task 14: MCP tools — notifiers

**Files:**
- Create: `crates/transcoderr-mcp/src/tools/notifiers.rs`
- Modify: `crates/transcoderr-mcp/src/tools/mod.rs`
- Modify: `crates/transcoderr-mcp/src/main.rs`

- [ ] **Step 1: Implement notifier tools**

Create `crates/transcoderr-mcp/src/tools/notifiers.rs`:

```rust
use crate::client::ApiClient;
use rmcp::{
    handler::server::wrapper::{Json, Parameters},
    model::{ErrorCode, ErrorData},
    tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use transcoderr_api_types::{CreatedIdResp, NotifierReq, NotifierSummary};

#[derive(Clone)]
pub struct NotifiersTools { pub api: ApiClient }

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct IdArgs { pub id: i64 }

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct UpdateArgs {
    pub id: i64,
    #[serde(flatten)]
    pub body: NotifierReq,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct DeleteArgs { pub id: i64, pub confirm: bool }

#[tool_router(router = notifiers_router)]
impl NotifiersTools {
    #[tool(name = "list_notifiers", description = "List notifier channels (discord/ntfy/telegram/webhook). Secret-bearing config keys are redacted.")]
    pub async fn list_notifiers(&self, _: Parameters<()>) -> Result<Json<Vec<NotifierSummary>>, ErrorData> {
        self.api.get::<Vec<NotifierSummary>>("/api/notifiers").await.map(Json).map_err(|e| e.into_error_data())
    }

    #[tool(name = "get_notifier", description = "Get a notifier by id. Secrets redacted.")]
    pub async fn get_notifier(&self, Parameters(a): Parameters<IdArgs>) -> Result<Json<NotifierSummary>, ErrorData> {
        self.api.get::<NotifierSummary>(&format!("/api/notifiers/{}", a.id)).await.map(Json).map_err(|e| e.into_error_data())
    }

    #[tool(name = "create_notifier", description = "Create a notifier. `kind` is one of `discord|ntfy|telegram|webhook`; `config` shape depends on kind.")]
    pub async fn create_notifier(&self, Parameters(a): Parameters<NotifierReq>) -> Result<Json<CreatedIdResp>, ErrorData> {
        self.api.post::<CreatedIdResp, _>("/api/notifiers", &a).await.map(Json).map_err(|e| e.into_error_data())
    }

    #[tool(name = "update_notifier", description = "Replace fields on an existing notifier. All fields required.")]
    pub async fn update_notifier(&self, Parameters(a): Parameters<UpdateArgs>) -> Result<Json<serde_json::Value>, ErrorData> {
        self.api.put::<serde_json::Value, _>(&format!("/api/notifiers/{}", a.id), &a.body).await.map(Json).map_err(|e| e.into_error_data())
    }

    #[tool(name = "delete_notifier", description = "Delete a notifier. Requires confirm=true.")]
    pub async fn delete_notifier(&self, Parameters(a): Parameters<DeleteArgs>) -> Result<Json<serde_json::Value>, ErrorData> {
        if !a.confirm {
            return Err(ErrorData { code: ErrorCode::INVALID_PARAMS, message: "delete_notifier requires `confirm: true`".into(), data: None });
        }
        self.api.delete::<serde_json::Value>(&format!("/api/notifiers/{}", a.id)).await.map(Json).map_err(|e| e.into_error_data())
    }

    #[tool(name = "test_notifier", description = "Send a test notification through this channel.")]
    pub async fn test_notifier(&self, Parameters(a): Parameters<IdArgs>) -> Result<Json<serde_json::Value>, ErrorData> {
        self.api.post::<serde_json::Value, _>(&format!("/api/notifiers/{}/test", a.id), &serde_json::Value::Null).await.map(Json).map_err(|e| e.into_error_data())
    }
}
```

- [ ] **Step 2: Register the module**

Add `pub mod notifiers;` to `crates/transcoderr-mcp/src/tools/mod.rs`.

- [ ] **Step 3: Compose into the server**

In `crates/transcoderr-mcp/src/main.rs`, mirror the pattern: add field, append router. Final `new`:

```rust
    pub fn new(api: ApiClient) -> Self {
        let runs = tools::runs::RunsTools { api: api.clone() };
        let flows = tools::flows::FlowsTools { api: api.clone() };
        let sources = tools::sources::SourcesTools { api: api.clone() };
        let notifiers = tools::notifiers::NotifiersTools { api: api.clone() };
        let tool_router = Self::tool_router()
            + tools::runs::RunsTools::runs_router()
            + tools::flows::FlowsTools::flows_router()
            + tools::sources::SourcesTools::sources_router()
            + tools::notifiers::NotifiersTools::notifiers_router();
        Self { runs, flows, sources, notifiers, tool_router }
    }
```

- [ ] **Step 4: Build**

Run: `cargo build -p transcoderr-mcp`
Expected: success.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(mcp): notifiers tools (list/get/create/update/delete/test)"
```

---

## Task 15: MCP tools — system

**Files:**
- Create: `crates/transcoderr-mcp/src/tools/system.rs`
- Modify: `crates/transcoderr-mcp/src/tools/mod.rs`
- Modify: `crates/transcoderr-mcp/src/main.rs`

- [ ] **Step 1: Implement system tools**

Create `crates/transcoderr-mcp/src/tools/system.rs`:

```rust
use crate::client::ApiClient;
use rmcp::{
    handler::server::wrapper::{Json, Parameters},
    model::ErrorData,
    tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use transcoderr_api_types::{Health, RunSummary};

#[derive(Clone)]
pub struct SystemTools { pub api: ApiClient }

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct QueueResp {
    pub pending: Vec<RunSummary>,
    pub running: Vec<RunSummary>,
}

#[tool_router(router = system_router)]
impl SystemTools {
    #[tool(name = "get_health", description = "Server health: probes /healthz and /readyz.")]
    pub async fn get_health(&self, _: Parameters<()>) -> Result<Json<Health>, ErrorData> {
        let healthy = self.api.get_text("/healthz").await.is_ok();
        let ready = self.api.get_text("/readyz").await.is_ok();
        Ok(Json(Health { healthy, ready }))
    }

    #[tool(name = "get_queue", description = "Pending and currently-running jobs.")]
    pub async fn get_queue(&self, _: Parameters<()>) -> Result<Json<QueueResp>, ErrorData> {
        let pending = self.api.get::<Vec<RunSummary>>("/api/runs?status=pending&limit=500").await.map_err(|e| e.into_error_data())?;
        let running = self.api.get::<Vec<RunSummary>>("/api/runs?status=running&limit=500").await.map_err(|e| e.into_error_data())?;
        Ok(Json(QueueResp { pending, running }))
    }

    #[tool(name = "get_hw_caps", description = "Hardware-encode capability snapshot (NVENC/QSV/VAAPI/VideoToolbox detection).")]
    pub async fn get_hw_caps(&self, _: Parameters<()>) -> Result<Json<serde_json::Value>, ErrorData> {
        self.api.get::<serde_json::Value>("/api/hw").await.map(Json).map_err(|e| e.into_error_data())
    }

    #[tool(name = "get_metrics", description = "Prometheus metrics text exposition (passthrough from /metrics).")]
    pub async fn get_metrics(&self, _: Parameters<()>) -> Result<Json<serde_json::Value>, ErrorData> {
        let txt = self.api.get_text("/metrics").await.map_err(|e| e.into_error_data())?;
        Ok(Json(serde_json::Value::String(txt)))
    }
}
```

- [ ] **Step 2: Register the module**

Add `pub mod system;` to `crates/transcoderr-mcp/src/tools/mod.rs`. Final mod.rs:

```rust
pub mod flows;
pub mod notifiers;
pub mod runs;
pub mod sources;
pub mod system;
```

- [ ] **Step 3: Compose into the server**

Final `Server::new` body in `main.rs`:

```rust
    pub fn new(api: ApiClient) -> Self {
        let runs = tools::runs::RunsTools { api: api.clone() };
        let flows = tools::flows::FlowsTools { api: api.clone() };
        let sources = tools::sources::SourcesTools { api: api.clone() };
        let notifiers = tools::notifiers::NotifiersTools { api: api.clone() };
        let system = tools::system::SystemTools { api: api.clone() };
        let tool_router = Self::tool_router()
            + tools::runs::RunsTools::runs_router()
            + tools::flows::FlowsTools::flows_router()
            + tools::sources::SourcesTools::sources_router()
            + tools::notifiers::NotifiersTools::notifiers_router()
            + tools::system::SystemTools::system_router();
        Self { runs, flows, sources, notifiers, system, tool_router }
    }
```

Add the `system: tools::system::SystemTools,` field.

- [ ] **Step 4: Build**

Run: `cargo build -p transcoderr-mcp`
Expected: success — all 25 tools registered.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(mcp): system tools (health/queue/hw/metrics)"
```

---

## Task 16: End-to-end integration test driving MCP over stdio

**Files:**
- Create: `crates/transcoderr-mcp/tests/mcp_stdio_e2e.rs`

- [ ] **Step 1: Write the integration test**

Create `crates/transcoderr-mcp/tests/mcp_stdio_e2e.rs`:

```rust
//! End-to-end: spin up `transcoderr serve` on an ephemeral port with a
//! tempdir DB, seed an api_token row, drive `transcoderr-mcp` over stdio,
//! exercise the happy path: list_runs → create_flow → dry_run_flow.

use serial_test::serial;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const FLOW_YAML: &str = r#"
name: e2e-flow
triggers:
  - radarr: [downloaded]
steps:
  - use: probe
"#;

fn wait_until_healthy(url: &str, deadline: Duration) {
    let start = Instant::now();
    while start.elapsed() < deadline {
        if reqwest::blocking::get(format!("{url}/healthz"))
            .map(|r| r.status().is_success()).unwrap_or(false) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("server did not become healthy within {deadline:?}");
}

fn jsonrpc(line: &str) -> serde_json::Value {
    serde_json::from_str(line).expect("valid jsonrpc")
}

#[test]
#[serial]
fn mcp_stdio_happy_path() {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().to_path_buf();

    // Write a minimal config.
    let cfg_path = data_dir.join("config.toml");
    std::fs::write(&cfg_path, format!(r#"
bind = "127.0.0.1:0"
data_dir = "{}"
[radarr]
bearer_token = "test"
"#, data_dir.display())).unwrap();

    // Start `transcoderr serve` on an ephemeral port. Capture stderr to find the bound port.
    // Strategy: bind = "127.0.0.1:0" → port logged on stderr by the server boot.
    let server_bin = env!("CARGO_BIN_EXE_transcoderr");
    let mut server = Command::new(server_bin)
        .arg("serve").arg("--config").arg(&cfg_path)
        .env("RUST_LOG", "warn,transcoderr=info")
        .stdout(Stdio::null()).stderr(Stdio::piped())
        .spawn().expect("spawn server");

    // Read stderr until we see "listening on 127.0.0.1:NNNN".
    let stderr = server.stderr.take().unwrap();
    let mut rdr = BufReader::new(stderr);
    let mut port: Option<u16> = None;
    for _ in 0..200 {
        let mut line = String::new();
        if rdr.read_line(&mut line).unwrap_or(0) == 0 { break; }
        if let Some(idx) = line.find("127.0.0.1:") {
            let rest = &line[idx + "127.0.0.1:".len()..];
            let n: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(p) = n.parse() { port = Some(p); break; }
        }
    }
    let port = port.expect("did not parse bound port from server stderr");
    let url = format!("http://127.0.0.1:{port}");
    wait_until_healthy(&url, Duration::from_secs(5));

    // Open a sqlite connection and seed an api token (skipping the password-auth flow).
    let db_path = data_dir.join("data.db");
    let conn = rusqlite_or_sqlx_seed(&db_path);
    let raw_token = "tcr_E2E_TEST_TOKEN_FIXED_LENGTH_OK_X";  // 4 + 32 chars
    conn(raw_token);

    // Spawn transcoderr-mcp.
    let mcp_bin = env!("CARGO_BIN_EXE_transcoderr-mcp");
    let mut mcp = Command::new(mcp_bin)
        .env("TRANSCODERR_URL", &url)
        .env("TRANSCODERR_TOKEN", raw_token)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().expect("spawn mcp");

    let mut stdin = mcp.stdin.take().unwrap();
    let stdout = mcp.stdout.take().unwrap();
    let mut stdout = BufReader::new(stdout);

    fn send(stdin: &mut impl Write, v: serde_json::Value) {
        let s = serde_json::to_string(&v).unwrap();
        writeln!(stdin, "{s}").unwrap();
        stdin.flush().unwrap();
    }
    fn recv(stdout: &mut impl BufRead) -> serde_json::Value {
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        jsonrpc(&line)
    }

    // initialize
    send(&mut stdin, serde_json::json!({
        "jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"e2e","version":"0"}}
    }));
    let init = recv(&mut stdout);
    assert!(init["result"]["serverInfo"]["name"].as_str().is_some());

    // initialized notification
    send(&mut stdin, serde_json::json!({
        "jsonrpc":"2.0","method":"notifications/initialized","params":{}
    }));

    // tools/list
    send(&mut stdin, serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/list","params":{}
    }));
    let listed = recv(&mut stdout);
    let tools = listed["result"]["tools"].as_array().expect("tools array");
    assert!(tools.iter().any(|t| t["name"] == "list_runs"));
    assert!(tools.iter().any(|t| t["name"] == "create_flow"));

    // call list_runs
    send(&mut stdin, serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"list_runs","arguments":{}}
    }));
    let runs = recv(&mut stdout);
    assert_eq!(runs["error"], serde_json::Value::Null);

    // call create_flow
    send(&mut stdin, serde_json::json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"create_flow","arguments":{"name":"e2e","yaml":FLOW_YAML}}
    }));
    let created = recv(&mut stdout);
    assert!(created["result"]["structuredContent"]["id"].as_i64().is_some(), "got: {created}");

    // call dry_run_flow
    send(&mut stdin, serde_json::json!({
        "jsonrpc":"2.0","id":5,"method":"tools/call",
        "params":{"name":"dry_run_flow","arguments":{"yaml":FLOW_YAML,"file_path":"/x.mkv"}}
    }));
    let dry = recv(&mut stdout);
    assert_eq!(dry["error"], serde_json::Value::Null);

    // shutdown
    drop(stdin);
    let _ = mcp.wait_timeout_ms_or_kill(2000);
    let _ = server.kill();
}

/// Helper: open a fresh sqlite connection via sqlx-blocking-style and seed
/// an api_tokens row that the production code will accept.
fn rusqlite_or_sqlx_seed(db_path: &std::path::Path) -> impl Fn(&str) {
    let path = db_path.to_path_buf();
    move |raw_token: &str| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let url = format!("sqlite://{}", path.display());
            let pool = sqlx::sqlite::SqlitePoolOptions::new()
                .connect(&url).await.unwrap();
            let hash = transcoderr::api::auth::hash_password(raw_token).unwrap();
            let prefix = &raw_token[..12];
            sqlx::query("INSERT INTO api_tokens (name, hash, prefix, created_at) VALUES (?, ?, ?, ?)")
                .bind("e2e").bind(hash).bind(prefix).bind(chrono::Utc::now().timestamp())
                .execute(&pool).await.unwrap();
            // Enable auth so require_auth runs.
            sqlx::query("INSERT OR REPLACE INTO settings (key, value) VALUES ('auth.enabled','true')")
                .execute(&pool).await.unwrap();
            // Set a placeholder password hash so login() doesn't 500 if anyone hits it.
            let pw_hash = transcoderr::api::auth::hash_password("unused").unwrap();
            sqlx::query("INSERT OR REPLACE INTO settings (key, value) VALUES ('auth.password_hash', ?)")
                .bind(pw_hash).execute(&pool).await.unwrap();
        });
    }
}

trait WaitTimeout {
    fn wait_timeout_ms_or_kill(&mut self, ms: u64) -> std::io::Result<()>;
}
impl WaitTimeout for std::process::Child {
    fn wait_timeout_ms_or_kill(&mut self, ms: u64) -> std::io::Result<()> {
        let deadline = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < deadline {
            if let Some(_) = self.try_wait()? { return Ok(()); }
            std::thread::sleep(Duration::from_millis(50));
        }
        self.kill()
    }
}
```

The test depends on the `transcoderr` server binary being in scope. Add to `crates/transcoderr-mcp/Cargo.toml` `[dev-dependencies]`:

```toml
transcoderr = { path = "../transcoderr" }
sqlx = { version = "0.8", default-features = false, features = ["runtime-tokio", "sqlite"] }
chrono = { workspace = true }
reqwest = { version = "0.12", default-features = false, features = ["blocking", "rustls-tls"] }
```

And add a `[[test]]` section to enable the binary handle:

```toml
[[test]]
name = "mcp_stdio_e2e"
path = "tests/mcp_stdio_e2e.rs"
```

The `env!("CARGO_BIN_EXE_transcoderr")` env var is set automatically when `transcoderr` is a workspace dev-dep.

- [ ] **Step 2: Run the test**

Run: `cargo test -p transcoderr-mcp --test mcp_stdio_e2e -- --nocapture`
Expected: PASS — all assertions hold (initialize, tools/list contains list_runs+create_flow, list_runs returns no error, create_flow returns id, dry_run_flow returns no error).

- [ ] **Step 3: If port-parsing breaks**

If the server's stderr line-format doesn't match the parser, check `crates/transcoderr/src/main.rs` for the boot log statement and adjust the `line.find("127.0.0.1:")` parse accordingly. The tracing format used by the project is `field=value` style — search for the actual line in stderr while running and adapt.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "test(mcp): end-to-end stdio integration test"
```

---

## Task 17: Release CI — add `transcoderr-mcp` artifacts

**Files:**
- Modify: `.github/workflows/release.yml`

- [ ] **Step 1: Update each `bins-*` job to also build the MCP binary**

In `.github/workflows/release.yml`, for each of the three jobs (`bins-linux-amd64`, `bins-linux-arm64`, `bins-darwin-arm64`), update the `Build release binary` and `Stage artifact` steps.

For `bins-linux-amd64`, replace those two steps with:

```yaml
      - name: Build release binaries
        run: cargo build --release --locked --workspace --target x86_64-unknown-linux-gnu
      - name: Stage artifacts
        run: |
          mkdir -p out
          cp target/x86_64-unknown-linux-gnu/release/transcoderr     out/transcoderr-linux-amd64
          cp target/x86_64-unknown-linux-gnu/release/transcoderr-mcp out/transcoderr-mcp-linux-amd64
```

For `bins-linux-arm64`:

```yaml
      - name: Build release binaries
        run: cargo build --release --locked --workspace --target aarch64-unknown-linux-gnu
      - name: Stage artifacts
        run: |
          mkdir -p out
          cp target/aarch64-unknown-linux-gnu/release/transcoderr     out/transcoderr-linux-arm64
          cp target/aarch64-unknown-linux-gnu/release/transcoderr-mcp out/transcoderr-mcp-linux-arm64
```

For `bins-darwin-arm64`:

```yaml
      - name: Build release binaries
        run: cargo build --release --locked --workspace --target aarch64-apple-darwin
      - name: Stage artifacts
        run: |
          mkdir -p out
          cp target/aarch64-apple-darwin/release/transcoderr     out/transcoderr-darwin-arm64
          cp target/aarch64-apple-darwin/release/transcoderr-mcp out/transcoderr-mcp-darwin-arm64
```

- [ ] **Step 2: Verify the release workflow YAML still parses**

Run: `npx --yes @action-validator/cli .github/workflows/release.yml`
Expected: no errors. (If `npx` is unavailable, run a YAML linter; the goal is to confirm well-formedness.)

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci: ship transcoderr-mcp alongside transcoderr in releases"
```

---

## Task 18: Documentation — README + `docs/mcp.md`

**Files:**
- Create: `docs/mcp.md`
- Modify: `README.md`

- [ ] **Step 1: Add an MCP section to the README**

In `README.md`, just before the `## Documentation` section, insert:

```markdown
## MCP server

`transcoderr-mcp` is a stdio MCP binary that lets AI clients (Claude Desktop,
Cursor) drive transcoderr's read & write surface. Download the binary for
your platform from the latest GitHub Release, then point your AI client at
it.

```json
{
  "mcpServers": {
    "transcoderr": {
      "command": "/usr/local/bin/transcoderr-mcp",
      "env": {
        "TRANSCODERR_URL": "http://192.168.1.176:8099",
        "TRANSCODERR_TOKEN": "tcr_xxxxxxxxxxxxxxxx"
      }
    }
  }
}
```

Create the token under **Settings → API tokens** in the web UI. See
[`docs/mcp.md`](docs/mcp.md) for the full tool reference.
```

- [ ] **Step 2: Add the `## Documentation` reference to the new file**

In the existing `## Documentation` list, add:

```markdown
- [`docs/mcp.md`](docs/mcp.md) — MCP server reference
```

- [ ] **Step 3: Write `docs/mcp.md`**

Create `docs/mcp.md`:

```markdown
# transcoderr MCP server

`transcoderr-mcp` is a Rust binary that speaks the Model Context Protocol
over stdio. It's a stateless proxy: the AI client invokes a tool, the
binary translates that into an authenticated HTTPS call against
`transcoderr serve`, and returns the result.

## Configuration

Three env vars (or the matching CLI flags):

| var                         | required | default | meaning                      |
| --------------------------- | -------- | ------- | ---------------------------- |
| `TRANSCODERR_URL`           | yes      | —       | base URL of the server       |
| `TRANSCODERR_TOKEN`         | yes      | —       | API token from Settings → API tokens |
| `TRANSCODERR_TIMEOUT_SECS`  | no       | `30`    | per-call HTTP timeout        |

CLI flags (`--url`, `--token`, `--timeout-secs`) override env vars when present.

## Creating a token

1. In the web UI, go to **Settings → API tokens**.
2. Click **Create token**, give it a name (e.g. `claude-desktop`).
3. Copy the token shown once — you can't recover it later.
4. Paste it into your AI client's MCP config under `env.TRANSCODERR_TOKEN`.

Tokens are stored hashed with argon2id. To rotate, revoke the old one and
create a new one.

## Tool reference

### Runs

- `list_runs(status?, flow_id?, limit?, offset?)` — list runs newest-first
- `get_run(id)` — run + last 200 events
- `get_run_events(id, limit?, offset?)` — raw events oldest-first
- `cancel_run(id)` — kill a running job (SIGKILL to ffmpeg)
- `rerun_run(id)` — enqueue a new job from this one's flow + file

### Flows

- `list_flows()`
- `get_flow(id)` — YAML + parsed AST
- `create_flow({name, yaml})`
- `update_flow({id, yaml, enabled?})`
- `delete_flow({id, confirm: true})`
- `dry_run_flow({yaml, file_path, probe?})` — walk the AST without execution

### Sources

- `list_sources()` — secret tokens redacted to `***`
- `get_source(id)`
- `create_source({kind, name, config, secret_token})`
- `update_source({id, name?, config?, secret_token?})`
- `delete_source({id, confirm: true})`

### Notifiers

- `list_notifiers()` — secret-bearing config keys redacted
- `get_notifier(id)`
- `create_notifier({name, kind, config})`
- `update_notifier({id, name, kind, config})`
- `delete_notifier({id, confirm: true})`
- `test_notifier(id)`

### System

- `get_health()` → `{healthy, ready}`
- `get_queue()` → `{pending: [], running: []}`
- `get_hw_caps()` — NVENC/QSV/VAAPI/VideoToolbox detection snapshot
- `get_metrics()` — Prometheus exposition (text passthrough)

## Worked example

> "Retry every failed run from the last 24 hours."

The AI does roughly:

1. `list_runs(status: "failed", limit: 500)` → filter results by `created_at > now - 86400`
2. For each id, `rerun_run(id)`
3. `get_queue()` to confirm they entered pending state.

## Errors

The binary maps HTTP responses to MCP errors:

| HTTP    | MCP code           | Meaning                                       |
| ------- | ------------------ | --------------------------------------------- |
| 400     | `INVALID_PARAMS`   | bad arguments — message has details           |
| 401     | `AUTH_FAILED`      | token rejected; check `TRANSCODERR_TOKEN`     |
| 403     | `FORBIDDEN`        | (rare)                                        |
| 404     | `NOT_FOUND`        | resource doesn't exist                        |
| 409     | `CONFLICT`         | uniqueness violation (e.g. flow name in use)  |
| 5xx     | `INTERNAL`         | server error; check server logs               |
| network | `UNREACHABLE`      | could not connect to `TRANSCODERR_URL`        |

## Logging

The binary logs to **stderr** (stdout is the MCP protocol). Set
`RUST_LOG=transcoderr_mcp=debug` to see request/response details. Tokens
are never logged.
```

- [ ] **Step 4: Commit**

```bash
git add README.md docs/mcp.md
git commit -m "docs: README MCP section and docs/mcp.md tool reference"
```

---

## Final verification

Before declaring done, run the full test suite end-to-end:

- [ ] **Run all tests**

Run: `cargo test --workspace`
Expected: every test in every crate passes.

- [ ] **Run a manual smoke test**

```bash
# Terminal A
cargo run -p transcoderr -- serve --config /tmp/test-config.toml
# (configure auth.enabled=true and create a token via the UI first)

# Terminal B — drive the binary by hand
TRANSCODERR_URL=http://localhost:8080 TRANSCODERR_TOKEN=tcr_... \
  cargo run -p transcoderr-mcp
# Then paste a JSON-RPC initialize request followed by a tools/list call
```

Expected: server-info handshake completes, tools/list returns all ~25 tool definitions with their JSON schemas.

- [ ] **Final commit if needed**

If anything in the manual smoke test required a fix, commit it here. Otherwise nothing to do.
