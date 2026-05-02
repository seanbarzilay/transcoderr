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
}
