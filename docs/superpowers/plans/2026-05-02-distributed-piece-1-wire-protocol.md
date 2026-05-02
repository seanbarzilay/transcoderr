# Distributed Transcoding — Piece 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the connection layer for distributed transcoding end-to-end with zero job dispatch — workers can connect, register, heartbeat, and appear in a Workers UI; the local in-process worker pool keeps doing all the actual work.

**Architecture:** New `transcoderr worker` subcommand on the same binary opens a WebSocket to the coordinator's `/api/worker/connect`, sends a one-shot `register` payload with hw caps + plugin manifest, then heartbeats every 30s. Coordinator persists the registration into a new `workers` table and updates `last_seen_at` on every heartbeat; an idle sweep marks workers stale after 90s. Auth is Bearer token validated against `workers.secret_token`. The existing in-process worker pool is left alone (Piece 2 wires it through the registry); only the file at `crates/transcoderr/src/worker.rs` moves into a `worker/` directory so the new daemon code can live alongside it.

**Tech Stack:** Rust 2021, axum 0.7 + new `ws` feature, sqlx + sqlite, `tokio-tungstenite = "0.24"` for the worker-side client, serde + serde_json for protocol JSON, anyhow for errors. React 18, TypeScript, TanStack Query v5 — same as the existing web app.

**Branch:** all tasks land on a fresh `feat/distributed-piece-1` branch off `main`. Implementer creates the branch before Task 1.

---

## File Structure

**Module layout reorg (Task 1):**
- `crates/transcoderr/src/worker.rs` → `crates/transcoderr/src/worker/pool.rs` (verbatim move)
- New `crates/transcoderr/src/worker/mod.rs` — re-exports `pub use pool::*` so existing `use crate::worker::Worker` in `main.rs` and `tests/common/mod.rs` keeps resolving

**New backend files:**
- `crates/transcoderr/migrations/20260502000001_workers.sql` — new migration (datestamp matches the existing `20260...` pattern)
- `crates/transcoderr/src/db/workers.rs` — typed CRUD for the `workers` table + token verify
- `crates/transcoderr/src/worker/protocol.rs` — `Envelope`, `Register`, `RegisterAck`, `Heartbeat` message types (shared between worker and coordinator)
- `crates/transcoderr/src/worker/connection.rs` — WS client + reconnect-with-backoff loop
- `crates/transcoderr/src/worker/daemon.rs` — daemon orchestration: hw probe, plugin discover, dial, register, heartbeat
- `crates/transcoderr/src/worker/config.rs` — `WorkerConfig` TOML struct
- `crates/transcoderr/src/api/workers.rs` — REST CRUD (`/api/workers`) + WS handler (`/api/worker/connect`) + idle sweep task
- `crates/transcoderr/tests/worker_connect.rs` — integration test, 4 scenarios

**New web files:**
- `web/src/pages/workers.tsx` — Workers UI page
- `web/src/components/forms/add-worker.tsx` — "Add worker" button → token mint modal
- `web/src/types.ts` — add `Worker` type alongside the existing types

**Modified backend files:**
- `crates/transcoderr/Cargo.toml` — add `ws` to axum's feature list, add `tokio-tungstenite = "0.24"` (worker-only via cfg-feature gating is overkill; just add as a regular dep — it's small)
- `crates/transcoderr/src/lib.rs` — `pub mod worker;` is already there; nothing changes
- `crates/transcoderr/src/main.rs` — add `Worker { config: PathBuf }` Cmd variant + dispatch
- `crates/transcoderr/src/api/mod.rs` — register the new routes
- `crates/transcoderr/src/api/auth.rs` — extend `require_auth` to also accept tokens from `workers.secret_token` (Task 7)
- `crates/transcoderr/src/db/mod.rs` — `pub mod workers;`

**Modified web files:**
- `web/src/App.tsx` — add `/workers` route
- `web/src/components/sidebar.tsx` — add "Workers" sidebar entry between "Plugins" and "Settings"
- `web/src/api/client.ts` — typed wrappers `api.workers.{list, create, delete}`

**Docs:**
- `README.md` — short mention of `transcoderr worker` mode
- `docs/deploy.md` — section on running a worker, including the reverse-proxy `Upgrade` header note

---

## Task 1: Module layout reorg (worker.rs → worker/pool.rs)

Pure refactor. Existing in-process worker pool keeps its exact semantics; only its file location changes. Adding a directory at the same path as a `.rs` file is a Rust module-resolution conflict, so this has to happen before Task 2.

**Files:**
- Move (verbatim): `crates/transcoderr/src/worker.rs` → `crates/transcoderr/src/worker/pool.rs`
- Create: `crates/transcoderr/src/worker/mod.rs`

- [ ] **Step 1: Branch verification + git mv the file**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-1" || { echo "WRONG BRANCH"; exit 1; }
mkdir -p crates/transcoderr/src/worker
git mv crates/transcoderr/src/worker.rs crates/transcoderr/src/worker/pool.rs
```

`git mv` preserves history.

- [ ] **Step 2: Create `worker/mod.rs` with re-exports**

```rust
//! Worker module. Pre-distributed-transcoding (Piece 1) this just held
//! the in-process job-claim pool at `pool.rs`. The Piece 1 wire
//! protocol skeleton adds `daemon.rs`, `connection.rs`, `protocol.rs`,
//! and `config.rs` as siblings; later pieces wire the local pool
//! through the same registration mechanism remote workers use.
//!
//! `pool::*` is re-exported so existing `use crate::worker::Worker`
//! callsites keep resolving without churn.

pub mod pool;

pub use pool::*;
```

- [ ] **Step 3: Build smoke**

```bash
cargo build -p transcoderr 2>&1 | tail -10
```

Expected: clean build. The two existing call-sites (`main.rs:112` uses `transcoderr::worker::Worker::new(...)`; `tests/common/mod.rs` uses `Worker::new`) keep resolving via the re-export.

- [ ] **Step 4: Run the existing test suite to confirm no regressions**

```bash
cargo test -p transcoderr 2>&1 | grep -E "test result|FAILED" | head -30
```

Expected: every `test result: ok.` line. No FAILED.

- [ ] **Step 5: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-1" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/worker/
git commit -m "refactor(worker): promote worker.rs to worker/pool.rs (no behaviour change)"
```

---

## Task 2: DB migration + `db/workers.rs` CRUD

The `workers` table holds local + remote worker rows. Local row is seeded by the migration. Piece 1 doesn't write to `jobs.worker_id` / `run_events.worker_id`; the columns are added now so Piece 2's local-worker refactor doesn't need its own migration.

**Files:**
- Create: `crates/transcoderr/migrations/20260502000001_workers.sql`
- Create: `crates/transcoderr/src/db/workers.rs`
- Modify: `crates/transcoderr/src/db/mod.rs`

- [ ] **Step 1: Write the migration**

```sql
-- crates/transcoderr/migrations/20260502000001_workers.sql

CREATE TABLE workers (
    id           INTEGER PRIMARY KEY,
    name         TEXT NOT NULL,
    kind         TEXT NOT NULL,            -- 'local' | 'remote'
    secret_token TEXT,                     -- NULL for the local worker row
    hw_caps_json TEXT,                     -- last register payload
    plugin_manifest_json TEXT,             -- last register payload
    enabled      INTEGER NOT NULL DEFAULT 1,
    last_seen_at INTEGER,                  -- unix seconds; NULL = never
    created_at   INTEGER NOT NULL
);

CREATE UNIQUE INDEX idx_workers_secret_token
    ON workers(secret_token)
    WHERE secret_token IS NOT NULL;

ALTER TABLE jobs       ADD COLUMN worker_id INTEGER REFERENCES workers(id);
ALTER TABLE run_events ADD COLUMN worker_id INTEGER;

INSERT INTO workers (name, kind, enabled, created_at)
VALUES ('local', 'local', 1, strftime('%s', 'now'));
```

The unique index on `secret_token` (with `WHERE secret_token IS NOT NULL` so multiple NULLs are allowed) is what makes token lookup safe — there's at most one row per token.

- [ ] **Step 2: Create `crates/transcoderr/src/db/workers.rs`**

