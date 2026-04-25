# transcoderr Phase 4 — Web UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a single-page web app (TypeScript + React + Vite) embedded in the binary that talks to a typed JSON API and one SSE stream. Six top-level pages: Dashboard, Flows (with Monaco YAML + visual mirror + dry-run), Runs, Sources, Plugins, Settings. Optional auth gate (single-user password).

**Architecture:** A new `src/api/` module exposes a typed JSON API on top of the existing DB layer. SSE stream broadcasts `JobState`, `RunEvent`, `QueueSnapshot`, `CapsUpdate`. Frontend lives in `web/` (Vite + React). At build time a `build.rs` (or a Cargo feature with `include_dir!`) embeds the compiled `web/dist/` into the binary; Axum serves it at `/`.

**Tech Stack:** Rust side — Axum, `tokio::sync::broadcast` for SSE fan-out. Frontend — TypeScript, React 18, Vite, TanStack Query, Zustand, Monaco editor, recharts, react-router. Auth — argon2 for password hashing, axum cookie session.

---

## Scope

**In:**
- JSON API: `/api/flows`, `/api/runs`, `/api/jobs/:id`, `/api/sources`, `/api/plugins`, `/api/notifiers`, `/api/settings`, `/api/dry-run`, `/api/auth/{login,logout,me}`
- SSE: `GET /api/stream` with broadcast topics
- Dry-run: probe a file, walk the flow simulating conditions, report which steps would run + with what params
- React SPA with six pages (Dashboard, Flows, Runs, Sources, Plugins, Settings)
- Monaco editor for YAML, JSON-schema-driven completion (schemas from each plugin manifest)
- Visual mirror: re-render on YAML AST change (debounced)
- Auth: optional, off by default; argon2 password hash stored in `settings` table; session cookie when on
- Mobile responsive: Dashboard + Run detail; Flow editor explicitly desktop-only with notice
- Embedded static assets via `include_dir`

**Out:**
- Drag-and-drop visual editing → not in v1
- Multi-user / OIDC → not in v1
- Prometheus / retention / Docker → Phase 5

---

## File Structure (delta)

```
migrations/
  20260425000004_phase4_settings.sql            (settings + sessions tables)
build.rs                                          Builds web/ before compiling Rust (optional)
src/
  api/
    mod.rs                                       Routes mounted under /api
    flows.rs                                     CRUD + parse-validate
    runs.rs                                      list, detail, cancel, rerun
    jobs.rs                                      single-job detail (events stream)
    sources.rs                                   CRUD
    plugins.rs                                   list (read-only — plugins are filesystem-driven)
    notifiers.rs                                 CRUD + test-fire
    settings.rs                                  read/update bootstrap-overridable settings
    dryrun.rs                                    POST /api/dry-run
    auth.rs                                      login/logout/me + middleware
  bus/
    mod.rs                                       broadcast channels for SSE
    sse.rs                                       SSE handler
  static_assets.rs                               include_dir wrapper for web/dist
web/
  package.json
  tsconfig.json
  vite.config.ts
  index.html
  src/
    main.tsx
    app.tsx                                      Router + sidebar shell
    api/client.ts                                Typed fetch helpers
    api/sse.ts                                   SSE subscriber + Zustand bridge
    pages/
      dashboard.tsx
      flows-list.tsx
      flow-detail.tsx                            tabs: editor, test, history, runs
      runs-list.tsx
      run-detail.tsx
      sources.tsx
      plugins.tsx
      settings.tsx
      login.tsx
    components/
      sidebar.tsx
      yaml-editor.tsx                            Monaco wrapper + schema integration
      flow-mirror.tsx                            renders parsed AST as a tree
      run-timeline.tsx                           vertical timeline of run_events
      live-progress.tsx                          progress bar wired to SSE
      notify-form.tsx
    state/
      live.ts                                    Zustand store, SSE-fed
    types.ts                                     mirrors Rust API DTOs
```

---

## Tasks

### Task 1: Migration — settings, sessions

**Files:**
- Create: `migrations/20260425000004_phase4_settings.sql`

- [ ] **Step 1: Write migration**

```sql
CREATE TABLE settings (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

INSERT INTO settings (key, value) VALUES
  ('auth.enabled', 'false'),
  ('auth.password_hash', ''),
  ('worker.pool_size', '2'),
  ('retention.events_days', '30'),
  ('retention.jobs_days', '90'),
  ('dedup.window_seconds', '300');

CREATE TABLE sessions (
  id          TEXT PRIMARY KEY,
  created_at  INTEGER NOT NULL,
  expires_at  INTEGER NOT NULL
);
```

- [ ] **Step 2: Run migrate test**

Run: `cargo test db::tests::opens_and_migrates`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add migrations/20260425000004_phase4_settings.sql
git commit -m "feat(db): settings + sessions tables"
```

---

### Task 2: Settings DAL + Auth (argon2 password + session middleware)

**Files:**
- Create: `src/db/settings.rs`
- Create: `src/api/auth.rs`
- Modify: `Cargo.toml`
- Modify: `src/api/mod.rs`
- Create: `tests/api_auth.rs`

- [ ] **Step 1: Add deps**

Add to `[dependencies]`:

```toml
argon2 = "0.5"
rand = "0.8"
tower-cookies = "0.10"
```

- [ ] **Step 2: Settings DAL**

Create `src/db/settings.rs`:

```rust
use sqlx::SqlitePool;

pub async fn get(pool: &SqlitePool, key: &str) -> anyhow::Result<Option<String>> {
    Ok(sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key = ?")
        .bind(key).fetch_optional(pool).await?)
}

pub async fn set(pool: &SqlitePool, key: &str, value: &str) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES (?, ?) \
         ON CONFLICT (key) DO UPDATE SET value = excluded.value"
    ).bind(key).bind(value).execute(pool).await?;
    Ok(())
}
```

Add `pub mod settings;` to `src/db/mod.rs`.

- [ ] **Step 3: Auth handlers + middleware**

Create `src/api/auth.rs`:

```rust
use crate::{db, http::AppState};
use argon2::{password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString}, Argon2};
use axum::{extract::State, http::StatusCode, Json};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use tower_cookies::{Cookie, Cookies};

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

    // Create session
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

