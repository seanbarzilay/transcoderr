use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct DedupCache {
    inner: Mutex<HashMap<String, Instant>>,
    window: Duration,
}

impl DedupCache {
    pub fn new(window: Duration) -> Self {
        Self { inner: Mutex::new(HashMap::new()), window }
    }

    /// Returns true if NEW (not a recent duplicate).
    pub fn observe(&self, source_id: i64, path: &str, raw_payload: &str) -> bool {
        let key = format!("{source_id}|{path}|{}", short_hash(raw_payload));
        let now = Instant::now();
        let mut g = self.inner.lock().unwrap();
        g.retain(|_, t| now.duration_since(*t) < self.window);
        match g.entry(key) {
            std::collections::hash_map::Entry::Occupied(_) => false,
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert(now);
                true
            }
        }
    }
}

fn short_hash(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}
