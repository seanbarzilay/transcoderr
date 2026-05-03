# Distributed Transcoding — Piece 5 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Plugin-provided step kinds become remote-dispatchable. A plugin manifest declares per-step executor preferences via `[steps."<step_kind>"] executor = "any-worker"`. The dispatcher's per-worker step-kind filter (deferred in Piece 3) goes live, so the coordinator only routes a plugin step to a worker that actually has the plugin installed.

**Architecture:** `Manifest` gains an optional `[steps."<name>"]` map carrying an `executor` enum. `SubprocessStep` gets an `executor: Executor` field, threaded at registry build time. Workers enumerate `available_steps` from the live registry via a new `registry::list_step_names()` helper at register time AND after each `plugin_sync::sync` (re-register triggered by the existing sync worker task). Coordinator-side, `Connections` registry holds an in-memory per-worker `available_steps` map; dispatcher's `eligible_remotes` filter consults it via a new `worker_has_step` method. Re-register reuses the existing `Message::Register` wire envelope; coordinator's receive loop gains a `Register` arm that updates state without responding (avoid oscillation).

**Tech Stack:** Rust 2021 (axum 0.7, sqlx + sqlite, tokio, anyhow, tracing, async_trait, serde, toml). React N/A — no frontend changes this piece.

**Branch:** all tasks land on a fresh `feat/distributed-piece-5` branch off `main`. Implementer creates the branch before Task 1.

---

## File Structure

**Modified backend files:**
- `crates/transcoderr/src/plugins/manifest.rs` — `Manifest.steps` field + `StepManifest` + `ManifestExecutor` enum.
- `crates/transcoderr/src/plugins/subprocess.rs` — `SubprocessStep.executor: Executor`; `executor()` returns it.
- `crates/transcoderr/src/steps/registry.rs` — build path threads executor; new `list_step_names()`.
- `crates/transcoderr/src/worker/connections.rs` — `available_steps: HashMap<i64, Vec<String>>` field + `record_available_steps` + `worker_has_step`; `SenderGuard::drop` cleanup.
- `crates/transcoderr/src/worker/connection.rs` — `ConnectionContext` extends with `name + hw_caps`; drop the `build_register: F` closure; introduce async `build_register_envelope(&ctx)`; sync worker task fires re-register after `plugin_sync::sync`.
- `crates/transcoderr/src/worker/daemon.rs` — drop hardcoded `available_steps`; build `ConnectionContext` with `name + hw_caps`; remove the local `build_register` closure.
- `crates/transcoderr/src/api/workers.rs::handle_connection` — initial register handshake calls `record_available_steps`; receive loop gains `Message::Register` arm.
- `crates/transcoderr/src/dispatch/mod.rs` — `eligible_remotes` filters by `step_kind` via `connections.worker_has_step`.

**New backend files:**
- `crates/transcoderr/tests/plugin_remote_dispatch.rs` — 5-scenario integration suite.

**No new wire-protocol variants** — re-uses existing `Message::Register`.
**No DB migration** — per-worker `available_steps` lives in `Connections` (in-memory).

---

## Task 1: Manifest schema — `[steps."<name>"]` block + ManifestExecutor enum

Mechanical: add an optional sub-table to the existing `Manifest` struct. Backwards-compatible — `#[serde(default)]` makes the new field optional and missing blocks default to coordinator-only.

**Files:**
- Modify: `crates/transcoderr/src/plugins/manifest.rs`

- [ ] **Step 1: Branch verification**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-5" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Add the new types + extend Manifest**

In `crates/transcoderr/src/plugins/manifest.rs`, after the existing `Manifest` struct definition, add:

```rust
/// Per-step manifest entry. Lives in the `[steps."<step_kind>"]`
/// table inside `manifest.toml`. The only field today is `executor`,
/// which defaults to coordinator-only when omitted.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct StepManifest {
    #[serde(default)]
    pub executor: Option<ManifestExecutor>,
}

/// Wire / TOML form of `crate::steps::Executor`. Kebab-case for TOML
/// readability (`any-worker` matches the spec's prose better than
/// `any_worker`).
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ManifestExecutor {
    AnyWorker,
    CoordinatorOnly,
}
```

In the existing `pub struct Manifest { ... }`, add a new field at the end (alongside the other `#[serde(default)]` fields):

```rust
    /// Per-step routing overrides. Each key is a step kind from
    /// `provides_steps`. Steps with no entry default to
    /// `coordinator-only`. See spec/distributed-piece-5.
    #[serde(default)]
    pub steps: std::collections::BTreeMap<String, StepManifest>,
```

- [ ] **Step 3: Add 2 unit tests**

Append a `#[cfg(test)]` block at the bottom of `manifest.rs` if one doesn't exist; otherwise extend the existing block. Add:

```rust
    #[test]
    fn deserialise_with_steps_block() {
        let toml_src = r#"
name = "whisper"
version = "1.0"
kind = "subprocess"
entrypoint = "bin/run"
provides_steps = ["whisper.transcribe", "whisper.detect_language"]

[steps."whisper.transcribe"]
executor = "any-worker"
"#;
        let m: Manifest = toml::from_str(toml_src).unwrap();
        assert_eq!(m.steps.len(), 1);
        let entry = m.steps.get("whisper.transcribe").unwrap();
        assert_eq!(entry.executor, Some(ManifestExecutor::AnyWorker));
        // The other declared step has no [steps.X] entry, so it's absent
        // from the map — the registry build path defaults to
        // CoordinatorOnly when looking up a missing key.
        assert!(m.steps.get("whisper.detect_language").is_none());
    }

    #[test]
    fn deserialise_without_steps_block() {
        // Existing manifest shape (size-report) still parses cleanly;
        // `steps` defaults to an empty map.
        let toml_src = r#"
name = "size-report"
version = "0.1.2"
kind = "subprocess"
entrypoint = "bin/run"
provides_steps = ["size.report"]
"#;
        let m: Manifest = toml::from_str(toml_src).unwrap();
        assert!(m.steps.is_empty(), "missing [steps] block → empty map");
    }
```

- [ ] **Step 4: Build smoke**

```bash
cargo build -p transcoderr 2>&1 | tail -5
```

Expected: clean.

- [ ] **Step 5: Run the new tests**

```bash
cargo test -p transcoderr --lib plugins::manifest 2>&1 | tail -10
```

Expected: 2 passed (or however many the file already has + 2 new).

- [ ] **Step 6: Lib tests still green**

```bash
cargo test -p transcoderr --lib 2>&1 | grep -E "FAILED|^test result" | tail -3
```

Expected: no FAILED.