/// Used at first-run config to set the password.
pub fn hash_password(p: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Ok(Argon2::default().hash_password(p.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("hash: {e}"))?
        .to_string())
}
```

- [ ] **Step 4: Mount auth routes (skip middleware on auth/* and webhooks)**

Create `src/api/mod.rs`:

```rust
pub mod auth;

use crate::http::AppState;
use axum::{middleware::from_fn_with_state, routing::{get, post}, Router};
use tower_cookies::CookieManagerLayer;

pub fn router(state: AppState) -> Router<AppState> {
    let public = Router::new()
        .route("/auth/login",  post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/me",     get(auth::me));
    let protected = Router::new()
        // (mount the rest in subsequent tasks)
        .route_layer(from_fn_with_state(state.clone(), auth::require_auth));
    public.merge(protected).layer(CookieManagerLayer::new())
}
```

Update `src/http/mod.rs::router` to nest `.nest("/api", crate::api::router(state.clone()))`.

- [ ] **Step 5: Login test**

Create `tests/api_auth.rs`:

```rust
mod common;
use common::boot;
use serde_json::json;
use transcoderr::{api::auth::hash_password, db};

#[tokio::test]
async fn login_with_correct_password_succeeds() {
    let app = boot().await;
    let h = hash_password("hunter2").unwrap();
    db::settings::set(&app.pool, "auth.enabled", "true").await.unwrap();
    db::settings::set(&app.pool, "auth.password_hash", &h).await.unwrap();

    let client = reqwest::Client::builder().cookie_store(true).build().unwrap();
    let bad = client.post(format!("{}/api/auth/login", app.url))
        .json(&json!({"password":"wrong"})).send().await.unwrap();
    assert_eq!(bad.status(), 401);

    let ok = client.post(format!("{}/api/auth/login", app.url))
        .json(&json!({"password":"hunter2"})).send().await.unwrap();
    assert!(ok.status().is_success());

    let me: serde_json::Value = client.get(format!("{}/api/auth/me", app.url))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(me["authed"], true);
}
```

Run: `cargo test --test api_auth`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock src/db/ src/api/ src/http/mod.rs src/lib.rs tests/api_auth.rs
git commit -m "feat(api): settings DAL, argon2 auth, session middleware"
```

---

### Task 3: API — Flows CRUD + parse/validate endpoint

**Files:**
- Create: `src/api/flows.rs`
- Modify: `src/api/mod.rs`
- Create: `tests/api_flows.rs`

- [ ] **Step 1: DTOs + handlers**

Create `src/api/flows.rs`:

```rust
use crate::{db, flow::parse_flow, http::AppState};
use axum::{extract::{Path, State}, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use sqlx::Row;

#[derive(Serialize)]
pub struct FlowSummary {
    pub id: i64, pub name: String, pub enabled: bool, pub version: i64
}

#[derive(Serialize)]
pub struct FlowDetail {
    pub id: i64, pub name: String, pub enabled: bool, pub version: i64,
    pub yaml_source: String, pub parsed_json: serde_json::Value,
}

#[derive(Deserialize)]
pub struct CreateFlowReq { pub name: String, pub yaml: String }

#[derive(Deserialize)]
pub struct UpdateFlowReq { pub yaml: String, pub enabled: Option<bool> }

#[derive(Serialize)]
pub struct ParseResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parsed: Option<serde_json::Value>,
}

pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<FlowSummary>>, StatusCode> {
    let rows = sqlx::query("SELECT id, name, enabled, version FROM flows ORDER BY name")
        .fetch_all(&state.pool).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let out = rows.into_iter().map(|r| FlowSummary {
        id: r.get(0), name: r.get(1), enabled: r.get::<i64,_>(2) != 0, version: r.get(3)
    }).collect();
    Ok(Json(out))
}

pub async fn get(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Json<FlowDetail>, StatusCode> {
    let row = sqlx::query("SELECT id, name, enabled, version, yaml_source, parsed_json FROM flows WHERE id = ?")
        .bind(id).fetch_optional(&state.pool).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(FlowDetail {
        id: row.get(0), name: row.get(1), enabled: row.get::<i64,_>(2) != 0, version: row.get(3),
        yaml_source: row.get(4),
        parsed_json: serde_json::from_str(row.get::<&str, _>(5)).unwrap_or_default(),
    }))
}

pub async fn create(State(state): State<AppState>, Json(req): Json<CreateFlowReq>) -> Result<Json<FlowSummary>, StatusCode> {
    let parsed = parse_flow(&req.yaml).map_err(|_| StatusCode::BAD_REQUEST)?;
    let id = db::flows::insert(&state.pool, &req.name, &req.yaml, &parsed).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(FlowSummary { id, name: req.name, enabled: true, version: 1 }))
}

pub async fn update(State(state): State<AppState>, Path(id): Path<i64>, Json(req): Json<UpdateFlowReq>) -> Result<StatusCode, StatusCode> {
    let parsed = parse_flow(&req.yaml).map_err(|_| StatusCode::BAD_REQUEST)?;
    let parsed_json = serde_json::to_string(&parsed).unwrap();
    let mut tx = state.pool.begin().await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let cur: i64 = sqlx::query_scalar("SELECT version FROM flows WHERE id = ?")
        .bind(id).fetch_one(&mut *tx).await.map_err(|_| StatusCode::NOT_FOUND)?;
    let next = cur + 1;
    sqlx::query("UPDATE flows SET yaml_source = ?, parsed_json = ?, version = ?, updated_at = strftime('%s','now') WHERE id = ?")
        .bind(&req.yaml).bind(&parsed_json).bind(next).bind(id)
        .execute(&mut *tx).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    sqlx::query("INSERT INTO flow_versions (flow_id, version, yaml_source, created_at) VALUES (?, ?, ?, strftime('%s','now'))")
        .bind(id).bind(next).bind(&req.yaml)
        .execute(&mut *tx).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if let Some(en) = req.enabled {
        sqlx::query("UPDATE flows SET enabled = ? WHERE id = ?")
            .bind(if en { 1 } else { 0 }).bind(id)
            .execute(&mut *tx).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }
    tx.commit().await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete(State(state): State<AppState>, Path(id): Path<i64>) -> Result<StatusCode, StatusCode> {
    sqlx::query("DELETE FROM flows WHERE id = ?").bind(id).execute(&state.pool).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn parse(Json(yaml): Json<String>) -> Json<ParseResult> {
    match parse_flow(&yaml) {
        Ok(f) => Json(ParseResult { ok: true, error: None, parsed: Some(serde_json::to_value(f).unwrap()) }),
        Err(e) => Json(ParseResult { ok: false, error: Some(e.to_string()), parsed: None }),
    }
}
```

- [ ] **Step 2: Mount routes**

In `src/api/mod.rs::router`, append to `protected`:

```rust
.route("/flows",       get(flows::list).post(flows::create))
.route("/flows/:id",   get(flows::get).put(flows::update).delete(flows::delete))
.route("/flows/parse", post(flows::parse))
```

Add `pub mod flows;` to `src/api/mod.rs`.

- [ ] **Step 3: Test**

Create `tests/api_flows.rs` mirroring the auth test pattern: list (empty), create, get, update, list again — assert versioning bumped from 1 → 2.

Run: `cargo test --test api_flows`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/api/flows.rs src/api/mod.rs tests/api_flows.rs
git commit -m "feat(api): flows CRUD with versioning + parse endpoint"
```

---

### Task 4: API — Runs/jobs/sources/plugins/notifiers/settings/dry-run

**Files:**
- Create: `src/api/runs.rs`, `src/api/jobs.rs`, `src/api/sources.rs`, `src/api/plugins.rs`, `src/api/notifiers.rs`, `src/api/settings.rs`, `src/api/dryrun.rs`
- Modify: `src/api/mod.rs`
- Create: `tests/api_misc.rs`

Each follows the pattern established in Task 3. Below: signatures + key endpoints. Implementations are short SQL queries against the existing tables.

- [ ] **Step 1: Runs**

Create `src/api/runs.rs` — list, get-by-job-id, cancel-running, rerun (clones the job into a new pending row). Implementation pattern follows `flows.rs`.

Endpoints:
```
GET    /api/runs                     ?status=&flow_id=&limit=&offset=
GET    /api/runs/:job_id             → run detail (job + flow snapshot + last 200 events)
GET    /api/runs/:job_id/events      → all events for a job (paginated)
POST   /api/runs/:job_id/cancel
POST   /api/runs/:job_id/rerun       ?with_current_flow_version=true
```

- [ ] **Step 2: Jobs**

Create `src/api/jobs.rs` — minimal: `GET /api/jobs/:id` for the SSE handshake to validate.

- [ ] **Step 3: Sources**

Create `src/api/sources.rs` — CRUD + `POST /api/sources/:id/test-fire` (records a synthetic webhook event that creates a no-op test job).

- [ ] **Step 4: Plugins**

Create `src/api/plugins.rs` — read-only list of discovered plugins (with their schemas). Toggle enabled (writes `plugins.enabled`).

- [ ] **Step 5: Notifiers**

Create `src/api/notifiers.rs` — CRUD + `POST /api/notifiers/:id/test` to send a test message.

- [ ] **Step 6: Settings**

Create `src/api/settings.rs` — get all, patch one. Special handling: setting `auth.enabled=true` requires also providing a password (rehashed and stored).

- [ ] **Step 7: Dry-run**

Create `src/api/dryrun.rs`:

```rust
use crate::{flow::{expr, parse_flow, Context, Node}, http::AppState};
use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct DryRunReq {
    pub yaml: String,
    pub file_path: String,
    /// Optional precomputed probe data (so the request can simulate without disk).
    pub probe: Option<serde_json::Value>,
}

#[derive(Serialize)]
pub struct DryRunStep {
    pub id: Option<String>,
    pub kind: &'static str,    // "step" | "if-true" | "if-false" | "return"
    pub use_or_label: String,
    pub with: Option<serde_json::Value>,
}

#[derive(Serialize)]
pub struct DryRunResp { pub steps: Vec<DryRunStep>, pub probe: serde_json::Value }

pub async fn dry_run(State(_state): State<AppState>, Json(req): Json<DryRunReq>) -> Json<DryRunResp> {
    let flow = match parse_flow(&req.yaml) {
        Ok(f) => f,
        Err(e) => return Json(DryRunResp { steps: vec![DryRunStep {
            id: None, kind: "step", use_or_label: format!("parse error: {e}"), with: None
        }], probe: serde_json::Value::Null }),
    };
    // Build a context. If no probe provided, hit ffprobe on the file.
    let probe = match req.probe {
        Some(p) => p,
        None => crate::ffmpeg::ffprobe_json(std::path::Path::new(&req.file_path)).await
            .unwrap_or(serde_json::Value::Null),
    };
    let mut ctx = Context::for_file(&req.file_path);
    ctx.probe = Some(probe.clone());

    let mut out = vec![];
    walk(&flow.steps, &mut ctx, &mut out);
    Json(DryRunResp { steps: out, probe })
}

fn walk(nodes: &[Node], ctx: &mut Context, out: &mut Vec<DryRunStep>) {
    for n in nodes {
        match n {
            Node::Step { id, use_, with, .. } => out.push(DryRunStep {
                id: id.clone(), kind: "step", use_or_label: use_.clone(),
                with: Some(serde_json::to_value(with).unwrap()),
            }),
            Node::Conditional { id, if_, then_, else_ } => {
                let v = expr::eval_bool(if_, ctx).unwrap_or(false);
                let kind = if v { "if-true" } else { "if-false" };
                out.push(DryRunStep { id: id.clone(), kind, use_or_label: if_.clone(), with: None });
                if v { walk(then_, ctx, out); }
                else if let Some(e) = else_ { walk(e, ctx, out); }
            }
            Node::Return { return_ } => {
                out.push(DryRunStep { id: None, kind: "return", use_or_label: return_.clone(), with: None });
                return;
            }
        }
    }
}
```

- [ ] **Step 8: Mount all routes**

Append to `src/api/mod.rs::router` `protected`:

```rust
.route("/runs",                       get(runs::list))
.route("/runs/:id",                   get(runs::get))
.route("/runs/:id/events",            get(runs::events))
.route("/runs/:id/cancel",            post(runs::cancel))
.route("/runs/:id/rerun",             post(runs::rerun))
.route("/jobs/:id",                   get(jobs::get))
.route("/sources",                    get(sources::list).post(sources::create))
.route("/sources/:id",                get(sources::get).put(sources::update).delete(sources::delete))
.route("/sources/:id/test-fire",      post(sources::test_fire))
.route("/plugins",                    get(plugins::list))
.route("/plugins/:id",                axum::routing::patch(plugins::update))
.route("/notifiers",                  get(notifiers::list).post(notifiers::create))
.route("/notifiers/:id",              get(notifiers::get).put(notifiers::update).delete(notifiers::delete))
.route("/notifiers/:id/test",         post(notifiers::test))
.route("/settings",                   get(settings::get_all).patch(settings::patch))
.route("/dry-run",                    post(dryrun::dry_run))
```

Add the corresponding `pub mod` lines.

- [ ] **Step 9: Test**

Create `tests/api_misc.rs` with one happy-path test per endpoint group (list-empty / list-after-create / detail). Aim for breadth, not depth.

Run: `cargo test --test api_misc`
Expected: PASS.

- [ ] **Step 10: Commit**

```bash
git add src/api/ tests/api_misc.rs
git commit -m "feat(api): runs/jobs/sources/plugins/notifiers/settings/dry-run"
```

---

### Task 5: SSE bus + stream endpoint

**Files:**
- Create: `src/bus/mod.rs`
- Create: `src/bus/sse.rs`
- Modify: `src/http/mod.rs`
- Modify: `src/db/run_events.rs` (broadcast on append)
- Modify: `src/db/jobs.rs` (broadcast on status change)
- Create: `tests/sse.rs`

- [ ] **Step 1: Bus**

Create `src/bus/mod.rs`:

```rust
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "topic", content = "data")]
pub enum Event {
    JobState { id: i64, status: String, label: Option<String> },
    RunEvent { job_id: i64, step_id: Option<String>, kind: String, payload: serde_json::Value },
    Queue    { pending: i64, running: i64 },
    Caps     { /* opaque, from /api/hw */ },
}

