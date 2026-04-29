# transcoderr MCP server

`transcoderr-mcp` is a Rust binary that speaks the Model Context Protocol
over stdio. It's a stateless proxy: the AI client invokes a tool, the
binary translates that into an authenticated HTTPS call against
`transcoderr serve`, and returns the result.

## Configuration

Three env vars (or the matching CLI flags):

| var                         | required | default | meaning                              |
| --------------------------- | -------- | ------- | ------------------------------------ |
| `TRANSCODERR_URL`           | yes      | —       | base URL of the server               |
| `TRANSCODERR_TOKEN`         | yes      | —       | API token from Settings → API tokens |
| `TRANSCODERR_TIMEOUT_SECS`  | no       | `30`    | per-call HTTP timeout                |

CLI flags (`--url`, `--token`, `--timeout-secs`) override env vars when present.

## Creating a token

1. In the web UI, go to **Settings → API tokens**.
2. Click **Create token**, give it a name (e.g. `claude-desktop`).
3. Copy the token shown once — you can't recover it later.
4. Paste it into your AI client's MCP config under `env.TRANSCODERR_TOKEN`.

Tokens are stored hashed with argon2id. To rotate, revoke the old one and
create a new one.

## Tool reference

### Runs

- `list_runs(status?, flow_id?, limit?, offset?)` — list runs newest-first
- `get_run(id)` — run + last 200 events
- `get_run_events(id, limit?, offset?)` — raw events oldest-first
- `cancel_run(id)` — kill a running job (SIGKILL to ffmpeg)
- `rerun_run(id)` — enqueue a new job from this one's flow + file

### Flows

- `list_flows()`
- `get_flow(id)` — YAML + parsed AST
- `create_flow({name, yaml})`
- `update_flow({id, yaml, enabled?})`
- `delete_flow({id, confirm: true})`
- `dry_run_flow({yaml, file_path, probe?})` — walk the AST without execution

### Sources

- `list_sources()` — secret tokens redacted to `***`
- `get_source(id)`
- `create_source({kind, name, config, secret_token})`
- `update_source({id, name?, config?, secret_token?})` — sending `"***"` is treated as 'unchanged'
- `delete_source({id, confirm: true})`

### Notifiers

- `list_notifiers()` — secret-bearing config keys redacted
- `get_notifier(id)`
- `create_notifier({name, kind, config})`
- `update_notifier({id, name, kind, config})` — sending `"***"` for any secret-bearing key is treated as 'unchanged'
- `delete_notifier({id, confirm: true})`
- `test_notifier(id)`

### Library browse (Radarr / Sonarr proxy)

These tools mirror the **Browse Radarr / Sonarr** pages in the web UI: they
proxy the *arr's REST API through transcoderr, return a trimmed view, and
filter to files that have actually been imported. Available only for
auto-provisioned `radarr`/`sonarr` sources (those whose `config` has
`base_url` + `api_key`).

- `list_movies({source_id, search?, sort?, codec?, resolution?, page?, limit?})` — Radarr movies. Response includes `available_codecs`/`available_resolutions` so you can discover valid filter values in one round-trip.
- `list_series({source_id, search?, sort?, codec?, resolution?, page?, limit?})` — Sonarr series. Each item carries the union of codecs/resolutions across its episode files plus a top-level `available_codecs`/`available_resolutions` for the page-level dropdowns.
- `get_series({source_id, series_id})` — series detail (poster, fanart, overview, season counts).
- `list_episodes({source_id, series_id, season?, codec?, resolution?})` — downloaded episodes of one series, with the same `available_*` discovery sets.
- `transcode_file({source_id, file_path, title, movie_id?, series_id?, episode_id?})` — enqueue runs for a specific file. Fans out across every enabled flow whose triggers match the source's kind (same semantics as a real *arr push). Returns the new run ids.

### System

- `get_health()` → `{healthy, ready}`
- `get_queue()` → `{pending: [], running: []}`
- `get_hw_caps()` — NVENC/QSV/VAAPI/VideoToolbox detection snapshot
- `get_metrics()` — Prometheus exposition (text passthrough)

## Worked examples

### Retry every failed run from the last 24 hours

1. `list_runs(status: "failed", limit: 500)` → filter results by `created_at > now - 86400`
2. For each id, `rerun_run(id)`
3. `get_queue()` to confirm they entered pending state.

### Re-encode every non-HEVC movie

1. `list_sources()` → find the radarr source id.
2. `list_movies({source_id, codec: "h264", limit: 200})` (loop pages until `items.length < limit`).
3. For each movie, `transcode_file({source_id, file_path: m.file.path, title: m.title, movie_id: m.id})`.
4. `get_queue()` to watch the pending queue drain.

### Re-encode every 1080p episode of one show

1. `list_series({source_id})` → pick the series id.
2. `list_episodes({source_id, series_id, resolution: "1920x1080"})`.
3. Loop `transcode_file(...)` per episode.

## Errors

The binary maps HTTP responses to MCP errors:

| HTTP    | MCP code           | Meaning                                       |
| ------- | ------------------ | --------------------------------------------- |
| 400     | `INVALID_PARAMS`   | bad arguments — message has details           |
| 401     | `AUTH_FAILED`      | token rejected; check `TRANSCODERR_TOKEN`     |
| 403     | `FORBIDDEN`        | (rare)                                        |
| 404     | `NOT_FOUND`        | resource doesn't exist                        |
| 409     | `CONFLICT`         | uniqueness violation (e.g. flow name in use)  |
| 5xx     | `INTERNAL`         | server error; check server logs               |
| network | `UNREACHABLE`      | could not connect to `TRANSCODERR_URL`        |

## Logging

The binary logs to **stderr** (stdout is the MCP protocol). Set
`RUST_LOG=transcoderr_mcp=debug` to see request/response details. Tokens
are never logged.
