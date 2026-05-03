# Distributed Transcoding — Piece 4: Plugin Push to Workers

## Goal

Coordinator pushes plugin tarballs to workers so that connected workers
mirror the coordinator's intended plugin set. After this piece, a
worker's `./plugins/` directory is a strict mirror of the coordinator's
`db::plugins WHERE enabled=1` — installed via the same atomic-swap
pipeline the coordinator uses, served from a coordinator-side tarball
cache, kept in sync via WS broadcasts on coordinator-side changes.

This piece does **not** wire plugin steps into the dispatcher's remote
path — that's Piece 5. After Piece 4, plugins live on workers but the
dispatcher still routes only the 7 built-in remote-eligible steps.

## Roadmap context

- Roadmap parent: `docs/superpowers/specs/2026-05-02-distributed-transcoding-design.md` (PR #81 merged).
- Piece 1: PR #83 / v0.32.0 — wire protocol skeleton + worker daemon.
- Piece 2: PR #85 / v0.33.0 — local-worker abstraction + per-row enable.
- Piece 3: PR #87 / v0.34.0 — per-step routing + remote dispatch (built-ins only).
- This piece (Piece 4): plugin push.
- Piece 5: plugin steps remote-eligible.
- Piece 6: failure handling + reassignment.

## Locked-in decisions (from brainstorming)

1. **Sync semantics: full mirror.** The worker installs missing
   plugins AND uninstalls anything the coordinator's manifest doesn't
   include. The worker's `./plugins/` directory is a strict mirror of
   the coordinator's `db::plugins WHERE enabled=1`.
2. **Failure handling: best-effort skip + log + continue.** A single
   plugin failing to download / verify / install logs a warning and
   does not abort the sync. Worker registration succeeds regardless.
   The plugin simply isn't on that worker; flows that need it will
   fail at dispatch time (or fall back to local since the worker
   doesn't advertise the step kind).
3. **Live deltas: broadcast full manifest.** When the operator
   installs/uninstalls/toggles a plugin on the coordinator, the
   coordinator pushes a `PluginSync` envelope carrying the **complete**
   intended manifest to every connected worker. Workers re-run the
   same full-sync logic they ran on `register_ack`. One code path
   covers boot + delta.
4. **Tarball cache location: `<data_dir>/plugins/.tarball-cache/<name>-<sha256>.tar.gz`.**
   Same volume as live plugin directories; dot-prefix means the
   existing `discover()` already skips it (mirrors `.tcr-install.*`).
   Sha256 suffix means content-addressable filenames.

## Architecture

### Coordinator side

**Tarball cache.** During plugin install (`api/plugins.rs::install`
calling `installer::install_from_entry`), the verified source tarball
is moved into `<data_dir>/plugins/.tarball-cache/<name>-<sha256>.tar.gz`
before the staging dir is wiped. On uninstall, the cached file is
deleted.

**Tarball serve endpoint.** `GET /api/worker/plugins/:name/tarball`
lives in the **public** router (auth happens inside the handler).
The handler:
1. Extracts `Authorization: Bearer <token>` from the request.
2. Looks up the worker via `db::workers::get_by_token`. None → 401.
3. Reads `db::plugins` row by name. Missing or disabled → 404.
4. Opens
   `<data_dir>/plugins/.tarball-cache/<name>-<row.tarball_sha256>.tar.gz`.
   Missing → 404.
5. Streams the file with `Content-Type: application/x-gzip`.

The endpoint mirrors the Bearer-on-Request pattern from Piece 1's
`/api/worker/connect`.

**`register_ack` manifest.** The existing
`RegisterAck { worker_id, plugin_install: Vec<PluginInstall> }`
(currently always `vec![]`) is populated at register time:

```rust
let plugins = db::plugins::list_enabled(&pool).await?;
let manifest = plugins.into_iter().map(|p| PluginInstall {
    name: p.name,
    version: p.version,
    sha256: p.tarball_sha256,
    tarball_url: format!("{public_url}/api/worker/plugins/{}/tarball", p.name),
}).collect();
```

`public_url` is already on `AppState` (used elsewhere for *arr
notifications etc.).

**Live broadcast on plugin changes.** After `api/plugins.rs::install`,
`api/plugins.rs::uninstall`, and the enable/disable handler, build the
same manifest and broadcast a new `PluginSync` envelope to all
connected workers via the existing `Connections` registry.

`Connections` gains a small helper:
```rust
pub async fn broadcast_plugin_sync(&self, manifest: Vec<PluginInstall>) {
    let env = Envelope {
        id: format!("psync-{}", uuid::Uuid::new_v4()),
        message: Message::PluginSync(PluginSync { plugins: manifest }),
    };
    let map = self.senders.read().await;
    for tx in map.values() {
        let _ = tx.send(env.clone()).await;
    }
}
```

### Worker side

**Sync logic** (`worker/plugin_sync.rs`):
1. `discover(plugins_dir)` → installed plugins, each carrying its
   `manifest.toml`'s name + version + (separately) the
   `tarball_sha256` we recorded on install.
2. **Diff:**
   - `to_install` = manifest entries whose name isn't installed
     locally OR whose locally-installed sha256 doesn't match.
   - `to_remove` = installed plugins whose name isn't in the
     incoming manifest at all.
3. For each `to_remove`: `uninstaller::uninstall(plugins_dir, &name)`.
   This also clears the cache entry on the coordinator side; on the
   worker side it just removes the plugin directory.
4. For each `to_install`: build an `IndexEntry` from the
   `PluginInstall` payload; call
   `installer::install_from_entry(entry, plugins_dir,
   archive_to=None, auth_token=Some(coordinator_token))`. The token
   is the worker's `coordinator_token` from `worker.toml`.
5. After the loop, `registry::rebuild_from_discovered(...)` so the
   worker's runtime registry reflects the new plugins. (The registry
   is the same global one Piece 3 already wires; this just refreshes
   it.)

Per Q2 (failure handling): every step in 3 and 4 is wrapped in a
`tracing::warn!` arm so a single failure doesn't abort the loop.

**Sync trigger points:**
- **Initial:** the worker daemon's `connect_once` already awaits
  `register_ack`. After the existing logging line, if
  `ack.plugin_install` is non-empty, spawn `plugin_sync::sync(...)`.
- **Live:** the worker's receive loop already dispatches `step_dispatch`
  envelopes (Piece 3). Add a branch for `Message::PluginSync(p)` that
  spawns the same `plugin_sync::sync(...)` with `p.plugins`.

Both paths spawn a tokio task so the WS receive loop stays
responsive — heartbeats keep flowing during a long sync. **Per-worker
serialisation:** if a `PluginSync` arrives while a previous sync is
running, the new one queues. Implementation: a single-element tokio
channel + a worker task that drains it. Newer messages overwrite
older ones (we only ever care about the latest manifest).

### Wire protocol additions

One new variant on `Message`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", tag = "type", content = "payload")]
pub enum Message {
    // existing
    Register(Register),
    RegisterAck(RegisterAck),
    Heartbeat(Heartbeat),
    StepDispatch(StepDispatch),
    StepProgress(StepProgressMsg),
    StepComplete(StepComplete),
    // NEW (Piece 4)
    PluginSync(PluginSync),
}

pub struct PluginSync {
    pub plugins: Vec<PluginInstall>,  // full intended manifest
}
```

`PluginInstall { name, version, sha256, tarball_url }` already exists
from Piece 1. The `tarball_url` field is now populated; previously it
was constructed for the (always-empty) `register_ack.plugin_install`
list.

### Installer / uninstaller modifications

**`installer::install_from_entry`** gains two optional parameters:

```rust
pub async fn install_from_entry(
    entry: &IndexEntry,
    plugins_dir: &Path,
    archive_to: Option<&Path>,        // NEW
    auth_token: Option<&str>,         // NEW
) -> Result<InstalledPlugin, InstallError>;
```

- `archive_to: Some(path)` — after sha256 verification, the verified
  tarball is **moved** (rename, same volume) to that path. Coordinator
  passes `Some(<data_dir>/plugins/.tarball-cache/<name>-<sha>.tar.gz)`;
  worker passes `None`.
- `auth_token: Some(t)` — the reqwest GET attaches
  `Authorization: Bearer <t>`. Worker passes `Some(coordinator_token)`;
  coordinator passes `None` (its catalog fetches don't authenticate
  to itself).

**`uninstaller::uninstall`** gains best-effort cache cleanup:
after removing the plugin directory, glob-delete
`<plugins_dir>/.tarball-cache/<name>-*.tar.gz`. Best-effort because
the worker's plugins_dir doesn't have a cache (worker passed
`archive_to=None`); the coordinator's does.

## File structure

**New backend files:**
- `crates/transcoderr/src/api/worker_plugins.rs` — tarball serve handler.
- `crates/transcoderr/src/worker/plugin_sync.rs` — worker-side sync
  logic + diff helper + unit tests.

**Modified backend files:**
- `crates/transcoderr/src/plugins/installer.rs` — `install_from_entry`
  gains `archive_to` + `auth_token` parameters.
- `crates/transcoderr/src/plugins/uninstaller.rs` — best-effort
  glob-clear of the tarball cache file matching the uninstalled name.
- `crates/transcoderr/src/api/plugins.rs` — pass cache path to
  `install_from_entry`; broadcast `PluginSync` after install /
  uninstall / enable-toggle changes.
- `crates/transcoderr/src/api/workers.rs` — populate
  `register_ack.plugin_install` with the real manifest (currently
  `vec![]`).
- `crates/transcoderr/src/api/mod.rs` — register
  `/api/worker/plugins/:name/tarball` in the **public** router (auth
  inside the handler).
- `crates/transcoderr/src/worker/connections.rs` — add
  `broadcast_plugin_sync(manifest)` helper.
- `crates/transcoderr/src/worker/protocol.rs` — `PluginSync` variant +
  struct + round-trip test.
- `crates/transcoderr/src/worker/connection.rs` — receive loop
  handles `Message::PluginSync`. After register_ack, also trigger
  initial sync if `ack.plugin_install` is non-empty.
- `crates/transcoderr/src/worker/daemon.rs` — pass
  `coordinator_token` + `plugins_dir` (the existing `./plugins`) into
  the connection.
- `crates/transcoderr/src/db/plugins.rs` — add a `list_enabled(pool)`
  helper if it doesn't exist yet (used by both register_ack
  population + broadcast).

## Wire / API summary

| Endpoint / Envelope | Direction | Purpose |
|---|---|---|
| `GET /api/worker/plugins/:name/tarball` | worker → coordinator | Fetch verified tarball; Bearer-on-Request |
| `register_ack.plugin_install` | coordinator → worker | Initial intended manifest |
| `PluginSync` | coordinator → worker | Live full-manifest re-sync trigger |

## Database

No schema migration. `db::plugins.tarball_sha256` was added in PR #55
(plugin catalog) — already populated for every installed plugin.

## Coordinator tarball cache lifecycle

```
operator installs plugin "X" version 1
  → installer fetches X-v1.tar.gz from catalog
  → verifies sha256
  → moves tarball to .tarball-cache/X-<sha_v1>.tar.gz
  → extracts + atomic swap to .../X/
  → broadcasts PluginSync to all workers
operator upgrades plugin "X" to version 2
  → installer fetches X-v2.tar.gz from catalog
  → verifies sha256
  → moves to .tarball-cache/X-<sha_v2>.tar.gz
  → atomic swap replaces .../X/ contents
  → uninstaller-style cleanup removes the OLD .tarball-cache/X-<sha_v1>.tar.gz
    (because sync_discovered's ON CONFLICT path triggers tarball cache pruning)
operator uninstalls plugin "X"
  → uninstaller removes .../X/
  → uninstaller globs and deletes .tarball-cache/X-*.tar.gz
  → broadcasts PluginSync (no longer mentions X)
```

## Error handling

| Scenario | Behavior |
|---|---|
| Worker tarball fetch: 401 / 404 / network error | log warn, skip plugin, continue sync |
| Worker tarball: sha256 mismatch | log warn, leave staging cleaned up, skip |
| Worker uninstall fails (e.g. file in use) | log warn, leave plugin in place, continue |
| Coordinator cache file missing for a manifest entry | endpoint returns 404; worker treats as fetch failure |
| `registry::rebuild_from_discovered` fails | log error; flow runs that need new steps fail until next sync |
| `PluginSync` arrives while previous sync running | latest manifest queued (single-slot); previous completes, then queued runs |
| Worker disconnects mid-sync | next register_ack triggers a fresh full sync |
| Two operators install/uninstall concurrently | DB is source of truth; whichever transaction commits last determines the manifest broadcast happens after the commit |

## Testing

### Unit tests

- `worker/protocol.rs` — `PluginSync` JSON round-trip + lock the wire
  tag (`"type":"plugin_sync"`).
- `worker/plugin_sync.rs::compute_diff` — exhaustive cases:
  - empty installed + empty manifest → empty diff
  - empty installed + manifest with one entry → 1 install, 0 remove
  - installed has X@sha1, manifest has X@sha1 → empty diff (no-op)
  - installed has X@sha1, manifest has X@sha2 → 1 install (replace),
    0 remove (the install path will atomic-swap the new version)
  - installed has X, manifest has Y → 1 install, 1 remove
- `plugins/installer.rs` — `install_from_entry` with
  `archive_to=Some(path)` writes the verified tarball there;
  `auth_token=Some(t)` causes the GET to include the Authorization
  header. (Use a `httpmock` fixture or wiremock if already a dep;
  otherwise a manual hyper test server — read existing installer
  tests for the canonical pattern.)

### Integration tests (`crates/transcoderr/tests/plugin_push.rs`)

Reuses the existing `common::boot()` test fixture.

1. **`tarball_endpoint_serves_cached_file`** — install a plugin via
   the test fixture (use the existing `tests/plugin_install_e2e.rs`
   helpers as a reference for how to seed an install). Curl
   `/api/worker/plugins/<name>/tarball` with the worker's bearer.
   Assert the bytes' sha256 matches `db::plugins.tarball_sha256`.
2. **`tarball_endpoint_rejects_missing_token`** — same fixture, no
   Authorization header → 401.
3. **`tarball_endpoint_404_for_unknown_plugin`** — valid bearer, but
   `:name` doesn't exist in `db::plugins` → 404.
4. **`register_ack_carries_plugin_manifest`** — install a plugin on
   the coordinator. Connect a fake worker via the existing
   `tests/worker_connect.rs`-style harness. Assert the received
   `register_ack` envelope's `plugin_install` is non-empty and lists
   the installed plugin with a non-empty `tarball_url`.
5. **`plugin_install_broadcasts_plugin_sync`** — connect a fake
   worker first, *then* install a plugin via the UI API. Assert the
   worker receives a `PluginSync` envelope within 1s.
6. **`plugin_uninstall_broadcasts_plugin_sync_without_it`** — connect
   worker, install plugin, then uninstall via API. Assert the worker
   receives a second `PluginSync` whose `plugins` list omits the
   uninstalled name.

### Existing tests must stay green

`worker_connect` (4), `local_worker` (4), `remote_dispatch` (5),
`api_auth` (7), `concurrent_claim`, `crash_recovery`, `flow_engine`,
`plugin_install_e2e`, the full lib suite. The installer signature
change ripples into the existing coordinator-side install handler;
the additive parameters (`archive_to`, `auth_token`) default-None
behavior is byte-identical to today.

## Risks

- **`discover()` walking into the cache dir** — mitigated by the
  dot-prefix (matches existing `.tcr-install.*` exclusion). Add a
  unit test that creates `.tarball-cache/foo.tar.gz` next to a
  legitimate plugin dir and asserts `discover()` doesn't surface it.
- **Stale cache after version bump** — when the install handler
  upgrades a plugin (same name, new sha), the OLD
  `.tarball-cache/<name>-<old_sha>.tar.gz` would otherwise remain.
  The install handler queries `db::plugins.tarball_sha256` BEFORE
  the install runs, captures the old sha, and after the new install
  succeeds, deletes the old cache file (best-effort). The
  uninstaller doesn't see version-bump events, so this cleanup
  belongs in the install handler.
- **Per-connection serialization vs heartbeat** — sync runs on a
  spawned task, not in the receive loop, so heartbeats keep flowing.
  Sync errors don't propagate to the connection.
- **Tarball cache disk usage** — coordinator now stores both extracted
  plugin dir AND its source tarball. For the size-report plugin
  that's ~10KB extra; for a future whisper plugin it could be 100MB+.
  Acceptable for Piece 4; if growth becomes a problem, a future
  garbage-collector can remove tarballs older than N versions.
- **Worker installs from a self-signed coordinator** — the worker's
  `reqwest` client uses the OS root cert store (configured in Piece
  1). Operators with self-signed coordinator TLS need to install
  their cert on the worker host. Documented; not changed here.
- **Concurrent operator install + uninstall on the same plugin name**
  — the DB UPDATE serialises; whoever commits last wins. The
  broadcast happens after the commit so worker state converges to
  the post-last-write manifest. Acceptable.

## Out of scope

- **Plugin steps remote-eligible** — Piece 5 (subprocess plugins gain
  `executor = "any-worker"` in their manifest; routing rules respect
  it).
- **Per-worker plugin observability** (which workers have which
  plugins; per-worker install status; per-worker failure surfaces) —
  future polish.
- **Streaming tarball download / upload** — the existing installer
  buffers full body. KB-MB scale plugins fine; a streaming impl is a
  follow-up if a multi-GB plugin ever ships.
- **Plugin signature trust** beyond sha256 — sha256 from the catalog
  is the trust root; coordinator signs nothing additional.
- **Worker-driven re-sync** (e.g. worker requests a manifest refresh) —
  not needed; coordinator-pushed updates are sufficient for Piece 4.
- **Garbage-collecting old tarball-cache entries** — out of scope;
  uninstall already cleans the deleted plugin's cache.

## Success criteria

1. Coordinator installs a plugin via the existing UI flow; the
   plugin's tarball lands at
   `<data_dir>/plugins/.tarball-cache/<name>-<sha>.tar.gz`.
2. A connected worker receives a `PluginSync` envelope within 1s of
   the install; the plugin appears in `<worker>/./plugins/<name>/`
   shortly after.
3. Worker's `registry::resolve(plugin_step_name)` returns the new
   plugin's `Step` impl after the sync completes.
4. Operator uninstalls the plugin; worker receives a second
   `PluginSync` (manifest now empty); plugin directory removed from
   worker; coordinator's `<sha>.tar.gz` cache file removed.
5. Worker connecting cold (no plugins yet) receives a non-empty
   `register_ack.plugin_install` and ends up with the full set
   installed.
6. A bad plugin entry (e.g. coordinator's cache file missing) does
   not abort sync of other plugins.
7. All existing integration tests stay green: `worker_connect` (4),
   `local_worker` (4), `remote_dispatch` (5), `api_auth` (7),
   critical-path tests, full lib suite.

## Branch / PR

Branch: `feat/distributed-piece-4` from main. Spec branch is
`spec/distributed-piece-4` (this file). Single PR per piece, matching
the Piece 1/2/3 pattern.