#[derive(Clone)]
pub struct Bus {
    pub tx: broadcast::Sender<Event>,
}

impl Default for Bus {
    fn default() -> Self {
        let (tx, _) = broadcast::channel(1024);
        Self { tx }
    }
}
```

Add to `src/http/mod.rs::AppState`: `pub bus: crate::bus::Bus`.

- [ ] **Step 2: Hook DB layer to broadcast**

Modify `db::run_events::append` to take `&AppState` (or `&Bus`) and emit a `RunEvent` after insert. Same for `db::jobs::set_status`. Update all call sites — including the engine.

The cleanest delta: add overloaded `append_with_bus` and `set_status_with_bus` and call those from the engine + worker. Existing tests that use the bus-less variants keep working.

- [ ] **Step 3: SSE handler**

Create `src/bus/sse.rs`:

```rust
use crate::http::AppState;
use axum::{extract::State, response::sse::{Event, KeepAlive, Sse}};
use futures::stream::{self, StreamExt};
use std::convert::Infallible;
use std::time::Duration;
use tokio_stream::wrappers::BroadcastStream;

pub async fn stream(State(state): State<AppState>) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let rx = state.bus.tx.subscribe();
    let s = BroadcastStream::new(rx).filter_map(|res| async move {
        match res {
            Ok(ev) => Some(Ok(Event::default().json_data(ev).unwrap())),
            Err(_) => None, // lagged; drop
        }
    });
    let initial = stream::once(async { Ok(Event::default().comment("connected")) });
    Sse::new(initial.chain(s)).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}