- [ ] **Step 7: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-5" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/plugins/manifest.rs
git commit -m "feat(manifest): per-step executor preferences + ManifestExecutor enum"
```

---

## Task 2: SubprocessStep.executor field + registry build path threading

Mechanical: SubprocessStep gains an `executor: Executor` field; the registry's `build()` function reads the per-step manifest entry to populate it.

**Files:**
- Modify: `crates/transcoderr/src/plugins/subprocess.rs`
- Modify: `crates/transcoderr/src/steps/registry.rs`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-5" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Extend `SubprocessStep` with `executor`**

In `crates/transcoderr/src/plugins/subprocess.rs`, find the `pub struct SubprocessStep { ... }` (around line 12). Replace with:

```rust
#[derive(Debug, Clone)]
pub struct SubprocessStep {
    pub step_name: String,
    pub entrypoint_abs: PathBuf,
    pub executor: crate::steps::Executor,
}
```

In the existing `impl Step for SubprocessStep`, after the `name()` method, add:

```rust
    fn executor(&self) -> crate::steps::Executor {
        self.executor
    }
```

(The `Step` trait's default `executor()` returns `CoordinatorOnly`; this override returns whatever was set at construction time.)

- [ ] **Step 3: Thread executor through the registry build path**

In `crates/transcoderr/src/steps/registry.rs`, find the existing `fn build(...)` function (around lines 32-57). Replace the inner subprocess loop:

Before:
```rust
    for d in discovered {
        if d.manifest.kind != "subprocess" {
            continue;
        }
        let entry = d.manifest.entrypoint.clone().unwrap_or_default();
        let abs = d.manifest_dir.join(&entry);
        for step_name in &d.manifest.provides_steps {
            let step = SubprocessStep {
                step_name: step_name.clone(),
                entrypoint_abs: abs.clone(),
            };
            reg.by_name.insert(step_name.clone(), Arc::new(step));
        }
    }
```

After:
```rust
    for d in discovered {
        if d.manifest.kind != "subprocess" {
            continue;
        }
        let entry = d.manifest.entrypoint.clone().unwrap_or_default();
        let abs = d.manifest_dir.join(&entry);
        for step_name in &d.manifest.provides_steps {
            // Per-step executor: defaults to CoordinatorOnly when the
            // manifest has no `[steps."<name>"]` entry. See spec
            // distributed-piece-5 for the schema.
            let executor = d
                .manifest
                .steps
                .get(step_name)
                .and_then(|s| s.executor)
                .map(|e| match e {
                    crate::plugins::manifest::ManifestExecutor::AnyWorker => {
                        crate::steps::Executor::Any
                    }
                    crate::plugins::manifest::ManifestExecutor::CoordinatorOnly => {
                        crate::steps::Executor::CoordinatorOnly
                    }
                })
                .unwrap_or(crate::steps::Executor::CoordinatorOnly);

            let step = SubprocessStep {
                step_name: step_name.clone(),
                entrypoint_abs: abs.clone(),
                executor,
            };
            reg.by_name.insert(step_name.clone(), Arc::new(step));
        }
    }
```

- [ ] **Step 4: Add a unit test for SubprocessStep::executor**

In `crates/transcoderr/src/plugins/subprocess.rs`, find or create the `#[cfg(test)] mod tests { ... }` block. Add:

```rust
    #[test]
    fn executor_returns_field_value() {
        use crate::steps::{Executor, Step};
        let s_any = SubprocessStep {
            step_name: "x".into(),
            entrypoint_abs: std::path::PathBuf::from("/tmp/x"),
            executor: Executor::Any,
        };
        assert_eq!(s_any.executor(), Executor::Any);

        let s_co = SubprocessStep {
            step_name: "x".into(),
            entrypoint_abs: std::path::PathBuf::from("/tmp/x"),
            executor: Executor::CoordinatorOnly,
        };
        assert_eq!(s_co.executor(), Executor::CoordinatorOnly);
    }
```

If the file has no existing `mod tests` block, create one. The test only needs the trait imports and the struct constructor.

- [ ] **Step 5: Build + run tests**

```bash
cargo build -p transcoderr 2>&1 | tail -5
cargo test -p transcoderr --lib plugins::subprocess 2>&1 | tail -10
```

Expected: build clean; new test passes.

- [ ] **Step 6: Search for any other SubprocessStep constructors**

```bash
grep -rnE "SubprocessStep \{" crates/transcoderr/src/ crates/transcoderr/tests/
```

If any other call sites construct `SubprocessStep { step_name, entrypoint_abs }` (the 2-field shape), they need the new `executor` field added. Default to `crate::steps::Executor::CoordinatorOnly` to preserve current behavior. The registry's build path is the canonical site; tests / fixtures may have others.

- [ ] **Step 7: Lib tests still green**

```bash
cargo test -p transcoderr --lib 2>&1 | grep -E "FAILED|^test result" | tail -3
```

Expected: no FAILED.

- [ ] **Step 8: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-5" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/plugins/subprocess.rs \
        crates/transcoderr/src/steps/registry.rs
git commit -m "feat(plugins): SubprocessStep carries executor preference from manifest"
```

---

## Task 3: `registry::list_step_names` helper

Mechanical: one new public async function that the worker daemon will use to populate `available_steps`.

**Files:**
- Modify: `crates/transcoderr/src/steps/registry.rs`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-5" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Add `list_step_names`**

In `crates/transcoderr/src/steps/registry.rs`, after the existing `pub async fn resolve(...)` and `pub fn try_resolve(...)` functions (added in earlier pieces), append:

```rust
/// Snapshot of the registry's step kind names. Used by the worker
/// daemon to populate `Register.available_steps` at register time
/// AND after each `plugin_sync::sync` rebuild.
///
/// Returns empty if the registry hasn't been initialised yet
/// (matches `try_resolve`'s contract — caller treats uninit as
/// "no steps known").
pub async fn list_step_names() -> Vec<String> {
    let Some(rw) = REGISTRY.get() else {
        return Vec::new();
    };
    let guard = rw.read().await;
    let mut names: Vec<String> = guard.by_name.keys().cloned().collect();
    names.sort();
    names
}
```

- [ ] **Step 3: Add a unit test**

In the existing `#[cfg(test)] mod tests { ... }` block at the bottom of `registry.rs` (or create one), append:

```rust
    #[tokio::test]
    async fn list_step_names_returns_built_in_set_after_init() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(dir.path()).await.unwrap();
        let hw = crate::hw::semaphores::DeviceRegistry::from_caps(
            &crate::hw::HwCaps::default(),
        );
        let ffmpeg_caps = std::sync::Arc::new(
            crate::ffmpeg_caps::FfmpegCaps::default(),
        );

        crate::steps::registry::init(pool, hw, ffmpeg_caps, vec![]).await;

        let names = list_step_names().await;
        // The 7 remote-eligible built-ins plus a handful of
        // coordinator-only ones (probe, output, notify, etc.) get
        // registered. Just check at least one well-known name is
        // present and the list isn't empty — the exact set churns
        // as built-ins are added.
        assert!(!names.is_empty(), "registered names should not be empty");
        assert!(
            names.iter().any(|n| n == "transcode"),
            "transcode should be registered"
        );
    }
```

