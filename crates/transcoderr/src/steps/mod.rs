use crate::flow::Context;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;

/// Where a step is allowed to run. Default is `CoordinatorOnly`; the
/// remote-eligible built-ins override to `Any`. Subprocess plugins
/// keep the default until Piece 5 wires plugin push.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Executor {
    CoordinatorOnly,
    Any,
}

pub mod audio_ensure;
pub mod builtin;
pub mod copy_step;
pub mod delete_step;
pub mod extract_subs;
pub mod iso_extract;
pub mod move_step;
pub mod notify;
pub mod output;
pub mod plan_execute;
pub mod plan_steps;
pub mod probe;
pub mod registry;
pub mod remux;
pub mod schemas;
pub mod shell;
pub mod strip_tracks;
pub mod transcode;
pub mod verify_playable;
pub mod webhook;

#[derive(Debug, Clone)]
pub enum StepProgress {
    Pct(f64),
    Log(String),
    Marker {
        kind: String,
        payload: serde_json::Value,
    },
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
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()>;

    /// Default: coordinator-only. Each remote-eligible built-in
    /// overrides this. See `dispatch::route` for how this is consumed.
    fn executor(&self) -> Executor {
        Executor::CoordinatorOnly
    }

    /// JSON schema for this step's `with:` map. Default `None` for
    /// steps that haven't declared one. Surfaced via the registry's
    /// `list_kinds()` and the `list_step_kinds` MCP tool so flow
    /// authors can discover what each step accepts without reading
    /// source. Plugin steps surface their schema separately via the
    /// manifest path; this is the built-in-only hook.
    fn with_schema(&self) -> Option<Value> {
        None
    }
}

/// Look up a built-in step by `use:` name.
/// Kept for backwards compatibility with Phase 1 tests.
pub fn dispatch(use_: &str) -> Option<Box<dyn Step>> {
    match use_ {
        "probe" => Some(Box::new(probe::ProbeStep)),
        "transcode" => Some(Box::new(transcode::TranscodeStep {
            hw: crate::hw::semaphores::DeviceRegistry::empty(),
        })),
        "output" => Some(Box::new(output::OutputStep)),
        _ => None,
    }
}
