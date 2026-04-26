# Structured Logging Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a runtime text/json log-format switch (env `LOG_FORMAT`, flag `--log-format`) to both workspace binaries, with shared init living in `transcoderr_api_types::logging`.

**Architecture:** A new `logging` module in the existing `transcoderr-api-types` crate exposes `LogFormat { Text, Json }` and `init(format, default_filter)`. Each binary's `Cli` gains a clap-derived `log_format` arg; `main.rs` calls `init` once at startup, replacing the inline `tracing_subscriber::fmt()` block. Production Dockerfiles export `LOG_FORMAT=json`.

**Tech Stack:** Rust workspace, `tracing` 0.1, `tracing-subscriber` 0.3 (`env-filter` + `json` features), `clap` 4 (`derive` feature).

**Spec:** `docs/superpowers/specs/2026-04-26-structured-logging-design.md`

---

## File Structure

```
crates/transcoderr-api-types/Cargo.toml          [modify: 3 new deps]
crates/transcoderr-api-types/src/logging.rs      [create: LogFormat + init + test]
crates/transcoderr-api-types/src/lib.rs          [modify: pub mod logging;]
crates/transcoderr-mcp/src/main.rs               [modify: Cli arg + init call]
crates/transcoderr/src/main.rs                   [modify: Cli arg + init call]
Cargo.toml                                        [modify: add clap to workspace.deps; add json feature to tracing-subscriber]
docker/Dockerfile.cpu                             [modify: ENV LOG_FORMAT=json]
docker/Dockerfile.intel                           [modify: ENV LOG_FORMAT=json]
docker/Dockerfile.nvidia                          [modify: ENV LOG_FORMAT=json]
docker/Dockerfile.full                            [modify: ENV LOG_FORMAT=json]
```

The new `logging.rs` is one file because the entire feature is ~30 lines of Rust. Splitting further would be premature.

---

## Task 1: Wire up workspace dependencies

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Modify: `crates/transcoderr-api-types/Cargo.toml`

- [ ] **Step 1: Add `json` feature to workspace `tracing-subscriber` dep**

In `Cargo.toml` (workspace root), find:

```toml
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

Replace with:

```toml
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
```

- [ ] **Step 2: Add `clap` to workspace dependencies**

In `Cargo.toml`, append after the existing `tracing-subscriber` line:

```toml
clap = { version = "4", features = ["derive", "env"] }
```

- [ ] **Step 3: Add three deps to `transcoderr-api-types/Cargo.toml`**

In `crates/transcoderr-api-types/Cargo.toml`, append to `[dependencies]`:

```toml
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
clap = { workspace = true }
```

- [ ] **Step 4: Verify the workspace still compiles**

Run: `cargo build --workspace --locked 2>&1 | tail -10`
Expected: warning about lockfile changes is fine; compile succeeds. If `--locked` rejects the lockfile change, run `cargo build --workspace` (without `--locked`) once to refresh `Cargo.lock`, then re-run with `--locked`.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/transcoderr-api-types/Cargo.toml
git commit -m "build(api-types): add tracing/clap deps for shared logging init"
```

---

## Task 2: Implement `LogFormat` and `init` (TDD)

**Files:**
- Create: `crates/transcoderr-api-types/src/logging.rs`
- Modify: `crates/transcoderr-api-types/src/lib.rs`

- [ ] **Step 1: Wire up the new module in `lib.rs`**

In `crates/transcoderr-api-types/src/lib.rs`, after the existing `use schemars::...` line at the top of the file, add:

```rust
pub mod logging;
```

- [ ] **Step 2: Write the failing test in a new `logging.rs`**

Create `crates/transcoderr-api-types/src/logging.rs` with this content:

```rust
use clap::ValueEnum;

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum LogFormat {
    Text,
    Json,
}

pub fn init(_format: LogFormat, _default_filter: &str) {
    todo!("implemented in Step 4")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_json_does_not_panic() {
        init(LogFormat::Json, "info");
        tracing::info!(test = "smoke", "hello from logging init");
    }
}
```

- [ ] **Step 3: Run the test and watch it fail**

Run: `cargo test -p transcoderr-api-types init_json_does_not_panic 2>&1 | tail -15`
Expected: panic from `todo!("implemented in Step 4")`. The test FAILS.

- [ ] **Step 4: Implement `init` to make the test pass**

Replace the body of `init` in `crates/transcoderr-api-types/src/logging.rs`:

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

- [ ] **Step 5: Run the test and watch it pass**

Run: `cargo test -p transcoderr-api-types init_json_does_not_panic 2>&1 | tail -10`
Expected: `test logging::tests::init_json_does_not_panic ... ok` — the test PASSES.

- [ ] **Step 6: Run the full api-types test suite to confirm no regression**

Run: `cargo test -p transcoderr-api-types 2>&1 | tail -10`
Expected: 3 tests pass total (`api_error_round_trips_through_json`, `api_error_omits_null_details`, `init_json_does_not_panic`).

