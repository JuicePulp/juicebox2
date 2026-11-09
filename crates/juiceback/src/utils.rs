use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
};

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{HeaderMap, Request},
    middleware::Next,
    response::Response,
};
pub(crate) use juiceutils::constant_time_eq;
pub use juiceutils::{
    ban::{hash_ip_for_ban, truncate_hash},
    ip_crypt::{decrypt_ip, encrypt_ip},
};

use crate::state::AppState;

/// 3 to 32 characters.
#[must_use]
pub fn is_valid_id(id: &str) -> bool {
    juiceutils::ids::is_valid_custom_id(
        id,
        crate::constants::MIN_CUSTOM_ID_LEN,
        crate::constants::MAX_CUSTOM_ID_LEN,
    )
}

pub use juiceutils::{
    ids::{is_valid_id_component, normalize_custom_id},
    urls::{public_url, public_url_for_route},
};

/// An empty host counts as official, matching records with no explicit storage
/// host.
#[must_use]
pub fn is_official_host(config: &crate::config::Config, host: &str) -> bool {
    crate::hosts::DeploymentHosts::from_config(config).is_share_host(Some(host))
}

#[must_use]
pub fn share_link(
    config: &crate::config::Config,
    storage_host: Option<&str>,
    id: &str,
    filename: &str,
) -> String {
    share_link_for(config, "v", storage_host, id, filename)
}

#[must_use]
pub fn share_link_for(
    config: &crate::config::Config,
    route: &str,
    storage_host: Option<&str>,
    id: &str,
    filename: &str,
) -> String {
    if let Some(router) = config
        .router_base_url
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        let official =
            crate::hosts::DeploymentHosts::from_config(config).is_share_host(storage_host);
        if official {
            return public_url_for_route(route, router, None, id, filename);
        }
    }
    public_url_for_route(route, &config.public_base_url, storage_host, id, filename)
}

#[must_use]
pub fn share_links(
    config: &crate::config::Config,
    storage_host: Option<&str>,
    id: &str,
    filename: &str,
) -> (String, String, String) {
    (
        share_link_for(config, "v", storage_host, id, filename),
        share_link_for(config, "f", storage_host, id, filename),
        share_link_for(config, "d", storage_host, id, filename),
    )
}

#[derive(Clone, Copy, Debug)]
pub struct ClientIp(pub IpAddr);

/// Forwarding headers are trusted only when the peer is a trusted proxy.
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

/// Keyed on the resolved client IP, not the socket peer.
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
