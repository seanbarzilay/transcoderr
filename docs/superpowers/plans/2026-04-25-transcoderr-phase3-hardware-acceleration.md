# transcoderr Phase 3 — Hardware Acceleration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Probe available hardware encoders at boot, schedule GPU-bound steps with per-device semaphores so we never oversubscribe NVENC/QSV/VAAPI, and fall back to CPU when GPU encodes fail at runtime — all surfaced as structured events users can see.

**Architecture:** A new `hw` module owns capability probing (parses `ffmpeg -encoders` and platform-specific device enumeration into a typed `HwCaps`) and concurrency semaphores (one per detected device). The transcode step honors `with.hw` directives by acquiring permits before spawning ffmpeg, falling back per the configured policy on acquire failure or runtime error.

**Tech Stack:** Tokio sync primitives (`Semaphore`), `which` for binary discovery, no new heavy deps.

---

## Scope

**In:**
- `HwCaps` data type + boot-time probe
- `hw_capabilities` table population on boot (single row)
- Per-device async semaphore registry
- `transcode` step extended with `hw: { prefer: [...], fallback: cpu }` honored
- Runtime CPU fallback after GPU runtime failure (one retry, degraded preset)
- ENOSPC detected and skipped from fallback (terminal failure)
- `hw_unavailable`, `hw_runtime_failure` structured events
- Capability re-probe on user request (placeholder API endpoint; UI uses it Phase 4)

**Out:**
- Web UI / dashboard tile → Phase 4
- Prometheus metric for GPU sessions → Phase 5
- Multi-GPU device pinning beyond what ffmpeg accepts as `-hwaccel_device` → Phase 6+

---

## File Structure (delta)

```
migrations/
  20260425000003_phase3_hw.sql                  (no schema change strictly required —
                                                hw_capabilities was created in Phase 1
                                                for forward compatibility, but Phase 3
                                                adds an enable/disable column for devices)
src/
  hw/
    mod.rs                                       Public surface: probe, semaphores, retry policy
    probe.rs                                     ffmpeg/system probe
    devices.rs                                   Device enum + parsing helpers
    semaphores.rs                                Per-device async semaphore registry
  steps/
    transcode.rs                                 EXTENDED: read `hw.prefer`, acquire permits, fallback path
  http/
    mod.rs                                       Add: GET /api/hw  POST /api/hw/reprobe
tests/
  hw_probe.rs                                    parse fake ffmpeg -encoders output
  hw_semaphores.rs                               concurrency contract
  step_transcode_hw.rs                           hw acquire/fallback flow (mocked failure)
```

---

## Tasks

### Task 1: HwCaps types + parsing

**Files:**
- Create: `src/hw/mod.rs`
- Create: `src/hw/devices.rs`
- Create: `src/hw/probe.rs`
- Modify: `src/lib.rs`
- Create: `tests/hw_probe.rs`

- [ ] **Step 1: Types**

Create `src/hw/devices.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Accel {
    Nvenc,
    Qsv,
    Vaapi,
    VideoToolbox,
}

impl Accel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Accel::Nvenc => "nvenc",
            Accel::Qsv => "qsv",
            Accel::Vaapi => "vaapi",
            Accel::VideoToolbox => "videotoolbox",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "nvenc" => Some(Self::Nvenc),
            "qsv" => Some(Self::Qsv),
            "vaapi" => Some(Self::Vaapi),
            "videotoolbox" => Some(Self::VideoToolbox),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub accel: Accel,
    pub index: u32,
    pub name: String,
    pub max_concurrent: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HwCaps {
    pub probed_at: i64,
    pub ffmpeg_version: Option<String>,
    pub devices: Vec<Device>,
    pub encoders: Vec<String>,    // raw list of ffmpeg-known encoders that match an accel
}
```

- [ ] **Step 2: Probe**

Create `src/hw/probe.rs`:

