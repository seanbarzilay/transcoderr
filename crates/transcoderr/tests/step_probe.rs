use std::collections::BTreeMap;
use tempfile::tempdir;
use transcoderr::ffmpeg::make_testsrc_mkv;
use transcoderr::flow::Context;
use transcoderr::steps::{probe::ProbeStep, Step, StepProgress};

#[tokio::test]
async fn probe_populates_context() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("t.mkv");
    make_testsrc_mkv(&p, 1).await.unwrap();
    let mut ctx = Context::for_file(p.to_string_lossy());
    let mut events: Vec<StepProgress> = vec![];
    let mut cb = |e: StepProgress| events.push(e);
    ProbeStep
        .execute(&BTreeMap::new(), &mut ctx, &mut cb)
        .await
        .unwrap();
    let probe = ctx.probe.as_ref().expect("probe set");
    assert!(probe["streams"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["codec_type"] == "video"));
    assert!(ctx.file.size_bytes.unwrap() > 0);
}
