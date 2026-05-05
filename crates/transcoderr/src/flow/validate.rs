//! Static validation for flow YAML. Surfaces YAML parse errors and
//! every CEL compile error in `if:` conditionals and `{{ ... }}`
//! templates. The runtime evaluator (engine.rs) silently treats
//! compile/exec failures in `if:` as `false`, so without this validator
//! a typo in a guard expression silently disables the branch — exactly
//! the failure mode that ate the first cut of the hevc fast-path.
//!
//! Pure data: no I/O, no DB, no execution. Safe to call from any
//! request handler.

use crate::flow::{expr, model::Node, parser::parse_flow};
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ValidationIssue {
    /// Human-readable JSON-pointer-ish path to the offending value, e.g.
    /// `steps[5].if` or `steps[3].with.template`.
    pub path: String,
    pub kind: IssueKind,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum IssueKind {
    /// The whole YAML failed to parse — only this issue is returned.
    YamlParseError,
    /// A CEL `if:` expression failed to compile.
    ConditionCompileError,
    /// A `{{ ... }}` template inside a step's `with:` failed to compile
    /// (or had an unterminated `{{`).
    TemplateCompileError,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ValidationReport {
    pub ok: bool,
    pub issues: Vec<ValidationIssue>,
}

/// Validate a flow YAML string. Returns a report listing every CEL
/// compile error found in `if:` expressions and `{{ ... }}` templates.
/// A YAML parse error short-circuits the walk and returns a single
/// `YamlParseError` issue.
pub fn validate_flow_yaml(yaml: &str) -> ValidationReport {
    let flow = match parse_flow(yaml) {
        Ok(f) => f,
        Err(e) => {
            return ValidationReport {
                ok: false,
                issues: vec![ValidationIssue {
                    path: "(root)".into(),
                    kind: IssueKind::YamlParseError,
                    message: e.to_string(),
                }],
            }
        }
    };

    let mut issues = Vec::new();
    walk_nodes("steps", &flow.steps, &mut issues);
    if let Some(of) = &flow.on_failure {
        walk_nodes("on_failure", of, &mut issues);
    }

    ValidationReport {
        ok: issues.is_empty(),
        issues,
    }
}

fn walk_nodes(prefix: &str, nodes: &[Node], issues: &mut Vec<ValidationIssue>) {
    for (idx, n) in nodes.iter().enumerate() {
        let here = format!("{prefix}[{idx}]");
        match n {
            Node::Step { with, .. } => {
                walk_with(&here, with, issues);
            }
            Node::Conditional {
                if_, then_, else_, ..
            } => {
                if let Err(e) = expr::compile(if_) {
                    issues.push(ValidationIssue {
                        path: format!("{here}.if"),
                        kind: IssueKind::ConditionCompileError,
                        message: e,
                    });
                }
                walk_nodes(&format!("{here}.then"), then_, issues);
                if let Some(e) = else_ {
                    walk_nodes(&format!("{here}.else"), e, issues);
                }
            }
            Node::Return { .. } => {}
        }
    }
}

/// Walk every string leaf under a step's `with:` map and validate any
/// `{{ ... }}` template found there. Conservative: a string containing
/// no `{{` is left alone, so non-template config values (URLs, codec
/// names, channel counts as text) don't trigger spurious errors.
fn walk_with(
    here: &str,
    with: &std::collections::BTreeMap<String, Value>,
    issues: &mut Vec<ValidationIssue>,
) {
    for (k, v) in with {
        walk_value(&format!("{here}.with.{k}"), v, issues);
    }
}

fn walk_value(path: &str, v: &Value, issues: &mut Vec<ValidationIssue>) {
    match v {
        Value::String(s) => {
            if !s.contains("{{") {
                return;
            }
            if let Err(msg) = expr::validate_template(s) {
                issues.push(ValidationIssue {
                    path: path.to_string(),
                    kind: IssueKind::TemplateCompileError,
                    message: msg,
                });
            }
        }
        Value::Array(arr) => {
            for (i, child) in arr.iter().enumerate() {
                walk_value(&format!("{path}[{i}]"), child, issues);
            }
        }
        Value::Object(map) => {
            for (k, child) in map {
                walk_value(&format!("{path}.{k}"), child, issues);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yaml_with_steps(extra: &str) -> String {
        format!("name: t\nenabled: true\ntriggers:\n  - radarr: [downloaded]\nsteps:\n{extra}")
    }

    #[test]
    fn clean_flow_validates() {
        let yaml = yaml_with_steps(
            "  - id: a\n    if: probe.streams != null\n    then:\n      - return: ok\n",
        );
        let r = validate_flow_yaml(&yaml);
        assert!(r.ok, "expected clean flow, got {:?}", r.issues);
        assert!(r.issues.is_empty());
    }

    #[test]
    fn yaml_parse_error_short_circuits() {
        let r = validate_flow_yaml("not: valid: yaml: at all:");
        assert!(!r.ok);
        assert_eq!(r.issues.len(), 1);
        assert_eq!(r.issues[0].kind, IssueKind::YamlParseError);
    }

    #[test]
    fn broken_if_is_reported() {
        let yaml = yaml_with_steps(
            "  - id: a\n    if: \"this is not valid cel ((\"\n    then:\n      - return: x\n",
        );
        let r = validate_flow_yaml(&yaml);
        assert!(!r.ok);
        assert_eq!(r.issues.len(), 1);
        assert_eq!(r.issues[0].kind, IssueKind::ConditionCompileError);
        assert_eq!(r.issues[0].path, "steps[0].if");
    }

    #[test]
    fn broken_template_in_with_is_reported() {
        let yaml = yaml_with_steps(
            "  - id: n\n    use: notify\n    with:\n      channel: tg-main\n      template: \"hello {{ this is not valid (( }}\"\n",
        );
        let r = validate_flow_yaml(&yaml);
        assert!(!r.ok);
        assert_eq!(r.issues.len(), 1);
        assert_eq!(r.issues[0].kind, IssueKind::TemplateCompileError);
        assert_eq!(r.issues[0].path, "steps[0].with.template");
    }

    #[test]
    fn unterminated_template_is_reported() {
        let yaml = yaml_with_steps(
            "  - id: n\n    use: notify\n    with:\n      template: \"hello {{ file.path \"\n",
        );
        let r = validate_flow_yaml(&yaml);
        assert!(!r.ok);
        assert_eq!(r.issues.len(), 1);
        assert_eq!(r.issues[0].kind, IssueKind::TemplateCompileError);
        assert!(
            r.issues[0].message.contains("unterminated"),
            "got: {}",
            r.issues[0].message
        );
    }

    #[test]
    fn nested_conditionals_walked_with_correct_paths() {
        let yaml = yaml_with_steps(
            "  - id: outer\n    if: \"true\"\n    then:\n      - id: inner\n        if: \"this is bad ((\"\n        then:\n          - return: x\n",
        );
        let r = validate_flow_yaml(&yaml);
        assert!(!r.ok);
        assert_eq!(r.issues.len(), 1);
        assert_eq!(r.issues[0].path, "steps[0].then[0].if");
    }

    #[test]
    fn on_failure_is_walked() {
        let yaml = "name: t\nenabled: true\ntriggers:\n  - radarr: [downloaded]\nsteps:\n  - use: noop\non_failure:\n  - use: notify\n    with:\n      template: \"err {{ broken (( }}\"\n";
        let r = validate_flow_yaml(yaml);
        assert!(!r.ok);
        assert_eq!(r.issues[0].path, "on_failure[0].with.template");
    }

    #[test]
    fn multiple_issues_all_reported() {
        let yaml = yaml_with_steps(
            "  - id: a\n    if: \"bad ((\"\n    then:\n      - return: x\n  - id: b\n    use: notify\n    with:\n      template: \"{{ also bad (( }}\"\n",
        );
        let r = validate_flow_yaml(&yaml);
        assert_eq!(r.issues.len(), 2, "got {:?}", r.issues);
    }

    #[test]
    fn plain_strings_in_with_are_not_flagged() {
        // Non-template strings (URLs, codec names) shouldn't trip the
        // template walker.
        let yaml = yaml_with_steps(
            "  - id: a\n    use: webhook\n    with:\n      url: \"http://example.com/hook\"\n      method: POST\n",
        );
        let r = validate_flow_yaml(&yaml);
        assert!(r.ok, "got {:?}", r.issues);
    }

    #[test]
    fn template_in_nested_object_value_is_walked() {
        // webhook.headers is a map; templates inside its values should
        // still be validated.
        let yaml = yaml_with_steps(
            "  - id: a\n    use: webhook\n    with:\n      headers:\n        Authorization: \"Bearer {{ broken (( }}\"\n",
        );
        let r = validate_flow_yaml(&yaml);
        assert!(!r.ok);
        assert_eq!(r.issues[0].path, "steps[0].with.headers.Authorization");
    }
}
