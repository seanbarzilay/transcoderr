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
        use serde::de::Error as _;
        // Deserialize as a map with serde_yaml::Value so Phase 2 variants like
        // `webhook: my-source` (single string) can be handled per-key without rework.
        let map: BTreeMap<String, serde_yaml::Value> = BTreeMap::deserialize(d)?;
        let mut iter = map.into_iter();
        let (key, val) = iter.next().ok_or_else(|| D::Error::custom("trigger map is empty"))?;
        if iter.next().is_some() {
            return Err(D::Error::custom("trigger map has multiple keys"));
        }
        match key.as_str() {
            "radarr" => {
                let events: Vec<String> = serde_yaml::from_value(val)
                    .map_err(|e| D::Error::custom(format!("radarr: {e}")))?;
                Ok(Trigger::Radarr(events))
            }
            other => Err(D::Error::custom(format!("unknown trigger kind {other:?}"))),
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
