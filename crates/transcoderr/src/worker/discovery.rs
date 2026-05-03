//! Worker-side mDNS browser. Used at boot when no `worker.toml`
//! exists: browses for `_transcoderr._tcp.local.` and returns the
//! first responder within a 5 s deadline.
//!
//! `mdns-sd` exposes a sync receiver; we drive it inside
//! `spawn_blocking` so we don't tie up the tokio runtime.

use anyhow::Context;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::time::Duration;

/// What a successful browse returns: enough to POST `/api/worker/enroll`.
#[derive(Debug, Clone)]
pub struct DiscoveredCoordinator {
    /// First IPv4 address advertised by the responder. We pick IPv4
    /// for now; if the only address is IPv6, we'll fall back to that
    /// in the same field (stored as a string).
    pub addr: String,
    pub port: u16,
    pub enroll_path: String,
    pub ws_path: String,
}

impl DiscoveredCoordinator {
    pub fn http_url(&self) -> String {
        format!("http://{}:{}", self.addr, self.port)
    }
    pub fn ws_url(&self) -> String {
        format!("ws://{}:{}{}", self.addr, self.port, self.ws_path)
    }
}

/// Browse for the first responder matching `_transcoderr._tcp.local.`.
/// Returns `Ok(None)` on timeout. `instance_filter`, when `Some`,
/// restricts results to instances whose fullname *contains* the given
/// substring — the integration test uses this to isolate concurrent runs.
pub async fn browse(
    deadline: Duration,
    instance_filter: Option<String>,
) -> anyhow::Result<Option<DiscoveredCoordinator>> {
    tokio::task::spawn_blocking(move || browse_blocking(deadline, instance_filter))
        .await
        .context("mDNS browse task join")?
}

fn browse_blocking(
    deadline: Duration,
    instance_filter: Option<String>,
) -> anyhow::Result<Option<DiscoveredCoordinator>> {
    let mdns = ServiceDaemon::new().context("start mDNS daemon for browse")?;
    let receiver = mdns
        .browse(crate::discovery::SERVICE_TYPE)
        .context("start mDNS browse")?;

    let start = std::time::Instant::now();
    while start.elapsed() < deadline {
        let remaining = deadline.saturating_sub(start.elapsed());
        match receiver.recv_timeout(remaining) {
            Ok(ServiceEvent::ServiceResolved(info)) => {
                if let Some(filter) = &instance_filter {
                    if !info.get_fullname().contains(filter) {
                        tracing::debug!(
                            fullname = info.get_fullname(),
                            filter = filter,
                            "skipping responder (instance filter mismatch)"
                        );
                        continue;
                    }
                }
                if let Some(parsed) = parse_service_info(&info) {
                    let _ = mdns.shutdown();
                    return Ok(Some(parsed));
                }
                tracing::warn!(
                    fullname = info.get_fullname(),
                    "found responder but TXT records were missing or malformed; ignoring"
                );
            }
            Ok(_other_event) => continue,
            Err(_timeout) => break,
        }
    }
    let _ = mdns.shutdown();
    Ok(None)
}

/// Pure helper: pull the address, port, and TXT records out of a
/// `ServiceInfo`. Returns `None` if any required field is missing.
/// Kept private but unit-testable.
fn parse_service_info(info: &ServiceInfo) -> Option<DiscoveredCoordinator> {
    let addrs = info.get_addresses();
    let addr = addrs
        .iter()
        .find(|a| a.is_ipv4())
        .or_else(|| addrs.iter().next())?
        .to_string();
    let port = info.get_port();
    let props = info.get_properties();
    // val_str() returns &str directly in mdns-sd 0.13 (not Option<&str>),
    // matching the pattern from Task 1's coordinator-side helper.
    let enroll_path = props.get("enroll")?.val_str().to_string();
    let ws_path = props.get("ws")?.val_str().to_string();
    Some(DiscoveredCoordinator { addr, port, enroll_path, ws_path })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_returns_none_when_addresses_unresolved() {
        // `build_service_info` uses `enable_addr_auto()` so addresses
        // are populated lazily by a running daemon. Without one, the
        // address set is empty and `parse_service_info` returns None
        // — that's the safe behavior we want at the boundary. The
        // populated case is covered by tests/auto_discovery.rs.
        let info = crate::discovery::build_service_info(8765, "test-instance").unwrap();
        assert!(parse_service_info(&info).is_none());
    }

    #[test]
    fn discovered_coordinator_url_helpers() {
        let d = DiscoveredCoordinator {
            addr: "192.168.1.50".into(),
            port: 8765,
            enroll_path: "/api/worker/enroll".into(),
            ws_path: "/api/worker/connect".into(),
        };
        assert_eq!(d.http_url(), "http://192.168.1.50:8765");
        assert_eq!(d.ws_url(), "ws://192.168.1.50:8765/api/worker/connect");
    }
}
