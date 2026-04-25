# transcoderr Phase 5 — Observability, Ops, Distribution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make transcoderr fit for self-hosting at scale: a Prometheus metrics endpoint, retention pruning, log spillover for huge ffmpeg stderr, robust health probes, and Docker images per accel target with a clean release pipeline.

**Architecture:** A `metrics` module owns Prometheus counters/gauges/histograms, fed from existing engine lifecycle hooks. A `retention` background task runs daily to vacuum old `run_events` and `jobs`. Log spillover happens in `db::run_events::append` when payload >64 KB — falls back to a side file under `data/logs/<job_id>/`. Docker images are produced by separate Dockerfiles per accel toolchain (`cpu`, `nvidia`, `intel`, `full`).

**Tech Stack:** `metrics` + `metrics-exporter-prometheus`, `tokio` cron-style timer, Docker BuildKit.

---

## Scope

**In:**
- `GET /metrics` Prometheus exporter
- Counters/gauges/histograms wired at job + step lifecycle
- Retention daemon (configurable: events_days, jobs_days)
- Log spillover for `run_events.payload_json > 64 KB`
- `GET /healthz` (liveness, always-200) and `GET /readyz` (boot probe + plugin init complete)
- Vacuum on a daily schedule
- Boot config `--log-format json` propagated correctly to systemd
- Refuse to start if DB schema is newer than binary
- Dockerfiles: `Dockerfile.cpu`, `Dockerfile.nvidia`, `Dockerfile.intel`, `Dockerfile.full`
- GitHub Actions release workflow producing static linux-amd64/arm64 + darwin-arm64 binaries + Docker images per tag
- Final README pass with deploy guide

**Out:**
- Distributed workers (forever out of v1 design)
- OIDC / SSO
- Multi-tenant features

---

## File Structure (delta)

```
src/
  metrics.rs                                      Counters/gauges + exporter
  retention.rs                                    Daily prune task
  ready.rs                                        Boot-readiness state
  log_spill.rs                                    Spillover writer
  http/
    mod.rs                                        EXTENDED: /metrics, /healthz, /readyz
  db/
    run_events.rs                                 EXTENDED: spillover branch
    retention.rs                                  Prune queries
docker/
  Dockerfile.cpu
  Dockerfile.nvidia
  Dockerfile.intel
  Dockerfile.full
  docker-compose.example.yml
.github/
  workflows/
    release.yml
docs/
  deploy.md
tests/
  metrics.rs
  retention.rs
  log_spill.rs
  readyz.rs
```

---

## Tasks

### Task 1: Boot-readiness probe + healthz / readyz endpoints

**Files:**
- Create: `src/ready.rs`
- Modify: `src/http/mod.rs`
- Modify: `src/main.rs`
- Create: `tests/readyz.rs`

- [ ] **Step 1: Ready state**

Create `src/ready.rs`:

```rust
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone, Default)]
pub struct Readiness {
    inner: Arc<RwLock<bool>>,
}

impl Readiness {
    pub fn new() -> Self { Self::default() }
    pub async fn mark_ready(&self) { *self.inner.write().await = true; }
    pub async fn is_ready(&self) -> bool { *self.inner.read().await }
}
```

- [ ] **Step 2: Routes**

Add to `src/http/mod.rs`:

```rust
.route("/healthz", axum::routing::get(|| async { axum::http::StatusCode::OK }))
.route("/readyz",  axum::routing::get(readyz))
```

```rust
async fn readyz(State(state): State<AppState>) -> axum::http::StatusCode {
    if state.ready.is_ready().await { axum::http::StatusCode::OK }
    else { axum::http::StatusCode::SERVICE_UNAVAILABLE }
}
```

Add `pub ready: crate::ready::Readiness` to `AppState`.

- [ ] **Step 3: Mark ready in main**

In `main.rs::Cmd::Serve`, after capability probe + plugin discovery + step registry init complete:

```rust
let ready = transcoderr::ready::Readiness::new();
// ... wire into AppState ...
ready.mark_ready().await;
```

- [ ] **Step 4: DB schema-newer-than-binary check**

In `src/db/mod.rs`:

```rust
pub async fn check_migrations_compatible(pool: &sqlx::SqlitePool) -> anyhow::Result<()> {
    let migrator = sqlx::migrate!("./migrations");
    let known: std::collections::HashSet<i64> = migrator.iter().map(|m| m.version).collect();
    let applied: Vec<(i64,)> = sqlx::query_as("SELECT version FROM _sqlx_migrations")
        .fetch_all(pool).await.unwrap_or_default();
    for (v,) in applied {
        if !known.contains(&v) {
            anyhow::bail!("DB has migration {v} unknown to this binary — refusing to start");
        }
    }
    Ok(())
}
```

Call from `db::open` after `migrate!().run(...)`.

- [ ] **Step 5: Test**

Create `tests/readyz.rs`:

```rust
mod common;
use common::boot;

#[tokio::test]
async fn readyz_eventually_returns_200() {
    let app = boot().await;
    // boot() helper marks ready after wiring (update common::boot accordingly).
    let r = reqwest::get(format!("{}/readyz", app.url)).await.unwrap();
    assert_eq!(r.status(), 200);
}
```

(Update `tests/common/mod.rs::boot` to call `ready.mark_ready().await` at the end.)

Run: `cargo test --test readyz`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/ready.rs src/http/mod.rs src/main.rs src/db/mod.rs tests/readyz.rs tests/common/mod.rs
git commit -m "feat: healthz/readyz endpoints + DB schema compatibility guard"
```

---

### Task 2: Prometheus metrics exporter

**Files:**
- Modify: `Cargo.toml`
- Create: `src/metrics.rs`
- Modify: `src/http/mod.rs`
- Modify: `src/flow/engine.rs`, `src/db/jobs.rs` (instrument)
- Create: `tests/metrics.rs`

- [ ] **Step 1: Add deps**

```toml
metrics = "0.23"
metrics-exporter-prometheus = { version = "0.15", default-features = false, features = ["http-listener"] }
```

(We'll use the library directly without its HTTP listener and route through Axum.)

```toml
metrics-exporter-prometheus = "0.15"
```

- [ ] **Step 2: Metrics module**

Create `src/metrics.rs`:

```rust
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

pub struct Metrics {
    pub handle: PrometheusHandle,
}

impl Metrics {
    pub fn install() -> Self {
        let handle = PrometheusBuilder::new()
            .install_recorder()
            .expect("install prometheus recorder");
        Self { handle }
    }

    pub fn render(&self) -> String { self.handle.render() }
}

pub fn record_job_finished(flow: &str, status: &str, duration_secs: f64) {
    metrics::counter!("transcoderr_jobs_total", "flow" => flow.to_string(), "status" => status.to_string()).increment(1);
    metrics::histogram!("transcoderr_job_duration_seconds", "flow" => flow.to_string(), "status" => status.to_string()).record(duration_secs);
}

pub fn record_step_finished(plugin: &str, status: &str, duration_secs: f64) {
    metrics::histogram!("transcoderr_step_duration_seconds", "plugin" => plugin.to_string(), "status" => status.to_string()).record(duration_secs);
}

