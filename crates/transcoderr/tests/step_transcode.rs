use serde_json::{json, Value};
use std::collections::BTreeMap;
use tempfile::tempdir;
use transcoderr::ffmpeg::make_testsrc_mkv;
use transcoderr::flow::Context;
use transcoderr::steps::{transcode::TranscodeStep, Step, StepProgress};

#[tokio::test]
async fn transcode_writes_output_and_records_path() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("in.mkv");
    make_testsrc_mkv(&src, 2).await.unwrap();
    let mut ctx = Context::for_file(src.to_string_lossy());
    ctx.probe = Some(json!({"format": {"duration": "2.000000"}}));
    let mut events = vec![];
    let mut cb = |e: StepProgress| events.push(e);

    let mut with: BTreeMap<String, Value> = BTreeMap::new();
    with.insert("codec".into(), json!("x264"));
    with.insert("crf".into(), json!(28));
    with.insert("preset".into(), json!("ultrafast"));

    let step = TranscodeStep {
        hw: transcoderr::hw::semaphores::DeviceRegistry::empty(),
    };
    step.execute(&with, &mut ctx, &mut cb).await.unwrap();

    let out_path = ctx.steps.get("transcode").unwrap()["output_path"]
        .as_str()
        .unwrap();
    assert!(
        std::path::Path::new(out_path).exists(),
        "output file missing"
    );
    assert!(
        events.iter().any(|e| matches!(e, StepProgress::Pct(_))),
        "no progress reported"
    );
}
