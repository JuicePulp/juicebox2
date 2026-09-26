#[must_use]
pub fn public_url(base_url: &str, storage_host: Option<&str>, id: &str, filename: &str) -> String {
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
        Some(ext) if !ext.is_empty() && ext != filename => format!("{host}/f/{id}.{ext}"),
        _ => format!("{host}/f/{id}"),
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
        assert_eq!(url, "https://files.example.com/f/abc123.gif");
    }

    #[test]
    fn without_storage_host() {
        let url = public_url("http://localhost:6402", None, "abc123", "cute.gif");
        assert_eq!(url, "http://localhost:6402/f/abc123.gif");
    }

    #[test]
    fn empty_storage_host() {
        let url = public_url("http://localhost:6402", Some(""), "abc123", "cute.gif");
        assert_eq!(url, "http://localhost:6402/f/abc123.gif");
    }

    #[test]
    fn bare_storage_host() {
        let url = public_url(
            "https://f.juicey.dev",
            Some("fx.juicey.dev"),
            "abc123",
            "cute.gif",
        );
        assert_eq!(url, "https://fx.juicey.dev/f/abc123.gif");
    }

    #[test]
    fn filename_without_extension() {
        let url = public_url("http://localhost:6402", None, "abc123", "README");
        assert_eq!(url, "http://localhost:6402/f/abc123");
    }
}