pub fn set_queue_depth(depth: i64) {
    metrics::gauge!("transcoderr_queue_depth").set(depth as f64);
}
pub fn set_workers_busy(busy: i64) {
    metrics::gauge!("transcoderr_workers_busy").set(busy as f64);
}
pub fn set_gpu_active(device: &str, n: i64) {
    metrics::gauge!("transcoderr_gpu_session_active", "device" => device.to_string()).set(n as f64);
}
pub fn add_bytes_saved(n: u64) {
    metrics::counter!("transcoderr_bytes_saved_total").increment(n);
}
```

- [ ] **Step 3: Wire `Metrics` into AppState + route**

In `src/main.rs` boot, install once: `let metrics = transcoderr::metrics::Metrics::install();`. Add to `AppState`. Route:

```rust
.route("/metrics", axum::routing::get(metrics_render))
```

```rust
async fn metrics_render(State(state): State<AppState>) -> String {
    state.metrics.render()
}
```

- [ ] **Step 4: Instrument engine + worker**

- In `worker::tick`, capture start time; on `set_status`, call `metrics::record_job_finished(flow_name, &status, elapsed)`.
- In `engine::run_nodes`, around each step's success/failure, capture start; call `metrics::record_step_finished(use_, "ok"/"err", elapsed)`.
- In `worker::run_loop`, on each iteration, query `SELECT COUNT(*) FROM jobs WHERE status='pending'` and call `metrics::set_queue_depth(...)`. Same for `running`.
- In `TranscodeStep`, track GPU acquire/release and call `metrics::set_gpu_active("nvenc:0", n)`.

Each instrumentation is one line at the right spot. Implementation is mechanical; the diffs go in the same commit.

- [ ] **Step 5: Test**

Create `tests/metrics.rs`:

```rust
mod common;
use common::boot;

#[tokio::test]
async fn metrics_endpoint_responds_with_text_format() {
    let app = boot().await;
    let body = reqwest::get(format!("{}/metrics", app.url)).await.unwrap().text().await.unwrap();
    assert!(body.contains("transcoderr_queue_depth"), "expected queue gauge in:\n{body}");
}
```

Run: `cargo test --test metrics`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock src/metrics.rs src/http/mod.rs src/main.rs src/flow/engine.rs src/worker.rs tests/metrics.rs
git commit -m "feat: Prometheus /metrics endpoint with engine/worker instrumentation"
```

---

### Task 3: Log spillover for large run_events payloads

**Files:**
- Modify: `src/db/run_events.rs`
- Create: `src/log_spill.rs`
- Create: `tests/log_spill.rs`

- [ ] **Step 1: Spill writer**

Create `src/log_spill.rs`:

```rust
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;

const INLINE_THRESHOLD: usize = 64 * 1024;

pub async fn maybe_spill(
    data_dir: &Path, job_id: i64, step_id: Option<&str>, event_id: i64, payload_json: &str,
) -> anyhow::Result<Option<PathBuf>> {
    if payload_json.len() <= INLINE_THRESHOLD { return Ok(None); }
    let dir = data_dir.join("logs").join(job_id.to_string());
    fs::create_dir_all(&dir).await?;
    let fname = format!("{}-{}.log", step_id.unwrap_or("unknown"), event_id);
    let path = dir.join(&fname);
    let mut f = fs::File::create(&path).await?;
    f.write_all(payload_json.as_bytes()).await?;
    Ok(Some(path))
}
```

- [ ] **Step 2: Modify `db::run_events::append` to spill**

Update signature to take `data_dir: &Path` (or pull it from a thread-local / state passed in). The cleanest path: add `append_with_spill(&self.pool, &state.cfg.data_dir, job_id, step_id, kind, payload)`.

Logic: insert the row first (with NULL payload_json), then if payload large, write file and update the row to set `payload_path = ?`. Otherwise update with `payload_json = ?`.

```rust
pub async fn append_with_spill(
    pool: &SqlitePool,
    data_dir: &Path,
    job_id: i64,
    step_id: Option<&str>,
    kind: &str,
    payload: Option<&Value>,
) -> anyhow::Result<()> {
    let payload_json = payload.map(|v| serde_json::to_string(v)).transpose()?;
    let event_id: i64 = sqlx::query_scalar(
        "INSERT INTO run_events (job_id, ts, step_id, kind) VALUES (?, ?, ?, ?) RETURNING id"
    ).bind(job_id).bind(now_unix()).bind(step_id).bind(kind).fetch_one(pool).await?;
    if let Some(p) = payload_json {
        if let Some(path) = crate::log_spill::maybe_spill(data_dir, job_id, step_id, event_id, &p).await? {
            sqlx::query("UPDATE run_events SET payload_path = ? WHERE id = ?")
                .bind(path.to_string_lossy().as_ref()).bind(event_id).execute(pool).await?;
        } else {
            sqlx::query("UPDATE run_events SET payload_json = ? WHERE id = ?")
                .bind(&p).bind(event_id).execute(pool).await?;
        }
    }
    Ok(())
}
```

