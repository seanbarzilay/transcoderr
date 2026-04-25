use std::time::Duration;
use transcoderr::http::dedup::DedupCache;

#[test]
fn duplicate_within_window_rejected() {
    let c = DedupCache::new(Duration::from_secs(60));
    assert!(c.observe(1, "/m/x", r#"{"a":1}"#));
    assert!(!c.observe(1, "/m/x", r#"{"a":1}"#));
    assert!(c.observe(1, "/m/x", r#"{"a":2}"#)); // payload differs → new
    assert!(c.observe(2, "/m/x", r#"{"a":1}"#)); // different source → new
}
