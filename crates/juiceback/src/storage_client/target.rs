//! Host validation: normalizes hosts and blocks SSRF to private addresses.

use std::sync::Arc;

use thiserror::Error;

use crate::state::AppState;

#[derive(Debug, PartialEq, Error)]
pub(crate) enum HostError {
    #[error("{0}")]
    Empty(&'static str),
    #[error("{0}")]
    InvalidUrl(String),
    #[error("{0}")]
    BadScheme(String),
    #[error("{0}")]
    Credentials(&'static str),
    #[error("{0}")]
    MissingHostname(&'static str),
    #[error("host must not contain a trailing-dot hostname")]
    TrailingDotHostname,
    #[error("host must not contain a path, query, or fragment")]
    UnexpectedPath,
    #[error("{0}")]
    MissingPort(&'static str),
    #[error("{0}")]
    ForbiddenHost(String),
    #[error("{0}")]
    NonPublicAddress(String),
    #[error("{0}")]
    Dns(String),
    #[error("{0}")]
    Unresolved(String),
    #[error("{0}")]
    ClientBuild(String),
}

/// Some callers treat an unset URL as "skip the remote operation".
pub(crate) fn require_juicehost_url(url: &str) -> Result<(), HostError> {
    if url.is_empty() {
        Err(HostError::Empty("JUICEHOST_URL not set"))
    } else {
        Ok(())
    }
}

pub(crate) struct JuicehostTarget {
    pub base_url: String,
    pub client: reqwest::Client,
    pub headers: reqwest::header::HeaderMap,
}

pub(crate) use juiceutils::host::{is_forbidden_hostname, is_public_ip};

struct ValidatedHost {
    parsed: url::Url,
    normalized: String,
    name: String,
    resolved: Vec<std::net::SocketAddr>,
}

/// `url` keeps brackets on IPv6 literals; DNS and IP parsing want them bare.
fn bare_hostname(host: &str) -> &str {
    host.strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host)
}

/// One parse and SSRF check shared by target construction and pre-upload
/// validation.
async fn validate_host(raw: &str, allow_private: bool) -> Result<ValidatedHost, HostError> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(HostError::Empty("empty host"));
    }
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    let parsed = url::Url::parse(&with_scheme)
        .map_err(|e| HostError::InvalidUrl(format!("invalid host URL: {e}")))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(HostError::BadScheme(format!(
            "host scheme must be http or https, got {}",
            parsed.scheme()
        )));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(HostError::Credentials(
            "host URL must not contain credentials",
        ));
    }
    if parsed.path() != "/" || parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(HostError::UnexpectedPath);
    }
    let hostname = bare_hostname(
        parsed
            .host_str()
            .ok_or(HostError::MissingHostname("host must include a hostname"))?,
    );
    if hostname.ends_with('.') {
        return Err(HostError::TrailingDotHostname);
    }

    let port = parsed
        .port_or_known_default()
        .ok_or(HostError::MissingPort("host URL must include a valid port"))?;
    let name = hostname.to_ascii_lowercase();
    if is_forbidden_hostname(&name) {
        return Err(HostError::ForbiddenHost(format!(
            "refusing to reach local or metadata host: {hostname}"
        )));
    }
    if allow_private {
        return Ok(ValidatedHost {
            parsed,
            normalized: trimmed.to_string(),
            name,
            resolved: Vec::new(),
        });
    }

    let mut resolved = if let Ok(ip) = name.parse::<std::net::IpAddr>() {
        vec![std::net::SocketAddr::new(ip, port)]
    } else {
        tokio::net::lookup_host((name.as_str(), port))
            .await
            .map_err(|e| HostError::Dns(format!("dns lookup failed for {name}: {e}")))?
            .collect::<Vec<_>>()
    };
    resolved.sort_unstable();
    resolved.dedup();
    if resolved.is_empty() {
        return Err(HostError::Unresolved(format!(
            "host did not resolve: {hostname}"
        )));
    }
    if let Some(addr) = resolved.iter().find(|addr| !is_public_ip(addr.ip())) {
        return Err(HostError::NonPublicAddress(format!(
            "refusing to reach non-public address: {}",
            addr.ip()
        )));
    }
    Ok(ValidatedHost {
        parsed,
        normalized: trimmed.to_string(),
        name,
        resolved,
    })
}

