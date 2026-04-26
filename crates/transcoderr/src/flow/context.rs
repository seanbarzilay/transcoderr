use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// The evolving state passed between steps. Snapshotted to checkpoints.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Context {
    pub file: FileMeta,
    pub probe: Option<Value>,
    pub steps: BTreeMap<String, Value>,
    /// Populated by the engine when running `on_failure` handlers. Templates can
    /// reference `{{ failed.id }}`, `{{ failed.use_ }}`, and `{{ failed.error }}`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub failed: Option<FailedInfo>,
    /// Cooperative cancellation. Set by the engine before running each job. Steps
    /// that spawn long-running subprocesses (ffmpeg) clone this and race it against
    /// `child.wait()` so a Cancel from the API kills the child immediately.
    /// Skipped from serialization — never persisted to checkpoints.
    #[serde(skip)]
    pub cancel: Option<tokio_util::sync::CancellationToken>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailedInfo {
    pub id: String,
    pub use_: String,
    pub error: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileMeta {
    pub path: String,
    pub size_bytes: Option<u64>,
}

impl Context {
    pub fn for_file(path: impl Into<String>) -> Self {
        Self {
            file: FileMeta { path: path.into(), size_bytes: None },
            ..Default::default()
        }
    }

    pub fn record_step_output(&mut self, id: &str, out: Value) {
        self.steps.insert(id.to_string(), out);
    }

    pub fn to_snapshot(&self) -> String {
        serde_json::to_string(self).expect("context serializable")
    }

    pub fn from_snapshot(s: &str) -> anyhow::Result<Self> {
        Ok(serde_json::from_str(s)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn snapshot_round_trip() {
        let mut c = Context::for_file("/m/Dune.mkv");
        c.probe = Some(json!({"video": {"codec": "h264"}}));
        c.record_step_output("probe", json!({"ok": true}));
        let s = c.to_snapshot();
        let r = Context::from_snapshot(&s).unwrap();
        assert_eq!(r.file.path, "/m/Dune.mkv");
        assert_eq!(r.probe.as_ref().unwrap()["video"]["codec"], "h264");
        assert_eq!(r.steps.get("probe").unwrap()["ok"], true);
    }
}
