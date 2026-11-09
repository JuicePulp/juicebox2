pub mod ban;
pub mod config;
pub mod file_validation;
pub mod host;
pub mod icons;
pub mod ids;
pub mod ip_crypt;
pub mod proxy;
pub mod text;
pub mod urls;
pub mod web;

const ISSUES_URL: &str = "https://github.com/juiceboxdev/juicebox2/issues";

pub fn install_panic_handler(service: &'static str) {
    human_panic::setup_panic!(
        human_panic::Metadata::new(service, env!("CARGO_PKG_VERSION"),)
            .authors("Juicebox Developers")
            .homepage("https://juicebox.dev")
            .repository(ISSUES_URL)
            .support("Open an issue and attach the generated crash report.")
    );
}

pub fn exit_on_config_error(service: &str, error: impl std::fmt::Display) -> ! {
    eprintln!("\n{service} cannot start: {error}\n");
    eprintln!("Check your configuration and environment, then try again.");
    eprintln!("Need help? {ISSUES_URL}");
    std::process::exit(1);
}

/// Paths the UI polls or scrapers hit constantly; excluded from access logs.
#[must_use]
pub fn is_noisy_http_path(path: &str) -> bool {
    matches!(
        path,
        "/api/health"
            | "/api/config"
            | "/api/presence"
            | "/api/ip"
            | "/api/device/status"
            | "/robots.txt"
            | "/agents.txt"
    )
}

#[cfg(feature = "quic")]
pub mod server;

#[cfg(feature = "quic")]
pub use server::{
    QuicServerLimits, generate_self_signed_cert, get_or_generate_cert, load_cert_for_pinning,
    start_quic_server, start_quic_server_with_limits,
};

/// Compare secrets without short-circuiting on the first differing byte.
#[must_use]
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    use subtle::ConstantTimeEq;
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

/// Another scheme or extra parts yield `None`.
#[must_use]
pub fn extract_bearer_token(headers: &axum::http::HeaderMap) -> Option<&str> {
    let auth = headers.get("authorization")?.to_str().ok()?;
    let mut parts = auth.split_whitespace();
    let scheme = parts.next()?;
    let token = parts.next()?;
    if scheme.eq_ignore_ascii_case("bearer") && parts.next().is_none() {
        Some(token)
    } else {
        None
    }
}

/// Resolve on Ctrl+C or SIGTERM so callers can drain connections.
pub async fn shutdown_signal(service_name: &str) {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {
            tracing::info!("{service_name} shutting down...");
        },
        () = terminate => {
            tracing::info!("{service_name} terminated...");
        },
    }
}

/// File and preview routes get a looser CSP; others stay strict.
pub async fn add_security_headers(
    req: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> impl axum::response::IntoResponse {
    use axum::http::{HeaderValue, header};

    let is_file_route = req.uri().path().starts_with("/f/");
    let is_preview_route = req.uri().path().starts_with("/v/");

    let mut response = next.run(req).await;
    let headers = response.headers_mut();

    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        "referrer-policy",
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    headers.insert(
        "permissions-policy",
        HeaderValue::from_static("geolocation=(), microphone=(), camera=()"),
    );

    if is_file_route {
        if !headers.contains_key(header::CONTENT_SECURITY_POLICY) {
            headers.insert(
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static(
                    "default-src 'self'; style-src 'self' 'unsafe-inline'; font-src 'self' data:; img-src 'self' data:",
                ),
            );
        }
    } else if is_preview_route {
        if !headers.contains_key(header::CONTENT_SECURITY_POLICY) {
            headers.insert(
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static(
                    "default-src 'self'; script-src 'self' 'unsafe-inline' https://static.cloudflareinsights.com; style-src 'self' 'unsafe-inline'; font-src 'self' data:; img-src 'self' data:; frame-ancestors 'none'",
                ),
            );
        }
    } else {
        headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'self'; script-src 'self' 'unsafe-inline' https://static.cloudflareinsights.com; style-src 'self' 'unsafe-inline'; font-src 'self' data:; img-src 'self' data:; frame-ancestors 'none'; connect-src 'self' http: https: ws: wss:",
            ),
        );
    }

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_matches_string_equality() {
        assert!(constant_time_eq("hello", "hello"));
        assert!(constant_time_eq("", ""));
        assert!(!constant_time_eq("hello", "world"));
        assert!(!constant_time_eq("a", "ab"));
        assert!(!constant_time_eq("ab", "a"));
        assert!(!constant_time_eq("abc\0", "abc"));
        assert!(!constant_time_eq("abc", "abc\0"));
        assert!(!constant_time_eq("abc\0", "abc\0\0"));
    }

    #[test]
    fn extract_bearer_token_requires_one_token() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("authorization", "bearer token".parse().unwrap());
        assert_eq!(extract_bearer_token(&headers), Some("token"));
        headers.insert("authorization", "Bearer token extra".parse().unwrap());
        assert_eq!(extract_bearer_token(&headers), None);
    }

    #[test]
    fn noisy_paths_cover_polling_and_sse() {
        for path in [
            "/api/health",
            "/api/config",
            "/api/presence",
            "/api/ip",
            "/api/device/status",
            "/robots.txt",
            "/agents.txt",
        ] {
            assert!(is_noisy_http_path(path), "{path} should be noisy");
        }
        for path in ["/api/owned-files", "/upload", "/file/abc/info", "/"] {
            assert!(!is_noisy_http_path(path), "{path} should stay info");
        }
    }
}