```rust
use super::devices::{Accel, Device, HwCaps};
use std::process::Stdio;
use tokio::process::Command;

const HW_ENCODERS: &[(&str, Accel)] = &[
    ("h264_nvenc", Accel::Nvenc), ("hevc_nvenc", Accel::Nvenc), ("av1_nvenc", Accel::Nvenc),
    ("h264_qsv",   Accel::Qsv),   ("hevc_qsv",   Accel::Qsv),
    ("h264_vaapi", Accel::Vaapi), ("hevc_vaapi", Accel::Vaapi),
    ("h264_videotoolbox", Accel::VideoToolbox), ("hevc_videotoolbox", Accel::VideoToolbox),
];

pub async fn probe() -> HwCaps {
    let mut caps = HwCaps::default();
    caps.probed_at = chrono::Utc::now().timestamp();

    let v = Command::new("ffmpeg").arg("-version")
        .stderr(Stdio::null()).output().await;
    if let Ok(o) = v {
        if let Some(line) = String::from_utf8_lossy(&o.stdout).lines().next() {
            caps.ffmpeg_version = Some(line.to_string());
        }
    }

    let encs = Command::new("ffmpeg").args(["-hide_banner", "-encoders"])
        .stderr(Stdio::null()).output().await;
    if let Ok(o) = encs {
        let s = String::from_utf8_lossy(&o.stdout).to_string();
        let mut found = vec![];
        for (name, accel) in HW_ENCODERS {
            if s.contains(name) {
                found.push(name.to_string());
                caps.devices.push(Device {
                    accel: accel.clone(),
                    index: 0,
                    name: format!("{} (default)", name),
                    max_concurrent: default_concurrency(accel),
                });
            }
        }
        caps.encoders = found;
    }

    // Refine NVENC device count using nvidia-smi if available.
    if caps.devices.iter().any(|d| d.accel == Accel::Nvenc) {
        if let Ok(o) = Command::new("nvidia-smi").args(["-L"])
            .stderr(Stdio::null()).output().await {
            let listing = String::from_utf8_lossy(&o.stdout);
            let n = listing.lines().filter(|l| l.starts_with("GPU ")).count() as u32;
            if n > 0 {
                // Replace the placeholder NVENC device with one per detected GPU.
                caps.devices.retain(|d| d.accel != Accel::Nvenc);
                for i in 0..n {
                    caps.devices.push(Device {
                        accel: Accel::Nvenc,
                        index: i,
                        name: format!("NVENC GPU{}", i),
                        max_concurrent: 3,
                    });
                }
            }
        }
    }

    caps
}

fn default_concurrency(accel: &Accel) -> u32 {
    match accel {
        Accel::Nvenc => 3,         // consumer-card session limit
        Accel::Qsv => 8,
        Accel::Vaapi => 8,
        Accel::VideoToolbox => 4,
    }
}

pub fn parse_encoders_listing(stdout: &str) -> Vec<&'static str> {
    HW_ENCODERS.iter().filter(|(n, _)| stdout.contains(*n)).map(|(n, _)| *n).collect()
}
```

- [ ] **Step 3: Parser test against fake input**

Create `tests/hw_probe.rs`:

```rust
use transcoderr::hw::probe::parse_encoders_listing;

#[test]
fn finds_known_hw_encoders_in_listing() {
    let stdout = r#"
 V..... h264               H.264 / AVC / MPEG-4 AVC / MPEG-4 part 10
 V..... h264_nvenc         NVIDIA NVENC H.264 encoder
 V..... hevc_qsv           HEVC (Intel Quick Sync Video acceleration)
 V..... libx264            H.264 (libx264)
"#;
    let found = parse_encoders_listing(stdout);
    assert!(found.contains(&"h264_nvenc"));
    assert!(found.contains(&"hevc_qsv"));
    assert!(!found.contains(&"hevc_vaapi"));
}
```

- [ ] **Step 4: Public surface**

Create `src/hw/mod.rs`:

```rust
pub mod devices;
pub mod probe;
pub mod semaphores;

pub use devices::{Accel, Device, HwCaps};
```

Add to `src/lib.rs`:

```rust
pub mod hw;
```

- [ ] **Step 5: Run**

Run: `cargo test --test hw_probe`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/hw/ src/lib.rs tests/hw_probe.rs
git commit -m "feat(hw): capability probe and device types"
```

---

### Task 2: Per-device semaphore registry

**Files:**
- Create: `src/hw/semaphores.rs`
- Create: `tests/hw_semaphores.rs`

- [ ] **Step 1: Implement**

Create `src/hw/semaphores.rs`:

```rust
use super::devices::{Accel, HwCaps};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Semaphore;

#[derive(Clone)]
pub struct DeviceRegistry {
    by_key: Arc<HashMap<String, Arc<Semaphore>>>,  // key: "nvenc:0"
}

impl DeviceRegistry {
    pub fn from_caps(caps: &HwCaps) -> Self {
        let mut map: HashMap<String, Arc<Semaphore>> = HashMap::new();
        for d in &caps.devices {
            map.insert(format!("{}:{}", d.accel.as_str(), d.index),
                       Arc::new(Semaphore::new(d.max_concurrent as usize)));
        }
        Self { by_key: Arc::new(map) }
    }