```rust
//! CRUD for the `workers` table.
//!
//! Tokens are stored verbatim (matching the existing sources/notifiers
//! pattern at `db/sources.rs`) — they're random 32-byte hex strings,
//! not user-chosen, so the hashed-bcrypt path used by `db/api_tokens.rs`
//! buys nothing here.

use serde::Serialize;
use sqlx::{FromRow, SqlitePool};

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct WorkerRow {
    pub id: i64,
    pub name: String,
    pub kind: String, // 'local' | 'remote'
    #[sqlx(default)]
    pub secret_token: Option<String>,
    #[sqlx(default)]
    pub hw_caps_json: Option<String>,
    #[sqlx(default)]
    pub plugin_manifest_json: Option<String>,
    pub enabled: i64,
    #[sqlx(default)]
    pub last_seen_at: Option<i64>,
    pub created_at: i64,
}

/// Insert a new remote worker. Returns its id.
pub async fn insert_remote(
    pool: &SqlitePool,
    name: &str,
    secret_token: &str,
) -> anyhow::Result<i64> {
    let id: (i64,) = sqlx::query_as(
        "INSERT INTO workers (name, kind, secret_token, enabled, created_at)
         VALUES (?, 'remote', ?, 1, strftime('%s','now'))
         RETURNING id",
    )
    .bind(name)
    .bind(secret_token)
    .fetch_one(pool)
    .await?;
    Ok(id.0)
}

pub async fn list_all(pool: &SqlitePool) -> anyhow::Result<Vec<WorkerRow>> {
    Ok(sqlx::query_as(
        "SELECT id, name, kind, secret_token, hw_caps_json, plugin_manifest_json,
                enabled, last_seen_at, created_at
           FROM workers
          ORDER BY id",
    )
    .fetch_all(pool)
    .await?)
}

pub async fn get_by_id(pool: &SqlitePool, id: i64) -> anyhow::Result<Option<WorkerRow>> {
    Ok(sqlx::query_as(
        "SELECT id, name, kind, secret_token, hw_caps_json, plugin_manifest_json,
                enabled, last_seen_at, created_at
           FROM workers WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

/// Find a worker by its (verbatim) secret token. Used by the auth path
/// and by the WS upgrade handler.
pub async fn get_by_token(
    pool: &SqlitePool,
    token: &str,
) -> anyhow::Result<Option<WorkerRow>> {
    Ok(sqlx::query_as(
        "SELECT id, name, kind, secret_token, hw_caps_json, plugin_manifest_json,
                enabled, last_seen_at, created_at
           FROM workers WHERE secret_token = ?",
    )
    .bind(token)
    .fetch_optional(pool)
    .await?)
}

/// Delete a remote worker by id. Refuses to touch the local row.
pub async fn delete_remote(pool: &SqlitePool, id: i64) -> anyhow::Result<u64> {
    let res = sqlx::query("DELETE FROM workers WHERE id = ? AND kind = 'remote'")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// Stamp the worker's last-seen timestamp + last register payload after a
/// successful register frame.
pub async fn record_register(
    pool: &SqlitePool,
    id: i64,
    hw_caps_json: &str,
    plugin_manifest_json: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE workers
            SET hw_caps_json         = ?,
                plugin_manifest_json = ?,
                last_seen_at         = strftime('%s','now')
          WHERE id = ?",
    )
    .bind(hw_caps_json)
    .bind(plugin_manifest_json)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Stamp last_seen_at on a heartbeat or any other live frame.
pub async fn record_heartbeat(pool: &SqlitePool, id: i64) -> anyhow::Result<()> {
    sqlx::query("UPDATE workers SET last_seen_at = strftime('%s','now') WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    async fn pool() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let pool = crate::db::open(dir.path()).await.unwrap();
        (pool, dir)
    }

    #[tokio::test]
    async fn local_row_is_seeded_by_migration() {
        let (pool, _dir) = pool().await;
        let rows = list_all(&pool).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "local");
        assert_eq!(rows[0].kind, "local");
        assert!(rows[0].secret_token.is_none());
        assert_eq!(rows[0].enabled, 1);
    }

    #[tokio::test]
    async fn insert_remote_returns_id_and_appears_in_list() {
        let (pool, _dir) = pool().await;
        let id = insert_remote(&pool, "gpu-box-1", "wkr_abcdef").await.unwrap();
        let rows = list_all(&pool).await.unwrap();
        assert_eq!(rows.len(), 2); // local + new remote
        assert!(rows.iter().any(|r| r.id == id && r.kind == "remote"));
    }

    #[tokio::test]
    async fn get_by_token_finds_remote_only() {
        let (pool, _dir) = pool().await;
        insert_remote(&pool, "gpu-box-1", "wkr_secret_xyz").await.unwrap();
        let found = get_by_token(&pool, "wkr_secret_xyz").await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "gpu-box-1");
        assert!(get_by_token(&pool, "nope").await.unwrap().is_none());
        // The local row has NULL secret_token so it's not findable by any value.
        assert!(get_by_token(&pool, "").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn delete_remote_refuses_local_row() {
        let (pool, _dir) = pool().await;
        let removed = delete_remote(&pool, 1).await.unwrap(); // id=1 is the seeded local row
        assert_eq!(removed, 0);
        let rows = list_all(&pool).await.unwrap();
        assert_eq!(rows.len(), 1); // local row still there
    }

    #[tokio::test]
    async fn record_register_stamps_payload_and_last_seen() {
        let (pool, _dir) = pool().await;
        let id = insert_remote(&pool, "w", "tok").await.unwrap();
        record_register(&pool, id, r#"{"encoders":[]}"#, r#"[]"#).await.unwrap();
        let row = get_by_id(&pool, id).await.unwrap().unwrap();
        assert_eq!(row.hw_caps_json.as_deref(), Some(r#"{"encoders":[]}"#));
        assert_eq!(row.plugin_manifest_json.as_deref(), Some(r#"[]"#));
        assert!(row.last_seen_at.is_some());
    }
}
```

- [ ] **Step 3: Wire `db/workers.rs` into `db/mod.rs`**

Read the current `crates/transcoderr/src/db/mod.rs`. Find the existing `pub mod ...;` declaration block and add (alphabetically near the others — after `pub mod sources;` if present, otherwise wherever modules are declared):

```rust
pub mod workers;
```

- [ ] **Step 4: Run the new tests**

```bash
cargo test -p transcoderr --lib db::workers 2>&1 | tail -10
```

Expected: 5 tests pass.

- [ ] **Step 5: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-1" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/migrations/20260502000001_workers.sql \
        crates/transcoderr/src/db/workers.rs \
        crates/transcoderr/src/db/mod.rs
git commit -m "feat(db): workers table + CRUD"
```

---

## Task 3: Protocol types (`worker/protocol.rs`)

Shared between worker and coordinator. Pure data + serde — no logic, no IO. JSON round-trip tested.

**Files:**
- Create: `crates/transcoderr/src/worker/protocol.rs`
- Modify: `crates/transcoderr/src/worker/mod.rs` (add `pub mod protocol;`)

- [ ] **Step 1: Create `worker/protocol.rs`**

```rust
//! Wire envelope + message variants for the worker ↔ coordinator
//! WebSocket protocol.
//!
//! Envelope shape:
//!   { "type": "<kind>", "id": "<uuid>", "payload": {...} }
//!
//! All frames are JSON text. Binary frames are reserved for future use.
//! `id` is a worker-side correlation id for request/response pairs
//! (e.g. register ↔ register_ack); for fire-and-forget messages
//! (heartbeat, the future step_progress) it's still a unique id but
//! the receiver doesn't reply.
//!
//! Piece 1 ships only three message types: `register`,
//! `register_ack`, `heartbeat`. Pieces 3 and 4 add the dispatch + plugin
//! sync variants.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", tag = "type", content = "payload")]
pub enum Message {
    Register(Register),
    RegisterAck(RegisterAck),
    Heartbeat(Heartbeat),
}

