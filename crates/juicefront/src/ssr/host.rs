use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use crate::state::AppState;

const CACHE_TTL: Duration = Duration::from_secs(300);
const FETCH_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HostConfig {
    pub max_file_size_bytes: Option<u64>,
    pub max_ttl_hours: Option<f64>,
    pub default_ttl_hours: Option<f64>,
    pub allowed_ttl_hours: Option<Vec<f64>>,
    pub danger_level: Option<String>,
    pub quick_link: Option<bool>,
    pub custom_id: Option<bool>,
    pub ultrafast: Option<bool>,
    pub quic: Option<bool>,
    pub cobalt: Option<bool>,
    pub upload_mode: Option<String>,
    pub fetch_rate_limit_per_minute: Option<u64>,
    pub public_base_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EffectiveHostConfig {
    pub config: HostConfig,
    pub custom: bool,
    pub host: String,
}

pub type HostCache = Mutex<HashMap<String, (HostConfig, Instant)>>;

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
    normalized.set_path("/");
    normalized.set_query(None);
    normalized.set_fragment(None);
    Some(normalized.to_string().trim_end_matches('/').to_owned())
}

fn is_blocked_host(base: &str) -> bool {
    let Ok(parsed) = base.parse::<url::Url>() else {
        return true;
    };
    let host = parsed.host_str().unwrap_or("").to_lowercase();
    let host = host.strip_prefix('[').unwrap_or(&host);
    let host = host.strip_suffix(']').unwrap_or(host);
    if host == "localhost" || host.ends_with(".local") || host.ends_with(".internal") {
        return true;
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return match ip {
            std::net::IpAddr::V4(v4) => {
                let octets = v4.octets();
                octets[0] == 10
                    || (octets[0] == 172 && (16..=31).contains(&octets[1]))
                    || (octets[0] == 192 && octets[1] == 168)
                    || (octets[0] == 169 && octets[1] == 254)
                    || octets[0] == 127
            }
            std::net::IpAddr::V6(v6) => {
                v6.is_loopback() || v6.is_unspecified() || is_v6_private(&v6)
            }
        };
    }
    let dotted: Vec<&str> = host.split('.').collect();
    if dotted.len() == 4 && dotted.iter().all(|part| part.parse::<u8>().is_ok()) {
        let octets: Vec<u8> = dotted
            .iter()
            .map(|part| part.parse().unwrap_or(0))
            .collect();
        return octets[0] == 10
            || (octets[0] == 172 && (16..=31).contains(&octets[1]))
            || (octets[0] == 192 && octets[1] == 168)
            || (octets[0] == 169 && octets[1] == 254)
            || octets[0] == 127;
    }
    false
}

const fn is_v6_private(ip: &std::net::Ipv6Addr) -> bool {
    let segments = ip.segments();
    (segments[0] & 0xfe00) == 0xfc00 || (segments[0] & 0xffc0) == 0xfe80
}

fn sanitize(mut config: HostConfig) -> Option<HostConfig> {
    if let Some(ttl) = config.allowed_ttl_hours.take() {
        let kept: Vec<f64> = ttl.into_iter().filter(|v| v.is_finite()).collect();
        config.allowed_ttl_hours = if kept.is_empty() { None } else { Some(kept) };
    }
    if config.max_file_size_bytes.is_none()
        && config.allowed_ttl_hours.is_none()
        && config.default_ttl_hours.is_none()
    {
        return None;
    }
    Some(config)
}

pub async fn fetch_host_config(
    state: &AppState,
    host: &str,
    allow_private: bool,
) -> Option<HostConfig> {
    let base = normalize_host(host)?;
    if !allow_private && is_blocked_host(&base) {
        return None;
    }
    if let Ok(cache) = state.host_cache.lock()
        && let Some((cached, at)) = cache.get(&base)
        && at.elapsed() < CACHE_TTL
    {
        return Some(cached.clone());
    }
    let fetched = async {
        let response = tokio::time::timeout(
            FETCH_TIMEOUT,
            async {
                let response = state.http.get(format!("{base}/api/config")).send().await.ok()?;
                if !response.status().is_success() {
                    return None;
                }
                response.json::<HostConfig>().await.ok()
            },
        )
        .await
        .ok()??;
        sanitize(response)
    }
    .await;
    match fetched {
        Some(config) => {
            if let Ok(mut cache) = state.host_cache.lock() {
                cache.insert(base, (config.clone(), Instant::now()));
            }
            Some(config)
        }
        None => state
            .host_cache
            .lock()
            .ok()
            .and_then(|cache| cache.get(&base).map(|(config, _)| config.clone())),
    }
}