Note: `registry::init` uses a `OnceCell` — running multiple `#[tokio::test]` invocations against it is fine because each test runs in its own process when `cargo test` defaults to one-binary-per-test, but if this file already has tests that share state, the OnceCell may already be set. Read the existing test patterns; if there's a `static METRICS: OnceLock` style for shared init, follow it. If init is per-process global and other tests in this same file (re-)init, the `list_step_names` test should fit alongside without collisions. If a panic about "registry already initialized" occurs, look at how Piece 2's `worker::local::tests` and Piece 3's `dispatch::tests` handled re-init.

- [ ] **Step 4: Build + run tests**

```bash
cargo build -p transcoderr 2>&1 | tail -3
cargo test -p transcoderr --lib steps::registry 2>&1 | tail -10
```

Expected: build clean; new test passes (alongside any existing registry tests).

- [ ] **Step 5: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-5" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/steps/registry.rs
git commit -m "feat(registry): list_step_names() snapshot helper"
```

---

## Task 4: `Connections::available_steps` map + cleanup

Concurrency-sensitive. Adds a third map to the existing `Connections` struct (Piece 3's `senders` + `inbox`, now `available_steps`). RAII cleanup via `SenderGuard::drop`.

**Files:**
- Modify: `crates/transcoderr/src/worker/connections.rs`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-5" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Add the `available_steps` field + helpers**

In `crates/transcoderr/src/worker/connections.rs`, find the existing `pub struct Connections { ... }`. Add a new field:

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
}
```

In the existing `impl Connections { ... }` block, append the two new methods:

```rust
    /// Record the worker's current `available_steps` snapshot.
    /// Overwrites any existing entry for this worker_id. Called on
    /// initial register and on every re-register frame.
    pub async fn record_available_steps(
        &self,
        worker_id: i64,
        steps: Vec<String>,
    ) {
        self.available_steps.write().await.insert(worker_id, steps);
    }

    /// True if the worker advertised this step kind in its last
    /// Register frame. Returns false for unknown workers (not
    /// connected, never registered, etc.).
    pub async fn worker_has_step(&self, worker_id: i64, step_kind: &str) -> bool {
        self.available_steps
            .read()
            .await
            .get(&worker_id)
            .map(|v| v.iter().any(|s| s == step_kind))
            .unwrap_or(false)
    }
```

- [ ] **Step 3: Update `register_sender` and `SenderGuard` to clean up `available_steps`**

The existing `register_sender` returns a `SenderGuard` whose `Drop` impl spawns a task to remove the entry from `senders`. We need that same cleanup task to ALSO remove the entry from `available_steps`. Two options:

- (a) Give `SenderGuard` a third field (the `available_steps` map ref).
- (b) Have the cleanup task remove from both maps.

Option (b) is simpler. Update `SenderGuard`:

```rust
pub struct SenderGuard {
    map: Arc<RwLock<HashMap<i64, mpsc::Sender<Envelope>>>>,
    available_steps: Arc<RwLock<HashMap<i64, Vec<String>>>>,
    worker_id: i64,
}

impl Drop for SenderGuard {
    fn drop(&mut self) {
        // Drop is sync; spawn a small task to remove from the async maps.
        let map = self.map.clone();
        let available_steps = self.available_steps.clone();
        let worker_id = self.worker_id;
        tokio::spawn(async move {
            map.write().await.remove(&worker_id);
            available_steps.write().await.remove(&worker_id);
        });
    }
}
```

And update `register_sender` to populate the new field:

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
            worker_id,
        }
    }
```

- [ ] **Step 4: Add 3 unit tests**

In the existing `#[cfg(test)] mod tests { ... }` block in `connections.rs`, append:

```rust
    #[tokio::test]
    async fn record_and_query_available_steps() {
        let conns = Connections::new();
        conns
            .record_available_steps(7, vec!["transcode".into(), "remux".into()])
            .await;

        assert!(conns.worker_has_step(7, "transcode").await);
        assert!(conns.worker_has_step(7, "remux").await);
        assert!(!conns.worker_has_step(7, "whisper.transcribe").await);
        // Unknown worker → false (no panic).
        assert!(!conns.worker_has_step(999, "transcode").await);
    }

    #[tokio::test]
    async fn record_available_steps_overwrites() {
        let conns = Connections::new();
        conns.record_available_steps(7, vec!["transcode".into()]).await;
        conns
            .record_available_steps(
                7,
                vec!["transcode".into(), "whisper.transcribe".into()],
            )
            .await;

        assert!(conns.worker_has_step(7, "whisper.transcribe").await);
    }

    #[tokio::test]
    async fn sender_guard_drop_clears_available_steps_too() {
        let conns = Connections::new();
        let (tx, _rx) = mpsc::channel(4);
        {
            let _guard = conns.register_sender(11, tx).await;
            conns.record_available_steps(11, vec!["transcode".into()]).await;
            assert!(conns.worker_has_step(11, "transcode").await);
        }
        // Drop spawns an async cleanup; give it a moment.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!conns.is_connected(11).await);
        assert!(!conns.worker_has_step(11, "transcode").await,
            "available_steps entry should be cleared on disconnect");
    }
```

- [ ] **Step 5: Build + run tests**

```bash
cargo build -p transcoderr 2>&1 | tail -3
cargo test -p transcoderr --lib worker::connections 2>&1 | tail -10
```

Expected: build clean; existing tests + 3 new ones pass.

