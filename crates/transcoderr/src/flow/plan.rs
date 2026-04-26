//! Stream plan: declarative description of what one ffmpeg pass should do.
//!
//! Plan-mutator steps (`plan.video`, `plan.audio.ensure`, `plan.streams.drop_*`,
//! `plan.subs.*`, etc.) build up `ctx.plan` without running ffmpeg. The single
//! `plan.execute` step materializes the plan into ONE ffmpeg invocation. This
//! avoids the multi-pass / multi-tmp problem of the old per-step ffmpeg model
//! and lets users compose orthogonal concerns (audio, subs, cover-art, video)
//! in any order.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StreamPlan {
    pub container: String,
    pub global_input_args: Vec<String>,

    pub video: VideoPlan,
    /// Per-stream-index decisions for the input. true = include in output.
    pub stream_keep: BTreeMap<i64, bool>,

    /// Brand-new audio streams to add (encoded from a seed input stream).
    pub audio_added: Vec<AddedAudio>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VideoPlan {
    pub mode: VideoMode,
    pub crf: Option<i64>,
    pub preset: Option<String>,
    pub preserve_10bit: bool,
    pub hw_prefer: Vec<String>,
    pub hw_fallback_cpu: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VideoMode {
    #[default]
    Copy,
    Encode {
        codec: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddedAudio {
    pub seed_index: i64,
    pub codec: String,
    pub channels: i64,
    pub language: String,
    pub title: String,
}

impl StreamPlan {
    /// Seed the plan from probe data: every stream is initially kept-as-copy,
    /// container defaults to mkv, video is copy.
    pub fn from_probe(probe: &serde_json::Value) -> Self {
        let mut plan = StreamPlan {
            container: "mkv".to_string(),
            ..Default::default()
        };
        if let Some(streams) = probe.get("streams").and_then(|s| s.as_array()) {
            for s in streams {
                let idx = s.get("index").and_then(|v| v.as_i64()).unwrap_or(-1);
                if idx >= 0 {
                    plan.stream_keep.insert(idx, true);
                }
            }
        }
        plan
    }

    /// Drop every input stream where `pred(stream)` returns true.
    pub fn drop_streams_where(
        &mut self,
        probe: &serde_json::Value,
        pred: impl Fn(&serde_json::Value) -> bool,
    ) -> usize {
        let Some(streams) = probe.get("streams").and_then(|s| s.as_array()) else {
            return 0;
        };
        let mut dropped = 0;
        for s in streams {
            let idx = s.get("index").and_then(|v| v.as_i64()).unwrap_or(-1);
            if idx < 0 {
                continue;
            }
            if pred(s) {
                self.stream_keep.insert(idx, false);
                dropped += 1;
            }
        }
        dropped
    }

    pub fn kept_indices(&self) -> Vec<i64> {
        let mut v: Vec<i64> = self
            .stream_keep
            .iter()
            .filter_map(|(idx, keep)| if *keep { Some(*idx) } else { None })
            .collect();
        v.sort();
        v
    }
}

/// Reads `ctx.steps["_plan"]` if present and returns it. Steps that mutate the
/// plan call this to load, then [`save_plan`] to persist.
pub fn load_plan(ctx: &crate::flow::Context) -> Option<StreamPlan> {
    ctx.steps
        .get("_plan")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
}

pub fn save_plan(ctx: &mut crate::flow::Context, plan: &StreamPlan) {
    if let Ok(v) = serde_json::to_value(plan) {
        ctx.steps.insert("_plan".to_string(), v);
    }
}

pub fn require_plan(ctx: &crate::flow::Context) -> anyhow::Result<StreamPlan> {
    load_plan(ctx).ok_or_else(|| {
        anyhow::anyhow!("no plan in context — run `plan.init` first to seed the stream plan")
    })
}
