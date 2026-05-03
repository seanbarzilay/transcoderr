# Distributed Transcoding — Piece 5: Plugin Steps Remote-Eligible

## Goal

Plugin-provided step kinds become remote-dispatchable. A plugin manifest
declares per-step executor preferences via `[steps."<step_kind>"]
executor = "any-worker"`. The dispatcher's per-worker step-kind filter
(deferred in Piece 3) goes live, so the coordinator only routes a
plugin step to a worker that actually has the plugin installed. This
unlocks heavy plugins (e.g. whisper transcription) to offload onto
remote workers.

This piece does NOT add failure handling or reassignment — that's
Piece 6.

## Roadmap context

- Roadmap parent: `docs/superpowers/specs/2026-05-02-distributed-transcoding-design.md` (PR #81 merged).
- Piece 1: PR #83 / v0.32.0 — wire protocol skeleton + worker daemon.
- Piece 2: PR #85 / v0.33.0 — local-worker abstraction + per-row enable.
- Piece 3: PR #87 / v0.34.0 — per-step routing + remote dispatch (built-ins only).
- Piece 4: PR #89 — plugin push to workers.
- This piece (Piece 5): plugin steps remote-eligible.
- Piece 6: failure handling + reassignment.

## Locked-in decisions (from brainstorming)

1. **Per-worker accurate dispatcher filter** — workers re-register
   after each `plugin_sync::sync` so the coordinator's view of "this
   worker can run kind X" stays current. Reuses the existing `Register`
   wire envelope; no new protocol variant.
2. **Per-step `[steps."<name>"] executor` map in `manifest.toml`** —
   per-step granularity. A plugin can mark `whisper.transcribe`
   remote-eligible while keeping `whisper.detect_language`
   coordinator-only. Backwards-compatible: missing `[steps]` block →
   default coordinator-only for every step.
3. **Worker daemon queries the registry for `available_steps`** —
   dynamic enumeration via a new `registry::list_step_names()`. Single
   source of truth; works correctly across both initial register AND
   post-PluginSync re-register without drift.

## Architecture

### Manifest schema

`manifest.toml` gains an optional `[steps."<step_kind>"]` table.
Inside, an optional `executor` field defaults to `"coordinator-only"`.

```toml
name = "whisper"
version = "1.0"
kind = "subprocess"
entrypoint = "bin/run"
provides_steps = ["whisper.transcribe", "whisper.detect_language"]

[steps."whisper.transcribe"]
executor = "any-worker"

# whisper.detect_language defaults to coordinator-only — no entry needed.
```

Rust types in `crates/transcoderr/src/plugins/manifest.rs`:

```rust
pub struct Manifest {
    // existing fields unchanged…
    #[serde(default)]
    pub steps: BTreeMap<String, StepManifest>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct StepManifest {
    #[serde(default)]
    pub executor: Option<ManifestExecutor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ManifestExecutor {
    AnyWorker,
    CoordinatorOnly,
}
```

`ManifestExecutor::AnyWorker` maps to `crate::steps::Executor::Any`;
`ManifestExecutor::CoordinatorOnly` and missing entries map to
`Executor::CoordinatorOnly`.

### `SubprocessStep` extension

`SubprocessStep` gains an `executor: Executor` field, threaded from the
manifest at registry-build time. The build path in
`crates/transcoderr/src/steps/registry.rs`:

```rust
for step_name in &d.manifest.provides_steps {
    let executor = d.manifest.steps.get(step_name)
        .and_then(|s| s.executor)
        .map(|e| match e {
            ManifestExecutor::AnyWorker => Executor::Any,
            ManifestExecutor::CoordinatorOnly => Executor::CoordinatorOnly,
        })
        .unwrap_or(Executor::CoordinatorOnly);
    let step = SubprocessStep {
        step_name: step_name.clone(),
        entrypoint_abs: abs.clone(),
        executor,
    };
    reg.by_name.insert(step_name.clone(), Arc::new(step));
}
```

`SubprocessStep::executor()` returns `self.executor`.

### `registry::list_step_names`

New helper:

```rust
pub async fn list_step_names() -> Vec<String> {
    let Some(rw) = REGISTRY.get() else { return Vec::new() };
    rw.read().await.by_name.keys().cloned().collect()
}
```

### Per-worker `available_steps` in `Connections`

In-memory map; **no DB migration**. Workers always re-register on
reconnect, so coordinator-restart self-heals.

```rust
pub struct Connections {
    senders: Arc<RwLock<HashMap<i64, mpsc::Sender<Envelope>>>>,
    inbox:   Arc<RwLock<HashMap<String, mpsc::Sender<InboundStepEvent>>>>,
    available_steps: Arc<RwLock<HashMap<i64, Vec<String>>>>,  // NEW
}

impl Connections {
    pub async fn record_available_steps(
        &self, worker_id: i64, steps: Vec<String>
    ) {
        self.available_steps.write().await.insert(worker_id, steps);
    }

    pub async fn worker_has_step(&self, worker_id: i64, step_kind: &str) -> bool {
        self.available_steps.read().await
            .get(&worker_id)
            .map(|v| v.iter().any(|s| s == step_kind))
            .unwrap_or(false)
    }
}
```

`SenderGuard::drop` is extended to also clear the per-worker
`available_steps` entry — RAII consistency, so a panicked WS task
leaves no stale dispatch targets.

### Worker daemon — dynamic available_steps

The worker daemon's existing hardcoded `available_steps` list goes
away. Instead the Register envelope is built at call time via:

```rust
pub async fn build_register_envelope(
    name: &str,
    hw_caps: serde_json::Value,
    plugins_dir: &Path,
) -> Envelope {
    let plugin_manifest: Vec<PluginManifestEntry> =
        match crate::plugins::discover(plugins_dir) {
            Ok(found) => found.into_iter().map(|d| PluginManifestEntry {
                name: d.manifest.name.clone(),
                version: d.manifest.version.clone(),
                sha256: None,
            }).collect(),
            Err(_) => Vec::new(),
        };
    let available_steps = crate::steps::registry::list_step_names().await;
    Envelope {
        id: format!("reg-{}", uuid::Uuid::new_v4()),
        message: Message::Register(Register {
            name: name.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            hw_caps,
            available_steps,
            plugin_manifest,
        }),
    }
}
```

The existing `build_register: F` closure in `connection.rs` is dropped
— it captured state once at boot, which doesn't work when plugins
come and go. `ConnectionContext` (added in Piece 4) is extended:

```rust
#[derive(Clone)]
pub struct ConnectionContext {
    pub plugins_dir: PathBuf,
    pub coordinator_token: String,
    pub name: String,                  // NEW
    pub hw_caps: serde_json::Value,    // NEW
}
```

The daemon builds `ConnectionContext` once at boot from `WorkerConfig +
ffmpeg_caps probe` and passes it into `connection::run`. The
connection's pre-handshake build call AND its post-sync re-register
both call `build_register_envelope(&ctx)`.

### Worker re-register after PluginSync

Piece 4 added a per-connection sync worker task that drains the
single-slot manifest queue. After `plugin_sync::sync` returns, the task
sends a fresh Register envelope:

```rust
let sync_task = tokio::spawn(async move {
    loop {
        notify.notified().await;
        let manifest = slot.lock().await.take();
        if let Some(m) = manifest {
            plugin_sync::sync(&plugins_dir, m, &token).await;
            // NEW: re-register so coordinator sees fresh available_steps.
            let env = build_register_envelope(&name, hw_caps.clone(), &plugins_dir).await;
            let _ = outbound_tx.send(env).await;
        }
    }
});
```

Sync runs on a separate task, so heartbeats keep flowing. Re-register
is a single send through the existing outbound mpsc — no new wire
machinery.

### Coordinator-side: handle Register in receive loop

The initial register handshake at `api/workers.rs::handle_connection`
is unchanged — pre-handshake awaits the FIRST register inline and
sends `register_ack`. After the initial register, subsequent Register
envelopes arrive in the receive loop. New arm:

```rust
Message::Register(r) => {
    let hw_caps_json = serde_json::to_string(&r.hw_caps)
        .unwrap_or_else(|_| "null".into());
    let plugin_manifest_json = serde_json::to_string(&r.plugin_manifest)
        .unwrap_or_else(|_| "[]".into());
    if let Err(e) = db::workers::record_register(
        &state.pool, worker_id, &hw_caps_json, &plugin_manifest_json
    ).await {
        tracing::warn!(worker_id, error = ?e, "failed to update register state");
    }
    state.connections.record_available_steps(worker_id, r.available_steps).await;
    // No register_ack response — would oscillate. Re-register is fire-and-forget.
}
```

The initial register-ack code path also calls
`record_available_steps(worker_id, payload.available_steps)` so the
in-memory map is populated from boot.

### Dispatcher per-worker filter

`dispatch::eligible_remotes` (currently has `_step_kind: &str` ignored
per Piece 3's `dispatch/mod.rs:84`) starts using the parameter:

```rust
async fn eligible_remotes(step_kind: &str, state: &AppState) -> anyhow::Result<Vec<i64>> {
    let cutoff = chrono::Utc::now().timestamp() - STALE_AFTER_SECS;
    let rows = db::workers::list_all(&state.pool).await?;
    let mut out = Vec::new();
    for r in rows {
        if r.id == LOCAL_WORKER_ID { continue; }
        if r.enabled == 0 { continue; }
        match r.last_seen_at { Some(seen) if seen > cutoff => {} _ => continue }
        if !state.connections.is_connected(r.id).await { continue; }
        if !state.connections.worker_has_step(r.id, step_kind).await { continue; }
        out.push(r.id);
    }
    Ok(out)
}
```

For the 7 built-in remote-eligible step kinds, every worker advertises
them in their initial Register, so the new filter is a no-op for the
existing remote_dispatch suite. For plugin step kinds, the filter
correctly skips workers that haven't installed (or failed to install)
the plugin.

## File structure

**Modified backend files:**
- `crates/transcoderr/src/plugins/manifest.rs` — `Manifest.steps`
  field, `StepManifest`, `ManifestExecutor` enum.
- `crates/transcoderr/src/plugins/subprocess.rs` —
  `SubprocessStep.executor: Executor`; `executor()` returns it.
- `crates/transcoderr/src/steps/registry.rs` — build path threads
  executor; new `list_step_names()` helper.
- `crates/transcoderr/src/worker/connections.rs` — new
  `available_steps: HashMap<i64, Vec<String>>` field; new
  `record_available_steps` + `worker_has_step` methods; SenderGuard
  cleanup extended.
- `crates/transcoderr/src/worker/daemon.rs` — drop hardcoded
  available_steps; build `ConnectionContext` with name + hw_caps.
- `crates/transcoderr/src/worker/connection.rs` — extend
  `ConnectionContext`; replace `build_register: F` closure pattern with
  `build_register_envelope(&ctx).await` calls; sync worker task fires
  re-register after sync.
- `crates/transcoderr/src/api/workers.rs::handle_connection` — initial
  register also calls `record_available_steps`; receive loop gains
  `Message::Register` arm for subsequent updates.
- `crates/transcoderr/src/dispatch/mod.rs` — `eligible_remotes` filters
  by `step_kind` via `connections.worker_has_step`.

**New backend files:**
- `crates/transcoderr/tests/plugin_remote_dispatch.rs` — 5-scenario
  integration suite.

**No new wire-protocol variants** — re-uses existing `Message::Register`.
**No DB migration** — per-worker available_steps lives in
`Connections` (in-memory).

## Wire / API summary

| Endpoint / Envelope | Direction | Purpose | Status |
|---|---|---|---|
| `Register` | worker → coordinator | Initial register handshake | unchanged |
| `Register` | worker → coordinator | **Re-register** after each plugin sync | NEW use of existing variant |
| (no register_ack on re-register) | — | Avoid oscillation loop | NEW |

## Database

No schema migration. Per-worker available_steps lives in
`Connections.available_steps` (in-memory `HashMap<i64, Vec<String>>`).
Workers always re-send Register on reconnect, so a coordinator restart
self-heals as workers come back via Piece 1's reconnect-with-backoff.

## Manifest backward compatibility

Existing manifests without `[steps]` blocks: `steps` is
`#[serde(default)]` → empty BTreeMap. Every step kind looked up via
`d.manifest.steps.get(step_name)` returns None → default
`Executor::CoordinatorOnly`. **Zero breaking change.** size-report
(currently the only published plugin) keeps running coordinator-only
unless its manifest is updated.

## Failure scenarios

| Scenario | Behavior |
|---|---|
| Worker connects, no plugins yet | initial Register's `available_steps` = built-in step kinds; in-memory map populated |
| PluginSync arrives, sync succeeds, registry rebuild adds plugin's step kinds | sync worker task re-registers; coordinator updates `connections.available_steps`; dispatcher routes new step kinds to this worker |
| PluginSync arrives, plugin install fails (best-effort skip from Piece 4) | registry doesn't have the step → `list_step_names` doesn't include it → re-register doesn't advertise it → dispatcher correctly skips this worker |
| Worker disconnects | `SenderGuard::drop` clears `available_steps` entry; dispatcher won't pick this worker |
| Coordinator restarts | in-memory map empty; workers reconnect via Piece 1's reconnect loop and re-send Register; map repopulates |
| Plugin manifest with no `[steps]` block | every step kind defaults to CoordinatorOnly — backwards-compatible |
| `[steps."xxx"] executor = "any-worker"` for an unknown step kind | manifest deserialise succeeds (BTreeMap takes any string); registry never sees it; effectively a no-op |
| Two workers, plugin install fails on one | dispatcher routes the step kind to whichever worker DID install it; if both failed, falls back to local |

## Testing

### Unit tests

- `plugins::manifest` — deserialize a manifest with
  `[steps."whisper.transcribe"] executor = "any-worker"`; assert the
  field maps correctly. Deserialize a manifest WITHOUT a `[steps]`
  block; assert `steps` is empty.
- `plugins::subprocess::SubprocessStep::executor()` — built from a
  manifest with explicit `Any` returns `Any`; built from a manifest
  without an entry returns `CoordinatorOnly`.
- `steps::registry::list_step_names` — returns built-in names + any
  plugin-provided names after init/rebuild; returns empty when
  registry is uninitialized.
- `worker::connections::{record_available_steps, worker_has_step}` —
  round-trip; `SenderGuard::drop` clears the entry; missing entries
  return false from `worker_has_step`.
- `dispatch::eligible_remotes` — when the only connected worker
  doesn't advertise the step kind, returns empty (fall back to
  local).

### Integration tests (`crates/transcoderr/tests/plugin_remote_dispatch.rs`)

Reuses `common::boot()` + the fake-worker harness from
`tests/remote_dispatch.rs` (Piece 3) and `tests/plugin_push.rs`
(Piece 4).

1. **`plugin_step_routes_to_worker_that_has_it`** — register a fake
   worker with `available_steps: ["whisper.transcribe"]`; submit a
   flow with a `whisper.transcribe` step that has `executor =
   "any-worker"` (mock the registry to inject this); assert the
   worker receives `step_dispatch`.
2. **`plugin_step_skips_worker_without_it`** — register a fake worker
   with `available_steps: ["transcode"]` only; submit a flow with a
   `whisper.transcribe` step; assert no `step_dispatch` arrives at
   the fake worker (dispatcher falls back to local).
3. **`coordinator_only_plugin_step_runs_locally`** — plugin step with
   no `[steps]` block in its manifest defaults to coordinator-only;
   even with a fake worker that advertises every step kind, the
   dispatcher routes locally.
4. **`re_register_updates_available_steps`** — fake worker sends an
   initial Register with `["transcode"]`; submit a `whisper.transcribe`
   step → no dispatch (correct, worker doesn't have it); fake worker
   sends a SECOND Register with `["transcode", "whisper.transcribe"]`;
   submit another step → dispatch arrives (correct).
5. **`disconnect_clears_available_steps_for_dispatch`** — fake worker
   advertises `["whisper.transcribe"]`; disconnects; submit a
   `whisper.transcribe` step → no dispatch (worker has been removed
   from the eligible set).

### Existing tests must stay green

- `worker_connect` (4) — initial register flow unchanged.
- `local_worker` (4) — no plugin work.
- `remote_dispatch` (5) — fake worker registers with
  `available_steps: ["transcode"]`, dispatcher's new step-kind filter
  correctly identifies it as eligible for `transcode` step kinds.
- `plugin_push` (6) — install/sync paths still work; re-register adds
  one more outbound Register frame after sync, which the test should
  tolerate (assertions don't count outbound frames).
- `api_auth` (7), critical-path tests, full lib suite.

## Risks

- **Existing `remote_dispatch.rs` tests use `available_steps:
  ["transcode"]`.** That's still valid for built-in step kinds — the
  filter passes. Verify with `cargo test --test remote_dispatch`
  before/after the dispatch filter change.
- **`build_register_envelope` async signature.** The previous
  `build_register: Fn() -> Envelope` was sync. The migration to async
  ripples through `connect_once`'s call sites (initial register pre-
  handshake AND sync worker task). Both already run in async contexts
  so the conversion is mechanical.
- **Race between PluginSync sync completion and dispatch decision.**
  If a step is dispatched at exactly the moment the worker's sync is
  finishing, the coordinator's `connections.available_steps` may be
  briefly stale. Worst case: dispatch picks a worker that's still
  installing → `step_dispatch` lands → worker's `registry::resolve`
  returns Some (the new SubprocessStep) → step runs. So even the
  "stale" path actually succeeds. Eventual consistency is fine.
- **Plugin install failure on every worker.** If 0/N workers
  successfully install a plugin, `eligible_remotes` returns empty →
  dispatcher falls back to local. The coordinator already has the
  plugin locally (it pushed the manifest), so local execution works.
  Self-healing.
- **Re-register collision with disconnect.** If a worker re-registers
  immediately before disconnecting, the coordinator briefly sees the
  fresh available_steps before SenderGuard cleanup. Dispatch picks
  it, send fails (channel closed), error propagates. Engine retries
  pick another worker. Acceptable; matches Piece 3 behavior.

## Out of scope

- **Failure handling + reassignment** — Piece 6.
- **Per-worker plugin observability UI** (which workers have which
  plugins, install error surfaces) — future polish.
- **Hot-reload of executor preferences without re-install** — an
  executor change is a manifest change is a version bump is a re-
  install; no special path needed.
- **`executor = "specific-worker:<name>"` targeting** — not requested;
  YAGNI.
- **Heavy-plugin-aware scheduling** (e.g. dispatch transcribe to GPU
  workers preferentially) — Piece 6 or later when hw_caps shape is
  real.
- **Multi-step plugin manifests with mixed runtimes** — beyond
  Piece 5's per-step executor.
- **Hot reload of plugin steps without daemon restart** — the
  registry's `rebuild_from_discovered` already handles this; Piece 4's
  PluginSync triggers it; no extra Piece 5 work.

## Success criteria

1. A `whisper` plugin manifest with `[steps."whisper.transcribe"]
   executor = "any-worker"` is installed via the existing UI flow.
2. Connected workers receive the `PluginSync`, install whisper,
   rebuild registry, re-register with updated available_steps
   including `whisper.transcribe`.
3. Coordinator's `connections.available_steps[worker_id]` contains
   `whisper.transcribe` after re-register lands.
4. A flow with a `whisper.transcribe` step dispatches to a remote
   worker; the subprocess runs on the worker host; output (e.g.
   transcript file) lands at the shared FS path.
5. The run-detail timeline shows `[worker: gpu-box-1]` against the
   `whisper.transcribe` event.
6. A second plugin step `whisper.detect_language` (no `[steps]` block)
   runs locally on the coordinator even with workers connected.
7. All existing integration tests stay green: `worker_connect` (4),
   `local_worker` (4), `remote_dispatch` (5), `plugin_push` (6),
   `api_auth` (7), critical-path tests, full lib suite.

## Branch / PR

Branch: `feat/distributed-piece-5` from main. Spec branch is
`spec/distributed-piece-5` (this file). Single PR per piece, matching
the Piece 1/2/3/4 pattern.
