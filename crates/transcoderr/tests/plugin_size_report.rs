//! End-to-end test of the shipped `size-report` example plugin.
//!
//! Loads the real manifest from `docs/plugins/size-report/`, drives both
//! step names through the subprocess protocol, and asserts the
//! before/after/ratio_pct values that show up in `ctx.steps`.

use std::collections::BTreeMap;
use std::io::Write;
use transcoderr::flow::Context;
use transcoderr::plugins::{discover, subprocess::SubprocessStep};
use transcoderr::steps::{Step, StepProgress};

fn python3_available() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn plugin_dir() -> std::path::PathBuf {
    // tests run from crates/transcoderr/, so repo root is two up.
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/plugins")
}

fn make_step(step_name: &str) -> SubprocessStep {
    let plugins = discover(&plugin_dir()).unwrap();
    let p = plugins
        .iter()
        .find(|p| p.manifest.name == "size-report")
        .expect("size-report plugin discovered");
    let entrypoint = p.manifest.entrypoint.clone().unwrap();
    let abs = p.manifest_dir.join(&entrypoint);
    SubprocessStep {
        step_name: step_name.into(),
        entrypoint_abs: abs,
    }
}

#[tokio::test]
async fn before_then_after_records_ratio_into_ctx() {
    if !python3_available() {
        eprintln!("python3 not on PATH; skipping size-report e2e test");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Movie.mkv");

    // 1000-byte input.
    {
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(&vec![0u8; 1000]).unwrap();
    }

    let mut ctx = Context::for_file(path.to_string_lossy().to_string());
    let mut events = vec![];
    let mut cb = |e: StepProgress| events.push(e);

    make_step("size.report.before")
        .execute(&BTreeMap::new(), &mut ctx, &mut cb)
        .await
        .expect("before step ok");

    let report = ctx
        .steps
        .get("size_report")
        .expect("size_report key written")
        .clone();
    assert_eq!(report["before_bytes"], 1000);
    assert!(report.get("after_bytes").is_none(), "after not set yet");

    // Simulate a transcode that shrunk the file to 600 bytes (this is
    // what `output.replace` would land on disk).
    {
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(&vec![0u8; 600]).unwrap();
    }

    make_step("size.report.after")
        .execute(&BTreeMap::new(), &mut ctx, &mut cb)
        .await
        .expect("after step ok");

    let report = ctx.steps.get("size_report").unwrap();
    assert_eq!(report["before_bytes"], 1000);
    assert_eq!(report["after_bytes"], 600);
    assert_eq!(report["saved_bytes"], 400);
    // 40.0% saved.
    assert!(
        (report["ratio_pct"].as_f64().unwrap() - 40.0).abs() < 0.01,
        "ratio_pct = {:?}",
        report["ratio_pct"]
    );
}

#[tokio::test]
async fn after_without_before_fails_with_clear_message() {
    if !python3_available() {
        eprintln!("python3 not on PATH; skipping size-report e2e test");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Movie.mkv");
    std::fs::File::create(&path).unwrap().write_all(b"x").unwrap();

    let mut ctx = Context::for_file(path.to_string_lossy().to_string());
    let mut cb = |_e: StepProgress| {};

    let err = make_step("size.report.after")
        .execute(&BTreeMap::new(), &mut ctx, &mut cb)
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("no before_bytes"),
        "expected guidance about missing before step, got: {msg}"
    );
}
