//! In-memory TTL cache for trimmed *arr browse responses. Stored on
//! `AppState` as `Arc<ArrCache>`; cache holds the full library so that
//! search/sort/pagination on hits are sub-millisecond.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct ArrCache {
    inner: Arc<RwLock<HashMap<(i64, String), CacheEntry>>>,
    ttl: Duration,
    now_fn: Arc<dyn Fn() -> Instant + Send + Sync>,
}

struct CacheEntry {
    data: serde_json::Value,
    expires_at: Instant,
}

impl ArrCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            ttl,
            now_fn: Arc::new(Instant::now),
        }
    }

    /// Test-only constructor with an injectable clock.
    #[cfg(test)]
    pub fn new_with_clock(ttl: Duration, now_fn: Arc<dyn Fn() -> Instant + Send + Sync>) -> Self {
        Self { inner: Arc::new(RwLock::new(HashMap::new())), ttl, now_fn }
    }

    pub fn get(&self, source_id: i64, key: &str) -> Option<serde_json::Value> {
        let now = (self.now_fn)();
        let g = self.inner.read().ok()?;
        let e = g.get(&(source_id, key.to_string()))?;
        if e.expires_at < now { return None; }
        Some(e.data.clone())
    }

    pub fn put(&self, source_id: i64, key: &str, data: serde_json::Value) {
        let expires_at = (self.now_fn)() + self.ttl;
        let Ok(mut g) = self.inner.write() else { return };
        g.insert((source_id, key.to_string()), CacheEntry { data, expires_at });
    }

    /// Drops every entry whose source_id matches.
    pub fn invalidate(&self, source_id: i64) {
        let Ok(mut g) = self.inner.write() else { return };
        g.retain(|(sid, _), _| *sid != source_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn fake_clock(start: Instant) -> (Arc<dyn Fn() -> Instant + Send + Sync>, Arc<Mutex<Instant>>) {
        let now = Arc::new(Mutex::new(start));
        let now_fn_handle = now.clone();
        let now_fn: Arc<dyn Fn() -> Instant + Send + Sync> =
            Arc::new(move || *now_fn_handle.lock().unwrap());
        (now_fn, now)
    }

    #[test]
    fn cache_returns_value_within_ttl() {
        let (clock, now_handle) = fake_clock(Instant::now());
        let c = ArrCache::new_with_clock(Duration::from_secs(300), clock);
        c.put(1, "movies", serde_json::json!([{"id": 42}]));
        // Advance clock by 4 minutes — still within 5-minute TTL.
        *now_handle.lock().unwrap() += Duration::from_secs(240);
        let got = c.get(1, "movies").unwrap();
        assert_eq!(got, serde_json::json!([{"id": 42}]));
    }

    #[test]
    fn cache_returns_none_after_ttl_expiry() {
        let (clock, now_handle) = fake_clock(Instant::now());
        let c = ArrCache::new_with_clock(Duration::from_secs(300), clock);
        c.put(1, "movies", serde_json::json!([{"id": 42}]));
        // Advance past the 5-minute TTL.
        *now_handle.lock().unwrap() += Duration::from_secs(301);
        assert!(c.get(1, "movies").is_none());
    }

    #[test]
    fn invalidate_drops_all_keys_for_source_id() {
        let (clock, _) = fake_clock(Instant::now());
        let c = ArrCache::new_with_clock(Duration::from_secs(300), clock);
        c.put(1, "movies", serde_json::json!([1]));
        c.put(1, "series", serde_json::json!([2]));
        c.put(2, "movies", serde_json::json!([3]));
        c.invalidate(1);
        assert!(c.get(1, "movies").is_none());
        assert!(c.get(1, "series").is_none());
        assert!(c.get(2, "movies").is_some()); // unaffected
    }
}
