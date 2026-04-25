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
    // Template is: literal text with {{ expr }} placeholders.
    let mut out = String::with_capacity(template.len());
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'{' && bytes[i + 1] == b'{' {
            let end = template[i + 2..]
                .find("}}")
                .ok_or_else(|| anyhow::anyhow!("unterminated {{"))?;
            let expr = template[i + 2..i + 2 + end].trim();
            let program = Program::compile(expr).map_err(|e| anyhow::anyhow!("compile: {e:?}"))?;
            let mut cel = CelCtx::default();
            bind_context(&mut cel, ctx);
            let v = program.execute(&cel).map_err(|e| anyhow::anyhow!("exec: {e:?}"))?;
            out.push_str(&format_cel(&v));
            i = i + 2 + end + 2;
        } else {
            out.push(bytes[i] as char);
            i += 1;
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
}
