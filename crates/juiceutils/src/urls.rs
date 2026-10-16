#[must_use]
pub fn public_url(base_url: &str, storage_host: Option<&str>, id: &str, filename: &str) -> String {
    public_url_for_route("v", base_url, storage_host, id, filename)
}

#[must_use]
pub fn public_url_for_route(
    route: &str,
    base_url: &str,
    storage_host: Option<&str>,
    id: &str,
    filename: &str,
) -> String {
    let host = match storage_host {
        Some(host) if !host.is_empty() => host,
        _ => base_url,
    };
    let host = if host.contains("://") {
        host.to_string()
    } else {
        format!("https://{host}")
    };
    match filename.rsplit('.').next() {
        Some(ext) if !ext.is_empty() && ext != filename => format!("{host}/{route}/{id}.{ext}"),
        _ => format!("{host}/{route}/{id}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_storage_host() {
        let url = public_url(
            "http://localhost:6402",
            Some("https://files.example.com"),
            "abc123",
            "cute.gif",
        );
        assert_eq!(url, "https://files.example.com/v/abc123.gif");
    }

    #[test]
    fn without_storage_host() {
        let url = public_url("http://localhost:6402", None, "abc123", "cute.gif");
        assert_eq!(url, "http://localhost:6402/v/abc123.gif");
    }

    #[test]
    fn empty_storage_host() {
        let url = public_url("http://localhost:6402", Some(""), "abc123", "cute.gif");
        assert_eq!(url, "http://localhost:6402/v/abc123.gif");
    }

    #[test]
    fn bare_storage_host() {
        let url = public_url(
            "https://f.juicey.dev",
            Some("fx.juicey.dev"),
            "abc123",
            "cute.gif",
        );
        assert_eq!(url, "https://fx.juicey.dev/v/abc123.gif");
    }

    #[test]
    fn filename_without_extension() {
        let url = public_url("http://localhost:6402", None, "abc123", "README");
        assert_eq!(url, "http://localhost:6402/v/abc123");
    }

    #[test]
    fn route_variants_share_host_and_extension() {
        for (route, expected) in [
            ("v", "http://localhost:6402/v/abc123.gif"),
            ("f", "http://localhost:6402/f/abc123.gif"),
            ("d", "http://localhost:6402/d/abc123.gif"),
        ] {
            assert_eq!(
                public_url_for_route(route, "http://localhost:6402", None, "abc123", "cute.gif"),
                expected
            );
        }
    }
}
