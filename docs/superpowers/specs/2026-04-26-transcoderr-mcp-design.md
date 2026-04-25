# transcoderr MCP server — design

**Status:** approved 2026-04-26
**Target version:** v0.7.0

## 1. Goal

Expose transcoderr's read and write surface to AI clients (Claude Desktop,
Cursor, etc.) over the Model Context Protocol so that an AI can inspect runs,
edit flows, and manage sources/notifiers without going through the web UI.

## 2. System shape

```
┌────────────────┐  stdio   ┌──────────────────┐   HTTPS    ┌──────────────────┐
│  AI client     │ ───────▶ │ transcoderr-mcp  │ ─────────▶ │ transcoderr serve│
│ (Claude, etc.) │ ◀─────── │ (stdio MCP proxy)│ ◀───────── │ (existing HTTP)  │
└────────────────┘          └──────────────────┘            └──────────────────┘
```

The repo becomes a Cargo workspace with three crates:

- `transcoderr` — the existing server, moved verbatim into `crates/transcoderr/`.
- `transcoderr-api-types` — shared request/response types (`serde` +
  `schemars` derives only; no logic).
- `transcoderr-mcp` — a stdio MCP binary built on the `rmcp` SDK that proxies
  every tool call to the server's HTTP API.

The MCP binary is a stateless proxy. It holds no DB, no cache, no business
logic. The server is authoritative.

### Lifecycle of one MCP tool call

1. AI client invokes a tool (e.g. `list_runs`) over stdio.
2. `rmcp` validates args against the JSON schema derived from the shared
   request type.
3. The tool handler builds a `reqwest` request, attaches
   `Authorization: Bearer $TRANSCODERR_TOKEN`, and POSTs/GETs the matching
   `/api/...` endpoint.
4. The HTTP response body deserializes into a shared response type and is
   returned to the client.
5. On failure, the HTTP error is mapped to an MCP `ToolError` (see §5).

## 3. Authentication

A new DB table holds API tokens, separate from the existing user/session
system:

```sql
CREATE TABLE api_tokens (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  name         TEXT NOT NULL,
  hash         TEXT NOT NULL,
  prefix       TEXT NOT NULL,
  created_at   INTEGER NOT NULL,
  last_used_at INTEGER
);
CREATE UNIQUE INDEX api_tokens_prefix_idx ON api_tokens(prefix);
```

**Token format.** `tcr_` + 32 url-safe random chars (~190 bits of entropy).
Hashed with `argon2id` (same scheme as `users.password_hash`). The `prefix`
column stores the first 8 chars in cleartext for UI display. The full token
is shown to the user **once** at creation and never persisted in cleartext.

**Endpoints** (all behind existing session auth):

| route                          | purpose                                          |
| ------------------------------ | ------------------------------------------------ |
| `GET /api/auth/tokens`         | list `{id, name, prefix, created_at, last_used_at}` |
| `POST /api/auth/tokens`        | body `{name}` → `{id, token}` (only response that reveals secret) |
| `DELETE /api/auth/tokens/:id`  | revoke                                            |

**Middleware extension.** The existing `require_auth` middleware already
validates session cookies. It is extended to also accept
`Authorization: Bearer tcr_...`. Lookup path:

1. Slice the prefix (`tcr_` + first 4 random chars).
2. Find the row by `prefix` (indexed).
3. Verify with `argon2`.
4. On success, fire-and-forget update of `last_used_at`.

Revocation has next-request granularity — an in-flight tool call may
complete after the token is deleted; subsequent requests fail with 401.
Tokens have no expiry in v1.

**UI.** New "API tokens" card on the Settings page: a table of existing
tokens (name + masked prefix + last-used) with a "Create token" button. On
create, a one-time modal shows the secret with copy-to-clipboard and a "I've
saved it" dismiss. Revocation is a trash icon with a `confirm()` prompt.

**MCP binary side.** Reads `TRANSCODERR_URL` and `TRANSCODERR_TOKEN` from
the environment. Sets the bearer header on every reqwest call. Missing env
vars cause exit-1 with a clear stderr message before MCP capabilities are
announced.

## 4. Tool surface

