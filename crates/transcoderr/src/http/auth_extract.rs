use axum::http::HeaderMap;
use base64::Engine;

/// Extract a webhook auth token from request headers.
///
/// Supports two header formats:
/// - `Authorization: Bearer <token>` — for clients that can set arbitrary headers
///   (curl, custom integrations).
/// - `Authorization: Basic <base64(user:pass)>` — for vendors whose webhook UI only
///   exposes Basic auth (Radarr, Sonarr, Lidarr). The password portion is used as
///   the token; the username is ignored, so the user can put anything there.
///
/// Returns an empty string if no recognized auth header is present, which makes the
/// caller's "no source matches an empty token" lookup naturally produce a 401.
pub fn extract_token(headers: &HeaderMap) -> String {
    let Some(auth) = headers.get("authorization").and_then(|v| v.to_str().ok()) else {
        return String::new();
    };
    if let Some(t) = auth.strip_prefix("Bearer ") {
        return t.trim().to_string();
    }
    if let Some(b64) = auth.strip_prefix("Basic ") {
        if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(b64.trim()) {
            if let Ok(s) = String::from_utf8(decoded) {
                if let Some((_user, pass)) = s.split_once(':') {
                    return pass.to_string();
                }
            }
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;
    use base64::Engine;

    fn hm(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("authorization", value.parse().unwrap());
        h
    }

    #[test]
    fn empty_when_missing() {
        assert_eq!(extract_token(&HeaderMap::new()), "");
    }

    #[test]
    fn parses_bearer() {
        assert_eq!(extract_token(&hm("Bearer my-token")), "my-token");
    }

    #[test]
    fn parses_basic_returns_password() {
        let cred = base64::engine::general_purpose::STANDARD.encode(b"someuser:my-token");
        assert_eq!(extract_token(&hm(&format!("Basic {cred}"))), "my-token");
    }

    #[test]
    fn empty_username_in_basic_works() {
        let cred = base64::engine::general_purpose::STANDARD.encode(b":my-token");
        assert_eq!(extract_token(&hm(&format!("Basic {cred}"))), "my-token");
    }

    #[test]
    fn malformed_basic_returns_empty() {
        assert_eq!(extract_token(&hm("Basic not-base64!")), "");
    }
}