Update engine + worker callers to use `append_with_spill`.

- [ ] **Step 3: Test**

Create `tests/log_spill.rs`:

```rust
use serde_json::json;
use tempfile::tempdir;
use transcoderr::db;

#[tokio::test]
async fn large_payload_spills_to_file() {
    let dir = tempdir().unwrap();
    let pool = db::open(dir.path()).await.unwrap();
    sqlx::query("INSERT INTO flows (id, name, enabled, yaml_source, parsed_json, version, updated_at) VALUES (1, 'x', 1, '', '{}', 1, 0)")
        .execute(&pool).await.unwrap();
    let job_id = db::jobs::insert(&pool, 1, 1, "radarr", "/x", "{}").await.unwrap();
    let big = json!({ "blob": "a".repeat(100 * 1024) });   // 100 KB
    db::run_events::append_with_spill(&pool, dir.path(), job_id, Some("step1"), "log", Some(&big)).await.unwrap();
    let row: (Option<String>, Option<String>) = sqlx::query_as("SELECT payload_json, payload_path FROM run_events ORDER BY id DESC LIMIT 1")
        .fetch_one(&pool).await.unwrap();
    assert!(row.0.is_none(), "payload_json should be empty for spill");
    let path = row.1.expect("payload_path set");
    let content = std::fs::read_to_string(path).unwrap();
    assert!(content.len() >= 100 * 1024);
}
```

Run: `cargo test --test log_spill`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/log_spill.rs src/db/run_events.rs src/lib.rs tests/log_spill.rs
git commit -m "feat: spill run_events payloads >64 KB to data/logs/"
```

---

### Task 4: Retention daemon

**Files:**
- Create: `src/retention.rs`
- Modify: `src/main.rs`
- Create: `tests/retention.rs`

- [ ] **Step 1: Implement**

Create `src/retention.rs`:

```rust
use crate::db;
use sqlx::SqlitePool;
use std::time::Duration;

pub async fn run_periodic(pool: SqlitePool, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    loop {
        if *shutdown.borrow() { return; }
        if let Err(e) = run_once(&pool).await { tracing::warn!(error = %e, "retention pass failed"); }
        tokio::select! {
            _ = shutdown.changed() => return,
            _ = tokio::time::sleep(Duration::from_secs(60 * 60 * 24)) => {}
        }
    }
}

pub async fn run_once(pool: &SqlitePool) -> anyhow::Result<()> {
    let events_days: i64 = db::settings::get(pool, "retention.events_days").await?
        .and_then(|s| s.parse().ok()).unwrap_or(30);
    let jobs_days: i64 = db::settings::get(pool, "retention.jobs_days").await?
        .and_then(|s| s.parse().ok()).unwrap_or(90);

    let now = chrono::Utc::now().timestamp();
    let event_cutoff = now - events_days * 86_400;
    let job_cutoff = now - jobs_days * 86_400;

    sqlx::query("DELETE FROM run_events WHERE job_id IN (SELECT id FROM jobs WHERE finished_at IS NOT NULL AND finished_at < ?)")
        .bind(event_cutoff).execute(pool).await?;
    sqlx::query("DELETE FROM checkpoints WHERE job_id IN (SELECT id FROM jobs WHERE finished_at IS NOT NULL AND finished_at < ?)")
        .bind(event_cutoff).execute(pool).await?;
    sqlx::query("DELETE FROM jobs WHERE finished_at IS NOT NULL AND finished_at < ?")
        .bind(job_cutoff).execute(pool).await?;
    sqlx::query("VACUUM").execute(pool).await?;
    Ok(())
}
```

- [ ] **Step 2: Spawn from main**

In `Cmd::Serve` boot, after `worker_task` is spawned:

```rust
let retention_rx = rx.clone();
tokio::spawn(transcoderr::retention::run_periodic(pool.clone(), retention_rx));
```

- [ ] **Step 3: Test**

Create `tests/retention.rs`:

```rust
use tempfile::tempdir;
use transcoderr::{db, retention};

