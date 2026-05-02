# Distributed Transcoding — Piece 2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the in-process worker pool register through the same mechanism remote workers use, surface a per-worker enable/disable toggle on `/workers`, and keep all existing job-claim semantics unchanged for the default-on case.

**Architecture:** A new `crate::worker::local` module owns the seeded `local` row (`id=1`). At boot the coordinator stamps that row with current hw_caps + plugin_manifest + last_seen_at; a 30s heartbeat keeps `last_seen_at` fresh regardless of enable state. The existing pool's `run_loop` consults `is_enabled` before each `claim_next` — when false, it 500ms-sleeps + re-checks (graceful drain: in-flight job finishes; new claims short-circuit). A new `PATCH /api/workers/:id` endpoint lets the UI toggle `workers.enabled` per row, uniformly for local + remote rows.

**Tech Stack:** Rust 2021 (axum 0.7, sqlx + sqlite, tokio, anyhow, tracing). React 18 + TypeScript + TanStack Query v5.

**Branch:** all tasks land on a fresh `feat/distributed-piece-2` branch off `main`. Implementer creates the branch before Task 1.

---

## File Structure

**New backend files:**
- `crates/transcoderr/src/worker/local.rs` — `LOCAL_WORKER_ID` constant, `register_local_worker`, `spawn_local_heartbeat`, `is_enabled`, plus 2 unit tests
- `crates/transcoderr/tests/local_worker.rs` — 4 integration scenarios

**Modified backend files:**
- `crates/transcoderr/src/db/workers.rs` — add `set_enabled(pool, id, enabled)` + 1 unit test (round-trip)
- `crates/transcoderr/src/worker/mod.rs` — `pub mod local;`
- `crates/transcoderr/src/worker/pool.rs` — `run_loop` consults `is_enabled` before each `claim_next`
- `crates/transcoderr/src/api/workers.rs` — add `PatchReq` + `patch` handler (returns updated `WorkerSummary`)
- `crates/transcoderr/src/api/mod.rs` — register `/workers/:id` PATCH route in the protected chain; import `axum::routing::patch`
- `crates/transcoderr/src/main.rs` — call `register_local_worker` synchronously before spawning the worker pool, then `spawn_local_heartbeat`
- `crates/transcoderr/tests/common/mod.rs` — same wiring as `main.rs` so integration tests exercise the boot path

**Modified web files:**
- `web/src/api/client.ts` — `api.workers.patch(id, body)` wrapper
- `web/src/pages/workers.tsx` — Enabled toggle column, status-badge logic recognizes `disabled`
- `web/src/index.css` — `.badge-disabled` rule

**No DB migration:** Piece 1 already added the `enabled`, `hw_caps_json`, `plugin_manifest_json`, and `last_seen_at` columns.

---

## Task 1: `db::workers::set_enabled` + unit test

Mechanical CRUD addition. Round-trip test only.

**Files:**
- Modify: `crates/transcoderr/src/db/workers.rs`

