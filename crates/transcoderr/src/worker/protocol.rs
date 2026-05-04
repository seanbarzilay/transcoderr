//! Wire envelope + message variants for the worker ↔ coordinator
//! WebSocket protocol.
//!
//! Envelope shape:
//!   { "type": "<kind>", "id": "<uuid>", "payload": {...} }
//!
//! All frames are JSON text. Binary frames are reserved for future use.
//! `id` is a worker-side correlation id for request/response pairs
//! (e.g. register ↔ register_ack); for fire-and-forget messages
//! (heartbeat, the future step_progress) it's still a unique id but
//! the receiver doesn't reply.
//!
//! Piece 1 ships only three message types: `register`,
//! `register_ack`, `heartbeat`. Pieces 3 and 4 add the dispatch + plugin
//! sync variants.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", tag = "type", content = "payload")]
pub enum Message {
    Register(Register),
    RegisterAck(RegisterAck),
    Heartbeat(Heartbeat),
    StepDispatch(StepDispatch),
    StepProgress(StepProgressMsg),
    StepComplete(StepComplete),
    PluginSync(PluginSync),
    StepCancel(StepCancelMsg),
}

/// Wire frame: the message variant plus its correlation id.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Envelope {
    pub id: String,
    #[serde(flatten)]
    pub message: Message,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Register {
    pub name: String,
    pub version: String,
    pub hw_caps: serde_json::Value,
    /// List of step kinds this worker can run. Piece 1 reports a fixed
    /// set; Piece 3 will trim it based on hw + plugins.
    pub available_steps: Vec<String>,
    /// Installed plugins on this worker. Piece 1 reports the discovered
    /// set; Piece 4 makes the coordinator drive this state.
    pub plugin_manifest: Vec<PluginManifestEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginManifestEntry {
    pub name: String,
    pub version: String,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RegisterAck {
    pub worker_id: i64,
    /// Plugins the coordinator wants this worker to have installed.
    /// Piece 1 sends an empty list; Piece 4 fills it in.
    pub plugin_install: Vec<PluginInstall>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginInstall {
    pub name: String,
    pub version: String,
    pub sha256: String,
    pub tarball_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Heartbeat {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StepDispatch {
    pub job_id: i64,
    pub step_id: String,
    /// Step kind ("transcode", "remux", ...). Renamed in JSON to
    /// `use` to match the YAML field operators already know.
    #[serde(rename = "use")]
    pub use_: String,
    pub with: serde_json::Value,
    pub ctx_snapshot: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StepProgressMsg {
    pub job_id: i64,
    pub step_id: String,
    /// "progress" | "log" | marker.kind.
    pub kind: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StepComplete {
    pub job_id: i64,
    pub step_id: String,
    /// "ok" | "failed".
    pub status: String,
    /// Set when status == "failed".
    pub error: Option<String>,
    /// Set when status == "ok" — the updated context to thread back
    /// into the engine for subsequent steps.
    pub ctx_snapshot: Option<String>,
}

/// Coordinator → worker. Tells the worker to abort the in-flight
/// step identified by the envelope's `id` (correlation_id, matching
/// the original `StepDispatch`). Worker side fires the registered
/// `CancellationToken` for that correlation, which propagates
/// through `Context.cancel` to running steps (kills ffmpeg etc.).
///
/// `job_id` and `step_id` are for log context on the worker side;
/// the correlation_id (envelope.id) is the actual lookup key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StepCancelMsg {
    pub job_id: i64,
    pub step_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginSync {
    /// Full intended plugin manifest (NOT a delta). Workers run the
    /// same full-mirror sync logic on every receive.
    pub plugins: Vec<PluginInstall>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn round_trip(env: &Envelope) -> Envelope {
        let s = serde_json::to_string(env).unwrap();
        serde_json::from_str(&s).unwrap()
    }

    #[test]
    fn register_round_trips() {
        let env = Envelope {
            id: "abc".into(),
            message: Message::Register(Register {
                name: "gpu-box-1".into(),
                version: "0.31.0".into(),
                hw_caps: json!({"encoders": ["h264_nvenc"]}),
                available_steps: vec!["plan.execute".into()],
                plugin_manifest: vec![PluginManifestEntry {
                    name: "size-report".into(),
                    version: "0.1.2".into(),
                    sha256: Some("abc123".into()),
                }],
            }),
        };
        assert_eq!(round_trip(&env), env);
    }

    #[test]
    fn register_ack_round_trips() {
        let env = Envelope {
            id: "abc".into(),
            message: Message::RegisterAck(RegisterAck {
                worker_id: 42,
                plugin_install: vec![],
            }),
        };
        assert_eq!(round_trip(&env), env);
    }

    #[test]
    fn heartbeat_round_trips() {
        let env = Envelope {
            id: "h1".into(),
            message: Message::Heartbeat(Heartbeat {}),
        };
        assert_eq!(round_trip(&env), env);
    }

    #[test]
    fn envelope_uses_snake_case_type_tag() {
        // Lock the wire format down: protocol consumers (including the
        // test fixtures and a future Go/Python worker reimplementation)
        // depend on `register_ack` not `RegisterAck`.
        let env = Envelope {
            id: "x".into(),
            message: Message::RegisterAck(RegisterAck {
                worker_id: 1,
                plugin_install: vec![],
            }),
        };
        let s = serde_json::to_string(&env).unwrap();
        assert!(s.contains(r#""type":"register_ack""#), "got: {s}");
    }

    #[test]
    fn step_dispatch_round_trips() {
        let env = Envelope {
            id: "d1".into(),
            message: Message::StepDispatch(StepDispatch {
                job_id: 17,
                step_id: "transcode_0".into(),
                use_: "transcode".into(),
                with: json!({"vcodec": "h265"}),
                ctx_snapshot: r#"{"file":"/tmp/m.mkv"}"#.into(),
            }),
        };
        assert_eq!(round_trip(&env), env);
        let s = serde_json::to_string(&env).unwrap();
        assert!(
            s.contains(r#""type":"step_dispatch""#),
            "snake_case tag: {s}"
        );
        // The "use_" field must serialize as "use".
        assert!(s.contains(r#""use":"transcode""#), "use rename: {s}");
    }

    #[test]
    fn step_progress_round_trips() {
        let env = Envelope {
            id: "d1".into(),
            message: Message::StepProgress(StepProgressMsg {
                job_id: 17,
                step_id: "transcode_0".into(),
                kind: "progress".into(),
                payload: json!({"pct": 42.5}),
            }),
        };
        assert_eq!(round_trip(&env), env);
    }

    #[test]
    fn step_complete_round_trips() {
        let ok = Envelope {
            id: "d1".into(),
            message: Message::StepComplete(StepComplete {
                job_id: 17,
                step_id: "transcode_0".into(),
                status: "ok".into(),
                error: None,
                ctx_snapshot: Some("{}".into()),
            }),
        };
        assert_eq!(round_trip(&ok), ok);

        let fail = Envelope {
            id: "d1".into(),
            message: Message::StepComplete(StepComplete {
                job_id: 17,
                step_id: "transcode_0".into(),
                status: "failed".into(),
                error: Some("timeout".into()),
                ctx_snapshot: None,
            }),
        };
        assert_eq!(round_trip(&fail), fail);
    }

    #[test]
    fn plugin_sync_round_trips() {
        let env = Envelope {
            id: "p1".into(),
            message: Message::PluginSync(PluginSync {
                plugins: vec![PluginInstall {
                    name: "size-report".into(),
                    version: "0.1.2".into(),
                    sha256: "abc123".into(),
                    tarball_url: "https://coord/api/worker/plugins/size-report/tarball".into(),
                }],
            }),
        };
        assert_eq!(round_trip(&env), env);
        let s = serde_json::to_string(&env).unwrap();
        assert!(s.contains(r#""type":"plugin_sync""#), "snake_case tag: {s}");
    }

    #[test]
    fn step_cancel_round_trips() {
        let env = Envelope {
            id: "dsp-abc".into(),
            message: Message::StepCancel(StepCancelMsg {
                job_id: 42,
                step_id: "transcode_0".into(),
            }),
        };
        assert_eq!(round_trip(&env), env);
        let s = serde_json::to_string(&env).unwrap();
        assert!(s.contains(r#""type":"step_cancel""#), "snake_case tag: {s}");
    }
}
