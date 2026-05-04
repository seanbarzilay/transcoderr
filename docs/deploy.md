# Deployment Guide

## Choose an image flavor

- `:cpu` — software-only ffmpeg. Smallest image. Works everywhere.
- `:nvidia` — NVENC/NVDEC via the NVIDIA Container Runtime. Requires `--gpus all` (Docker) or `nvidia` runtime (Compose). Linux/x86_64 only.
- `:intel` — VAAPI / QSV. Requires `/dev/dri` to be exposed to the container.
- `:full` — NVIDIA + Intel toolchains. Largest image.

## Volumes

Mount `/data` for state. The directory layout:

```
/data
  ├── data.db           SQLite (jobs, flows, runs, settings, catalogs)
  ├── data.db-wal/-shm  WAL artifacts (don't edit)
  ├── plugins/          Installed plugin directories (managed via UI / MCP)
  ├── logs/             Spilled run-event payloads
  └── tmp/              Transcoder scratch (auto-cleaned)
```

`plugins/` is server-managed: install / uninstall via **Plugins → Browse**
in the UI (or the `install_plugin` / `uninstall_plugin` MCP tools). Hand-
dropping a directory here still works for local development, but won't
get a catalog provenance row.

Mount your media library read/write under whatever path your flows reference (commonly `/media`).

## Bootstrap config

`/data/config.toml`:

```toml
bind = "0.0.0.0:8080"
data_dir = "/data"

[radarr]
bearer_token = "ignored-but-required-for-now"  # Phase 1 holdover; sources are now in DB
```

## Connecting Radarr / Sonarr / Lidarr

1. Settings → Sources → Add. Pick the kind, give it a name, paste a bearer token.
2. In Radarr/Sonarr → Settings → Connect → Webhook:
   - URL: `http://transcoderr:8080/webhook/radarr` (or `/sonarr`, `/lidarr`)
   - Method: POST
   - Add header: `Authorization: Bearer <your-token>`
   - Triggers: On Download, On Upgrade

## Generic webhooks

Use `kind=webhook` for arbitrary integrations. Each webhook source has its own URL: `/webhook/<source-name>`. Provide a `path_expr` in `config_json` (default: `steps.payload.path`) that extracts the file path from the inbound JSON via CEL.

## Reverse proxy + auth

The binary serves both webhooks and the UI on the same port. To require login on the UI:

1. Settings → Auth → Enable + set password.
2. UI traffic now requires a session cookie. Webhooks remain authenticated by their per-source token regardless.

If you put the binary behind nginx/caddy/traefik, terminate TLS at the proxy. SSE works through any HTTP/1.1 proxy. The `/api/worker/connect` endpoint is a WebSocket upgrade — see the Distributed transcoding section below for proxy notes.

## Distributed transcoding (worker mode)

