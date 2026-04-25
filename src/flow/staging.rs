use crate::flow::Context;
use std::path::PathBuf;

/// Path helpers for steps that produce intermediate `.transcoderr.tmp.*` files.
///
/// The flow engine runs steps sequentially, but historically each transformer step
/// (transcode, remux, strip.tracks, audio.ensure, extract.subs) read from
/// `ctx.file.path` (the *original* file). That meant chaining two of them in one flow
/// produced surprising results: the second step re-read the original and overwrote the
/// first step's tmp.
///
/// This module fixes that. Each step calls [`next_io`] which returns:
/// - `input`: the latest staged tmp file if one exists, else the original file.
/// - `output`: a fresh, unique tmp filename next to the original.
///
/// After a step finishes, [`record_output`] writes the new `output_path` into
/// `ctx.steps["transcode"]` (preserving any extra metadata the step wants to attach)
/// and bumps an internal counter so the next call to [`next_io`] produces a different
/// filename. The downstream `output: replace` step still reads
/// `ctx.steps["transcode"]["output_path"]` and renames it over `ctx.file.path`.
pub fn next_io(ctx: &Context, ext: &str) -> (PathBuf, PathBuf) {
    let original = PathBuf::from(&ctx.file.path);
    let current_input = ctx
        .steps
        .get("transcode")
        .and_then(|v| v.get("output_path"))
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .unwrap_or_else(|| original.clone());
    let counter = ctx
        .steps
        .get("_tcr_chain")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let stem = original
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("output");
    let parent = original
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let next = parent.join(format!("{stem}.tcr-{counter:02}.tmp.{ext}"));
    (current_input, next)
}

/// Persist the new staged output path into the context and tick the chain counter.
/// `extras` is merged into `ctx.steps["transcode"]` so callers can record fields like
/// `codec`, `hw`, `added_audio_index`, etc. without overwriting `output_path`.
pub fn record_output(
    ctx: &mut Context,
    output_path: &std::path::Path,
    extras: serde_json::Value,
) {
    // Read the previous staged path BEFORE overwriting ctx.steps["transcode"],
    // so we can delete it once the new step has superseded it. Without this,
    // every transformer step in a chain leaves a `.tcr-NN.tmp.*` orphan next
    // to the original (e.g. audio.ensure → transcode left a 7.6 GB tcr-00).
    let previous_output: Option<String> = ctx
        .steps
        .get("transcode")
        .and_then(|v| v.get("output_path"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let mut map = match extras {
        serde_json::Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    map.insert(
        "output_path".into(),
        serde_json::json!(output_path.to_string_lossy()),
    );
    ctx.steps
        .insert("transcode".into(), serde_json::Value::Object(map));

    if let Some(prev) = previous_output {
        let new_str = output_path.to_string_lossy().to_string();
        if prev != new_str && prev != ctx.file.path {
            // Best-effort: ignore errors. If it's already gone or in use, fine.
            let _ = std::fs::remove_file(&prev);
        }
    }

    let counter = ctx
        .steps
        .get("_tcr_chain")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    ctx.steps
        .insert("_tcr_chain".into(), serde_json::json!(counter + 1));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn first_call_uses_original_input_and_unique_output() {
        let ctx = Context::for_file("/m/Dune.mkv");
        let (input, output) = next_io(&ctx, "mkv");
        assert_eq!(input.to_string_lossy(), "/m/Dune.mkv");
        assert_eq!(output.to_string_lossy(), "/m/Dune.mkv.tcr-00.tmp.mkv");
    }

    #[test]
    fn second_call_reads_from_first_output_and_uses_new_filename() {
        let mut ctx = Context::for_file("/m/Dune.mkv");
        let (_, first_out) = next_io(&ctx, "mkv");
        record_output(&mut ctx, &first_out, json!({}));

        let (input, second_out) = next_io(&ctx, "mkv");
        assert_eq!(input, first_out);
        assert_ne!(input, second_out);
        assert_eq!(second_out.to_string_lossy(), "/m/Dune.mkv.tcr-01.tmp.mkv");
    }
}
