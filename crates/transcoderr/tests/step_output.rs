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

/// Build a `replace`-mode context for `original` with a staged file and a
/// planned container, so the final path is `<stem>.<container>`.
fn replace_ctx(original: &std::path::Path, staged: &std::path::Path, container: &str) -> Context {
    let mut ctx = Context::for_file(original.to_string_lossy());
    save_plan(
        &mut ctx,
        &StreamPlan {
            container: container.into(),
            ..Default::default()
        },
    );
    ctx.record_step_output(
        "transcode",
        json!({ "output_path": staged.to_string_lossy() }),
    );
    ctx
}

#[tokio::test]
async fn output_replace_refuses_to_clobber_a_different_existing_file() {
    // A library holding both a phone copy and a hand-kept full-quality
    // remux. Transcoding the .mp4 with a planned mkv container aims the
    // output straight at the .mkv. `std::fs::rename` would silently
    // replace it, and the step would then delete the .mp4 as the "source"
    // — two files gone, run reported completed.
    let dir = tempdir().unwrap();
    let original = dir.path().join("Movie.mp4");
    let bystander = dir.path().join("Movie.mkv");
    let staged = dir.path().join("Movie.mp4.tcr-j1-00.tmp.mkv");
    std::fs::write(&original, b"phone copy").unwrap();
    std::fs::write(&bystander, b"the 4k remux").unwrap();
    std::fs::write(&staged, b"transcoded").unwrap();

    let mut ctx = replace_ctx(&original, &staged, "mkv");
    let mut with: BTreeMap<String, Value> = BTreeMap::new();
    with.insert("mode".into(), json!("replace"));
    let mut cb = |_: StepProgress| {};

    let err = OutputStep
        .execute(&with, &mut ctx, &mut cb)
        .await
        .expect_err("must refuse to overwrite an unrelated file");
    assert!(
        err.to_string().contains("refusing to replace"),
        "error should name the collision: {err}"
    );

    // Nothing was destroyed, and the work is still on disk to retry.
    assert_eq!(std::fs::read(&bystander).unwrap(), b"the 4k remux");
    assert_eq!(std::fs::read(&original).unwrap(), b"phone copy");
    assert!(staged.exists(), "staged output should survive the refusal");
}

#[tokio::test]
async fn output_replace_still_writes_when_the_container_path_is_free() {
    // The guard must not block the ordinary container swap.
    let dir = tempdir().unwrap();
    let original = dir.path().join("Movie.mp4");
    let staged = dir.path().join("Movie.mp4.tcr-j1-00.tmp.mkv");
    let expected = dir.path().join("Movie.mkv");
    std::fs::write(&original, b"source").unwrap();
    std::fs::write(&staged, b"transcoded").unwrap();

    let mut ctx = replace_ctx(&original, &staged, "mkv");
    let mut with: BTreeMap<String, Value> = BTreeMap::new();
    with.insert("mode".into(), json!("replace"));
    let mut cb = |_: StepProgress| {};

    OutputStep.execute(&with, &mut ctx, &mut cb).await.unwrap();

    assert_eq!(std::fs::read(&expected).unwrap(), b"transcoded");
    assert!(
        !original.exists(),
        "source is removed after a container swap"
    );
    assert_eq!(ctx.file.path, expected.to_string_lossy());
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
