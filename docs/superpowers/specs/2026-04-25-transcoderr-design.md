# transcoderr — Design Spec

**Status:** Draft
**Date:** 2026-04-25
**Author:** sean@enclave.ai

## Summary

A self-hosted, single-binary transcoding service. Listens for "media downloaded" webhooks (typed Radarr/Sonarr adapters and a generic webhook receiver), runs configurable per-flow transcode pipelines against the file, and exposes a web UI for dashboard, flow configuration, and run management.

It is a deliberately narrower replacement for tdarr: push-driven instead of library-scanning, single-node with a worker pool instead of distributed workers, with a tighter integration story for the *arr stack and stronger observability.

## Goals

- **Simpler deployment than tdarr** — one Rust binary, one SQLite file, one Docker image. No node sprawl, no broker.
- **Better flow DX** — YAML as source of truth, live visual mirror, schema-validated editing, dry-run testing, version history.
- **Tight push integration** — first-class typed adapters for Radarr/Sonarr; generic webhook for everything else.
- **Strong observability** — per-run live logs, structured event timeline (probe → conditions → step lifecycle), Prometheus-compatible metrics endpoint.
- **Robust GPU handling** — capability probe at boot, GPU-aware concurrency scheduling, runtime CPU fallback when GPU encodes fail.
- **Pluggable** — minimal core with rich first-party plugins; subprocess JSON-RPC contract for language-agnostic external plugins.

## Non-Goals (v1)

- Distributed / remote workers
- Library scanning or file-watcher triggers (push-only)
- Multi-user, OIDC, RBAC
- Drag-and-drop visual flow *editing* (mirror is read-only)
- Quality-based encode comparison (VMAF gates) — pluggable later
- Cron / scheduled flow triggers

## Architecture

One Rust binary embeds:
- HTTP server (Axum) — webhook ingress, JSON API, SSE stream, static SPA assets
- SQLite (WAL mode) — durable state for jobs, flows, run events, plugins, sources
- Async worker pool (Tokio) — fixed-size, configurable, drives jobs to completion
- Flow engine — interprets parsed YAML, evaluates expressions, runs steps, checkpoints state
- Plugin host — invokes built-in steps in-process; spawns and communicates with subprocess plugins via stdio JSON-RPC
- Hardware capability probe — runs at boot, snapshots available encoders/devices

Frontend is a TypeScript + Vite SPA, served from the same binary, communicating over a typed JSON API and one SSE stream for live updates.

### Component breakdown

**Ingress.** Mounts:
- `POST /webhook/radarr`, `POST /webhook/sonarr`, `POST /webhook/lidarr` — typed adapters that parse vendor payloads, validate per-source bearer tokens, filter event types per source config.
- `POST /webhook/:name` — generic receiver. Accepts arbitrary JSON; per-source config defines how to extract `file.path` and metadata via expression mappings.
- `GET/POST /api/*` — JSON API (flows, runs, sources, plugins, settings, dry-run).
- `GET /api/stream` — single SSE stream carrying job state changes, run-event broadcasts, queue updates, capability changes.
- `GET /healthz`, `GET /readyz`, `GET /metrics`.
- `GET /` and asset routes — embedded SPA bundle.

**Job intake.** A successful webhook produces zero or more **Jobs**. Job creation:
1. Resolve source from path/token.
2. For each enabled flow, evaluate the flow's `triggers` (does this event type apply?) and `match.expr` (does this payload pass the filter?).
3. For each match, INSERT a `jobs` row with status=`pending`, the resolved `flow_version`, the original payload, and the file path.
4. Deduplicate by `(source_id, file_path, payload_hash)` within a configurable window (default 5 min) to absorb duplicate events from *arr.

**Worker pool.** Tokio task-per-slot. Each slot loops:
1. `SELECT … FROM jobs WHERE status='pending' ORDER BY priority DESC, created_at ASC LIMIT 1 FOR UPDATE` (using SQLite WAL + `BEGIN IMMEDIATE` for atomicity).
2. Mark `running`, drive the flow engine.
3. On completion/failure, mark final status, fire notifications, release slot.

GPU-bound steps (transcode with `hw:`) acquire a separate per-device semaphore before starting, so total concurrency = `min(pool_size, sum_per_device_limits + cpu_slots)`. This prevents NVENC oversubscription regardless of pool size.