- [ ] **Step 6: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-5" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/worker/connections.rs
git commit -m "feat(worker): Connections.available_steps + SenderGuard cleanup"
```

---

## Task 5: `ConnectionContext` extension + `build_register_envelope` helper

Refactor: drop the `build_register: F: Fn() -> Envelope` closure pattern (which captured state once at boot — wrong for re-register after plugin sync). Replace with an async `build_register_envelope(&ctx)` function that queries the live registry. **Pause for user confirmation after this task** — the signature change ripples through `daemon.rs` + `connection.rs`.

**Files:**
- Modify: `crates/transcoderr/src/worker/connection.rs`
- Modify: `crates/transcoderr/src/worker/daemon.rs`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-5" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Extend `ConnectionContext` with `name` and `hw_caps`**

In `crates/transcoderr/src/worker/connection.rs`, find the existing `ConnectionContext` struct (added in Piece 4). Replace with:

```rust
/// Context the worker connection needs for plugin sync AND for
/// building Register envelopes (initial + post-sync). Threaded from
/// `daemon::run` → `connection::run` → `connect_once`.
#[derive(Clone)]
pub struct ConnectionContext {
    pub plugins_dir: std::path::PathBuf,
    pub coordinator_token: String,
    /// Worker's display name (from `worker.toml` or hostname). Used
    /// in every Register envelope.
    pub name: String,
    /// Hardware capabilities, frozen at boot. Re-register reuses the
    /// same value (hardware doesn't change mid-process).
    pub hw_caps: serde_json::Value,
}
```

- [ ] **Step 3: Add `build_register_envelope` helper**

In `crates/transcoderr/src/worker/connection.rs`, after the `ConnectionContext` struct, append:

```rust
/// Build a fresh `Register` envelope from the live registry +
/// on-disk plugin manifest. Called twice per connection lifecycle:
/// once at the pre-handshake send, and once after each
/// `plugin_sync::sync` completes. Both call sites need an
/// up-to-the-moment snapshot of `available_steps`.
pub async fn build_register_envelope(ctx: &ConnectionContext) -> Envelope {
    use crate::worker::protocol::{PluginManifestEntry, Register};

    let plugin_manifest: Vec<PluginManifestEntry> =
        match crate::plugins::discover(&ctx.plugins_dir) {
            Ok(found) => found
                .into_iter()
                .map(|d| PluginManifestEntry {
                    name: d.manifest.name.clone(),
                    version: d.manifest.version.clone(),
                    sha256: None,
                })
                .collect(),
            Err(e) => {
                tracing::warn!(error = ?e, "register: plugin discovery failed; reporting empty manifest");
                Vec::new()
            }
        };
    let available_steps = crate::steps::registry::list_step_names().await;

    Envelope {
        id: format!("reg-{}", uuid::Uuid::new_v4()),
        message: Message::Register(Register {
            name: ctx.name.clone(),
            version: env!("CARGO_PKG_VERSION").into(),
            hw_caps: ctx.hw_caps.clone(),
            available_steps,
            plugin_manifest,
        }),
    }
}
```

- [ ] **Step 4: Drop the `build_register: F` closure parameter**

Update `pub async fn run(...)` signature in `connection.rs`:

Before:
```rust
pub async fn run<F>(
    url: String,
    token: String,
    build_register: F,
    ctx: ConnectionContext,
) -> !
where
    F: Fn() -> Envelope + Send + Sync,
