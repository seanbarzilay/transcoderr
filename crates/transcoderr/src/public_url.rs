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
    let url = format!("http://{host}:{}", bound_addr.port());
    PublicUrl {
        url,
        source: Source::Default,
    }
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
}
