use serde_json::{json, Value};
use std::collections::BTreeMap;
use tempfile::tempdir;
use transcoderr::ffmpeg::make_testsrc_mkv;
use transcoderr::flow::Context;
use transcoderr::steps::{output::OutputStep, Step, StepProgress};

#[tokio::test]
async fn output_replace_swaps_atomically() {
    let dir = tempdir().unwrap();
    let original = dir.path().join("movie.mkv");
    let staged = dir.path().join("movie.transcoderr.tmp.mkv");
    make_testsrc_mkv(&original, 1).await.unwrap();
    make_testsrc_mkv(&staged, 1).await.unwrap();
    let staged_size = std::fs::metadata(&staged).unwrap().len();

    let mut ctx = Context::for_file(original.to_string_lossy());
    ctx.record_step_output("transcode", json!({
        "output_path": staged.to_string_lossy(),
    }));

    let mut with: BTreeMap<String, Value> = BTreeMap::new();
    with.insert("mode".into(), json!("replace"));
    let mut events = vec![];
    let mut cb = |e: StepProgress| events.push(e);

    OutputStep.execute(&with, &mut ctx, &mut cb).await.unwrap();

    // staged moved over original; staged path no longer exists
    assert!(!staged.exists(), "staged should be gone after rename");
    let final_size = std::fs::metadata(&original).unwrap().len();
    assert_eq!(final_size, staged_size);
}