- [ ] **Step 7: Commit**

```bash
git add crates/transcoderr-api-types/src/logging.rs crates/transcoderr-api-types/src/lib.rs
git commit -m "feat(api-types): logging::init for shared text/json subscriber setup"
```

---

## Task 3: Wire up `transcoderr-mcp`

**Files:**
- Modify: `crates/transcoderr-mcp/src/main.rs:14-26` (Cli struct), `:60-66` (init block)

- [ ] **Step 1: Add the `log_format` field to the `Cli` struct**

In `crates/transcoderr-mcp/src/main.rs`, replace the `Cli` struct:

```rust
#[derive(Parser, Debug, Clone)]
#[command(name = "transcoderr-mcp", version)]
struct Cli {
    /// transcoderr server base URL.
    #[arg(long, env = "TRANSCODERR_URL")]
    url: String,
    /// API token from Settings → API tokens.
    #[arg(long, env = "TRANSCODERR_TOKEN")]
    token: String,
    /// Per-call HTTP timeout, seconds.
    #[arg(long, env = "TRANSCODERR_TIMEOUT_SECS", default_value_t = 30)]
    timeout_secs: u64,
}
```

with:

```rust
#[derive(Parser, Debug, Clone)]
#[command(name = "transcoderr-mcp", version)]
struct Cli {
    /// transcoderr server base URL.
    #[arg(long, env = "TRANSCODERR_URL")]
    url: String,
    /// API token from Settings → API tokens.
    #[arg(long, env = "TRANSCODERR_TOKEN")]
    token: String,
    /// Per-call HTTP timeout, seconds.
    #[arg(long, env = "TRANSCODERR_TIMEOUT_SECS", default_value_t = 30)]
    timeout_secs: u64,
    /// Log output format.
    #[arg(long, env = "LOG_FORMAT", value_enum, default_value_t = transcoderr_api_types::logging::LogFormat::Text)]
    log_format: transcoderr_api_types::logging::LogFormat,
}
```

- [ ] **Step 2: Replace the inline `tracing_subscriber::fmt()...init()` block**

In `crates/transcoderr-mcp/src/main.rs`, replace:

```rust
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "info".into()))
        .with_writer(std::io::stderr)
        .init();
```

with:

```rust
    transcoderr_api_types::logging::init(cli.log_format, "info");
```

- [ ] **Step 3: Build and verify**

Run: `cargo build -p transcoderr-mcp 2>&1 | tail -10`
Expected: clean build, no warnings introduced by these edits.

- [ ] **Step 4: Smoke-test JSON output**

Run: `LOG_FORMAT=json TRANSCODERR_URL=http://192.168.1.176:8099 TRANSCODERR_TOKEN=tcr_ZDnxcfTK5q3rrAANMPzXiTiX0y6nRIKg cargo run -q -p transcoderr-mcp 2>&1 | head -3`

Expected: a single JSON line on stderr containing `"level":"INFO"`, `"message":"transcoderr-mcp starting"`, `"url":"http://192.168.1.176:8099"`, and `"target":"transcoderr_mcp"`. The process will block on stdin waiting for MCP frames — kill it with Ctrl-C after the line is printed.

- [ ] **Step 5: Smoke-test text output (default)**

Run: `TRANSCODERR_URL=http://192.168.1.176:8099 TRANSCODERR_TOKEN=tcr_ZDnxcfTK5q3rrAANMPzXiTiX0y6nRIKg cargo run -q -p transcoderr-mcp 2>&1 | head -3`

Expected: a single text line like `2026-04-26T... INFO transcoderr_mcp: transcoderr-mcp starting url=http://192.168.1.176:8099`. Same behavior as today. Kill with Ctrl-C.

- [ ] **Step 6: Commit**

```bash
git add crates/transcoderr-mcp/src/main.rs
git commit -m "feat(mcp): --log-format flag with shared logging init"
```

---

## Task 4: Wire up `transcoderr` server

**Files:**
- Modify: `crates/transcoderr/src/main.rs:7-21` (Cli struct), `:24-31` (init block)

- [ ] **Step 1: Add the `log_format` field to the top-level `Cli` struct**

In `crates/transcoderr/src/main.rs`, replace the `Cli` struct:

```rust
#[derive(Parser)]
#[command(name = "transcoderr", version)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}
```

with:

```rust
#[derive(Parser)]
#[command(name = "transcoderr", version)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
    /// Log output format.
    #[arg(long, env = "LOG_FORMAT", value_enum, default_value_t = transcoderr_api_types::logging::LogFormat::Text, global = true)]
    log_format: transcoderr_api_types::logging::LogFormat,
}
```

The `global = true` makes `--log-format` valid both before and after the subcommand (`cargo run -p transcoderr -- --log-format json serve` and `cargo run -p transcoderr -- serve --log-format json` both work).

- [ ] **Step 2: Move `Cli::parse()` ahead of the subscriber init and replace the init block**

