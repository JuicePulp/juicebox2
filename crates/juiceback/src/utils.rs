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
    response::Response,
};

pub use juiceutils::ban::hash_ip_for_ban;

pub use juiceutils::ban::truncate_hash;

pub(crate) use juiceutils::constant_time_eq;

pub use juiceutils::ip_crypt::decrypt_ip;

pub use juiceutils::ip_crypt::encrypt_ip;

use crate::state::AppState;

#[must_use]
pub fn is_valid_id(id: &str) -> bool {
    juiceutils::ids::is_valid_custom_id(
        id,
        crate::constants::MIN_CUSTOM_ID_LEN,
        crate::constants::MAX_CUSTOM_ID_LEN,
    )
}

pub use juiceutils::ids::normalize_custom_id;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_url_with_storage_host() {
        let url = public_url(
            "http://localhost:6402",
            Some("https://files.example.com"),
            "abc123",
            "cute.gif",
        );
        assert_eq!(url, "https://files.example.com/f/abc123.gif");
    }

    #[test]
    fn public_url_without_storage_host() {
        let url = public_url("http://localhost:6402", None, "abc123", "cute.gif");
        assert_eq!(url, "http://localhost:6402/f/abc123.gif");
    }

    #[test]
    fn public_url_empty_storage_host() {
        let url = public_url("http://localhost:6402", Some(""), "abc123", "cute.gif");
        assert_eq!(url, "http://localhost:6402/f/abc123.gif");
    }

    #[test]
    fn public_url_bare_storage_host() {
        let url = public_url(
            "https://f.juicey.dev",
            Some("fx.juicey.dev"),
            "abc123",
            "cute.gif",
        );
        assert_eq!(url, "https://fx.juicey.dev/f/abc123.gif");
    }
}
