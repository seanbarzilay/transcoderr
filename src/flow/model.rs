use serde::{Deserialize, Deserializer, Serialize, Serializer};
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
    pub steps: Vec<Step>,
}

fn default_true() -> bool { true }

/// A trigger source.  In YAML it looks like `radarr: [downloaded, upgraded]`
/// which serde_yaml parses as a single-key map.
#[derive(Debug, Clone, PartialEq)]
pub enum Trigger {
    Radarr(Vec<String>),  // event names: ["downloaded", "upgraded", ...]
}

impl<'de> Deserialize<'de> for Trigger {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // Each trigger in YAML is a single-key map: { radarr: [event, ...] }
        let map: BTreeMap<String, Vec<String>> = BTreeMap::deserialize(d)?;
        if map.len() != 1 {
            return Err(serde::de::Error::custom(
                format!("trigger must have exactly one key, got {}", map.len())
            ));
        }
        let (key, events) = map.into_iter().next().unwrap();
        match key.as_str() {
            "radarr" => Ok(Trigger::Radarr(events)),
            other => Err(serde::de::Error::custom(
                format!("unknown trigger type {:?}", other)
            )),
        }
    }
}

impl Serialize for Trigger {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = s.serialize_map(Some(1))?;
        match self {
            Trigger::Radarr(events) => map.serialize_entry("radarr", events)?,
        }
        map.end()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Step {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(rename = "use")]
    pub use_: String,
    #[serde(default)]
    pub with: BTreeMap<String, Value>,
}
