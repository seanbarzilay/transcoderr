use crate::hw::semaphores::DeviceRegistry;
use crate::plugins::manifest::DiscoveredPlugin;
use crate::plugins::subprocess::SubprocessStep;
use crate::steps::{builtin, Step};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::OnceCell;

static REGISTRY: OnceCell<Arc<Registry>> = OnceCell::const_new();

pub struct Registry {
    by_name: HashMap<String, Arc<dyn Step>>,
}

impl Registry {
    pub fn empty() -> Self {
        Self { by_name: HashMap::new() }
    }
}

pub async fn init(
    pool: SqlitePool,
    hw: DeviceRegistry,
    ffmpeg_caps: std::sync::Arc<crate::ffmpeg_caps::FfmpegCaps>,
    discovered: Vec<DiscoveredPlugin>,
) {
    let mut reg = Registry::empty();
    builtin::register_all(&mut reg.by_name, pool, hw, ffmpeg_caps);
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
    let _ = REGISTRY.set(Arc::new(reg));
}

/// Resolve a step by name. If the registry has not been initialized (e.g. in
/// unit tests that skip `init`), falls back to the built-in dispatch table.
/// NOTE: the fallback cannot instantiate `notify` (needs a pool) — tests that
/// exercise notify must call `init` explicitly.
pub async fn resolve(name: &str) -> Option<Arc<dyn Step>> {
    if let Some(reg) = REGISTRY.get() {
        return reg.by_name.get(name).cloned();
    }
    // Fallback: registry not yet initialized — serve built-ins directly
    // (notify excluded as it requires a pool).
    let mut map: HashMap<String, Arc<dyn Step>> = HashMap::new();
    builtin::register_all(
        &mut map,
        SqlitePool::connect_lazy("sqlite::memory:").unwrap(),
        DeviceRegistry::empty(),
        std::sync::Arc::new(crate::ffmpeg_caps::FfmpegCaps::default()),
    );
    map.remove(name)
}
