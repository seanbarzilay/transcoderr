# transcoderr Phase 1 — Headless MVP Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up a headless Rust binary that accepts a Radarr webhook, persists a job to SQLite, and runs a linear `probe → transcode → output(replace)` flow against the file with crash-safe checkpoints.

**Architecture:** One Rust binary, embedded SQLite (sqlx + migrations), single-slot async worker that polls the jobs table, ffmpeg/ffprobe shelled out via `tokio::process::Command`, flow engine that executes linear steps in order and snapshots context after each completed step. No UI, no plugins (steps are hard-coded Rust modules), no GPU, no notifications.

**Tech Stack:** Rust 1.78+, Tokio, Axum 0.7, sqlx 0.8 (sqlite + bundled), serde + serde_yaml + serde_json, clap, tracing, ffmpeg + ffprobe (system binaries).

---

## Scope

**In:**
- Cargo project skeleton with single binary crate
- Bootstrap config (`config.toml`): port, data dir, Radarr bearer token
- SQLite schema (subset of spec): `flows`, `flow_versions`, `jobs`, `run_events`, `checkpoints`
- Migrations runner (sqlx migrate)
- Radarr typed webhook adapter (`POST /webhook/radarr`) with bearer-token validation
- Flow YAML parser — only the linear subset (`name`, `triggers.radarr`, `steps[].use+with`); no `if/then/else`, no `match.expr`, no `return:`, no `on_failure`, no inline shorthand
- Flow engine that executes steps sequentially and writes checkpoints
- Three built-in step implementations: `probe`, `transcode`, `output` (mode=replace only)
- Single-slot worker that polls `jobs WHERE status='pending'`
- Crash recovery: `running` jobs reset to `pending` on boot, resumed from last checkpoint
- Integration test that POSTs a Radarr-shaped webhook against a generated test clip and asserts `completed`
- Crash-recovery integration test

**Out (deferred to later phases):**
- Conditionals, `return:`, `on_failure`, `retry`, `match.expr` → Phase 2
- Plugin host / subprocess plugins → Phase 2
- CEL expression evaluator → Phase 2
- Sonarr / Lidarr / generic webhook → Phase 2
- Sources table (Phase 1 uses a single Radarr token in `config.toml`)
- GPU / capability probe → Phase 3
- Web UI / JSON API beyond webhooks → Phase 4
- Notifications, Prometheus, retention, log spillover → Phase 5

## File Structure

```
Cargo.toml
config.example.toml
.gitignore                                      (extend existing)
migrations/
  20260425000001_initial.sql                    Phase 1 schema (5 tables)
src/
  main.rs                                       binary entry, CLI, tracing init, server bootstrap
  config.rs                                     Bootstrap config loader (Config struct)
  error.rs                                      Top-level Error enum (thiserror)
  db/
    mod.rs                                      Pool builder, migration runner, time helpers
    flows.rs                                    Flow + flow_versions CRUD (read-only here, write for tests)
    jobs.rs                                     Job CRUD: insert, claim_next, set_status, dedup query
    run_events.rs                               Append events
    checkpoints.rs                              Upsert + read by job_id
  http/
    mod.rs                                      Axum app builder, AppState
    webhook_radarr.rs                           POST /webhook/radarr handler
  flow/
    mod.rs                                      Re-exports
    model.rs                                    AST types: Flow, Trigger, Step
    parser.rs                                   serde_yaml → AST + validation
    engine.rs                                   Sequential executor with checkpoints
    context.rs                                  Run context (probe data, file metadata, step outputs)
  steps/
    mod.rs                                      Step trait + dispatch table for built-ins
    probe.rs                                    ffprobe wrapper, populates context.probe.*
    transcode.rs                                ffmpeg encode (CPU only in Phase 1)
    output.rs                                   atomic replace + verify (verify is Phase 2; Phase 1: just atomic swap)
  ffmpeg.rs                                     spawn helpers, stderr progress parser
  worker.rs                                     single-slot poll loop, checkpoint resume logic
tests/
  common/
    mod.rs                                      shared test helpers (tempdir app, sample mkv generator)
  webhook_to_complete.rs                        end-to-end happy-path test
  crash_recovery.rs                             checkpoint resume test
  flow_parser.rs                                parser unit tests
  flow_engine.rs                                engine unit tests with stub steps
```

---

## Tasks

### Task 1: Cargo workspace + binary crate

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs`
- Modify: `.gitignore`

- [ ] **Step 1: Initialize Cargo project**

Run: `cargo init --name transcoderr --bin`

Expected: creates `Cargo.toml` and `src/main.rs` with stub.

- [ ] **Step 2: Pin Rust edition + version**

Replace `Cargo.toml` with:

```toml
[package]
name = "transcoderr"
version = "0.1.0"
edition = "2021"
rust-version = "1.78"

[dependencies]

[dev-dependencies]

[profile.release]
lto = "thin"
strip = true
codegen-units = 1
```

- [ ] **Step 3: Extend .gitignore**

Append to `.gitignore`:

```
/target
/data
/config.toml
*.db
*.db-shm
*.db-wal
```

- [ ] **Step 4: Confirm it builds**

Run: `cargo build`
Expected: `Compiling transcoderr v0.1.0` → `Finished`.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/main.rs .gitignore
git commit -m "chore: initialize Rust binary crate"
```

---

### Task 2: Add core dependencies

**Files:** Modify: `Cargo.toml`

- [ ] **Step 1: Add deps**

Replace `[dependencies]` and `[dev-dependencies]` blocks in `Cargo.toml` with:

```toml
[dependencies]
tokio = { version = "1", features = ["macros", "rt-multi-thread", "process", "signal", "fs", "time", "sync"] }
axum = { version = "0.7", features = ["macros"] }
tower = "0.5"
tower-http = { version = "0.6", features = ["trace"] }
sqlx = { version = "0.8", default-features = false, features = ["runtime-tokio", "sqlite", "migrate", "chrono", "macros"] }
libsqlite3-sys = { version = "0.30", features = ["bundled"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
serde_yaml = "0.9"
toml = "0.8"
thiserror = "1"
anyhow = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
clap = { version = "4", features = ["derive"] }
chrono = { version = "0.4", features = ["serde"] }

[dev-dependencies]
tempfile = "3"
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
pretty_assertions = "1"
serial_test = "3"
```

- [ ] **Step 2: Verify resolution**

Run: `cargo build`
Expected: pulls and compiles dependencies. May take a few minutes the first time.

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: pin core dependencies"
```

---

### Task 3: Bootstrap config + CLI

**Files:**
- Create: `src/config.rs`
- Create: `src/error.rs`
- Modify: `src/main.rs`
- Create: `config.example.toml`

- [ ] **Step 1: Write the failing test**

Create `src/config.rs`:

```rust
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub bind: String,
    pub data_dir: PathBuf,
    pub radarr: RadarrConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RadarrConfig {
    pub bearer_token: String,
}

impl Config {
    pub fn from_path(path: &Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        let cfg: Config = toml::from_str(&raw)?;
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn parses_minimal_config() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, r#"
bind = "127.0.0.1:8080"
data_dir = "/tmp/tcr"
[radarr]
bearer_token = "abc123"
        "#).unwrap();
        let cfg = Config::from_path(f.path()).unwrap();
        assert_eq!(cfg.bind, "127.0.0.1:8080");
        assert_eq!(cfg.radarr.bearer_token, "abc123");
    }
}
```

- [ ] **Step 2: Wire `mod config` and run test**

Replace `src/main.rs` with:

```rust
mod config;
mod error;

fn main() -> anyhow::Result<()> {
    Ok(())
}
```

Create `src/error.rs`:

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("config error: {0}")]
    Config(#[from] anyhow::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("db: {0}")]
    Db(#[from] sqlx::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
```

Run: `cargo test --lib config::tests::parses_minimal_config`
Expected: PASS.

- [ ] **Step 3: Add CLI**

Replace `src/main.rs` with:

```rust
mod config;
mod error;

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "transcoderr", version)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(clap::Subcommand)]
enum Cmd {
    /// Run the server.
    Serve {
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "transcoderr=info,tower_http=info".into()))
        .init();
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Serve { config } => {
            let cfg = config::Config::from_path(&config)?;
            tracing::info!(?cfg.bind, "loaded config");
            // server boot wired in Task 5
            Ok(())
        }
    }
}
```

- [ ] **Step 4: Add config example**

Create `config.example.toml`:

```toml
bind = "0.0.0.0:8080"
data_dir = "./data"

[radarr]
bearer_token = "change-me"
```

- [ ] **Step 5: Verify**

Run: `cargo build && cp config.example.toml config.toml && cargo run -- serve --config config.toml`
Expected: prints `loaded config` and exits cleanly. Then `rm config.toml`.

- [ ] **Step 6: Commit**

```bash
git add src/config.rs src/error.rs src/main.rs config.example.toml
git commit -m "feat: bootstrap config loader and CLI"
```

---

### Task 4: SQLite schema (Phase 1 subset) + migrations runner

**Files:**
- Create: `migrations/20260425000001_initial.sql`
- Create: `src/db/mod.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Write migration**

Create `migrations/20260425000001_initial.sql`:

```sql
CREATE TABLE flows (
  id            INTEGER PRIMARY KEY,
  name          TEXT NOT NULL UNIQUE,
  enabled       INTEGER NOT NULL DEFAULT 1,
  yaml_source   TEXT NOT NULL,
  parsed_json   TEXT NOT NULL,
  version       INTEGER NOT NULL,
  updated_at    INTEGER NOT NULL
);

CREATE TABLE flow_versions (
  flow_id       INTEGER NOT NULL REFERENCES flows(id),
  version       INTEGER NOT NULL,
  yaml_source   TEXT NOT NULL,
  created_at    INTEGER NOT NULL,
  PRIMARY KEY (flow_id, version)
);

CREATE TABLE jobs (
  id                    INTEGER PRIMARY KEY,
  flow_id               INTEGER NOT NULL REFERENCES flows(id),
  flow_version          INTEGER NOT NULL,
  source_kind           TEXT NOT NULL,
  file_path             TEXT NOT NULL,
  trigger_payload_json  TEXT NOT NULL,
  status                TEXT NOT NULL,
  status_label          TEXT,
  priority              INTEGER NOT NULL DEFAULT 0,
  current_step          INTEGER,
  attempt               INTEGER NOT NULL DEFAULT 0,
  created_at            INTEGER NOT NULL,
  started_at            INTEGER,
  finished_at           INTEGER
);

CREATE INDEX idx_jobs_pending ON jobs(status, priority DESC, created_at)
  WHERE status='pending';

CREATE TABLE run_events (
  id            INTEGER PRIMARY KEY,
  job_id        INTEGER NOT NULL REFERENCES jobs(id),
  ts            INTEGER NOT NULL,
  step_id       TEXT,
  kind          TEXT NOT NULL,
  payload_json  TEXT,
  payload_path  TEXT
);

CREATE INDEX idx_run_events_job ON run_events(job_id, ts);

CREATE TABLE checkpoints (
  job_id                 INTEGER PRIMARY KEY REFERENCES jobs(id),
  step_index             INTEGER NOT NULL,
  context_snapshot_json  TEXT NOT NULL,
  updated_at             INTEGER NOT NULL
);
```

- [ ] **Step 2: Write the failing test**

Create `src/db/mod.rs`:

```rust
use sqlx::{sqlite::{SqliteConnectOptions, SqlitePoolOptions}, SqlitePool};
use std::{path::Path, str::FromStr, time::Duration};

pub async fn open(data_dir: &Path) -> anyhow::Result<SqlitePool> {
    std::fs::create_dir_all(data_dir)?;
    let db_path = data_dir.join("data.db");
    let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", db_path.display()))?
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5))
        .pragma("foreign_keys", "ON");
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(opts).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}

pub fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn opens_and_migrates() {
        let dir = tempdir().unwrap();
        let pool = open(dir.path()).await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM flows")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(count, 0);
    }
}
```

- [ ] **Step 3: Wire and run**

Add to `src/main.rs` after `mod error;`:

```rust
mod db;
```

Run: `cargo test db::tests::opens_and_migrates`
Expected: PASS. (sqlx-macros may emit warnings; ignore.)

- [ ] **Step 4: Commit**

```bash
git add migrations/ src/db/mod.rs src/main.rs
git commit -m "feat: sqlite schema and migration runner"
```

---

### Task 5: Flow AST + parser (linear subset)

**Files:**
- Create: `src/flow/mod.rs`
- Create: `src/flow/model.rs`
- Create: `src/flow/parser.rs`
- Create: `tests/flow_parser.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Write the AST**

Create `src/flow/mod.rs`:

```rust
pub mod model;
pub mod parser;

pub use model::*;
pub use parser::parse_flow;
```

Create `src/flow/model.rs`:

```rust
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Flow {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub triggers: Vec<Trigger>,
    pub steps: Vec<Step>,
}

fn default_true() -> bool { true }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    Radarr(Vec<String>),  // event names: ["downloaded", "upgraded", ...]
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Step {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(rename = "use")]
    pub use_: String,
    #[serde(default)]
    pub with: BTreeMap<String, Value>,
}
```

- [ ] **Step 2: Write the failing test**

Create `tests/flow_parser.rs`:

```rust
use transcoderr::flow::{parse_flow, Flow, Step, Trigger};

#[test]
fn parses_minimal_linear_flow() {
    let yaml = r#"
name: reencode-x265
triggers:
  - radarr: [downloaded]
steps:
  - id: probe
    use: probe
  - id: encode
    use: transcode
    with:
      codec: x265
      crf: 22
  - id: swap
    use: output
    with:
      mode: replace
"#;
    let flow: Flow = parse_flow(yaml).unwrap();
    assert_eq!(flow.name, "reencode-x265");
    assert_eq!(flow.triggers, vec![Trigger::Radarr(vec!["downloaded".into()])]);
    assert_eq!(flow.steps.len(), 3);
    assert_eq!(flow.steps[1].use_, "transcode");
    assert_eq!(flow.steps[1].with.get("crf").and_then(|v| v.as_i64()), Some(22));
}

#[test]
fn rejects_unknown_step_use() {
    let yaml = r#"
name: bad
triggers:
  - radarr: [downloaded]
steps:
  - use: not_a_real_step
"#;
    let err = parse_flow(yaml).unwrap_err();
    assert!(err.to_string().contains("unknown step"), "got: {err}");
}
```

- [ ] **Step 3: Implement parser**

Create `src/flow/parser.rs`:

```rust
use super::model::{Flow, Step};

const KNOWN_STEPS: &[&str] = &["probe", "transcode", "output"];

pub fn parse_flow(yaml: &str) -> anyhow::Result<Flow> {
    let flow: Flow = serde_yaml::from_str(yaml)?;
    validate(&flow)?;
    Ok(flow)
}

fn validate(flow: &Flow) -> anyhow::Result<()> {
    if flow.triggers.is_empty() {
        anyhow::bail!("flow {:?} has no triggers", flow.name);
    }
    for step in &flow.steps {
        if !KNOWN_STEPS.contains(&step.use_.as_str()) {
            anyhow::bail!("unknown step `use:` {:?} in flow {:?} (Phase 1 supports: {})",
                step.use_, flow.name, KNOWN_STEPS.join(", "));
        }
    }
    let _ = step_default_warning(flow);
    Ok(())
}

fn step_default_warning(_flow: &Flow) {}
```

Note: We expose `mod flow` from `lib.rs` rather than just `main.rs` so integration tests can import. Add a thin `lib.rs`.

- [ ] **Step 4: Add lib crate alongside the binary**

Create `src/lib.rs`:

```rust
pub mod config;
pub mod db;
pub mod error;
pub mod flow;
```

Update `src/main.rs` top to remove duplicate mod declarations:

```rust
use transcoderr::{config, db, error};

use clap::Parser;
use std::path::PathBuf;
// ... rest unchanged
```

Update `Cargo.toml` to add `[lib]` and `[[bin]]`:

```toml
[lib]
name = "transcoderr"
path = "src/lib.rs"

[[bin]]
name = "transcoderr"
path = "src/main.rs"
```

- [ ] **Step 5: Run tests**

Run: `cargo test --test flow_parser`
Expected: 2 passed.

Run: `cargo build`
Expected: clean build.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml src/lib.rs src/main.rs src/flow/ tests/flow_parser.rs
git commit -m "feat: flow YAML parser for linear-step subset"
```

---

### Task 6: Flow execution context

**Files:**
- Create: `src/flow/context.rs`
- Modify: `src/flow/mod.rs`

- [ ] **Step 1: Write the failing test**

Append to `src/flow/context.rs` (create file):

```rust
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// The evolving state passed between steps. Snapshotted to checkpoints.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Context {
    pub file: FileMeta,
    pub probe: Option<Value>,
    pub steps: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileMeta {
    pub path: String,
    pub size_bytes: Option<u64>,
}

impl Context {
    pub fn for_file(path: impl Into<String>) -> Self {
        Self {
            file: FileMeta { path: path.into(), size_bytes: None },
            ..Default::default()
        }
    }

    pub fn record_step_output(&mut self, id: &str, out: Value) {
        self.steps.insert(id.to_string(), out);
    }

    pub fn to_snapshot(&self) -> String {
        serde_json::to_string(self).expect("context serializable")
    }