- [ ] **Step 1: Branch verification + add the function**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-2" || { echo "WRONG BRANCH"; exit 1; }
```

Append to `crates/transcoderr/src/db/workers.rs` (after `record_heartbeat`, before `mod tests`):

```rust
/// Toggle `enabled` for a worker. Returns the number of rows affected
/// (0 if id doesn't exist; 1 on success).
pub async fn set_enabled(pool: &SqlitePool, id: i64, enabled: bool) -> anyhow::Result<u64> {
    let res = sqlx::query("UPDATE workers SET enabled = ? WHERE id = ?")
        .bind(if enabled { 1_i64 } else { 0_i64 })
        .bind(id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}
```

- [ ] **Step 2: Add the unit test**

In the existing `mod tests` block in `db/workers.rs`, append:

```rust
    #[tokio::test]
    async fn set_enabled_round_trips() {
        let (pool, _dir) = pool().await;
        // Seeded local row starts enabled=1.
        let row = get_by_id(&pool, 1).await.unwrap().unwrap();
        assert_eq!(row.enabled, 1);

        let n = set_enabled(&pool, 1, false).await.unwrap();
        assert_eq!(n, 1);
        let row = get_by_id(&pool, 1).await.unwrap().unwrap();
        assert_eq!(row.enabled, 0);

        let n = set_enabled(&pool, 1, true).await.unwrap();
        assert_eq!(n, 1);
        let row = get_by_id(&pool, 1).await.unwrap().unwrap();
        assert_eq!(row.enabled, 1);

        // Missing id returns 0.
        let n = set_enabled(&pool, 9999, true).await.unwrap();
        assert_eq!(n, 0);
    }
```

- [ ] **Step 3: Run the new test**

```bash
cargo test -p transcoderr --lib db::workers::tests::set_enabled_round_trips 2>&1 | tail -10
```

Expected: 1 passed.

- [ ] **Step 4: Run all `db::workers` tests so we know nothing regressed**

```bash
cargo test -p transcoderr --lib db::workers 2>&1 | tail -10
```

Expected: 6 passed (5 existing + 1 new).

- [ ] **Step 5: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-2" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/db/workers.rs
git commit -m "feat(db): workers.set_enabled toggle"
```

---

## Task 2: `worker/local.rs` module

The new local-worker module: `LOCAL_WORKER_ID`, `register_local_worker`, `spawn_local_heartbeat`, `is_enabled`, plus unit tests for `is_enabled`.

**Files:**
- Create: `crates/transcoderr/src/worker/local.rs`
- Modify: `crates/transcoderr/src/worker/mod.rs`

- [ ] **Step 1: Branch verification + create `local.rs`**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-2" || { echo "WRONG BRANCH"; exit 1; }
```

Create `crates/transcoderr/src/worker/local.rs`:

```rust
//! Local-worker registration. The seeded `local` row in the `workers`
//! table (id=1) gets stamped with hw_caps + plugin_manifest at boot,
//! and a background heartbeat task keeps `last_seen_at` fresh every 30s
//! regardless of whether the row is enabled.
//!
//! `is_enabled` is consulted by `pool::Worker::run_loop` before each
//! claim — toggling `workers.enabled` from the UI is the per-worker
//! kill switch (graceful drain: the in-flight job finishes; the next
//! claim short-circuits).

use crate::db;
use crate::ffmpeg_caps::FfmpegCaps;
use crate::plugins::DiscoveredPlugin;
use crate::worker::protocol::PluginManifestEntry;
use sqlx::SqlitePool;
use std::time::Duration;

/// Pinned to the migration's `INSERT INTO workers (...) VALUES ('local',
/// 'local', 1, ...)` which gets `rowid=1` on a fresh database. If the
/// migration ever reorders that insert, this constant moves with it.
pub const LOCAL_WORKER_ID: i64 = 1;

/// 30s — matches the remote worker `HEARTBEAT_INTERVAL` in
/// `worker/connection.rs`. Keeping the cadence identical means the UI's
/// "stale after 90s" logic is uniform across local and remote rows.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

/// Stamp the seeded `local` row with the coordinator's current hw_caps
/// and plugin manifest. Failure logs a warning and returns `Ok(())` —
/// boot must not block on this. The pool keeps working; only the
/// Workers UI shows stale data until next register.
pub async fn register_local_worker(
    pool: &SqlitePool,
    ffmpeg_caps: &FfmpegCaps,
    plugins: &[DiscoveredPlugin],
) -> anyhow::Result<()> {
    let hw_caps = serde_json::json!({
        "has_libplacebo": ffmpeg_caps.has_libplacebo,
    });
    let hw_caps_json = serde_json::to_string(&hw_caps).unwrap_or_else(|_| "null".into());

    let manifest: Vec<PluginManifestEntry> = plugins
        .iter()
        .map(|p| PluginManifestEntry {
            name: p.manifest.name.clone(),
            version: p.manifest.version.clone(),
            sha256: None,
        })
        .collect();
    let plugin_manifest_json =
        serde_json::to_string(&manifest).unwrap_or_else(|_| "[]".into());

    if let Err(e) = db::workers::record_register(
        pool,
        LOCAL_WORKER_ID,
        &hw_caps_json,
        &plugin_manifest_json,
    )
    .await
    {
        tracing::warn!(error = ?e, "local worker register failed; UI may show stale row");
    }
    Ok(())
}

/// Spawn the local heartbeat task. Stamps `last_seen_at` every 30s on
/// the seeded `local` row regardless of `enabled`. This is what makes
/// the UI distinguish "operator turned it off" (enabled=false, fresh
/// last_seen) from "the daemon is dead" (enabled=true, stale last_seen).
pub fn spawn_local_heartbeat(pool: SqlitePool) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(HEARTBEAT_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // First tick fires immediately; skip it because we already
        // stamped `last_seen_at` via record_register at boot.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if let Err(e) = db::workers::record_heartbeat(&pool, LOCAL_WORKER_ID).await {
                tracing::warn!(error = ?e, "local heartbeat failed");
            }
        }
    });
}

