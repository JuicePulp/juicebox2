use std::sync::Arc;

use crate::state::AppState;

#[derive(Debug, PartialEq)]
pub(crate) enum HostError {
    Empty(&'static str),
    InvalidUrl(String),
    BadScheme(String),
    Credentials(&'static str),
    MissingHostname(&'static str),
    TrailingDotHostname,
    UnexpectedPath,
    MissingPort(&'static str),
    ForbiddenHost(String),
    NonPublicAddress(String),
    Dns(String),
    Unresolved(String),
    ClientBuild(String),
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty(msg)
            | Self::Credentials(msg)
            | Self::MissingHostname(msg)
            | Self::MissingPort(msg) => write!(f, "{msg}"),
            Self::InvalidUrl(msg)
            | Self::BadScheme(msg)
            | Self::ForbiddenHost(msg)
            | Self::NonPublicAddress(msg)
            | Self::Dns(msg)
            | Self::Unresolved(msg)
            | Self::ClientBuild(msg) => write!(f, "{msg}"),
            Self::TrailingDotHostname => {
                write!(f, "host must not contain a trailing-dot hostname")
            }
            Self::UnexpectedPath => {
                write!(f, "host must not contain a path, query, or fragment")
            }
        }
    }
}

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

pub(crate) async fn custom_juicehost_target(host: &str) -> Result<JuicehostTarget, HostError> {
    let trimmed = host.trim();
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
    let hostname = parsed
        .host_str()
        .ok_or(HostError::MissingHostname("host must include a hostname"))?;
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

    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none());
    if name.parse::<std::net::IpAddr>().is_err() {
        builder = builder.resolve_to_addrs(&name, &resolved);
    }
    let client = builder
        .build()
        .map_err(|e| HostError::ClientBuild(format!("failed to build custom host client: {e}")))?;

    Ok(JuicehostTarget {
        base_url: parsed.origin().ascii_serialization(),
        client,
        headers: reqwest::header::HeaderMap::new(),
    })
}

pub(crate) fn is_public_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => is_public_ipv4(v4.octets()),
        std::net::IpAddr::V6(v6) => {
            let octets = v6.octets();
            if let Some(v4) = v6.to_ipv4() {
                return is_public_ipv4(v4.octets());
            }
            !(v6.is_unspecified()
                || v6.is_loopback()
                || v6.is_multicast()
                || (octets[0] & 0xfe) == 0xfc
                || (octets[0] == 0xfe && octets[1] & 0xc0 == 0x80)
                || octets[..4] == [0x20, 0x01, 0x0d, 0xb8])
        }
    }
}

fn is_public_ipv4([a, b, c, _]: [u8; 4]) -> bool {
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

pub(crate) fn is_forbidden_hostname(name: &str) -> bool {
    name == "localhost"
        || name.ends_with(".localhost")
        || name.ends_with(".local")
        || name.ends_with(".internal")
        || name == "metadata"
        || name.starts_with("metadata.")
        || name == "instance-data"
        || name.starts_with("instance-data.")
}

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
    let host = parsed
        .host_str()
        .ok_or(HostError::MissingHostname("url must include a hostname"))?
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
    let trimmed = raw.trim().trim_end_matches('/').to_string();
    if trimmed.is_empty() {
        return Err(HostError::Empty("empty host"));
    }
    let with_scheme = if trimmed.contains("://") {
        trimmed.clone()
    } else {
        format!("https://{trimmed}")
    };
    let parsed = url::Url::parse(&with_scheme)
        .map_err(|e| HostError::InvalidUrl(format!("invalid host: {e}")))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(HostError::BadScheme(
            "host scheme must be http or https".to_string(),
        ));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(HostError::Credentials("host must not contain credentials"));
    }
    if parsed.path() != "/" || parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(HostError::UnexpectedPath);
    }
    let hostname = parsed
        .host_str()
        .ok_or(HostError::MissingHostname("host must include a hostname"))?;
    if hostname.ends_with('.') {
        return Err(HostError::TrailingDotHostname);
    }
    let name = hostname.to_ascii_lowercase();
    if is_forbidden_hostname(&name) {
        return Err(HostError::ForbiddenHost(format!(
            "refusing local or metadata host: {hostname}"
        )));
    }
    let port = parsed
        .port_or_known_default()
        .ok_or(HostError::MissingPort("host must include a valid port"))?;
    resolve_host_public(&name, port, allow_private).await?;
    Ok(trimmed)
}

pub(crate) async fn resolve_juicehost_target(
    state: &Arc<AppState>,
    host: Option<&str>,
) -> Result<JuicehostTarget, HostError> {
    match host.map(str::trim).filter(|h| !h.is_empty()) {
        Some(h) => custom_juicehost_target(h).await,
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

pub(crate) fn juicehost_headers(state: &Arc<AppState>) -> reqwest::header::HeaderMap {
    state.juicehost_headers.clone()
}