The MCP binary registers ~28 granular tools. Names use `verb_resource` form.
Each tool maps 1:1 to an HTTP endpoint.

### Runs (read-heavy)

| tool             | maps to                          |
| ---------------- | -------------------------------- |
| `list_runs`      | `GET /api/runs`                  |
| `get_run`        | `GET /api/runs/:id`              |
| `cancel_run`     | `POST /api/runs/:id/cancel`      |
| `rerun_run`      | `POST /api/runs/:id/rerun`       |
| `get_run_events` | `GET /api/runs/:id/events`       |

`list_runs` accepts `status`, `flow_id`, `limit`, `cursor`. Default
`limit=50`, max `500`. Other list tools cap server-side and do not paginate.

### Flows (read + write)

| tool            | maps to                                  |
| --------------- | ---------------------------------------- |
| `list_flows`    | `GET /api/flows`                         |
| `get_flow`      | `GET /api/flows/:id`                     |
| `create_flow`   | `POST /api/flows` (`{name, yaml}`)       |
| `update_flow`   | `PUT /api/flows/:id`                     |
| `delete_flow`   | `DELETE /api/flows/:id`                  |
| `dry_run_flow`  | `POST /api/dry-run`                      |

### Sources (CRUD)

| tool              | maps to                            |
| ----------------- | ---------------------------------- |
| `list_sources`    | `GET /api/sources`                 |
| `get_source`      | `GET /api/sources/:id`             |
| `create_source`   | `POST /api/sources`                |
| `update_source`   | `PUT /api/sources/:id`             |
| `delete_source`   | `DELETE /api/sources/:id`          |

### Notifiers (CRUD)

| tool                | maps to                              |
| ------------------- | ------------------------------------ |
| `list_notifiers`    | `GET /api/notifiers`                 |
| `get_notifier`      | `GET /api/notifiers/:id`             |
| `create_notifier`   | `POST /api/notifiers`                |
| `update_notifier`   | `PUT /api/notifiers/:id`             |
| `delete_notifier`   | `DELETE /api/notifiers/:id`          |
| `test_notifier`     | `POST /api/notifiers/:id/test`       |

### System / observability

| tool          | maps to                                                  |
| ------------- | -------------------------------------------------------- |
| `get_queue`   | `GET /api/runs?status=pending,running` (composed)        |
| `get_health`  | `GET /healthz` + `/readyz`                               |
| `get_hw_caps` | `GET /api/hw`                                            |
| `get_metrics` | `GET /metrics` (Prometheus text passed through verbatim) |

### Confirmation requirement

All `delete_*` tools require an explicit `confirm: true` arg. The schema
marks it required, so rmcp rejects the call before any HTTP request fires
if the AI client omits it. No other tool requires confirmation.

### Schema strategy

The `transcoderr-api-types` crate exports `JsonSchema`-deriving structs.
The MCP binary calls `schema_for!(CreateFlowRequest)` once at startup per
tool and feeds the resulting schema to rmcp. The schema the AI sees is
literally the request type the HTTP API validates — drift impossible.

### Out of scope for v1

- `start_run` / `enqueue_job` — runs are triggered by webhooks.
- `get_config` / `update_config` — global TOML editing stays UI-only.
- `download_file` / `upload_file` — out of scope for an automation surface.

## 5. Errors, validation, observability

### Error mapping

The shared types crate defines:

```rust
pub struct ApiError {
    pub code: String,       // stable machine code, e.g. "flow.not_found"
    pub message: String,    // human-readable
    pub details: Option<serde_json::Value>,
}
```

The MCP binary translates HTTP responses into rmcp `ToolError`:

| HTTP             | MCP outcome                  | Example message                                      |
| ---------------- | ---------------------------- | ---------------------------------------------------- |
| 200 / 204        | success                      | tool returns deserialized payload                    |
| 400              | `INVALID_PARAMS`             | "flow.yaml: line 4: unknown step `transcode`"        |
| 401              | `AUTH_FAILED`                | "API token rejected — check `TRANSCODERR_TOKEN`"     |
| 403              | `FORBIDDEN`                  | (rare; reserved)                                     |
| 404              | `NOT_FOUND`                  | "run 123 does not exist"                             |
| 409              | `CONFLICT`                   | "flow name already in use"                           |
| 5xx              | `INTERNAL`                   | message includes `details` if present                |
| network failure  | `UNREACHABLE`                | "could not connect to $TRANSCODERR_URL"              |

