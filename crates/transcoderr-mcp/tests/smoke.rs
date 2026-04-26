#[test]
fn binary_builds_and_help_works() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_transcoderr-mcp"))
        .arg("--help").output().expect("run --help");
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("--url"), "got: {s}");
    assert!(s.contains("--token"), "got: {s}");
}
