# transcoderr

A push-driven, single-binary media transcoder for self-hosted media stacks.
Webhook in, ffmpeg out, configurable in between.

```
┌──────────┐   webhook   ┌──────────────┐   one ffmpeg pass   ┌────────┐
│  Radarr  │ ──────────▶ │  transcoderr │ ──────────────────▶ │  .mkv  │
│  Sonarr  │             │  flow engine │                     │ replaced│
└──────────┘             └──────────────┘                     └────────┘
```

## What it does

- **Push-driven.** Typed adapters for Radarr / Sonarr / Lidarr plus a generic
  `/webhook/:name` for anything else. No library scanning.
- **Browse & queue.** Built-in **Browse Radarr** / **Browse Sonarr** pages
  mirror your library as poster grids with codec/resolution filters, debounced
  search, and per-season drill-down for shows. Click a file to fan it out to
  every enabled flow that matches the source kind. Same flow engine as the
  webhook path — see [Browse & queue](#browse--queue) below.
- **Plan-then-execute flows.** Compose declarative `plan.*` steps
  (`plan.video.encode`, `plan.audio.ensure`, `plan.streams.drop_cover_art`, …).
  A single `plan.execute` materializes the whole flow into **one** ffmpeg
  invocation — no chained tmp files, no per-step IO churn.
- **Hardware-aware.** Boot-time probe of NVENC / QSV / VAAPI / VideoToolbox,
  per-device concurrency semaphores, runtime CPU fallback if the GPU encode
  fails mid-job.
- **Live observability.** Per-run progress bar, the latest ffmpeg status line
  streamed live (one event every ~1.5s), structured timeline of every step
  decision, Prometheus-compatible `/metrics`.
- **Notifiers.** Discord, ntfy, Telegram, generic webhook. Configurable in the
  UI.
- **Single binary.** Rust + embedded SQLite + embedded React SPA. One image,
  one volume mount, no broker, no external DB.

## Quickstart

```yaml
# docker-compose.yml
services:
  transcoderr:
    image: ghcr.io/seanbarzilay/transcoderr:cpu-latest
    restart: unless-stopped
    ports: ["8099:8080"]
    volumes:
      - ./data:/data
      # IMPORTANT: mount your media at the SAME path it has in Radarr/Sonarr.
      - /mnt/movies:/mnt/movies
```

Then:

```bash
docker compose up -d
open http://localhost:8099
```

First boot creates `/data/config.toml` from the bundled example. The web UI
walks you through:

1. **Sources → Add.** Pick `radarr` (or sonarr/lidarr), give it a name, paste a
   secret token. In Radarr → Settings → Connect → Webhook, set:
   - URL: `http://transcoderr:8080/webhook/radarr`
   - Method: `POST`
   - Username: anything
   - Password: the secret token (Basic auth is supported alongside Bearer)
2. **Notifiers → Add.** Optional. Configure Discord/ntfy/Telegram/webhook so
   flows can `notify`.
3. **Flows → New flow.** Paste in a flow YAML (example below).

## Browse & queue

Every Radarr or Sonarr source you add gets two extra pages in the sidebar:
**Browse Radarr** and **Browse Sonarr**. They turn the source's own library
into a searchable view, so you don't need to wait for the next webhook to
re-encode something.

- **Poster grid** with debounced title search, codec and resolution filters,
  and title/year sort. Pagination keeps the grid responsive on libraries with
  thousands of titles.
- **Detail panel** (movies) or **series drill-down** (shows). For Sonarr, click
  a series to get season tabs with `have/total` counts, then filter episodes
  inside a season by codec or resolution.
- **One-click queue.** The Transcode button on any file fans it out to every
  enabled flow whose triggers match the source kind — a movie hits all flows
  that listen to `radarr`, an episode hits all flows that listen to `sonarr`.
  Each fan-out is a regular run with its own progress bar, ffmpeg tail, and
  cancel button.
- **Refresh** re-pulls library state from the source so newly added or
  upgraded files show up without a server restart.

Useful for backfilling a library after introducing a new flow, re-encoding a
single bad file, or testing flow changes on a known-good title without
waiting for the next *arr push. The MCP server exposes the same surface
(`list_movies`, `list_series`, `list_episodes`, `transcode_file`), so prompts
like *"queue every non-HEVC movie"* drive the exact same code path.

## Example flow

[`docs/flows/hevc-normalize.yaml`](docs/flows/hevc-normalize.yaml) re-encodes
anything that isn't already hevc, ensures an English AC3 6ch audio track
exists, drops cover-art / data streams, and preserves every other stream
(including subtitles). Edited live in the **Flows** page; the visual mirror
re-renders as you type.

```yaml
name: hevc-normalize
triggers:
  - radarr: [downloaded, upgraded]
  - sonarr: [downloaded]

steps:
  - use: probe
  - use: plan.init
  - use: plan.input.tolerate_errors
  - use: plan.streams.drop_cover_art
  - use: plan.streams.drop_data

  - if: probe.streams[0].codec_name == "hevc"
    then: []
    else:
      - use: plan.video.encode
        with:
          codec: x265
          crf: 19
          preset: fast
          preserve_10bit: true
          hw: { prefer: [nvenc, qsv, vaapi, videotoolbox], fallback: cpu }

  - use: plan.audio.ensure
    with: { codec: ac3, channels: 6, language: eng, dedupe: true }

  - use: plan.execute        # ONE ffmpeg pass
  - use: verify.playable
  - use: output
    with: { mode: replace }
  - use: notify
    with: { channel: tg-main, template: "✓ {{ file.path }} normalized" }

on_failure:
  - use: notify
    with: { channel: tg-main, template: "✗ {{ file.path }} failed at {{ failed.id }}: {{ failed.error }}" }
```

## Image flavors

| tag | base | hardware accel |
|---|---|---|
| `:cpu-latest` | `debian:bookworm-slim` + ffmpeg | software only |
| `:intel-latest` | bookworm + intel-media-va-driver | QSV / VAAPI |
| `:nvidia-latest` | jrottenberg/ffmpeg-nvidia | NVENC / NVDEC |
| `:full-latest` | NVIDIA base + Intel runtime | NVENC + QSV/VAAPI |

Each tag also exists pinned to a version (`:cpu-v0.6.2`, etc.). Static
binaries (`linux-amd64`, `linux-arm64`, `darwin-arm64`) ship attached to
each GitHub Release.

## Build from source

```bash
npm --prefix web ci && npm --prefix web run build   # builds the SPA
cargo build --release                                # embeds dist/ via include_dir
./target/release/transcoderr serve --config config.toml
```

Requires Rust ≥ 1.85, Node 20, and `ffmpeg`/`ffprobe` on PATH at runtime.

## How it works (one paragraph)

A webhook turns into a `jobs` row. The single-process worker pool claims
pending jobs via SQLite WAL, hands each to the flow engine, which walks the
flow YAML — recording every step lifecycle event into `run_events` and
broadcasting onto an internal SSE bus that the React UI subscribes to.
Plan-mutator steps tweak a `StreamPlan` carried in the run context;
`plan.execute` materializes that plan into one ffmpeg invocation. Crash
recovery resets `running` rows on boot and resumes from the last completed
step's checkpoint.

## Endpoints worth knowing

| path | purpose |
|---|---|
| `/` | the web UI |
| `/webhook/{radarr,sonarr,lidarr}` | typed source adapters (Bearer or Basic auth) |
| `/webhook/:name` | generic JSON webhook |
| `/api/...` | typed JSON API the UI uses (authed when auth is on) |
| `/api/stream` | SSE event stream (job state + run events + queue) |
| `/healthz` / `/readyz` | k8s-friendly probes |
| `/metrics` | Prometheus exposition |

## MCP server

`transcoderr-mcp` is a stdio MCP binary that lets AI clients (Claude Desktop,
Cursor) drive transcoderr's read & write surface. Download the binary for
your platform from the latest GitHub Release, then point your AI client at
it. Tools cover runs, flows, sources, notifiers, plus library browse +
per-file transcode — so prompts like "queue every non-HEVC movie" or
"re-encode every 1080p episode of *this* show" work as one operation.

```json
{
  "mcpServers": {
    "transcoderr": {
      "command": "/usr/local/bin/transcoderr-mcp",
      "env": {
        "TRANSCODERR_URL": "http://192.168.1.176:8099",
        "TRANSCODERR_TOKEN": "tcr_xxxxxxxxxxxxxxxx"
      }
    }
  }
}
```

Create the token under **Settings → API tokens** in the web UI. See
[`docs/mcp.md`](docs/mcp.md) for the full tool reference.

## Documentation

- [`docs/deploy.md`](docs/deploy.md) — production deploy notes
- [`docs/mcp.md`](docs/mcp.md) — MCP server reference
- [`docs/flows/`](docs/flows/) — example flow YAMLs
- [`docs/superpowers/specs/`](docs/superpowers/specs/) — original design spec
- [`docs/superpowers/plans/`](docs/superpowers/plans/) — phase-by-phase
  implementation plans

## License

(TBD by the project owner.)
