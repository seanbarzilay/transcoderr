# Worker Auto-Discovery — Design Spec

**Status:** approved (brainstorm 2026-05-03)
**Scope:** single-PR feature, additive to the existing manual enrollment flow.
**Predecessors:** Pieces 1–6 of the distributed-transcoding roadmap (issue #79), all merged.

## Problem

Adding a remote worker today is a four-step manual flow:

1. Operator clicks **Workers → Add worker** in the coordinator UI and enters a name.
2. Coordinator returns a one-time-displayed `secret_token`; the UI builds a `worker.toml` snippet on screen.
3. Operator copy-pastes the snippet into a `worker.toml` on the remote host.
4. Operator runs `transcoderr worker --config /etc/transcoderr/worker.toml` (typically inside Docker).

Three friction points: clicking the UI, copying secret material, dropping a file on the worker host. The roadmap's distributed-transcoding pieces all assumed this manual step; with the roadmap closed (Piece 6, v0.37.0), the remaining UX gap is the initial enrollment.

## Goal

A worker on the same LAN as the coordinator should be runnable with a single command and no pre-shared config:

```bash
docker run --rm --network=host \
  -v transcoderr-worker:/var/lib/transcoderr \
  ghcr.io/seanbarzilay/transcoderr:latest \
  transcoderr worker
```

The worker discovers the coordinator via mDNS, enrolls itself, persists the resulting token to its config volume, and proceeds through the existing connect handshake.

## Decisions (locked in brainstorm)

- **Q1-A — Topology: same-LAN only.** mDNS / DNS-SD only. WAN / cross-subnet / behind-VPN deployments keep using the existing manual flow. No pairing-code mode, no pre-shared bootstrap secret.
- **Q2-A — Trust model: open enrollment.** Any device that finds the coordinator via mDNS may enroll itself and receive a token. Trust = LAN access. This matches the implicit security model of every other LAN appliance the typical operator runs (Plex, HomeAssistant, Sonos). No operator confirmation step.
- **Q3-A — Persistence: write once, skip discovery thereafter.** First boot: discover → enroll → write `worker.toml`. Subsequent boots: read the file, skip discovery. If the cached token is rejected with `401`, delete the file and re-discover (handles coordinator DB wipe).

These three decisions cover the main design space; the remainder of this spec is implementation detail.

## Architecture

```
┌─────────────────────────────┐                ┌──────────────────────────────┐
│ Coordinator                 │                │ Worker (fresh, no config)    │
│                             │                │                              │
│  HTTP listener :8765        │                │   1. mdns_browse(...)        │
│  ↓                          │ ◄──────────────┤      service type            │
│  mDNS responder             │   _transcoderr │      _transcoderr._tcp.local │
│   advertises                │   ._tcp.local. │                              │
│   _transcoderr._tcp.local.  │                │   2. picks first responder   │
│   TXT enroll=/api/worker/   │                │   3. POST /api/worker/enroll │
│       enroll                │ ◄──────────────┤      body {name: "host-1"}   │
│   TXT ws=/api/worker/       │                │                              │
│       connect               │ ──────────────►│   ← {id, secret_token,       │
│                             │                │      ws_url}                 │
│  POST /api/worker/enroll    │                │                              │
│   (unauthenticated)         │                │   4. write /var/lib/         │
│   mints token, inserts row, │                │      transcoderr/worker.toml │
│   returns {id, token, ws}   │                │                              │
│                             │                │   5. existing connect path:  │
│                             │ ◄──────────────┤      WS upgrade with bearer  │
└─────────────────────────────┘   /api/worker/ └──────────────────────────────┘
                                  connect
```

The mDNS service is published *as well as*, not instead of, the existing UI flow. Operators on networks where multicast doesn't propagate (Docker default-bridge, corporate Wi-Fi, VPN-only) keep using the existing **Workers → Add worker** modal.

## Components

### Coordinator side (new)

- **`crates/transcoderr/src/discovery/mod.rs`** — wraps `mdns-sd::ServiceDaemon`. Exposes `start(port: u16) -> ServiceDaemon` called from `main.rs` after the HTTP listener binds (so we know the actual port, including the `:0` ephemeral case). The returned daemon is held for the process lifetime; `Drop` unregisters cleanly.
- **`crates/transcoderr/src/api/worker_enroll.rs`** — new `POST /api/worker/enroll` endpoint. Body: `{"name": String}`. Reuses `db::workers::insert_remote(pool, name, token)` to mint a token and insert a `kind='remote'` row. Returns:
  ```json
  {"id": 7, "secret_token": "abc…", "ws_url": "ws://192.168.1.50:8765/api/worker/connect"}
  ```
  **Unauthenticated.** Rate-limited to 10 enrollments / minute / source IP via the existing `tower_governor` middleware (already in the dep tree). The rate limit is a small safety net against accidental enrollment storms (e.g., a docker-compose `restart: always` with broken connect logic); it is **not** a security boundary — the trust model is LAN access.

### Worker side (new)

- **`crates/transcoderr/src/worker/discovery.rs`** — wraps `mdns-sd::ServiceDaemon::browse`. Returns the first matching `_transcoderr._tcp.local.` instance within a 5-second deadline. Returns `Ok(None)` on timeout (caller decides whether to exit or retry).
- **`crates/transcoderr/src/worker/enroll.rs`** — `enroll(coordinator_addr, port, name) -> EnrollResponse`. POSTs to the discovered enrollment endpoint and parses the response. On non-2xx, surfaces the response body in the error.
- **`crates/transcoderr/src/worker/daemon.rs` (extended)** — boot logic becomes:
  ```
  let cfg_path = args.config.unwrap_or(default_path());
  let cfg = match WorkerConfig::load(&cfg_path) {
      Ok(c) => c,
      Err(_) => discover_and_enroll(&cfg_path).await?, // writes the file
  };
  match connect_loop(cfg).await {
      ConnectError::Unauthorised if cached_was_used => {
          tracing::warn!("cached token rejected; re-running discovery");
          std::fs::remove_file(&cfg_path).ok();
          let cfg = discover_and_enroll(&cfg_path).await?;
          connect_loop(cfg).await
      }
      other => other,
  }
  ```
  The retry on `401` is bounded to one attempt to avoid loops.

### Default config path

- **`/var/lib/transcoderr/worker.toml`** — Docker convention. Documented as a persistent volume mount in `docs/deploy.md`. Override via the existing `--config` CLI flag is unchanged.
- The worker creates the parent directory if it doesn't exist (`fs::create_dir_all`).

## mDNS service definition

| Field | Value |
|---|---|
| Service type | `_transcoderr._tcp.local.` |
| Instance name | `<hostname>` (the coordinator's hostname) |
| Port | The coordinator's HTTP listener port |
| TXT `enroll` | `/api/worker/enroll` |
| TXT `ws` | `/api/worker/connect` |
| TXT `version` | `<CARGO_PKG_VERSION>` (informational) |

The TXT records are versioned by content rather than by a separate `txtvers` field; if a future server changes the path, workers reading `enroll=` and `ws=` directly will still work.

## Disable switch

`TRANSCODERR_DISCOVERY=disabled` (env var, parsed in `main.rs`) skips the mDNS advertisement. Coordinator continues to serve the manual flow normally. Default is enabled. The reverse — disabling discovery on the worker side — is unnecessary: the worker only browses when no `worker.toml` is found, and a manually-supplied file makes discovery moot.

## Data flow (first-run enrollment)

```
worker process starts
  → load /var/lib/transcoderr/worker.toml
  → ENOENT
  → discover() : mdns_browse("_transcoderr._tcp.local.", 5s)
     ← ServiceInfo { addr: 192.168.1.50, port: 8765,
                     txt: { enroll: "/api/worker/enroll",
                            ws: "/api/worker/connect",
                            version: "0.37.0" } }
  → enroll(addr, port, hostname()) : POST http://192.168.1.50:8765/api/worker/enroll
                                     body = {"name": "fluffy-gpu-1"}
     ← 200 {"id": 7, "secret_token": "abc…",
            "ws_url": "ws://192.168.1.50:8765/api/worker/connect"}
  → write /var/lib/transcoderr/worker.toml :
        coordinator_url   = "ws://192.168.1.50:8765/api/worker/connect"
        coordinator_token = "abc…"
        name              = "fluffy-gpu-1"
  → continue with existing connect logic
        (WebSocket upgrade with Bearer header → Register → RegisterAck → heartbeat loop)
```

Subsequent boots: file load succeeds, skip directly to the connect logic.

## Failure modes

| Situation | Behavior |
|---|---|
| mDNS browse times out (5s, no responder seen) | Worker exits with friendly message: `"no coordinator found on the LAN — see docs/deploy.md for manual config"`. Exit code `1`. No partial file written. |
| Multiple coordinators on the LAN | First responder wins. The pick is logged at `info` so the operator can debug if it grabbed the wrong one. Manual config remains the override. |
| Enroll endpoint returns non-2xx | Worker exits with the response body in the error message. No file written. |
| WS connect returns `401` with a cached token | Worker logs at `warn`, deletes `worker.toml`, re-runs `discover_and_enroll` once. If second `401`, exits with error. |
| Docker default-bridge networking (multicast doesn't traverse) | Discovery times out → manual fallback. Documented in `docs/deploy.md` with the `network_mode: host` recommendation. |
| Coordinator's mDNS responder fails to bind | Logged as `warn` in coordinator; coordinator runs without auto-discovery. Manual flow still works. |
| `worker.toml` exists but is malformed | Existing `WorkerConfig::load` returns `Err` → triggers the discover-and-enroll path. (Acts as a soft reset.) |
| Two enroll requests with the same `name` | Both succeed; two rows in the `workers` table with different IDs and tokens. Display name collisions are an existing UI concern, not new. |

## Testing

### Unit

- **`discovery::publish` (coordinator)** — verify the `ServiceInfo` is constructed with `_transcoderr._tcp.local.`, the configured port, and the expected TXT keys.
- **`enroll::write_config` (worker)** — verify the generated TOML round-trips through `WorkerConfig::load`.
- **`api/worker_enroll`** — handler test: posts `{name: "x"}`, asserts a row is inserted with `kind='remote'`, response includes a non-empty token and `ws_url`.

### Integration

**`crates/transcoderr/tests/auto_discovery.rs`** — single end-to-end test:

1. Boot the coordinator on an ephemeral port with discovery enabled.
2. Use a unique mDNS instance suffix (e.g. include the test PID or a uuid) so concurrent test runs don't see each other.
3. Run the worker-side `discover_and_enroll` routine against that suffix.
4. Assert the worker found the coordinator within 5s.
5. Assert the resulting `worker.toml` parses and contains a non-empty token.
6. Assert one row landed in the `workers` table with `kind='remote'`.

The test is sandbox-friendly because `mdns-sd` operates entirely in-process over loopback multicast — no network privilege required.

### Manual / acceptance

- Fresh worker container with `--network=host` and an empty `transcoderr-worker` volume → comes up green within 10s on a vanilla home LAN.
- Same container with default-bridge networking → fails fast with the documented error pointing at deploy.md.

## Migration / backward compat

- Existing `worker.toml` files continue to work unchanged. The new logic only triggers when `WorkerConfig::load` fails.
- Existing `POST /api/workers` endpoint is unchanged. The new `POST /api/worker/enroll` is additive.
- The web UI's **Workers → Add worker** modal is unchanged. (A future improvement could add a "this worker can also auto-enroll on the LAN" hint, but that is out of scope.)
- No DB migration. The `workers` table already supports the schema we need.

## Dependencies

- **`mdns-sd`** — pure-Rust mDNS responder/browser. Latest version on crates.io. No system dependencies (avahi/Bonjour not required).
- **`tower_governor`** — already in the dep tree from earlier API hardening; reuse for the enrollment rate limiter.

No new system requirements beyond the existing transcoderr deployment.

## Open questions

None — all design decisions are locked from the brainstorm. Implementation plan will decompose the work into bite-sized tasks following the established Pieces 1–6 cadence.

## Out of scope

- WAN / cross-subnet auto-discovery (would need pairing codes or a relay).
- Operator-confirmed enrollment (Q2-A: open enrollment chosen).
- Multi-tenant or revocable bootstrap secrets.
- An mDNS bridge for Docker default-bridge networking (out-of-the-box `network_mode: host` is the documented requirement).
- Re-discovery when the coordinator's IP changes without a DB wipe (rare; `rm worker.toml` recovers).
- Worker-side mDNS advertisement (workers don't advertise themselves; only the coordinator does).
- IPv6 (`mdns-sd` supports it; we just don't depend on it. If the LAN is IPv6-only, behavior is the same — the crate handles both stacks).
