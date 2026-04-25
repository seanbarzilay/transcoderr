use transcoderr::flow::{parse_flow, Node};

#[test]
fn parses_conditional_and_return() {
    let yaml = r#"
name: cond
triggers:
  - radarr: [downloaded]
match:
  expr: file.size_gb > 1
steps:
  - id: probe
    use: probe
  - id: gate
    if: probe.video.codec == "hevc"
    then:
      - return: skipped
    else:
      - id: enc
        use: transcode
        with: { codec: x265 }
on_failure:
  - use: notify
    with: { channel: discord, template: "fail {{file.name}}" }
"#;
    let flow = parse_flow(yaml).unwrap();
    assert_eq!(flow.match_expr(), Some("file.size_gb > 1"));
    assert!(flow.on_failure.is_some());
    assert_eq!(flow.steps.len(), 2);
    match &flow.steps[1] {
        Node::Conditional { if_, then_, else_, .. } => {
            assert_eq!(if_, "probe.video.codec == \"hevc\"");
            assert_eq!(then_.len(), 1);
            assert!(matches!(then_[0], Node::Return { .. }));
            assert_eq!(else_.as_ref().unwrap().len(), 1);
        }
        _ => panic!("expected conditional"),
    }
}