    /// Acquire from the first available preferred accel. Returns the key + permit, or None.
    pub async fn acquire_preferred(&self, prefer: &[Accel]) -> Option<(String, tokio::sync::OwnedSemaphorePermit)> {
        for accel in prefer {
            for key in self.by_key.keys().filter(|k| k.starts_with(&format!("{}:", accel.as_str()))) {
                if let Some(sem) = self.by_key.get(key) {
                    if let Ok(permit) = sem.clone().try_acquire_owned() {
                        return Some((key.clone(), permit));
                    }
                }
            }
        }
        None
    }
}
```

- [ ] **Step 2: Test concurrency contract**

Create `tests/hw_semaphores.rs`:

```rust
use transcoderr::hw::{devices::{Accel, Device, HwCaps}, semaphores::DeviceRegistry};

#[tokio::test]
async fn semaphore_blocks_after_max_concurrent() {
    let caps = HwCaps {
        probed_at: 0,
        ffmpeg_version: None,
        devices: vec![Device { accel: Accel::Nvenc, index: 0, name: "n0".into(), max_concurrent: 1 }],
        encoders: vec![],
    };
    let reg = DeviceRegistry::from_caps(&caps);
    let (k1, p1) = reg.acquire_preferred(&[Accel::Nvenc]).await.expect("first acquires");
    assert_eq!(k1, "nvenc:0");
    let none = reg.acquire_preferred(&[Accel::Nvenc]).await;
    assert!(none.is_none(), "second should fail because limit=1");
    drop(p1);
    let (_, _) = reg.acquire_preferred(&[Accel::Nvenc]).await.expect("after drop, free again");
}
```

Run: `cargo test --test hw_semaphores`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src/hw/semaphores.rs tests/hw_semaphores.rs
git commit -m "feat(hw): per-device semaphore registry"
```

---

### Task 3: Wire HwCaps + DeviceRegistry into AppState

**Files:**
- Modify: `src/http/mod.rs`
- Modify: `src/main.rs`
- Modify: `src/db/mod.rs` (snapshot caps to `hw_capabilities` table)

- [ ] **Step 1: AppState extension**

Update `src/http/mod.rs` `AppState`:

```rust
#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub cfg: Arc<Config>,
    pub hw_caps: Arc<tokio::sync::RwLock<crate::hw::HwCaps>>,
    pub hw_devices: crate::hw::semaphores::DeviceRegistry,
}
```

- [ ] **Step 2: Snapshot caps to DB**

Append to `src/db/mod.rs`:

```rust
pub async fn snapshot_hw_caps(pool: &SqlitePool, caps: &crate::hw::HwCaps) -> anyhow::Result<()> {
    let json = serde_json::to_string(caps)?;
    sqlx::query(
        "INSERT INTO hw_capabilities (id, probed_at, devices_json) VALUES (1, ?, ?)
         ON CONFLICT (id) DO UPDATE SET probed_at = excluded.probed_at, devices_json = excluded.devices_json"
    ).bind(caps.probed_at).bind(json).execute(pool).await?;
    Ok(())
}
```

- [ ] **Step 3: Boot wiring**

In `src/main.rs` `Cmd::Serve`, after `pool` is built and before the worker starts:

```rust
let caps = transcoderr::hw::probe::probe().await;
transcoderr::db::snapshot_hw_caps(&pool, &caps).await?;
let registry = transcoderr::hw::semaphores::DeviceRegistry::from_caps(&caps);
let hw_caps = std::sync::Arc::new(tokio::sync::RwLock::new(caps));

// later, when building state:
let state = transcoderr::http::AppState {
    pool: pool.clone(), cfg: cfg.clone(),
    hw_caps: hw_caps.clone(), hw_devices: registry.clone(),
};

// Pass registry to the steps registry too — TranscodeStep needs it.
transcoderr::steps::registry::init(pool.clone(), registry.clone(), plugins).await;
```

The registry's `init` signature needs an additional `DeviceRegistry` parameter — update accordingly. `TranscodeStep` will read from it (next task).

- [ ] **Step 4: GET /api/hw + POST /api/hw/reprobe**

Add to `src/http/mod.rs`:

```rust
.route("/api/hw", axum::routing::get(get_hw))
.route("/api/hw/reprobe", axum::routing::post(reprobe_hw))
```

