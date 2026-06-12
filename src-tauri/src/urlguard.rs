//! SSRF guard for outbound media downloads.
//!
//! Caller-supplied URLs (REST `/download`, `/download/batch`, `/combo/...`,
//! and video `urls["poster"]`) are fetched by the downloader. Without a guard
//! an attacker can point those at internal hosts or cloud-metadata endpoints
//! (169.254.169.254) and use the app as a proxy. This module enforces an
//! `http`/`https` scheme allowlist and rejects hosts that resolve to
//! loopback / private / link-local / unspecified / multicast addresses.
//!
//! Note: a residual TOCTOU window exists because reqwest re-resolves the host
//! when it connects. Each redirect hop is re-validated by the downloader, but
//! a fully airtight fix would require a custom connector pinned to the
//! validated IP. This guard blocks the realistic drive-by cases.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// True if connecting to `ip` should be refused (not a public address).
pub fn ip_is_blocked(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()        // 127.0.0.0/8
                || v4.is_private()  // 10/8, 172.16/12, 192.168/16
                || v4.is_link_local() // 169.254.0.0/16 (incl. 169.254.169.254 metadata)
                || v4.is_unspecified() // 0.0.0.0
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.octets()[0] == 0 // 0.0.0.0/8 "this network"
                || is_shared_v4(v4) // 100.64.0.0/10 CGNAT
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || is_ula_v6(v6) // fc00::/7 unique-local
                || is_link_local_v6(v6) // fe80::/10
                // IPv4-mapped (::ffff:a.b.c.d) AND deprecated IPv4-compatible
                // (::a.b.c.d) — `to_ipv4` covers both — apply the v4 rules.
                || v6
                    .to_ipv4()
                    .map(|m| ip_is_blocked(&IpAddr::V4(m)))
                    .unwrap_or(false)
                // NAT64 (64:ff9b::/96) embeds a v4 target in the low 32 bits.
                || nat64_embedded_v4(v6)
                    .map(|m| ip_is_blocked(&IpAddr::V4(m)))
                    .unwrap_or(false)
        }
    }
}

fn is_shared_v4(ip: &Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 100 && (o[1] & 0xc0) == 0x40
}

fn is_ula_v6(ip: &Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xfe00) == 0xfc00
}

fn is_link_local_v6(ip: &Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xffc0) == 0xfe80
}

/// If `ip` is a NAT64 address (64:ff9b::/96), return the embedded IPv4.
fn nat64_embedded_v4(ip: &Ipv6Addr) -> Option<Ipv4Addr> {
    let s = ip.segments();
    if s[0] == 0x0064 && s[1] == 0xff9b {
        let o = ip.octets();
        Some(Ipv4Addr::new(o[12], o[13], o[14], o[15]))
    } else {
        None
    }
}

/// Validate scheme + any IP-literal host synchronously.
/// Returns `Ok(Some(host))` when the host is a name that still needs DNS
/// resolution, or `Ok(None)` when it was an IP literal that already passed.
fn validate_scheme_host(url: &reqwest::Url) -> Result<Option<String>, String> {
    match url.scheme() {
        "http" | "https" => {}
        other => return Err(format!("blocked URL scheme: {other}")),
    }
    let host = url
        .host_str()
        .ok_or_else(|| "URL has no host".to_string())?;
    if let Ok(ip) = host.parse::<IpAddr>() {
        if ip_is_blocked(&ip) {
            return Err("blocked address (loopback/private/link-local)".to_string());
        }
        return Ok(None);
    }
    Ok(Some(host.to_string()))
}

/// Full async validation: scheme allowlist + resolve the host and ensure every
/// resolved address is a routable public address.
pub async fn validate_outbound_url(url: &reqwest::Url) -> Result<(), String> {
    let Some(host) = validate_scheme_host(url)? else {
        return Ok(());
    };
    let port = url.port_or_known_default().unwrap_or(0);
    let mut resolved = false;
    let addrs = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|e| format!("DNS resolution failed: {e}"))?;
    for sa in addrs {
        resolved = true;
        if ip_is_blocked(&sa.ip()) {
            return Err("host resolves to a blocked address".to_string());
        }
    }
    if !resolved {
        return Err("host did not resolve to any address".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocked(s: &str) -> bool {
        ip_is_blocked(&s.parse::<IpAddr>().unwrap())
    }

    #[test]
    fn blocks_private_and_meta() {
        assert!(blocked("127.0.0.1"));
        assert!(blocked("10.0.0.5"));
        assert!(blocked("192.168.1.1"));
        assert!(blocked("172.16.0.1"));
        assert!(blocked("169.254.169.254")); // cloud metadata
        assert!(blocked("0.0.0.0"));
        assert!(blocked("::1"));
        assert!(blocked("fc00::1"));
        assert!(blocked("fe80::1"));
        assert!(blocked("::ffff:127.0.0.1")); // v4-mapped loopback
        assert!(blocked("::ffff:169.254.169.254")); // v4-mapped metadata
        assert!(blocked("64:ff9b::10.0.0.1")); // NAT64-embedded private v4
    }

    #[test]
    fn allows_public() {
        assert!(!blocked("1.1.1.1"));
        assert!(!blocked("8.8.8.8"));
        assert!(!blocked("2606:4700:4700::1111"));
    }

    #[test]
    fn scheme_allowlist() {
        assert!(validate_scheme_host(&reqwest::Url::parse("file:///etc/passwd").unwrap()).is_err());
        assert!(
            validate_scheme_host(&reqwest::Url::parse("ftp://example.com/x").unwrap()).is_err()
        );
        assert!(validate_scheme_host(&reqwest::Url::parse("http://127.0.0.1/x").unwrap()).is_err());
        assert!(
            validate_scheme_host(&reqwest::Url::parse("https://example.com/x").unwrap()).is_ok()
        );
    }
}
