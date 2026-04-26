use serde_json::{json, Value};
use std::collections::BTreeMap;
use tempfile::tempdir;
use transcoderr::flow::Context;
use transcoderr::steps::{
    copy_step::CopyStep, delete_step::DeleteStep, move_step::MoveStep, shell::ShellStep,
    Step, StepProgress,
};

#[tokio::test]
async fn move_step_relocates_file() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("a.bin");
    std::fs::write(&src, b"x").unwrap();
    let dest_dir = dir.path().join("nested/dest");

    let mut ctx = Context::for_file(src.to_string_lossy());
    let mut with: BTreeMap<String, Value> = BTreeMap::new();
    with.insert("to".into(), json!(dest_dir.to_string_lossy()));
    let mut cb = |_: StepProgress| {};

    MoveStep.execute(&with, &mut ctx, &mut cb).await.unwrap();
    assert!(!src.exists());
    assert!(dest_dir.join("a.bin").exists());
    assert_eq!(ctx.file.path, dest_dir.join("a.bin").to_string_lossy());
}

#[tokio::test]
async fn copy_step_preserves_original() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("a.bin");
    std::fs::write(&src, b"x").unwrap();
    let dest_dir = dir.path().join("dest");

    let mut ctx = Context::for_file(src.to_string_lossy());
    let mut with: BTreeMap<String, Value> = BTreeMap::new();
    with.insert("to".into(), json!(dest_dir.to_string_lossy()));
    let mut cb = |_: StepProgress| {};

    CopyStep.execute(&with, &mut ctx, &mut cb).await.unwrap();
    assert!(src.exists());
    assert!(dest_dir.join("a.bin").exists());
    assert_eq!(ctx.file.path, src.to_string_lossy());
}

#[tokio::test]
async fn delete_step_removes_file() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("a.bin");
    std::fs::write(&p, b"x").unwrap();

    let mut ctx = Context::for_file(p.to_string_lossy());
    let mut cb = |_: StepProgress| {};
    DeleteStep.execute(&BTreeMap::new(), &mut ctx, &mut cb).await.unwrap();
    assert!(!p.exists());
}

#[tokio::test]
async fn shell_step_runs_command_with_template() {
    let dir = tempdir().unwrap();
    let marker = dir.path().join("ran");
    let mut ctx = Context::for_file(marker.to_string_lossy());
    let mut with: BTreeMap<String, Value> = BTreeMap::new();
    with.insert("cmd".into(), json!(format!("touch {{{{ file.path }}}}")));
    let mut cb = |_: StepProgress| {};

    ShellStep.execute(&with, &mut ctx, &mut cb).await.unwrap();
    assert!(marker.exists());
}