```

Mount: `protected.route("/stream", get(crate::bus::sse::stream))`.

Add tokio-stream + futures deps:

```toml
tokio-stream = { version = "0.1", features = ["sync"] }
futures = "0.3"
```

- [ ] **Step 4: Test**

Create `tests/sse.rs` — start the app, open the SSE stream, fire a job that completes, assert at least one `JobState` and one `RunEvent` arrive in the stream within ~5s.

Run: `cargo test --test sse`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/bus/ src/http/mod.rs src/db/run_events.rs src/db/jobs.rs src/api/mod.rs tests/sse.rs
git commit -m "feat(api): broadcast bus + SSE stream endpoint"
```

---

### Task 6: Frontend bootstrap (Vite + React + TS)

**Files:**
- Create: `web/package.json`, `web/tsconfig.json`, `web/vite.config.ts`, `web/index.html`
- Create: `web/src/main.tsx`, `web/src/app.tsx`
- Create: `web/src/api/client.ts`, `web/src/api/sse.ts`
- Create: `web/src/types.ts`
- Create: `web/.gitignore`

- [ ] **Step 1: Initialize Vite app**

Run: `cd web && npm create vite@latest . -- --template react-ts && npm install`

This creates the boilerplate. Replace `web/src/main.tsx` and `App.tsx` in subsequent steps.

- [ ] **Step 2: Add core deps**

Run: `cd web && npm install @tanstack/react-query zustand react-router-dom @monaco-editor/react recharts`

- [ ] **Step 3: vite.config.ts (proxy /api during dev)**

Replace `web/vite.config.ts`:

```ts
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      "/api": "http://localhost:8080",
      "/webhook": "http://localhost:8080",
    },
  },
  build: { outDir: "dist", sourcemap: true },
});
```