/// Wire frame: the message variant plus its correlation id.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Envelope {
    pub id: String,
    #[serde(flatten)]
    pub message: Message,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Register {
    pub name: String,
    pub version: String,
    pub hw_caps: serde_json::Value,
    /// List of step kinds this worker can run. Piece 1 reports a fixed
    /// set; Piece 3 will trim it based on hw + plugins.
    pub available_steps: Vec<String>,
    /// Installed plugins on this worker. Piece 1 reports the discovered
    /// set; Piece 4 makes the coordinator drive this state.
    pub plugin_manifest: Vec<PluginManifestEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginManifestEntry {
    pub name: String,
    pub version: String,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RegisterAck {
    pub worker_id: i64,
    /// Plugins the coordinator wants this worker to have installed.
    /// Piece 1 sends an empty list; Piece 4 fills it in.
    pub plugin_install: Vec<PluginInstall>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginInstall {
    pub name: String,
    pub version: String,
    pub sha256: String,
    pub tarball_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Heartbeat {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn round_trip(env: &Envelope) -> Envelope {
        let s = serde_json::to_string(env).unwrap();
        serde_json::from_str(&s).unwrap()
    }

    #[test]
    fn register_round_trips() {
        let env = Envelope {
            id: "abc".into(),
            message: Message::Register(Register {
                name: "gpu-box-1".into(),
                version: "0.31.0".into(),
                hw_caps: json!({"encoders": ["h264_nvenc"]}),
                available_steps: vec!["plan.execute".into()],
                plugin_manifest: vec![PluginManifestEntry {
                    name: "size-report".into(),
                    version: "0.1.2".into(),
                    sha256: Some("abc123".into()),
                }],
            }),
        };
        assert_eq!(round_trip(&env), env);
    }

    #[test]
    fn register_ack_round_trips() {
        let env = Envelope {
            id: "abc".into(),
            message: Message::RegisterAck(RegisterAck {
                worker_id: 42,
                plugin_install: vec![],
            }),
        };
        assert_eq!(round_trip(&env), env);
    }

    #[test]
    fn heartbeat_round_trips() {
        let env = Envelope {
            id: "h1".into(),
            message: Message::Heartbeat(Heartbeat {}),
        };
        assert_eq!(round_trip(&env), env);
    }

    #[test]
    fn envelope_uses_snake_case_type_tag() {
        // Lock the wire format down: protocol consumers (including the
        // test fixtures and a future Go/Python worker reimplementation)
        // depend on `register_ack` not `RegisterAck`.
        let env = Envelope {
            id: "x".into(),
            message: Message::RegisterAck(RegisterAck {
                worker_id: 1,
                plugin_install: vec![],
            }),
        };
        let s = serde_json::to_string(&env).unwrap();
        assert!(s.contains(r#""type":"register_ack""#), "got: {s}");
    }
}
```

- [ ] **Step 2: Add `pub mod protocol;` to `worker/mod.rs`**

Edit `crates/transcoderr/src/worker/mod.rs` so it reads:

```rust
//! Worker module. Pre-distributed-transcoding (Piece 1) this just held
//! the in-process job-claim pool at `pool.rs`. The Piece 1 wire
//! protocol skeleton adds `daemon.rs`, `connection.rs`, `protocol.rs`,
//! and `config.rs` as siblings; later pieces wire the local pool
//! through the same registration mechanism remote workers use.
//!
//! `pool::*` is re-exported so existing `use crate::worker::Worker`
//! callsites keep resolving without churn.

pub mod pool;
pub mod protocol;

pub use pool::*;
```

- [ ] **Step 3: Run the new tests**

```bash
cargo test -p transcoderr --lib worker::protocol 2>&1 | tail -10
```

Expected: 4 tests pass.

- [ ] **Step 4: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-1" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/worker/protocol.rs crates/transcoderr/src/worker/mod.rs
git commit -m "feat(worker): protocol types for register / register_ack / heartbeat"
```

---

## Task 4: REST endpoints — `GET/POST/DELETE /api/workers`

Coordinator-side mint + list + delete. No WebSocket yet.

**Files:**
- Create: `crates/transcoderr/src/api/workers.rs`
- Modify: `crates/transcoderr/src/api/mod.rs`

- [ ] **Step 1: Create `crates/transcoderr/src/api/workers.rs`**

```rust
//! REST endpoints for the workers registry. The WebSocket upgrade
//! handler (`/api/worker/connect`) lives in this same file (added in
//! Task 5) — one file per resource matches the existing
//! `api/sources.rs` / `api/notifiers.rs` pattern.

use crate::api::auth::AuthSource;
use crate::db;
use crate::http::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use rand::RngCore;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct WorkerSummary {
    pub id: i64,
    pub name: String,
    pub kind: String,
    /// Redacted to "***" for token-authed callers; clear text for
    /// session callers (the UI). Local-worker rows always serialize as
    /// `null` since they have no token.
    pub secret_token: Option<String>,
    pub hw_caps: Option<serde_json::Value>,
    pub plugin_manifest: Option<serde_json::Value>,
    pub enabled: bool,
    pub last_seen_at: Option<i64>,
    pub created_at: i64,
}

fn row_to_summary(row: db::workers::WorkerRow, redact: bool) -> WorkerSummary {
    WorkerSummary {
        id: row.id,
        name: row.name,
        kind: row.kind,
        secret_token: row.secret_token.map(|t| if redact { "***".to_string() } else { t }),
        hw_caps: row
            .hw_caps_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok()),
        plugin_manifest: row
            .plugin_manifest_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok()),
        enabled: row.enabled != 0,
        last_seen_at: row.last_seen_at,
        created_at: row.created_at,
    }
}

pub async fn list(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthSource>,
) -> Result<Json<Vec<WorkerSummary>>, StatusCode> {
    let rows = db::workers::list_all(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let redact = auth == AuthSource::Token;
    Ok(Json(rows.into_iter().map(|r| row_to_summary(r, redact)).collect()))
}

#[derive(serde::Deserialize)]
pub struct CreateReq {
    pub name: String,
}

#[derive(Serialize)]
pub struct CreateResp {
    pub id: i64,
    /// One-time-display: this is the only response that ever contains
    /// the cleartext token. Subsequent reads return `***`.
    pub secret_token: String,
}

/// Mint a new remote worker. Returns `{id, secret_token}` once;
/// subsequent reads via `/api/workers` redact the token for token-authed
/// callers. The token is a 32-byte hex string (matches the format used
/// by the auto-provisioned *arr secret_tokens at `api/sources.rs`).
pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<CreateReq>,
) -> Result<Json<CreateResp>, StatusCode> {
    if req.name.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();

    let id = db::workers::insert_remote(&state.pool, &req.name, &token)
        .await
        .map_err(|e| {
            tracing::error!(error = ?e, "failed to insert worker row");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(CreateResp { id, secret_token: token }))
}

pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    let removed = db::workers::delete_remote(&state.pool, id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if removed == 0 {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(StatusCode::NO_CONTENT)
}
```

- [ ] **Step 2: Confirm `rand` is in deps**

```bash
grep -nE "^rand\s*=" crates/transcoderr/Cargo.toml /Users/seanbarzilay/projects/private/transcoderr/Cargo.toml
```

Expected: at least one match (the existing radarr/sonarr auto-provision flow at `api/sources.rs:68-70` uses `rand::thread_rng().fill_bytes`, so it's already a dep).

- [ ] **Step 3: Register the routes**

Read `crates/transcoderr/src/api/mod.rs`. Find the `protected` Router builder (around line 28-50, where `flows`, `runs`, etc. are registered). Add to the import list at the top of the file (alphabetical):

```rust
use crate::api::workers;
```

Add to the `protected` chain (anywhere among the other resources is fine; pick alphabetical order — between `verify_playable` and the closing chain):

```rust
        .route("/workers",            get(workers::list).post(workers::create))
        .route("/workers/:id",        delete(workers::delete))
```

Add `pub mod workers;` near the other `pub mod ...;` declarations in the same file.

- [ ] **Step 4: Build smoke**

```bash
cargo build -p transcoderr 2>&1 | tail -5
```

Expected: clean build.

- [ ] **Step 5: Manual smoke (optional but recommended)**

Start the binary against a fresh data dir and curl:

```bash
mkdir -p /tmp/p1-test && cat > /tmp/p1-test/config.toml <<'EOF'
bind = "127.0.0.1:8080"
data_dir = "/tmp/p1-test"
[radarr]
bearer_token = "test"
EOF
./target/debug/transcoderr serve --config /tmp/p1-test/config.toml &
SERVER_PID=$!
sleep 2

curl -s http://127.0.0.1:8080/api/workers
# expect: [{"id":1,"name":"local","kind":"local",...}]

curl -s -X POST http://127.0.0.1:8080/api/workers -H "Content-Type: application/json" -d '{"name":"gpu-box-1"}'
# expect: {"id":2,"secret_token":"<64 hex chars>"}

curl -s -X DELETE http://127.0.0.1:8080/api/workers/2 -o /dev/null -w "%{http_code}\n"
# expect: 204

kill $SERVER_PID
rm -rf /tmp/p1-test
```

- [ ] **Step 6: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-1" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/api/workers.rs crates/transcoderr/src/api/mod.rs
git commit -m "feat(api): /api/workers REST CRUD + token mint"
```

---

## Task 5: WebSocket upgrade handler at `/api/worker/connect`

Coordinator-side WS handling: token validation, `register` round-trip, heartbeat consumption, idle sweep that marks workers stale.

**Files:**
- Modify: `crates/transcoderr/Cargo.toml` — add `"ws"` to axum's features
- Modify: `crates/transcoderr/src/api/workers.rs` — add the `connect` handler
- Modify: `crates/transcoderr/src/api/mod.rs` — register the route in the **public** router (the WS handler does its own token auth; the standard `require_auth` middleware doesn't know about Bearer-on-Upgrade)
- Modify: `crates/transcoderr/src/main.rs` — spawn the idle sweep task at boot

- [ ] **Step 1: Enable axum's `ws` feature**

In `crates/transcoderr/Cargo.toml`, find:

```toml
axum = { version = "0.7", features = ["macros"] }
```

Change to:

```toml
axum = { version = "0.7", features = ["macros", "ws"] }
```

- [ ] **Step 2: Append the `connect` handler + idle sweep to `api/workers.rs`**

Add at the bottom of `crates/transcoderr/src/api/workers.rs`:

```rust
// --- WebSocket upgrade -----------------------------------------------------

use crate::worker::protocol::{Envelope, Heartbeat, Message, Register, RegisterAck};
use axum::{
    extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade},
    http::HeaderMap,
    response::Response,
};
use std::time::Duration;

/// Window we wait for the worker to send its `register` after the WS
/// upgrade completes. Anything beyond this is treated as a misbehaving
/// client and the connection closes.
const REGISTER_TIMEOUT: Duration = Duration::from_secs(5);

/// Workers older than this without an inbound frame are marked stale by
/// the idle sweep task. Stays loose to absorb a missed heartbeat or two
/// over a flaky link.
pub const STALE_AFTER_SECS: i64 = 90;

/// GET /api/worker/connect — upgrade to WebSocket. Auth is Bearer-on-the
/// upgrade-request (workers don't have session cookies). Token must
/// match a row in the `workers` table.
pub async fn connect(
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, StatusCode> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?
        .to_string();

    let row = db::workers::get_by_token(&state.pool, &token)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::UNAUTHORIZED)?;

    Ok(ws.on_upgrade(move |socket| handle_connection(state, socket, row.id)))
}

async fn handle_connection(state: AppState, mut socket: WebSocket, worker_id: i64) {
    // 1. Wait up to REGISTER_TIMEOUT for the `register` frame.
    let register = match tokio::time::timeout(REGISTER_TIMEOUT, recv_message(&mut socket)).await {
        Ok(Ok(Envelope { id, message: Message::Register(r) })) => (id, r),
        _ => {
            tracing::warn!(worker_id, "no valid register within {REGISTER_TIMEOUT:?}; closing");
            let _ = socket.close().await;
            return;
        }
    };
    let (correlation_id, register_payload) = register;

    // 2. Persist the registration.
    let hw_caps_json = serde_json::to_string(&register_payload.hw_caps).unwrap_or_else(|_| "null".into());
    let plugin_manifest_json =
        serde_json::to_string(&register_payload.plugin_manifest).unwrap_or_else(|_| "[]".into());
    if let Err(e) = db::workers::record_register(
        &state.pool,
        worker_id,
        &hw_caps_json,
        &plugin_manifest_json,
    )
    .await
    {
        tracing::error!(worker_id, error = ?e, "failed to record register");
        let _ = socket.close().await;
        return;
    }

    // 3. Send the register_ack with the same correlation id.
    let ack = Envelope {
        id: correlation_id,
        message: Message::RegisterAck(RegisterAck {
            worker_id,
            plugin_install: vec![], // Piece 4 fills this in
        }),
    };
    if !send_message(&mut socket, &ack).await {
        return;
    }

    tracing::info!(worker_id, name = %register_payload.name, "worker registered");

    // 4. Receive loop. Piece 1 only handles heartbeats; Pieces 3+ add
    //    step_progress / step_complete.
    while let Ok(env) = recv_message(&mut socket).await {
        match env.message {
            Message::Heartbeat(_) => {
                if let Err(e) = db::workers::record_heartbeat(&state.pool, worker_id).await {
                    tracing::warn!(worker_id, error = ?e, "failed to record heartbeat");
                }
            }
            other => {
                tracing::warn!(worker_id, ?other, "unexpected message; ignoring");
            }
        }
    }
    tracing::info!(worker_id, "worker disconnected");
}

async fn recv_message(socket: &mut WebSocket) -> anyhow::Result<Envelope> {
    while let Some(msg) = socket.recv().await {
        match msg? {
            WsMessage::Text(t) => return Ok(serde_json::from_str(&t)?),
            WsMessage::Close(_) => anyhow::bail!("connection closed"),
            // Pings are answered automatically by axum; ignore everything else.
            _ => continue,
        }
    }
    anyhow::bail!("stream ended");
}

async fn send_message(socket: &mut WebSocket, env: &Envelope) -> bool {
    match serde_json::to_string(env) {
        Ok(s) => socket.send(WsMessage::Text(s)).await.is_ok(),
        Err(e) => {
            tracing::error!(error = ?e, "failed to serialise outbound envelope");
            false
        }
    }
}

/// Background task: every 60s, log when any remote worker has gone
/// stale (last_seen older than STALE_AFTER_SECS). Piece 1 just logs;
/// Piece 6 reassigns in-flight jobs.
pub async fn spawn_idle_sweep(state: AppState) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            if let Ok(rows) = db::workers::list_all(&state.pool).await {
                let now = chrono::Utc::now().timestamp();
                for row in rows {
                    if row.kind != "remote" {
                        continue;
                    }
                    if let Some(seen) = row.last_seen_at {
                        if now - seen > STALE_AFTER_SECS {
                            tracing::debug!(
                                worker_id = row.id, name = %row.name,
                                age_secs = now - seen, "worker stale"
                            );
                        }
                    }
                }
            }
        }
    });
}
```

- [ ] **Step 3: Confirm `chrono` is available**

```bash
grep -n "^chrono\|^chrono " /Users/seanbarzilay/projects/private/transcoderr/Cargo.toml /Users/seanbarzilay/projects/private/transcoderr/crates/transcoderr/Cargo.toml | head
```

If chrono isn't a workspace dep, replace `chrono::Utc::now().timestamp()` in `spawn_idle_sweep` with the existing pattern used elsewhere in the codebase, e.g.:

```rust
let now = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map(|d| d.as_secs() as i64)
    .unwrap_or(0);
```

- [ ] **Step 4: Register the WS route + spawn the sweep**

In `crates/transcoderr/src/api/mod.rs`, **the WS route goes in `public`, not `protected`** because the upgrade handler does its own token auth (the `Authorization: Bearer` header on the GET / upgrade request is read inside `connect`, not by the standard `require_auth` middleware).

Add to the `public` Router:

```rust
        .route("/worker/connect", get(workers::connect))
```

In `crates/transcoderr/src/main.rs`, after the existing worker pool spawn (around line 113-115), add:

```rust
            transcoderr::api::workers::spawn_idle_sweep(state.clone()).await;
```

- [ ] **Step 5: Build smoke**

```bash
cargo build -p transcoderr 2>&1 | tail -10
```

Expected: clean build.

- [ ] **Step 6: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-1" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/Cargo.toml \
        crates/transcoderr/src/api/workers.rs \
        crates/transcoderr/src/api/mod.rs \
        crates/transcoderr/src/main.rs
git commit -m "feat(api): /api/worker/connect WS handler + idle sweep"
```

---

## Task 6: Worker daemon — subcommand + WS connection + reconnect loop

The actual `transcoderr worker` binary path. Reads config, dials, registers, heartbeats, reconnects with exponential backoff on failure.

**Files:**
- Modify: `crates/transcoderr/Cargo.toml` — add `tokio-tungstenite = "0.24"`
- Create: `crates/transcoderr/src/worker/config.rs`
- Create: `crates/transcoderr/src/worker/connection.rs`
- Create: `crates/transcoderr/src/worker/daemon.rs`
- Modify: `crates/transcoderr/src/worker/mod.rs` — add `pub mod ...;` for the new files
- Modify: `crates/transcoderr/src/main.rs` — add `Worker { config }` Cmd variant + dispatch

- [ ] **Step 1: Add `tokio-tungstenite` to Cargo.toml**

In `crates/transcoderr/Cargo.toml`'s `[dependencies]` section, add (alphabetically):

```toml
tokio-tungstenite = { version = "0.24", default-features = false, features = ["rustls-tls-native-roots", "connect"] }
```

`rustls-tls-native-roots` matches reqwest's existing rustls-tls feature so the worker uses the OS trust store for `wss://`.

- [ ] **Step 2: Create `worker/config.rs`**

```rust
//! Worker daemon config. TOML at the path passed to
//! `transcoderr worker --config <path>`.

use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct WorkerConfig {
    /// Where to dial. Use `wss://` for TLS, `ws://` for plaintext.
    pub coordinator_url: String,
    /// The token minted in the coordinator's UI.
    pub coordinator_token: String,
    /// Optional friendly name for the Workers UI. Defaults to hostname.
    pub name: Option<String>,
}

