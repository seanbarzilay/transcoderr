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
            Node::Step { use_, .. } => {
                let _ = use_;  // accepted; final validation at runtime via plugin registry
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
