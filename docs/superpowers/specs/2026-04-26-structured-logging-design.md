# Structured Logging — Design

**Date:** 2026-04-26
**Branch:** `feature/structured-logging`
**Status:** Draft, pending implementation

## Goal

Switch the workspace's two binaries (`transcoderr` server, `transcoderr-mcp`
proxy) from plain-text-only logs to a runtime-selectable text/JSON output.
The JSON variant targets a Loki + Grafana + Promtail pipeline: flat keys,
`level` / `target` / `message`, and arbitrary structured fields the call
sites already emit (`run_id`, `addr`, `url`, etc.).

Local `cargo run` keeps today's pretty text output. Containerized
deployments opt into JSON via env.

## Non-goals

These are explicitly out of scope for this branch:

- Per-run correlation IDs (`#[instrument]` propagation through the worker
  → ffmpeg → events → notifier).
- MCP tool-call request/response tracing.
- HTTP request-trace layer (`tower_http::trace::TraceLayer`).
- ffmpeg progress-line parsing into `tracing` events.
- Audit of existing call sites for missing structured fields.

Each of these is a follow-up branch with its own design.

## Design

### Module location

A new module `transcoderr_api_types::logging` lives at
`crates/transcoderr-api-types/src/logging.rs`, re-exported as
`pub mod logging` from `lib.rs`.

`transcoderr-api-types` already plays the role of the workspace's shared
utility crate (it currently houses both wire types and the
`json_object_schema` schemars helper). Adding a logging init there avoids
introducing a new crate. If the crate's role grows further it can be
renamed to `transcoderr-common` in a separate change.

### New crate dependencies

`crates/transcoderr-api-types/Cargo.toml` adds three workspace dependencies:

- `tracing`
- `tracing-subscriber` — features `env-filter`, `json`
- `clap` — feature `derive`

All three are already pulled by `transcoderr` and `transcoderr-mcp`, so
adding them to `api-types` does not increase the workspace's compile
footprint.

### Public API

```rust
// transcoderr_api_types::logging

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum LogFormat {
    Text,
    Json,
}

/// Initialize the global tracing subscriber.
///
/// `default_filter` is used when `RUST_LOG` is unset
/// (e.g. `"transcoderr=info,tower_http=info"`).
pub fn init(format: LogFormat, default_filter: &str);
```

That is the entire public surface. No `init_from_env` helper — clap handles
flag → env → default precedence in each binary's `Cli` struct.

### Init implementation

```rust
pub fn init(format: LogFormat, default_filter: &str) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_filter));

    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr);

    match format {
        LogFormat::Text => builder.init(),
        LogFormat::Json => builder
            .json()
            .flatten_event(true)
            .with_current_span(true)
            .with_span_list(false)
            .with_target(true)
            .init(),
    }
}
```

Stderr matches today's behavior. `flatten_event(true)` puts event fields
at the top level (Loki-friendly) rather than nesting them under
`"fields"`.

### CLI integration

In `crates/transcoderr/src/main.rs` and
`crates/transcoderr-mcp/src/main.rs`, each `Cli` struct gains:

```rust
#[arg(long, env = "LOG_FORMAT", value_enum, default_value_t = LogFormat::Text)]
log_format: LogFormat,
```

The existing `tracing_subscriber::fmt()...init()` block is replaced with
one call:

```rust
transcoderr_api_types::logging::init(
    cli.log_format,
    "transcoderr=info,tower_http=info", // server
);
// — or —
transcoderr_api_types::logging::init(cli.log_format, "info"); // mcp
```

Each binary keeps its own default filter — they differ because the server
ships `tower_http` middleware whereas the MCP proxy does not.

### JSON output shape

For the existing call site `tracing::info!(addr = %addr, "serving")` the
emitted line looks like:

```json
{"timestamp":"2026-04-26T06:50:01.234567Z","level":"INFO","message":"serving","addr":"0.0.0.0:8099","target":"transcoderr"}
```

Loki/Promtail can label-extract `level`, `target`, and any application
fields the call site emits, without further processing.

### Dockerfile changes

Add `ENV LOG_FORMAT=json` to each of:

- `docker/Dockerfile.cpu`
- `docker/Dockerfile.intel`
- `docker/Dockerfile.nvidia`
- `docker/Dockerfile.full`

Local `cargo run`, the `transcoderr-mcp` binary in `~/Downloads`, and CI
test runs are unaffected (text output remains the default).

### Testing

One unit test in `transcoderr-api-types/src/logging.rs`:

1. `init_json_does_not_panic` — calls `init(LogFormat::Json, "info")`
   once, emits a `tracing::info!` event, asserts no panic.

Only one test, because `init` sets a process-wide singleton subscriber
and a second call from the same test binary would conflict. The JSON
path is picked because it exercises more code branches; the text path is
exercised every time anyone runs the workspace locally.

Manual verification (`LOG_FORMAT=json cargo run -p transcoderr-mcp -- ...`)
covers the JSON shape end-to-end.

## Acceptance

The branch is ready to merge when:

- Both binaries accept `--log-format=text|json` and the `LOG_FORMAT` env.
- `cargo run -p transcoderr -- serve` and `cargo run -p transcoderr-mcp --`
  produce text output identical to today's.
- `LOG_FORMAT=json cargo run -p transcoderr-mcp -- ...` emits one
  `flatten_event(true)` JSON object per line on stderr.
- All four `docker/Dockerfile.*` files set `ENV LOG_FORMAT=json`.
- `cargo test --workspace` passes.
