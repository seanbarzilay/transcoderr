use serde_json::{json, Value};
use std::collections::BTreeMap;
use tempfile::tempdir;
use transcoderr::ffmpeg::make_testsrc_mkv;
use transcoderr::flow::Context;
use transcoderr::steps::{remux::RemuxStep, Step, StepProgress};

#[tokio::test]
async fn remux_changes_container_only() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("in.mkv");
    make_testsrc_mkv(&src, 1).await.unwrap();
    let mut ctx = Context::for_file(src.to_string_lossy());
    let mut with: BTreeMap<String, Value> = BTreeMap::new();
    with.insert("container".into(), json!("mp4"));
    let mut cb = |_: StepProgress| {};
    RemuxStep.execute(&with, &mut ctx, &mut cb).await.unwrap();
    let out = ctx.steps.get("transcode").unwrap()["output_path"]
        .as_str()
        .unwrap();
    // staging::next_io produces names like "<original>.tcr-NN.tmp.<ext>"
    assert!(out.ends_with(".tmp.mp4"), "got {out}");
    assert!(out.contains(".tcr-"), "got {out}");
}