- [ ] **Step 4: API client + types**

Create `web/src/types.ts`:

```ts
export type FlowSummary = { id: number; name: string; enabled: boolean; version: number };
export type FlowDetail  = FlowSummary & { yaml_source: string; parsed_json: any };
export type RunRow      = { id: number; flow_id: number; status: string; created_at: number; finished_at?: number };
export type RunEvent    = { id: number; job_id: number; ts: number; step_id?: string; kind: string; payload?: any };
export type Source      = { id: number; kind: string; name: string };
export type Notifier    = { id: number; name: string; kind: string; config: any };
export type Plugin      = { id: number; name: string; version: string; kind: string; enabled: boolean; schema: any };
```

Create `web/src/api/client.ts`:

```ts
const base = "/api";

async function req<T>(path: string, init: RequestInit = {}): Promise<T> {
  const r = await fetch(base + path, { credentials: "same-origin", headers: { "content-type": "application/json", ...init.headers }, ...init });
  if (!r.ok) throw new Error(`${r.status} ${r.statusText}: ${await r.text()}`);
  if (r.status === 204) return undefined as T;
  return r.json();
}

export const api = {
  flows: {
    list:   () => req<import("../types").FlowSummary[]>("/flows"),
    get:    (id: number) => req<import("../types").FlowDetail>(`/flows/${id}`),
    create: (body: { name: string; yaml: string }) => req<{ id: number }>("/flows", { method: "POST", body: JSON.stringify(body) }),
    update: (id: number, body: { yaml: string; enabled?: boolean }) => req<void>(`/flows/${id}`, { method: "PUT", body: JSON.stringify(body) }),
    parse:  (yaml: string) => req<{ ok: boolean; error?: string; parsed?: any }>("/flows/parse", { method: "POST", body: JSON.stringify(yaml) }),
  },
  runs: {
    list:   (params?: { status?: string; flow_id?: number; limit?: number; offset?: number }) => {
      const q = new URLSearchParams(Object.entries(params ?? {}).filter(([,v]) => v != null).map(([k,v]) => [k, String(v)])).toString();
      return req<import("../types").RunRow[]>(`/runs${q ? `?${q}` : ""}`);
    },
    get:    (id: number) => req<{ run: import("../types").RunRow; events: import("../types").RunEvent[] }>(`/runs/${id}`),
    cancel: (id: number) => req<void>(`/runs/${id}/cancel`, { method: "POST" }),
    rerun:  (id: number) => req<{ id: number }>(`/runs/${id}/rerun`, { method: "POST" }),
  },
  // ...sources, plugins, notifiers, settings, dryRun, auth analogous
  dryRun: (body: { yaml: string; file_path: string; probe?: any }) =>
    req<{ steps: any[]; probe: any }>("/dry-run", { method: "POST", body: JSON.stringify(body) }),
  auth: {
    me:     () => req<{ auth_required: boolean; authed: boolean }>("/auth/me"),
    login:  (password: string) => req<void>("/auth/login", { method: "POST", body: JSON.stringify({ password }) }),
    logout: () => req<void>("/auth/logout", { method: "POST" }),
  },
};
```

- [ ] **Step 5: SSE subscriber**

Create `web/src/api/sse.ts`:

```ts
type Event =
  | { topic: "JobState"; data: { id: number; status: string; label?: string } }
  | { topic: "RunEvent"; data: { job_id: number; step_id?: string; kind: string; payload: any } }
  | { topic: "Queue";    data: { pending: number; running: number } };

export function connectSSE(onEvent: (e: Event) => void): () => void {
  const es = new EventSource("/api/stream", { withCredentials: true });
  es.onmessage = (m) => {
    try { onEvent(JSON.parse(m.data)); } catch {}
  };
  return () => es.close();
}
```

- [ ] **Step 6: Verify dev server runs**

Run: `cd web && npm run dev`
Expected: opens on localhost:5173, proxies /api to :8080.

- [ ] **Step 7: Commit**

```bash
git add web/
git commit -m "feat(web): Vite + React + TS bootstrap and API/SSE clients"
```

---

### Task 7: App shell + sidebar + auth gate

**Files:**
- Replace: `web/src/main.tsx`, `web/src/app.tsx`
- Create: `web/src/components/sidebar.tsx`, `web/src/pages/login.tsx`

- [ ] **Step 1: main.tsx**

Replace `web/src/main.tsx`:

```tsx
import React from "react";
import ReactDOM from "react-dom/client";
import { BrowserRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import App from "./app";
import "./index.css";

const qc = new QueryClient();

ReactDOM.createRoot(document.getElementById("root")!).render(
  <QueryClientProvider client={qc}>
    <BrowserRouter><App /></BrowserRouter>
  </QueryClientProvider>
);
```

- [ ] **Step 2: app.tsx with routes + auth guard**

Replace `web/src/app.tsx`:

```tsx
import { Routes, Route, Navigate } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { api } from "./api/client";
import Sidebar from "./components/sidebar";
import Dashboard from "./pages/dashboard";
import FlowsList from "./pages/flows-list";
import FlowDetail from "./pages/flow-detail";
import RunsList from "./pages/runs-list";
import RunDetail from "./pages/run-detail";
import Sources from "./pages/sources";
import Plugins from "./pages/plugins";
import Settings from "./pages/settings";
import Login from "./pages/login";

export default function App() {
  const me = useQuery({ queryKey: ["me"], queryFn: api.auth.me });
  if (me.isLoading) return null;
  if (me.data?.auth_required && !me.data?.authed) return <Login onLoggedIn={() => me.refetch()} />;
  return (
    <div style={{ display: "flex", height: "100vh" }}>
      <Sidebar />
      <main style={{ flex: 1, overflow: "auto" }}>
        <Routes>
          <Route path="/" element={<Navigate to="/dashboard" />} />
          <Route path="/dashboard" element={<Dashboard />} />
          <Route path="/flows" element={<FlowsList />} />
          <Route path="/flows/:id" element={<FlowDetail />} />
          <Route path="/runs" element={<RunsList />} />
          <Route path="/runs/:id" element={<RunDetail />} />
          <Route path="/sources" element={<Sources />} />
          <Route path="/plugins" element={<Plugins />} />
          <Route path="/settings" element={<Settings />} />
        </Routes>
      </main>
    </div>
  );
}
```

