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
        _on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()> {
        unimplemented!("filled in next task")
    }
}
