//! Worker-side mDNS browser. Used at boot when no `worker.toml`
//! exists: browses for `_transcoderr._tcp.local.` and returns the
//! first responder within a 5 s deadline.
//!
//! `mdns-sd` exposes a sync receiver; we drive it inside
//! `spawn_blocking` so we don't tie up the tokio runtime.

use anyhow::Context;
use mdns_sd::{IfKind, ServiceDaemon, ServiceEvent, ServiceInfo};
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
    /// Format the host:port part, bracketing IPv6 addresses so URLs are
    /// valid (RFC 2732). E.g. `[fe80::1]:8765` vs `192.168.1.50:8765`.
    fn host_port(&self) -> String {
        if self.addr.contains(':') {
            format!("[{}]:{}", self.addr, self.port)
        } else {
            format!("{}:{}", self.addr, self.port)
        }
    }

    pub fn http_url(&self) -> String {
        format!("http://{}", self.host_port())
    }
    pub fn ws_url(&self) -> String {
        format!("ws://{}{}", self.host_port(), self.ws_path)
    }
}

/// Browse for the first responder matching `_transcoderr._tcp.local.`.
/// Returns `Ok(None)` on timeout. `instance_filter`, when `Some`,
/// restricts results to instances whose fullname *contains* the given
/// substring — the integration test uses this to isolate concurrent runs.
///
/// `with_loopback`: when `true`, enables the loopback IPv4 interface on
/// the browse daemon. Used by the integration test where both sides run
/// on 127.0.0.1 (loopback is disabled in mdns-sd by default).
pub async fn browse(
    deadline: Duration,
    instance_filter: Option<String>,
    with_loopback: bool,
) -> anyhow::Result<Option<DiscoveredCoordinator>> {
    tokio::task::spawn_blocking(move || browse_blocking(deadline, instance_filter, with_loopback))
        .await
        .context("mDNS browse task join")?
}

fn browse_blocking(
    deadline: Duration,
    instance_filter: Option<String>,
    with_loopback: bool,
) -> anyhow::Result<Option<DiscoveredCoordinator>> {
    let mdns = ServiceDaemon::new().context("start mDNS daemon for browse")?;
    if with_loopback {
        mdns.enable_interface(IfKind::LoopbackV4)
            .context("enable loopback for browse")?;
    }
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

/// Validate a path advertised in an mDNS TXT record before it is
/// concatenated onto the coordinator's origin.
///
/// mDNS is unauthenticated — any host on the link can answer — and these
/// values are pasted straight into a URL (`http://host:port` + path). A
/// value that does not begin with a single `/` can move the request to a
/// different host entirely: `@evil.example/x` makes the real `host:port`
/// the userinfo component and `evil.example` the authority. Accept only a
/// plain absolute path so the origin the caller resolved is the origin it
/// actually talks to.
///
/// Returns `None` for anything suspicious; `browse_blocking` treats that
/// as a malformed responder and keeps looking.
fn sanitize_txt_path(raw: &str) -> Option<String> {
    // Must be an absolute path, and must not open with `//` (which a URL
    // parser reads as the start of an authority).
    if !raw.starts_with('/') || raw.starts_with("//") {
        return None;
    }
    // `@` would introduce userinfo, `?`/`#` a query or fragment, `\` is
    // treated as `/` by some parsers, and control/whitespace characters
    // have no business in a path.
    if raw
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || matches!(c, '@' | '?' | '#' | '\\'))
    {
        return None;
    }
    Some(raw.to_string())
}

/// Pure helper: pull the address, port, and TXT records out of a
/// `ServiceInfo`. Returns `None` if any required field is missing or if a
/// TXT-supplied path fails `sanitize_txt_path`.
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
    let enroll_path = sanitize_txt_path(props.get("enroll")?.val_str())?;
    let ws_path = sanitize_txt_path(props.get("ws")?.val_str())?;
    Some(DiscoveredCoordinator {
        addr,
        port,
        enroll_path,
        ws_path,
    })
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

    #[test]
    fn discovered_coordinator_url_helpers_ipv6() {
        let d = DiscoveredCoordinator {
            addr: "fe80::1".into(),
            port: 8765,
            enroll_path: "/api/worker/enroll".into(),
            ws_path: "/api/worker/connect".into(),
        };
        // IPv6 addresses must be bracketed in URLs (RFC 2732).
        assert_eq!(d.http_url(), "http://[fe80::1]:8765");
        assert_eq!(d.ws_url(), "ws://[fe80::1]:8765/api/worker/connect");
    }

    #[test]
    fn sanitize_accepts_ordinary_absolute_paths() {
        for p in [
            "/api/worker/enroll",
            "/api/worker/connect",
            "/enroll",
            "/a/b/c-d_e.f",
        ] {
            assert_eq!(sanitize_txt_path(p).as_deref(), Some(p));
        }
    }

    #[test]
    fn sanitize_rejects_authority_rewrites() {
        // The important case: anything that can move the request to a
        // host other than the one we resolved over mDNS.
        for p in [
            "@evil.example/x",       // real host:port becomes userinfo
            "//evil.example/x",      // parsed as a new authority
            "/x@evil.example",       // `@` anywhere is refused outright
            "http://evil.example/x", // absolute URL, not a path
            "evil.example/x",        // relative, would append to origin
        ] {
            assert!(
                sanitize_txt_path(p).is_none(),
                "{p} must be rejected as a TXT path"
            );
        }
    }

    #[test]
    fn sanitize_rejects_query_fragment_and_control_characters() {
        for p in [
            "/enroll?to=evil",
            "/enroll#frag",
            "/enroll\\x",
            "/enroll\nHost: evil",
            "/enroll with space",
            "/enroll\u{0}",
            "",
        ] {
            assert!(
                sanitize_txt_path(p).is_none(),
                "{p:?} must be rejected as a TXT path"
            );
        }
    }
}
