//! Typed client for Radarr / Sonarr / Lidarr's `/api/v3/notification`
//! webhook-management endpoint. All three are servarr forks and share
//! the same JSON shape. `Kind` discriminates which event flags to
//! enable on create.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Radarr,
    Sonarr,
    Lidarr,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "radarr" => Some(Kind::Radarr),
            "sonarr" => Some(Kind::Sonarr),
            "lidarr" => Some(Kind::Lidarr),
            _ => None,
        }
    }
}

/// Subset of the *arr Notification model we care about. Other fields
/// (id, includeHealth, tags, etc.) are deserialized via `#[serde(flatten)]`
/// into `extra` so we round-trip them on update without dropping
/// operator-set values.
#[derive(Debug, Serialize, Deserialize)]
pub struct Notification {
    pub id: i64,
    pub name: String,
    pub implementation: String,
    #[serde(rename = "configContract")]
    pub config_contract: String,
    pub fields: Vec<Field>,
    #[serde(default, rename = "onGrab")]
    pub on_grab: bool,
    #[serde(default, rename = "onDownload")]
    pub on_download: bool,
    #[serde(default, rename = "onUpgrade")]
    pub on_upgrade: bool,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    pub value: serde_json::Value,
}

pub struct Client {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl Client {
    /// Construct a client. Trims trailing `/` from `base_url` so callers
    /// can pass either form. 15-second per-request timeout — generous
    /// for typical homelab latencies, tight enough that an unreachable
    /// *arr fails fast.
    pub fn new(base_url: &str, api_key: &str) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .context("building reqwest client")?,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_parse_known_kinds() {
        assert_eq!(Kind::parse("radarr"), Some(Kind::Radarr));
        assert_eq!(Kind::parse("sonarr"), Some(Kind::Sonarr));
        assert_eq!(Kind::parse("lidarr"), Some(Kind::Lidarr));
    }

    #[test]
    fn kind_parse_rejects_other_strings() {
        assert_eq!(Kind::parse("generic"), None);
        assert_eq!(Kind::parse("webhook"), None);
        assert_eq!(Kind::parse(""), None);
        assert_eq!(Kind::parse("RADARR"), None); // case-sensitive
    }

    #[test]
    fn client_new_trims_trailing_slash() {
        let c = Client::new("http://radarr:7878/", "k").unwrap();
        assert_eq!(c.base_url, "http://radarr:7878");
        let c = Client::new("http://radarr:7878", "k").unwrap();
        assert_eq!(c.base_url, "http://radarr:7878");
    }
}
