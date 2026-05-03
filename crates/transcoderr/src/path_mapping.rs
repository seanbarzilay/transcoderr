//! Per-worker path mapping: walks a `serde_json::Value` and rewrites
//! string leaves whose value starts with a configured prefix.
//!
//! Spec: `docs/superpowers/specs/2026-05-03-worker-path-mappings-design.md`
//!
//! - **Boundary rule**: a rule with `from = "/mnt/movies"` matches
//!   `"/mnt/movies"` exactly OR `"/mnt/movies/anything"`, but NOT
//!   `"/mnt/movies-archive/Y.mkv"`. After stripping `from` from the
//!   leading edge, the next char must be `/` or end-of-string.
//! - **Longest-`from` wins**: rules are sorted by `from.len()` desc on
//!   construction; the first match in that order is applied.
//! - **Trailing slash normalisation**: `from = "/mnt/movies/"` and
//!   `from = "/mnt/movies"` produce identical match behavior. We
//!   normalise on construction (strip any trailing `/` characters) so
//!   display stays consistent. Same for `to`. A bare `/` (root) is
//!   left intact.
//! - **Reverse direction** swaps `from` ↔ `to` at apply time; no
//!   separate sorted vector is needed.
//! - Object **keys** are not rewritten — paths live in values.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathMapping {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Default)]
pub struct PathMappings {
    /// Rules sorted by `from.len()` desc so the first matching prefix
    /// is the longest one.
    rules: Vec<PathMapping>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Outbound: rewrite coordinator paths to worker paths.
    /// (replace `from` prefix with `to`.)
    CoordToWorker,
    /// Inbound: rewrite worker paths back to coordinator paths.
    /// (replace `to` prefix with `from`.)
    WorkerToCoord,
}

impl PathMappings {
    /// Construct from already-validated rules. Empty vec → identity.
    pub fn from_rules(rules: Vec<PathMapping>) -> Self {
        let mut rules: Vec<PathMapping> = rules
            .into_iter()
            .map(|r| PathMapping {
                from: strip_trailing_slash(r.from),
                to: strip_trailing_slash(r.to),
            })
            .collect();
        rules.sort_by(|a, b| b.from.len().cmp(&a.from.len()));
        Self { rules }
    }