#[tokio::test]
async fn run_once_prunes_old_completed_jobs() {
    let dir = tempdir().unwrap();
    let pool = db::open(dir.path()).await.unwrap();
    db::settings::set(&pool, "retention.events_days", "1").await.unwrap();
    db::settings::set(&pool, "retention.jobs_days", "1").await.unwrap();
    sqlx::query("INSERT INTO flows (id, name, enabled, yaml_source, parsed_json, version, updated_at) VALUES (1, 'x', 1, '', '{}', 1, 0)")
        .execute(&pool).await.unwrap();
    let two_days = chrono::Utc::now().timestamp() - 2 * 86_400;
    sqlx::query("INSERT INTO jobs (id, flow_id, flow_version, source_kind, file_path, trigger_payload_json, status, priority, attempt, created_at, finished_at) VALUES (1, 1, 1, 'radarr', '/x', '{}', 'completed', 0, 0, ?, ?)")
        .bind(two_days).bind(two_days).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO run_events (job_id, ts, kind, payload_json) VALUES (1, ?, 'completed', '{}')")
        .bind(two_days).execute(&pool).await.unwrap();

    retention::run_once(&pool).await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs").fetch_one(&pool).await.unwrap();
    assert_eq!(n, 0);
    let m: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM run_events").fetch_one(&pool).await.unwrap();
    assert_eq!(m, 0);
}
```

Run: `cargo test --test retention`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/retention.rs src/main.rs src/lib.rs tests/retention.rs
git commit -m "feat: daily retention daemon for events and completed jobs"
```

---

### Task 5: Dockerfiles per accel target

**Files:**
- Create: `docker/Dockerfile.cpu`
- Create: `docker/Dockerfile.nvidia`
- Create: `docker/Dockerfile.intel`
- Create: `docker/Dockerfile.full`
- Create: `docker/docker-compose.example.yml`
- Create: `.dockerignore`

- [ ] **Step 1: Common base — `Dockerfile.cpu`**

Create `docker/Dockerfile.cpu`:

```dockerfile
# syntax=docker/dockerfile:1.6

# Frontend builder
FROM node:20-bookworm-slim AS web
WORKDIR /web
COPY web/package*.json ./
RUN npm ci
COPY web/ .
RUN npm run build

# Rust builder
FROM rust:1.79-bookworm AS rust
WORKDIR /src
COPY . .
COPY --from=web /web/dist /src/web/dist
RUN cargo build --release --locked

# Runtime
FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends \
      ffmpeg ca-certificates tini \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=rust /src/target/release/transcoderr /usr/local/bin/transcoderr
COPY config.example.toml /app/config.example.toml
EXPOSE 8080
VOLUME ["/data"]
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/transcoderr"]
CMD ["serve", "--config", "/data/config.toml"]
```

- [ ] **Step 2: `Dockerfile.nvidia`**

Create `docker/Dockerfile.nvidia`:

```dockerfile
# syntax=docker/dockerfile:1.6
FROM node:20-bookworm-slim AS web
WORKDIR /web
COPY web/package*.json ./
RUN npm ci
COPY web/ .
RUN npm run build

FROM rust:1.79-bookworm AS rust
WORKDIR /src
COPY . .
COPY --from=web /web/dist /src/web/dist
RUN cargo build --release --locked

# NVENC-capable ffmpeg base
FROM jrottenberg/ffmpeg:6.0-nvidia2204 AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates tini \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=rust /src/target/release/transcoderr /usr/local/bin/transcoderr
EXPOSE 8080
VOLUME ["/data"]
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/transcoderr"]
CMD ["serve", "--config", "/data/config.toml"]
```