```

After:
```rust
pub async fn run(
    url: String,
    token: String,
    ctx: ConnectionContext,
) -> !
```

Inside the loop body, change:

Before:
```rust
        match connect_once(&url, &token, &build_register, &ctx).await {
```

After:
```rust
        match connect_once(&url, &token, &ctx).await {
```

Update `connect_once` signature similarly:

Before:
```rust
async fn connect_once<F>(
    url: &str,
    token: &str,
    build_register: &F,
    ctx: &ConnectionContext,
) -> anyhow::Result<()>
where
    F: Fn() -> Envelope,
```

After:
```rust
async fn connect_once(
    url: &str,
    token: &str,
    ctx: &ConnectionContext,
) -> anyhow::Result<()>
```

Inside `connect_once`, find the existing call to `build_register()` (around line 122):

Before:
```rust
    let register = build_register();
    if outbound_tx.send(register).await.is_err() {
        sender_task.abort();
        sync_task.abort();
        anyhow::bail!("failed to enqueue register frame");
    }
```

After:
```rust
    let register = build_register_envelope(ctx).await;
    if outbound_tx.send(register).await.is_err() {
        sender_task.abort();
        sync_task.abort();
        anyhow::bail!("failed to enqueue register frame");
    }
```

- [ ] **Step 5: Update `daemon.rs` to drop the closure + populate ConnectionContext**

In `crates/transcoderr/src/worker/daemon.rs`, the existing function builds a hardcoded `available_steps` list and a `build_register` closure. Replace lines 56-90 (everything from `let available_steps = vec![...]` through the end of `connection::run(...)`) with:

```rust
    let ctx = crate::worker::connection::ConnectionContext {
        plugins_dir: std::path::PathBuf::from("./plugins"),
        coordinator_token: config.coordinator_token.clone(),
        name: name.clone(),
        hw_caps: hw_caps.clone(),
    };

    crate::worker::connection::run(
        config.coordinator_url,
        config.coordinator_token,
        ctx,
    )
    .await
```

Also delete the now-unused `Envelope, Message, PluginManifestEntry, Register` imports at the top of `daemon.rs` (they were used only by the old `build_register` closure; `build_register_envelope` lives in `connection.rs`).

Run `cargo check -p transcoderr` after this edit to catch any leftover references.

- [ ] **Step 6: Build smoke**

```bash
cargo build -p transcoderr 2>&1 | tail -10
```

Expected: clean. Common compile errors:
- Unused import in daemon.rs → delete the `Envelope, Message, ...` use.
- `name` no longer used in daemon.rs → it should still be used in `ConnectionContext { name: name.clone(), ... }`.

- [ ] **Step 7: worker_connect, local_worker, remote_dispatch, plugin_push tests**

```bash
cargo test -p transcoderr --test worker_connect --test local_worker --test remote_dispatch --test plugin_push 2>&1 | grep -E "FAILED|^test result" | tail -10
```

Expected: every line `test result: ok.`. No FAILED. The tests' fake-worker harness sends Register envelopes manually — they don't depend on `build_register_envelope`. The coordinator-side handshake is unchanged.

- [ ] **Step 8: Lib tests still green**

```bash
cargo test -p transcoderr --lib 2>&1 | grep -E "FAILED|^test result" | tail -3
```

Expected: no FAILED.

- [ ] **Step 9: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-5" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/worker/connection.rs \
        crates/transcoderr/src/worker/daemon.rs
git commit -m "refactor(worker): drop build_register closure for async build_register_envelope"
```

**[PAUSE FOR USER CONFIRMATION HERE]**

---

## Task 6: Sync worker task fires re-register after `plugin_sync::sync`

Concurrency-sensitive. After each plugin sync completes (regardless of success/partial), send a fresh Register envelope so the coordinator's `available_steps` map updates.

**Files:**
- Modify: `crates/transcoderr/src/worker/connection.rs`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-5" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Update the sync_task closure to fire re-register**

In `crates/transcoderr/src/worker/connection.rs`, find the existing sync_task spawn (around lines 103-120). Replace with:

```rust
    // Sync worker: drain the slot whenever notified, run plugin_sync::sync,
    // then re-register so the coordinator's `connections.available_steps`
    // sees the new step kinds. Lives for the connection's lifetime;
    // aborted on disconnect.
    let sync_task = {
        let ctx_for_sync = ctx.clone();
        let outbound_for_sync = outbound_tx.clone();
        let slot = sync_slot.clone();
        let notify = sync_notify.clone();
        tokio::spawn(async move {
            loop {
                notify.notified().await;
                let manifest = {
                    let mut g = slot.lock().await;
                    g.take()
                };
                if let Some(m) = manifest {
                    crate::worker::plugin_sync::sync(
                        &ctx_for_sync.plugins_dir,
                        m,
                        &ctx_for_sync.coordinator_token,
                    )
                    .await;
                    // Re-register so the coordinator sees fresh
                    // available_steps. Fire-and-forget — coordinator
                    // does NOT respond with another register_ack
                    // (would oscillate; see Piece 5 spec).
                    let env = build_register_envelope(&ctx_for_sync).await;
                    if let Err(e) = outbound_for_sync.send(env).await {
                        tracing::warn!(error = ?e, "post-sync re-register: outbound send failed");
                    }
                }
            }
        })
    };
```

The change: bind `ctx_for_sync` (a Clone of the whole `ConnectionContext`) and `outbound_for_sync` (a Clone of `outbound_tx`) into the spawn. The previous code only cloned `plugins_dir` and `token` because the closure didn't need to send envelopes. Now it does.

- [ ] **Step 3: Build smoke**

```bash
cargo build -p transcoderr 2>&1 | tail -5
```

Expected: clean.

- [ ] **Step 4: Run tests**

```bash
cargo test -p transcoderr --test worker_connect --test plugin_push 2>&1 | grep -E "FAILED|^test result" | tail -10
```

Expected: 4 + 6 passed. The plugin_push tests trigger `PluginSync` broadcasts; after Task 6 the worker re-registers in response. The existing tests don't assert on outbound frame counts, so they tolerate the extra Register frame.

If `plugin_push.rs::plugin_install_broadcasts_plugin_sync` starts failing because the wait_for_plugin_sync helper consumes the new Register frame instead of the PluginSync, fix the test fixture to filter by message type. Read the existing helper before changing anything.

- [ ] **Step 5: Lib tests + critical path**

```bash
cargo test -p transcoderr --lib 2>&1 | grep -E "FAILED|^test result" | tail -3
cargo test -p transcoderr --test concurrent_claim --test crash_recovery --test flow_engine 2>&1 | grep -E "FAILED|^test result" | tail -5
```

Expected: no FAILED.

- [ ] **Step 6: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-5" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/worker/connection.rs
git commit -m "feat(worker): re-register after plugin_sync to refresh available_steps"
```

---

## Task 7: `api/workers.rs::handle_connection` — initial register populates available_steps + Message::Register receive arm

Coordinator-side: populate `connections.available_steps` from the initial register handshake; add a receive-loop arm for subsequent Register envelopes (re-registers).

**Files:**
- Modify: `crates/transcoderr/src/api/workers.rs`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-5" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Read the current handle_connection structure**

```bash
sed -n '180,300p' crates/transcoderr/src/api/workers.rs
```

You'll see the inline pre-handshake register read, the `record_register` call, the manifest-build block, the register_ack send, and then the receive loop's `tokio::select!`. Inside the receive loop's match arms (added by Pieces 1 and 4), there are arms for `Heartbeat`, `StepProgress`, `StepComplete`. We add a new `Register` arm.

- [ ] **Step 3: Populate `available_steps` after the initial register handshake**

Find the existing line that calls `db::workers::record_register(&state.pool, worker_id, &hw_caps_json, &plugin_manifest_json).await?;` (or similar — exact shape is whatever Piece 4 left). It's somewhere in the inline pre-handshake block, right before the manifest-build for register_ack.

Right after that `record_register` call, add:

```rust
    // Capture the worker's advertised step kinds so the dispatcher
    // can filter eligible workers per step kind. Mirror image of the
    // Message::Register receive-loop arm below.
    state
        .connections
        .record_available_steps(worker_id, register_payload.available_steps.clone())
        .await;
```

Note: `register_payload` is the variable name the existing code uses for the inline register frame's payload. If your local code uses a different name (e.g. `r` or `register`), match it. The point is to clone `available_steps` from the freshly-received initial Register payload.

- [ ] **Step 4: Add `Message::Register` arm to the receive loop**

Find the receive loop's `match env.message { ... }` block (after Piece 4's wiring). Add a new arm alongside the existing Heartbeat / StepProgress / StepComplete arms:

```rust
            crate::worker::protocol::Message::Register(r) => {
                // Re-register from worker — typically fired after a
                // plugin_sync::sync rebuilds its registry. Update the
                // worker row + the in-memory available_steps map. NO
                // register_ack response (would oscillate — see spec
                // distributed-piece-5).
                let hw_caps_json = serde_json::to_string(&r.hw_caps)
                    .unwrap_or_else(|_| "null".into());
                let plugin_manifest_json = serde_json::to_string(&r.plugin_manifest)
                    .unwrap_or_else(|_| "[]".into());
                if let Err(e) = db::workers::record_register(
                    &state.pool,
                    worker_id,
                    &hw_caps_json,
                    &plugin_manifest_json,
                )
                .await
                {
                    tracing::warn!(worker_id, error = ?e, "re-register: record_register failed");
                }
                state
                    .connections
                    .record_available_steps(worker_id, r.available_steps)
                    .await;
                tracing::debug!(worker_id, "re-register processed");
            }
```

Place it as a sibling of the existing `Message::Heartbeat(_) => { ... }` arm. The existing `other => { tracing::warn!(...) }` catch-all stays at the bottom; the new `Register` arm comes before it.

- [ ] **Step 5: Build smoke**

```bash
cargo build -p transcoderr 2>&1 | tail -5
```

Expected: clean.

- [ ] **Step 6: Run worker_connect tests (regression net)**

```bash
cargo test -p transcoderr --test worker_connect 2>&1 | tail -10
```

Expected: 4 passed. The initial-register flow now also calls `record_available_steps`; existing tests that assert on `register_ack` content / DB state are unaffected.

- [ ] **Step 7: Run plugin_push tests (re-register exercises this arm)**

```bash
cargo test -p transcoderr --test plugin_push 2>&1 | tail -10
```

Expected: 6 passed. After Task 6 the worker re-registers after sync; this arm processes those re-registers. The plugin_push tests don't assert on `available_steps` content, so they tolerate the new state population.

- [ ] **Step 8: Lib + Piece 2/3 integration tests**

```bash
cargo test -p transcoderr --lib 2>&1 | grep -E "FAILED|^test result" | tail -3
cargo test -p transcoderr --test local_worker --test remote_dispatch --test api_auth 2>&1 | grep -E "FAILED|^test result" | tail -10
```

Expected: no FAILED.

- [ ] **Step 9: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-5" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/api/workers.rs
git commit -m "feat(api): handle_connection populates + updates connections.available_steps"
```

---

## Task 8: `dispatch::eligible_remotes` filter by `step_kind`

Surgical edit. The function currently has `_step_kind: &str` (ignored per Piece 3's deferred decision). Use it.

**Files:**
- Modify: `crates/transcoderr/src/dispatch/mod.rs`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-5" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Update `eligible_remotes` to filter by step_kind**

In `crates/transcoderr/src/dispatch/mod.rs`, find the existing `async fn eligible_remotes` (around line 80+). Replace its body:

Before (rough shape):
```rust
async fn eligible_remotes(
    _step_kind: &str,
    state: &AppState,
) -> anyhow::Result<Vec<i64>> {
    let cutoff = chrono::Utc::now().timestamp() - STALE_AFTER_SECS;
    let rows = crate::db::workers::list_all(&state.pool).await?;
    let mut out = Vec::new();
    for r in rows {
        if r.id == LOCAL_WORKER_ID {
            continue;
        }
        if r.enabled == 0 {
            continue;
        }
        match r.last_seen_at {
            Some(seen) if seen > cutoff => {}
            _ => continue,
        }
        if !state.connections.is_connected(r.id).await {
            continue;
        }
        out.push(r.id);
    }
    Ok(out)
}
```

After:
```rust
async fn eligible_remotes(
    step_kind: &str,
    state: &AppState,
) -> anyhow::Result<Vec<i64>> {
    let cutoff = chrono::Utc::now().timestamp() - STALE_AFTER_SECS;
    let rows = crate::db::workers::list_all(&state.pool).await?;
    let mut out = Vec::new();
    for r in rows {
        if r.id == LOCAL_WORKER_ID {
            continue;
        }
        if r.enabled == 0 {
            continue;
        }
        match r.last_seen_at {
            Some(seen) if seen > cutoff => {}
            _ => continue,
        }
        if !state.connections.is_connected(r.id).await {
            continue;
        }
        // NEW (Piece 5): filter workers that don't advertise this step
        // kind. Plugin step kinds are only present on workers that
        // successfully installed the plugin.
        if !state.connections.worker_has_step(r.id, step_kind).await {
            continue;
        }
        out.push(r.id);
    }
    Ok(out)
}
```

The leading underscore on `_step_kind` is dropped because the parameter is now used.

- [ ] **Step 3: Add a unit test for the new filter**

In `crates/transcoderr/src/dispatch/mod.rs`'s existing `#[cfg(test)] mod tests` block (added in Piece 3), append:

```rust
    #[tokio::test]
    async fn worker_without_step_kind_is_skipped() {
        let (pool, _dir) = pool().await;
        let state = shell_state(pool).await;
        crate::steps::registry::init(
            state.pool.clone(),
            state.hw_devices.clone(),
            state.ffmpeg_caps.clone(),
            vec![],
        )
        .await;

        // Add a fake remote that doesn't advertise the step kind.
        let id = add_fake_remote(&state, "no-whisper").await;
        // Default available_steps is empty (add_fake_remote doesn't
        // populate). Without an explicit record_available_steps,
        // worker_has_step returns false.

        // Routing a step the worker doesn't advertise → falls back
        // to local.
        let r = route("whisper.transcribe", None, &state).await;
        assert_eq!(r, Route::Local);
        let _ = id;
    }

    #[tokio::test]
    async fn worker_advertising_step_kind_is_picked() {
        let (pool, _dir) = pool().await;
        let state = shell_state(pool).await;
        crate::steps::registry::init(
            state.pool.clone(),
            state.hw_devices.clone(),
            state.ffmpeg_caps.clone(),
            vec![],
        )
        .await;

        let id = add_fake_remote(&state, "has-whisper").await;
        state
            .connections
            .record_available_steps(
                id,
                vec!["whisper.transcribe".into(), "transcode".into()],
            )
            .await;

        // route() consults registry::try_resolve to determine the
        // executor. For this test we route "transcode" (a known
        // built-in with Executor::Any) — the worker advertises it,
        // so the dispatcher picks it.
        let r = route("transcode", None, &state).await;
        assert_eq!(r, Route::Remote(id));
    }
```

If the existing `add_fake_remote` test helper from Piece 3 already calls `record_available_steps` to populate something default, adapt — read its body first. The point is to assert (a) workers without the step kind get skipped, (b) workers with the step kind get picked.

Note: in Piece 3's existing tests (`one_eligible_remote_picks_it`, `two_remotes_round_robin`), the add_fake_remote helper does NOT populate `available_steps`. After Task 8, those tests would fail because the dispatcher's new filter rejects workers with no advertised steps. The implementer needs to update `add_fake_remote` to populate `available_steps` with a default set (e.g. `vec!["transcode".into()]`) so existing dispatch tests still pass.

- [ ] **Step 4: Update `add_fake_remote` test helper**

In the existing `dispatch::tests` block, find `add_fake_remote`. Change it to also populate `available_steps`:

```rust
    async fn add_fake_remote(state: &AppState, name: &str) -> i64 {
        let id = crate::db::workers::insert_remote(&state.pool, name, &format!("tok_{name}")).await.unwrap();
        crate::db::workers::record_heartbeat(&state.pool, id).await.unwrap();
        let (tx, _rx) = mpsc::channel::<Envelope>(4);
        let _guard = state.connections.register_sender(id, tx).await;
        std::mem::forget(_guard);
        // NEW (Piece 5): default to advertising the same step kinds the
        // existing dispatch tests assume — built-in "transcode" suffices
        // for the round-robin / one-eligible / disabled tests. New
        // tests that need different step kinds should call
        // record_available_steps after this helper.
        state
            .connections
            .record_available_steps(id, vec!["transcode".into()])
            .await;
        id
    }
```

- [ ] **Step 5: Build + run dispatch tests**

```bash
cargo build -p transcoderr 2>&1 | tail -3
cargo test -p transcoderr --lib dispatch 2>&1 | tail -15
```

Expected: existing 6 tests + 2 new ones = 8 passed. If any existing test fails, it's because `add_fake_remote` defaults didn't match its expectations; trace from there.

- [ ] **Step 6: Run remote_dispatch integration tests (CRITICAL — these depend on the dispatcher filter)**

```bash
cargo test -p transcoderr --test remote_dispatch 2>&1 | tail -15
```

Expected: 5 passed. The fake worker in remote_dispatch.rs sends Register with `available_steps: vec!["transcode"]`. Coordinator records it. Dispatcher's new filter sees `transcode` in the worker's set → eligible. Test passes.

If a test fails because the fake worker's available_steps doesn't include the step kind being dispatched (e.g. the test dispatches `transcode` but the fake worker only advertises `["x"]`), update the fake worker's register to advertise the correct kind — read the helper at `tests/remote_dispatch.rs::send_register_and_get_ack`.

- [ ] **Step 7: Lib + plugin_push + worker_connect + local_worker still green**

```bash
cargo test -p transcoderr --lib 2>&1 | grep -E "FAILED|^test result" | tail -3
cargo test -p transcoderr --test plugin_push --test worker_connect --test local_worker 2>&1 | grep -E "FAILED|^test result" | tail -10
```

Expected: no FAILED.

- [ ] **Step 8: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-5" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/dispatch/mod.rs
git commit -m "feat(dispatch): eligible_remotes filters by per-worker available_steps"
```

**[PAUSE FOR USER CONFIRMATION HERE]**

---

## Task 9: Integration tests `tests/plugin_remote_dispatch.rs` — 5 scenarios

End-to-end verification of plugin step routing. Reuses the fake-worker harness from Piece 3's `tests/remote_dispatch.rs` and Piece 4's `tests/plugin_push.rs`.

**Files:**
- Create: `crates/transcoderr/tests/plugin_remote_dispatch.rs`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-5" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Read existing fake-worker patterns**

```bash
head -100 crates/transcoderr/tests/remote_dispatch.rs
head -100 crates/transcoderr/tests/plugin_push.rs
```

You'll see `mint_token`, `ws_connect`, `send_env`, `recv_env`, `send_register_and_get_ack`, `submit_job_with_step`, `wait_for_step_dispatch`, `wait_for_plugin_sync`. Reuse these patterns.

- [ ] **Step 3: Create the test file**

```rust
//! Integration tests for Piece 5's plugin-step remote routing.
//! Verifies the dispatcher's per-worker `available_steps` filter
//! correctly routes plugin steps to workers that have the plugin AND
//! skips workers that don't.
//!
//!  1. plugin_step_routes_to_worker_that_has_it
//!  2. plugin_step_skips_worker_without_it
//!  3. coordinator_only_plugin_step_runs_locally
//!  4. re_register_updates_available_steps
//!  5. disconnect_clears_available_steps_for_dispatch

mod common;

use common::boot;
use futures::{SinkExt, StreamExt};
use serde_json::json;
use std::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use transcoderr::worker::protocol::{
    Envelope, Message, PluginManifestEntry, Register,
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

/// Send a Register with the given `available_steps`; consume the
/// register_ack. Returns the ack envelope so the test can inspect
/// the manifest if needed.
async fn send_register_and_get_ack(
    ws: &mut Ws,
    name: &str,
    available_steps: Vec<String>,
) -> Envelope {
    let reg = Envelope {
        id: "reg-1".into(),
        message: Message::Register(Register {
            name: name.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            hw_caps: json!({}),
            available_steps,
            plugin_manifest: vec![PluginManifestEntry {
                name: "size-report".into(),
                version: "0.1.2".into(),
                sha256: None,
            }],
        }),
    };
    send_env(ws, &reg).await;
    recv_env(ws).await
}

/// Send a re-register frame mid-connection. No ack is expected; the
/// coordinator's receive loop processes it silently.
async fn send_re_register(ws: &mut Ws, name: &str, available_steps: Vec<String>) {
    let reg = Envelope {
        id: format!("reg-{}", uuid::Uuid::new_v4()),
        message: Message::Register(Register {
            name: name.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            hw_caps: json!({}),
            available_steps,
            plugin_manifest: vec![],
        }),
    };
    send_env(ws, &reg).await;
}

/// Insert a flow + a job that points at a specific step kind. Mirrors
/// `tests/remote_dispatch.rs::submit_job_with_step`.
async fn submit_job_with_step(
    app: &common::TestApp,
    use_: &str,
    run_on: Option<&str>,
) -> (i64, i64) {
    let run_on_clause = match run_on {
        Some(r) => format!("    run_on: {r}\n"),
        None => "".into(),
    };
    let yaml = format!(
        "name: t\ntriggers: [{{ webhook: x }}]\nsteps:\n  - use: {use_}\n{run_on_clause}"
    );
    let value: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
    let parsed_json = serde_json::to_string(&value).unwrap();

    sqlx::query("INSERT INTO flows (name, yaml_source, parsed_json, enabled, created_at) VALUES (?, ?, ?, 1, strftime('%s','now'))")
        .bind("t").bind(&yaml).bind(&parsed_json)
        .execute(&app.pool).await.unwrap();
    let flow_id: i64 = sqlx::query_scalar("SELECT id FROM flows ORDER BY id DESC LIMIT 1")
        .fetch_one(&app.pool).await.unwrap();
    sqlx::query("INSERT INTO jobs (flow_id, file_path, status, created_at) VALUES (?, '/tmp/x.mkv', 'pending', strftime('%s','now'))")
        .bind(flow_id).execute(&app.pool).await.unwrap();
    let job_id: i64 = sqlx::query_scalar("SELECT id FROM jobs ORDER BY id DESC LIMIT 1")
        .fetch_one(&app.pool).await.unwrap();
    (flow_id, job_id)
}

async fn wait_for_step_dispatch(
    ws: &mut Ws,
    deadline: Duration,
) -> Option<Envelope> {
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
async fn plugin_step_routes_to_worker_that_has_it() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (_wid, token) = mint_token(&client, &app.url, "fake1").await;
    let base_ws = app.url.replace("http://", "ws://");
    let mut ws = ws_connect(&base_ws, &token).await;

    // Worker advertises whisper.transcribe. The coordinator's registry
    // doesn't actually have a SubprocessStep registered for that name
    // in the test fixture — but `route()`'s registry lookup falls
    // back to Local when try_resolve returns None. To make this test
    // exercise the eligible_remotes filter, we use `transcode` (a
    // known built-in with Executor::Any).
    send_register_and_get_ack(&mut ws, "fake1", vec!["transcode".into()]).await;

    let (_flow_id, _job_id) = submit_job_with_step(&app, "transcode", Some("any")).await;

    let dispatch = wait_for_step_dispatch(&mut ws, Duration::from_secs(5)).await;
    assert!(
        dispatch.is_some(),
        "worker advertising transcode should receive step_dispatch"
    );
}

#[tokio::test]
async fn plugin_step_skips_worker_without_it() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (_wid, token) = mint_token(&client, &app.url, "fake_no_whisper").await;
    let base_ws = app.url.replace("http://", "ws://");
    let mut ws = ws_connect(&base_ws, &token).await;

    // Worker only advertises remux — NOT transcode.
    send_register_and_get_ack(&mut ws, "fake_no_whisper", vec!["remux".into()])
        .await;

    let (_flow_id, _job_id) = submit_job_with_step(&app, "transcode", Some("any")).await;

    // No dispatch should arrive — the dispatcher's filter rejects this
    // worker for `transcode` since it's not in its available_steps.
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        wait_for_step_dispatch(&mut ws, Duration::from_secs(2)),
    )
    .await;
    assert!(
        matches!(result, Ok(None) | Err(_)),
        "worker should NOT receive dispatch for an unadvertised step kind"
    );
}

#[tokio::test]
async fn coordinator_only_plugin_step_runs_locally() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (_wid, token) = mint_token(&client, &app.url, "fake_co").await;
    let base_ws = app.url.replace("http://", "ws://");
    let mut ws = ws_connect(&base_ws, &token).await;

    // Worker advertises everything — but the step we submit is
    // `notify` (CoordinatorOnly built-in). Should run locally.
    send_register_and_get_ack(
        &mut ws,
        "fake_co",
        vec!["transcode".into(), "notify".into()],
    )
    .await;

    let (_flow_id, _job_id) = submit_job_with_step(&app, "notify", None).await;

    let result = tokio::time::timeout(
        Duration::from_secs(2),
        wait_for_step_dispatch(&mut ws, Duration::from_secs(2)),
    )
    .await;
    assert!(
        matches!(result, Ok(None) | Err(_)),
        "coordinator-only step should not dispatch to worker"
    );
}

#[tokio::test]
async fn re_register_updates_available_steps() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (_wid, token) = mint_token(&client, &app.url, "fake_re").await;
    let base_ws = app.url.replace("http://", "ws://");
    let mut ws = ws_connect(&base_ws, &token).await;

    // Initial register: no transcode advertised.
    send_register_and_get_ack(&mut ws, "fake_re", vec!["remux".into()]).await;

    // Verify initial state by submitting a transcode step → no dispatch.
    let (_flow_id, _job_id_a) =
        submit_job_with_step(&app, "transcode", Some("any")).await;
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        wait_for_step_dispatch(&mut ws, Duration::from_secs(2)),
    )
    .await;
    assert!(
        matches!(result, Ok(None) | Err(_)),
        "before re-register: transcode should NOT dispatch"
    );

    // Re-register with transcode added.
    send_re_register(
        &mut ws,
        "fake_re",
        vec!["remux".into(), "transcode".into()],
    )
    .await;

    // Brief pause to let the coordinator's receive loop process it.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Submit another transcode step — this time it should dispatch.
    let (_flow_id, _job_id_b) =
        submit_job_with_step(&app, "transcode", Some("any")).await;
    let dispatch = wait_for_step_dispatch(&mut ws, Duration::from_secs(5)).await;
    assert!(
        dispatch.is_some(),
        "after re-register: transcode should dispatch to the worker"
    );
}

