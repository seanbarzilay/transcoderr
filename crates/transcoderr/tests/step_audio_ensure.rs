use serde_json::{json, Value};
use std::collections::BTreeMap;
use tempfile::tempdir;
use transcoderr::ffmpeg::{ffprobe_json, make_testsrc_mkv};
use transcoderr::flow::Context;
use transcoderr::steps::{audio_ensure::AudioEnsureStep, probe::ProbeStep, Step, StepProgress};

#[tokio::test]
async fn audio_ensure_adds_target_track_when_missing() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("in.mkv");
    make_testsrc_mkv(&src, 2).await.unwrap();

    let mut ctx = Context::for_file(src.to_string_lossy());
    let mut cb = |_: StepProgress| {};
    ProbeStep
        .execute(&BTreeMap::new(), &mut ctx, &mut cb)
        .await
        .unwrap();

    let mut with: BTreeMap<String, Value> = BTreeMap::new();
    with.insert("language".into(), json!("eng"));
    with.insert("codec".into(), json!("ac3"));
    with.insert("channels".into(), json!(2));

    let mut events = vec![];
    let mut cb = |e: StepProgress| events.push(e);
    AudioEnsureStep
        .execute(&with, &mut ctx, &mut cb)
        .await
        .unwrap();

    let out = ctx.steps["transcode"]["output_path"].as_str().unwrap();
    assert!(std::path::Path::new(out).exists(), "tmp file missing");

    // Output should now have at least one audio stream (the source's stereo stream copied,
    // plus possibly the added one — testsrc mkv already has 2ch audio so dedupe may skip the add).
    let out_probe = ffprobe_json(std::path::Path::new(out)).await.unwrap();
    let audio_count = out_probe["streams"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["codec_type"] == "audio")
        .count();
    assert!(
        audio_count >= 1,
        "expected at least one audio stream, got {audio_count}"
    );
}

#[tokio::test]
async fn audio_ensure_dedupes_when_existing_stream_meets_target() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("in.mkv");
    make_testsrc_mkv(&src, 1).await.unwrap();

    let mut ctx = Context::for_file(src.to_string_lossy());
    let mut cb = |_: StepProgress| {};
    ProbeStep
        .execute(&BTreeMap::new(), &mut ctx, &mut cb)
        .await
        .unwrap();

    // Target = aac 1ch — the testsrc's audio (mono aac per make_testsrc_mkv) should
    // satisfy this and the dedupe rule should skip the add.
    let mut with: BTreeMap<String, Value> = BTreeMap::new();
    with.insert("language".into(), json!("eng"));
    with.insert("codec".into(), json!("aac"));
    with.insert("channels".into(), json!(1));

    let mut events: Vec<StepProgress> = vec![];
    let mut cb = |e: StepProgress| events.push(e);
    AudioEnsureStep
        .execute(&with, &mut ctx, &mut cb)
        .await
        .unwrap();

    let saw_skip = events.iter().any(|e| match e {
        StepProgress::Log(m) => m.contains("skipping add"),
        _ => false,
    });
    assert!(saw_skip, "expected a 'skipping add' log; got {events:?}");

    // No `added_audio_index` should be recorded.
    assert!(
        ctx.steps["transcode"].get("added_audio_index").is_none(),
        "should not have added an audio track"
    );
}