### Validation

Validation runs twice, intentionally:

1. **Client-side** via rmcp + JsonSchema. Wrong types, missing required
   fields, missing `confirm: true` — rejected before HTTP.
2. **Server-side** in transcoderr — sqlx constraints, YAML parse, CEL
   parse. The server is authoritative; the schema is fast-fail.

No new validation logic on the HTTP side. Existing handlers already return
clean 400s for malformed bodies; the MCP binary relays them.

### Secret redaction

`Source` and `Notifier` responses include cleartext secrets in the existing
HTTP API (cookie-authed UI use). When the request was authenticated via a
**bearer token** (i.e. came from the MCP binary), the server replaces
secret-bearing fields with `"***"` before serializing. This keeps secrets
from reaching AI clients while preserving existing UI behavior. The
relevant fields:

- `Source.secret` — the per-source token used to authenticate inbound webhooks
- `Notifier.config.token` (and equivalents per kind: bot tokens, webhook
  URLs, etc.)

Implementation note: redaction lives in the response serialization path,
keyed off a request-extension flag set by `require_auth` when the
credential was a bearer token.

### Logging

- MCP binary logs to **stderr** (stdout is the MCP protocol). `tracing`
  with env-filter, default `info`.
- Each tool call logs: tool name, redacted args (token never logged), HTTP
  status, latency. Errors include the response body.
- Server-side logging unchanged. The `last_used_at` field on `api_tokens`
  gives a coarse audit trail visible in the UI.

### Metrics

The MCP binary does not export Prometheus (short-lived stdio process). The
server's `/metrics` already counts API hits; MCP traffic shows up there
naturally. Per-token counters are out of scope for v1.

### Cancellation / timeouts

- HTTP timeout per tool call: `TRANSCODERR_TIMEOUT_SECS`, default 30s.
  Quick ops finish well under this; `dry_run_flow` is the only CPU-bound
  call and is <1s in practice.
- If the AI client disconnects mid-call, rmcp drops the in-flight request.
  The HTTP request continues to completion server-side — we do not
  propagate cancellation through reqwest. A successful `create_flow`
  whose result is dropped is recoverable via `list_flows`.

## 6. Distribution

### Build artifacts

`cargo build --release` from the workspace produces two binaries:

- `transcoderr` (unchanged, the server)
- `transcoderr-mcp` (new, stdio MCP proxy)

### Release pipeline

The existing GitHub Actions release workflow already cross-compiles static
binaries for `linux-amd64`, `linux-arm64`, `darwin-arm64`. The matrix is
extended to also build `transcoderr-mcp` for the same three targets.
v0.7.0 ships six binaries instead of three.

### Docker images

`transcoderr-mcp` is **not** added to the existing image flavors. AI
clients run the MCP binary on the client machine, not in the server
container. A standalone `transcoderr-mcp` image is YAGNI for v1; revisit if
asked.

### Configuration

Three env vars, no config file:

| var                         | required | default | purpose                                 |
| --------------------------- | -------- | ------- | --------------------------------------- |
| `TRANSCODERR_URL`           | yes      | —       | base URL, e.g. `http://192.168.1.176:8099` |
| `TRANSCODERR_TOKEN`         | yes      | —       | bearer token from Settings → API tokens |
| `TRANSCODERR_TIMEOUT_SECS`  | no       | `30`    | per-call HTTP timeout                   |

CLI flags mirror these (`--url`, `--token`, `--timeout-secs`) and override
env vars when present.

### Sample Claude Desktop config

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

### Startup

On launch, `transcoderr-mcp`:

1. Validates env (fail-fast with a clear stderr error if missing).
2. Issues `GET /healthz` to confirm reachability and that the token works.
   Failure → exit 1 before announcing MCP capabilities.
3. Builds the rmcp server, registers all tools with their schemas, serves
   on stdio.