```rust
async fn get_hw(State(state): State<AppState>) -> axum::Json<crate::hw::HwCaps> {
    let g = state.hw_caps.read().await.clone();
    axum::Json(g)
}
async fn reprobe_hw(State(state): State<AppState>) -> axum::Json<crate::hw::HwCaps> {
    let new_caps = crate::hw::probe::probe().await;
    let _ = crate::db::snapshot_hw_caps(&state.pool, &new_caps).await;
    *state.hw_caps.write().await = new_caps.clone();
    axum::Json(new_caps)
}
```

- [ ] **Step 5: Build + smoke test**

Run: `cargo build`
Expected: clean.

Run a quick smoke: `cargo test`
Expected: all existing tests still pass.

- [ ] **Step 6: Commit**

```bash
git add src/main.rs src/http/mod.rs src/db/mod.rs src/steps/registry.rs
git commit -m "feat(hw): wire caps + device registry into AppState; expose /api/hw"
```

---

### Task 4: TranscodeStep honors `hw:` directive

**Files:**
- Modify: `src/steps/transcode.rs`
- Modify: `src/steps/builtin.rs` and `src/steps/registry.rs`
- Create: `tests/step_transcode_hw.rs`

- [ ] **Step 1: Inject DeviceRegistry into TranscodeStep**

Replace `src/steps/transcode.rs` (delta from Phase 1: store the registry; read `with.hw`):

```rust
use super::{Step, StepProgress};
use crate::ffmpeg::{drain_stderr_progress, ProgressParser};
use crate::flow::Context;
use crate::hw::{devices::Accel, semaphores::DeviceRegistry};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

pub struct TranscodeStep { pub hw: DeviceRegistry }

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
        let crf   = with.get("crf").and_then(|v| v.as_i64()).unwrap_or(22);
        let preset = with.get("preset").and_then(|v| v.as_str()).unwrap_or("medium");

        // Parse hw block
        let hw_block = with.get("hw").cloned().unwrap_or(Value::Null);
        let prefer: Vec<Accel> = hw_block.get("prefer")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().and_then(Accel::parse)).collect())
            .unwrap_or_default();
        let cpu_fallback = hw_block.get("fallback")
            .and_then(|v| v.as_str()) == Some("cpu");

        let src = Path::new(&ctx.file.path).to_path_buf();
        let dest = src.with_extension("transcoderr.tmp.mkv");
        let _ = std::fs::remove_file(&dest);
        let duration_sec = ctx.probe.as_ref()
            .and_then(|p| p["format"]["duration"].as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);

        // Acquire a GPU permit if requested.
        let mut acquired_key: Option<String> = None;
        let mut hw_permit: Option<tokio::sync::OwnedSemaphorePermit> = None;
        if !prefer.is_empty() {
            if let Some((key, permit)) = self.hw.acquire_preferred(&prefer).await {
                acquired_key = Some(key);
                hw_permit = Some(permit);
            } else {
                on_progress(StepProgress::Log("hw_unavailable: no preferred accel slot free".into()));
                if !cpu_fallback {
                    anyhow::bail!("no preferred hw accel available and cpu fallback disabled");
                }
            }
        }

        let codec_arg = pick_codec_arg(codec, acquired_key.as_deref())?;

        // First attempt
        let result = run_ffmpeg(&src, &dest, codec_arg, preset, crf, duration_sec, on_progress).await;

        // Drop GPU permit before any fallback so the slot is freed immediately.
        drop(hw_permit);

        match result {
            Ok(()) => {
                ctx.record_step_output("transcode", json!({
                    "output_path": dest.to_string_lossy(),
                    "codec": codec,
                    "hw": acquired_key,
                }));
                Ok(())
            }
            Err(e) => {
                if is_disk_full(&e) {
                    anyhow::bail!("disk_full");
                }
                if cpu_fallback && acquired_key.is_some() {
                    on_progress(StepProgress::Log(format!("hw_runtime_failure: {e}; retrying on CPU")));
                    let cpu_codec = match codec { "x264" => "libx264", _ => "libx265" };
                    run_ffmpeg(&src, &dest, cpu_codec, "ultrafast", crf, duration_sec, on_progress).await?;
                    ctx.record_step_output("transcode", json!({
                        "output_path": dest.to_string_lossy(),
                        "codec": codec,
                        "hw": null,
                        "fallback_from": acquired_key,
                    }));
                    Ok(())
                } else {
                    Err(e)
                }
            }
        }
    }
}

fn pick_codec_arg(codec: &str, acquired_key: Option<&str>) -> anyhow::Result<&'static str> {
    Ok(match (codec, acquired_key.map(|k| k.split(':').next().unwrap_or(""))) {
        ("x264", Some("nvenc")) => "h264_nvenc",
        ("x265" | "hevc", Some("nvenc")) => "hevc_nvenc",
        ("x264", Some("qsv")) => "h264_qsv",
        ("x265" | "hevc", Some("qsv")) => "hevc_qsv",
        ("x264", Some("vaapi")) => "h264_vaapi",
        ("x265" | "hevc", Some("vaapi")) => "hevc_vaapi",
        ("x264", Some("videotoolbox")) => "h264_videotoolbox",
        ("x265" | "hevc", Some("videotoolbox")) => "hevc_videotoolbox",
        ("x264", _) => "libx264",
        ("x265" | "hevc", _) => "libx265",
        (other, _) => anyhow::bail!("unsupported codec {other}"),
    })
}

fn is_disk_full(e: &anyhow::Error) -> bool {
    let s = e.to_string().to_lowercase();
    s.contains("no space left") || s.contains("enospc")
}

async fn run_ffmpeg(
    src: &Path, dest: &Path, codec_arg: &str, preset: &str, crf: i64, duration_sec: f64,
    on_progress: &mut dyn FnMut(StepProgress) + Send,
) -> anyhow::Result<()> {
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-y", "-i"]).arg(src)
       .args(["-c:v", codec_arg, "-preset", preset, "-crf", &crf.to_string(),
              "-c:a", "copy", "-c:s", "copy"]).arg(dest)
       .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let stderr = child.stderr.take().expect("piped");
    let parser = ProgressParser { duration_sec };
    let parse_task = tokio::spawn(async move {
        let mut last = 0.0; let mut buf: Vec<f64> = vec![];
        drain_stderr_progress(stderr, parser, |pct| {
            if pct - last >= 1.0 { last = pct; buf.push(pct); }
        }).await;
        buf
    });
    let status = child.wait().await?;
    let pcts = parse_task.await.unwrap_or_default();
    for p in pcts { on_progress(StepProgress::Pct(p)); }
    if !status.success() { anyhow::bail!("ffmpeg exit {:?}", status.code()); }
    Ok(())
}
```

