//! Resolve the URL that *arr instances should use to reach this
//! transcoderr server. Set once at boot, stored in AppState, baked
//! into webhook configurations on source-create.

use std::net::SocketAddr;

#[derive(Debug, Clone, Copy)]
pub enum Source {
    Env,
    Default,
}

#[derive(Debug, Clone)]
pub struct PublicUrl {
    pub url: String,
    pub source: Source,
}

/// Resolve from `TRANSCODERR_PUBLIC_URL` if set, else
/// `http://{gethostname()}:{addr.port()}`. Falls back to `localhost`
/// if the gethostname() syscall fails (extremely rare).
pub fn resolve(bound_addr: SocketAddr) -> PublicUrl {
    if let Ok(url) = std::env::var("TRANSCODERR_PUBLIC_URL") {
        let url = url.trim_end_matches('/').to_string();
        return PublicUrl {
            url,
            source: Source::Env,
        };
    }
    let host = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "localhost".to_string());
    if looks_like_container_id(&host) {
        tracing::warn!(
            hostname = %host,
            "resolved hostname looks like a docker container id; \
             this URL is not reachable from outside the container's network. \
             Set TRANSCODERR_PUBLIC_URL=http://<service-or-host>:<port> for *arr auto-provisioning."
        );
    }
    let url = format!("http://{host}:{}", bound_addr.port());
    PublicUrl {
        url,
        source: Source::Default,
    }
}

/// Heuristic for "this hostname is probably a docker container id". Docker
/// uses the 12-char short id as the in-container hostname by default. We
/// also catch the rarer 64-char full id.
fn looks_like_container_id(host: &str) -> bool {
    let len = host.len();
    (len == 12 || len == 64) && host.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::net::SocketAddr;

    fn addr() -> SocketAddr {
        "127.0.0.1:8099".parse().unwrap()
    }

    #[test]
    #[serial]
    fn resolve_uses_env_var_when_set() {
        std::env::set_var("TRANSCODERR_PUBLIC_URL", "https://t.example.com/");
        let p = resolve(addr());
        std::env::remove_var("TRANSCODERR_PUBLIC_URL");
        assert_eq!(p.url, "https://t.example.com");
        assert!(matches!(p.source, Source::Env));
    }

    #[test]
    #[serial]
    fn resolve_defaults_to_hostname_and_bound_port() {
        std::env::remove_var("TRANSCODERR_PUBLIC_URL");
        let p = resolve(addr());
        assert!(matches!(p.source, Source::Default));
        // The actual hostname depends on the test host — assert the
        // shape rather than the literal. URL must start with http://
        // and end with the bound port.
        assert!(p.url.starts_with("http://"), "got {}", p.url);
        assert!(p.url.ends_with(":8099"), "got {}", p.url);
    }

    #[test]
    fn container_id_heuristic() {
        // Short docker container ids: 12 lowercase hex chars.
        assert!(looks_like_container_id("c2a507c37ae6"));
        assert!(looks_like_container_id("0123456789ab"));
        // Full container id: 64 lowercase hex chars.
        let full = "0123456789abcdef".repeat(4);
        assert!(looks_like_container_id(&full));
        // Real hostnames are not flagged.
        assert!(!looks_like_container_id("transcoderr"));
        assert!(!looks_like_container_id("localhost"));
        assert!(!looks_like_container_id("media-server-01"));
        // Wrong length, even if hex-only.
        assert!(!looks_like_container_id("abc"));
        // Right length but uppercase: not a docker id (docker uses lowercase).
        assert!(!looks_like_container_id("C2A507C37AE6"));
    }
}