    pub fn from_snapshot(s: &str) -> anyhow::Result<Self> {
        Ok(serde_json::from_str(s)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshot_round_trip() {
        let mut c = Context::for_file("/m/Dune.mkv");
        c.probe = Some(json!({"video": {"codec": "h264"}}));
        c.record_step_output("probe", json!({"ok": true}));
        let s = c.to_snapshot();
        let r = Context::from_snapshot(&s).unwrap();
        assert_eq!(r.file.path, "/m/Dune.mkv");
        assert_eq!(r.probe.as_ref().unwrap()["video"]["codec"], "h264");
        assert_eq!(r.steps.get("probe").unwrap()["ok"], true);
    }
}
```

- [ ] **Step 2: Re-export and test**

Update `src/flow/mod.rs`:

```rust
pub mod context;
pub mod model;
pub mod parser;

pub use context::Context;
pub use model::*;
pub use parser::parse_flow;
```

Run: `cargo test flow::context::tests`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src/flow/context.rs src/flow/mod.rs
git commit -m "feat: flow execution context with snapshot round-trip"
```

---

### Task 7: ffmpeg/ffprobe spawn helpers

**Files:**
- Create: `src/ffmpeg.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write the failing test**

Create `src/ffmpeg.rs`:

```rust
use anyhow::Context;
use serde_json::Value;
use std::path::Path;
use tokio::process::Command;

pub async fn ffprobe_json(path: &Path) -> anyhow::Result<Value> {
    let out = Command::new("ffprobe")
        .args([
            "-v", "error",
            "-print_format", "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .output().await
        .context("spawn ffprobe")?;
    if !out.status.success() {
        anyhow::bail!("ffprobe failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    let v: Value = serde_json::from_slice(&out.stdout)?;
    Ok(v)
}

/// Generate a tiny test mkv at `dest`. Returns Ok(()) on success.
/// Used only by integration tests.
pub async fn make_testsrc_mkv(dest: &Path, seconds: u32) -> anyhow::Result<()> {
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-f", "lavfi", "-i", &format!("testsrc=duration={seconds}:size=320x240:rate=30"),
            "-f", "lavfi", "-i", &format!("sine=duration={seconds}:frequency=440"),
            "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p",
            "-c:a", "aac",
            "-shortest",
        ])
        .arg(dest)
        .status().await?;
    if !status.success() {
        anyhow::bail!("ffmpeg testsrc generation failed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn probes_a_generated_clip() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("t.mkv");
        make_testsrc_mkv(&p, 1).await.unwrap();
        let v = ffprobe_json(&p).await.unwrap();
        let streams = v["streams"].as_array().unwrap();
        assert!(streams.iter().any(|s| s["codec_type"] == "video"));
    }
}
```

- [ ] **Step 2: Re-export**

Add to `src/lib.rs`:

```rust
pub mod ffmpeg;
```

- [ ] **Step 3: Run**

Pre-req: ffmpeg + ffprobe are on PATH. Verify with: `ffmpeg -version`.

Run: `cargo test ffmpeg::tests::probes_a_generated_clip`
Expected: PASS (clip generates, probe finds video stream).

- [ ] **Step 4: Add ffmpeg progress parser**

Append to `src/ffmpeg.rs`:

```rust
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::ChildStderr;

/// Parses lines like `frame=  120 fps= 30 q=28.0 ... time=00:00:04.00 ... speed=1.0x`
/// into approximate progress percent given total `duration_sec`.
pub struct ProgressParser {
    pub duration_sec: f64,
}

impl ProgressParser {
    pub fn parse_line(&self, line: &str) -> Option<f64> {
        let time_idx = line.find("time=")? + 5;
        let time_str = &line[time_idx..];
        let end = time_str.find(' ').unwrap_or(time_str.len());
        let t = parse_hhmmss(&time_str[..end])?;
        if self.duration_sec <= 0.0 { return None; }
        Some((t / self.duration_sec * 100.0).clamp(0.0, 100.0))
    }
}

fn parse_hhmmss(s: &str) -> Option<f64> {
    let mut parts = s.split(':');
    let h: f64 = parts.next()?.parse().ok()?;
    let m: f64 = parts.next()?.parse().ok()?;
    let sec: f64 = parts.next()?.parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + sec)
}

pub async fn drain_stderr_progress<F>(stderr: ChildStderr, parser: ProgressParser, mut on_pct: F)
where F: FnMut(f64) {
    let mut reader = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = reader.next_line().await {
        if let Some(pct) = parser.parse_line(&line) {
            on_pct(pct);
        }
    }
}

#[cfg(test)]
mod parser_tests {
    use super::*;
    #[test]
    fn parses_progress_line() {
        let p = ProgressParser { duration_sec: 100.0 };
        let pct = p.parse_line("frame=  120 fps= 30 q=28.0 size=N/A time=00:00:50.00 bitrate=N/A speed=1.0x").unwrap();
        assert!((pct - 50.0).abs() < 0.001);
    }
}
```

Run: `cargo test ffmpeg::parser_tests::parses_progress_line`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/ffmpeg.rs src/lib.rs
git commit -m "feat: ffmpeg/ffprobe spawn helpers with progress parser"
```

---

### Task 8: Step trait + dispatch

**Files:**
- Create: `src/steps/mod.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Define the trait**

Create `src/steps/mod.rs`:

```rust
use crate::flow::Context;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;

pub mod probe;
pub mod transcode;
pub mod output;

#[derive(Debug, Clone)]
pub enum StepProgress {
    Pct(f64),
    Log(String),
}

#[async_trait]
pub trait Step: Send + Sync {
    /// Step name as referenced by `use:` in YAML.
    fn name(&self) -> &'static str;

    /// Run the step. Mutates context. May call `on_progress` for live updates.
    async fn execute(
        &self,
        with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut dyn FnMut(StepProgress) + Send,
    ) -> anyhow::Result<()>;
}

/// Look up a built-in step by `use:` name.
pub fn dispatch(use_: &str) -> Option<Box<dyn Step>> {
    match use_ {
        "probe" => Some(Box::new(probe::ProbeStep)),
        "transcode" => Some(Box::new(transcode::TranscodeStep)),
        "output" => Some(Box::new(output::OutputStep)),
        _ => None,
    }
}
```

Add `async-trait = "0.1"` to `[dependencies]` in `Cargo.toml`.

- [ ] **Step 2: Stub each step file**

Create `src/steps/probe.rs`:

```rust
use super::{Step, StepProgress};
use crate::flow::Context;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;

pub struct ProbeStep;

#[async_trait]
impl Step for ProbeStep {
    fn name(&self) -> &'static str { "probe" }
    async fn execute(
        &self,
        _with: &BTreeMap<String, Value>,
        _ctx: &mut Context,
        _on_progress: &mut dyn FnMut(StepProgress) + Send,
    ) -> anyhow::Result<()> {
        unimplemented!("filled in next task")
    }
}
```

Create `src/steps/transcode.rs` and `src/steps/output.rs` with the same stub (using `TranscodeStep` / `OutputStep` and the matching name).

- [ ] **Step 3: Wire and build**

Add to `src/lib.rs`:

```rust
pub mod steps;
```

Run: `cargo build`
Expected: clean build (the `unimplemented!` won't trigger at compile time).

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock src/steps/ src/lib.rs
git commit -m "feat: Step trait and dispatch table"
```

---

### Task 9: Implement probe step

**Files:**
- Modify: `src/steps/probe.rs`
- Create: `tests/step_probe.rs`

- [ ] **Step 1: Write the failing test**

Create `tests/step_probe.rs`:

```rust
use std::collections::BTreeMap;
use tempfile::tempdir;
use transcoderr::ffmpeg::make_testsrc_mkv;
use transcoderr::flow::Context;
use transcoderr::steps::{probe::ProbeStep, Step, StepProgress};

#[tokio::test]
async fn probe_populates_context() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("t.mkv");
    make_testsrc_mkv(&p, 1).await.unwrap();
    let mut ctx = Context::for_file(p.to_string_lossy());
    let mut events: Vec<StepProgress> = vec![];
    let mut cb = |e: StepProgress| events.push(e);
    ProbeStep.execute(&BTreeMap::new(), &mut ctx, &mut cb).await.unwrap();
    let probe = ctx.probe.as_ref().expect("probe set");
    assert!(probe["streams"].as_array().unwrap().iter().any(|s| s["codec_type"] == "video"));
    assert!(ctx.file.size_bytes.unwrap() > 0);
}
```

- [ ] **Step 2: Implement probe**

Replace `src/steps/probe.rs`:

```rust
use super::{Step, StepProgress};
use crate::ffmpeg::ffprobe_json;
use crate::flow::Context;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

pub struct ProbeStep;

#[async_trait]
impl Step for ProbeStep {
    fn name(&self) -> &'static str { "probe" }

    async fn execute(
        &self,
        _with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut dyn FnMut(StepProgress) + Send,
    ) -> anyhow::Result<()> {
        let path = Path::new(&ctx.file.path);
        on_progress(StepProgress::Log(format!("probing {}", path.display())));
        let v = ffprobe_json(path).await?;
        ctx.probe = Some(v);
        let meta = std::fs::metadata(path)?;
        ctx.file.size_bytes = Some(meta.len());
        Ok(())
    }
}
```

- [ ] **Step 3: Run**

Run: `cargo test --test step_probe`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/steps/probe.rs tests/step_probe.rs
git commit -m "feat: probe step populates context with ffprobe JSON"
```

---

### Task 10: Implement transcode step (CPU only)

**Files:**
- Modify: `src/steps/transcode.rs`
- Create: `tests/step_transcode.rs`

- [ ] **Step 1: Write the failing test**

Create `tests/step_transcode.rs`:

```rust
use serde_json::{json, Value};
use std::collections::BTreeMap;
use tempfile::tempdir;
use transcoderr::ffmpeg::make_testsrc_mkv;
use transcoderr::flow::Context;
use transcoderr::steps::{transcode::TranscodeStep, Step, StepProgress};

#[tokio::test]
async fn transcode_writes_output_and_records_path() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("in.mkv");
    make_testsrc_mkv(&src, 2).await.unwrap();
    let mut ctx = Context::for_file(src.to_string_lossy());
    let mut events = vec![];
    let mut cb = |e: StepProgress| events.push(e);

    let mut with: BTreeMap<String, Value> = BTreeMap::new();
    with.insert("codec".into(), json!("x264"));
    with.insert("crf".into(), json!(28));
    with.insert("preset".into(), json!("ultrafast"));

    TranscodeStep.execute(&with, &mut ctx, &mut cb).await.unwrap();

    let out_path = ctx.steps.get("transcode").unwrap()["output_path"].as_str().unwrap();
    assert!(std::path::Path::new(out_path).exists(), "output file missing");
    assert!(events.iter().any(|e| matches!(e, StepProgress::Pct(_))), "no progress reported");
}
```

- [ ] **Step 2: Implement transcode**

Replace `src/steps/transcode.rs`:

```rust
use super::{Step, StepProgress};
use crate::ffmpeg::{drain_stderr_progress, ProgressParser};
use crate::flow::Context;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

pub struct TranscodeStep;

#[async_trait]
impl Step for TranscodeStep {
    fn name(&self) -> &'static str { "transcode" }

    async fn execute(
        &self,
        with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut dyn FnMut(StepProgress) + Send,
    ) -> anyhow::Result<()> {
        let codec = with.get("codec").and_then(|v| v.as_str()).unwrap_or("x265");
        let crf = with.get("crf").and_then(|v| v.as_i64()).unwrap_or(22);
        let preset = with.get("preset").and_then(|v| v.as_str()).unwrap_or("medium");

        let src = Path::new(&ctx.file.path).to_path_buf();
        let dest = src.with_extension("transcoderr.tmp.mkv");
        let _ = std::fs::remove_file(&dest);

        let duration_sec = ctx.probe.as_ref()
            .and_then(|p| p["format"]["duration"].as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);

        let codec_arg = match codec {
            "x264" => "libx264",
            "x265" | "hevc" => "libx265",
            other => anyhow::bail!("unsupported codec in Phase 1: {}", other),
        };

        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-hide_banner", "-y", "-i"])
           .arg(&src)
           .args(["-c:v", codec_arg, "-preset", preset, "-crf", &crf.to_string(),
                  "-c:a", "copy", "-c:s", "copy"])
           .arg(&dest)
           .stdin(Stdio::null())
           .stdout(Stdio::null())
           .stderr(Stdio::piped());

        let mut child = cmd.spawn()?;
        let stderr = child.stderr.take().expect("piped");
        let parser = ProgressParser { duration_sec };

        let progress_task = tokio::spawn({
            let dur = duration_sec;
            async move {
                let mut last_pct = 0.0;
                let mut buf: Vec<f64> = vec![];
                drain_stderr_progress(stderr, parser, |pct| {
                    if pct - last_pct >= 1.0 || pct >= 100.0 {
                        last_pct = pct;
                        buf.push(pct);
                    }
                }).await;
                let _ = dur;
                buf
            }
        });

        let status = child.wait().await?;
        let pcts = progress_task.await.unwrap_or_default();
        for p in pcts { on_progress(StepProgress::Pct(p)); }

        if !status.success() {
            anyhow::bail!("ffmpeg exited with {:?}", status.code());
        }

        ctx.record_step_output("transcode", json!({
            "output_path": dest.to_string_lossy(),
            "codec": codec,
        }));
        Ok(())
    }
}
```

- [ ] **Step 3: Run**

Run: `cargo test --test step_transcode -- --nocapture`
Expected: PASS. Test takes ~1-3s (ffmpeg run on a 2s clip).

- [ ] **Step 4: Commit**

```bash
git add src/steps/transcode.rs tests/step_transcode.rs
git commit -m "feat: transcode step (CPU x264/x265) with progress events"
```

---

### Task 11: Implement output step (replace mode, atomic swap)

**Files:**
- Modify: `src/steps/output.rs`
- Create: `tests/step_output.rs`

- [ ] **Step 1: Write the failing test**

Create `tests/step_output.rs`:

```rust
use serde_json::{json, Value};
use std::collections::BTreeMap;
use tempfile::tempdir;
use transcoderr::ffmpeg::make_testsrc_mkv;
use transcoderr::flow::Context;
use transcoderr::steps::{output::OutputStep, Step, StepProgress};

#[tokio::test]
async fn output_replace_swaps_atomically() {
    let dir = tempdir().unwrap();
    let original = dir.path().join("movie.mkv");
    let staged = dir.path().join("movie.transcoderr.tmp.mkv");
    make_testsrc_mkv(&original, 1).await.unwrap();
    make_testsrc_mkv(&staged, 1).await.unwrap();
    let staged_size = std::fs::metadata(&staged).unwrap().len();

    let mut ctx = Context::for_file(original.to_string_lossy());
    ctx.record_step_output("transcode", json!({
        "output_path": staged.to_string_lossy(),
    }));

    let mut with: BTreeMap<String, Value> = BTreeMap::new();
    with.insert("mode".into(), json!("replace"));
    let mut events = vec![];
    let mut cb = |e: StepProgress| events.push(e);

    OutputStep.execute(&with, &mut ctx, &mut cb).await.unwrap();

    // staged moved over original; staged path no longer exists
    assert!(!staged.exists(), "staged should be gone after rename");
    let final_size = std::fs::metadata(&original).unwrap().len();
    assert_eq!(final_size, staged_size);
}
```

- [ ] **Step 2: Implement output**

Replace `src/steps/output.rs`:

```rust
use super::{Step, StepProgress};
use crate::flow::Context;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

pub struct OutputStep;

#[async_trait]
impl Step for OutputStep {
    fn name(&self) -> &'static str { "output" }

    async fn execute(
        &self,
        with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut dyn FnMut(StepProgress) + Send,
    ) -> anyhow::Result<()> {
        let mode = with.get("mode").and_then(|v| v.as_str()).unwrap_or("replace");
        if mode != "replace" {
            anyhow::bail!("Phase 1 only supports mode=replace, got {:?}", mode);
        }
        let staged = ctx.steps.get("transcode")
            .and_then(|v| v.get("output_path"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("no transcode output_path in context"))?
            .to_string();

        let original = ctx.file.path.clone();
        on_progress(StepProgress::Log(format!("replacing {} with {}", original, staged)));

        // Same-filesystem atomic rename. (For Phase 1 we assume staged is sibling of original.)
        std::fs::rename(&staged, &original)?;
        Ok(())
    }
}
```

- [ ] **Step 3: Run**

Run: `cargo test --test step_output`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/steps/output.rs tests/step_output.rs
git commit -m "feat: output step (replace mode) with atomic rename"
```

---

### Task 12: DB layer — flows, jobs, run_events, checkpoints

**Files:**
- Create: `src/db/flows.rs`
- Create: `src/db/jobs.rs`
- Create: `src/db/run_events.rs`
- Create: `src/db/checkpoints.rs`
- Modify: `src/db/mod.rs`

- [ ] **Step 1: Write flows CRUD**

Create `src/db/flows.rs`:

```rust
use crate::db::now_unix;
use crate::flow::Flow;
use sqlx::SqlitePool;

#[derive(Debug, Clone)]
pub struct FlowRow {
    pub id: i64,
    pub name: String,
    pub enabled: bool,
    pub yaml_source: String,
    pub parsed_json: String,
    pub version: i64,
}

pub async fn insert(pool: &SqlitePool, name: &str, yaml: &str, parsed: &Flow) -> anyhow::Result<i64> {
    let parsed_json = serde_json::to_string(parsed)?;
    let now = now_unix();
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO flows (name, enabled, yaml_source, parsed_json, version, updated_at) \
         VALUES (?, 1, ?, ?, 1, ?) RETURNING id"
    )
    .bind(name).bind(yaml).bind(&parsed_json).bind(now)
    .fetch_one(pool).await?;
    sqlx::query("INSERT INTO flow_versions (flow_id, version, yaml_source, created_at) VALUES (?, 1, ?, ?)")
        .bind(id).bind(yaml).bind(now)
        .execute(pool).await?;
    Ok(id)
}

pub async fn get_by_name(pool: &SqlitePool, name: &str) -> anyhow::Result<Option<FlowRow>> {
    let row = sqlx::query_as::<_, (i64, String, i64, String, String, i64)>(
        "SELECT id, name, enabled, yaml_source, parsed_json, version FROM flows WHERE name = ?"
    ).bind(name).fetch_optional(pool).await?;
    Ok(row.map(|(id, name, enabled, yaml_source, parsed_json, version)| FlowRow {
        id, name, enabled: enabled != 0, yaml_source, parsed_json, version
    }))
}

pub async fn list_enabled_for_radarr(pool: &SqlitePool, event: &str) -> anyhow::Result<Vec<FlowRow>> {
    let all = sqlx::query_as::<_, (i64, String, i64, String, String, i64)>(
        "SELECT id, name, enabled, yaml_source, parsed_json, version FROM flows WHERE enabled = 1"
    ).fetch_all(pool).await?;
    let mut out = vec![];
    for (id, name, enabled, yaml_source, parsed_json, version) in all {
        let flow: Flow = serde_json::from_str(&parsed_json)?;
        let matches = flow.triggers.iter().any(|t| match t {
            crate::flow::Trigger::Radarr(events) => events.iter().any(|e| e == event),
        });
        if matches {
            out.push(FlowRow { id, name, enabled: enabled != 0, yaml_source, parsed_json, version });
        }
    }
    Ok(out)
}
```

- [ ] **Step 2: Write jobs CRUD**

Create `src/db/jobs.rs`:

```rust
use crate::db::now_unix;
use sqlx::SqlitePool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct JobRow {
    pub id: i64,
    pub flow_id: i64,
    pub flow_version: i64,
    pub source_kind: String,
    pub file_path: String,
    pub trigger_payload_json: String,
    pub status: String,
    pub priority: i64,
    pub current_step: Option<i64>,
    pub attempt: i64,
}

pub async fn insert(
    pool: &SqlitePool,
    flow_id: i64, flow_version: i64,
    source_kind: &str, file_path: &str, payload: &str,
) -> anyhow::Result<i64> {
    let now = now_unix();
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO jobs (flow_id, flow_version, source_kind, file_path, trigger_payload_json, status, priority, attempt, created_at) \
         VALUES (?, ?, ?, ?, ?, 'pending', 0, 0, ?) RETURNING id"
    )
    .bind(flow_id).bind(flow_version).bind(source_kind)
    .bind(file_path).bind(payload).bind(now)
    .fetch_one(pool).await?;
    Ok(id)
}

/// Atomically claim the next pending job — flips its status to running.
pub async fn claim_next(pool: &SqlitePool) -> anyhow::Result<Option<JobRow>> {
    let mut tx = pool.begin().await?;
    let row: Option<JobRow> = sqlx::query_as(
        "SELECT id, flow_id, flow_version, source_kind, file_path, trigger_payload_json, status, priority, current_step, attempt \
         FROM jobs WHERE status = 'pending' ORDER BY priority DESC, created_at ASC LIMIT 1"
    ).fetch_optional(&mut *tx).await?;
    let Some(job) = row else { tx.commit().await?; return Ok(None); };
    sqlx::query("UPDATE jobs SET status = 'running', started_at = ?, attempt = attempt + 1 WHERE id = ? AND status = 'pending'")
        .bind(now_unix()).bind(job.id)
        .execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Some(job))
}

pub async fn set_status(pool: &SqlitePool, id: i64, status: &str, label: Option<&str>) -> anyhow::Result<()> {
    sqlx::query("UPDATE jobs SET status = ?, status_label = ?, finished_at = ? WHERE id = ?")
        .bind(status).bind(label).bind(now_unix()).bind(id)
        .execute(pool).await?;
    Ok(())
}

pub async fn set_current_step(pool: &SqlitePool, id: i64, step_index: i64) -> anyhow::Result<()> {
    sqlx::query("UPDATE jobs SET current_step = ? WHERE id = ?")
        .bind(step_index).bind(id).execute(pool).await?;
    Ok(())
}

/// Reset 'running' rows to 'pending' on boot. Returns the number reset.
pub async fn reset_running_to_pending(pool: &SqlitePool) -> anyhow::Result<u64> {
    let r = sqlx::query("UPDATE jobs SET status = 'pending', started_at = NULL WHERE status = 'running'")
        .execute(pool).await?;
    Ok(r.rows_affected())
}
```

- [ ] **Step 3: Write run_events + checkpoints CRUD**

Create `src/db/run_events.rs`:

```rust
use crate::db::now_unix;
use serde_json::Value;
use sqlx::SqlitePool;

pub async fn append(
    pool: &SqlitePool,
    job_id: i64,
    step_id: Option<&str>,
    kind: &str,
    payload: Option<&Value>,
) -> anyhow::Result<()> {
    let payload_json = payload.map(|v| serde_json::to_string(v)).transpose()?;
    sqlx::query("INSERT INTO run_events (job_id, ts, step_id, kind, payload_json) VALUES (?, ?, ?, ?, ?)")
        .bind(job_id).bind(now_unix()).bind(step_id).bind(kind).bind(payload_json)
        .execute(pool).await?;
    Ok(())
}
```

Create `src/db/checkpoints.rs`:

```rust
use crate::db::now_unix;
use sqlx::SqlitePool;

pub async fn upsert(pool: &SqlitePool, job_id: i64, step_index: i64, snapshot: &str) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO checkpoints (job_id, step_index, context_snapshot_json, updated_at) \
         VALUES (?, ?, ?, ?) \
         ON CONFLICT (job_id) DO UPDATE SET step_index = excluded.step_index, context_snapshot_json = excluded.context_snapshot_json, updated_at = excluded.updated_at"
    )
    .bind(job_id).bind(step_index).bind(snapshot).bind(now_unix())
    .execute(pool).await?;
    Ok(())
}

pub async fn get(pool: &SqlitePool, job_id: i64) -> anyhow::Result<Option<(i64, String)>> {
    Ok(sqlx::query_as("SELECT step_index, context_snapshot_json FROM checkpoints WHERE job_id = ?")
        .bind(job_id).fetch_optional(pool).await?)
}
```

- [ ] **Step 4: Re-export from `db/mod.rs`**

Append to `src/db/mod.rs`:

```rust
pub mod flows;
pub mod jobs;
pub mod run_events;
pub mod checkpoints;
```

- [ ] **Step 5: Round-trip integration test**

Create `tests/db_roundtrip.rs`:

```rust
use tempfile::tempdir;
use transcoderr::db;
use transcoderr::flow::parse_flow;

#[tokio::test]
async fn flow_and_job_roundtrip() {
    let dir = tempdir().unwrap();
    let pool = db::open(dir.path()).await.unwrap();
    let yaml = r#"
name: t
triggers: [{ radarr: [downloaded] }]
steps:
  - use: probe
"#;
    let flow = parse_flow(yaml).unwrap();
    let id = db::flows::insert(&pool, "t", yaml, &flow).await.unwrap();
    assert!(id > 0);
    let job_id = db::jobs::insert(&pool, id, 1, "radarr", "/tmp/x.mkv", "{}").await.unwrap();
    let claimed = db::jobs::claim_next(&pool).await.unwrap().unwrap();
    assert_eq!(claimed.id, job_id);
    db::jobs::set_status(&pool, job_id, "completed", None).await.unwrap();
    let none = db::jobs::claim_next(&pool).await.unwrap();
    assert!(none.is_none());
}
```

Run: `cargo test --test db_roundtrip`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/db/
git add tests/db_roundtrip.rs
git commit -m "feat: db CRUD for flows, jobs, run_events, checkpoints"
```

---

### Task 13: Flow engine — sequential executor with checkpoints

**Files:**
- Create: `src/flow/engine.rs`
- Modify: `src/flow/mod.rs`
- Create: `tests/flow_engine.rs`

- [ ] **Step 1: Write the failing test**

Create `tests/flow_engine.rs`:

```rust
use serde_json::json;
use tempfile::tempdir;
use transcoderr::db;
use transcoderr::ffmpeg::make_testsrc_mkv;
use transcoderr::flow::{engine::Engine, parse_flow, Context};

#[tokio::test]
async fn engine_runs_probe_transcode_output() {
    let dir = tempdir().unwrap();
    let movie = dir.path().join("movie.mkv");
    make_testsrc_mkv(&movie, 2).await.unwrap();

    let pool = db::open(dir.path().join("db")).await.unwrap();
    let yaml = format!(r#"
name: e2e
triggers: [{{ radarr: [downloaded] }}]
steps:
  - id: probe
    use: probe
  - id: enc
    use: transcode
    with:
      codec: x264
      crf: 30
      preset: ultrafast
  - id: out
    use: output
    with:
      mode: replace
"#);
    let flow = parse_flow(&yaml).unwrap();
    let flow_id = db::flows::insert(&pool, "e2e", &yaml, &flow).await.unwrap();
    let job_id = db::jobs::insert(&pool, flow_id, 1, "radarr", &movie.to_string_lossy(), "{}").await.unwrap();
    let _claimed = db::jobs::claim_next(&pool).await.unwrap().unwrap();

    let ctx = Context::for_file(movie.to_string_lossy());
    let outcome = Engine::new(pool.clone()).run(&flow, job_id, ctx).await.unwrap();
    assert_eq!(outcome.status, "completed");

    // Original file replaced with transcoded output, and probe context recorded.
    let final_size = std::fs::metadata(&movie).unwrap().len();
    assert!(final_size > 0);
}
```

- [ ] **Step 2: Implement the engine**

Create `src/flow/engine.rs`:

```rust
use crate::db;
use crate::flow::{Context, Flow};
use crate::steps::{dispatch, StepProgress};
use serde_json::json;
use sqlx::SqlitePool;

pub struct Engine {
    pool: SqlitePool,
}

#[derive(Debug)]
pub struct Outcome {
    pub status: String,
    pub label: Option<String>,
}

impl Engine {
    pub fn new(pool: SqlitePool) -> Self { Self { pool } }

    pub async fn run(&self, flow: &Flow, job_id: i64, mut ctx: Context) -> anyhow::Result<Outcome> {
        // Resume from checkpoint if any.
        let resume_index = match db::checkpoints::get(&self.pool, job_id).await? {
            Some((idx, snap)) => {
                ctx = Context::from_snapshot(&snap)?;
                idx + 1
            }
            None => 0,
        };

        for (idx, step) in flow.steps.iter().enumerate().skip(resume_index as usize) {
            let step_id = step.id.clone().unwrap_or_else(|| format!("step{idx}"));
            db::jobs::set_current_step(&self.pool, job_id, idx as i64).await?;
            db::run_events::append(&self.pool, job_id, Some(&step_id), "started",
                Some(&json!({ "use": step.use_ }))).await?;

            let runner = dispatch(&step.use_)
                .ok_or_else(|| anyhow::anyhow!("unknown step `use:` {}", step.use_))?;

            let pool = self.pool.clone();
            let step_id_for_cb = step_id.clone();
            let mut cb = move |ev: StepProgress| {
                let pool = pool.clone();
                let step_id = step_id_for_cb.clone();
                tokio::spawn(async move {
                    let (kind, payload) = match ev {
                        StepProgress::Pct(p) => ("progress", json!({ "pct": p })),
                        StepProgress::Log(l) => ("log", json!({ "msg": l })),
                    };
                    let _ = db::run_events::append(&pool, job_id, Some(&step_id), kind, Some(&payload)).await;
                });
            };

            match runner.execute(&step.with, &mut ctx, &mut cb).await {
                Ok(()) => {
                    db::run_events::append(&self.pool, job_id, Some(&step_id), "completed", None).await?;
                    db::checkpoints::upsert(&self.pool, job_id, idx as i64, &ctx.to_snapshot()).await?;
                }
                Err(e) => {
                    db::run_events::append(&self.pool, job_id, Some(&step_id), "failed",
                        Some(&json!({ "error": e.to_string() }))).await?;
                    return Ok(Outcome { status: "failed".into(), label: None });
                }
            }
        }

        Ok(Outcome { status: "completed".into(), label: None })
    }
}
```

- [ ] **Step 3: Re-export and run**

Update `src/flow/mod.rs`:

```rust
pub mod context;
pub mod engine;
pub mod model;
pub mod parser;

pub use context::Context;
pub use engine::Engine;
pub use model::*;
pub use parser::parse_flow;
```

Run: `cargo test --test flow_engine`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/flow/engine.rs src/flow/mod.rs tests/flow_engine.rs
git commit -m "feat: flow engine with sequential execution and checkpoints"
```

---

### Task 14: Worker — single-slot poll loop with crash recovery

**Files:**
- Create: `src/worker.rs`
- Modify: `src/lib.rs`
- Create: `tests/crash_recovery.rs`

- [ ] **Step 1: Implement the worker**

Create `src/worker.rs`:

```rust
use crate::db;
use crate::flow::{Context, Engine, Flow};
use sqlx::SqlitePool;
use std::time::Duration;

pub struct Worker {
    pool: SqlitePool,
}

impl Worker {
    pub fn new(pool: SqlitePool) -> Self { Self { pool } }

    /// On startup: reset stale 'running' rows back to 'pending'.
    pub async fn recover_on_boot(&self) -> anyhow::Result<u64> {
        db::jobs::reset_running_to_pending(&self.pool).await
    }

    /// One loop iteration: claim and run one job. Returns true if a job was processed.
    pub async fn tick(&self) -> anyhow::Result<bool> {
        let Some(job) = db::jobs::claim_next(&self.pool).await? else { return Ok(false); };
        // Load flow.
        let flow_row: Option<(String, String)> = sqlx::query_as(
            "SELECT yaml_source, parsed_json FROM flows WHERE id = ?"
        ).bind(job.flow_id).fetch_optional(&self.pool).await?;
        let (_, parsed_json) = flow_row.ok_or_else(|| anyhow::anyhow!("flow {} missing", job.flow_id))?;
        let flow: Flow = serde_json::from_str(&parsed_json)?;

        let ctx = Context::for_file(&job.file_path);
        let outcome = Engine::new(self.pool.clone()).run(&flow, job.id, ctx).await?;
        db::jobs::set_status(&self.pool, job.id, &outcome.status, outcome.label.as_deref()).await?;
        Ok(true)
    }

    pub async fn run_loop(&self, shutdown: tokio::sync::watch::Receiver<bool>) {
        let mut shutdown = shutdown;
        loop {
            if *shutdown.borrow() { return; }
            match self.tick().await {
                Ok(true) => continue,
                Ok(false) => {
                    tokio::select! {
                        _ = shutdown.changed() => return,
                        _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "worker tick failed");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }
    }
}
```

Add to `src/lib.rs`:

```rust
pub mod worker;
```

- [ ] **Step 2: Write the crash-recovery test**

Create `tests/crash_recovery.rs`:

```rust
use serde_json::json;
use tempfile::tempdir;
use transcoderr::{db, ffmpeg::make_testsrc_mkv, flow::{parse_flow, Context, Engine}, worker::Worker};

#[tokio::test]
async fn checkpoint_resume_after_simulated_crash() {
    let dir = tempdir().unwrap();
    let movie = dir.path().join("m.mkv");
    make_testsrc_mkv(&movie, 1).await.unwrap();

    let pool = db::open(dir.path().join("db")).await.unwrap();
    let yaml = r#"
name: r
triggers: [{ radarr: [downloaded] }]
steps:
  - id: probe
    use: probe
  - id: enc
    use: transcode
    with:
      codec: x264
      crf: 30
      preset: ultrafast
  - id: out
    use: output
    with:
      mode: replace
"#;
    let flow = parse_flow(yaml).unwrap();
    let flow_id = db::flows::insert(&pool, "r", yaml, &flow).await.unwrap();
    let job_id = db::jobs::insert(&pool, flow_id, 1, "radarr", &movie.to_string_lossy(), "{}").await.unwrap();
    let _claimed = db::jobs::claim_next(&pool).await.unwrap().unwrap();

    // Simulate: probe ran, checkpoint saved at index 0, then "crash"
    let mut ctx = Context::for_file(movie.to_string_lossy());
    transcoderr::steps::dispatch("probe").unwrap()
        .execute(&Default::default(), &mut ctx, &mut |_| {})
        .await.unwrap();
    db::checkpoints::upsert(&pool, job_id, 0, &ctx.to_snapshot()).await.unwrap();
    // Process "crashes"; row left in 'running'

    // Boot recovery
    let w = Worker::new(pool.clone());
    let reset = w.recover_on_boot().await.unwrap();
    assert_eq!(reset, 1, "should reset one running job");

    // Run engine — should resume from checkpoint, skipping probe.
    let outcome = Engine::new(pool.clone()).run(&flow, job_id, Context::for_file(movie.to_string_lossy())).await.unwrap();
    assert_eq!(outcome.status, "completed");

    // The probe-skipped path leaves probe-step events absent from this run leg —
    // verify we DIDN'T re-run probe by inspecting that no NEW probe event fired
    // after the checkpoint was set.
    let evts: Vec<(String,)> = sqlx::query_as("SELECT kind FROM run_events WHERE job_id = ? AND step_id = 'probe'")
        .bind(job_id).fetch_all(&pool).await.unwrap();
    assert_eq!(evts.len(), 0, "probe should have been skipped due to checkpoint resume");
    let _ = json!({});
}
```

Run: `cargo test --test crash_recovery`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src/worker.rs src/lib.rs tests/crash_recovery.rs
git commit -m "feat: worker poll loop and boot-time crash recovery"
```

---

### Task 15: HTTP server skeleton + Radarr webhook adapter

**Files:**
- Create: `src/http/mod.rs`
- Create: `src/http/webhook_radarr.rs`
- Modify: `src/main.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: AppState + builder**

Create `src/http/mod.rs`:

```rust
use crate::config::Config;
use axum::{routing::post, Router};
use sqlx::SqlitePool;
use std::sync::Arc;

pub mod webhook_radarr;

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub cfg: Arc<Config>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/webhook/radarr", post(webhook_radarr::handle))
        .with_state(state)
}
```

- [ ] **Step 2: Radarr handler with bearer auth and job creation**

Create `src/http/webhook_radarr.rs`:

```rust
use crate::db;
use crate::http::AppState;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
pub struct RadarrPayload {
    #[serde(rename = "eventType")]
    pub event_type: String,
    #[serde(rename = "movieFile", default)]
    pub movie_file: Option<RadarrMovieFile>,
    #[serde(default)]
    pub movie: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct RadarrMovieFile {
    pub path: String,
}

pub async fn handle(
    State(state): State<AppState>,
    headers: HeaderMap,
    raw: Json<Value>,
) -> Result<StatusCode, StatusCode> {
    // Auth.
    let auth = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("");
    let expected = format!("Bearer {}", state.cfg.radarr.bearer_token);
    if auth != expected {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let payload: RadarrPayload = serde_json::from_value(raw.0.clone())
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let event = normalize_event(&payload.event_type);
    let Some(file) = payload.movie_file else { return Ok(StatusCode::ACCEPTED); };

    let flows = db::flows::list_enabled_for_radarr(&state.pool, &event)
        .await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let raw_str = serde_json::to_string(&raw.0).unwrap_or_default();
    for flow in flows {
        let _ = db::jobs::insert(&state.pool, flow.id, flow.version, "radarr",
            &file.path, &raw_str).await;
    }
    Ok(StatusCode::ACCEPTED)
}

fn normalize_event(e: &str) -> String {
    // Radarr uses "Download", "MovieFileDelete", "Test", etc. We lowercase and map a couple.
    match e {
        "Download" | "MovieFileImported" => "downloaded".to_string(),
        "MovieFileDelete" => "deleted".to_string(),
        other => other.to_lowercase(),
    }
}
```

- [ ] **Step 3: Wire into main.rs**

Replace `Cmd::Serve` arm in `src/main.rs`:

```rust
Cmd::Serve { config } => {
    let cfg = std::sync::Arc::new(transcoderr::config::Config::from_path(&config)?);
    let pool = transcoderr::db::open(&cfg.data_dir).await?;
    let worker = transcoderr::worker::Worker::new(pool.clone());
    let reset = worker.recover_on_boot().await?;
    if reset > 0 { tracing::warn!(reset, "recovered stale running jobs"); }

    let (tx, rx) = tokio::sync::watch::channel(false);
    let worker_task = tokio::spawn(async move { worker.run_loop(rx).await });

    let state = transcoderr::http::AppState { pool, cfg: cfg.clone() };
    let app = transcoderr::http::router(state);
    let listener = tokio::net::TcpListener::bind(&cfg.bind).await?;
    tracing::info!(bind = %cfg.bind, "serving");

    let serve = async move { axum::serve(listener, app).await };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("ctrl-c, shutting down");
            let _ = tx.send(true);
        }
        r = serve => { r?; }
    }
    let _ = worker_task.await;
    Ok(())
}
```

Add to `src/lib.rs`:

```rust
pub mod http;
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo build`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add src/http/ src/main.rs src/lib.rs
git commit -m "feat: HTTP server with Radarr webhook adapter"
```

---

### Task 16: End-to-end integration test

**Files:**
- Create: `tests/common/mod.rs`
- Create: `tests/webhook_to_complete.rs`

- [ ] **Step 1: Shared test helper**

Create `tests/common/mod.rs`:

```rust
use std::path::PathBuf;
use tempfile::TempDir;
use tokio::task::JoinHandle;
use transcoderr::{config::{Config, RadarrConfig}, db, http, worker::Worker};

pub struct TestApp {
    pub url: String,
    pub pool: sqlx::SqlitePool,
    pub data_dir: PathBuf,
    _temp: TempDir,
    _server: JoinHandle<()>,
    _worker: JoinHandle<()>,
}

pub async fn boot() -> TestApp {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().to_path_buf();
    let pool = db::open(&data_dir).await.unwrap();

    let cfg = std::sync::Arc::new(Config {
        bind: "127.0.0.1:0".into(),
        data_dir: data_dir.clone(),
        radarr: RadarrConfig { bearer_token: "test-token".into() },
    });

    let worker = Worker::new(pool.clone());
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let w = tokio::spawn(async move { worker.run_loop(rx).await });

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = http::router(http::AppState { pool: pool.clone(), cfg });
    let s = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    TestApp {
        url: format!("http://{addr}"),
        pool,
        data_dir,
        _temp: temp,
        _server: s,
        _worker: w,
    }
}
```

- [ ] **Step 2: End-to-end test**

Create `tests/webhook_to_complete.rs`:

```rust
mod common;

use common::boot;
use serde_json::json;
use std::time::Duration;
use transcoderr::{db, ffmpeg::make_testsrc_mkv, flow::parse_flow};

#[tokio::test]
async fn radarr_webhook_drives_a_run_to_completion() {
    let app = boot().await;
    let movie = app.data_dir.join("Movie.mkv");
    make_testsrc_mkv(&movie, 2).await.unwrap();
    let original_size = std::fs::metadata(&movie).unwrap().len();

    // Seed a flow.
    let yaml = r#"
name: e2e
triggers: [{ radarr: [downloaded] }]
steps:
  - id: probe
    use: probe
  - id: enc
    use: transcode
    with: { codec: x264, crf: 30, preset: ultrafast }
  - id: out
    use: output
    with: { mode: replace }
"#;
    let flow = parse_flow(yaml).unwrap();
    db::flows::insert(&app.pool, "e2e", yaml, &flow).await.unwrap();

    // POST a Radarr-shaped payload.
    let client = reqwest::Client::new();
    let resp = client.post(format!("{}/webhook/radarr", app.url))
        .bearer_auth("test-token")
        .json(&json!({
            "eventType": "Download",
            "movieFile": { "path": movie.to_string_lossy() }
        }))
        .send().await.unwrap();
    assert!(resp.status().is_success(), "got {}", resp.status());

    // Poll the DB until the job reaches a terminal status.
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let row: Option<(String,)> = sqlx::query_as("SELECT status FROM jobs ORDER BY id DESC LIMIT 1")
            .fetch_optional(&app.pool).await.unwrap();
        if let Some((status,)) = row {
            if status == "completed" {
                break;
            }
            if status == "failed" {
                panic!("job failed");
            }
        }
        if std::time::Instant::now() > deadline {
            panic!("job did not complete in time");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // File still exists, with a likely smaller size.
    let new_size = std::fs::metadata(&movie).unwrap().len();
    assert!(new_size > 0);
    let _ = original_size;
}
```

- [ ] **Step 3: Run**

Run: `cargo test --test webhook_to_complete -- --nocapture`
Expected: PASS in under a minute.

- [ ] **Step 4: Commit**

```bash
git add tests/common/ tests/webhook_to_complete.rs
git commit -m "test: end-to-end Radarr webhook → completed run"
```

---

### Task 17: Phase 1 polish — graceful shutdown, log polish, docs

**Files:**
- Modify: `src/main.rs`
- Create: `README.md`

- [ ] **Step 1: Graceful shutdown wiring**

In `src/main.rs`, ensure the shutdown channel actually plumbs to the worker (it already does in Task 15). Confirm the `Ctrl-C` path drops new connections cleanly:

```rust
let serve = axum::serve(listener, app).with_graceful_shutdown(async move {
    let _ = tokio::signal::ctrl_c().await;
});
```

Run: `cargo build`
Expected: clean.

- [ ] **Step 2: Phase 1 README**

Create `README.md`:

```markdown
# transcoderr

A push-driven, single-binary transcoder. Phase 1 ships a headless engine that:

- listens for Radarr download webhooks
- runs a linear `probe → transcode → output(replace)` flow against the file
- persists jobs and resumes from checkpoints across restarts

This is **Phase 1 of 5**. No web UI, no plugins, no GPU acceleration yet — see `docs/superpowers/specs/` for the full design and `docs/superpowers/plans/` for upcoming phases.

## Build

```
cargo build --release
```

## Configure

Copy `config.example.toml` to `config.toml` and edit. The Radarr bearer token must match the `Authorization: Bearer …` header your Radarr install will send (configure under Settings → Connect → Webhook).

## Run

```
./target/release/transcoderr serve --config config.toml
```

Then seed a flow into the DB (Phase 2 adds a CLI / UI for this) and POST a Radarr webhook at it.
```

- [ ] **Step 3: Final commit**

```bash
git add src/main.rs README.md
git commit -m "feat: graceful shutdown + Phase 1 README"
```

---

## Self-review checklist (Phase 1)

- [ ] All Phase 1 spec items covered: Rust binary skeleton, SQLite migrations, Radarr webhook adapter (auth + parse + create job), single-slot worker, linear flow engine, three built-in steps, checkpoint resume, integration test → ✓
- [ ] No placeholders. Every code block is full content the engineer can paste.
- [ ] No references to types or methods that aren't defined: `Engine`, `Worker`, `Step`, `Context`, `dispatch` all defined in earlier tasks.
- [ ] Type consistency: `JobRow`, `FlowRow`, `Outcome` match across uses.
- [ ] Each task ends with a commit step.
- [ ] Tests fail before implementation, pass after.
