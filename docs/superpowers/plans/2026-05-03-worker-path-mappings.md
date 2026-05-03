# Per-Worker Path Mappings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Operator-configured path prefix translations per remote worker, applied transparently by the coordinator's dispatcher in both directions (coord→worker on dispatch, worker→coord on completion).

**Architecture:** A new pure-data `path_mapping` module (PathMapping / PathMappings / Direction) walks the JSON `Context` snapshot rewriting string leaves whose value starts with a configured `from:` prefix. Mappings are stored in a new nullable `path_mappings_json` column on the existing `workers` table, cached in the `Connections` registry, and invalidated by a new `PUT /api/workers/:id/path-mappings` endpoint. `RemoteRunner::run` snapshots the per-worker mappings at dispatch time and uses the same snapshot for the reverse pass on `StepComplete`. Workers and plugins do not change.

**Tech Stack:** Rust 2021 (axum 0.7, sqlx + sqlite, anyhow, tokio, serde_json), React + Vite (web UI). No new crates.

**Branch:** all tasks land on a fresh `feat/worker-path-mappings` branch off `main`. The implementer creates the branch before Task 1.

---

## File Structure

**New backend files:**
- `crates/transcoderr/migrations/20260503000001_worker_path_mappings.sql` — `ALTER TABLE workers ADD COLUMN path_mappings_json TEXT`.
- `crates/transcoderr/src/path_mapping.rs` — pure-data module with `PathMapping`, `PathMappings`, `Direction`, and `apply`. ~200 lines including 8 unit tests.
- `crates/transcoderr/tests/worker_path_mappings_api.rs` — 4 API integration tests.
- `crates/transcoderr/tests/path_mapping.rs` — single end-to-end integration test.

**New web UI files:**
- `web/src/components/path-mappings-modal.tsx` — modal component.

**Modified backend files:**
- `crates/transcoderr/src/lib.rs` — `pub mod path_mapping;`.
- `crates/transcoderr/src/db/workers.rs` — `WorkerRow.path_mappings_json` field; `update_path_mappings` helper; existing SELECT queries gain the new column.
- `crates/transcoderr/src/worker/connections.rs` — new `path_mappings: HashMap<i64, PathMappings>` cache + `path_mappings_for` / `set_path_mappings` accessors + cleared on disconnect.
- `crates/transcoderr/src/api/workers.rs` — new `path_mappings` field on `WorkerSummary`; new `set_path_mappings` handler.
- `crates/transcoderr/src/api/mod.rs` — new `PUT /api/workers/:id/path-mappings` route.
- `crates/transcoderr/src/dispatch/remote.rs` — load mappings; rewrite `ctx_snapshot` outbound; reverse-rewrite on `StepComplete`.

**Modified web UI files:**
- `web/src/pages/workers.tsx` — "Edit mappings" button per remote worker.
- `web/src/api/client.ts` — `api.workers.updatePathMappings`.

**No new dependencies. No protocol change.** Workers are entirely unaware of mappings.

---

## Task 1: DB migration + `WorkerRow.path_mappings_json` + `update_path_mappings` helper

Schema-only additive change. New nullable column on `workers`; new helper that refuses `kind='local'` rows. Existing SELECT queries gain the new column so `WorkerRow` round-trips cleanly.

**Files:**
- Create: `crates/transcoderr/migrations/20260503000001_worker_path_mappings.sql`
- Modify: `crates/transcoderr/src/db/workers.rs`

- [ ] **Step 1: Branch verification + branch create**

```bash
git checkout main && git pull --ff-only
git checkout -b feat/worker-path-mappings
test "$(git branch --show-current)" = "feat/worker-path-mappings" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Add the migration**

Create `crates/transcoderr/migrations/20260503000001_worker_path_mappings.sql`:

```sql
-- Per-worker path mapping rules (spec
-- 2026-05-03-worker-path-mappings-design.md). NULL = identity (current
-- behavior). Stores `[{"from": "...", "to": "..."}, ...]` for
-- kind='remote' rows; kind='local' rows must keep this NULL.
ALTER TABLE workers ADD COLUMN path_mappings_json TEXT;
```

- [ ] **Step 3: Extend `WorkerRow` and SELECTs in `db/workers.rs`**

In `crates/transcoderr/src/db/workers.rs`, add the new field to `WorkerRow`:

```rust
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct WorkerRow {
    pub id: i64,
    pub name: String,
    pub kind: String,
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
    /// JSON array of `{from, to}` rules; NULL = identity (no mapping).
    /// kind='local' rows always keep this NULL.
    #[sqlx(default)]
    pub path_mappings_json: Option<String>,
}
```

Update each existing SELECT query in this file to include `path_mappings_json` in the column list. There are three:

```rust
// list_all
"SELECT id, name, kind, secret_token, hw_caps_json, plugin_manifest_json,
        enabled, last_seen_at, created_at, path_mappings_json
   FROM workers
  ORDER BY id"

// get_by_id
"SELECT id, name, kind, secret_token, hw_caps_json, plugin_manifest_json,
        enabled, last_seen_at, created_at, path_mappings_json
   FROM workers WHERE id = ?"

// get_by_token
"SELECT id, name, kind, secret_token, hw_caps_json, plugin_manifest_json,
        enabled, last_seen_at, created_at, path_mappings_json
   FROM workers WHERE secret_token = ?"
```

- [ ] **Step 4: Add `update_path_mappings` helper**

Append to `db/workers.rs` (alongside `set_enabled`):

```rust
/// Update the per-worker path-mapping rules. Pass `None` (or
/// `Some("[]")` from the API layer turned into None) to clear the
/// column. Refuses `kind='local'` rows — returns `Ok(0)`. The API
/// layer turns 0 into a 400.
pub async fn update_path_mappings(
    pool: &SqlitePool,
    id: i64,
    json: Option<&str>,
) -> anyhow::Result<u64> {
    let res = sqlx::query(
        "UPDATE workers
            SET path_mappings_json = ?
          WHERE id = ? AND kind = 'remote'",
    )
    .bind(json)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}
```

- [ ] **Step 5: Add unit tests at the bottom of `db/workers.rs`**

Inside the existing `#[cfg(test)] mod tests` block, append:

```rust
    #[tokio::test]
    async fn update_path_mappings_round_trips() {
        let (pool, _dir) = pool().await;
        let id = insert_remote(&pool, "gpu-1", "wkr_xxx").await.unwrap();
        let n = update_path_mappings(
            &pool,
            id,
            Some(r#"[{"from":"/mnt","to":"/data"}]"#),
        )
        .await
        .unwrap();
        assert_eq!(n, 1);
        let row = get_by_id(&pool, id).await.unwrap().unwrap();
        assert_eq!(
            row.path_mappings_json.as_deref(),
            Some(r#"[{"from":"/mnt","to":"/data"}]"#)
        );
    }

    #[tokio::test]
    async fn update_path_mappings_clears_to_null() {
        let (pool, _dir) = pool().await;
        let id = insert_remote(&pool, "gpu-1", "wkr_xxx").await.unwrap();
        update_path_mappings(&pool, id, Some(r#"[{"from":"/a","to":"/b"}]"#))
            .await
            .unwrap();
        let n = update_path_mappings(&pool, id, None).await.unwrap();
        assert_eq!(n, 1);
        let row = get_by_id(&pool, id).await.unwrap().unwrap();
        assert!(row.path_mappings_json.is_none());
    }

    #[tokio::test]
    async fn update_path_mappings_refuses_local_row() {
        let (pool, _dir) = pool().await;
        // id=1 is the seeded local row.
        let n = update_path_mappings(
            &pool,
            1,
            Some(r#"[{"from":"/a","to":"/b"}]"#),
        )
        .await
        .unwrap();
        assert_eq!(n, 0, "kind='local' must reject path mapping updates");
        let row = get_by_id(&pool, 1).await.unwrap().unwrap();
        assert!(row.path_mappings_json.is_none());
    }
```

- [ ] **Step 6: Build smoke**

```bash
cargo build -p transcoderr 2>&1 | tail -5
```

Expected: clean.

- [ ] **Step 7: Run the new unit tests**

```bash
cargo test -p transcoderr --lib db::workers::tests 2>&1 | tail -10
```

Expected: 8 passed (5 existing + 3 new).

- [ ] **Step 8: Lib + critical-path tests still green**

```bash
cargo test -p transcoderr --lib 2>&1 | grep -E "FAILED|^test result" | tail -3
cargo test -p transcoderr --test worker_connect --test remote_dispatch --test plugin_remote_dispatch 2>&1 | grep -E "FAILED|^test result" | tail -10
```