- [ ] **Step 3: Sidebar + Login**

Create `web/src/components/sidebar.tsx` — vertical nav with the six links.

Create `web/src/pages/login.tsx` — simple form posting `password` to `/api/auth/login`.

(For brevity I'm not pasting all CSS — use simple flex layout + spacing. Each component is ~30-50 lines.)

- [ ] **Step 4: Manually verify shell loads**

Run: `cd web && npm run dev` → open browser. With auth disabled, should show empty Dashboard.

- [ ] **Step 5: Commit**

```bash
git add web/src/main.tsx web/src/app.tsx web/src/components/sidebar.tsx web/src/pages/login.tsx
git commit -m "feat(web): app shell with sidebar, routing, and auth gate"
```

---

### Task 8: Pages — Dashboard + Runs list/detail

**Files:**
- Create: `web/src/state/live.ts`
- Create: `web/src/pages/dashboard.tsx`, `web/src/pages/runs-list.tsx`, `web/src/pages/run-detail.tsx`
- Create: `web/src/components/run-timeline.tsx`, `web/src/components/live-progress.tsx`

- [ ] **Step 1: Live store wired to SSE**

Create `web/src/state/live.ts`:

```ts
import { create } from "zustand";
import { connectSSE } from "../api/sse";

type Live = {
  queue: { pending: number; running: number };
  jobStatus: Record<number, { status: string; label?: string }>;
  jobProgress: Record<number, { pct?: number; lastStepId?: string }>;
};
export const useLive = create<Live>(() => ({
  queue: { pending: 0, running: 0 },
  jobStatus: {},
  jobProgress: {},
}));

export function startSSE() {
  return connectSSE((e) => {
    if (e.topic === "Queue") useLive.setState({ queue: e.data });
    if (e.topic === "JobState") {
      useLive.setState((s) => ({ jobStatus: { ...s.jobStatus, [e.data.id]: { status: e.data.status, label: e.data.label } } }));
    }
    if (e.topic === "RunEvent" && e.data.kind === "progress") {
      const pct = e.data.payload?.pct;
      if (pct != null) useLive.setState((s) => ({
        jobProgress: { ...s.jobProgress, [e.data.job_id]: { pct, lastStepId: e.data.step_id } }
      }));
    }
  });
}
```

In `app.tsx`, call `startSSE()` once in a `useEffect`.

- [ ] **Step 2: Dashboard**

Create `web/src/pages/dashboard.tsx`:

```tsx
import { useEffect } from "react";
import { useLive, startSSE } from "../state/live";
import { useQuery } from "@tanstack/react-query";
import { api } from "../api/client";

export default function Dashboard() {
  useEffect(() => { const stop = startSSE(); return stop; }, []);
  const live = useLive();
  const recent = useQuery({ queryKey: ["runs", "recent"], queryFn: () => api.runs.list({ limit: 10 }) });

  return (
    <div style={{ padding: 24 }}>
      <h2>Dashboard</h2>
      <div style={{ display: "grid", gridTemplateColumns: "repeat(4, 1fr)", gap: 12 }}>
        <Tile label="Queue" value={live.queue.pending} />
        <Tile label="Running" value={live.queue.running} />
        <Tile label="Recent runs" value={recent.data?.length ?? 0} />
        <Tile label="Failures (24h)" value={recent.data?.filter(r => r.status === "failed").length ?? 0} />
      </div>
      <h3 style={{ marginTop: 24 }}>Recent activity</h3>
      <table>
        <thead><tr><th>ID</th><th>Status</th><th>Progress</th><th>Created</th></tr></thead>
        <tbody>
          {(recent.data ?? []).map(r => (
            <tr key={r.id}>
              <td><a href={`/runs/${r.id}`}>{r.id}</a></td>
              <td>{live.jobStatus[r.id]?.status ?? r.status}</td>
              <td>{live.jobProgress[r.id]?.pct ? `${live.jobProgress[r.id].pct!.toFixed(1)}%` : ""}</td>
              <td>{new Date(r.created_at*1000).toLocaleString()}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function Tile({ label, value }: { label: string; value: number }) {
  return (
    <div style={{ background: "rgba(255,255,255,0.05)", padding: 16, borderRadius: 8 }}>
      <div style={{ fontSize: 12, opacity: 0.7 }}>{label}</div>
      <div style={{ fontSize: 32, fontWeight: 700 }}>{value}</div>
    </div>
  );
}
```

- [ ] **Step 3: Runs list + detail**

Create `web/src/pages/runs-list.tsx` — paginated table with filter dropdowns (status, flow). Renders rows clickable into `/runs/:id`.

Create `web/src/pages/run-detail.tsx` — fetches `api.runs.get(id)`; renders run header (status, duration), then `<RunTimeline events={…} />` + a `<LiveProgress jobId={id} />` strip if status is `running`.

Create `web/src/components/run-timeline.tsx`:

```tsx
import { RunEvent } from "../types";
export default function RunTimeline({ events }: { events: RunEvent[] }) {
  return (
    <ol style={{ listStyle: "none", padding: 0 }}>
      {events.map(e => (
        <li key={e.id} style={{ borderLeft: "2px solid #444", paddingLeft: 12, marginBottom: 6 }}>
          <code style={{ opacity: 0.7 }}>{new Date(e.ts*1000).toISOString()}</code>{" "}
          <strong>{e.kind}</strong>{" "}
          {e.step_id && <span style={{ opacity: 0.8 }}>· {e.step_id}</span>}
          {e.payload && <pre style={{ marginTop: 4 }}>{JSON.stringify(e.payload, null, 2)}</pre>}
        </li>
      ))}
    </ol>
  );
}
```

Create `web/src/components/live-progress.tsx` — pulls progress from `useLive().jobProgress[jobId]` and renders a bar.

- [ ] **Step 4: Smoke**

Run: `cd web && npm run dev`. With backend running and a seeded flow + a manually-fired webhook, observe Dashboard tiles update in realtime as the run progresses.

- [ ] **Step 5: Commit**

```bash
git add web/src/state/ web/src/pages/dashboard.tsx web/src/pages/runs-list.tsx web/src/pages/run-detail.tsx web/src/components/
git commit -m "feat(web): Dashboard and Runs pages with live SSE"
```

---

### Task 9: Pages — Flows list + detail (Monaco + visual mirror + dry-run tab)

**Files:**
- Create: `web/src/pages/flows-list.tsx`
- Create: `web/src/pages/flow-detail.tsx`
- Create: `web/src/components/yaml-editor.tsx`
- Create: `web/src/components/flow-mirror.tsx`

- [ ] **Step 1: Flows list**

Create `web/src/pages/flows-list.tsx` — table with columns: name, enabled, version, last-run-status. Buttons: New flow (opens modal with name+empty YAML), enable/disable toggle.

- [ ] **Step 2: YAML editor wrapper**

Create `web/src/components/yaml-editor.tsx`:

```tsx
import Editor from "@monaco-editor/react";
import { useEffect, useRef } from "react";

export default function YamlEditor({
  value, onChange, schema,
}: {
  value: string;
  onChange: (v: string) => void;
  schema?: any;
}) {
  const editorRef = useRef<any>(null);

  useEffect(() => {
    if (!editorRef.current || !schema) return;
    // Monaco YAML lang server schema integration is provided by 'monaco-yaml'.
    // For Phase 4 we configure JSON-schema validation only; YAML schema is a polish item.
  }, [schema]);

  return (
    <Editor
      height="60vh"
      language="yaml"
      theme="vs-dark"
      value={value}
      onChange={(v) => onChange(v ?? "")}
      onMount={(e) => { editorRef.current = e; }}
      options={{ minimap: { enabled: false }, tabSize: 2 }}
    />
  );
}
```

- [ ] **Step 3: Flow mirror**

Create `web/src/components/flow-mirror.tsx` — receives `parsed` (the flow AST as JSON) and renders a vertical tree:

```tsx
export default function FlowMirror({ parsed }: { parsed: any }) {
  if (!parsed) return null;
  return (
    <div>
      <div style={{ fontWeight: 600 }}>{parsed.name}</div>
      <Steps nodes={parsed.steps} />
    </div>
  );
}

function Steps({ nodes }: { nodes: any[] }) {
  return (
    <ul>{nodes.map((n, i) => <li key={i}>{describe(n)}{n.then ? <Steps nodes={n.then}/> : null}{n.else ? <Steps nodes={n.else}/> : null}</li>)}</ul>
  );
}

function describe(n: any): string {
  if (n.use) return `▶ ${n.use}${n.id ? ` (${n.id})` : ""}`;
  if (n.if  != null) return `? if ${n.if}`;
  if (n.return != null) return `← return ${n.return}`;
  return JSON.stringify(n);
}
```

- [ ] **Step 4: Flow detail page (tabs)**

Create `web/src/pages/flow-detail.tsx`:

```tsx
import { useParams } from "react-router-dom";
import { useState } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../api/client";
import YamlEditor from "../components/yaml-editor";
import FlowMirror from "../components/flow-mirror";

export default function FlowDetail() {
  const { id } = useParams();
  const qc = useQueryClient();
  const flow = useQuery({ queryKey: ["flow", id], queryFn: () => api.flows.get(Number(id)) });
  const [yaml, setYaml] = useState<string>("");
  const [tab, setTab] = useState<"editor"|"test"|"history">("editor");
  const [parseResult, setParseResult] = useState<any>(null);

  if (flow.data && yaml === "") setYaml(flow.data.yaml_source);

  // Live parse for the visual mirror
  const debouncedYaml = useDebounced(yaml, 200);
  useEffectAsync(async () => {
    if (!debouncedYaml) return;
    try { setParseResult(await api.flows.parse(debouncedYaml)); } catch {}
  }, [debouncedYaml]);

  const save = useMutation({
    mutationFn: () => api.flows.update(Number(id), { yaml }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["flow", id] }),
  });

  return (
    <div style={{ padding: 24 }}>
      <h2>{flow.data?.name}</h2>
      <div style={{ display: "flex", gap: 12, marginBottom: 12 }}>
        {(["editor","test","history"] as const).map(t =>
          <button key={t} onClick={() => setTab(t)} style={{ fontWeight: tab===t?700:400 }}>{t}</button>
        )}
        <button onClick={() => save.mutate()} disabled={save.isPending}>Save</button>
      </div>
      {tab === "editor" && (
        <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 12 }}>
          <YamlEditor value={yaml} onChange={setYaml} />
          <div>
            {parseResult?.ok
              ? <FlowMirror parsed={parseResult.parsed} />
              : <pre style={{ color: "#f88" }}>{parseResult?.error ?? ""}</pre>}
          </div>
        </div>
      )}
      {tab === "test" && <DryRunPane yaml={yaml} />}
      {tab === "history" && <p>history view (Phase 5 polish)</p>}
    </div>
  );
}

function DryRunPane({ yaml }: { yaml: string }) {
  const [path, setPath] = useState("");
  const [result, setResult] = useState<any>(null);
  return (
    <div>
      <input value={path} onChange={e => setPath(e.target.value)} placeholder="/path/to/file.mkv" style={{ width: "60%" }} />
      <button onClick={async () => setResult(await api.dryRun({ yaml, file_path: path }))}>Test</button>
      {result && (
        <ol>{result.steps.map((s: any, i: number) => <li key={i}>{s.kind}: {s.use_or_label}</li>)}</ol>
      )}
    </div>
  );
}

// helpers
import { useEffect, useRef } from "react";
function useDebounced<T>(v: T, ms: number) {
  const [out, set] = useState(v);
  useEffect(() => { const t = setTimeout(() => set(v), ms); return () => clearTimeout(t); }, [v, ms]);
  return out;
}
function useEffectAsync(fn: () => Promise<void>, deps: any[]) {
  useEffect(() => { fn().catch(() => {}); /* eslint-disable-next-line */ }, deps);
}
```

- [ ] **Step 5: Smoke test**

Run dev server, open a flow, edit YAML, watch the mirror update live.

- [ ] **Step 6: Commit**

```bash
git add web/src/pages/flows-list.tsx web/src/pages/flow-detail.tsx web/src/components/yaml-editor.tsx web/src/components/flow-mirror.tsx
git commit -m "feat(web): Flows list and detail with Monaco editor and visual mirror"
```

---

### Task 10: Pages — Sources, Plugins, Settings

**Files:**
- Create: `web/src/pages/sources.tsx`, `web/src/pages/plugins.tsx`, `web/src/pages/settings.tsx`

- [ ] **Step 1: Sources**

Create `web/src/pages/sources.tsx` — list with kind, name, generated webhook URL (for `kind=webhook`) + bearer token (masked, click to reveal). Add/Edit form with kind selector. Test-fire button.

- [ ] **Step 2: Plugins**

Create `web/src/pages/plugins.tsx` — list discovered plugins with name, version, kind, enable toggle, "view schema" expander. Read-only beyond the toggle.

- [ ] **Step 3: Settings**

Create `web/src/pages/settings.tsx` — sections:

- General: pool size, dedup window
- Auth: enable toggle + password field
- Retention: events/jobs days
- Notifiers: list + add/edit
- System info: read-only — pulls `/api/hw` and shows ffmpeg version + devices.

- [ ] **Step 4: Smoke**

Browse all five non-Dashboard pages. Verify CRUD round-trips work end-to-end.

- [ ] **Step 5: Commit**

```bash
git add web/src/pages/sources.tsx web/src/pages/plugins.tsx web/src/pages/settings.tsx
git commit -m "feat(web): Sources, Plugins, Settings pages"
```

---

### Task 11: Embed `web/dist` into the binary

**Files:**
- Modify: `Cargo.toml`
- Create: `src/static_assets.rs`
- Modify: `src/http/mod.rs`
- Modify: `build.rs` (or document npm build step)

- [ ] **Step 1: Add include_dir**

Add to `[dependencies]`:

```toml
include_dir = "0.7"
mime_guess = "2"
```

- [ ] **Step 2: Static asset handler**

Create `src/static_assets.rs`:

```rust
use axum::{body::Body, http::{header, StatusCode, Uri}, response::Response};
use include_dir::{include_dir, Dir};

static DIST: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/web/dist");

pub async fn serve(uri: Uri) -> Result<Response<Body>, StatusCode> {
    let path = uri.path().trim_start_matches('/');
    let candidate = if path.is_empty() { "index.html" } else { path };
    let file = DIST.get_file(candidate).or_else(|| DIST.get_file("index.html"))
        .ok_or(StatusCode::NOT_FOUND)?;
    let mime = mime_guess::from_path(candidate).first_or_octet_stream();
    Ok(Response::builder()
        .header(header::CONTENT_TYPE, mime.as_ref())
        .body(Body::from(file.contents()))
        .unwrap())
}
```

- [ ] **Step 3: Mount fallback**

In `src/http/mod.rs::router`:

```rust
.fallback(crate::static_assets::serve)
```

- [ ] **Step 4: Build orchestration**

Document in README that the user (or CI) must run `npm --prefix web ci && npm --prefix web run build` before `cargo build --release` so `web/dist/` exists.

Optionally add a `build.rs`:

```rust
fn main() {
    println!("cargo:rerun-if-changed=web/src");
    println!("cargo:rerun-if-changed=web/index.html");
    if std::path::Path::new("web/dist").exists() { return; }
    let st = std::process::Command::new("npm").args(["--prefix", "web", "ci"]).status();
    if st.map(|s| s.success()).unwrap_or(false) {
        let _ = std::process::Command::new("npm").args(["--prefix", "web", "run", "build"]).status();
    }
}
```

- [ ] **Step 5: Verify embedded**

Build with frontend: `npm --prefix web run build && cargo build --release`. Run the binary, hit `http://localhost:8080/` — SPA loads from the binary alone.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock src/static_assets.rs src/http/mod.rs build.rs README.md
git commit -m "feat: embed compiled web/dist via include_dir"
```

---

### Task 12: Mobile responsive polish

**Files:**
- Modify: `web/src/app.tsx`
- Modify: a couple of CSS files

- [ ] **Step 1: Detect narrow viewport**

In `app.tsx`, if `window.innerWidth < 640`, hide the sidebar behind a toggle. Add a top app-bar with a hamburger.

- [ ] **Step 2: Flow editor desktop-only banner**

In `flow-detail.tsx`, render a banner if narrow:

```tsx
{window.innerWidth < 1024 && <div style={{ background: "#822", padding: 8 }}>The flow editor is desktop-only. Open this page on a wider screen.</div>}
```

- [ ] **Step 3: Smoke at narrow viewport**

Test in browser devtools at iPhone size; Dashboard + Runs should be usable.

- [ ] **Step 4: Commit**

```bash
git add web/src/app.tsx web/src/pages/flow-detail.tsx
git commit -m "feat(web): mobile responsive polish; flow editor flagged desktop-only"
```

---

## Self-review checklist (Phase 4)

- [ ] JSON API endpoints from spec → Tasks 3, 4
- [ ] SSE stream → Task 5
- [ ] Dashboard with live tiles → Task 8
- [ ] Flows page with Monaco + visual mirror + dry-run → Task 9
- [ ] Runs page with timeline + progress → Task 8
- [ ] Sources, Plugins, Settings pages → Task 10
- [ ] Auth toggle + password → Task 2
- [ ] Mobile responsive on Dashboard + Run detail → Task 12
- [ ] Web assets embedded into binary → Task 11
- [ ] No placeholders that block execution; later steps reference earlier types correctly
