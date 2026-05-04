use super::{Step, StepProgress};
use crate::flow::{expr, Context};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;
use std::process::Stdio;
use tokio::process::Command;

pub struct ShellStep;

#[async_trait]
impl Step for ShellStep {
    fn name(&self) -> &'static str {
        "shell"
    }

    async fn execute(
        &self,
        with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()> {
        let cmd_template = with
            .get("cmd")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("shell: missing `cmd`"))?;
        let cmd = expr::eval_string_template(cmd_template, ctx)?;
        on_progress(StepProgress::Log(format!("$ {cmd}")));
        let status = Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await?;
        if !status.success() {
            anyhow::bail!("shell exited {:?}", status.code());
        }
        Ok(())
    }
}
