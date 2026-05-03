use super::model::{Flow, Node};

const KNOWN_STEPS: &[&str] = &[
    "probe", "transcode", "output",
    // Phase 2 built-ins (added in later tasks):
    "verify.playable", "remux", "extract.subs", "strip.tracks",
    "move", "copy", "delete", "notify", "shell",
];

pub fn parse_flow(yaml: &str) -> anyhow::Result<Flow> {
    let flow: Flow = serde_yaml::from_str(yaml)?;
    validate(&flow)?;
    Ok(flow)
}

fn validate(flow: &Flow) -> anyhow::Result<()> {
    if flow.triggers.is_empty() {
        anyhow::bail!("flow {:?} has no triggers", flow.name);
    }
    walk(&flow.steps)?;
    if let Some(of) = &flow.on_failure { walk(of)?; }
    Ok(())
}

fn walk(nodes: &[Node]) -> anyhow::Result<()> {
    for n in nodes {
        match n {
            Node::Step { use_, run_on, .. } => {
                let _ = use_;  // accepted; final validation at runtime via plugin registry
                if let Some(crate::flow::model::RunOn::Any) = run_on {
                    if let Some(step) = crate::steps::registry::try_resolve(use_) {
                        if step.executor() == crate::steps::Executor::CoordinatorOnly {
                            anyhow::bail!("step `{use_}` is coordinator-only; `run_on: any` is invalid");
                        }
                    }
                }
            }
            Node::Conditional { then_, else_, .. } => {
                walk(then_)?;
                if let Some(e) = else_ { walk(e)?; }
            }
            Node::Return { .. } => {}
        }
    }
    Ok(())
}

#[allow(dead_code)]
fn known_step(use_: &str) -> bool { KNOWN_STEPS.contains(&use_) }

#[cfg(test)]
mod tests {

    #[test]
    fn parses_run_on_any() {
        let yaml = r#"
name: t
triggers: [{ webhook: x }]
steps:
  - use: transcode
    run_on: any
"#;
        let flow = crate::flow::parser::parse_flow(yaml).expect("parse ok");
        let crate::flow::model::Node::Step { run_on, .. } = &flow.steps[0] else { panic!() };
        assert_eq!(*run_on, Some(crate::flow::model::RunOn::Any));
    }

    #[test]
    fn parses_run_on_coordinator() {
        let yaml = r#"
name: t
triggers: [{ webhook: x }]
steps:
  - use: transcode
    run_on: coordinator
"#;
        let flow = crate::flow::parser::parse_flow(yaml).expect("parse ok");
        let crate::flow::model::Node::Step { run_on, .. } = &flow.steps[0] else { panic!() };
        assert_eq!(*run_on, Some(crate::flow::model::RunOn::Coordinator));
    }

    #[test]
    fn rejects_unknown_run_on_value() {
        let yaml = r#"
name: t
triggers: [{ webhook: x }]
steps:
  - use: transcode
    run_on: nope
"#;
        let err = crate::flow::parser::parse_flow(yaml).expect_err("must reject");
        // The YAML parser fails to deserialize unknown enum variant 'nope'
        assert!(format!("{err}").to_lowercase().contains("variant") || format!("{err}").contains("nope"),
            "error should mention the bad variant: {err}");
    }

    // NOTE: "rejects run_on:any on CoordinatorOnly step" is covered
    // by `tests/remote_dispatch.rs::coordinator_only_step_runs_locally`
    // because that scenario needs the registry initialised.
}
