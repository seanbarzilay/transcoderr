use std::collections::BTreeMap;
use transcoderr::flow::Context;
use transcoderr::plugins::{discover, subprocess::SubprocessStep};
use transcoderr::steps::{Step, StepProgress};

#[tokio::test]
async fn subprocess_plugin_round_trip() {
    let plugins = discover(std::path::Path::new("tests/fixtures/plugins")).unwrap();
    let p = plugins.iter().find(|p| p.manifest.name == "hello").unwrap();
    let entrypoint = p.manifest.entrypoint.clone().unwrap();
    let abs = p.manifest_dir.join(&entrypoint);
    let step = SubprocessStep { step_name: "hello".into(), entrypoint_abs: abs };

    let mut ctx = Context::for_file("/tmp/x");
    let mut events = vec![];
    let mut cb = |e: StepProgress| events.push(e);
    step.execute(&BTreeMap::new(), &mut ctx, &mut cb).await.unwrap();
    assert!(events.iter().any(|e| matches!(e, StepProgress::Pct(p) if (*p - 50.0).abs() < 0.01)));
    assert_eq!(ctx.steps.get("hello").unwrap()["greeted"], true);
}
