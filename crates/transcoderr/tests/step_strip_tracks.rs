//! `strip.tracks` must never publish an audio-less file.
//!
//! The audio map uses ffmpeg's *optional* stream specifier
//! (`0:a:m:language:eng:?`). A selector that matches nothing is a no-op
//! rather than an error, so a language filter matching no stream produced
//! an output with zero audio streams and exit status 0. The step checked
//! only `status.success()`, published the result as the chain head, and a
//! downstream `output: replace` renamed it over the user's only copy —
//! video and subtitles intact, audio gone, run reported `completed`.

use serde_json::{json, Value};
use std::collections::BTreeMap;
use tempfile::tempdir;
use transcoderr::ffmpeg::{ffprobe_json, make_testsrc_mkv};
use transcoderr::flow::Context;
use transcoderr::steps::{strip_tracks::StripTracksStep, Step, StepProgress};

fn with_langs(langs: &[&str]) -> BTreeMap<String, Value> {
    let mut with: BTreeMap<String, Value> = BTreeMap::new();
    with.insert("languages".into(), json!(langs));
    with
}

/// A probe payload with a single audio stream carrying `language`.
/// `None` means the stream has no language tag at all.
fn probe_with_audio(language: Option<&str>) -> Value {
    let mut stream = json!({ "index": 1, "codec_type": "audio", "codec_name": "aac" });
    if let Some(l) = language {
        stream["tags"] = json!({ "language": l });
    }
    json!({ "streams": [ { "index": 0, "codec_type": "video", "codec_name": "h264" }, stream ] })
}

#[tokio::test]
async fn refuses_when_no_audio_stream_matches_the_filter() {
    // Japanese-only audio, asked for English. Dropping every audio stream
    // is never what was meant — fail instead of writing a silent file.
    let dir = tempdir().unwrap();
    let src = dir.path().join("Movie.mkv");
    std::fs::write(&src, b"not really a video").unwrap();

    let mut ctx = Context::for_file(src.to_string_lossy());
    ctx.probe = Some(probe_with_audio(Some("jpn")));

    let mut cb = |_: StepProgress| {};
    let err = StripTracksStep
        .execute(&with_langs(&["eng"]), &mut ctx, &mut cb)
        .await
        .expect_err("must not produce an audio-less file");

    let msg = err.to_string();
    assert!(
        msg.contains("no audio stream matches"),
        "expected the refusal, got: {msg}"
    );
    assert!(
        msg.contains("jpn"),
        "error should name what is present: {msg}"
    );
    // It bailed before invoking ffmpeg, so nothing was staged.
    assert!(
        !ctx.steps.contains_key("transcode"),
        "a refused step must not publish a chain head"
    );
}

#[tokio::test]
async fn keeps_untagged_audio_rather_than_producing_a_silent_file() {
    // Untagged audio is the common case in remuxes — and in files this
    // project's own helpers generate. `0:a:m:language:eng:?` matches none
    // of it, so before the fix this produced a file with no audio at all.
    let dir = tempdir().unwrap();
    let src = dir.path().join("Movie.mkv");
    make_testsrc_mkv(&src, 1).await.unwrap();

    let mut ctx = Context::for_file(src.to_string_lossy());
    let probe = ffprobe_json(&src).await.unwrap();
    // Precondition: the generated file really does have untagged audio.
    let tagged = probe["streams"].as_array().unwrap().iter().any(|s| {
        s["codec_type"] == "audio" && s.get("tags").and_then(|t| t.get("language")).is_some()
    });
    assert!(!tagged, "fixture should have untagged audio: {probe}");
    ctx.probe = Some(probe);

    let mut cb = |_: StepProgress| {};
    StripTracksStep
        .execute(&with_langs(&["eng"]), &mut ctx, &mut cb)
        .await
        .expect("untagged audio should be kept, not dropped");

    let out = ctx.steps["transcode"]["output_path"].as_str().unwrap();
    let out_probe = ffprobe_json(std::path::Path::new(out)).await.unwrap();
    let audio = out_probe["streams"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["codec_type"] == "audio")
        .count();
    assert_eq!(audio, 1, "output must still have audio: {out_probe}");
}

#[tokio::test]
async fn honours_the_documented_languages_key() {
    // `StripTracksConfig` publishes `languages` and is `deny_unknown_fields`,
    // but the step used to read `keep_audio_languages` — so the documented
    // key was ignored and the real one was rejected by validation. An
    // operator could not change the filter at all.
    let dir = tempdir().unwrap();
    let src = dir.path().join("Movie.mkv");
    std::fs::write(&src, b"not really a video").unwrap();

    let mut ctx = Context::for_file(src.to_string_lossy());
    ctx.probe = Some(probe_with_audio(Some("jpn")));

    // Asking for jpn via the documented key must satisfy the filter, so the
    // step gets past the guard (and then fails on the bogus input file,
    // which is a different error).
    let mut cb = |_: StepProgress| {};
    let err = StripTracksStep
        .execute(&with_langs(&["jpn"]), &mut ctx, &mut cb)
        .await
        .expect_err("the fake source file cannot be transcoded");
    assert!(
        !err.to_string().contains("no audio stream matches"),
        "`languages` must be honoured; got the refusal instead: {err}"
    );
}

#[tokio::test]
async fn still_accepts_the_legacy_keep_audio_languages_key() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("Movie.mkv");
    std::fs::write(&src, b"not really a video").unwrap();

    let mut ctx = Context::for_file(src.to_string_lossy());
    ctx.probe = Some(probe_with_audio(Some("jpn")));

    let mut with: BTreeMap<String, Value> = BTreeMap::new();
    with.insert("keep_audio_languages".into(), json!(["jpn"]));

    let mut cb = |_: StepProgress| {};
    let err = StripTracksStep
        .execute(&with, &mut ctx, &mut cb)
        .await
        .expect_err("the fake source file cannot be transcoded");
    assert!(
        !err.to_string().contains("no audio stream matches"),
        "legacy key must keep working: {err}"
    );
}
