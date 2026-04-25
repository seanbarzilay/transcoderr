use super::model::Flow;

const KNOWN_STEPS: &[&str] = &["probe", "transcode", "output"];

pub fn parse_flow(yaml: &str) -> anyhow::Result<Flow> {
    let flow: Flow = serde_yaml::from_str(yaml)?;
    validate(&flow)?;
    Ok(flow)
}

fn validate(flow: &Flow) -> anyhow::Result<()> {
    if flow.triggers.is_empty() {
        anyhow::bail!("flow {:?} has no triggers", flow.name);
    }
    for step in &flow.steps {
        if !KNOWN_STEPS.contains(&step.use_.as_str()) {
            anyhow::bail!("unknown step `use:` {:?} in flow {:?} (Phase 1 supports: {})",
                step.use_, flow.name, KNOWN_STEPS.join(", "));
        }
    }
    let _ = step_default_warning(flow);
    Ok(())
}

fn step_default_warning(_flow: &Flow) {}