- [ ] **Step 3: `Dockerfile.intel` (QSV/VAAPI)**

Create `docker/Dockerfile.intel`:

```dockerfile
# syntax=docker/dockerfile:1.6
FROM node:20-bookworm-slim AS web
WORKDIR /web
COPY web/package*.json ./
RUN npm ci
COPY web/ .
RUN npm run build

FROM rust:1.79-bookworm AS rust
WORKDIR /src
COPY . .
COPY --from=web /web/dist /src/web/dist
RUN cargo build --release --locked

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends \
      ffmpeg ca-certificates tini \
      intel-media-va-driver-non-free i965-va-driver-shaders \
      vainfo libva-drm2 libva-x11-2 \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=rust /src/target/release/transcoderr /usr/local/bin/transcoderr
EXPOSE 8080
VOLUME ["/data"]
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/transcoderr"]
CMD ["serve", "--config", "/data/config.toml"]
```

- [ ] **Step 4: `Dockerfile.full`**

Same shape as `nvidia` but adds Intel VAAPI/QSV runtime packages on top of the NVIDIA base.

- [ ] **Step 5: docker-compose example**

Create `docker/docker-compose.example.yml`:

```yaml
services:
  transcoderr:
    image: ghcr.io/your-org/transcoderr:nvidia-latest
    restart: unless-stopped
    ports: ["8080:8080"]
    volumes:
      - ./data:/data
      - /mnt/movies:/media/movies
    deploy:
      resources:
        reservations:
          devices:
            - driver: nvidia
              count: all
              capabilities: [gpu, video]
```

- [ ] **Step 6: `.dockerignore`**

```
target/
node_modules/
web/node_modules/
web/dist/
.superpowers/
.git/
*.db*
data/
```

- [ ] **Step 7: Build and smoke**

Run locally:
```
docker build -f docker/Dockerfile.cpu -t transcoderr:cpu .
docker run --rm -v $PWD/data:/data -p 8080:8080 transcoderr:cpu
```
Expected: container starts, GET `http://localhost:8080/healthz` returns 200.

- [ ] **Step 8: Commit**

```bash
git add docker/ .dockerignore
git commit -m "build: dockerfiles for cpu/nvidia/intel/full targets"
```

---

### Task 6: GitHub Actions release workflow

**Files:**
- Create: `.github/workflows/release.yml`

- [ ] **Step 1: Workflow**

Create `.github/workflows/release.yml`:

```yaml
name: release
on:
  push:
    tags: ["v*"]
  workflow_dispatch:

jobs:
  bins:
    strategy:
      fail-fast: false
      matrix:
        include:
          - { os: ubuntu-latest,  target: x86_64-unknown-linux-gnu,   suffix: linux-amd64 }
          - { os: ubuntu-latest,  target: aarch64-unknown-linux-gnu,  suffix: linux-arm64,  cross: true }
          - { os: macos-14,       target: aarch64-apple-darwin,        suffix: darwin-arm64 }
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with: { node-version: 20 }
      - run: npm --prefix web ci && npm --prefix web run build
      - uses: dtolnay/rust-toolchain@stable
        with: { targets: ${{ matrix.target }} }
      - if: matrix.cross
        run: cargo install cross --locked
      - if: matrix.cross
        run: cross build --release --target ${{ matrix.target }} --locked
      - if: '!matrix.cross'
        run: cargo build --release --target ${{ matrix.target }} --locked
      - run: |
          mkdir -p out
          cp target/${{ matrix.target }}/release/transcoderr out/transcoderr-${{ matrix.suffix }}
      - uses: softprops/action-gh-release@v2
        if: startsWith(github.ref, 'refs/tags/')
        with:
          files: out/*

  images:
    needs: bins
    runs-on: ubuntu-latest
    permissions: { packages: write, contents: read }
    strategy:
      fail-fast: false
      matrix:
        flavor: [cpu, nvidia, intel, full]
    steps:
      - uses: actions/checkout@v4
      - uses: docker/setup-qemu-action@v3
      - uses: docker/setup-buildx-action@v3
      - uses: docker/login-action@v3
        with:
          registry: ghcr.io
          username: ${{ github.actor }}
          password: ${{ secrets.GITHUB_TOKEN }}
      - uses: docker/build-push-action@v6
        with:
          context: .
          file: docker/Dockerfile.${{ matrix.flavor }}
          push: true
          tags: |
            ghcr.io/${{ github.repository }}:${{ matrix.flavor }}-latest
            ghcr.io/${{ github.repository }}:${{ matrix.flavor }}-${{ github.ref_name }}
          platforms: linux/amd64
```