A second host can connect to a running coordinator as a worker and
receive ffmpeg / heavy plugin work over a WebSocket. As of v0.37 the
full distributed pipeline is live: the dispatcher routes per-step
work between the local worker and any registered remote workers, the
operator can cancel a run mid-flight (signal propagates all the way
to the worker's ffmpeg child), and plugin steps marked
`run_on: any-worker` are eligible for remote dispatch.

Three operator concerns: **enrolling a worker**, **mismatched
filesystem paths**, and **how many runs proceed in parallel**.

### Enrolling a worker

Two flows:

**A. LAN auto-discovery (v0.38+, recommended).** On the same
broadcast domain as the coordinator, a worker started with no
`worker.toml` finds the coordinator via mDNS
(`_transcoderr._tcp.local.`), enrolls for a fresh token over
`POST /api/worker/enroll`, and writes its config to
`/var/lib/transcoderr/worker.toml`. One command on the worker host:

```bash
docker run --rm --network=host \
  -v transcoderr-worker:/var/lib/transcoderr \
  ghcr.io/seanbarzilay/transcoderr:nvidia-latest \
  transcoderr worker
```

`--network=host` is required because mDNS multicast doesn't propagate
through Docker's default bridge. If the cached token is later
rejected (the coordinator's database was wiped, etc.), the worker
re-discovers and re-enrolls automatically — once, then exits. To
disable the responder on the coordinator side, set
`TRANSCODERR_DISCOVERY=disabled`.

**B. Manual token.** When the worker isn't on the same LAN
(remote VPS, behind a VPN, Docker default-bridge networking), mint a
token in the coordinator UI (**Workers → Add worker**) and drop a
`worker.toml` on the worker host:

```toml
coordinator_url   = "wss://transcoderr.example/api/worker/connect"
coordinator_token = "<token-shown-once>"
name              = "gpu-box-1"
```

```bash
docker run --rm \
  -v $(pwd)/worker.toml:/etc/transcoderr/worker.toml \
  -v /mnt/movies:/mnt/movies \
  ghcr.io/seanbarzilay/transcoderr:nvidia-latest \
  transcoderr worker --config /etc/transcoderr/worker.toml
```

Either flow enrolls a row in the coordinator's `workers` table; the
Workers UI shows it as `remote` once it connects.

### Mismatched filesystem paths

Older releases required the worker to mount the media volume at the
same absolute path as the coordinator. As of v0.39+ that's optional
— the coordinator can rewrite paths per-worker on the wire. From the
Workers page, click **Edit mappings** on a remote worker and add
prefix pairs (e.g. coordinator `/mnt/movies` → worker
`/data/media/movies`). The dispatcher walks the Context JSON snapshot
on dispatch and again on completion, so paths in `ctx.file.path` and
`ctx.steps.<id>.output_path` are translated transparently. Workers
with no mappings configured stay on identity translation —
homogeneous-mount setups keep working unchanged.

### Concurrent run limit

The `runs.max_concurrent` setting (Settings page) caps how many flow
runs the coordinator processes in parallel — one job per claim loop.
Hardware semaphores still cap concurrent ffmpeg invocations per GPU,
so this setting only controls flow-level parallelism. Defaults to 2.
This setting was renamed from `worker.pool_size` in v0.39.2; the
migration preserves any operator-customised value automatically.

### Reverse-proxy notes

Workers connect over WebSocket. If the coordinator is behind
nginx / caddy / traefik, make sure `Upgrade` and `Connection` headers
are passed through (most defaults do, but it's worth checking on a
502).

## Plugin catalogs and runtimes

The default catalog is
[`seanbarzilay/transcoderr-plugins`](https://github.com/seanbarzilay/transcoderr-plugins);
add private catalogs in **Plugins → Catalogs** (any HTTPS-served
`index.json` + sha256-pinned tarballs). Install gates on the plugin's
declared `runtimes` being on PATH and runs the plugin's `deps` shell
command (e.g. `pip install -r requirements.txt`) before the steps
register; failures roll the install back.

To add interpreters to the container without baking a new image, set
`TRANSCODERR_RUNTIMES` to a comma-separated list of apt package names:

```yaml
services:
  transcoderr:
    image: ghcr.io/seanbarzilay/transcoderr:cpu-latest
    environment:
      TRANSCODERR_RUNTIMES: "python3,nodejs"
```

The entrypoint runs `apt-get install` on every boot — fresh containers
always match the env var. Adds ~10–60s to startup depending on which
runtimes you ask for; empty/unset is a no-op. Names are passed verbatim
to apt; the entrypoint rejects anything outside `[a-zA-Z0-9.+-]`.

## Backups

Back up the `/data` directory. SQLite WAL is durable on graceful shutdown; the binary handles SIGTERM cleanly.

## Common troubleshooting

- **GPU not detected.** `GET /api/hw` returns the probed devices. NVENC requires NVIDIA driver + `--gpus all`. VAAPI requires `/dev/dri/renderD128` in the container.
- **NVENC session limit.** Consumer cards are limited to 3 concurrent sessions by driver. Settings → Hardware → adjust the per-device limit.
- **ffprobe errors.** Usually a missing/unreadable file. Check the source path is mounted into the container.
- **Webhook 401.** Authorization header missing or token mismatch — re-check the source's `secret_token`.
- **Worker stuck.** Inspect `/api/runs` for jobs in `running` state; the next boot resets them and resumes from the last checkpoint.

## Observability

- `/healthz` — always 200 if reachable.
- `/readyz` — 200 once boot completes.
- `/metrics` — Prometheus exposition format.

Add to your Prometheus scrape config:

```yaml
- job_name: transcoderr
  static_configs:
    - targets: ["transcoderr:8080"]
```
