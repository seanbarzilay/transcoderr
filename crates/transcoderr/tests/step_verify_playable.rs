use std::collections::BTreeMap;
use tempfile::tempdir;
use transcoderr::ffmpeg::make_testsrc_mkv;
use transcoderr::flow::Context;
use transcoderr::steps::{verify_playable::VerifyPlayableStep, Step, StepProgress};

#[tokio::test]
async fn verify_playable_succeeds_on_good_file() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("ok.mkv");
    make_testsrc_mkv(&p, 2).await.unwrap();
    let mut ctx = Context::for_file(p.to_string_lossy());
    ctx.probe = Some(serde_json::json!({ "format": { "duration": "2.000000" }}));
    ctx.steps.insert(
        "transcode".into(),
        serde_json::json!({ "output_path": p.to_string_lossy() }),
    );
    let mut events = vec![];
    let mut cb = |e: StepProgress| events.push(e);
    VerifyPlayableStep
        .execute(&BTreeMap::new(), &mut ctx, &mut cb)
        .await
        .unwrap();
}

#[tokio::test]
async fn verify_playable_fails_on_truncated_output() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("bad.mkv");
    std::fs::write(&p, b"not a real mkv").unwrap();
    let mut ctx = Context::for_file(p.to_string_lossy());
    ctx.probe = Some(serde_json::json!({ "format": { "duration": "10.000000" }}));
    ctx.steps.insert(
        "transcode".into(),
        serde_json::json!({ "output_path": p.to_string_lossy() }),
    );
    let mut cb = |_: StepProgress| {};
    let err = VerifyPlayableStep
        .execute(&BTreeMap::new(), &mut ctx, &mut cb)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("verify"));
}
