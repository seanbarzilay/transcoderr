//! Coordinator-side mDNS responder. Advertises
//! `_transcoderr._tcp.local.` so workers on the same LAN can find
//! us without operator-supplied config.
//!
//! TXT records: `enroll` (path), `ws` (path), `version` (informational).
//! Workers read `enroll` and `ws` directly; the version field is a
//! debugging aid for future protocol changes.

use anyhow::Context;
use mdns_sd::{ServiceDaemon, ServiceInfo};

/// Service type advertised by the coordinator and queried by workers.
pub const SERVICE_TYPE: &str = "_transcoderr._tcp.local.";

/// Build the `ServiceInfo` for our advertisement. Public so unit tests
/// can inspect it without actually starting a daemon.
pub fn build_service_info(
    port: u16,
    instance_name: &str,
) -> anyhow::Result<ServiceInfo> {
    let host_name = format!("{}.local.", instance_name);
    let txt: Vec<(&str, &str)> = vec![
        ("enroll", "/api/worker/enroll"),
        ("ws", "/api/worker/connect"),
        ("version", env!("CARGO_PKG_VERSION")),
    ];
    // Empty `host_ipv4` means mdns-sd will auto-detect interfaces and
    // publish on all of them. That's what we want for a multi-homed host.
    let info = ServiceInfo::new(
        SERVICE_TYPE,
        instance_name,
        &host_name,
        "",
        port,
        &txt[..],
    )
    .context("build mDNS ServiceInfo")?
    .enable_addr_auto();
    Ok(info)
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
