use std::net::{IpAddr, Ipv4Addr};

/// Normalizes a user-supplied host to `scheme://host[:port]`, adding `https`
/// when the scheme is missing and discarding any path, query, or fragment.
#[must_use]
pub fn normalize_host(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_owned()
    } else {
        format!("https://{trimmed}")
    };
    let parsed: url::Url = with_scheme.parse().ok()?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return None;
    }
    let mut normalized = parsed;
    let _ = normalized.set_username("");
    normalized.set_password(None).ok()?;
    normalized.set_path("/");
    normalized.set_query(None);
    normalized.set_fragment(None);
    Some(normalized.to_string().trim_end_matches('/').to_owned())
}

/// Names that typically resolve to local services or cloud metadata.
#[must_use]
pub fn is_forbidden_hostname(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name == "localhost"
        || name.ends_with(".localhost")
        || name.ends_with(".local")
        || name.ends_with(".internal")
        || name == "metadata"
        || name.starts_with("metadata.")
        || name == "instance-data"
        || name.starts_with("instance-data.")
}

/// False for loopback, private, link-local, CGNAT, documentation, benchmark,
/// and multicast ranges.
#[must_use]
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_ipv4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4() {
                return is_public_ipv4(v4);
            }
            let octets = v6.octets();
            !(v6.is_unspecified()
                || v6.is_loopback()
                || v6.is_multicast()
                || (octets[0] & 0xfe) == 0xfc
                || (octets[0] == 0xfe && octets[1] & 0xc0 == 0x80)
                || octets[..4] == [0x20, 0x01, 0x0d, 0xb8])
        }
    }
}

fn is_public_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !matches!(
        (a, b, c),
        (0, _, _)
            | (10, _, _)
            | (100, 64..=127, _)
            | (127, _, _)
            | (169, 254, _)
            | (172, 16..=31, _)
            | (192, 0, 0 | 2)
            | (192, 168, _)
            | (198, 18 | 19 | 51, _)
            | (203, 0, 113)
            | (224..=255, _, _)
    )
}

/// True when the host is a literal private/loopback address or a forbidden
/// name, so it must not be used as an outbound target. Accepts a bare host or a
/// full URL; unparseable URLs are blocked.
#[must_use]
pub fn is_blocked_host(host: &str) -> bool {
    let host = host.trim();
    let owned;
    let host = if host.contains("://") {
        let Ok(parsed) = url::Url::parse(host) else {
            return true;
        };
        let Some(parsed_host) = parsed.host_str() else {
            return true;
        };
        owned = parsed_host.to_owned();
        owned.as_str()
    } else {
        host
    };
    let bare = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    if is_forbidden_hostname(bare) {
        return true;
    }
    bare.parse::<IpAddr>().is_ok_and(|ip| !is_public_ip(ip))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_adds_scheme_and_strips_path() {
        assert_eq!(
            normalize_host("f.juicey.dev/some/path?x=1").as_deref(),
            Some("https://f.juicey.dev")
        );
        assert_eq!(
            normalize_host("http://127.0.0.1:6401/").as_deref(),
            Some("http://127.0.0.1:6401")
        );
        assert_eq!(normalize_host(""), None);
        assert_eq!(normalize_host("ftp://example.com"), None);
        assert_eq!(
            normalize_host("https://user:pass@f.juicey.dev").as_deref(),
            Some("https://f.juicey.dev")
        );
    }

    #[test]
    fn blocked_host_accepts_urls_and_bare_hosts() {
        assert!(is_blocked_host("http://127.0.0.1:6401"));
        assert!(is_blocked_host("http://[::1]:6401/"));
        assert!(is_blocked_host("http://localhost:6401"));
        assert!(is_blocked_host("http://[::ffff:192.168.1.10]"));
        assert!(is_blocked_host("10.0.0.1"));
        assert!(is_blocked_host("not a url://"));
        assert!(!is_blocked_host("https://box.juicey.dev"));
    }

    #[test]
    fn private_ranges_are_not_public() {
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "::1",
            "fc00::1",
            "fe80::1",
        ] {
            assert!(!is_public_ip(ip.parse().unwrap()), "{ip}");
        }
        assert!(is_public_ip("1.1.1.1".parse().unwrap()));
        assert!(is_public_ip("2606:4700::1111".parse().unwrap()));
    }
}