impl WorkerConfig {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let s = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("read {}: {e}", path.display()))?;
        toml::from_str(&s).map_err(|e| anyhow::anyhow!("parse {}: {e}", path.display()))
    }

    pub fn resolved_name(&self) -> String {
        self.name.clone().unwrap_or_else(|| {
            hostname::get()
                .ok()
                .and_then(|h| h.into_string().ok())
                .unwrap_or_else(|| "unnamed-worker".into())
        })
    }
}
```

If `hostname` isn't already in deps, the implementer can either add `hostname = "0.4"` or fall back to reading `$HOSTNAME` / `$HOST` env vars. Check first:

```bash
grep -n "^hostname" crates/transcoderr/Cargo.toml /Users/seanbarzilay/projects/private/transcoderr/Cargo.toml | head
```

If absent, add `hostname = "0.4"` to `crates/transcoderr/Cargo.toml`'s `[dependencies]`.

- [ ] **Step 3: Create `worker/connection.rs`**

```rust
//! WebSocket dial + reconnect loop. The daemon (in `daemon.rs`) calls
//! `run` once; this function never returns until the process is
//! killed — it loops forever, opening a fresh connection on every
//! disconnect with exponential backoff.

use crate::worker::protocol::{Envelope, Heartbeat, Message};
use futures::{SinkExt, StreamExt};
use std::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::Message as WsMessage;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// Run the worker connection loop. Never returns. On every disconnect
/// (clean or error), waits for the current backoff and retries. On a
/// successful register handshake, the backoff resets.
pub async fn run<F>(url: String, token: String, build_register: F) -> !
where
    F: Fn() -> Envelope + Send + Sync,
{
    let mut backoff = BACKOFF_INITIAL;

    loop {
        match connect_once(&url, &token, &build_register).await {
            Ok(()) => {
                tracing::info!("worker connection closed cleanly; reconnecting");
                backoff = BACKOFF_INITIAL;
            }
            Err(e) => {
                tracing::warn!(error = %e, "worker connection error");
            }
        }

        tracing::info!(?backoff, "waiting before reconnect");
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

/// Open one WS connection, register, run the heartbeat loop until the
/// connection drops. Returns `Ok(())` on a clean close, `Err` on any
/// I/O or protocol error.
async fn connect_once<F>(
    url: &str,
    token: &str,
    build_register: &F,
) -> anyhow::Result<()>
where
    F: Fn() -> Envelope,
{
    // Build the upgrade request manually so we can attach the
    // Authorization header — `connect_async` accepts anything that
    // implements IntoClientRequest, but the URL form drops headers.
    let mut req = url.into_client_request()?;
    req.headers_mut().insert(
        AUTHORIZATION,
        format!("Bearer {token}").parse()?,
    );

    let (ws, _resp) = tokio_tungstenite::connect_async(req).await?;
    tracing::info!(url, "worker WS connected");
    let (mut tx, mut rx) = ws.split();

    // Send the register frame.
    let register = build_register();
    tx.send(WsMessage::Text(serde_json::to_string(&register)?)).await?;

    // Wait for register_ack.
    let ack_raw = rx.next().await
        .ok_or_else(|| anyhow::anyhow!("stream closed before register_ack"))??;
    let ack: Envelope = match ack_raw {
        WsMessage::Text(s) => serde_json::from_str(&s)?,
        WsMessage::Close(_) => anyhow::bail!("server closed before register_ack"),
        other => anyhow::bail!("unexpected non-text frame from server: {other:?}"),
    };
    match ack.message {
        Message::RegisterAck(_) => {
            tracing::info!("worker register acknowledged");
        }
        other => anyhow::bail!("expected register_ack, got {other:?}"),
    }

    // Heartbeat loop. Emits every HEARTBEAT_INTERVAL and exits if the
    // socket closes from either end.
    let mut ticker = tokio::time::interval(HEARTBEAT_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let hb = Envelope {
                    id: format!("hb-{}", uuid::Uuid::new_v4()),
                    message: Message::Heartbeat(Heartbeat {}),
                };
                tx.send(WsMessage::Text(serde_json::to_string(&hb)?)).await?;
            }
            frame = rx.next() => {
                match frame {
                    Some(Ok(WsMessage::Close(_))) => return Ok(()),
                    Some(Ok(_)) => {
                        // Piece 1 doesn't handle inbound frames beyond
                        // register_ack. Future pieces add step_dispatch
                        // / plugin_install handling here.
                    }
                    Some(Err(e)) => return Err(e.into()),
                    None => return Ok(()),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // Reconnect schedule is encoded by the consts; pin them so future
    // changes are deliberate.
    use super::*;

    #[test]
    fn backoff_grows_then_caps_at_max() {
        let mut b = BACKOFF_INITIAL;
        let mut history = vec![b];
        for _ in 0..10 {
            b = (b * 2).min(BACKOFF_MAX);
            history.push(b);
        }
        // 1, 2, 4, 8, 16, 30, 30, 30, 30, 30, 30
        assert_eq!(
            history,
            vec![
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8),
                Duration::from_secs(16),
                Duration::from_secs(30),
                Duration::from_secs(30),
                Duration::from_secs(30),
                Duration::from_secs(30),
                Duration::from_secs(30),
                Duration::from_secs(30),
            ]
        );
    }

    #[test]
    fn heartbeat_interval_is_30s() {
        assert_eq!(HEARTBEAT_INTERVAL, Duration::from_secs(30));
    }
}
```

If `uuid` isn't a dep, the implementer can use any unique-id generator. Check:

```bash
grep -n "^uuid" crates/transcoderr/Cargo.toml /Users/seanbarzilay/projects/private/transcoderr/Cargo.toml | head
```

If absent, add `uuid = { version = "1", features = ["v4"] }` to `crates/transcoderr/Cargo.toml`.

If `futures` isn't a direct dep already, check too — `tokio-tungstenite` re-exports the `Stream`/`Sink` traits, but the `.split()` / `.send()` / `.next()` calls need them in scope; usually `futures = "0.3"` covers it. Workspace already has it (saw earlier: `futures = "0.3"` in transcoderr's Cargo.toml).

- [ ] **Step 4: Create `worker/daemon.rs`**

```rust
//! Worker daemon entry point. Probes hardware, discovers installed
//! plugins, then hands off to `connection::run` which is the long-lived
//! reconnect loop.

use crate::worker::config::WorkerConfig;
use crate::worker::protocol::{Envelope, Message, PluginManifestEntry, Register};
use std::path::Path;

/// Run the worker daemon. Never returns. Called from `main.rs` when
/// the operator runs `transcoderr worker --config <path>`.
pub async fn run(config: WorkerConfig) -> ! {
    let name = config.resolved_name();
    tracing::info!(name = %name, coordinator = %config.coordinator_url, "starting worker daemon");

    // Hw probe — same path the coordinator uses at boot.
    let hw_caps = match crate::ffmpeg_caps::FfmpegCaps::probe().await {
        caps => serde_json::to_value(&caps).unwrap_or(serde_json::Value::Null),
    };

    // Plugin discovery — operator's data_dir/plugins/ on this host.
    // The worker's data_dir is implicit: we always look at "./plugins"
    // relative to the cwd. If operators want a different path they can
    // chdir in their service unit. (Piece 4 adds proper config for this.)
    let plugin_manifest: Vec<PluginManifestEntry> = match crate::plugins::discover(Path::new("./plugins")) {
        Ok(found) => found
            .into_iter()
            .map(|d| PluginManifestEntry {
                name: d.manifest.name.clone(),
                version: d.manifest.version.clone(),
                sha256: None,
            })
            .collect(),
        Err(e) => {
            tracing::warn!(error = ?e, "plugin discovery failed; reporting empty manifest");
            Vec::new()
        }
    };

    // Available steps: hard-code the known remote-eligible built-ins
    // for Piece 1. Pieces 3+ refine this list (e.g. only report plugin
    // steps that successfully registered locally).
    let available_steps = vec![
        "plan.execute".into(),
        "transcode".into(),
        "remux".into(),
        "extract.subs".into(),
        "iso.extract".into(),
        "audio.ensure".into(),
        "strip.tracks".into(),
    ];

    let build_register = move || -> Envelope {
        Envelope {
            id: format!("reg-{}", uuid::Uuid::new_v4()),
            message: Message::Register(Register {
                name: name.clone(),
                version: env!("CARGO_PKG_VERSION").into(),
                hw_caps: hw_caps.clone(),
                available_steps: available_steps.clone(),
                plugin_manifest: plugin_manifest.clone(),
            }),
        }
    };

    crate::worker::connection::run(
        config.coordinator_url,
        config.coordinator_token,
        build_register,
    )
    .await
}
```

- [ ] **Step 5: Update `worker/mod.rs`**

```rust
//! Worker module. Pre-distributed-transcoding (Piece 1) this just held
//! the in-process job-claim pool at `pool.rs`. The Piece 1 wire
//! protocol skeleton adds `daemon.rs`, `connection.rs`, `protocol.rs`,
//! and `config.rs` as siblings; later pieces wire the local pool
//! through the same registration mechanism remote workers use.
//!
//! `pool::*` is re-exported so existing `use crate::worker::Worker`
//! callsites keep resolving without churn.

pub mod config;
pub mod connection;
pub mod daemon;
pub mod pool;
pub mod protocol;

pub use pool::*;
```

- [ ] **Step 6: Add the `Worker` subcommand to `main.rs`**

Read `crates/transcoderr/src/main.rs`. Find the `Cmd` enum (around line 14-21):

```rust
#[derive(clap::Subcommand)]
enum Cmd {
    /// Run the server.
    Serve {
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
    },
}
```

Replace with:

```rust
#[derive(clap::Subcommand)]
enum Cmd {
    /// Run the server (coordinator).
    Serve {
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
    },
    /// Run as a remote worker. Connects to a coordinator over WebSocket.
    Worker {
        #[arg(long, default_value = "worker.toml")]
        config: PathBuf,
    },
}
```

In the `match cli.cmd` block, add a `Worker { config }` arm at the end:

```rust
        Cmd::Worker { config } => {
            let cfg = transcoderr::worker::config::WorkerConfig::load(&config)?;
            transcoderr::worker::daemon::run(cfg).await
        }
```

The trailing `await` returns `!`, which is fine for the function's `Result<()>` return type — `!` coerces to anything.

- [ ] **Step 7: Build smoke**

```bash
cargo build -p transcoderr 2>&1 | tail -10
```

Expected: clean build.

- [ ] **Step 8: Run the new tests**

```bash
cargo test -p transcoderr --lib worker::connection 2>&1 | tail -10
```

Expected: 2 tests pass.

- [ ] **Step 9: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-1" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/Cargo.toml \
        crates/transcoderr/src/worker/config.rs \
        crates/transcoderr/src/worker/connection.rs \
        crates/transcoderr/src/worker/daemon.rs \
        crates/transcoderr/src/worker/mod.rs \
        crates/transcoderr/src/main.rs
git commit -m "feat(worker): daemon + WS connection + reconnect loop"
```

---

## Task 7: Auth middleware — accept worker tokens

Critical: existing API token auth (every protected endpoint) must keep working. Worker tokens become a second valid Bearer source.

**Files:**
- Modify: `crates/transcoderr/src/api/auth.rs`

- [ ] **Step 1: Read the current `require_auth`**

```bash
sed -n '82,127p' crates/transcoderr/src/api/auth.rs
```

The relevant Bearer block is at lines 97-113. After the existing `crate::db::api_tokens::verify` check, add a worker-token check.

- [ ] **Step 2: Edit `require_auth` to accept worker tokens too**

Replace the inner Bearer block:

```rust
    if let Some(h) = request.headers().get(axum::http::header::AUTHORIZATION) {
        if let Ok(s) = h.to_str() {
            if let Some(token) = s.strip_prefix("Bearer ") {
                if crate::db::api_tokens::verify(&state.pool, token).await.is_some() {
                    request.extensions_mut().insert(AuthSource::Token);
                    return Ok(next.run(request).await);
                }
                if enabled {
                    return Err(StatusCode::UNAUTHORIZED);
                }
            }
        }
    }
```

with:

```rust
    if let Some(h) = request.headers().get(axum::http::header::AUTHORIZATION) {
        if let Ok(s) = h.to_str() {
            if let Some(token) = s.strip_prefix("Bearer ") {
                if crate::db::api_tokens::verify(&state.pool, token).await.is_some() {
                    request.extensions_mut().insert(AuthSource::Token);
                    return Ok(next.run(request).await);
                }
                // Worker tokens are a second valid Bearer source. They
                // grant the same surface as API tokens for the
                // /api/workers* and /api/worker/* paths the worker
                // daemon uses; for other paths, treat as redacted Token
                // (same redaction policy applies).
                if let Ok(Some(_row)) = crate::db::workers::get_by_token(&state.pool, token).await {
                    request.extensions_mut().insert(AuthSource::Token);
                    return Ok(next.run(request).await);
                }
                if enabled {
                    return Err(StatusCode::UNAUTHORIZED);
                }
            }
        }
    }
```

- [ ] **Step 3: Build smoke**

```bash
cargo build -p transcoderr 2>&1 | tail -5
```

- [ ] **Step 4: Run the existing auth tests**

```bash
cargo test -p transcoderr --lib api::auth 2>&1 | tail -10
cargo test -p transcoderr --test '*' 2>&1 | grep -E "test result|FAILED" | head -30
```

Expected: every line `test result: ok.`. The auth_* integration tests must stay green; this change is additive and only fires when an unrecognized token is presented.

- [ ] **Step 5: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-1" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/api/auth.rs
git commit -m "feat(auth): accept worker tokens as a second Bearer source"
```

---

## Task 8: Workers UI page + AddWorkerForm token-mint modal

Frontend. Read-only listing + a button that mints a token and shows it once.

**Files:**
- Modify: `web/src/types.ts` — add `Worker` type
- Modify: `web/src/api/client.ts` — `api.workers.{list, create, delete}`
- Create: `web/src/components/forms/add-worker.tsx` — token-mint modal
- Create: `web/src/pages/workers.tsx` — Workers page
- Modify: `web/src/components/sidebar.tsx` — add "Workers" entry
- Modify: `web/src/App.tsx` — add `/workers` route

- [ ] **Step 1: Add `Worker` type**

In `web/src/types.ts`, append:

```ts
export type Worker = {
  id: number;
  name: string;
  kind: "local" | "remote";
  secret_token: string | null;       // "***" or null after mint
  hw_caps: any | null;
  plugin_manifest: any[] | null;
  enabled: boolean;
  last_seen_at: number | null;
  created_at: number;
};

export type WorkerCreateResp = {
  id: number;
  secret_token: string;              // one-time-display
};
```

- [ ] **Step 2: Add the api client wrappers**

In `web/src/api/client.ts`, find the existing `pluginCatalogs` block and add a `workers` block alongside it:

```ts
  workers: {
    list:   () => req<import("../types").Worker[]>("/workers"),
    create: (name: string) =>
      req<import("../types").WorkerCreateResp>("/workers", {
        method: "POST",
        body: JSON.stringify({ name }),
      }),
    delete: (id: number) => req<void>(`/workers/${id}`, { method: "DELETE" }),
  },
```

- [ ] **Step 3: Create `web/src/components/forms/add-worker.tsx`**

```tsx
import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../../api/client";

interface Props {
  /// URL-base the operator will paste into the worker config (e.g.
  /// "wss://transcoderr.example"). Defaults to the page's origin with
  /// the scheme flipped to wss/ws.
  coordinatorUrlGuess: string;
  onClose: () => void;
}

/// Two-stage modal. Stage 1: pick a name + click Create. Stage 2: show
/// the cleartext token + a copy of the worker.toml the operator should
/// drop on the worker host. The token is shown ONCE — closing the modal
/// removes it from memory.
export default function AddWorkerForm({ coordinatorUrlGuess, onClose }: Props) {
  const qc = useQueryClient();
  const [name, setName] = useState("");
  const [created, setCreated] = useState<{ id: number; token: string; name: string } | null>(null);
  const [error, setError] = useState<string | null>(null);

  const create = useMutation({
    mutationFn: () => api.workers.create(name),
    onSuccess: (resp) => {
      setCreated({ id: resp.id, token: resp.secret_token, name });
      qc.invalidateQueries({ queryKey: ["workers"] });
    },
    onError: (e: any) => setError(e?.message ?? "create failed"),
  });

  const configToml =
    created &&
    `coordinator_url   = "${coordinatorUrlGuess}/api/worker/connect"\n` +
    `coordinator_token = "${created.token}"\n` +
    `name              = "${created.name}"\n`;

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal-header">
          <h3>{created ? "Worker token" : "Add worker"}</h3>
          <button className="btn-text" onClick={onClose}>✕</button>
        </div>
        <div style={{ padding: 16 }}>
          {!created && (
            <>
              <p className="muted" style={{ fontSize: 13 }}>
                Pick a name for this worker (shown in the Workers list).
                A one-time token will be generated; drop it into the
                worker host's <code>worker.toml</code>.
              </p>
              <input
                placeholder="e.g. gpu-box-1"
                value={name}
                onChange={(e) => setName(e.target.value)}
                style={{ width: "100%", marginBottom: 8 }}
              />
              {error && (
                <p className="hint" style={{ color: "var(--bad)" }}>{error}</p>
              )}
              <div style={{ display: "flex", gap: 8 }}>
                <button
                  onClick={() => create.mutate()}
                  disabled={!name.trim() || create.isPending}
                >
                  Create
                </button>
                <button className="btn-ghost" onClick={onClose}>Cancel</button>
              </div>
            </>
          )}
          {created && (
            <>
              <p className="muted" style={{ fontSize: 13 }}>
                Copy this token now — this is the only time it will be
                shown. Save it as <code>worker.toml</code> on the worker
                host and run <code>transcoderr worker --config worker.toml</code>.
              </p>
              <pre
                style={{
                  background: "var(--surface)",
                  border: "1px solid var(--border)",
                  borderRadius: "var(--r-2)",
                  padding: 12,
                  fontSize: 12,
                  fontFamily: "var(--font-mono)",
                  whiteSpace: "pre-wrap",
                  wordBreak: "break-all",
                  marginBottom: 12,
                }}
              >
                {configToml}
              </pre>
              <div style={{ display: "flex", gap: 8 }}>
                <button
                  onClick={() => navigator.clipboard?.writeText(configToml ?? "")}
                >
                  Copy
                </button>
                <button onClick={onClose}>Done</button>
              </div>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
```

- [ ] **Step 4: Create `web/src/pages/workers.tsx`**

```tsx
import { useState } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../api/client";
import type { Worker } from "../types";
import AddWorkerForm from "../components/forms/add-worker";

const STALE_AFTER_SECS = 90;

function formatSeen(now: number, last: number | null): { label: string; status: string } {
  if (last == null) return { label: "never", status: "offline" };
  const age = now - last;
  if (age < STALE_AFTER_SECS) {
    if (age < 60) return { label: `${age}s ago`, status: "connected" };
    return { label: `${Math.floor(age / 60)}m ago`, status: "connected" };
  }
  return { label: `${Math.floor(age / 60)}m ago`, status: "stale" };
}

function hwCapsSummary(caps: any): string {
  if (!caps || typeof caps !== "object") return "—";
  const devices = Array.isArray(caps.devices) ? caps.devices : [];
  if (devices.length === 0) return "software only";
  const counts: Record<string, number> = {};
  for (const d of devices) {
    const accel = String(d.accel ?? "?").toUpperCase();
    counts[accel] = (counts[accel] ?? 0) + (d.max_concurrent ?? 1);
  }
  return Object.entries(counts).map(([a, n]) => `${a} ×${n}`).join(", ");
}

export default function Workers() {
  const qc = useQueryClient();
  const list = useQuery({ queryKey: ["workers"], queryFn: api.workers.list });
  const [addOpen, setAddOpen] = useState(false);

  const del = useMutation({
    mutationFn: (id: number) => api.workers.delete(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["workers"] }),
  });

  const now = Math.floor(Date.now() / 1000);
  const coordinatorUrlGuess = window.location.origin
    .replace(/^http:/, "ws:")
    .replace(/^https:/, "wss:");

  return (
    <div className="page">
      <div className="page-header">
        <div>
          <div className="crumb">Operate</div>
          <h2>Workers</h2>
        </div>
        <button onClick={() => setAddOpen(true)}>Add worker</button>
      </div>

      <div className="surface">
        <table>
          <thead>
            <tr>
              <th style={{ width: 110 }}>Status</th>
              <th>Name</th>
              <th style={{ width: 90 }}>Kind</th>
              <th>Hardware</th>
              <th style={{ width: 130 }}>Last seen</th>
              <th style={{ width: 90 }}></th>
            </tr>
          </thead>
          <tbody>
            {(list.data ?? []).map((w: Worker) => {
              const seen = formatSeen(now, w.last_seen_at);
              return (
                <tr key={w.id}>
                  <td>
                    <span className={`badge badge-${seen.status}`}>{seen.status}</span>
                  </td>
                  <td>{w.name}</td>
                  <td><span className="label">{w.kind}</span></td>
                  <td className="mono dim">{hwCapsSummary(w.hw_caps)}</td>
                  <td className="dim">{seen.label}</td>
                  <td>
                    {w.kind === "remote" && (
                      <button
                        className="btn-danger"
                        onClick={() => {
                          if (confirm(`Delete worker "${w.name}"?`)) del.mutate(w.id);
                        }}
                      >
                        Delete
                      </button>
                    )}
                  </td>
                </tr>
              );
            })}
            {(list.data ?? []).length === 0 && !list.isLoading && (
              <tr><td colSpan={6} className="empty">No workers yet.</td></tr>
            )}
          </tbody>
        </table>
      </div>

      {addOpen && (
        <AddWorkerForm
          coordinatorUrlGuess={coordinatorUrlGuess}
          onClose={() => setAddOpen(false)}
        />
      )}
    </div>
  );
}
```

- [ ] **Step 5: Add the sidebar entry**

Read `web/src/components/sidebar.tsx`. Find the existing entries for `Plugins` and `Settings`. Add a new `Workers` entry between them. Match whatever pattern the existing entries use (likely a `<NavLink to="/workers">Workers</NavLink>` or similar — the file is small and the pattern repeats).

- [ ] **Step 6: Add the route**

In `web/src/App.tsx`, find the existing `<Routes>` block. Add the import:

```tsx
import Workers from "./pages/workers";
```

And add the route:

```tsx
          <Route path="/workers" element={<Workers />} />
```

(Place it between `/plugins` and `/settings` to match the sidebar order.)

- [ ] **Step 7: Add CSS for badge variants if missing**

Check whether `.badge-connected` / `.badge-stale` / `.badge-offline` already exist:

```bash
grep -n "badge-connected\|badge-stale\|badge-offline\|badge-auto\|badge-manual" web/src/index.css | head
```

If missing, append to `web/src/index.css`:

```css
.badge-connected { background: var(--ok-soft); color: var(--ok); }
.badge-stale     { background: var(--warn-soft); color: var(--warn); }
.badge-offline   { background: var(--neutral-soft); color: var(--neutral); }
```

(Use the same shape as the existing `.badge-auto` / `.badge-manual` rules.)

- [ ] **Step 8: Build smoke**

```bash
npm --prefix web run build 2>&1 | tail -5
```

Expected: clean.

- [ ] **Step 9: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-1" || { echo "WRONG BRANCH"; exit 1; }
git add web/src/types.ts \
        web/src/api/client.ts \
        web/src/components/forms/add-worker.tsx \
        web/src/pages/workers.tsx \
        web/src/components/sidebar.tsx \
        web/src/App.tsx \
        web/src/index.css
git commit -m "web: Workers page + AddWorkerForm token-mint modal"
```

---

## Task 9: Integration test — `tests/worker_connect.rs`

End-to-end exercise: spin up a real coordinator, open a real WebSocket, send register, assert the database state, send heartbeats, drop and reopen.

**Files:**
- Create: `crates/transcoderr/tests/worker_connect.rs`

- [ ] **Step 1: Confirm tokio-tungstenite is available in dev deps**

It's already a regular dep from Task 6, so it's automatically usable in `tests/`. No `[dev-dependencies]` change needed.

- [ ] **Step 2: Create the test file**

```rust
//! Integration tests for the worker WS upgrade + register handshake.
//! Spins up the in-process axum router, connects a real WS client to
//! it, exercises the protocol end to end.

mod common;

use common::boot;
use futures::{SinkExt, StreamExt};
use serde_json::json;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use transcoderr::worker::protocol::{
    Envelope, Heartbeat, Message, PluginManifestEntry, Register,
};

/// Mint a remote-worker token via the REST endpoint and return it.
async fn mint_token(client: &reqwest::Client, base: &str, name: &str) -> (i64, String) {
    let resp: serde_json::Value = client
        .post(format!("{base}/api/workers"))
        .json(&json!({"name": name}))
        .send().await.unwrap()
        .json().await.unwrap();
    let id = resp["id"].as_i64().expect("id");
    let token = resp["secret_token"].as_str().expect("token").to_string();
    (id, token)
}

/// Open a real WS connection to the in-process router with the given
/// Bearer token. URL is the running app's `ws://...` form.
async fn ws_connect(
    base_ws: &str,
    token: &str,
) -> Result<
    tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    tokio_tungstenite::tungstenite::Error,
> {
    let url = format!("{base_ws}/api/worker/connect");
    let mut req = url.as_str().into_client_request().unwrap();
    req.headers_mut().insert(
        AUTHORIZATION,
        format!("Bearer {token}").parse().unwrap(),
    );
    tokio_tungstenite::connect_async(req).await.map(|(s, _)| s)
}

fn make_register(name: &str) -> Envelope {
    Envelope {
        id: "reg-1".into(),
        message: Message::Register(Register {
            name: name.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            hw_caps: json!({"encoders": []}),
            available_steps: vec!["plan.execute".into()],
            plugin_manifest: vec![PluginManifestEntry {
                name: "size-report".into(),
                version: "0.1.2".into(),
                sha256: None,
            }],
        }),
    }
}

async fn send_env(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    env: &Envelope,
) {
    let s = serde_json::to_string(env).unwrap();
    ws.send(WsMessage::Text(s)).await.unwrap();
}

async fn recv_env(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> Envelope {
    let raw = ws.next().await.unwrap().unwrap();
    match raw {
        WsMessage::Text(s) => serde_json::from_str(&s).unwrap(),
        other => panic!("expected text, got {other:?}"),
    }
}

#[tokio::test]
async fn connect_with_valid_token_succeeds_and_register_persists() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (worker_id, token) = mint_token(&client, &app.url, "gpu-box-1").await;

    let base_ws = app.url.replace("http://", "ws://");
    let mut ws = ws_connect(&base_ws, &token).await.unwrap();

    send_env(&mut ws, &make_register("gpu-box-1")).await;

    // Expect register_ack with the worker_id we just minted.
    let ack = recv_env(&mut ws).await;
    match ack.message {
        Message::RegisterAck(a) => assert_eq!(a.worker_id, worker_id),
        other => panic!("expected register_ack, got {other:?}"),
    }

    // DB should now have hw_caps_json + last_seen_at populated.
    let row: (Option<String>, Option<i64>) = sqlx::query_as(
        "SELECT hw_caps_json, last_seen_at FROM workers WHERE id = ?",
    )
    .bind(worker_id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert!(row.0.is_some(), "hw_caps_json must be persisted");
    assert!(row.1.is_some(), "last_seen_at must be set");
}

#[tokio::test]
async fn connect_with_invalid_token_fails() {
    let app = boot().await;
    let base_ws = app.url.replace("http://", "ws://");

    let result = ws_connect(&base_ws, "not-a-real-token").await;
    // Tungstenite reports the 401 as an Http error during handshake.
    assert!(result.is_err(), "connect with bogus token must fail");
}

#[tokio::test]
async fn heartbeat_keeps_last_seen_fresh() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (worker_id, token) = mint_token(&client, &app.url, "hb-box").await;
    let base_ws = app.url.replace("http://", "ws://");

    let mut ws = ws_connect(&base_ws, &token).await.unwrap();
    send_env(&mut ws, &make_register("hb-box")).await;
    let _ack = recv_env(&mut ws).await;

    // Capture initial last_seen.
    let initial: i64 = sqlx::query_as::<_, (i64,)>(
        "SELECT COALESCE(last_seen_at, 0) FROM workers WHERE id = ?",
    )
    .bind(worker_id)
    .fetch_one(&app.pool)
    .await
    .unwrap()
    .0;

    // Wait a real second so unix-second granularity advances.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;

    // Send a heartbeat.
    send_env(
        &mut ws,
        &Envelope {
            id: "hb-1".into(),
            message: Message::Heartbeat(Heartbeat {}),
        },
    )
    .await;

    // Give the coordinator a moment to record it.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let after: i64 = sqlx::query_as::<_, (i64,)>(
        "SELECT COALESCE(last_seen_at, 0) FROM workers WHERE id = ?",
    )
    .bind(worker_id)
    .fetch_one(&app.pool)
    .await
    .unwrap()
    .0;

    assert!(
        after > initial,
        "heartbeat must advance last_seen_at (was {initial}, now {after})"
    );
}

#[tokio::test]
async fn list_endpoint_redacts_secret_token_under_token_auth() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (_worker_id, token) = mint_token(&client, &app.url, "redact-test").await;

    // Read /api/workers with the worker token as Bearer auth — auth.rs
    // accepts worker tokens (Task 7) and marks the call as Token-authed,
    // which triggers the redaction policy in api/workers.rs::list.
    let resp: serde_json::Value = client
        .get(format!("{}/api/workers", app.url))
        .bearer_auth(&token)
        .send().await.unwrap()
        .json().await.unwrap();
    let arr = resp.as_array().unwrap();
    let remote = arr.iter().find(|w| w["kind"] == "remote").unwrap();
    assert_eq!(remote["secret_token"], "***");
}
```

- [ ] **Step 3: Run the test file**

```bash
cargo test -p transcoderr --test worker_connect 2>&1 | tail -15
```

Expected: 4 tests pass.

- [ ] **Step 4: Run the full suite to confirm no regressions**

```bash
cargo test -p transcoderr 2>&1 | grep -E "test result|FAILED" | head -40
```

Expected: every line `test result: ok.`. No FAILED.

- [ ] **Step 5: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-1" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/tests/worker_connect.rs
git commit -m "test(worker): WS connect + register + heartbeat round-trip"
```

---

## Task 10: README + docs/deploy.md mention

Operators need to know `transcoderr worker` exists and that reverse proxies need WebSocket Upgrade headers passed through.

**Files:**
- Modify: `README.md`
- Modify: `docs/deploy.md`

- [ ] **Step 1: Add a section to `docs/deploy.md`**

Read `docs/deploy.md`. Find a sensible spot near the `reverse proxy + auth` section. Insert a new section, e.g. after "Reverse proxy + auth":

```markdown
## Distributed transcoding (worker mode)

A second host can connect to a running coordinator as a worker and
receive ffmpeg / heavy plugin work over a WebSocket. As of v0.31 (this
release ships only the connection layer — Workers UI shows them as
connected; jobs still run on the coordinator. Future releases route
work to remote workers).

Mint a token in the coordinator UI (Settings → Workers → Add worker),
save it as `worker.toml` on the worker host:

```toml
coordinator_url   = "wss://transcoderr.example/api/worker/connect"
coordinator_token = "<token-shown-once>"
name              = "gpu-box-1"
```

Then on the worker host:

```bash
docker run --rm \
  -v $(pwd)/worker.toml:/etc/transcoderr/worker.toml \
  -v /mnt/movies:/mnt/movies \
  ghcr.io/seanbarzilay/transcoderr:nvidia-latest \
  transcoderr worker --config /etc/transcoderr/worker.toml
```

Mount the media volume at the same path the coordinator uses (the
worker reads/writes the file directly). Docker images already include
both `serve` and `worker` subcommands.

### Reverse-proxy notes

Workers connect over WebSocket. If the coordinator is behind nginx /
caddy / traefik, make sure `Upgrade` and `Connection` headers are
passed through (most defaults do, but it's worth checking on a 502).
```

- [ ] **Step 2: Add a one-line mention to `README.md`**

Read `README.md`. Find the "What it does" bullet list. Add a new bullet after the "Single binary." line:

```markdown
- **Distributed-ready.** Optional `transcoderr worker` mode connects
  to a coordinator over WebSocket and (in future releases) takes
  ffmpeg / heavy plugin work off your main box. As of v0.31 the
  connection layer is in; routing lands in subsequent releases.
```

- [ ] **Step 3: Verify the docs render**

```bash
ls docs/deploy.md README.md
```

Both files exist after edits. Markdown render is checked at PR review time on GitHub.

- [ ] **Step 4: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-1" || { echo "WRONG BRANCH"; exit 1; }
git add README.md docs/deploy.md
git commit -m "docs: distributed-transcoding worker mode (Piece 1 connection layer)"
```

---

## Self-Review Notes

This plan covers every section of the spec:
- CLI subcommand `transcoderr worker` → Task 6.
- WebSocket dial + Bearer auth → Task 6 (client) + Task 5 (server).
- Envelope shape + 3 message types → Task 3.
- 30s heartbeat / 90s stale → Task 5 (constants `HEARTBEAT_INTERVAL`, `STALE_AFTER_SECS`).
- Reconnect with exponential backoff (1→30s) → Task 6.
- Migration: workers table + jobs.worker_id + run_events.worker_id → Task 2.
- REST CRUD `/api/workers` → Task 4.
- WS upgrade `/api/worker/connect` → Task 5.
- Auth middleware extension → Task 7.
- Module reorg `worker.rs` → `worker/pool.rs` → Task 1.
- `api/workers.rs` holds REST + WS → Tasks 4 & 5 (same file).
- `db/workers.rs` CRUD → Task 2.
- Workers page + AddWorkerForm → Task 8.
- Integration tests (4 scenarios) → Task 9.
- Docs → Task 10.
- `tokio-tungstenite = "0.24"` for the client → Task 6.
- Local row seeded by migration → Task 2.

No placeholders. No "Similar to Task N." Type names + API shapes (`Envelope`, `Register`, `Worker`, `WorkerSummary`, `WorkerCreateResp`) consistent across tasks. Every code block is complete and pasteable.

The plan touches both Rust and TypeScript and includes one DB migration; total commit chain is 10 commits, each independently shippable. PR weight is moderate — call it ~1500 lines net including tests.
