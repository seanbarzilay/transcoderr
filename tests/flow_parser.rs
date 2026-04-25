use transcoderr::flow::{parse_flow, Flow, Node, Trigger};

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
    match &flow.steps[1] {
        Node::Step { use_, with, .. } => {
            assert_eq!(use_, "transcode");
            assert_eq!(with.get("crf").and_then(|v| v.as_i64()), Some(22));
        }
        _ => panic!("expected Step node at index 1"),
    }
}

#[test]
fn rejects_unknown_trigger_kind() {
    let yaml = r#"
name: bad-trigger
triggers:
  - plex: my-source
steps:
  - use: probe
"#;
    let err = parse_flow(yaml).unwrap_err();
    assert!(
        err.to_string().contains("unknown trigger kind"),
        "got: {err}"
    );
}

/// Parse-time step validation is now permissive (unknown step names are
/// accepted and resolved at runtime via the plugin registry). This test
/// verifies that an unknown step name parses successfully and the name
/// is preserved in the AST.
#[test]
fn unknown_step_use_preserved_in_ast() {
    let yaml = r#"
name: bad
triggers:
  - radarr: [downloaded]
steps:
  - use: not_a_real_step
"#;
    let flow = parse_flow(yaml).unwrap();
    match &flow.steps[0] {
        Node::Step { use_, .. } => assert_eq!(use_, "not_a_real_step"),
        _ => panic!("expected Step node"),
    }
}
