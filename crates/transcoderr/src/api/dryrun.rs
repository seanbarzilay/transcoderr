use crate::{
    flow::{expr, parse_flow, Context, Node},
    http::AppState,
};
use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct DryRunReq {
    pub yaml: String,
    pub file_path: String,
    pub probe: Option<serde_json::Value>,
}

#[derive(Serialize)]
pub struct DryRunStep {
    pub id: Option<String>,
    pub kind: &'static str,
    pub use_or_label: String,
    pub with: Option<serde_json::Value>,
}

#[derive(Serialize)]
pub struct DryRunResp {
    pub steps: Vec<DryRunStep>,
    pub probe: serde_json::Value,
}

pub async fn dry_run(
    State(_state): State<AppState>,
    Json(req): Json<DryRunReq>,
) -> Json<DryRunResp> {
    let flow = match parse_flow(&req.yaml) {
        Ok(f) => f,
        Err(e) => {
            return Json(DryRunResp {
                steps: vec![DryRunStep {
                    id: None,
                    kind: "step",
                    use_or_label: format!("parse error: {e}"),
                    with: None,
                }],
                probe: serde_json::Value::Null,
            })
        }
    };
    let probe = match req.probe {
        Some(p) => p,
        None => crate::ffmpeg::ffprobe_json(std::path::Path::new(&req.file_path))
            .await
            .unwrap_or(serde_json::Value::Null),
    };
    let mut ctx = Context::for_file(&req.file_path);
    ctx.probe = Some(probe.clone());

    let mut out = vec![];
    walk(&flow.steps, &mut ctx, &mut out);
    Json(DryRunResp { steps: out, probe })
}

fn walk(nodes: &[Node], ctx: &mut Context, out: &mut Vec<DryRunStep>) {
    for n in nodes {
        match n {
            Node::Step { id, use_, with, .. } => out.push(DryRunStep {
                id: id.clone(),
                kind: "step",
                use_or_label: use_.clone(),
                with: Some(serde_json::to_value(with).unwrap()),
            }),
            Node::Conditional {
                id,
                if_,
                then_,
                else_,
            } => {
                let v = expr::eval_bool(if_, ctx).unwrap_or(false);
                let kind = if v { "if-true" } else { "if-false" };
                out.push(DryRunStep {
                    id: id.clone(),
                    kind,
                    use_or_label: if_.clone(),
                    with: None,
                });
                if v {
                    walk(then_, ctx, out);
                } else if let Some(e) = else_ {
                    walk(e, ctx, out);
                }
            }
            Node::Return { return_ } => {
                out.push(DryRunStep {
                    id: None,
                    kind: "return",
                    use_or_label: return_.clone(),
                    with: None,
                });
                return;
            }
        }
    }
}