Expected: no FAILED.

- [ ] **Step 9: Commit**

```bash
test "$(git branch --show-current)" = "feat/worker-path-mappings" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/migrations/20260503000001_worker_path_mappings.sql \
        crates/transcoderr/src/db/workers.rs
git commit -m "feat(db): workers.path_mappings_json column + update helper"
```

---

## Task 2: Pure-data `path_mapping` module + 8 unit tests

The whole rewriting algorithm lives here as a single self-contained module. No I/O, no async, no integration. Eight unit tests lock down the boundary, longest-prefix, recursion, and round-trip semantics.

**Files:**
- Create: `crates/transcoderr/src/path_mapping.rs`
- Modify: `crates/transcoderr/src/lib.rs`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/worker-path-mappings" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Create `path_mapping.rs`**

Create `crates/transcoderr/src/path_mapping.rs` with the following content:

```rust
//! Per-worker path mapping: walks a `serde_json::Value` and rewrites
//! string leaves whose value starts with a configured prefix.
//!
//! Spec: `docs/superpowers/specs/2026-05-03-worker-path-mappings-design.md`
//!
//! - **Boundary rule**: a rule with `from = "/mnt/movies"` matches
//!   `"/mnt/movies"` exactly OR `"/mnt/movies/anything"`, but NOT
//!   `"/mnt/movies-archive/Y.mkv"`. After stripping `from` from the
//!   leading edge, the next char must be `/` or end-of-string.
//! - **Longest-`from` wins**: rules are sorted by `from.len()` desc on
//!   construction; the first match in that order is applied.
//! - **Trailing slash normalisation**: `from = "/mnt/movies/"` and
//!   `from = "/mnt/movies"` produce identical match behavior. We
//!   normalise on construction (strip a single trailing `/`) so display
//!   stays consistent. Same for `to`.
//! - **Reverse direction** swaps `from` ↔ `to` at apply time; no
//!   separate sorted vector is needed.
//! - Object **keys** are not rewritten — paths live in values.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathMapping {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Default)]
pub struct PathMappings {
    /// Rules sorted by `from.len()` desc so the first matching prefix
    /// is the longest one.
    rules: Vec<PathMapping>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Outbound: rewrite coordinator paths to worker paths.
    /// (replace `from` prefix with `to`.)
    CoordToWorker,
    /// Inbound: rewrite worker paths back to coordinator paths.
    /// (replace `to` prefix with `from`.)
    WorkerToCoord,
}

impl PathMappings {
    /// Construct from already-validated rules. Empty vec → identity.
    pub fn from_rules(rules: Vec<PathMapping>) -> Self {
        let mut rules: Vec<PathMapping> = rules
            .into_iter()
            .map(|r| PathMapping {
                from: strip_trailing_slash(r.from),
                to: strip_trailing_slash(r.to),
            })
            .collect();
        rules.sort_by(|a, b| b.from.len().cmp(&a.from.len()));
        Self { rules }
    }

    /// Parse a `path_mappings_json` column value. NULL/empty → identity.
    /// Returns Err only on malformed JSON.
    pub fn from_json(s: &str) -> anyhow::Result<Self> {
        if s.trim().is_empty() {
            return Ok(Self::default());
        }
        let parsed: Vec<PathMapping> = serde_json::from_str(s)?;
        Ok(Self::from_rules(parsed))
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// View the underlying rules (post-normalisation). Used by the API
    /// layer to echo back what was stored.
    pub fn rules(&self) -> &[PathMapping] {
        &self.rules
    }

    /// Walk `value` in place, rewriting string leaves that match a
    /// rule's prefix. No-op if `is_empty()`.
    pub fn apply(&self, value: &mut Value, dir: Direction) {
        if self.is_empty() {
            return;
        }
        walk(value, &self.rules, dir);
    }
}

fn walk(value: &mut Value, rules: &[PathMapping], dir: Direction) {
    match value {
        Value::String(s) => {
            if let Some(replaced) = try_replace(s, rules, dir) {
                *s = replaced;
            }
        }
        Value::Array(items) => {
            for item in items {
                walk(item, rules, dir);
            }
        }
        Value::Object(map) => {
            for (_k, v) in map.iter_mut() {
                walk(v, rules, dir);
            }
        }
        // Numbers, booleans, nulls — untouched.
        _ => {}
    }
}

/// Returns the rewritten string if any rule matches, else None.
fn try_replace(s: &str, rules: &[PathMapping], dir: Direction) -> Option<String> {
    for rule in rules {
        let (lhs, rhs) = match dir {
            Direction::CoordToWorker => (&rule.from, &rule.to),
            Direction::WorkerToCoord => (&rule.to, &rule.from),
        };
        if let Some(rest) = s.strip_prefix(lhs.as_str()) {
            // Boundary: the next char (or end-of-string) must be '/'
            // so `/mnt/movies` does NOT match `/mnt/movies-archive/...`.
            if rest.is_empty() || rest.starts_with('/') {
                return Some(format!("{rhs}{rest}"));
            }
        }
    }
    None
}

fn strip_trailing_slash(s: String) -> String {
    if s.len() > 1 && s.ends_with('/') {
        s.trim_end_matches('/').to_string()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rule(from: &str, to: &str) -> PathMapping {
        PathMapping { from: from.into(), to: to.into() }
    }

    #[test]
    fn empty_mappings_is_identity() {
        let m = PathMappings::default();
        assert!(m.is_empty());
        let mut v = json!({"file": {"path": "/mnt/movies/X.mkv"}});
        let snapshot = v.clone();
        m.apply(&mut v, Direction::CoordToWorker);
        assert_eq!(v, snapshot);
    }

    #[test]
    fn single_rule_rewrites_string_leaf() {
        let m = PathMappings::from_rules(vec![rule("/mnt/movies", "/data/media/movies")]);
        let mut v = json!({"file": {"path": "/mnt/movies/X.mkv"}});
        m.apply(&mut v, Direction::CoordToWorker);
        assert_eq!(v["file"]["path"], json!("/data/media/movies/X.mkv"));
    }

    #[test]
    fn longest_prefix_wins() {
        let m = PathMappings::from_rules(vec![
            rule("/mnt/movies", "/data/media/movies"),
            rule("/mnt/movies/4k", "/data/4k"),
        ]);
        let mut v = json!({"file": {"path": "/mnt/movies/4k/X.mkv"}});
        m.apply(&mut v, Direction::CoordToWorker);
        // The longer "/mnt/movies/4k" wins over "/mnt/movies".
        assert_eq!(v["file"]["path"], json!("/data/4k/X.mkv"));
    }

    #[test]
    fn path_component_boundary_respected() {
        // /mnt/movies must NOT rewrite /mnt/movies-archive/...
        let m = PathMappings::from_rules(vec![rule("/mnt/movies", "/data/media/movies")]);
        let mut v = json!({
            "a": "/mnt/movies/X.mkv",          // matches
            "b": "/mnt/movies-archive/Y.mkv",  // does NOT match (boundary)
            "c": "/mnt/movies",                // matches exactly (end-of-string)
        });
        m.apply(&mut v, Direction::CoordToWorker);
        assert_eq!(v["a"], json!("/data/media/movies/X.mkv"));
        assert_eq!(v["b"], json!("/mnt/movies-archive/Y.mkv"));
        assert_eq!(v["c"], json!("/data/media/movies"));
    }

    #[test]
    fn reverse_round_trip() {
        let m = PathMappings::from_rules(vec![
            rule("/mnt/movies", "/data/media/movies"),
            rule("/mnt/tv", "/data/media/tv"),
        ]);
        let original = json!({
            "file": {"path": "/mnt/movies/X.mkv", "size_bytes": 12345678},
            "steps": {
                "tx": {"output_path": "/mnt/tv/Y.transcoded.mkv"},
                "size_report": {"before_bytes": 9999, "msg": "ok"}
            }
        });
        let mut v = original.clone();
        m.apply(&mut v, Direction::CoordToWorker);
        assert_ne!(v, original, "forward must change something");
        m.apply(&mut v, Direction::WorkerToCoord);
        assert_eq!(v, original, "round-trip must restore the original");
    }

    #[test]
    fn walks_nested_objects_and_arrays() {
        let m = PathMappings::from_rules(vec![rule("/mnt", "/data")]);
        let mut v = json!({
            "file": {"path": "/mnt/X.mkv"},
            "steps": {
                "tx": {"output_path": "/mnt/X.transcoded.mkv"}
            },
            "extras": ["/mnt/A", "/other/B", {"nested": "/mnt/C"}]
        });
        m.apply(&mut v, Direction::CoordToWorker);
        assert_eq!(v["file"]["path"], json!("/data/X.mkv"));
        assert_eq!(v["steps"]["tx"]["output_path"], json!("/data/X.transcoded.mkv"));
        assert_eq!(v["extras"][0], json!("/data/A"));
        assert_eq!(v["extras"][1], json!("/other/B"), "non-matching prefix untouched");
        assert_eq!(v["extras"][2]["nested"], json!("/data/C"));
    }

    #[test]
    fn non_string_leaves_untouched() {
        let m = PathMappings::from_rules(vec![rule("/mnt/movies", "/data/media/movies")]);
        // Object keys that look like paths must NOT be rewritten — only
        // values. Numbers, bools, nulls untouched.
        let mut v = json!({
            "/mnt/movies": "leave-the-key-alone",
            "size": 12345,
            "ok": true,
            "missing": null
        });
        let snapshot = v.clone();
        m.apply(&mut v, Direction::CoordToWorker);
        assert_eq!(v, snapshot);
    }

    #[test]
    fn trailing_slash_normalisation() {
        // from = "/mnt/movies/" must behave identically to "/mnt/movies".
        let with_slash = PathMappings::from_rules(vec![rule("/mnt/movies/", "/data/media/movies/")]);
        let without_slash = PathMappings::from_rules(vec![rule("/mnt/movies", "/data/media/movies")]);

        let input = json!({"path": "/mnt/movies/X.mkv"});

        let mut a = input.clone();
        with_slash.apply(&mut a, Direction::CoordToWorker);

        let mut b = input.clone();
        without_slash.apply(&mut b, Direction::CoordToWorker);

        assert_eq!(a, b, "trailing slash must be normalised on construction");
        assert_eq!(a["path"], json!("/data/media/movies/X.mkv"));
    }

    #[test]
    fn from_json_round_trips() {
        let s = r#"[{"from":"/mnt/a","to":"/data/a"},{"from":"/mnt/b","to":"/data/b"}]"#;
        let m = PathMappings::from_json(s).unwrap();
        assert_eq!(m.rules().len(), 2);
        // Sorted by from.len() desc — both have the same length here.
        // Importantly: empty/whitespace input → identity, no error.
        assert!(PathMappings::from_json("").unwrap().is_empty());
        assert!(PathMappings::from_json("   ").unwrap().is_empty());
        // Malformed → Err.
        assert!(PathMappings::from_json("not json").is_err());
    }
}
```

