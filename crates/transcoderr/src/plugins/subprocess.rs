use crate::flow::Context;
use crate::steps::{Step, StepProgress};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct SubprocessStep {
    pub step_name: String,
    pub entrypoint_abs: PathBuf,
    pub executor: crate::steps::Executor,
}

#[async_trait]
impl Step for SubprocessStep {
    fn name(&self) -> &'static str {
        // Hack: we leak a static — but step names are stable strings the host
        // controls, so the leak is bounded.
        Box::leak(self.step_name.clone().into_boxed_str())
    }

    fn executor(&self) -> crate::steps::Executor {
        self.executor
    }

    async fn execute(
        &self,
        with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()> {
        let mut child = Command::new(&self.entrypoint_abs)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let mut stdin = child.stdin.take().expect("piped");
        let mut stdout = BufReader::new(child.stdout.take().expect("piped")).lines();

        // init
        let init = json!({ "method": "init", "params": { "workdir": "." } });
        stdin.write_all(format!("{init}\n").as_bytes()).await?;

        // execute
        let exec = json!({ "method": "execute", "params": {
            "step_id": self.step_name,
            "with": with,
            "context": ctx,
        }});
        stdin.write_all(format!("{exec}\n").as_bytes()).await?;

        let mut step_result: Option<Value> = None;
        while let Ok(Some(line)) = stdout.next_line().await {
            let v: Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            match v["event"].as_str() {
                Some("progress") => {
                    if let Some(p) = v["pct"].as_f64() {
                        on_progress(StepProgress::Pct(p));
                    }
                }
                Some("log") => {
                    if let Some(m) = v["msg"].as_str() {
                        on_progress(StepProgress::Log(m.into()));
                    }
                }
                Some("context_set") => {
                    if let (Some(k), Some(val)) = (v["key"].as_str(), v.get("value")) {
                        ctx.steps.insert(k.into(), val.clone());
                    }
                }
                Some("result") => {
                    step_result = Some(v);
                    break;
                }
                _ => {}
            }
        }
        let _ = stdin.shutdown().await;
        let _ = child.wait().await;
        let res = step_result
            .ok_or_else(|| anyhow::anyhow!("plugin {} produced no result", self.step_name))?;
        if res["status"] == "ok" {
            Ok(())
        } else {
            anyhow::bail!("plugin {} failed: {}", self.step_name, res["error"]["msg"])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::steps::{Executor, Step};

    #[test]
    fn executor_returns_field_value() {
        let s_any = SubprocessStep {
            step_name: "x".into(),
            entrypoint_abs: std::path::PathBuf::from("/tmp/x"),
            executor: Executor::Any,
        };
        assert_eq!(s_any.executor(), Executor::Any);

        let s_co = SubprocessStep {
            step_name: "x".into(),
            entrypoint_abs: std::path::PathBuf::from("/tmp/x"),
            executor: Executor::CoordinatorOnly,
        };
        assert_eq!(s_co.executor(), Executor::CoordinatorOnly);
    }
}
