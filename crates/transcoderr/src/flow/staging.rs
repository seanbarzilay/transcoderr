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
    // The job id makes the staged path unique per run. Without it the name is
    // a pure function of the original filename and a counter that always starts
    // at 0, so two runs over the same media file compute byte-identical staged
    // paths and hand them to ffmpeg with `-y`. Both write the same file, and
    // whichever `output: replace` finishes last renames the interleaved result
    // over the original — the source is gone and the replacement is unplayable.
    // Every webhook enqueues one job per matching flow for the same path, so a
    // single Radarr event with two enabled flows is enough to trigger it.
    //
    // `db::jobs::claim_next` also refuses to claim a job whose file is already
    // being processed; this is the second line of defence, and it makes an
    // orphaned tmp file traceable to the run that left it behind.
    let next = match ctx.job_id {
        Some(job) => parent.join(format!("{stem}.tcr-j{job}-{counter:02}.tmp.{ext}")),
        None => parent.join(format!("{stem}.tcr-{counter:02}.tmp.{ext}")),
    };
    (current_input, next)
}

/// Persist the new staged output path into the context and tick the chain counter.
/// `extras` is merged into `ctx.steps["transcode"]` so callers can record fields like
/// `codec`, `hw`, `added_audio_index`, etc. without overwriting `output_path`.
pub fn record_output(ctx: &mut Context, output_path: &std::path::Path, extras: serde_json::Value) {
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
        // Skip URL-shaped chain heads (e.g. `bluray:/path/to.iso` written by
        // iso.extract). They aren't real filesystem paths, so there's nothing
        // to delete. Keeping the guard explicit avoids a no-op ENOENT call on
        // every chain step that follows iso.extract.
        if prev != new_str && prev != ctx.file.path && !prev.starts_with("bluray:") {
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

/// The current input path for steps that consume the staging chain.
/// Returns the latest staged tmp file (chain head) if one exists, else the
/// original `ctx.file.path`. Used by `probe` so it sees what transformer
/// steps have produced upstream (e.g. an extracted M2TS from `iso.extract`).
pub fn current_input(ctx: &Context) -> &str {
    ctx.steps
        .get("transcode")
        .and_then(|v| v.get("output_path"))
        .and_then(|v| v.as_str())
        .unwrap_or(&ctx.file.path)
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

    #[test]
    fn staged_output_is_scoped_to_the_job() {
        let mut ctx = Context::for_file("/m/Dune.mkv");
        ctx.job_id = Some(42);
        let (_, output) = next_io(&ctx, "mkv");
        assert_eq!(output.to_string_lossy(), "/m/Dune.mkv.tcr-j42-00.tmp.mkv");
    }

    #[test]
    fn two_jobs_on_the_same_file_never_share_a_staged_path() {
        // The data-loss case: one webhook enqueues a job per matching flow
        // for the same file. Identical staged paths meant two ffmpeg
        // processes writing one file, and `output: replace` renaming the
        // interleaved result over the original.
        let mut a = Context::for_file("/m/Dune.mkv");
        a.job_id = Some(1);
        let mut b = Context::for_file("/m/Dune.mkv");
        b.job_id = Some(2);

        let (_, out_a) = next_io(&a, "mkv");
        let (_, out_b) = next_io(&b, "mkv");
        assert_ne!(
            out_a, out_b,
            "concurrent jobs on one file must not target the same tmp path"
        );
    }

    #[test]
    fn job_scoped_names_still_advance_with_the_chain_counter() {
        let mut ctx = Context::for_file("/m/Dune.mkv");
        ctx.job_id = Some(7);
        let (_, first) = next_io(&ctx, "mkv");
        record_output(&mut ctx, &first, json!({}));
        let (input, second) = next_io(&ctx, "mkv");

        assert_eq!(input, first);
        assert_eq!(second.to_string_lossy(), "/m/Dune.mkv.tcr-j7-01.tmp.mkv");
    }

    #[test]
    fn current_input_returns_file_path_when_no_chain() {
        let ctx = Context::for_file("/m/Dune.mkv");
        assert_eq!(current_input(&ctx), "/m/Dune.mkv");
    }

    #[test]
    fn current_input_returns_chain_head_when_present() {
        let mut ctx = Context::for_file("/m/Dune.iso");
        record_output(
            &mut ctx,
            std::path::Path::new("/m/Dune.iso.tcr-00.tmp.m2ts"),
            json!({}),
        );
        assert_eq!(current_input(&ctx), "/m/Dune.iso.tcr-00.tmp.m2ts");
    }
}