/// True if the local worker row is enabled. Defaults to `true` on DB
/// error so transient sqlite hiccups don't stall the pool.
pub async fn is_enabled(pool: &SqlitePool) -> bool {
    let row: Result<(i64,), _> =
        sqlx::query_as("SELECT enabled FROM workers WHERE id = ?")
            .bind(LOCAL_WORKER_ID)
            .fetch_one(pool)
            .await;
    match row {
        Ok((flag,)) => flag != 0,
        Err(e) => {
            tracing::warn!(error = ?e, "is_enabled query failed; defaulting to true");
            true
        }
    }
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
    async fn is_enabled_returns_column_value() {
        let (pool, _dir) = pool().await;
        // Seeded enabled=1.
        assert!(is_enabled(&pool).await);

        db::workers::set_enabled(&pool, LOCAL_WORKER_ID, false).await.unwrap();
        assert!(!is_enabled(&pool).await);

        db::workers::set_enabled(&pool, LOCAL_WORKER_ID, true).await.unwrap();
        assert!(is_enabled(&pool).await);
    }

    #[tokio::test]
    async fn is_enabled_defaults_true_when_row_missing() {
        // We can't easily fabricate a "closed pool" so cover the
        // not-found path instead (also routes through fetch_one's
        // RowNotFound error → the warn path).
        let (pool, _dir) = pool().await;
        // Drop the seeded row.
        sqlx::query("DELETE FROM workers WHERE id = ?")
            .bind(LOCAL_WORKER_ID)
            .execute(&pool)
            .await
            .unwrap();
        assert!(is_enabled(&pool).await, "missing row must default to true");
    }
}
```

- [ ] **Step 2: Wire the module into `worker/mod.rs`**

Read `crates/transcoderr/src/worker/mod.rs`. Add `pub mod local;` next to the existing `pub mod pool;`/`pub mod protocol;` lines:

```rust
pub mod config;
pub mod connection;
pub mod daemon;
pub mod local;
pub mod pool;
pub mod protocol;

pub use pool::*;
```

- [ ] **Step 3: Build smoke**

```bash
cargo build -p transcoderr 2>&1 | tail -5
```

Expected: clean build.

- [ ] **Step 4: Run the new tests**

```bash
cargo test -p transcoderr --lib worker::local 2>&1 | tail -10
```

Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-2" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/worker/local.rs crates/transcoderr/src/worker/mod.rs
git commit -m "feat(worker): local module — register, heartbeat, is_enabled"
```

---

## Task 3: Boot wiring in `main.rs`