    /// Parse a `path_mappings_json` column value. NULL/empty → identity.
    /// Returns Err only on malformed JSON.
    pub fn from_json(s: &str) -> anyhow::Result<Self> {
        if s.trim().is_empty() {
            return Ok(Self::default());
        }
        let parsed: Vec<PathMapping> = serde_json::from_str(s)?;
        Ok(Self::from_rules(parsed))
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// View the underlying rules (post-normalisation). Used by the API
    /// layer to echo back what was stored.
    pub fn rules(&self) -> &[PathMapping] {
        &self.rules
    }

    /// Walk `value` in place, rewriting string leaves that match a
    /// rule's prefix. No-op if `is_empty()`.
    pub fn apply(&self, value: &mut Value, dir: Direction) {
        if self.is_empty() {
            return;
        }
        walk(value, &self.rules, dir);
    }
}

fn walk(value: &mut Value, rules: &[PathMapping], dir: Direction) {
    match value {
        Value::String(s) => {
            if let Some(replaced) = try_replace(s, rules, dir) {
                *s = replaced;
            }
        }
        Value::Array(items) => {
            for item in items {
                walk(item, rules, dir);
            }
        }
        Value::Object(map) => {
            for (_k, v) in map.iter_mut() {
                walk(v, rules, dir);
            }
        }
        // Numbers, booleans, nulls — untouched.
        _ => {}
    }
}

/// Returns the rewritten string if any rule matches, else None.
fn try_replace(s: &str, rules: &[PathMapping], dir: Direction) -> Option<String> {
    for rule in rules {
        let (lhs, rhs) = match dir {
            Direction::CoordToWorker => (&rule.from, &rule.to),
            Direction::WorkerToCoord => (&rule.to, &rule.from),
        };
        if let Some(rest) = s.strip_prefix(lhs.as_str()) {
            // Boundary: the next char (or end-of-string) must be '/'
            // so `/mnt/movies` does NOT match `/mnt/movies-archive/...`.
            if rest.is_empty() || rest.starts_with('/') {
                return Some(format!("{rhs}{rest}"));
            }
        }
    }
    None
}

fn strip_trailing_slash(s: String) -> String {
    if s.len() > 1 && s.ends_with('/') {
        s.trim_end_matches('/').to_string()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rule(from: &str, to: &str) -> PathMapping {
        PathMapping { from: from.into(), to: to.into() }
    }

    #[test]
    fn empty_mappings_is_identity() {
        let m = PathMappings::default();
        assert!(m.is_empty());
        let mut v = json!({"file": {"path": "/mnt/movies/X.mkv"}});
        let snapshot = v.clone();
        m.apply(&mut v, Direction::CoordToWorker);
        assert_eq!(v, snapshot);
    }

    #[test]
    fn single_rule_rewrites_string_leaf() {
        let m = PathMappings::from_rules(vec![rule("/mnt/movies", "/data/media/movies")]);
        let mut v = json!({"file": {"path": "/mnt/movies/X.mkv"}});
        m.apply(&mut v, Direction::CoordToWorker);
        assert_eq!(v["file"]["path"], json!("/data/media/movies/X.mkv"));
    }

    #[test]
    fn longest_prefix_wins() {
        let m = PathMappings::from_rules(vec![
            rule("/mnt/movies", "/data/media/movies"),
            rule("/mnt/movies/4k", "/data/4k"),
        ]);
        let mut v = json!({"file": {"path": "/mnt/movies/4k/X.mkv"}});
        m.apply(&mut v, Direction::CoordToWorker);
        // The longer "/mnt/movies/4k" wins over "/mnt/movies".
        assert_eq!(v["file"]["path"], json!("/data/4k/X.mkv"));
    }

    #[test]
    fn path_component_boundary_respected() {
        // /mnt/movies must NOT rewrite /mnt/movies-archive/...
        let m = PathMappings::from_rules(vec![rule("/mnt/movies", "/data/media/movies")]);
        let mut v = json!({
            "a": "/mnt/movies/X.mkv",          // matches
            "b": "/mnt/movies-archive/Y.mkv",  // does NOT match (boundary)
            "c": "/mnt/movies",                // matches exactly (end-of-string)
        });
        m.apply(&mut v, Direction::CoordToWorker);
        assert_eq!(v["a"], json!("/data/media/movies/X.mkv"));
        assert_eq!(v["b"], json!("/mnt/movies-archive/Y.mkv"));
        assert_eq!(v["c"], json!("/data/media/movies"));
    }

    #[test]
    fn reverse_round_trip() {
        let m = PathMappings::from_rules(vec![
            rule("/mnt/movies", "/data/media/movies"),
            rule("/mnt/tv", "/data/media/tv"),
        ]);
        let original = json!({
            "file": {"path": "/mnt/movies/X.mkv", "size_bytes": 12345678},
            "steps": {
                "tx": {"output_path": "/mnt/tv/Y.transcoded.mkv"},
                "size_report": {"before_bytes": 9999, "msg": "ok"}
            }
        });
        let mut v = original.clone();
        m.apply(&mut v, Direction::CoordToWorker);
        assert_ne!(v, original, "forward must change something");
        m.apply(&mut v, Direction::WorkerToCoord);
        assert_eq!(v, original, "round-trip must restore the original");
    }

    #[test]
    fn walks_nested_objects_and_arrays() {
        let m = PathMappings::from_rules(vec![rule("/mnt", "/data")]);
        let mut v = json!({
            "file": {"path": "/mnt/X.mkv"},
            "steps": {
                "tx": {"output_path": "/mnt/X.transcoded.mkv"}
            },
            "extras": ["/mnt/A", "/other/B", {"nested": "/mnt/C"}]
        });
        m.apply(&mut v, Direction::CoordToWorker);
        assert_eq!(v["file"]["path"], json!("/data/X.mkv"));
        assert_eq!(v["steps"]["tx"]["output_path"], json!("/data/X.transcoded.mkv"));
        assert_eq!(v["extras"][0], json!("/data/A"));
        assert_eq!(v["extras"][1], json!("/other/B"), "non-matching prefix untouched");
        assert_eq!(v["extras"][2]["nested"], json!("/data/C"));
    }

    #[test]
    fn non_string_leaves_untouched() {
        let m = PathMappings::from_rules(vec![rule("/mnt/movies", "/data/media/movies")]);
        // Object keys that look like paths must NOT be rewritten — only
        // values. Numbers, bools, nulls untouched.
        let mut v = json!({
            "/mnt/movies": "leave-the-key-alone",
            "size": 12345,
            "ok": true,
            "missing": null
        });
        let snapshot = v.clone();
        m.apply(&mut v, Direction::CoordToWorker);
        assert_eq!(v, snapshot);
    }

    #[test]
    fn trailing_slash_normalisation() {
        // from = "/mnt/movies/" must behave identically to "/mnt/movies".
        let with_slash = PathMappings::from_rules(vec![rule("/mnt/movies/", "/data/media/movies/")]);
        let without_slash = PathMappings::from_rules(vec![rule("/mnt/movies", "/data/media/movies")]);

        let input = json!({"path": "/mnt/movies/X.mkv"});

        let mut a = input.clone();
        with_slash.apply(&mut a, Direction::CoordToWorker);

        let mut b = input.clone();
        without_slash.apply(&mut b, Direction::CoordToWorker);

        assert_eq!(a, b, "trailing slash must be normalised on construction");
        assert_eq!(a["path"], json!("/data/media/movies/X.mkv"));
    }

    #[test]
    fn from_json_round_trips() {
        let s = r#"[{"from":"/mnt/a","to":"/data/a"},{"from":"/mnt/b","to":"/data/b"}]"#;
        let m = PathMappings::from_json(s).unwrap();
        assert_eq!(m.rules().len(), 2);
        // Sorted by from.len() desc — both have the same length here.
        // Importantly: empty/whitespace input → identity, no error.
        assert!(PathMappings::from_json("").unwrap().is_empty());
        assert!(PathMappings::from_json("   ").unwrap().is_empty());
        // Malformed → Err.
        assert!(PathMappings::from_json("not json").is_err());
    }
}