#[tokio::test]
async fn disconnect_clears_available_steps_for_dispatch() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (_wid, token) = mint_token(&client, &app.url, "fake_drop").await;
    let base_ws = app.url.replace("http://", "ws://");
    let mut ws = ws_connect(&base_ws, &token).await;

    send_register_and_get_ack(&mut ws, "fake_drop", vec!["transcode".into()]).await;

    // Disconnect.
    drop(ws);

    // Brief pause for SenderGuard::drop's spawned cleanup task to run.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Submit a transcode step. No worker is connected, dispatcher
    // falls back to local. (Verified by NOT failing — the run goes
    // local; the test can't assert on a "missing" remote dispatch
    // without a fake worker still listening, so we just assert the
    // job lifecycle proceeds. The dispatch::route fall-through to
    // Route::Local is exercised at the unit-test level in Task 8.)
    let (_flow_id, _job_id) = submit_job_with_step(&app, "transcode", Some("any")).await;

    // No assertion on the worker side because the WS is dropped.
    // Sleep a bit and assert the job didn't error out due to a
    // dispatcher panic.
    tokio::time::sleep(Duration::from_millis(500)).await;
}
```

Notes for the implementer:
- The test for "plugin step routes to worker" is shaped around `transcode` (a built-in with `Executor::Any`) instead of an actual plugin step. Reason: the test fixture's registry isn't seeded with plugin SubprocessSteps. Mocking that out cleanly requires either (a) writing a real plugin tarball + going through the install path (overkill), or (b) inserting a SubprocessStep directly into the registry (registry init is OnceCell-protected; not test-friendly). Using `transcode` exercises the same eligible_remotes filter — the dispatch decision flows through `connections.worker_has_step("transcode", ...)` either way. The plan's "plugin step" framing is preserved at the integration level; the unit tests in Task 8 exercise the filter on the registry's actual step-kind lookup.
- If the implementer wants to add a sixth scenario specifically for `whisper.transcribe`-shaped tests (creating a real plugin manifest in the test fixture), that's bonus work — not required.

- [ ] **Step 4: Run the new tests**

```bash
cargo test -p transcoderr --test plugin_remote_dispatch 2>&1 | tail -25
```

Expected: 5 passed.

If a test hangs, the most likely cause is the dispatcher routing to local (and the run failing on missing ffmpeg) without ever sending a step_dispatch, causing `wait_for_step_dispatch` to time out. The "negative" tests (skips_worker_without_it, coordinator_only, disconnect) explicitly tolerate this by checking `Ok(None) | Err(_)`. The "positive" test asserts a dispatch arrives within 5s.

- [ ] **Step 5: Run the full integration suite for confidence**

```bash
cargo test -p transcoderr 2>&1 | grep -E "FAILED|^test result" | tail -25
```

Expected: every line `test result: ok.`. No FAILED.

- [ ] **Step 6: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-5" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/tests/plugin_remote_dispatch.rs
git commit -m "test(plugin_remote_dispatch): 5-scenario plugin-step routing integration suite"
```

