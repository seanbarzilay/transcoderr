use crate::flow::Context;
use cel_interpreter::{Context as CelCtx, Program, Value as CelValue};
use serde_json::Value;

pub fn eval_bool(expr: &str, ctx: &Context) -> anyhow::Result<bool> {
    let program = Program::compile(expr).map_err(|e| anyhow::anyhow!("compile: {e:?}"))?;
    let mut cel = CelCtx::default();
    bind_context(&mut cel, ctx);
    let v = program.execute(&cel).map_err(|e| anyhow::anyhow!("exec: {e:?}"))?;
    Ok(matches!(v, CelValue::Bool(true)))
}

pub fn eval_string_template(template: &str, ctx: &Context) -> anyhow::Result<String> {
    // Template is: literal text with {{ expr }} placeholders. Walk the string with
    // UTF-8 aware indices so multibyte characters in the literal text (✗, ✓, em-dashes,
    // accented letters) round-trip cleanly. The previous version cast each byte to a
    // char, which mangled any non-ASCII char into per-byte Latin-1 codepoints.
    let mut out = String::with_capacity(template.len());
    let mut i = 0usize;
    while i < template.len() {
        if template[i..].starts_with("{{") {
            let after = i + 2;
            let end = template[after..]
                .find("}}")
                .ok_or_else(|| anyhow::anyhow!("unterminated {{"))?;
            let expr = template[after..after + end].trim();
            let program = Program::compile(expr).map_err(|e| anyhow::anyhow!("compile: {e:?}"))?;
            let mut cel = CelCtx::default();
            bind_context(&mut cel, ctx);
            let v = program.execute(&cel).map_err(|e| anyhow::anyhow!("exec: {e:?}"))?;
            out.push_str(&format_cel(&v));
            i = after + end + 2;
        } else {
            // Push exactly one full UTF-8 char and advance by its byte length.
            let next_char = template[i..]
                .chars()
                .next()
                .expect("non-empty slice has at least one char");
            out.push(next_char);
            i += next_char.len_utf8();
        }
    }
    Ok(out)
}

fn bind_context(cel: &mut CelCtx, ctx: &Context) {
    let v = serde_json::to_value(ctx).unwrap_or(Value::Null);
    if let Value::Object(map) = v {
        for (k, vv) in map {
            cel.add_variable(k, vv).ok();
        }
    }
}

fn format_cel(v: &CelValue) -> String {
    match v {
        // In v0.10 String wraps Arc<String>, so we dereference it.
        CelValue::String(s) => s.as_ref().clone(),
        CelValue::Int(i) => i.to_string(),
        CelValue::UInt(u) => u.to_string(),
        CelValue::Float(f) => f.to_string(),
        CelValue::Bool(b) => b.to_string(),
        CelValue::Null => "null".into(),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn evaluates_bool_expression_against_context() {
        let mut ctx = Context::for_file("/m/Dune.mkv");
        ctx.probe = Some(json!({ "video": { "codec": "h264" } }));
        assert!(eval_bool("probe.video.codec == \"h264\"", &ctx).unwrap());
        assert!(!eval_bool("probe.video.codec == \"hevc\"", &ctx).unwrap());
    }

    #[test]
    fn interpolates_template() {
        let ctx = Context::for_file("/m/Dune.mkv");
        let s = eval_string_template("file is {{ file.path }}", &ctx).unwrap();
        assert_eq!(s, "file is /m/Dune.mkv");
    }

    #[test]
    fn template_preserves_multibyte_chars() {
        let ctx = Context::for_file("/m/Dune.mkv");
        let s = eval_string_template("✗ {{ file.path }} — ✓ done", &ctx).unwrap();
        assert_eq!(s, "✗ /m/Dune.mkv — ✓ done");
    }

    /// Step outputs (whether from built-in steps or plugins via
    /// `context_set`) live under `ctx.steps.<key>` and are reachable from
    /// templates as `{{ steps.<key>.<field> }}`. The bare `{{ <key>... }}`
    /// form is *not* available -- bind_context only exposes the top-level
    /// Context fields (file, probe, steps, failed). This pins the contract
    /// down so a future binder change doesn't silently break notify
    /// templates that plugins document.
    #[test]
    fn template_reads_step_output_via_steps_prefix() {
        let mut ctx = Context::for_file("/m/Dune.mkv");
        ctx.steps.insert(
            "size_report".into(),
            json!({"ratio_pct": 38.4, "after_bytes": 7659011840u64}),
        );
        let s = eval_string_template(
            "saved {{ steps.size_report.ratio_pct }}% to {{ steps.size_report.after_bytes }}",
            &ctx,
        )
        .unwrap();
        assert!(s.contains("38.4"), "ratio missing: {s}");
        assert!(s.contains("7659011840"), "after_bytes missing: {s}");

        // The bare-key form has no binding and must fail with the same
        // UndeclaredReference error operators saw in the wild before the
        // size-report README was corrected.
        let err = eval_string_template("{{ size_report.ratio_pct }}", &ctx).unwrap_err();
        assert!(
            err.to_string().contains("UndeclaredReference"),
            "expected UndeclaredReference, got: {err}"
        );
    }
}