- [ ] **Step 3: Wire into the lib**

In `crates/transcoderr/src/lib.rs`, add (alphabetically near `path_*` or `plugins`):

```rust
pub mod path_mapping;
```

- [ ] **Step 4: Build smoke**

```bash
cargo build -p transcoderr 2>&1 | tail -5
```

Expected: clean.

- [ ] **Step 5: Run the new unit tests**

```bash
cargo test -p transcoderr --lib path_mapping::tests 2>&1 | tail -15
```

Expected: 9 passed (the 8 listed in the spec plus `from_json_round_trips`).

- [ ] **Step 6: Lib tests still green**

```bash
cargo test -p transcoderr --lib 2>&1 | grep -E "FAILED|^test result" | tail -3
```

Expected: no FAILED.

- [ ] **Step 7: Commit**

```bash
test "$(git branch --show-current)" = "feat/worker-path-mappings" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/path_mapping.rs \
        crates/transcoderr/src/lib.rs
git commit -m "feat(path_mapping): walk-the-tree prefix replace module"
```

---

## Task 3: `Connections` registry per-worker mappings cache

Add a `path_mappings: Arc<RwLock<HashMap<i64, PathMappings>>>` cache alongside the existing `available_steps` map. Cleared on disconnect (via `SenderGuard::drop`). The PUT API endpoint (Task 4) and the `RemoteRunner` (Task 6) both read from this; the API endpoint writes to it.

**Files:**
- Modify: `crates/transcoderr/src/worker/connections.rs`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/worker-path-mappings" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Add the cache field + accessors**

In `crates/transcoderr/src/worker/connections.rs`, extend the `Connections` struct:

```rust
#[derive(Default)]
pub struct Connections {
    senders: Arc<RwLock<HashMap<i64, mpsc::Sender<Envelope>>>>,
    inbox: Arc<RwLock<HashMap<String, mpsc::Sender<InboundStepEvent>>>>,
    /// Per-worker advertised step kinds. Populated on initial register
    /// AND on every re-register. Cleared by `SenderGuard::drop` when
    /// the worker disconnects. Used by `dispatch::eligible_remotes`
    /// to filter workers that can't run a given step kind.
    available_steps: Arc<RwLock<HashMap<i64, Vec<String>>>>,
    /// Per-worker path-mapping rules (spec
    /// `2026-05-03-worker-path-mappings-design.md`). Populated lazily
    /// on first dispatch (loaded from `workers.path_mappings_json`)
    /// and refreshed by the `PUT /api/workers/:id/path-mappings`
    /// endpoint. Cleared by `SenderGuard::drop` on disconnect so a
    /// reconnect re-loads from the DB. Empty `PathMappings` (via
    /// `Self::default()`) means identity.
    path_mappings: Arc<RwLock<HashMap<i64, crate::path_mapping::PathMappings>>>,
}
```

In the same file, extend `SenderGuard` to also clear the new map:

```rust
pub struct SenderGuard {
    map: Arc<RwLock<HashMap<i64, mpsc::Sender<Envelope>>>>,
    available_steps: Arc<RwLock<HashMap<i64, Vec<String>>>>,
    path_mappings: Arc<RwLock<HashMap<i64, crate::path_mapping::PathMappings>>>,
    worker_id: i64,
}

impl Drop for SenderGuard {
    fn drop(&mut self) {
        let map = self.map.clone();
        let available_steps = self.available_steps.clone();
        let path_mappings = self.path_mappings.clone();
        let worker_id = self.worker_id;
        tokio::spawn(async move {
            map.write().await.remove(&worker_id);
            available_steps.write().await.remove(&worker_id);
            path_mappings.write().await.remove(&worker_id);
        });
    }
}
```

In `register_sender`, populate the new field on the guard:

```rust
    pub async fn register_sender(
        self: &Arc<Self>,
        worker_id: i64,
        tx: mpsc::Sender<Envelope>,
    ) -> SenderGuard {
        self.senders.write().await.insert(worker_id, tx);
        SenderGuard {
            map: self.senders.clone(),
            available_steps: self.available_steps.clone(),
            path_mappings: self.path_mappings.clone(),
            worker_id,
        }
    }
```

- [ ] **Step 3: Add the cache accessors**

Append to the `impl Connections` block (next to `record_available_steps`):

```rust
    /// Set the per-worker path-mapping cache entry. Called by the PUT
    /// API endpoint after a successful DB update (cache invalidation),
    /// AND lazily by `RemoteRunner` on the first dispatch to a worker
    /// after register (cache fill). An empty `PathMappings` means the
    /// worker has no mappings configured (identity).
    pub async fn set_path_mappings(
        &self,
        worker_id: i64,
        mappings: crate::path_mapping::PathMappings,
    ) {
        self.path_mappings.write().await.insert(worker_id, mappings);
    }

    /// Look up the cached mappings for this worker. Returns `None` if
    /// no entry exists yet (caller should populate it lazily from the
    /// DB) — distinct from `Some(empty)` which means "we know there
    /// are no mappings, identity translation".
    pub async fn path_mappings_for(
        &self,
        worker_id: i64,
    ) -> Option<crate::path_mapping::PathMappings> {
        self.path_mappings.read().await.get(&worker_id).cloned()
    }
```

- [ ] **Step 4: Add unit tests**

Inside the existing `#[cfg(test)] mod tests` block, append:

```rust
    #[tokio::test]
    async fn set_and_query_path_mappings() {
        let conns = Connections::new();
        let mappings = crate::path_mapping::PathMappings::from_rules(vec![
            crate::path_mapping::PathMapping {
                from: "/mnt".into(),
                to: "/data".into(),
            },
        ]);
        conns.set_path_mappings(7, mappings).await;
        let got = conns.path_mappings_for(7).await.expect("entry exists");
        assert!(!got.is_empty());
        assert!(conns.path_mappings_for(999).await.is_none(),
            "missing worker → None, distinct from Some(empty)");
    }

    #[tokio::test]
    async fn sender_guard_drop_clears_path_mappings_too() {
        let conns = Connections::new();
        let (tx, _rx) = mpsc::channel(4);
        {
            let _guard = conns.register_sender(13, tx).await;
            conns
                .set_path_mappings(
                    13,
                    crate::path_mapping::PathMappings::from_rules(vec![
                        crate::path_mapping::PathMapping {
                            from: "/a".into(),
                            to: "/b".into(),
                        },
                    ]),
                )
                .await;
            assert!(conns.path_mappings_for(13).await.is_some());
        }
        // Drop spawns an async cleanup; give it a moment.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(conns.path_mappings_for(13).await.is_none(),
            "entry must be cleared on disconnect");
    }
```

