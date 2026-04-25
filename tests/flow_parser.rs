use transcoderr::flow::{parse_flow, Flow, Step, Trigger};

#[test]
fn parses_minimal_linear_flow() {
    let yaml = r#"
name: reencode-x265
triggers:
  - radarr: [downloaded]
steps:
  - id: probe
    use: probe
  - id: encode
    use: transcode
    with:
      codec: x265
      crf: 22
  - id: swap
    use: output
    with:
      mode: replace
"#;
    let flow: Flow = parse_flow(yaml).unwrap();
    assert_eq!(flow.name, "reencode-x265");
    assert_eq!(flow.triggers, vec![Trigger::Radarr(vec!["downloaded".into()])]);
    assert_eq!(flow.steps.len(), 3);
    assert_eq!(flow.steps[1].use_, "transcode");
    assert_eq!(flow.steps[1].with.get("crf").and_then(|v| v.as_i64()), Some(22));
}

#[test]
fn rejects_unknown_step_use() {
    let yaml = r#"
name: bad
triggers:
  - radarr: [downloaded]
steps:
  - use: not_a_real_step
"#;
    let err = parse_flow(yaml).unwrap_err();
    assert!(err.to_string().contains("unknown step"), "got: {err}");
}
