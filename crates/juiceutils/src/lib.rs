pub mod ban;
pub mod config;
pub mod file_validation;
pub mod ids;
pub mod ip_crypt;
pub mod proxy;
pub mod urls;
pub mod web;

/// High-frequency polling / long-lived endpoints whose per-request
/// `http.request` span would otherwise spam `info` logs via
/// `FmtSpan::CLOSE` (health + config polling, SSE presence handshakes).
/// Callers skip the span entirely (`Span::none()`) so neither `info` nor
/// `debug` deployments pay per-request log lines for these paths.
#[must_use]
pub fn is_noisy_http_path(path: &str) -> bool {
    matches!(
        path,
        "/api/health" | "/api/config" | "/api/presence" | "/api/ip" | "/api/device/status"
    )
}

#[cfg(feature = "quic")]
pub mod server;

#[cfg(feature = "quic")]
pub use server::{
    QuicServerLimits, generate_self_signed_cert, get_or_generate_cert, load_cert_for_pinning,
    start_quic_server, start_quic_server_with_limits,
};

#[must_use]
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    use subtle::ConstantTimeEq;
    let max_len = a.len().max(b.len());
    let a_padded = a.bytes().chain(std::iter::repeat(0u8)).take(max_len);
    let b_padded = b.bytes().chain(std::iter::repeat(0u8)).take(max_len);
    let mut result = 0u8;
    for (x, y) in a_padded.zip(b_padded) {
        result |= x ^ y;
    }
    result.ct_eq(&0u8).into()
}

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

/// Wait for Ctrl+C (and SIGTERM on unix) before returning.
///
/// # Panics
///
/// Panics if the OS signal handlers cannot be installed, which only
/// happens in environments without signal support.
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
        // Bytes AND unlock shells share this path. Shells set their own
        // tight CSP in the handler (see below); anything else keeps the
        // strict no-script policy so stored HTML/SVG uploads can't execute.
        if !headers.contains_key(header::CONTENT_SECURITY_POLICY) {
            headers.insert(
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static(
                    "default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:",
                ),
            );
        }
    } else if is_preview_route {
        // The preview page carries inline styles, one inline script, and
        // an inlined brand font (data: URI) - everything else is same-origin.
        // Unlock shells served here set their own CSP (cross-origin gateway
        // fetch); keep it.
        if !headers.contains_key(header::CONTENT_SECURITY_POLICY) {
            headers.insert(
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static(
                    "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; font-src 'self' data:; img-src 'self' data:; frame-ancestors 'none'",
                ),
            );
        }
    } else {
        headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; frame-ancestors 'none'; connect-src 'self' http: https: ws: wss:",
            ),
        );
    }

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_equal() {
        assert!(constant_time_eq("hello", "hello"));
    }

    #[test]
    fn constant_time_eq_unequal() {
        assert!(!constant_time_eq("hello", "world"));
    }

    #[test]
    fn constant_time_eq_different_lengths() {
        assert!(!constant_time_eq("a", "ab"));
        assert!(!constant_time_eq("ab", "a"));
    }

    #[test]
    fn constant_time_eq_empty() {
        assert!(constant_time_eq("", ""));
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
        ] {
            assert!(is_noisy_http_path(path), "{path} should be noisy");
        }
        for path in ["/api/owned-files", "/upload", "/file/abc/info", "/"] {
            assert!(!is_noisy_http_path(path), "{path} should stay info");
        }
    }
}