- [ ] **Step 5: Build smoke**

```bash
cargo build -p transcoderr 2>&1 | tail -5
```

Expected: clean.

- [ ] **Step 6: Run the new unit tests**

```bash
cargo test -p transcoderr --lib worker::connections::tests 2>&1 | tail -15
```

Expected: 8 passed (6 existing + 2 new).

- [ ] **Step 7: Worker-side regression net**

```bash
cargo test -p transcoderr --test worker_connect --test remote_dispatch --test plugin_remote_dispatch --test cancel_remote 2>&1 | grep -E "FAILED|^test result" | tail -10
```

Expected: no FAILED.

- [ ] **Step 8: Commit**

```bash
test "$(git branch --show-current)" = "feat/worker-path-mappings" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/worker/connections.rs
git commit -m "feat(connections): per-worker path mappings cache + drop cleanup"
```

---

## Task 4: `PUT /api/workers/:id/path-mappings` endpoint + 4 API tests

Authenticated endpoint. Validates non-empty `from`/`to`, refuses `kind='local'`, normalises trailing slashes via `PathMappings::from_rules`, persists the canonical JSON via `db::workers::update_path_mappings`, and refreshes the `Connections` cache via `set_path_mappings`. Empty `rules: []` → DB stores NULL.

**Files:**
- Modify: `crates/transcoderr/src/api/workers.rs`
- Modify: `crates/transcoderr/src/api/mod.rs`
- Create: `crates/transcoderr/tests/worker_path_mappings_api.rs`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/worker-path-mappings" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Add the handler in `api/workers.rs`**

In `crates/transcoderr/src/api/workers.rs`, append (after `patch`):

```rust
#[derive(Debug, serde::Deserialize)]
pub struct SetPathMappingsReq {
    pub rules: Vec<crate::path_mapping::PathMapping>,
}

#[derive(Debug, Serialize)]
pub struct SetPathMappingsResp {
    pub id: i64,
    /// Echo of the canonical (trailing-slash-normalised) rules that
    /// were stored. Empty array if the operator cleared mappings.
    pub rules: Vec<crate::path_mapping::PathMapping>,
}

/// PUT /api/workers/:id/path-mappings — set or clear the per-worker
/// path-mapping rules. Empty `rules` array clears (column → NULL).
/// Refuses `kind='local'` rows with 400. Same auth as the rest of
/// `/api/workers` (lives in the protected Router branch).
pub async fn set_path_mappings(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<SetPathMappingsReq>,
) -> Result<Json<SetPathMappingsResp>, StatusCode> {
    // Reject any rule with empty from/to.
    for rule in &req.rules {
        if rule.from.trim().is_empty() || rule.to.trim().is_empty() {
            return Err(StatusCode::BAD_REQUEST);
        }
    }

    // Normalise (trailing slashes) by round-tripping through PathMappings.
    let mappings = crate::path_mapping::PathMappings::from_rules(req.rules);
    let canonical = mappings.rules().to_vec();

    // Empty rules → store NULL; non-empty → re-serialise the canonical
    // (trailing-slash-stripped) form.
    let json: Option<String> = if canonical.is_empty() {
        None
    } else {
        Some(
            serde_json::to_string(&canonical)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        )
    };

    let n = db::workers::update_path_mappings(&state.pool, id, json.as_deref())
        .await
        .map_err(|e| {
            tracing::error!(id, error = ?e, "failed to update path_mappings_json");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    if n == 0 {
        // Either the id is missing OR the row is kind='local'. Either way,
        // 400 is the right answer for the operator-facing error: the
        // request was rejected because the target worker can't accept
        // mappings.
        return Err(StatusCode::BAD_REQUEST);
    }

    // Refresh the Connections cache so a subsequent dispatch picks up
    // the new mappings without re-reading the DB.
    state
        .connections
        .set_path_mappings(id, mappings)
        .await;

    Ok(Json(SetPathMappingsResp { id, rules: canonical }))
}
```

- [ ] **Step 3: Wire the route in `api/mod.rs`**

In `crates/transcoderr/src/api/mod.rs`, the existing `protected` Router has:

```rust
        .route("/workers/:id",        patch(workers::patch).delete(workers::delete))
```

Add a new route line right after it:

```rust
        .route("/workers/:id/path-mappings", axum::routing::put(workers::set_path_mappings))
```

(The `put` import is already in scope via `use axum::routing::{delete, get, patch, post};`.)

Actually the existing import block in `api/mod.rs` does NOT include `put`. Either add `put` to the import list or use `axum::routing::put` qualified at the route line. **Read `api/mod.rs` first and choose whichever matches the existing style.** If extending the import, the line becomes:

```rust
use axum::routing::{delete, get, patch, post, put};
```

Then the route line is just `.route("/workers/:id/path-mappings", put(workers::set_path_mappings))`.

- [ ] **Step 4: Create the integration test file**

Create `crates/transcoderr/tests/worker_path_mappings_api.rs` with **3 tests** (the GET-backed round-trip and the empty-clear-via-GET tests are deferred to Task 5 because they assert against the new `path_mappings` field on the GET response):

```rust
//! Integration tests for the `PUT /api/workers/:id/path-mappings`
//! endpoint. The endpoint refuses kind='local' rows, validates
//! non-empty from/to, normalises trailing slashes, and persists via
//! `db::workers::update_path_mappings`. Two more tests that assert
//! against the GET response shape (`path_mappings` field) live in
//! Task 5 — they are appended to this same file.

mod common;

use common::boot;
use serde_json::json;

#[tokio::test]
async fn put_path_mappings_echoes_canonical_rules() {
    let app = boot().await;
    let client = reqwest::Client::new();

    let create_resp: serde_json::Value = client
        .post(format!("{}/api/workers", app.url))
        .json(&json!({"name": "gpu-1"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = create_resp["id"].as_i64().unwrap();

    let put_resp = client
        .put(format!("{}/api/workers/{}/path-mappings", app.url, id))
        .json(&json!({
            "rules": [
                {"from": "/mnt/movies/", "to": "/data/media/movies/"},
                {"from": "/mnt/tv",      "to": "/data/media/tv"}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(put_resp.status(), 200);
    let body: serde_json::Value = put_resp.json().await.unwrap();
    assert_eq!(body["id"].as_i64().unwrap(), id);
    let rules = body["rules"].as_array().unwrap();
    assert_eq!(rules.len(), 2);
    // Trailing slashes stripped on save (canonicalised before echo).
    assert!(rules.iter().any(|r| r["from"].as_str() == Some("/mnt/movies")));
    assert!(rules.iter().any(|r| r["from"].as_str() == Some("/mnt/tv")));
}

#[tokio::test]
async fn put_path_mappings_refuses_local_worker() {
    let app = boot().await;
    let client = reqwest::Client::new();

    // id=1 is the seeded local worker row.
    let resp = client
        .put(format!("{}/api/workers/1/path-mappings", app.url))
        .json(&json!({"rules": [{"from": "/a", "to": "/b"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "kind='local' must be rejected with 400");
}

#[tokio::test]
async fn put_path_mappings_rejects_empty_from_or_to() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let create_resp: serde_json::Value = client
        .post(format!("{}/api/workers", app.url))
        .json(&json!({"name": "gpu-1"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = create_resp["id"].as_i64().unwrap();

    for body in &[
        json!({"rules": [{"from": "",     "to": "/b"}]}),
        json!({"rules": [{"from": "/a",   "to": ""}]}),
        json!({"rules": [{"from": "   ",  "to": "/b"}]}),
    ] {
        let resp = client
            .put(format!("{}/api/workers/{}/path-mappings", app.url, id))
            .json(body)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 400, "rejected: {body}");
    }
}
```

- [ ] **Step 5: Build smoke**

```bash
cargo build -p transcoderr 2>&1 | tail -5
```

Expected: clean.

- [ ] **Step 6: Run the new tests**

```bash
cargo test -p transcoderr --test worker_path_mappings_api 2>&1 | tail -10
```

Expected: 3 passed (the three listed above).

- [ ] **Step 7: Regression net**

