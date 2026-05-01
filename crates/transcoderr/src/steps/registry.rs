use crate::hw::semaphores::DeviceRegistry;
use crate::plugins::manifest::DiscoveredPlugin;
use crate::plugins::subprocess::SubprocessStep;
use crate::steps::{builtin, Step};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{OnceCell, RwLock};

/// Inputs needed to (re)build the registry. Stashed at boot so
/// `rebuild_from_discovered` can recreate without the caller having
/// to re-thread these values from main.rs.
struct BuildInputs {
    pool: SqlitePool,
    hw: DeviceRegistry,
    ffmpeg_caps: Arc<crate::ffmpeg_caps::FfmpegCaps>,
}

static REGISTRY: OnceCell<RwLock<Arc<Registry>>> = OnceCell::const_new();
static BUILD_INPUTS: OnceCell<BuildInputs> = OnceCell::const_new();

pub struct Registry {
    by_name: HashMap<String, Arc<dyn Step>>,
}

impl Registry {
    pub fn empty() -> Self {
        Self { by_name: HashMap::new() }
    }
}

fn build(
    inputs: &BuildInputs,
    discovered: Vec<DiscoveredPlugin>,
) -> Registry {
    let mut reg = Registry::empty();
    builtin::register_all(
        &mut reg.by_name,
        inputs.pool.clone(),
        inputs.hw.clone(),
        inputs.ffmpeg_caps.clone(),
    );
    for d in discovered {
        if d.manifest.kind != "subprocess" {
            continue;
        }
        let entry = d.manifest.entrypoint.clone().unwrap_or_default();
        let abs = d.manifest_dir.join(&entry);
        for step_name in &d.manifest.provides_steps {
            let step = SubprocessStep {
                step_name: step_name.clone(),
                entrypoint_abs: abs.clone(),
            };
            reg.by_name.insert(step_name.clone(), Arc::new(step));
        }
    }
    reg
}

pub async fn init(
    pool: SqlitePool,
    hw: DeviceRegistry,
    ffmpeg_caps: Arc<crate::ffmpeg_caps::FfmpegCaps>,
    discovered: Vec<DiscoveredPlugin>,
) {
    let inputs = BuildInputs { pool, hw, ffmpeg_caps };
    let reg = build(&inputs, discovered);
    let _ = BUILD_INPUTS.set(inputs);
    let _ = REGISTRY.set(RwLock::new(Arc::new(reg)));
}

/// Rebuild and atomically swap the registry. In-flight runs that
/// already called `resolve()` keep their `Arc<dyn Step>` so they
/// finish on the old code; new `resolve()` calls return the new
/// step set.
pub async fn rebuild_from_discovered(discovered: Vec<DiscoveredPlugin>) {
    let Some(inputs) = BUILD_INPUTS.get() else { return };
    let new = build(inputs, discovered);
    if let Some(rw) = REGISTRY.get() {
        *rw.write().await = Arc::new(new);
    }
}

/// Resolve a step by name. If the registry has not been initialized
/// (e.g. unit tests that skip `init`), falls back to the built-in
/// dispatch table. NOTE: the fallback cannot instantiate `notify`
/// (needs a pool) — tests that exercise notify must call `init`.
pub async fn resolve(name: &str) -> Option<Arc<dyn Step>> {
    if let Some(rw) = REGISTRY.get() {
        return rw.read().await.by_name.get(name).cloned();
    }
    let mut map: HashMap<String, Arc<dyn Step>> = HashMap::new();
    builtin::register_all(
        &mut map,
        SqlitePool::connect_lazy("sqlite::memory:").unwrap(),
        DeviceRegistry::empty(),
        Arc::new(crate::ffmpeg_caps::FfmpegCaps::default()),
    );
    map.remove(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow::Context;
    use crate::plugins::manifest::Manifest;
    use crate::steps::StepProgress;
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    /// Build a minimal DiscoveredPlugin pointing at a shell script that
    /// emits `result:ok`. Used to verify rebuild_from_discovered swaps
    /// in a new step that wasn't there at boot.
    fn discovered_with_step(plugin_name: &str, step_name: &str, dir: &std::path::Path) -> DiscoveredPlugin {
        let plugin_dir = dir.join(plugin_name);
        std::fs::create_dir_all(plugin_dir.join("bin")).unwrap();
        let script = "#!/bin/sh\nread INIT\nread EXEC\necho '{\"event\":\"result\",\"status\":\"ok\",\"outputs\":{}}'\n";
        let entry = plugin_dir.join("bin/run");
        std::fs::write(&entry, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut p = std::fs::metadata(&entry).unwrap().permissions();
            p.set_mode(0o755);
            std::fs::set_permissions(&entry, p).unwrap();
        }
        DiscoveredPlugin {
            manifest: Manifest {
                name: plugin_name.into(),
                version: "0.1.0".into(),
                kind: "subprocess".into(),
                entrypoint: Some("bin/run".into()),
                provides_steps: vec![step_name.into()],
                requires: serde_json::Value::Null,
                capabilities: vec![],
                summary: None,
                min_transcoderr_version: None,
            },
            manifest_dir: plugin_dir,
            schema: serde_json::Value::Null,
        }
    }

    /// Initialize the registry once. The OnceCell is process-wide, so
    /// tests in this binary share it. We use a marker test that only
    /// installs init the first time.
    async fn ensure_init() {
        let dir = tempdir().unwrap();
        let pool = crate::db::open(dir.path()).await.unwrap();
        init(
            pool,
            DeviceRegistry::empty(),
            Arc::new(crate::ffmpeg_caps::FfmpegCaps::default()),
            vec![],
        ).await;
        // Leak the temp dir so the migration files stay reachable; this
        // is a one-shot global init for the whole test binary.
        std::mem::forget(dir);
    }

    #[tokio::test]
    async fn rebuild_adds_a_new_step_visible_to_subsequent_resolves() {
        ensure_init().await;
        let dir = tempdir().unwrap();
        let d = discovered_with_step("hello", "rebuild.test.step", dir.path());

        // Step is not in the registry yet.
        assert!(resolve("rebuild.test.step").await.is_none());

        rebuild_from_discovered(vec![d]).await;

        let step = resolve("rebuild.test.step").await.expect("step now present");
        let mut ctx = Context::for_file("/x");
        let mut cb = |_: StepProgress| {};
        step.execute(&BTreeMap::new(), &mut ctx, &mut cb).await.unwrap();
    }

    #[tokio::test]
    async fn in_flight_arc_survives_a_swap() {
        ensure_init().await;
        let dir = tempdir().unwrap();
        let d = discovered_with_step("inflight", "inflight.test.step", dir.path());
        rebuild_from_discovered(vec![d]).await;

        let step = resolve("inflight.test.step").await.expect("step present pre-swap");

        // Swap to an empty registry (drops the step). The in-flight
        // Arc<dyn Step> we hold should still be runnable.
        rebuild_from_discovered(vec![]).await;

        assert!(resolve("inflight.test.step").await.is_none(), "step gone after swap");

        let mut ctx = Context::for_file("/x");
        let mut cb = |_: StepProgress| {};
        step.execute(&BTreeMap::new(), &mut ctx, &mut cb).await.unwrap();
    }
}