/// Client pinned to the addresses that passed the public-address check.
pub(crate) async fn custom_juicehost_target(host: &str) -> Result<JuicehostTarget, HostError> {
    let validated = validate_host(host, false).await?;

    // Bypass proxies and pin DNS to the validated addresses; a second lookup could
    // be rebound.
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none());
    if validated.name.parse::<std::net::IpAddr>().is_err() {
        builder = builder.resolve_to_addrs(&validated.name, &validated.resolved);
    }
    let client = builder
        .build()
        .map_err(|e| HostError::ClientBuild(format!("failed to build custom host client: {e}")))?;

    Ok(JuicehostTarget {
        base_url: validated.parsed.origin().ascii_serialization(),
        client,
        headers: reqwest::header::HeaderMap::new(),
    })
}

/// Checks every resolved address; `allow_private` skips the check entirely.
pub(crate) async fn resolve_host_public(
    host: &str,
    port: u16,
    allow_private: bool,
) -> Result<(), HostError> {
    let normalized = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    if allow_private {
        return Ok(());
    }
    if let Ok(ip) = normalized.parse::<std::net::IpAddr>() {
        return if is_public_ip(ip) {
            Ok(())
        } else {
            Err(HostError::NonPublicAddress(format!(
                "refusing to reach non-public address: {ip}"
            )))
        };
    }
    let addrs = tokio::net::lookup_host((normalized.as_str(), port))
        .await
        .map_err(|e| HostError::Dns(format!("dns lookup failed for {normalized}: {e}")))?;
    let mut any = false;
    for addr in addrs {
        any = true;
        if !is_public_ip(addr.ip()) {
            return Err(HostError::NonPublicAddress(format!(
                "refusing to reach non-public address: {}",
                addr.ip()
            )));
        }
    }
    if any {
        Ok(())
    } else {
        Err(HostError::Unresolved(format!(
            "host did not resolve: {normalized}"
        )))
    }
}

pub(crate) async fn check_outbound_url(
    raw: &str,
    allow_private: bool,
) -> Result<String, HostError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(HostError::Empty("url is required"));
    }
    let parsed =
        url::Url::parse(trimmed).map_err(|_| HostError::InvalidUrl("invalid url".to_string()))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(HostError::BadScheme(
            "url scheme must be http or https".to_string(),
        ));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(HostError::Credentials("url must not contain credentials"));
    }
    let host = bare_hostname(
        parsed
            .host_str()
            .ok_or(HostError::MissingHostname("url must include a hostname"))?,
    )
    .to_ascii_lowercase();
    if is_forbidden_hostname(&host) {
        return Err(HostError::ForbiddenHost(format!(
            "refusing to reach local or metadata host: {host}"
        )));
    }
    let port = parsed
        .port_or_known_default()
        .ok_or(HostError::MissingPort("url must include a valid port"))?;
    resolve_host_public(&host, port, allow_private).await?;
    Ok(parsed.to_string())
}

pub(crate) async fn check_storage_host(
    raw: &str,
    allow_private: bool,
) -> Result<String, HostError> {
    Ok(validate_host(raw, allow_private).await?.normalized)
}

/// Internal auth headers only go to official hosts over https, or a local
/// plain-http address.
pub(crate) async fn resolve_juicehost_target(
    state: &Arc<AppState>,
    host: Option<&str>,
) -> Result<JuicehostTarget, HostError> {
    match host.map(str::trim).filter(|h| !h.is_empty()) {
        Some(h) => {
            let mut target = custom_juicehost_target(h).await?;
            let local_http = target.base_url.starts_with("http://127.0.0.1")
                || target.base_url.starts_with("http://localhost");
            let secure = target.base_url.starts_with("https://") || local_http;
            if secure && state.is_official_host(h) {
                target.headers = state.juicehost_headers.clone();
            }
            Ok(target)
        }
        None => {
            require_juicehost_url(&state.config.juicehost_url)?;
            Ok(JuicehostTarget {
                base_url: state.config.juicehost_url.trim_end_matches('/').to_string(),
                client: state.http.clone(),
                headers: state.juicehost_headers.clone(),
            })
        }
    }
}