```bash
cargo test -p transcoderr --test api_auth 2>&1 | grep -E "FAILED|^test result" | tail -3
cargo test -p transcoderr --lib 2>&1 | grep -E "FAILED|^test result" | tail -3
```

Expected: no FAILED.

- [ ] **Step 8: Commit**

```bash
test "$(git branch --show-current)" = "feat/worker-path-mappings" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/api/workers.rs \
        crates/transcoderr/src/api/mod.rs \
        crates/transcoderr/tests/worker_path_mappings_api.rs
git commit -m "feat(api): PUT /api/workers/:id/path-mappings"
```

---

## Task 5: Include `path_mappings` in `GET /api/workers` response

Extend `WorkerSummary` with a `path_mappings: Option<Vec<PathMapping>>` field. Parsed from the row's `path_mappings_json`. NULL/empty → field renders as `null`. Adds the two deferred tests from Task 4.

**Files:**
- Modify: `crates/transcoderr/src/api/workers.rs`
- Modify: `crates/transcoderr/tests/worker_path_mappings_api.rs`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/worker-path-mappings" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Extend `WorkerSummary` and `row_to_summary`**

In `crates/transcoderr/src/api/workers.rs`, modify the struct:

```rust
#[derive(Debug, Serialize)]
pub struct WorkerSummary {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub secret_token: Option<String>,
    pub hw_caps: Option<serde_json::Value>,
    pub plugin_manifest: Option<serde_json::Value>,
    pub enabled: bool,
    pub last_seen_at: Option<i64>,
    pub created_at: i64,
    /// Per-worker path-mapping rules. `None` = no mapping (identity).
    pub path_mappings: Option<Vec<crate::path_mapping::PathMapping>>,
}
```

Update `row_to_summary` to populate the new field:

```rust
fn row_to_summary(row: db::workers::WorkerRow, redact: bool) -> WorkerSummary {
    let path_mappings: Option<Vec<crate::path_mapping::PathMapping>> = row
        .path_mappings_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());
    WorkerSummary {
        id: row.id,
        name: row.name,
        kind: row.kind,
        secret_token: row.secret_token.map(|t| if redact { "***".to_string() } else { t }),
        hw_caps: row.hw_caps_json.as_deref().and_then(|s| serde_json::from_str(s).ok()),
        plugin_manifest: row.plugin_manifest_json.as_deref().and_then(|s| serde_json::from_str(s).ok()),
        enabled: row.enabled != 0,
        last_seen_at: row.last_seen_at,
        created_at: row.created_at,
        path_mappings,
    }
}
```

- [ ] **Step 3: Add the two deferred tests**

Append to `crates/transcoderr/tests/worker_path_mappings_api.rs`:

```rust
#[tokio::test]
async fn put_round_trips_via_get() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let create_resp: serde_json::Value = client
        .post(format!("{}/api/workers", app.url))
        .json(&json!({"name": "gpu-1"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = create_resp["id"].as_i64().unwrap();

    client
        .put(format!("{}/api/workers/{}/path-mappings", app.url, id))
        .json(&json!({
            "rules": [{"from": "/mnt/movies", "to": "/data/media/movies"}]
        }))
        .send()
        .await
        .unwrap();

    let workers: serde_json::Value = client
        .get(format!("{}/api/workers", app.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let row = workers
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["id"].as_i64() == Some(id))
        .expect("worker row");
    let rules = row["path_mappings"].as_array().expect("path_mappings present");
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0]["from"].as_str().unwrap(), "/mnt/movies");
    assert_eq!(rules[0]["to"].as_str().unwrap(), "/data/media/movies");
}

#[tokio::test]
async fn put_empty_rules_clears_to_null_in_get() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let create_resp: serde_json::Value = client
        .post(format!("{}/api/workers", app.url))
        .json(&json!({"name": "gpu-1"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = create_resp["id"].as_i64().unwrap();

    client
        .put(format!("{}/api/workers/{}/path-mappings", app.url, id))
        .json(&json!({"rules": [{"from": "/a", "to": "/b"}]}))
        .send()
        .await
        .unwrap();
    let resp = client
        .put(format!("{}/api/workers/{}/path-mappings", app.url, id))
        .json(&json!({"rules": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let workers: serde_json::Value = client
        .get(format!("{}/api/workers", app.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let row = workers
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["id"].as_i64() == Some(id))
        .expect("worker row");
    assert!(row["path_mappings"].is_null(),
        "empty rules → DB NULL → JSON null");
}
```

- [ ] **Step 4: Build smoke**

```bash
cargo build -p transcoderr 2>&1 | tail -5
```

Expected: clean.

- [ ] **Step 5: Run the tests**

```bash
cargo test -p transcoderr --test worker_path_mappings_api 2>&1 | tail -10
```

Expected: 5 passed (3 from Task 4 + 2 new).

- [ ] **Step 6: Regression net**

```bash
cargo test -p transcoderr --test api_auth --test worker_connect --test remote_dispatch 2>&1 | grep -E "FAILED|^test result" | tail -10
cargo test -p transcoderr --lib 2>&1 | grep -E "FAILED|^test result" | tail -3
```

Expected: no FAILED.

- [ ] **Step 7: Commit**

```bash
test "$(git branch --show-current)" = "feat/worker-path-mappings" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/api/workers.rs \
        crates/transcoderr/tests/worker_path_mappings_api.rs
git commit -m "feat(api): WorkerSummary.path_mappings field in GET /api/workers"
```

---

## Task 6: Wire path mapping into `RemoteRunner::run`

**Critical-path change.** Every remote dispatch flows through this code; a regression breaks all remote work. Adds two hooks: rewrite `ctx_snapshot` outbound; reverse-rewrite the worker's returned `ctx_snapshot` on `StepComplete`. Mappings are loaded from the cache; if absent, lazily loaded from the DB and stored in the cache.

**Files:**
- Modify: `crates/transcoderr/src/dispatch/remote.rs`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/worker-path-mappings" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Extend `RemoteRunner::run`**

Open `crates/transcoderr/src/dispatch/remote.rs`. Modify `RemoteRunner::run` to load mappings, rewrite the dispatch snapshot, and reverse-rewrite the completion snapshot. The full new body:

```rust
    pub async fn run(
        state: &AppState,
        worker_id: i64,
        job_id: i64,
        step_id: &str,
        use_: &str,
        with: &BTreeMap<String, serde_json::Value>,
        ctx: &mut Context,
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()> {
        let correlation_id = format!("dsp-{}", uuid::Uuid::new_v4());

        // 0. Load (or lazily fill) the per-worker path mappings cache.
        //    Snapshot once for the duration of this step so a mid-flight
        //    edit by the operator can't desync the round-trip.
        let mappings = load_or_fill_mappings(state, worker_id).await;

        // 1. Register an inbox for inbound frames keyed by correlation_id.
        let (mut rx, _inbox_guard) = state
            .connections
            .register_inbox(correlation_id.clone())
            .await;

        // 2. Build the context snapshot, rewriting paths on the way out.
        let ctx_snapshot = if mappings.is_empty() {
            ctx.to_snapshot()
        } else {
            let mut value: serde_json::Value =
                serde_json::from_str(&ctx.to_snapshot())?;
            mappings.apply(&mut value, crate::path_mapping::Direction::CoordToWorker);
            serde_json::to_string(&value)?
        };

        let with_json: serde_json::Value = serde_json::to_value(with)?;
        let dispatch_env = Envelope {
            id: correlation_id.clone(),
            message: Message::StepDispatch(StepDispatch {
                job_id,
                step_id: step_id.into(),
                use_: use_.into(),
                with: with_json,
                ctx_snapshot,
            }),
        };
        state
            .connections
            .send_to_worker(worker_id, dispatch_env)
            .await
            .map_err(|e| anyhow::anyhow!("dispatch send failed: {e}"))?;

        // 3. Pump inbound frames until completion, timeout, or cancel.
        let cancel = ctx.cancel.clone();
        loop {
            let frame = tokio::select! {
                f = tokio::time::timeout(STEP_FRAME_TIMEOUT, rx.recv()) => match f {
                    Ok(Some(f)) => f,
                    Ok(None) => anyhow::bail!("worker inbox channel closed"),
                    Err(_) => anyhow::bail!("worker step timed out"),
                },
                _ = async {
                    match &cancel {
                        Some(c) => c.cancelled().await,
                        None => std::future::pending::<()>().await,
                    }
                } => {
                    tracing::info!(
                        job_id,
                        step_id,
                        worker_id,
                        correlation_id = %correlation_id,
                        "cancelling in-flight remote step; sending StepCancel to worker"
                    );
                    let cancel_env = Envelope {
                        id: correlation_id.clone(),
                        message: Message::StepCancel(StepCancelMsg {
                            job_id,
                            step_id: step_id.into(),
                        }),
                    };
                    let _ = state
                        .connections
                        .send_to_worker(worker_id, cancel_env)
                        .await;
                    anyhow::bail!("step cancelled by operator");
                }
            };

            match frame {
                InboundStepEvent::Progress(p) => {
                    let progress = match p.kind.as_str() {
                        "progress" => {
                            let pct = p.payload.get("pct").and_then(|v| v.as_f64()).unwrap_or(0.0);
                            StepProgress::Pct(pct)
                        }
                        "log" => {
                            let msg = p
                                .payload
                                .get("msg")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            StepProgress::Log(msg)
                        }
                        other => StepProgress::Marker {
                            kind: other.to_string(),
                            payload: p.payload,
                        },
                    };
                    on_progress(progress);
                }
                InboundStepEvent::Complete(c) => {
                    if c.status == "ok" {
                        if let Some(snap) = c.ctx_snapshot {
                            // Reverse-rewrite paths on the way back so
                            // the next step on the coordinator sees
                            // coordinator-space paths.
                            let restored = if mappings.is_empty() {
                                snap
                            } else {
                                let mut value: serde_json::Value =
                                    serde_json::from_str(&snap)?;
                                mappings.apply(
                                    &mut value,
                                    crate::path_mapping::Direction::WorkerToCoord,
                                );
                                serde_json::to_string(&value)?
                            };
                            let cancel = ctx.cancel.clone();
                            *ctx = Context::from_snapshot(&restored)?;
                            ctx.cancel = cancel;
                        }
                        return Ok(());
                    }
                    anyhow::bail!(
                        "remote step failed: {}",
                        c.error.unwrap_or_else(|| "unknown error".into())
                    );
                }
            }
        }
    }
}

/// Look up the cached `PathMappings` for `worker_id`. If there's no
/// entry, load from `workers.path_mappings_json`, populate the cache,
/// and return the loaded value. Errors are non-fatal: a parse failure
/// is logged and the dispatch falls back to identity translation.
async fn load_or_fill_mappings(
    state: &AppState,
    worker_id: i64,
) -> crate::path_mapping::PathMappings {
    if let Some(cached) = state.connections.path_mappings_for(worker_id).await {
        return cached;
    }
    let row = match crate::db::workers::get_by_id(&state.pool, worker_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return crate::path_mapping::PathMappings::default(),
        Err(e) => {
            tracing::warn!(worker_id, error = ?e, "load_or_fill_mappings: db read failed; falling back to identity");
            return crate::path_mapping::PathMappings::default();
        }
    };
    let mappings = match row.path_mappings_json.as_deref() {
        Some(s) => match crate::path_mapping::PathMappings::from_json(s) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(
                    worker_id,
                    error = ?e,
                    "path_mappings_json failed to parse; falling back to identity"
                );
                crate::path_mapping::PathMappings::default()
            }
        },
        None => crate::path_mapping::PathMappings::default(),
    };
    state
        .connections
        .set_path_mappings(worker_id, mappings.clone())
        .await;
    mappings
}
```

- [ ] **Step 3: Build smoke**

```bash
cargo build -p transcoderr 2>&1 | tail -10
```

Expected: clean.

- [ ] **Step 4: Critical-path tests must stay green**

```bash
cargo test -p transcoderr --test concurrent_claim --test crash_recovery --test flow_engine 2>&1 | grep -E "FAILED|^test result" | tail -10
cargo test -p transcoderr --test remote_dispatch --test plugin_remote_dispatch --test cancel_remote 2>&1 | grep -E "FAILED|^test result" | tail -10
```

Expected: every line `test result: ok.`. NO FAILED. These tests don't configure path mappings (they use NULL = identity), so the new code paths are short-circuited via `mappings.is_empty()` and the round-trip is byte-for-byte identical to the previous behaviour.

- [ ] **Step 5: Lib + remaining integration tests**

```bash
cargo test -p transcoderr --lib 2>&1 | grep -E "FAILED|^test result" | tail -3
cargo test -p transcoderr --test worker_connect --test local_worker --test plugin_push --test api_auth --test auto_discovery --test worker_enroll --test worker_path_mappings_api 2>&1 | grep -E "FAILED|^test result" | tail -10
```

Expected: no FAILED.

- [ ] **Step 6: Commit**

```bash
test "$(git branch --show-current)" = "feat/worker-path-mappings" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/dispatch/remote.rs
git commit -m "feat(dispatch): apply per-worker path mappings on dispatch + completion"
```

**[PAUSE FOR USER CONFIRMATION HERE]**

Reason: this commit changes the critical-path remote dispatch code. The mid-step `mappings.is_empty()` short-circuit makes existing behaviour identical for any worker without configured mappings, and the regression net above (concurrent_claim, crash_recovery, flow_engine, remote_dispatch, plugin_remote_dispatch, cancel_remote, worker_connect, local_worker, plugin_push, api_auth, auto_discovery, worker_enroll, worker_path_mappings_api, lib) covers all the existing remote paths. The user should review the diff before continuing.

---

## Task 7: End-to-end integration test `tests/path_mapping.rs`

The single integration test that proves the full forward + reverse rewrite over the wire. Boots a fake remote worker, configures `path_mappings_json = [{"from":"/coord", "to":"/worker"}]` directly via `db::workers::update_path_mappings`, dispatches a step with a `/coord/...` path, asserts the worker sees `/worker/...`, has the worker reply with worker-space paths, and asserts the coordinator's restored ctx has coordinator-space paths.

**Files:**
- Create: `crates/transcoderr/tests/path_mapping.rs`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/worker-path-mappings" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Read the existing fake-worker harness for patterns**

```bash
head -120 crates/transcoderr/tests/remote_dispatch.rs
```

Note the canonical helpers and test fixture pattern: `mint_token`, `ws_connect`, `send_env`, `recv_env`, `send_register_and_get_ack`, `submit_job_with_step`, `wait_for_step_dispatch`. Reuse the same shape.

- [ ] **Step 3: Create the test file**

Create `crates/transcoderr/tests/path_mapping.rs`:

```rust
//! End-to-end: per-worker path mappings rewrite paths on the wire in
//! both directions. Boot a coordinator, register a fake worker,
//! configure path_mappings_json for that worker, dispatch a step with
//! `/coord/...` path. Assert the worker sees `/worker/...`. Worker
//! replies with worker-space paths in ctx.steps.tx.output_path; assert
//! the coordinator's restored ctx has coordinator-space paths.

mod common;

use common::boot;
use futures::{SinkExt, StreamExt};
use serde_json::json;
use std::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use transcoderr::worker::protocol::{
    Envelope, Message, PluginManifestEntry, Register, StepComplete,
};

type Ws = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

async fn mint_token(client: &reqwest::Client, base: &str, name: &str) -> (i64, String) {
    let resp: serde_json::Value = client
        .post(format!("{base}/api/workers"))
        .json(&json!({"name": name}))
        .send().await.unwrap()
        .json().await.unwrap();
    (
        resp["id"].as_i64().unwrap(),
        resp["secret_token"].as_str().unwrap().to_string(),
    )
}

async fn ws_connect(base_ws: &str, token: &str) -> Ws {
    let mut req = format!("{base_ws}/api/worker/connect")
        .as_str()
        .into_client_request()
        .unwrap();
    req.headers_mut().insert(
        AUTHORIZATION,
        format!("Bearer {token}").parse().unwrap(),
    );
    let (ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    ws
}

async fn send_env(ws: &mut Ws, env: &Envelope) {
    let s = serde_json::to_string(env).unwrap();
    ws.send(WsMessage::Text(s)).await.unwrap();
}

async fn recv_env(ws: &mut Ws) -> Envelope {
    let raw = ws.next().await.unwrap().unwrap();
    match raw {
        WsMessage::Text(s) => serde_json::from_str(&s).unwrap(),
        other => panic!("expected text, got {other:?}"),
    }
}

async fn send_register_and_drain_ack(ws: &mut Ws, name: &str) {
    let reg = Envelope {
        id: "reg-1".into(),
        message: Message::Register(Register {
            name: name.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            hw_caps: json!({"encoders": []}),
            available_steps: vec!["transcode".into()],
            plugin_manifest: vec![PluginManifestEntry {
                name: "size-report".into(),
                version: "0.1.2".into(),
                sha256: None,
            }],
        }),
    };
    send_env(ws, &reg).await;
    let _ack = recv_env(ws).await;
}

/// Insert a flow + pending job pointing at the given step kind. Returns
/// (flow_id, job_id). Mirrors the pattern in `tests/remote_dispatch.rs`.
async fn submit_job_with_step(
    app: &common::TestApp,
    use_: &str,
    file_path: &str,
) -> (i64, i64) {
    let yaml = format!(
        "name: t\ntriggers: [{{ webhook: x }}]\nsteps:\n  - use: {use_}\n    run_on: any\n"
    );
    let value: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
    let parsed_json = serde_json::to_string(&value).unwrap();

    sqlx::query("INSERT INTO flows (name, yaml_source, parsed_json, enabled, created_at) VALUES (?, ?, ?, 1, strftime('%s','now'))")
        .bind("t").bind(&yaml).bind(&parsed_json)
        .execute(&app.pool).await.unwrap();
    let flow_id: i64 = sqlx::query_scalar("SELECT id FROM flows ORDER BY id DESC LIMIT 1")
        .fetch_one(&app.pool).await.unwrap();
    sqlx::query("INSERT INTO jobs (flow_id, file_path, status, created_at) VALUES (?, ?, 'pending', strftime('%s','now'))")
        .bind(flow_id).bind(file_path)
        .execute(&app.pool).await.unwrap();
    let job_id: i64 = sqlx::query_scalar("SELECT id FROM jobs ORDER BY id DESC LIMIT 1")
        .fetch_one(&app.pool).await.unwrap();
    (flow_id, job_id)
}

async fn wait_for_step_dispatch(ws: &mut Ws, deadline: Duration) -> Option<Envelope> {
    let res = tokio::time::timeout(deadline, async {
        loop {
            let env = recv_env(ws).await;
            if matches!(env.message, Message::StepDispatch(_)) {
                return env;
            }
        }
    })
    .await;
    res.ok()
}

#[tokio::test]
async fn round_trip_rewrites_paths_in_both_directions() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (worker_id, token) = mint_token(&client, &app.url, "fake_pm").await;

    // Configure path mappings for this worker BEFORE the WS connect, so
    // the cache fill on first dispatch picks them up.
    transcoderr::db::workers::update_path_mappings(
        &app.pool,
        worker_id,
        Some(r#"[{"from":"/coord","to":"/worker"}]"#),
    )
    .await
    .unwrap();

    let base_ws = app.url.replace("http://", "ws://");
    let mut ws = ws_connect(&base_ws, &token).await;
    send_register_and_drain_ack(&mut ws, "fake_pm").await;

    // Submit a job with a /coord/... path; the engine will dispatch
    // a transcode step to the only eligible remote worker (us).
    let (_flow_id, job_id) =
        submit_job_with_step(&app, "transcode", "/coord/movies/X.mkv").await;

    // 1. Forward rewrite: assert the worker sees the /worker/ path.
    let dispatch = wait_for_step_dispatch(&mut ws, Duration::from_secs(10))
        .await
        .expect("worker should receive step_dispatch within 10s");
    let dispatched_ctx = match &dispatch.message {
        Message::StepDispatch(d) => d.ctx_snapshot.clone(),
        _ => unreachable!(),
    };
    let dispatched_value: serde_json::Value =
        serde_json::from_str(&dispatched_ctx).unwrap();
    assert_eq!(
        dispatched_value["file"]["path"].as_str().unwrap(),
        "/worker/movies/X.mkv",
        "forward rewrite must map /coord -> /worker"
    );

    // 2. Worker replies with a NEW path inside ctx.steps.tx.output_path
    //    in worker-space.
    let mut returned_ctx = dispatched_value.clone();
    returned_ctx["steps"] = json!({
        "transcode": {
            "output_path": "/worker/movies/X.transcoded.mkv"
        }
    });

    let correlation_id = dispatch.id.clone();
    let step_id_str = match dispatch.message {
        Message::StepDispatch(d) => d.step_id,
        _ => unreachable!(),
    };
    let complete = Envelope {
        id: correlation_id,
        message: Message::StepComplete(StepComplete {
            job_id,
            step_id: step_id_str,
            status: "ok".into(),
            error: None,
            ctx_snapshot: Some(returned_ctx.to_string()),
        }),
    };
    send_env(&mut ws, &complete).await;

    // 3. Reverse rewrite: assert the coordinator's stored ctx has
    //    coordinator-space paths after the run finishes.
    //    The simplest way to check: poll the runs table for the
    //    completed run's ctx snapshot. The runs table stores
    //    `ctx_snapshot_json` after each step.
    let start = std::time::Instant::now();
    let target = "completed";
    let final_status: Option<String> = loop {
        let status: Option<String> = sqlx::query_scalar(
            "SELECT status FROM jobs WHERE id = ?",
        )
        .bind(job_id)
        .fetch_optional(&app.pool)
        .await
        .unwrap();
        if let Some(s) = &status {
            if s == target {
                break Some(s.clone());
            }
        }
        if start.elapsed() >= Duration::from_secs(5) {
            break status;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(
        final_status.as_deref(),
        Some("completed"),
        "job should complete after StepComplete",
    );

    // Read the final ctx_snapshot from the runs table.
    let snapshot: Option<String> = sqlx::query_scalar(
        "SELECT ctx_snapshot_json FROM runs WHERE job_id = ? ORDER BY id DESC LIMIT 1",
    )
    .bind(job_id)
    .fetch_optional(&app.pool)
    .await
    .unwrap();
    let snapshot = snapshot.expect("run row exists");
    let restored: serde_json::Value = serde_json::from_str(&snapshot).unwrap();
    assert_eq!(
        restored["file"]["path"].as_str().unwrap(),
        "/coord/movies/X.mkv",
        "reverse rewrite must map /worker -> /coord on file.path"
    );
    assert_eq!(
        restored["steps"]["transcode"]["output_path"].as_str().unwrap(),
        "/coord/movies/X.transcoded.mkv",
        "reverse rewrite must map /worker -> /coord inside ctx.steps too"
    );
}
```

- [ ] **Step 4: Run the new test**

```bash
cargo test -p transcoderr --test path_mapping 2>&1 | tail -10
```

Expected: 1 passed.

If this fails:
- The most common cause is that the worker's `available_steps` cache wasn't populated yet when the engine tried to route the step. Confirm `record_available_steps` is called during the register handshake (it should be — Piece 5 wired it).
- Second-most-common: `submit_job_with_step` doesn't produce a parseable flow. Compare with the working pattern in `tests/cancel_remote.rs::submit_job_with_step` and adjust if the schema diverged.
- Third: the runs table doesn't store `ctx_snapshot_json` under that name. Read `crates/transcoderr/src/db/runs.rs` and adjust the query column name if needed.

If you find the runs table column has a different name, fix the assertion query rather than naming the runs table — the column name is the only ambiguity.

- [ ] **Step 5: Run the full integration suite for confidence**

```bash
cargo test -p transcoderr 2>&1 | grep -E "FAILED|^test result" | tail -25
```

Expected: every line `test result: ok.`. No FAILED.

- [ ] **Step 6: Commit**

```bash
test "$(git branch --show-current)" = "feat/worker-path-mappings" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/tests/path_mapping.rs
git commit -m "test(path_mapping): end-to-end forward + reverse rewrite suite"
```

---

## Task 8: Web UI — `path-mappings-modal` + workers-page button + api client method

Operator-facing UI. New "Edit mappings" button per `kind='remote'` worker; opens a modal with a list of `{from, to}` rows + add/remove controls; Save PUTs to `/api/workers/:id/path-mappings`. Reuses the existing `.modal-*` CSS classes from `web/src/index.css`.

**Files:**
- Modify: `web/src/api/client.ts`
- Create: `web/src/components/path-mappings-modal.tsx`
- Modify: `web/src/pages/workers.tsx`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/worker-path-mappings" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Add the API client method**

Open `web/src/api/client.ts`. Find the `workers` namespace (look for `workers: {`). Add `updatePathMappings`:

```ts
  workers: {
    // ... existing list / create / patch / delete methods ...

    async updatePathMappings(
      id: number,
      rules: Array<{ from: string; to: string }>,
    ): Promise<{ id: number; rules: Array<{ from: string; to: string }> }> {
      const resp = await fetch(`/api/workers/${id}/path-mappings`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ rules }),
      });
      if (!resp.ok) {
        const text = await resp.text().catch(() => "");
        throw new Error(`PUT /api/workers/${id}/path-mappings: ${resp.status} ${text}`);
      }
      return resp.json();
    },
  },
```