- [ ] **Step 2: Update registry init**

In `src/steps/registry.rs::init` and `src/steps/builtin.rs::register_all`, pass `DeviceRegistry` and construct `TranscodeStep { hw: registry.clone() }`.

```rust
// builtin.rs
pub fn register_all(map: &mut HashMap<String, Arc<dyn Step>>, pool: SqlitePool, hw: DeviceRegistry) {
    map.insert("transcode".into(), Arc::new(TranscodeStep { hw }));
    // ... others unchanged ...
}
```

- [ ] **Step 3: hw integration test (mocked GPU using `prefer: []`)**

Create `tests/step_transcode_hw.rs`:

```rust
use serde_json::{json, Value};
use std::collections::BTreeMap;
use tempfile::tempdir;
use transcoderr::ffmpeg::make_testsrc_mkv;
use transcoderr::flow::Context;
use transcoderr::hw::{devices::{Accel, Device, HwCaps}, semaphores::DeviceRegistry};
use transcoderr::steps::{transcode::TranscodeStep, Step, StepProgress};

#[tokio::test]
async fn cpu_path_when_no_hw_preferred() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("in.mkv");
    make_testsrc_mkv(&p, 2).await.unwrap();
    let mut ctx = Context::for_file(p.to_string_lossy());
    let reg = DeviceRegistry::from_caps(&HwCaps::default());
    let step = TranscodeStep { hw: reg };

    let mut with: BTreeMap<String, Value> = BTreeMap::new();
    with.insert("codec".into(), json!("x264"));
    with.insert("crf".into(), json!(30));
    with.insert("preset".into(), json!("ultrafast"));

    let mut events = vec![];
    let mut cb = |e: StepProgress| events.push(e);
    step.execute(&with, &mut ctx, &mut cb).await.unwrap();
    let out = ctx.steps["transcode"]["output_path"].as_str().unwrap();
    assert!(std::path::Path::new(out).exists());
    assert!(ctx.steps["transcode"]["hw"].is_null());
}

#[tokio::test]
async fn fallback_to_cpu_when_no_gpu_slot() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("in.mkv");
    make_testsrc_mkv(&p, 1).await.unwrap();
    let mut ctx = Context::for_file(p.to_string_lossy());
    // Caps says nvenc exists with limit=1, but we'll exhaust it before calling.
    let caps = HwCaps {
        probed_at: 0, ffmpeg_version: None, encoders: vec![],
        devices: vec![Device { accel: Accel::Nvenc, index: 0, name: "fake".into(), max_concurrent: 1 }],
    };
    let reg = DeviceRegistry::from_caps(&caps);
    // Pre-acquire to exhaust.
    let _hold = reg.acquire_preferred(&[Accel::Nvenc]).await.unwrap();
    let step = TranscodeStep { hw: reg };

    let mut with: BTreeMap<String, Value> = BTreeMap::new();
    with.insert("codec".into(), json!("x264"));
    with.insert("crf".into(), json!(30));
    with.insert("preset".into(), json!("ultrafast"));
    with.insert("hw".into(), json!({ "prefer": ["nvenc"], "fallback": "cpu" }));

    let mut cb = |_: StepProgress| {};
    step.execute(&with, &mut ctx, &mut cb).await.unwrap();
    assert!(ctx.steps["transcode"]["output_path"].is_string());
}
```