### Versioning

The MCP binary embeds the workspace version (`env!("CARGO_PKG_VERSION")`)
and exposes it via the standard MCP `serverInfo`. If `/healthz` reports a
different server version, log a warning to stderr and continue. Major
drift surfaces as `NOT_FOUND` errors on missing endpoints.

## 7. Repo layout

```
transcoderr/
├── Cargo.toml                     # [workspace], not [package]
├── crates/
│   ├── transcoderr/               # existing crate, moved verbatim
│   │   ├── Cargo.toml
│   │   ├── src/...
│   │   └── migrations/
│   ├── transcoderr-api-types/     # NEW — serde + schemars only
│   │   ├── Cargo.toml
│   │   └── src/lib.rs
│   └── transcoderr-mcp/           # NEW — stdio MCP binary
│       ├── Cargo.toml             # rmcp + reqwest + tokio + the types crate
│       └── src/main.rs
├── docs/
└── web/
```

### `transcoderr-api-types` content

Pure data, no logic:

- `RunSummary`, `RunDetail`, `RunEvent`, `RunStatus`
- `Flow`, `FlowSummary`, `CreateFlowRequest`, `UpdateFlowRequest`
- `Source`, `CreateSourceRequest`, `SourceKind`
- `Notifier`, `CreateNotifierRequest`, `NotifierKind` + per-kind config structs
- `ApiError`, `Health`, `HwCaps`

The existing `transcoderr` crate switches its handlers to use these types.
Net zero behavior change; the structs now live in one place and derive
`JsonSchema`. Folded into the implementation plan as a one-time refactor.

### `transcoderr-mcp` content

- `main.rs` — env parsing, healthz check, rmcp setup
- `client.rs` — thin reqwest wrapper that sets the bearer header and
  translates HTTP errors into `ApiError`
- `tools/` — one module per resource (`runs.rs`, `flows.rs`, `sources.rs`,
  `notifiers.rs`, `system.rs`); each registers its tools with rmcp
- `redact.rs` — helper used in tests and (defensively) at the response
  boundary; primary redaction lives server-side

## 8. Testing

| layer                         | test type                | covers                                                                      |
| ----------------------------- | ------------------------ | --------------------------------------------------------------------------- |
| `transcoderr-api-types`       | round-trip serde tests   | `to_value` + back; ensures schemas wire up                                  |
| `transcoderr` (server)        | existing integration     | unchanged; refactor must not regress                                        |
| `transcoderr` (server)        | new tests                | `api_tokens` CRUD; bearer auth path in `require_auth`; secret redaction     |
| `transcoderr-mcp`             | unit                     | env parsing; HTTP→MCP error mapping; redaction defensive helper             |
| `transcoderr-mcp`             | integration              | spin up `transcoderr serve` on ephemeral port + tempdir DB; seed token via sqlx; drive MCP binary over stdio; happy path: `list_runs → create_flow → dry_run_flow` |

`serial_test` and `tempfile` are already in dev-dependencies.

### CI

The existing workflow runs `cargo test --workspace` once the workspace
exists; matrix unchanged. Release workflow gets one new artifact-upload
step per target (×3 targets).

## 9. Documentation

- `README.md` — new "MCP server" section with the Claude Desktop config snippet
- `docs/mcp.md` (new) — env-var reference, token creation walkthrough, a
  worked example (e.g. "ask Claude to retry all failed runs from yesterday")
- CHANGELOG / GitHub Release notes for v0.7.0 — call out the new binary

## Open questions resolved during design

- **Use case scope:** full read/write surface, including Sources & Notifiers CRUD.
- **Transport:** standalone stdio binary that proxies to HTTP API (not embedded in `transcoderr serve`).
- **Auth model:** dedicated API tokens in a new DB table (not OAuth, not session reuse).
- **Tool granularity:** ~28 fine-grained tools rather than a few mega-tools.
- **Resources / prompts:** skipped for v1 (tools only).
- **Confirmation on destructive ops:** required only on `delete_*`.
- **Token revocation:** next-request granularity, no expiry in v1.
- **Secret redaction:** server redacts when authed via bearer token.