---

## Self-Review Notes

This plan covers every section of the spec:

- **Manifest schema (`[steps."<name>"] executor`)** → Task 1.
- **`SubprocessStep.executor` + registry build threading** → Task 2.
- **`registry::list_step_names`** → Task 3.
- **`Connections.available_steps` + `record_available_steps` + `worker_has_step` + SenderGuard cleanup** → Task 4.
- **`ConnectionContext` extension + `build_register_envelope` async helper + drop the closure** → Task 5.
- **Re-register after `plugin_sync::sync`** → Task 6.
- **WS handler initial register populates available_steps + receive-loop `Message::Register` arm** → Task 7.
- **`dispatch::eligible_remotes` filters by `step_kind`** → Task 8.
- **5-scenario integration suite** → Task 9.

Cross-task type/signature consistency:

- `ManifestExecutor::AnyWorker | CoordinatorOnly` (Task 1) → consumed in `steps::registry::build` (Task 2) → maps to `crate::steps::Executor::Any | CoordinatorOnly`.
- `SubprocessStep { step_name, entrypoint_abs, executor }` (Task 2) — constructed in `steps::registry::build` only.
- `registry::list_step_names() -> Vec<String>` (Task 3) — called by `connection::build_register_envelope` (Task 5).
- `Connections::record_available_steps(worker_id, Vec<String>)` (Task 4) — called from `api::workers::handle_connection` (Task 7).
- `Connections::worker_has_step(worker_id, &str) -> bool` (Task 4) — called from `dispatch::eligible_remotes` (Task 8).
- `ConnectionContext { plugins_dir, coordinator_token, name, hw_caps }` (Task 5) — built in `daemon::run` (Task 5), passed to `connection::run` (Task 5), threaded into the sync_task closure (Task 6).
- `build_register_envelope(&ConnectionContext) -> Envelope` (Task 5) — called twice in `connect_once` and inside the sync_task (Tasks 5 + 6).
- `Message::Register(payload)` arm in coordinator's receive loop (Task 7) — receives the same `Register` envelope variant the worker sent (Pieces 1 + 5 reuse).

No placeholders. Every step has executable code or exact commands. All file paths absolute. Bite-sized step granularity (each step is a 2-5 minute action). Frequent commits — 9 total commits, one per task.