- [ ] **Step 4: Run**

Run: `cargo test --test step_transcode_hw`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/steps/transcode.rs src/steps/builtin.rs src/steps/registry.rs tests/step_transcode_hw.rs
git commit -m "feat(hw): transcode honors hw.prefer with semaphores and CPU fallback"
```

---

### Task 5: Structured hw events from engine + run_events

**Files:**
- Modify: `src/steps/transcode.rs` (emit context_set marking hw outcome)
- Modify: `src/flow/engine.rs` (record hw markers as run_events of kind `hw_unavailable` / `hw_runtime_failure`)

- [ ] **Step 1: Add `StepProgress::Marker(kind, payload)` variant**

In `src/steps/mod.rs`:

```rust
#[derive(Debug, Clone)]
pub enum StepProgress {
    Pct(f64),
    Log(String),
    Marker { kind: String, payload: serde_json::Value },
}
```

Update existing matches in the engine to handle `Marker` by writing a run_events row of kind `kind`.

- [ ] **Step 2: Emit markers from transcode**

In `transcode.rs`, replace the relevant `on_progress(StepProgress::Log(...))` for hw events:

```rust
on_progress(StepProgress::Marker {
    kind: "hw_unavailable".into(),
    payload: json!({ "prefer": prefer.iter().map(|a| a.as_str()).collect::<Vec<_>>() }),
});
```

```rust
on_progress(StepProgress::Marker {
    kind: "hw_runtime_failure".into(),
    payload: json!({ "device": acquired_key, "error": e.to_string() }),
});
```

- [ ] **Step 3: Engine wires Marker → run_events**

In `src/flow/engine.rs`, in the progress callback:

```rust
let (kind, payload) = match ev {
    StepProgress::Pct(p)  => ("progress".into(), json!({ "pct": p })),
    StepProgress::Log(l)  => ("log".into(), json!({ "msg": l })),
    StepProgress::Marker { kind, payload } => (kind, payload),
};
let _ = db::run_events::append(&pool, job_id, Some(&step_id), &kind, Some(&payload)).await;
```

(Adjust `db::run_events::append` signature: `kind: &str` is fine since `kind` is a `String` we can borrow.)

- [ ] **Step 4: Test marker recording**

Extend `tests/step_transcode_hw.rs::fallback_to_cpu_when_no_gpu_slot` to drive through the engine (instead of calling `step.execute` directly), then assert `run_events` has rows with `kind = 'hw_unavailable'` and `kind = 'log'`.

(Pattern: insert a flow with one transcode step, insert a job, run engine, query.)

Run: `cargo test --test step_transcode_hw`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/steps/mod.rs src/steps/transcode.rs src/flow/engine.rs tests/step_transcode_hw.rs
git commit -m "feat(hw): structured hw_unavailable/hw_runtime_failure events"
```

---

## Self-review checklist (Phase 3)

- [ ] HwCaps + probe → Task 1
- [ ] Per-device semaphores → Task 2
- [ ] AppState + boot wiring + reprobe endpoint → Task 3
- [ ] Transcode honors hw.prefer + fallback to CPU → Task 4
- [ ] ENOSPC short-circuits fallback → Task 4 (`is_disk_full`)
- [ ] Structured `hw_unavailable` / `hw_runtime_failure` events → Task 5
- [ ] No placeholders, all code snippets are paste-ready
- [ ] Type names consistent: `HwCaps`, `Device`, `Accel`, `DeviceRegistry`, `TranscodeStep`
