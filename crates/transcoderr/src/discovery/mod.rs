//! Coordinator-side mDNS responder. Advertises
//! `_transcoderr._tcp.local.` so workers on the same LAN can find
//! us without operator-supplied config.
//!
//! TXT records: `enroll` (path), `ws` (path), `version` (informational).
//! Workers read `enroll` and `ws` directly; the version field is a
//! debugging aid for future protocol changes.

use anyhow::Context;
use mdns_sd::{IfKind, ServiceDaemon, ServiceInfo};

/// Service type advertised by the coordinator and queried by workers.
pub const SERVICE_TYPE: &str = "_transcoderr._tcp.local.";

/// Build the `ServiceInfo` for our advertisement. Public so unit tests
/// can inspect it without actually starting a daemon.
pub fn build_service_info(
    port: u16,
    instance_name: &str,
) -> anyhow::Result<ServiceInfo> {
    // Empty `host_ipv4` means mdns-sd will auto-detect interfaces and
    // publish on all of them. That's what we want for a multi-homed host.
    build_service_info_with_host(port, instance_name, "")
}

/// Like [`build_service_info`] but pins the advertised host address.
/// Pass `"127.0.0.1"` to ensure the responder is reachable over loopback
/// (used by integration tests where the coordinator is bound to 127.0.0.1).
pub fn build_service_info_with_host(
    port: u16,
    instance_name: &str,
    host_ipv4: &str,
) -> anyhow::Result<ServiceInfo> {
    let host_name = format!("{}.local.", instance_name);
    let txt: Vec<(&str, &str)> = vec![
        ("enroll", "/api/worker/enroll"),
        ("ws", "/api/worker/connect"),
        ("version", env!("CARGO_PKG_VERSION")),
    ];
    let mut info = ServiceInfo::new(
        SERVICE_TYPE,
        instance_name,
        &host_name,
        host_ipv4,
        port,
        &txt[..],
    )
    .context("build mDNS ServiceInfo")?;
    if host_ipv4.is_empty() {
        info = info.enable_addr_auto();
    }
    Ok(info)
}

/// Start the responder pinned to `127.0.0.1`. Used by integration tests
/// where the coordinator HTTP server is bound to loopback only.
///
/// Loopback is disabled in mdns-sd by default; this helper explicitly
/// enables it on both the responder and the browse daemon.
pub fn start_responder_on_loopback(
    port: u16,
    instance_name: &str,
) -> anyhow::Result<ServiceDaemon> {
    let mdns = ServiceDaemon::new().context("start mDNS daemon")?;
    // Loopback is disabled by default; enable it so the mDNS daemon
    // sends/receives on 127.0.0.1.
    mdns.enable_interface(IfKind::LoopbackV4)
        .context("enable loopback interface")?;
    let info = build_service_info_with_host(port, instance_name, "127.0.0.1")?;
    mdns.register(info).context("register mDNS service")?;
    tracing::info!(
        port,
        instance_name,
        service = SERVICE_TYPE,
        "mDNS responder started (loopback)"
    );
    Ok(mdns)
}

/// Start the responder. Returned `ServiceDaemon` holds the registration
/// for its lifetime; drop it (or call `shutdown`) to unregister.
pub fn start_responder(
    port: u16,
    instance_name: &str,
) -> anyhow::Result<ServiceDaemon> {
    let mdns = ServiceDaemon::new().context("start mDNS daemon")?;
    let info = build_service_info(port, instance_name)?;
    mdns.register(info).context("register mDNS service")?;
    tracing::info!(
        port,
        instance_name,
        service = SERVICE_TYPE,
        "mDNS responder started"
    );
    Ok(mdns)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_info_has_expected_shape() {
        let info = build_service_info(8765, "transcoderr-test").unwrap();
        assert_eq!(info.get_type(), SERVICE_TYPE);
        assert_eq!(info.get_port(), 8765);
        let props = info.get_properties();
        assert_eq!(
            props.get("enroll").map(|p| p.val_str()),
            Some("/api/worker/enroll")
        );
        assert_eq!(
            props.get("ws").map(|p| p.val_str()),
            Some("/api/worker/connect")
        );
        assert!(
            props.get("version").is_some(),
            "version TXT record should be present (informational)"
        );
    }

    #[test]
    fn instance_name_is_used_in_fullname() {
        let info = build_service_info(8765, "fluffy-coord").unwrap();
        let fullname = info.get_fullname();
        assert!(
            fullname.starts_with("fluffy-coord."),
            "fullname should start with the instance name; got {fullname}"
        );
    }
}