Also extend the `Worker` type returned by `list()` (or wherever it's defined in this file) to include the new field:

```ts
export type Worker = {
  // ... existing fields ...
  path_mappings: Array<{ from: string; to: string }> | null;
};
```

- [ ] **Step 3: Create the modal component**

Create `web/src/components/path-mappings-modal.tsx`:

```tsx
import { useState } from "react";
import { api } from "../api/client";

type Rule = { from: string; to: string };

type Props = {
  workerId: number;
  workerName: string;
  initialRules: Rule[];
  onClose: () => void;
  onSaved: () => void;
};

export function PathMappingsModal({
  workerId,
  workerName,
  initialRules,
  onClose,
  onSaved,
}: Props) {
  const [rules, setRules] = useState<Rule[]>(
    initialRules.length > 0 ? initialRules : [],
  );
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  function setRule(i: number, patch: Partial<Rule>) {
    setRules((rs) => rs.map((r, idx) => (idx === i ? { ...r, ...patch } : r)));
  }
  function addRule() {
    setRules((rs) => [...rs, { from: "", to: "" }]);
  }
  function removeRule(i: number) {
    setRules((rs) => rs.filter((_, idx) => idx !== i));
  }
  async function save() {
    setError(null);
    // Drop entirely-empty rows so the operator can leave a stub at the
    // bottom without it triggering a 400.
    const cleaned = rules.filter(
      (r) => r.from.trim() !== "" || r.to.trim() !== "",
    );
    // Both fields required if either is present.
    for (const r of cleaned) {
      if (r.from.trim() === "" || r.to.trim() === "") {
        setError("Each rule needs both From and To.");
        return;
      }
    }
    setSaving(true);
    try {
      await api.workers.updatePathMappings(workerId, cleaned);
      onSaved();
      onClose();
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
      setSaving(false);
    }
  }

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal-header">
          <h3>Path mappings — {workerName}</h3>
        </div>
        <div className="modal-body">
          <p style={{ marginTop: 0, fontSize: "0.9em", opacity: 0.8 }}>
            Rewrite filesystem paths between coordinator and worker. Use this
            when the worker mounts the same media at a different absolute path.
            Longest matching prefix wins.
          </p>
          {rules.length === 0 ? (
            <p>
              <em>No mappings — paths pass through unchanged.</em>
            </p>
          ) : (
            <table style={{ width: "100%" }}>
              <thead>
                <tr>
                  <th align="left">From (coordinator)</th>
                  <th align="left">To (worker)</th>
                  <th></th>
                </tr>
              </thead>
              <tbody>
                {rules.map((r, i) => (
                  <tr key={i}>
                    <td>
                      <input
                        type="text"
                        value={r.from}
                        placeholder="/mnt/movies"
                        onChange={(e) => setRule(i, { from: e.target.value })}
                        style={{ width: "100%" }}
                      />
                    </td>
                    <td>
                      <input
                        type="text"
                        value={r.to}
                        placeholder="/data/media/movies"
                        onChange={(e) => setRule(i, { to: e.target.value })}
                        style={{ width: "100%" }}
                      />
                    </td>
                    <td>
                      <button onClick={() => removeRule(i)}>✕</button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          <button onClick={addRule} style={{ marginTop: "0.5rem" }}>
            + Add mapping
          </button>
          {error && (
            <p style={{ color: "var(--color-error, red)" }}>{error}</p>
          )}
        </div>
        <div className="modal-footer">
          <button onClick={onClose} disabled={saving}>
            Cancel
          </button>
          <button onClick={save} disabled={saving}>
            {saving ? "Saving…" : "Save"}
          </button>
        </div>
      </div>
    </div>
  );
}
```

- [ ] **Step 4: Wire the button into `workers.tsx`**

Open `web/src/pages/workers.tsx`. Read the existing per-worker row rendering. Add:

1. An import for the modal:
   ```ts
   import { PathMappingsModal } from "../components/path-mappings-modal";
   ```
2. State for which worker (if any) is currently being edited:
   ```ts
   const [mappingFor, setMappingFor] = useState<{ id: number; name: string; rules: Array<{from: string; to: string}> } | null>(null);
   ```
3. A new "Edit mappings" button in the per-worker row, gated on `kind === "remote"` (similar to how the existing Delete button is gated). Clicking sets `mappingFor`:
   ```tsx
   {worker.kind === "remote" && (
     <button
       onClick={() => setMappingFor({
         id: worker.id,
         name: worker.name,
         rules: worker.path_mappings ?? [],
       })}
       title="Edit path mappings"
     >
       Edit mappings
     </button>
   )}
   ```
4. The modal at the bottom of the page render, conditional on `mappingFor`:
   ```tsx
   {mappingFor && (
     <PathMappingsModal
       workerId={mappingFor.id}
       workerName={mappingFor.name}
       initialRules={mappingFor.rules}
       onClose={() => setMappingFor(null)}
       onSaved={() => {
         // Re-fetch the workers list so the new path_mappings show up.
         refetch();
       }}
     />
   )}
   ```

(`refetch` is whatever the existing page uses to reload — copy from how the existing add-worker / delete flows trigger a refresh. If the page uses a `useQuery` from a tanstack-style hook, `refetch` is its returned function. Read the file to find the right name.)

- [ ] **Step 5: Run the web build**

```bash
npm --prefix web ci 2>&1 | tail -3
npm --prefix web run build 2>&1 | tail -5
```

Expected: build clean.

- [ ] **Step 6: Commit**

```bash
test "$(git branch --show-current)" = "feat/worker-path-mappings" || { echo "WRONG BRANCH"; exit 1; }
git add web/src/api/client.ts \
        web/src/components/path-mappings-modal.tsx \
        web/src/pages/workers.tsx
git commit -m "feat(web): path mappings modal on the workers page"
```

---

## Self-Review Notes

Spec coverage (every section maps to a task):

| Spec section | Task |
|---|---|
| ALTER TABLE workers ADD COLUMN path_mappings_json | Task 1 |
| `path_mapping::PathMapping` / `PathMappings` / `Direction` / `apply` | Task 2 |
| Boundary rule (`/mnt/movies` not matching `/mnt/movies-archive`) | Task 2 (`path_component_boundary_respected`) |
| Longest-prefix-match | Task 2 (`longest_prefix_wins`) |
| Trailing-slash normalisation | Task 2 (`trailing_slash_normalisation`) |
| Reverse round-trip | Task 2 (`reverse_round_trip`) |
| Walks nested objects + arrays | Task 2 (`walks_nested_objects_and_arrays`) |
| Object keys + non-string leaves untouched | Task 2 (`non_string_leaves_untouched`) |
| Empty mappings = identity | Task 2 (`empty_mappings_is_identity`) |
| `db::workers::update_path_mappings` (refuses kind='local') | Task 1 |
| `Connections::path_mappings_for` / `set_path_mappings` | Task 3 |
| Cache cleared on disconnect | Task 3 (`sender_guard_drop_clears_path_mappings_too`) |
| `PUT /api/workers/:id/path-mappings` (auth, validation, kind='local' 400, empty=NULL) | Task 4 + Task 5 (5 API tests total) |
| `GET /api/workers` returns `path_mappings` field | Task 5 |
| `RemoteRunner` snapshots mappings at dispatch + reverse on completion | Task 6 |
| Mid-flight edits don't desync | Task 6 (mappings captured before `send_to_worker`, used in both halves) |
| End-to-end integration | Task 7 |
| Web UI: button + modal + api method | Task 8 |
| Setup wizard / auto-discovery unaffected | All tasks (no touch points) |

Cross-task type/signature consistency:

- `PathMapping { from: String, to: String }` — Task 2 defines, Tasks 4/5/6 consume via `crate::path_mapping::PathMapping`.
- `PathMappings::default()` = identity — Task 2 contract, Task 6 uses for fall-back, Task 3 cache stores `Some(default())` distinct from `None`.
- `Direction::CoordToWorker` / `Direction::WorkerToCoord` — Task 2 defines, Task 6 uses both.
- `connections.set_path_mappings(worker_id, PathMappings)` — Task 3 declares, Tasks 4 + 6 call.
- `connections.path_mappings_for(worker_id) -> Option<PathMappings>` — Task 3 declares, Task 6 calls.
- `db::workers::update_path_mappings(pool, id, json: Option<&str>) -> Result<u64>` — Task 1 declares, Tasks 4 + 7 call.

No placeholders. Every step has executable code or exact commands. All file paths absolute. Bite-sized step granularity. Frequent commits — 8 total commits, one per task. Single-PR feature.

Pause checkpoint at Task 6 (RemoteRunner — critical-path) per the brainstorm guidance.