pub async fn fetch_cobalt_services(state: &AppState) -> Vec<String> {
    let url = format!("{}/api/fetch/services", state.config.juiceback_url);
    let fetch = async {
        let response = state.http.get(&url).send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        response.json::<serde_json::Value>().await.ok()
    };
    match tokio::time::timeout(FETCH_TIMEOUT, fetch).await {
        Ok(Some(data)) => data
            .get("services")
            .and_then(|services| services.as_array()).cloned()
            .map(|services| {
                services
                    .into_iter()
                    .filter_map(|service| service.as_str().map(str::to_owned))
                    .filter(|service| !service.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

pub fn read_cookie_host(cookie_header: Option<&str>) -> Option<String> {
    let header = cookie_header?;
    for part in header.split(';') {
        let (name, value) = part.split_once('=')?;
        if name.trim() != "juicebox_host" {
            continue;
        }
        let value = value.trim();
        if value.is_empty() {
            return None;
        }
        return Some(percent_decode(value));
    }
    None
}

pub(crate) fn percent_decode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
        {
            out.push((h * 16 + l) as char);
            i += 3;
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

const fn hex_val(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub async fn resolve_effective_config(
    state: &AppState,
    cookie_host: Option<&str>,
) -> EffectiveHostConfig {
    let fallback = EffectiveHostConfig {
        config: HostConfig::default(),
        custom: false,
        host: state.config.juiceback_url.clone(),
    };
    let default_config = fetch_host_config(state, &state.config.juiceback_url, true).await;
    let Some(cookie_host) = cookie_host else {
        return EffectiveHostConfig {
            config: default_config.unwrap_or_default(),
            ..fallback
        };
    };
    match fetch_host_config(state, cookie_host, false).await {
        Some(mut custom) => {
            if custom.cobalt.is_none() {
                custom.cobalt = default_config.as_ref().and_then(|config| config.cobalt);
            }
            EffectiveHostConfig {
                host: normalize_host(cookie_host).unwrap_or_else(|| cookie_host.to_owned()),
                config: custom,
                custom: true,
            }
        }
        None => EffectiveHostConfig {
            config: default_config.unwrap_or_default(),
            ..fallback
        },
    }
}

pub fn prewarm_known_providers(state: std::sync::Arc<AppState>) {
    let mut providers = vec![state.config.juiceback_url.clone()];
    if let Some(extra) = juiceutils::config::optional_secret("JUICEFRONT_KNOWN_HOSTS") {
        for host in extra
            .split(',')
            .map(str::trim)
            .filter(|host| !host.is_empty())
        {
            if !providers.iter().any(|known| known == host) {
                providers.push(host.to_owned());
            }
        }
    }
    tokio::spawn(async move {
        for host in providers {
            fetch_host_config(&state, &host, true).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_host_adds_scheme_and_strips_path() {
        assert_eq!(
            normalize_host("f.juicey.dev/some/path?x=1"),
            Some("https://f.juicey.dev".to_owned())
        );
        assert_eq!(
            normalize_host("http://127.0.0.1:6401/"),
            Some("http://127.0.0.1:6401".to_owned())
        );
        assert_eq!(normalize_host(""), None);
        assert_eq!(normalize_host("ftp://example.com"), None);
    }

    #[test]
    fn private_hosts_are_blocked() {
        assert!(is_blocked_host("http://127.0.0.1:6401"));
        assert!(is_blocked_host("http://localhost:6401"));
        assert!(is_blocked_host("http://192.168.1.10"));
        assert!(is_blocked_host("http://10.0.0.5"));
        assert!(!is_blocked_host("https://box.juicey.dev"));
    }

    #[test]
    fn cookie_host_parses() {
        assert_eq!(
            read_cookie_host(Some("a=1; juicebox_host=https%3A%2F%2Ff.juicey.dev; b=2")),
            Some("https://f.juicey.dev".to_owned())
        );
        assert_eq!(read_cookie_host(Some("a=1")), None);
        assert_eq!(read_cookie_host(None), None);
    }
}