**Flow engine.** Walks the parsed flow AST. Maintains a per-job `context` map populated incrementally (probe data, prior step outputs, file metadata, env). Conditional nodes evaluate expressions against context. After every completed step, writes a `checkpoints` row (`step_index`, snapshotted context). On a `return:` node or end of steps, marks the job's terminal status.

**Plugin host.** Two implementations behind one trait:
- **Built-in:** Rust modules (`probe`, `transcode`, `remux`, `verify.playable`, `output`, `move`, `copy`, `delete`, `extract.subs`, `strip.tracks`, `notify`, `shell`). Same input/output shape as external.
- **External:** Spawned subprocess. JSON-RPC over stdio. Emits structured progress/log/context_set events during `execute`; concludes with one `result` event.

**Frontend.** React + Vite. Monaco editor for YAML with JSON-schema-driven completion (schemas merged from each plugin's manifest). TanStack Query for API calls; small Zustand store for SSE-driven live state. Recharts for run progress and dashboards. Static SVG/HTML visual mirror re-renders on YAML AST changes.

### Data flow (happy path)

```
Radarr POST /webhook/radarr
        ↓
Adapter validates token, parses MovieFileImported event
        ↓
For each enabled flow whose triggers match:
  evaluate match.expr against payload
  if true → INSERT jobs row (status=pending, flow_version=N)
        ↓
Worker pool slot picks the job (CAS to running)
        ↓
Flow engine: probe → eval condition → transcode (with GPU semaphore)
            → verify → output:replace → notify
        ↓
Each step appends rows to run_events (started/progress/log/completed)
After each completed step, upsert checkpoints
        ↓
Final status = completed; notifier fires; SSE broadcast updates UI
```

## Flow Language

YAML, validated against a per-plugin JSON-schema-merged document.

```yaml
name: reencode-x265
description: Re-encode anything not already x265
enabled: true

triggers:
  - radarr: [downloaded, upgraded]
  - sonarr: [downloaded]
  - webhook: my-custom

match:
  expr: file.size_gb > 1 and file.path contains "/movies/"

concurrency: 2

steps:
  - id: probe
    use: probe

  - id: skip-if-x265
    if: probe.video.codec == "hevc"
    then:
      - notify: { channel: discord, template: "skipped {{file.name}} (already x265)" }
      - return: skipped

  - id: encode
    use: transcode
    with:
      codec: x265
      hw: { prefer: [nvenc, vaapi], fallback: cpu }
      crf: 22
      preset: medium
      audio: copy
      subs: copy

  - id: verify
    use: verify.playable
    with: { min_duration_ratio: 0.99 }

  - id: swap
    use: output
    with: { mode: replace, keep_original_for: 7d }

  - notify:
      channel: discord
      template: "✓ {{file.name}}: {{stats.size_delta_pct}}% smaller in {{stats.duration}}"

on_failure:
  - notify: { channel: discord, template: "✗ {{file.name}} failed at step {{failed.id}}" }
  - move_to: /media/quarantine/
```

### Grammar rules

- **`triggers`** — list of trigger predicates. A flow runs if any predicate matches the inbound event. Typed forms (`radarr: [...]`, `sonarr: [...]`) and generic (`webhook: <name>`).
- **`match.expr`** — optional CEL expression evaluated against the trigger payload. Filters out events that pass triggers but shouldn't run.
- **`concurrency`** — optional per-flow override of the global worker-pool default.
- **`steps`** — ordered list. Each entry is one of:
  - **Step:** `{ id?, use, with? }`. `use` references a plugin by name; `with` is the parameters block, validated against the plugin's schema.
  - **Conditional:** `{ id?, if: <expr>, then: [steps], else?: [steps] }`. Branches nest.
  - **Return:** `{ return: <label> }`. Short-circuits the flow with a labeled terminal outcome (so run history shows `skipped`, `partial`, etc., distinct from `completed`/`failed`).
  - **Inline shorthand** for built-ins that accept a single value (`notify: { … }`, `move_to: <path>`).
- **`on_failure`** — flow-level handler block, executed once if any step throws. Has access to `failed.{id, error, step}` in expressions.
- **`retry`** (per-step, optional) — `{ max: N, on: <expr> }`. Default: no retries.

### Expression language

CEL (Common Expression Language). Justification: well-specified, embeddable Rust crate, familiar syntax for users coming from Kubernetes/IAM. Same evaluator powers `match.expr`, `if:`, `retry.on`, and `{{ }}` template interpolation. Pure → dry-run mode evaluates conditions without side effects.

### Schema validation

Each plugin manifest declares a JSON schema for its `with:` block. The flow editor:
1. Loads all enabled plugins' schemas.
2. Constructs a unified schema for the whole flow file.
3. Feeds it to Monaco for completion + inline error squiggles.
4. Backend re-validates on save (UI is convenience, server is authoritative).

## Data Model (SQLite)

```sql
CREATE TABLE flows (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  enabled INTEGER NOT NULL DEFAULT 1,
  yaml_source TEXT NOT NULL,
  parsed_json TEXT NOT NULL,
  version INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE flow_versions (
  flow_id INTEGER NOT NULL REFERENCES flows(id),
  version INTEGER NOT NULL,
  yaml_source TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (flow_id, version)
);

CREATE TABLE sources (
  id INTEGER PRIMARY KEY,
  kind TEXT NOT NULL,                    -- 'radarr'|'sonarr'|'lidarr'|'webhook'
  name TEXT NOT NULL UNIQUE,
  config_json TEXT NOT NULL,
  secret_token TEXT NOT NULL
);

CREATE TABLE plugins (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  version TEXT NOT NULL,
  kind TEXT NOT NULL,                    -- 'builtin'|'subprocess'
  path TEXT,
  schema_json TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE notifiers (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  kind TEXT NOT NULL,                    -- 'discord'|'ntfy'|'webhook'|...
  config_json TEXT NOT NULL
);

CREATE TABLE jobs (
  id INTEGER PRIMARY KEY,
  flow_id INTEGER NOT NULL REFERENCES flows(id),
  flow_version INTEGER NOT NULL,
  source_id INTEGER REFERENCES sources(id),
  file_path TEXT NOT NULL,
  trigger_payload_json TEXT NOT NULL,
  status TEXT NOT NULL,                  -- 'pending'|'running'|'completed'
                                         -- |'failed'|'skipped'|'cancelled'
  status_label TEXT,                     -- e.g. user-defined `return:` label
  priority INTEGER NOT NULL DEFAULT 0,
  current_step INTEGER,
  attempt INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  started_at INTEGER,
  finished_at INTEGER
);
CREATE INDEX idx_jobs_pending ON jobs(status, priority DESC, created_at) WHERE status='pending';
CREATE INDEX idx_jobs_dedup ON jobs(source_id, file_path, created_at);

CREATE TABLE run_events (
  id INTEGER PRIMARY KEY,
  job_id INTEGER NOT NULL REFERENCES jobs(id),
  ts INTEGER NOT NULL,
  step_id TEXT,
  kind TEXT NOT NULL,                    -- 'started'|'progress'|'log'
                                         -- |'completed'|'failed'|'context_set'
  payload_json TEXT,                     -- inline if <=64KB
  payload_path TEXT                      -- file path if spilled
);
CREATE INDEX idx_run_events_job ON run_events(job_id, ts);

CREATE TABLE checkpoints (
  job_id INTEGER PRIMARY KEY REFERENCES jobs(id),
  step_index INTEGER NOT NULL,
  context_snapshot_json TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE hw_capabilities (
  id INTEGER PRIMARY KEY CHECK (id=1),   -- single-row
  probed_at INTEGER NOT NULL,
  devices_json TEXT NOT NULL
);
```

### Log spillover policy

`run_events.payload_json` stores inline up to 64 KB. Beyond that, the row sets `payload_path` to `data/logs/<job_id>/<step_id>-<event_id>.log` and writes the bulk content there. Read API reassembles transparently. Keeps the DB compact while preserving "DB is source of truth for structure."

### Retention

- `run_events` for completed jobs older than `N` days are pruned (default 30, configurable).
- `jobs` rows themselves kept longer (default 90 days, per-flow override).
- `checkpoints` deleted when its job reaches a terminal status.
- `flow_versions` never auto-deleted; manual prune available.
- Vacuum on a daily schedule.

### Crash recovery

On boot, every `jobs` row with `status='running'` is reset to `pending` and re-queued. A worker picks it up and:
1. Reads the `checkpoints` row for that job.
2. Hydrates `context` from the snapshot.
3. Resumes execution at `step_index + 1`.

If no checkpoint exists (crash before first step completed), restart from step 0. Transcode is *not* resume-aware — an interrupted ffmpeg run starts over. We don't fake granular resume.

## Plugin Contract

Plugins are directories under `data/plugins/`:
```
data/plugins/av1-svt/
  manifest.toml
  schema.json
  README.md
  bin/run                    # only for subprocess plugins
```

`manifest.toml`:
```toml
name = "av1-svt"
version = "0.3.0"
kind = "subprocess"
entrypoint = "bin/run"
provides_steps = ["av1.encode"]
requires = { ffmpeg = ">=6", binaries = ["SvtAv1EncApp"] }
capabilities = ["filesystem.write", "spawn.process"]
```

### JSON-RPC contract (subprocess plugins)

Methods (host → plugin):
```
init({ workdir, env, hw_caps })            → { ready: true, version }
execute({ step_id, with: {...}, context }) → streams events, then a result event
cancel({ step_id })                        → ack
shutdown()                                 → ack
```

Streaming events during `execute` (one JSON object per line, plugin → host):
```
{"event":"progress","pct":42.7,"fps":78,"eta_s":240}
{"event":"log","level":"info","msg":"ffmpeg: frame=1234 ..."}
{"event":"context_set","key":"output.size_bytes","value":1234567890}
{"event":"result","status":"ok","outputs":{"path":"/tmp/x.mkv"}}
{"event":"result","status":"err","error":{"code":"probe_failed","msg":"..."}}
```

### Decisions

- **Capabilities** are documentation-only in v1. No host-side sandboxing. Deferred to a later version when there's a concrete threat model.
- **Discovery** is automatic — boot scans `data/plugins/` and registers each found manifest. UI shows them with enable/disable toggle.
- **Built-ins** are Rust modules in-process (zero spawn overhead), but expose the same `manifest.toml` + `schema.json` metadata to the UI so the editor treats them identically.
- **Per-step timeouts** are host-enforced. Defaults: `transcode` 24h, `probe` 60s, others 10m. Configurable per step via `with: { timeout: ... }`. SIGTERM, then SIGKILL after 10s, then step fails with `timeout`.

## Web UI

### Shell

Persistent left sidebar. Six top-level sections:
1. **Dashboard** — live "now running" tiles with progress bars, queue depth, last-24h throughput, GPU utilization, recent failures.
2. **Flows** — list + detail. Detail page has tabs: *Editor* (Monaco YAML + visual mirror), *Test* (dry-run), *History* (version diff), *Recent runs*.
3. **Runs** — paginated, filterable. Detail page shows event timeline + per-step expandable logs + ffmpeg progress chart + actions (cancel, rerun-same-version, rerun-current-version).
4. **Sources** — *arr connections + generic webhooks. Auto-generated webhook URLs and bearer tokens. Test-fire button per source.
5. **Plugins** — discovered plugins, enable/disable, schema viewer.
6. **Settings** — pool size, GPU device limits, retention, auth (toggle + password), notifier configuration, system info.

### Live updates

One SSE stream `GET /api/stream` carries:
- `job.state` — pending/running/completed/etc.
- `run.event` — new `run_events` row
- `queue.snapshot` — depth, running count
- `caps.update` — capability re-probe

Frontend tabs subscribe to filtered slices via topic prefix. No polling.

### Auth

- Disabled by default (LAN/homelab convention).
- When enabled: single-user password, session cookie, all `/api/*` endpoints require it.
- Webhook authentication is **independent** — each source carries its own bearer token, validated whether UI auth is on or off.

### Mobile posture

- Dashboard and Run detail are responsive and usable on phones.
- Flow editor is desktop-only and explicitly states so on small viewports.

## Hardware Acceleration

### Probe

At boot:
- Run `ffmpeg -encoders` and parse for `h264_nvenc`, `hevc_nvenc`, `h264_qsv`, `hevc_qsv`, `h264_vaapi`, `hevc_vaapi`, `h264_videotoolbox`, etc.
- Detect device count via `nvidia-smi -L` (NVENC), `/dev/dri/render*` enumeration (VAAPI/QSV), `system_profiler SPDisplaysDataType` (VideoToolbox).
- Snapshot to `hw_capabilities.devices_json`.

### Scheduling

- Each detected device is a semaphore. `nvenc:0` permits = configured session limit (default 3 for consumer NVENC, configurable in Settings).
- Steps with `hw: { prefer: [nvenc, ...] }` acquire from preferred devices in order.
- If no preferred device available, optionally block (default) or fall back to next preference.

### Fallback

Steps with `hw: { fallback: cpu }`:
- If GPU acquire fails → fall back immediately, log `hw_unavailable`.
- If GPU encode fails mid-run (driver hiccup, OOM) → kill subprocess, retry once on CPU with same parameters but degraded preset (configurable). Logged as `hw_runtime_failure`. Visible in run history and `/metrics`.
- ENOSPC (disk full) does *not* trigger CPU fallback — the retry would fail the same way. Step fails immediately with `disk_full`.

## Notifications

Pluggable notifier system. First-party notifiers shipped:
- **Discord** (webhook URL)
- **ntfy** (server + topic)
- **Generic webhook** (URL + headers + JSON template)

Configured under Settings → Notifiers. Each gets a name; flows reference by name. Templates are CEL-interpolated against the run's final context.

Long tail (Telegram, Slack, Pushover, email) is handled by the plugin system — anyone can write a notifier plugin that conforms to the same `Step` contract.

## Observability

- **Live logs** — `run_events` rows of `kind='log'` are streamed via SSE to the run detail view in real time.
- **Structured timeline** — every step lifecycle event is a row. UI renders a vertical timeline; filterable by kind. This is the "why did the flow branch this way" view.
- **Prometheus** — `GET /metrics` exposes:
  - `transcoderr_jobs_total{status,flow}` (counter)
  - `transcoderr_job_duration_seconds{flow,status}` (histogram)
  - `transcoderr_queue_depth` (gauge)
  - `transcoderr_workers_busy` (gauge)
  - `transcoderr_gpu_session_active{device}` (gauge)
  - `transcoderr_step_duration_seconds{plugin,status}` (histogram)
  - `transcoderr_bytes_saved_total` (counter)
- **Application logs** — stdout, JSON when `--log-format json`, systemd-friendly.

## Failure Modes

| Failure | Behavior |
|---|---|
| ffmpeg non-zero exit | Step fails with captured stderr; `on_failure` runs; job → `failed`. No automatic retry unless flow opts in. |
| GPU acquire fails | If `fallback: cpu` declared → CPU path, logged. Else step fails. |
| GPU runtime failure | Killed; one CPU retry (if `fallback: cpu`); logged as `hw_runtime_failure`. |
| Source file missing mid-job | Fail fast with `source_missing`; `on_failure` runs; no partial output written. |
| `verify.playable` fails | Subsequent destructive steps (`output: replace`) skipped; original intact; job → `failed`. |
| Disk full (ENOSPC) | ffmpeg fails; partial temp file cleaned up by step's `Drop`; job → `failed`. |
| Process crash mid-job | On boot, `running` jobs reset to `pending`; resume from last completed checkpoint. Interrupted step re-runs from scratch. |
| Subprocess plugin hangs | Per-step timeout (default 24h transcode, 60s probe). SIGTERM, then SIGKILL+10s, then `timeout` failure. |
| Webhook flood / dupe | Dedup by `(source_id, file_path, payload_hash)` within 5-min window. |

## Distribution & Operations

- **Binaries** — static for `linux-amd64`, `linux-arm64`, `darwin-arm64`.
- **Docker images** — separate tags per hardware accel target:
  - `:cpu` — minimal (ffmpeg without proprietary toolchains)
  - `:nvidia` — with NVENC/NVDEC tooling
  - `:intel` — with QSV/VAAPI
  - `:full` — everything (largest)
- **Stateful directory** — `data/`. Contains `data.db`, `logs/`, `plugins/`, `tmp/`. Volume-mount this and the deployment is portable.
- **Bootstrap config** — `config.toml`: port, data dir, worker count, auth toggle, log format. Everything else lives in SQLite, edited via UI.
- **Migrations** — applied on startup. Refuse to start if DB schema version is newer than binary's.
- **Health** — `/healthz` (always 200 if reachable), `/readyz` (200 only after capability probe + plugin init complete).

## Testing Strategy

- **Unit tests** — flow parser, expression evaluator (CEL), conditional resolution, schema validation, dedup logic. No ffmpeg.
- **Integration tests** — real ffmpeg against tiny generated test clips (`testsrc=duration=2`). Cover probe, codec switch, container remux, hw-fallback path (mock-fail GPU).
- **End-to-end** — boot the binary, POST a Radarr-shaped webhook, assert job reaches `completed`, assert output file exists and is playable.
- **No mocked ffmpeg.** Mocks here would test almost nothing of value.

## Open Questions

None remaining at design time — all forks have been resolved. Implementation may surface new questions, captured in the implementation plan.