(arm64 image builds for cpu/intel/full are an obvious extension; nvidia base is amd64-only so leave as-is.)

- [ ] **Step 2: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci: release workflow for binaries and Docker images"
```

---

### Task 7: Deploy guide + final README pass

**Files:**
- Create: `docs/deploy.md`
- Modify: `README.md`

- [ ] **Step 1: Deploy guide**

Create `docs/deploy.md` covering:
- Choosing an image flavor (cpu / nvidia / intel / full)
- Volume layout and config.toml
- Connecting Radarr/Sonarr (URL, bearer token, event types)
- Adding notifiers
- Setting up reverse proxy + auth
- Backups (the `data/` directory)
- Common troubleshooting (GPU not detected, NVENC session limit, ffprobe errors)

- [ ] **Step 2: README final pass**

Replace the Phase 1 README with a complete one covering:
- What it is, what tdarr it isn't
- Quickstart with Docker (one paragraph + one compose snippet)
- Link to design spec, plans, deploy guide
- Plugin author intro (link to a future plugin author guide)

- [ ] **Step 3: Commit**

```bash
git add docs/deploy.md README.md
git commit -m "docs: deploy guide and final README pass"
```

---

### Task 8: Performance + smoke pass

**Files:** none new — just verification.

- [ ] **Step 1: Local smoke**

Build full release: `npm --prefix web run build && cargo build --release`.
Start: `./target/release/transcoderr serve --config config.toml`.
Verify in a browser: Dashboard, Flows, Runs all reachable; SSE stream works; `/metrics` returns text.

- [ ] **Step 2: Run a stress fixture**

Generate 10 small clips. POST 10 webhooks in parallel. Verify:
- All jobs queue and run (respecting pool size)
- `/metrics` shows `transcoderr_jobs_total{...,status="completed"}` rising
- No worker leaks (check `transcoderr_workers_busy` returns to 0)
- Retention `run_once` callable from a debug command and idempotent

- [ ] **Step 3: Document any flake**

If anything is racy, capture in an issue / follow-up plan rather than papering over with sleeps.

- [ ] **Step 4: Tag**

When green:

```bash
git tag v0.1.0
```

(Push tag separately when ready: `git push origin v0.1.0`.)

---

## Self-review checklist (Phase 5)

- [ ] `/metrics` Prometheus exporter with the metrics named in the spec → Task 2
- [ ] Retention daemon with configurable days → Task 4
- [ ] Log spillover for >64 KB payloads → Task 3
- [ ] `/healthz` + `/readyz` (readiness gated on plugin init + capability probe) → Task 1
- [ ] DB schema-newer-than-binary refusal → Task 1
- [ ] Vacuum on schedule → Task 4
- [ ] Docker images per accel target → Task 5
- [ ] Static binary release pipeline → Task 6
- [ ] Deploy guide + README → Task 7
- [ ] Smoke + stress validation → Task 8
- [ ] No placeholders; every command + path is concrete
