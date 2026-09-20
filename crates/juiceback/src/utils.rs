use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{HeaderMap, Request},
    middleware::Next,
    response::{IntoResponse, Response},
};
/// Compute HMAC-SHA256(pepper, ip) for deterministic ban lookups.
pub use juiceutils::ban::hash_ip_for_ban;
/// Truncate a hex string to 12 characters for display in logs/notifications.
pub use juiceutils::ban::truncate_hash;
/// Constant-time string comparison to prevent timing attacks on secrets
/// like delete tokens. Pads the shorter input to avoid leaking length.
pub(crate) use juiceutils::constant_time_eq;
/// Decrypt an IP address that was encrypted with [`encrypt_ip`].
pub use juiceutils::ip_crypt::decrypt_ip;
/// Encrypt an IP address with AES-256-GCM. See [`juiceutils::ip_crypt`].
pub use juiceutils::ip_crypt::encrypt_ip;

use crate::{error::AppError, state::AppState};

/// Validate a file ID: must be non-empty, within length bounds,
/// and contain only alphanumeric, `-`, or `_`.
#[must_use]
pub fn is_valid_id(id: &str) -> bool {
    juiceutils::ids::is_valid_custom_id(
        id,
        crate::constants::MIN_CUSTOM_ID_LEN,
        crate::constants::MAX_CUSTOM_ID_LEN,
    )
}

/// Normalize a custom ID: trim whitespace and convert to lowercase.
pub use juiceutils::ids::normalize_custom_id;
/// Build the public URL for a file, including an extension for client
/// compatibility.
pub use juiceutils::urls::public_url;

pub fn log_quic_throughput(id: &str, size: u64, total: Duration, parse: Duration) {
    let throughput = if total.as_secs_f64() > 0.0 {
        size as f64 / total.as_secs_f64() / crate::constants::BYTES_PER_MIB
    } else {
        0.0
    };
    tracing::info!(
        "quic_upload: id={} size={} total={:?} throughput={:.2} MB/s multipart_parse={:?}",
        id,
        size,
        total,
        throughput,
        parse,
    );
}

#[derive(Clone, Copy, Debug)]
pub struct ClientIp(pub IpAddr);

#[must_use]
pub fn client_ip(headers: &HeaderMap, peer_ip: IpAddr, state: &AppState) -> IpAddr {
    juiceutils::proxy::client_ip(headers, peer_ip, &state.config.trusted_proxy_cidrs)
}

pub async fn client_ip_middleware(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|value| value.0.ip())
        .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
    let ip = client_ip(req.headers(), peer, &state);
    req.extensions_mut().insert(ClientIp(ip));
    tracing::Span::current().record("client_ip", tracing::field::display(ip));
    next.run(req).await
}

#[derive(Clone, Debug)]
pub struct TrustedClientIpKeyExtractor {
    trusted_proxy_cidrs: Arc<Vec<juiceutils::proxy::IpCidr>>,
}

impl TrustedClientIpKeyExtractor {
    #[must_use]
    pub fn new(cidrs: Vec<juiceutils::proxy::IpCidr>) -> Self {
        Self {
            trusted_proxy_cidrs: Arc::new(cidrs),
        }
    }
}

impl tower_governor::key_extractor::KeyExtractor for TrustedClientIpKeyExtractor {
    type Key = IpAddr;

    fn extract<T>(
        &self,
        req: &axum::http::Request<T>,
    ) -> Result<Self::Key, tower_governor::GovernorError> {
        if let Some(client_ip) = req.extensions().get::<ClientIp>() {
            return Ok(client_ip.0);
        }
        let peer = req
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|value| value.0.ip())
            .ok_or(tower_governor::GovernorError::UnableToExtractKey)?;
        Ok(juiceutils::proxy::client_ip(
            req.headers(),
            peer,
            &self.trusted_proxy_cidrs,
        ))
    }
}

/// Axum middleware that checks if the client IP is banned.
pub async fn ban_check_middleware(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    // The ban-status endpoint must stay reachable for banned IPs: it is what
    // the frontend uses to render the /banned page (with the reason). Blocking
    // it would make the ban page itself unreachable.
    if req.uri().path() == "/api/ban-status" {
        return next.run(req).await;
    }

    // Prefer the ClientIp resolved by client_ip_middleware;;
    let raw_ip = req
        .extensions()
        .get::<ClientIp>()
        .map(|value| value.0)
        .filter(|ip| !ip.is_unspecified())
        .unwrap_or_else(|| {
            let peer = req
                .extensions()
                .get::<ConnectInfo<SocketAddr>>()
                .map(|value| value.0.ip())
                .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
            client_ip(req.headers(), peer, &state)
        })
        .to_string();
    if state.is_banned(&hash_ip_for_ban(&raw_ip, &state.config.ip_pepper)) {
        return AppError::Forbidden("you are banned".into()).into_response();
    }

    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_url_with_storage_host() {
        let url = public_url(
            "http://localhost:6402",
            &Some("https://files.example.com".into()),
            "abc123",
            "cute.gif",
        );
        assert_eq!(url, "https://files.example.com/f/abc123.gif");
    }

    #[test]
    fn public_url_without_storage_host() {
        let url = public_url("http://localhost:6402", &None, "abc123", "cute.gif");
        assert_eq!(url, "http://localhost:6402/f/abc123.gif");
    }

    #[test]
    fn public_url_empty_storage_host() {
        let url = public_url(
            "http://localhost:6402",
            &Some(String::new()),
            "abc123",
            "cute.gif",
        );
        assert_eq!(url, "http://localhost:6402/f/abc123.gif");
    }

    #[test]
    fn public_url_bare_storage_host() {
        let url = public_url(
            "https://f.juicey.dev",
            &Some("fx.juicey.dev".into()),
            "abc123",
            "cute.gif",
        );
        assert_eq!(url, "https://fx.juicey.dev/f/abc123.gif");
    }
}
