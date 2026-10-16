//! Endpoint URL policy. Decisions are made on the PARSED URL, never on string
//! prefixes (`http://127.0.0.1.evil.com/` and `http://127.0.0.1:1@evil.com/`
//! both start with "http://127.0.0.1" but are not loopback).

use url::{Host, Url};

/// True only for http(s) URLs whose host is exactly 127.0.0.1, ::1 or
/// `localhost`, with no userinfo.
pub fn is_loopback_url(u: &Url) -> bool {
    if !matches!(u.scheme(), "http" | "https") || !u.username().is_empty() || u.password().is_some() {
        return false;
    }
    match u.host() {
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        Some(Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

/// Cleartext http is only acceptable for loopback (the local mock); anything
/// else must be https. Used by the token exchange as defense in depth.
pub fn require_secure_or_loopback(raw: &str) -> Result<Url, String> {
    let u = Url::parse(raw).map_err(|e| format!("invalid URL: {e}"))?;
    if !u.username().is_empty() || u.password().is_some() {
        return Err("URL must not contain userinfo".into());
    }
    match u.scheme() {
        "https" if u.host().is_some() => Ok(u),
        "http" if is_loopback_url(&u) => Ok(u),
        _ => Err(format!("refusing non-https, non-loopback endpoint (scheme={})", u.scheme())),
    }
}

/// `--live` policy: BOTH endpoints must be https with a host and no userinfo.
/// Plain http is refused even for loopback (live mode targets real providers).
pub fn validate_live_endpoint(raw: &str) -> Result<Url, String> {
    let u = Url::parse(raw).map_err(|e| format!("invalid URL: {e}"))?;
    if u.scheme() != "https" {
        return Err(format!("live endpoints must use https (got scheme '{}')", u.scheme()));
    }
    if u.host().is_none() {
        return Err("live endpoint has no host".into());
    }
    if !u.username().is_empty() || u.password().is_some() {
        return Err("live endpoint must not contain userinfo".into());
    }
    Ok(u)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lb(s: &str) -> bool {
        is_loopback_url(&Url::parse(s).unwrap())
    }

    #[test]
    fn real_loopback_hosts_are_loopback() {
        assert!(lb("http://127.0.0.1:8080/token"));
        assert!(lb("http://localhost:1/x"));
        assert!(lb("http://[::1]:9/x"));
    }

    #[test]
    fn prefix_lookalikes_are_not_loopback() {
        assert!(!lb("http://127.0.0.1.evil.com/token"));
        assert!(!lb("http://127.0.0.1:1@evil.com/token"));
        assert!(!lb("http://localhost.evil.example/token"));
        assert!(!lb("http://evil.com/?http://127.0.0.1"));
        assert!(!lb("http://user@127.0.0.1/token"), "userinfo disqualifies");
        assert!(!lb("ftp://127.0.0.1/x"));
    }

    #[test]
    fn live_mode_rejects_non_https_and_lookalikes() {
        for bad in [
            "http://127.0.0.1.evil.com/token",
            "http://127.0.0.1:1@evil.com/token",
            "http://localhost.evil.example/token",
            "http://example.com/token",
            "http://127.0.0.1:8080/token",
            "ftp://example.com/token",
            "https://user:pw@example.com/token",
            "not a url",
        ] {
            assert!(validate_live_endpoint(bad).is_err(), "should reject {bad}");
        }
    }

    #[test]
    fn live_mode_accepts_https() {
        assert!(validate_live_endpoint("https://auth.example.com/oauth/token").is_ok());
        assert!(validate_live_endpoint("https://auth.example.com:8443/authorize?x=1").is_ok());
    }

    #[test]
    fn alternative_loopback_spellings_normalize_to_canonical_form() {
        // What the HTTP client is given must be the normalized URL the policy judged.
        for raw in [
            "http://127.1:5/token",
            "http://0x7f.1:5/token",
            "http://2130706433:5/token",
            "http://127.0.0.1.:5/token",
            "http://0177.0.0.1:5/token",
        ] {
            let u = require_secure_or_loopback(raw).unwrap_or_else(|e| panic!("{raw}: {e}"));
            assert!(is_loopback_url(&u), "{raw}");
            assert_eq!(u.as_str(), "http://127.0.0.1:5/token", "{raw} not normalized");
        }
    }

    #[test]
    fn cleartext_allowed_only_for_real_loopback() {
        assert!(require_secure_or_loopback("http://127.0.0.1:5/token").is_ok());
        assert!(require_secure_or_loopback("https://auth.example.com/token").is_ok());
        assert!(require_secure_or_loopback("http://127.0.0.1.evil.com/token").is_err());
        assert!(require_secure_or_loopback("http://127.0.0.1:1@evil.com/token").is_err());
        assert!(require_secure_or_loopback("http://example.com/token").is_err());
    }
}
