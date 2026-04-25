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
  ├── data.db           SQLite (jobs, flows, runs, settings)
  ├── data.db-wal/-shm  WAL artifacts (don't edit)
  ├── plugins/          Subprocess plugin directories
  ├── logs/             Spilled run-event payloads
  └── tmp/              Transcoder scratch (auto-cleaned)
```

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

If you put the binary behind nginx/caddy/traefik, terminate TLS at the proxy. WebSocket-style streaming is not used — SSE works through any HTTP/1.1 proxy.

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
