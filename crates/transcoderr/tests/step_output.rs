use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::Write;
use tempfile::tempdir;
use transcoderr::ffmpeg::make_testsrc_mkv;
use transcoderr::flow::{
    plan::{save_plan, StreamPlan},
    Context,
};
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
    ctx.record_step_output(
        "transcode",
        json!({
            "output_path": staged.to_string_lossy(),
        }),
    );

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

#[tokio::test]
async fn output_alongside_keeps_original_and_writes_visible_mkv() {
    let dir = tempdir().unwrap();
    let original = dir.path().join("movie.mp4");
    let staged = dir.path().join("movie.mp4.tcr-00.tmp.mkv");
    let final_mkv = dir.path().join("movie.mkv");
    std::fs::File::create(&original)
        .unwrap()
        .write_all(b"original")
        .unwrap();
    std::fs::File::create(&staged)
        .unwrap()
        .write_all(b"transcoded")
        .unwrap();

    let mut ctx = Context::for_file(original.to_string_lossy());
    save_plan(
        &mut ctx,
        &StreamPlan {
            container: "mkv".into(),
            ..Default::default()
        },
    );
    ctx.record_step_output(
        "transcode",
        json!({
            "output_path": staged.to_string_lossy(),
        }),
    );

    let mut with: BTreeMap<String, Value> = BTreeMap::new();
    with.insert("mode".into(), json!("alongside"));
    let mut events = vec![];
    let mut cb = |e: StepProgress| events.push(e);

    OutputStep.execute(&with, &mut ctx, &mut cb).await.unwrap();

    assert!(!staged.exists(), "staged should be gone after rename");
    assert_eq!(std::fs::read(&original).unwrap(), b"original");
    assert_eq!(std::fs::read(&final_mkv).unwrap(), b"transcoded");
    assert_eq!(ctx.file.path, final_mkv.to_string_lossy());
}