Call `register_local_worker` synchronously before spawning the worker pool (avoids the boot race documented in the spec's Risks section), then spawn the heartbeat task.

**Files:**
- Modify: `crates/transcoderr/src/main.rs`

- [ ] **Step 1: Read the boot section**

```bash
sed -n '95,135p' crates/transcoderr/src/main.rs
```

Confirm the order:
1. Plugin deps run (line ~85-96)
2. `FfmpegCaps::probe()` (line ~97-99)
3. `steps::registry::init` (line ~105-111)
4. Metrics install (line ~113)
5. `bus`, `cancellations`, `Worker::new` (line ~115-122)
6. `worker.recover_on_boot()` + spawn loop(s)

- [ ] **Step 2: Insert `register_local_worker` after registry init, before `Worker::new`**

Find this block (around line 111-117):

```rust
            transcoderr::steps::registry::init(
                pool.clone(),
                registry.clone(),
                ffmpeg_caps.clone(),
                discovered,
            )
            .await;

            let metrics = std::sync::Arc::new(transcoderr::metrics::Metrics::install()?);
```

It currently passes `discovered` (a `Vec<DiscoveredPlugin>`) to `registry::init` by move — we need it for `register_local_worker` too. Change the call site so `register_local_worker` runs BEFORE `registry::init` consumes `discovered`. Replace the block above with:

```rust
            transcoderr::worker::local::register_local_worker(
                &pool,
                &ffmpeg_caps,
                &discovered,
            )
            .await?;

            transcoderr::steps::registry::init(
                pool.clone(),
                registry.clone(),
                ffmpeg_caps.clone(),
                discovered,
            )
            .await;

            let metrics = std::sync::Arc::new(transcoderr::metrics::Metrics::install()?);
```

`register_local_worker` borrows `&discovered`, so the `registry::init(... discovered)` move that follows still works.

- [ ] **Step 3: Spawn the heartbeat right before the worker-pool spawn**

Find this block (around line 117-127):

```rust
            let bus = transcoderr::bus::Bus::default();
            let cancellations = transcoderr::cancellation::JobCancellations::new();
            let worker = transcoderr::worker::Worker::new(
                pool.clone(),
                bus.clone(),
                cfg.data_dir.clone(),
                cancellations.clone(),
            );
            let reset = worker.recover_on_boot().await?;
```

Add the heartbeat spawn right before `Worker::new`:

```rust
            let bus = transcoderr::bus::Bus::default();
            let cancellations = transcoderr::cancellation::JobCancellations::new();
            transcoderr::worker::local::spawn_local_heartbeat(pool.clone());
            let worker = transcoderr::worker::Worker::new(
                pool.clone(),
                bus.clone(),
                cfg.data_dir.clone(),
                cancellations.clone(),
            );
            let reset = worker.recover_on_boot().await?;
```

- [ ] **Step 4: Build smoke**

```bash
cargo build -p transcoderr 2>&1 | tail -5
```

Expected: clean build.

- [ ] **Step 5: Tests still green**

```bash
cargo test -p transcoderr --lib 2>&1 | grep -E "FAILED|^test result" | tail -5
```

Expected: no FAILED. (The integration tests still spawn via the old `common/mod.rs` boot path; that gets updated in Task 4.)

- [ ] **Step 6: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-2" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/main.rs
git commit -m "feat(boot): register local worker + spawn heartbeat at boot"
```

---

## Task 4: `tests/common/mod.rs` boot helper

Mirror the production boot path so integration tests exercise the same wiring.

**Files:**
- Modify: `crates/transcoderr/tests/common/mod.rs`

- [ ] **Step 1: Branch verification + read current state**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-2" || { echo "WRONG BRANCH"; exit 1; }
sed -n '30,65p' crates/transcoderr/tests/common/mod.rs
```

The current shape (line 33-62):
- `db::open(&data_dir)` → `pool`
- `HwCaps::default()` → `caps` + `hw_devices`
- `FfmpegCaps::default()` → `ffmpeg_caps`
- `steps::registry::init(pool, hw_devices, ffmpeg_caps, vec![])` → done
- ... config / cfg ...
- `Worker::new(...)` → `worker`
- `tokio::spawn(worker.run_loop(rx))` → background

- [ ] **Step 2: Insert `register_local_worker` before `Worker::new` and spawn heartbeat**

Find the block:

```rust
    let cancellations = transcoderr::cancellation::JobCancellations::new();
    let worker = Worker::new(pool.clone(), bus.clone(), data_dir.clone(), cancellations.clone());
```

Replace with:

```rust
    let cancellations = transcoderr::cancellation::JobCancellations::new();

    // Mirror the production boot path: register the local worker row
    // and start its heartbeat before spawning the pool. Tests rely on
    // `workers.enabled` being a real toggle the dispatcher honors.
    transcoderr::worker::local::register_local_worker(
        &pool,
        &ffmpeg_caps,
        &[],
    )
    .await
    .unwrap();
    transcoderr::worker::local::spawn_local_heartbeat(pool.clone());

    let worker = Worker::new(pool.clone(), bus.clone(), data_dir.clone(), cancellations.clone());
```

`ffmpeg_caps` here is an `Arc<FfmpegCaps>` — `&ffmpeg_caps` deref-coerces to `&FfmpegCaps` via `&*ffmpeg_caps`. If the compiler complains (e.g. the local arc shape differs), use `&*ffmpeg_caps` explicitly:

```rust
    transcoderr::worker::local::register_local_worker(
        &pool,
        &*ffmpeg_caps,
        &[],
    )
    .await
    .unwrap();
```

- [ ] **Step 3: Build smoke**

```bash
cargo build -p transcoderr --tests 2>&1 | tail -5
```

Expected: clean build. (`--tests` ensures the test binaries compile — including ones that link `common`.)

- [ ] **Step 4: Run a smoke integration test to confirm boot still works**

```bash
cargo test -p transcoderr --test webhook_dedup 2>&1 | tail -10
```

Expected: tests pass. (`webhook_dedup` is a small existing integration test that uses `boot()` — picks up any wiring breakage immediately.)

- [ ] **Step 5: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-2" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/tests/common/mod.rs
git commit -m "test(common): mirror production boot — register local worker + heartbeat"
```

---

## Task 5: `Worker::run_loop` consults `is_enabled`

The critical-path change. Surgical edit to the existing `run_loop`. **Pause for user confirmation after this task before continuing — a regression here breaks every flow run.**

**Files:**
- Modify: `crates/transcoderr/src/worker/pool.rs`

- [ ] **Step 1: Branch verification + read current `run_loop`**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-2" || { echo "WRONG BRANCH"; exit 1; }
sed -n '107,127p' crates/transcoderr/src/worker/pool.rs
```

Current shape:

```rust
    pub async fn run_loop(&self, shutdown: tokio::sync::watch::Receiver<bool>) {
        let mut shutdown = shutdown;
        loop {
            if *shutdown.borrow() { return; }
            match self.tick().await {
                Ok(true) => continue,
                Ok(false) => {
                    tokio::select! {
                        _ = shutdown.changed() => return,
                        _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "worker tick failed");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }
    }
```

- [ ] **Step 2: Add the `is_enabled` gate before `tick()`**

Replace the `run_loop` body with:

```rust
    pub async fn run_loop(&self, shutdown: tokio::sync::watch::Receiver<bool>) {
        let mut shutdown = shutdown;
        loop {
            if *shutdown.borrow() { return; }

            // Per-worker enable toggle (Piece 2). When the operator
            // disables the local worker, the in-flight job (if any)
            // finishes naturally inside `tick()`; subsequent claims
            // short-circuit here. Defaults to enabled on DB error.
            if !crate::worker::local::is_enabled(&self.pool).await {
                tokio::select! {
                    _ = shutdown.changed() => return,
                    _ = tokio::time::sleep(Duration::from_millis(500)) => continue,
                }
            }

            match self.tick().await {
                Ok(true) => continue,
                Ok(false) => {
                    tokio::select! {
                        _ = shutdown.changed() => return,
                        _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "worker tick failed");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }
    }
```

The gate fires on every iteration. The `select!` body matches the existing idle-backoff shape so shutdown still wins promptly.

- [ ] **Step 3: Build smoke**

```bash
cargo build -p transcoderr 2>&1 | tail -3
```

Expected: clean build.

- [ ] **Step 4: Run the existing flow / claim tests so we know the critical path is still green**

```bash
cargo test -p transcoderr --test concurrent_claim --test crash_recovery --test flow_engine 2>&1 | grep -E "FAILED|^test result" | tail -10
```

Expected: every line `test result: ok.`. No FAILED.

- [ ] **Step 5: Run the lib tests**

```bash
cargo test -p transcoderr --lib 2>&1 | grep -E "FAILED|^test result" | tail -5
```

Expected: no FAILED.

- [ ] **Step 6: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-2" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/worker/pool.rs
git commit -m "feat(worker): gate run_loop on workers.enabled"
```

**[PAUSE FOR USER CONFIRMATION HERE]**

---

## Task 6: `PATCH /api/workers/:id` endpoint

REST handler + route registration. Returns the updated `WorkerSummary`.

**Files:**
- Modify: `crates/transcoderr/src/api/workers.rs`
- Modify: `crates/transcoderr/src/api/mod.rs`

- [ ] **Step 1: Branch verification + add `PatchReq` and `patch`**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-2" || { echo "WRONG BRANCH"; exit 1; }
```

In `crates/transcoderr/src/api/workers.rs`, find the existing `delete` handler. Append directly after it:

```rust
#[derive(serde::Deserialize)]
pub struct PatchReq {
    pub enabled: Option<bool>,
}

/// PATCH /api/workers/:id — currently the only mutable field is
/// `enabled`. Returns the updated row as `WorkerSummary` (un-redacted —
/// PATCH is session/UI-authed, not a token-replay surface). 404 if id
/// missing; 400 if no settable fields supplied.
pub async fn patch(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<PatchReq>,
) -> Result<Json<WorkerSummary>, StatusCode> {
    let Some(enabled) = req.enabled else {
        return Err(StatusCode::BAD_REQUEST);
    };
    let n = db::workers::set_enabled(&state.pool, id, enabled)
        .await
        .map_err(|e| {
            tracing::error!(error = ?e, id, "failed to set workers.enabled");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    if n == 0 {
        return Err(StatusCode::NOT_FOUND);
    }
    let row = db::workers::get_by_id(&state.pool, id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    // PATCH is UI-driven (session-authed) — return un-redacted, same
    // policy as create() returning the cleartext mint.
    Ok(Json(row_to_summary(row, false)))
}
```

- [ ] **Step 2: Register the route**

In `crates/transcoderr/src/api/mod.rs`:

1. Update the `routing` import to include `patch`. Find:

```rust
use axum::{
    extract::State,
    middleware::from_fn_with_state,
    routing::{delete, get, post},
    Router,
};
```

Change to:

```rust
use axum::{
    extract::State,
    middleware::from_fn_with_state,
    routing::{delete, get, patch, post},
    Router,
};
```

2. Find the existing workers routes in the `protected` chain:

```rust
        .route("/workers",            get(workers::list).post(workers::create))
        .route("/workers/:id",        delete(workers::delete))
```

Add a chained `.patch(...)` to the `:id` route:

```rust
        .route("/workers",            get(workers::list).post(workers::create))
        .route("/workers/:id",        patch(workers::patch).delete(workers::delete))
```

- [ ] **Step 3: Build smoke**

```bash
cargo build -p transcoderr 2>&1 | tail -3
```

Expected: clean build.

- [ ] **Step 4: Tests still green**

```bash
cargo test -p transcoderr --lib 2>&1 | grep -E "FAILED|^test result" | tail -5
```

Expected: no FAILED.

- [ ] **Step 5: Manual smoke (optional but recommended)**

```bash
mkdir -p /tmp/p2-test && cat > /tmp/p2-test/config.toml <<'EOF'
bind = "127.0.0.1:8081"
data_dir = "/tmp/p2-test"
[radarr]
bearer_token = "test"
EOF
./target/debug/transcoderr serve --config /tmp/p2-test/config.toml &
SERVER_PID=$!
sleep 2

# Verify GET shows the local row populated.
curl -s http://127.0.0.1:8081/api/workers
# expect: enabled=true, hw_caps populated, last_seen_at recent

# Disable.
curl -s -X PATCH http://127.0.0.1:8081/api/workers/1 \
  -H "Content-Type: application/json" -d '{"enabled":false}'
# expect: WorkerSummary with enabled=false

# Re-enable.
curl -s -X PATCH http://127.0.0.1:8081/api/workers/1 \
  -H "Content-Type: application/json" -d '{"enabled":true}'
# expect: WorkerSummary with enabled=true

# 400 on empty body.
curl -s -o /dev/null -w "%{http_code}\n" -X PATCH http://127.0.0.1:8081/api/workers/1 \
  -H "Content-Type: application/json" -d '{}'
# expect: 400

# 404 on missing id.
curl -s -o /dev/null -w "%{http_code}\n" -X PATCH http://127.0.0.1:8081/api/workers/9999 \
  -H "Content-Type: application/json" -d '{"enabled":true}'
# expect: 404

kill $SERVER_PID
rm -rf /tmp/p2-test
```

- [ ] **Step 6: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-2" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/api/workers.rs crates/transcoderr/src/api/mod.rs
git commit -m "feat(api): PATCH /api/workers/:id (enabled toggle)"
```

**[PAUSE FOR USER CONFIRMATION HERE]**

---

## Task 7: Frontend — Enabled toggle + disabled badge

UI work: add the toggle column, recognize the "disabled" state in the badge logic, ship the api wrapper.

**Files:**
- Modify: `web/src/api/client.ts`
- Modify: `web/src/pages/workers.tsx`
- Modify: `web/src/index.css`

- [ ] **Step 1: Branch verification + add `api.workers.patch`**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-2" || { echo "WRONG BRANCH"; exit 1; }
```

In `web/src/api/client.ts`, find the existing `workers:` block:

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

Add a `patch` member after `create`:

```ts
  workers: {
    list:   () => req<import("../types").Worker[]>("/workers"),
    create: (name: string) =>
      req<import("../types").WorkerCreateResp>("/workers", {
        method: "POST",
        body: JSON.stringify({ name }),
      }),
    patch: (id: number, body: { enabled: boolean }) =>
      req<import("../types").Worker>(`/workers/${id}`, {
        method: "PATCH",
        body: JSON.stringify(body),
      }),
    delete: (id: number) => req<void>(`/workers/${id}`, { method: "DELETE" }),
  },
```

- [ ] **Step 2: Update `web/src/pages/workers.tsx`**

Read the current file:

```bash
sed -n '1,120p' web/src/pages/workers.tsx
```

Replace its entire content with:

```tsx
import { useState } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../api/client";
import type { Worker } from "../types";
import AddWorkerForm from "../components/forms/add-worker";

const STALE_AFTER_SECS = 90;

function formatSeen(
  now: number,
  last: number | null,
  enabled: boolean,
): { label: string; status: string } {
  if (!enabled) {
    // Disabled is a UI-only status — last_seen still updates because
    // the local heartbeat fires regardless of enable. The label stays
    // useful as a "yes, the daemon is alive, you just turned it off"
    // hint.
    if (last == null) return { label: "off", status: "disabled" };
    const age = now - last;
    if (age < 60) return { label: `off (${age}s ago)`, status: "disabled" };
    return { label: `off (${Math.floor(age / 60)}m ago)`, status: "disabled" };
  }
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

  const togg = useMutation({
    mutationFn: (vars: { id: number; enabled: boolean }) =>
      api.workers.patch(vars.id, { enabled: vars.enabled }),
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
          <div className="crumb">Configure</div>
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
              <th style={{ width: 90 }}>Enabled</th>
              <th style={{ width: 90 }}></th>
            </tr>
          </thead>
          <tbody>
            {(list.data ?? []).map((w: Worker) => {
              const seen = formatSeen(now, w.last_seen_at, w.enabled);
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
                    <input
                      type="checkbox"
                      checked={w.enabled}
                      disabled={togg.isPending}
                      onChange={(e) =>
                        togg.mutate({ id: w.id, enabled: e.target.checked })
                      }
                    />
                  </td>
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
              <tr><td colSpan={7} className="empty">No workers yet.</td></tr>
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

Key changes from the existing file:
- `formatSeen` takes `enabled` and returns `status: "disabled"` when off
- New "Enabled" column with a `<input type="checkbox">` calling `togg.mutate`
- `colSpan={6}` → `colSpan={7}` on the empty-state row
- Disabled toggle shows the daemon as "off (Ns ago)" so operators see the heartbeat is alive

- [ ] **Step 3: Add `.badge-disabled` to `web/src/index.css`**

```bash
grep -n "badge-connected\|badge-stale\|badge-offline" web/src/index.css | head
```

You should see lines like:

```css
.badge-connected { background: var(--ok-soft); color: var(--ok); }
.badge-stale     { background: var(--warn-soft); color: var(--warn); }
.badge-offline   { background: var(--neutral-soft); color: var(--neutral); }
```

Append directly below the `.badge-offline` line:

```css
.badge-disabled  { background: var(--neutral-soft); color: var(--muted); }
```

(Uses muted text on the neutral background — visually distinct from `.badge-offline`'s neutral-on-neutral. If `--muted` isn't a defined CSS variable in this project, fall back to `color: var(--neutral); opacity: 0.6;`.)

- [ ] **Step 4: Build smoke**

```bash
npm --prefix web run build 2>&1 | tail -10
```

Expected: clean build, no TypeScript errors.

- [ ] **Step 5: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-2" || { echo "WRONG BRANCH"; exit 1; }
git add web/src/api/client.ts web/src/pages/workers.tsx web/src/index.css
git commit -m "web: Enabled toggle on Workers page + disabled badge"
```

---

## Task 8: Integration tests `tests/local_worker.rs`

End-to-end exercises against the in-process router.

**Files:**
- Create: `crates/transcoderr/tests/local_worker.rs`

- [ ] **Step 1: Branch verification + create the file**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-2" || { echo "WRONG BRANCH"; exit 1; }
```

Create `crates/transcoderr/tests/local_worker.rs`:

```rust
//! Integration tests for the local-worker abstraction:
//! - boot populates the seeded `local` row
//! - heartbeat advances `last_seen_at`
//! - PATCH /api/workers/1 disabled stops claiming
//! - PATCH back to enabled resumes claiming

mod common;

use common::boot;
use serde_json::json;
use transcoderr::worker::local::LOCAL_WORKER_ID;

async fn submit_simple_flow_job(app: &common::TestApp) -> i64 {
    // Insert a flow that has a single trivial step the test worker can
    // run without ffmpeg / external deps. Reuse the flows API to keep
    // the test honest about the round-trip.
    let client = reqwest::Client::new();
    let yaml = "name: t\non: [\"manual\"]\nsteps:\n  - id: noop\n    kind: noop\n";
    let resp: serde_json::Value = client
        .post(format!("{}/api/flows", app.url))
        .json(&json!({"name": "t", "yaml_source": yaml}))
        .send().await.unwrap()
        .json().await.unwrap();
    let flow_id = resp["id"].as_i64().expect("flow id");

    // Insert a job directly into the queue (mirrors what the webhook
    // path does) — bypasses the *arr push so tests don't need fixture
    // payloads.
    sqlx::query(
        "INSERT INTO jobs (flow_id, file_path, status, created_at)
         VALUES (?, '/tmp/test.mkv', 'pending', strftime('%s','now'))
         RETURNING id",
    )
    .bind(flow_id)
    .execute(&app.pool)
    .await
    .unwrap();

    let id: i64 = sqlx::query_scalar(
        "SELECT id FROM jobs WHERE flow_id = ? ORDER BY id DESC LIMIT 1",
    )
    .bind(flow_id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    id
}

async fn job_status(pool: &sqlx::SqlitePool, id: i64) -> String {
    sqlx::query_scalar::<_, String>("SELECT status FROM jobs WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn local_row_populated_after_boot() {
    let app = boot().await;

    // boot() already ran register_local_worker (Task 4). Verify the
    // seeded row is now stamped with hw_caps + plugin_manifest +
    // last_seen_at.
    let row: (Option<String>, Option<String>, Option<i64>) = sqlx::query_as(
        "SELECT hw_caps_json, plugin_manifest_json, last_seen_at
           FROM workers WHERE id = ?",
    )
    .bind(LOCAL_WORKER_ID)
    .fetch_one(&app.pool)
    .await
    .unwrap();

    assert!(row.0.is_some(), "hw_caps_json must be populated");
    assert!(row.1.is_some(), "plugin_manifest_json must be populated");
    assert!(row.2.is_some(), "last_seen_at must be set");
}

#[tokio::test]
async fn heartbeat_advances_last_seen_when_idle() {
    let app = boot().await;

    let initial: i64 = sqlx::query_scalar(
        "SELECT COALESCE(last_seen_at, 0) FROM workers WHERE id = ?",
    )
    .bind(LOCAL_WORKER_ID)
    .fetch_one(&app.pool)
    .await
    .unwrap();

    // We don't wait the real 30s tick. Force one explicit heartbeat
    // after a >1s pause so unix-second granularity advances.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    transcoderr::db::workers::record_heartbeat(&app.pool, LOCAL_WORKER_ID)
        .await
        .unwrap();

    let after: i64 = sqlx::query_scalar(
        "SELECT COALESCE(last_seen_at, 0) FROM workers WHERE id = ?",
    )
    .bind(LOCAL_WORKER_ID)
    .fetch_one(&app.pool)
    .await
    .unwrap();

    assert!(after > initial, "heartbeat must advance last_seen_at (was {initial}, now {after})");
}

#[tokio::test]
async fn disabled_local_worker_drains_and_stops_claiming() {
    let app = boot().await;
    let client = reqwest::Client::new();

    // Disable the local worker.
    let resp = client
        .patch(format!("{}/api/workers/{LOCAL_WORKER_ID}", app.url))
        .json(&json!({"enabled": false}))
        .send().await.unwrap();
    assert!(resp.status().is_success(), "PATCH must succeed");

    // Submit a job. Pool is gated; it should stay pending.
    let job_id = submit_simple_flow_job(&app).await;

    // Wait long enough for the pool's 500ms gate to have re-checked
    // multiple times.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let status = job_status(&app.pool, job_id).await;
    assert_eq!(
        status, "pending",
        "disabled local worker must not claim jobs (got {status})"
    );
}

#[tokio::test]
async fn re_enabling_resumes_dispatch() {
    let app = boot().await;
    let client = reqwest::Client::new();

    // Disable, submit, re-enable.
    client
        .patch(format!("{}/api/workers/{LOCAL_WORKER_ID}", app.url))
        .json(&json!({"enabled": false}))
        .send().await.unwrap();

    let job_id = submit_simple_flow_job(&app).await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert_eq!(job_status(&app.pool, job_id).await, "pending");

    client
        .patch(format!("{}/api/workers/{LOCAL_WORKER_ID}", app.url))
        .json(&json!({"enabled": true}))
        .send().await.unwrap();

    // Poll for up to 5s for the job to leave 'pending'.
    let mut left_pending = false;
    for _ in 0..50 {
        let s = job_status(&app.pool, job_id).await;
        if s != "pending" {
            left_pending = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(left_pending, "re-enabling must let the pool claim");
}
```

Note: the `submit_simple_flow_job` helper uses a `kind: noop` step. If the existing test fixtures already have a no-op flow (check `crates/transcoderr/tests/common/`), reuse it. If `kind: noop` isn't a registered step in the test boot path, the noop here will fail at run time but the *job-claim* assertion in tests 3 and 4 still works — the test only cares whether `claim_next` ran (status moves off `pending`), not whether the flow completes. If the test infrastructure rejects an unknown step kind at *insert* time, replace `kind: noop` with a known no-op step from the codebase (search `crates/transcoderr/src/steps/` for one — `kind: extract.subs` with no inputs typically resolves to a no-op pass-through; alternatively use the same pattern other integration tests use).

- [ ] **Step 2: Run the new test file**

```bash
cargo test -p transcoderr --test local_worker 2>&1 | tail -15
```

Expected: 4 passed. If the `submit_simple_flow_job` helper fails because `noop` isn't a registered step, look at how an existing integration test (e.g. `crash_recovery.rs` or `concurrent_claim.rs`) inserts jobs and copy that pattern verbatim.

- [ ] **Step 3: Run the full integration suite to confirm no regressions**

```bash
cargo test -p transcoderr --tests 2>&1 | grep -E "FAILED|^test result" | tail -20
```

Expected: every line `test result: ok.`. No FAILED.

- [ ] **Step 4: Run the full lib suite**

```bash
cargo test -p transcoderr --lib 2>&1 | grep -E "FAILED|^test result" | tail -5
```

Expected: no FAILED.

- [ ] **Step 5: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-2" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/tests/local_worker.rs
git commit -m "test(worker): local-worker abstraction integration tests"
```

---

## Self-Review Notes

This plan covers every section of the spec:

- **Boot registration in-process** → Task 2 (`register_local_worker`) + Task 3 (main.rs wiring) + Task 4 (test fixture wiring).
- **30s heartbeat regardless of enable** → Task 2 (`spawn_local_heartbeat`) + Task 3 (main.rs spawn).
- **Pool's `run_loop` consults `is_enabled` before claim** → Task 5.
- **`workers.enabled` defaults true on DB error** → Task 2 (`is_enabled` warn-and-true fallback).
- **PATCH /api/workers/:id with body `{enabled}` returning `WorkerSummary`** → Task 6.
- **400/404/500 error mapping** → Task 6 handler.
- **`set_enabled` DB function** → Task 1.
- **`LOCAL_WORKER_ID = 1` constant pinned to the migration** → Task 2.
- **No DB migration** → confirmed (Tasks 1, 2 use existing columns).
- **UI: per-row toggle on /workers + `disabled` badge variant** → Task 7.
- **Graceful drain (in-flight finishes; new claims short-circuit)** → Task 5 places the gate before `tick`, so `tick`'s in-progress run completes before the next iteration sees the gate.
- **No new web migration / no token for local row** → confirmed by Task 2's `record_register` call without touching `secret_token`.
- **Existing-install upgrade path** → no schema change; first boot of the new binary populates the row's nullable columns (Task 3).
- **4 integration scenarios** → Task 8 covers all four named in the spec.
- **Unit tests for `set_enabled`, `is_enabled`** → Task 1 + Task 2 (3 unit tests total).

Cross-task consistency check:
- `LOCAL_WORKER_ID = 1` defined once in `worker/local.rs` (Task 2), referenced from Tasks 3, 4, 6, 8 — name + type match.
- `register_local_worker(&pool, &ffmpeg_caps, &discovered)` signature: `&SqlitePool`, `&FfmpegCaps`, `&[DiscoveredPlugin]`. Tasks 3 and 4 both pass these by ref — Task 4 passes `&[]` for the plugin slice (test fixture uses no plugins, matching the existing `registry::init(... vec![])` in common/mod.rs).
- `spawn_local_heartbeat(pool: SqlitePool)` takes ownership of a clone — Task 3 calls `spawn_local_heartbeat(pool.clone())`, Task 4 same.
- `is_enabled(&self.pool)` in Task 5 → `pub async fn is_enabled(pool: &SqlitePool) -> bool` in Task 2. Match.
- `db::workers::set_enabled(pool, id, enabled) -> anyhow::Result<u64>` (Task 1) → called from `is_enabled_returns_column_value` test (Task 2) and the PATCH handler (Task 6). Signatures align.
- `PatchReq { enabled: Option<bool> }` → API client sends `{ enabled: boolean }` (Task 7). On the wire it's just JSON `{"enabled": true|false}`; Rust accepts it as `Option<bool> = Some(...)` and the handler 400s on `None`.
