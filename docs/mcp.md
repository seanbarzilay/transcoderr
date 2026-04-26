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

### System

- `get_health()` → `{healthy, ready}`
- `get_queue()` → `{pending: [], running: []}`
- `get_hw_caps()` — NVENC/QSV/VAAPI/VideoToolbox detection snapshot
- `get_metrics()` — Prometheus exposition (text passthrough)

## Worked example

> "Retry every failed run from the last 24 hours."

The AI does roughly:

1. `list_runs(status: "failed", limit: 500)` → filter results by `created_at > now - 86400`
2. For each id, `rerun_run(id)`
3. `get_queue()` to confirm they entered pending state.

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