In `crates/transcoderr/src/main.rs`, replace:

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "transcoderr=info,tower_http=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
```

with:

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    transcoderr_api_types::logging::init(cli.log_format, "transcoderr=info,tower_http=info");
```

(`Cli::parse()` now runs before logging init so we can read `cli.log_format`. clap's `--help` and parse errors go to stderr regardless of subscriber state, so the reorder is safe.)

- [ ] **Step 3: Build and verify**

Run: `cargo build -p transcoderr 2>&1 | tail -10`
Expected: clean build.

- [ ] **Step 4: Smoke-test JSON output**

Run: `LOG_FORMAT=json cargo run -q -p transcoderr -- serve --config /nonexistent/config.toml 2>&1 | head -1`

Expected: a JSON line. The server will exit non-zero with a config-not-found error after printing — that's fine, we only need to confirm the format. The line should contain `"level":"INFO"` or `"level":"ERROR"` and the typical event fields.

(If the server doesn't emit any startup log before failing on the missing config, this step may produce no log output. In that case re-run with a valid config path that exists in the dev environment, or skip — Task 6's `cargo test --workspace` will exercise the init path.)

- [ ] **Step 5: Commit**

```bash
git add crates/transcoderr/src/main.rs
git commit -m "feat(server): --log-format flag with shared logging init"
```

---

## Task 5: Dockerfiles export `LOG_FORMAT=json`

**Files:**
- Modify: `docker/Dockerfile.cpu`, `docker/Dockerfile.intel`, `docker/Dockerfile.nvidia`, `docker/Dockerfile.full`

- [ ] **Step 1: Add `ENV LOG_FORMAT=json` to `Dockerfile.cpu`**

In `docker/Dockerfile.cpu`, find the runtime stage's `WORKDIR /app` line (around line 23) and insert immediately after it:

```dockerfile
ENV LOG_FORMAT=json
```

- [ ] **Step 2: Add `ENV LOG_FORMAT=json` to `Dockerfile.intel`**

In `docker/Dockerfile.intel`, find the runtime stage's `WORKDIR /app` line (around line 24) and insert immediately after it:

```dockerfile
ENV LOG_FORMAT=json
```

- [ ] **Step 3: Add `ENV LOG_FORMAT=json` to `Dockerfile.nvidia`**

In `docker/Dockerfile.nvidia`, find the runtime stage's `WORKDIR /app` line (around line 19) and insert immediately after it:

```dockerfile
ENV LOG_FORMAT=json
```

- [ ] **Step 4: Add `ENV LOG_FORMAT=json` to `Dockerfile.full`**

In `docker/Dockerfile.full`, find the runtime stage's `WORKDIR /app` line (around line 22) and insert immediately after it:

```dockerfile
ENV LOG_FORMAT=json
```

- [ ] **Step 5: Verify all four files have the line**

Run from the repo root: `grep -n "LOG_FORMAT" docker/Dockerfile.{cpu,intel,nvidia,full}`
Expected: four lines, one per file, each showing `ENV LOG_FORMAT=json`.

- [ ] **Step 6: Commit**

```bash
git add docker/Dockerfile.cpu docker/Dockerfile.intel docker/Dockerfile.nvidia docker/Dockerfile.full
git commit -m "build(docker): default to JSON logs in container images"
```

---

## Task 6: Workspace verification

**Files:** none (verification only)

- [ ] **Step 1: Run the full workspace test suite**

Run: `cargo test --workspace --locked 2>&1 | tail -20`
Expected: all crates compile and all tests pass. Specifically watch for `init_json_does_not_panic ... ok` in the api-types section.

- [ ] **Step 2: Confirm acceptance criteria from the spec**

Cross off each acceptance item from `docs/superpowers/specs/2026-04-26-structured-logging-design.md`:

- [ ] Both binaries accept `--log-format=text|json` and the `LOG_FORMAT` env. — verified by Tasks 3 & 4 smoke tests.
- [ ] `cargo run -p transcoderr -- serve` and `cargo run -p transcoderr-mcp` produce text output identical to today's. — verified by Task 3 Step 5.
- [ ] `LOG_FORMAT=json cargo run -p transcoderr-mcp` emits one `flatten_event(true)` JSON object per line on stderr. — verified by Task 3 Step 4.
- [ ] All four `docker/Dockerfile.*` files set `ENV LOG_FORMAT=json`. — verified by Task 5 Step 5.
- [ ] `cargo test --workspace` passes. — verified by Step 1 above.

If any item fails, return to the relevant task to fix.

- [ ] **Step 3: (No commit — verification only.)**

The branch is ready for review/merge. `git log --oneline feature/structured-logging ^main` should show:

```
build(api-types): add tracing/clap deps for shared logging init
feat(api-types): logging::init for shared text/json subscriber setup
feat(mcp): --log-format flag with shared logging init
feat(server): --log-format flag with shared logging init
build(docker): default to JSON logs in container images
```

(plus the spec commit `docs(spec): structured logging design` from before this plan was written).
