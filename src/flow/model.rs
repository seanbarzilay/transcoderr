use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Flow {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub triggers: Vec<Trigger>,
    #[serde(default, rename = "match")]
    pub match_block: Option<MatchBlock>,
    #[serde(default)]
    pub concurrency: Option<u32>,
    pub steps: Vec<Node>,
    #[serde(default)]
    pub on_failure: Option<Vec<Node>>,
}

fn default_true() -> bool { true }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MatchBlock {
    pub expr: String,
}

impl Flow {
    pub fn match_expr(&self) -> Option<&str> {
        self.match_block.as_ref().map(|m| m.expr.as_str())
    }
}

/// A trigger source.  In YAML it looks like `radarr: [downloaded, upgraded]`
/// which serde_yaml parses as a single-key map.
#[derive(Debug, Clone, PartialEq)]
pub enum Trigger {
    Radarr(Vec<String>),
    Sonarr(Vec<String>),
    Lidarr(Vec<String>),
    Webhook(String),
}

impl<'de> serde::Deserialize<'de> for Trigger {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        let map: BTreeMap<String, serde_yaml::Value> = serde::Deserialize::deserialize(d)?;
        let mut iter = map.into_iter();
        let (key, val) = iter.next().ok_or_else(|| D::Error::custom("trigger map is empty"))?;
        if iter.next().is_some() { return Err(D::Error::custom("trigger map has multiple keys")); }
        match key.as_str() {
            "radarr"  => Ok(Trigger::Radarr( serde_yaml::from_value(val).map_err(|e| D::Error::custom(format!("radarr: {e}")))? )),
            "sonarr"  => Ok(Trigger::Sonarr( serde_yaml::from_value(val).map_err(|e| D::Error::custom(format!("sonarr: {e}")))? )),
            "lidarr"  => Ok(Trigger::Lidarr( serde_yaml::from_value(val).map_err(|e| D::Error::custom(format!("lidarr: {e}")))? )),
            "webhook" => Ok(Trigger::Webhook(serde_yaml::from_value(val).map_err(|e| D::Error::custom(format!("webhook: {e}")))? )),
            other     => Err(D::Error::custom(format!("unknown trigger kind {other:?}"))),
        }
    }
}

impl serde::Serialize for Trigger {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = s.serialize_map(Some(1))?;
        match self {
            Trigger::Radarr(events)  => map.serialize_entry("radarr", events)?,
            Trigger::Sonarr(events)  => map.serialize_entry("sonarr", events)?,
            Trigger::Lidarr(events)  => map.serialize_entry("lidarr", events)?,
            Trigger::Webhook(source) => map.serialize_entry("webhook", source)?,
        }
        map.end()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Node {
    Conditional {
        #[serde(default)]
        id: Option<String>,
        #[serde(rename = "if")]
        if_: String,
        #[serde(rename = "then")]
        then_: Vec<Node>,
        #[serde(rename = "else", default)]
        else_: Option<Vec<Node>>,
    },
    Return {
        #[serde(rename = "return")]
        return_: String,
    },
    Step {
        #[serde(default)]
        id: Option<String>,
        #[serde(rename = "use")]
        use_: String,
        #[serde(default)]
        with: BTreeMap<String, Value>,
        #[serde(default)]
        retry: Option<Retry>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Retry {
    pub max: u32,
    #[serde(default)]
    pub on: Option<String>,
}
