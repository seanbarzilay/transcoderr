use crate::flow::Context;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;

pub mod builtin;
pub mod output;
pub mod probe;
pub mod registry;
pub mod transcode;

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
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()>;
}

/// Look up a built-in step by `use:` name.
/// Kept for backwards compatibility with Phase 1 tests.
pub fn dispatch(use_: &str) -> Option<Box<dyn Step>> {
    match use_ {
        "probe" => Some(Box::new(probe::ProbeStep)),
        "transcode" => Some(Box::new(transcode::TranscodeStep)),
        "output" => Some(Box::new(output::OutputStep)),
        _ => None,
    }
}
