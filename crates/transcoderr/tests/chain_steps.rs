// Verifies that chaining transformer steps reads each step's output as the next
// step's input (the staging::next_io contract). Before this fix, every step read
// from the original file and chaining was a silent no-op.

use serde_json::{json, Value};
use std::collections::BTreeMap;
use tempfile::tempdir;
use transcoderr::ffmpeg::make_testsrc_mkv;
use transcoderr::flow::Context;
use transcoderr::steps::{
    audio_ensure::AudioEnsureStep, probe::ProbeStep, remux::RemuxStep, Step, StepProgress,
};

#[tokio::test]
async fn audio_ensure_then_remux_uses_audio_ensure_output() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("in.mkv");
    make_testsrc_mkv(&src, 2).await.unwrap();

    let mut ctx = Context::for_file(src.to_string_lossy());
    let mut noop = |_: StepProgress| {};

    // Probe.
    ProbeStep
        .execute(&BTreeMap::new(), &mut ctx, &mut noop)
        .await
        .unwrap();

    // Step 1: audio.ensure with target = ac3 6ch (will add a track).
    let mut with: BTreeMap<String, Value> = BTreeMap::new();
    with.insert("language".into(), json!("eng"));
    with.insert("codec".into(), json!("ac3"));
    with.insert("channels".into(), json!(6));
    AudioEnsureStep
        .execute(&with, &mut ctx, &mut noop)
        .await
        .unwrap();

    let after_audio = ctx.steps["transcode"]["output_path"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(std::path::Path::new(&after_audio).exists());
    assert!(after_audio.contains(".tcr-00."), "got {after_audio}");

    // Step 2: remux to mkv. Should read from after_audio (the tmp from step 1), not from src.
    let mut with: BTreeMap<String, Value> = BTreeMap::new();
    with.insert("container".into(), json!("mkv"));
    RemuxStep.execute(&with, &mut ctx, &mut noop).await.unwrap();

    let after_remux = ctx.steps["transcode"]["output_path"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(std::path::Path::new(&after_remux).exists());
    assert_ne!(
        after_audio, after_remux,
        "second step must produce a new file"
    );
    assert!(
        after_remux.contains(".tcr-01."),
        "expected step counter to advance; got {after_remux}"
    );
}
