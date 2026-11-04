use std::{
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

use dashmap::DashMap;
use serde::{Deserialize, Serialize};

use crate::config::{Config, GeoSettings};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    Unknown,
    Healthy,
    /// Reached the node, but it rejected the shared key.
    AuthError,
    /// Reached the node, but `/internal` is not exposed.
    RouteMissing,
    Unreachable,
}

impl HealthState {
    fn rank(self) -> u8 {
        match self {
            Self::Healthy => 0,
            Self::Unknown => 1,
            Self::Unreachable => 2,
            Self::AuthError => 3,
            Self::RouteMissing => 4,
        }
    }

    #[must_use]
    pub fn usable(self) -> bool {
        matches!(self, Self::Healthy | Self::Unknown)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct NodeHealth {
    pub state: HealthState,
    pub latency_ms: Option<u64>,
    pub checked_secs_ago: Option<u64>,
    pub error: Option<String>,
}

impl Default for NodeHealth {
    fn default() -> Self {
        Self {
            state: HealthState::Unknown,
            latency_ms: None,
            checked_secs_ago: None,
            error: None,
        }
    }
}

#[derive(Debug)]
pub struct Node {
    pub region: String,
    pub url: String,
    health: RwLock<NodeHealth>,
    checked_at: RwLock<Option<Instant>>,
}

impl Node {
    fn new(region: String, url: String) -> Self {
        Self {
            region,
            url: url.trim_end_matches('/').to_string(),
            health: RwLock::new(NodeHealth::default()),
            checked_at: RwLock::new(None),
        }
    }

    #[must_use]
    pub fn health(&self) -> NodeHealth {
        let mut health = self
            .health
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        health.checked_secs_ago = self
            .checked_at
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .map(|at| at.elapsed().as_secs());
        health
    }

    pub fn set_health(&self, health: NodeHealth) {
        *self
            .health
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = health;
        *self
            .checked_at
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Instant::now());
    }
}

#[derive(Debug)]
pub struct Registry {
    pub nodes: Vec<Node>,
}

impl Registry {
    #[must_use]
    pub fn new(config: &Config) -> Self {
        let mut nodes: Vec<Node> = config
            .regions
            .public_juicehosts
            .iter()
            .map(|(region, url)| Node::new(region.clone(), url.clone()))
            .collect();
        nodes.sort_by(|a, b| a.region.cmp(&b.region));

        let home = config.router.home_url.trim_end_matches('/');
        if !home.is_empty() && !nodes.iter().any(|node| node.url == home) {
            nodes.push(Node::new("home".to_string(), home.to_string()));
        }
        Self { nodes }
    }

    #[must_use]
    #[allow(dead_code)] // used by tests and kept for future policy callers
    pub fn get(&self, region: &str) -> Option<&Node> {
        self.nodes.iter().find(|node| node.region == region)
    }

    fn best_in<'a>(&'a self, region: &str) -> Option<&'a Node> {
        self.nodes
            .iter()
            .filter(|node| node.region == region)
            .min_by_key(|node| {
                let health = node.health();
                (health.state.rank(), health.latency_ms.unwrap_or(u64::MAX))
            })
    }

    fn best_overall(&self) -> Option<&Node> {
        self.nodes
            .iter()
            .filter(|node| node.health().state.usable())
            .min_by_key(|node| {
                let health = node.health();
                (health.state.rank(), health.latency_ms.unwrap_or(u64::MAX))
            })
    }

    /// Pick a node for an upload.
    ///
    /// Preference order: an explicitly requested region, then the country
    /// map, then the Cloudflare colo map, then the configured default. If the
    /// preferred region has no usable node, fall back to any usable node.
    #[must_use]
    pub fn select(
        &self,
        geo: &GeoSettings,
        requested: Option<&str>,
        country: Option<&str>,
        colo: Option<&str>,
    ) -> Option<Selection> {
        let mut candidates: Vec<(&str, &str)> = Vec::new();
        if let Some(region) = requested.map(str::trim).filter(|value| !value.is_empty()) {
            candidates.push((region, "requested"));
        }
        if let Some(region) = country
            .map(|value| value.trim().to_ascii_uppercase())
            .and_then(|value| geo.countries.get(&value).map(String::as_str))
        {
            candidates.push((region, "country"));
        }
        if let Some(region) = colo
            .map(|value| value.trim().to_ascii_uppercase())
            .and_then(|value| geo.colos.get(&value).map(String::as_str))
        {
            candidates.push((region, "colo"));
        }
        if let Some(region) = geo.default_region.as_deref() {
            candidates.push((region, "default"));
        }

        for (region, reason) in candidates {
            if let Some(node) = self.best_in(region) {
                let health = node.health();
                if health.state.usable() {
                    return Some(Selection {
                        region: node.region.clone(),
                        url: node.url.clone(),
                        reason: reason.to_string(),
                        health,
                    });
                }
            }
        }

        self.best_overall().map(|node| Selection {
            region: node.region.clone(),
            url: node.url.clone(),
            reason: "fallback".to_string(),
            health: node.health(),
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Selection {
    pub region: String,
    pub url: String,
    pub reason: String,
    pub health: NodeHealth,
}

/// Routing information for one file, as returned by juiceback's
/// `/internal/resolve/{id}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolveRecord {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub storage_host: Option<String>,
    #[serde(default)]
    pub tier: Option<String>,
    #[serde(default)]
    pub r2_url: Option<String>,
    #[serde(default)]
    pub r2_key: Option<String>,
    #[serde(default)]
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteKind {
    Local,
    Remote,
    R2,
    Uploading,
}

impl RouteKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote => "remote",
            Self::R2 => "r2",
            Self::Uploading => "uploading",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RouteDecision {
    pub id: String,
    pub kind: RouteKind,
    pub status: String,
    /// Absolute URL including the `/v|f|d` path, or the R2 object URL.
    pub target: Option<String>,
    pub storage_host: Option<String>,
    pub r2_url: Option<String>,
    pub expires_at: Option<i64>,
}

pub struct AppState {
    pub config: Config,
    pub registry: Registry,
    pub client: reqwest::Client,
    /// No global timeout: used for streaming local files in proxy mode.
    pub stream_client: reqwest::Client,
    pub cache: DashMap<String, (Instant, ResolveRecord)>,
    pub api_key: String,
}

impl AppState {
    #[must_use]
    pub fn new(config: Config) -> Arc<Self> {
        let registry = Registry::new(&config);
        let timeout = Duration::from_millis(config.router.backend_timeout_ms.max(100));
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(Duration::from_secs(2))
            .pool_idle_timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        let stream_client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .pool_idle_timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        let api_key = config.api_key();
        if api_key.is_empty() {
            tracing::warn!("no API key configured; juiceback resolve will fail");
        }
        Arc::new(Self {
            config,
            registry,
            client,
            stream_client,
            cache: DashMap::new(),
            api_key,
        })
    }

    /// Probe a node's writability/auth by calling the same `/internal` route
    /// juiceback uses before a push. Requires no writes: a missing file still
    /// returns 200 with `{"exists":false}` when the key is accepted.
    pub async fn probe_node(&self, node: &Node) {
        let url = format!("{}/internal/file/00000000/stat", node.url);
        let started = Instant::now();
        let timeout = Duration::from_millis(self.config.router.probe_timeout_ms.max(100));
        let mut request = self.client.get(url).timeout(timeout);
        if !self.api_key.is_empty() {
            request = request
                .header("x-juicehost-api-key", &self.api_key)
                .header("x-juiceback-origin", &self.config.router.juiceback_origin);
        }
        let health = match request.send().await {
            Ok(response) => {
                let status = response.status();
                let state = match status.as_u16() {
                    200..=299 => HealthState::Healthy,
                    401 | 403 => HealthState::AuthError,
                    404 => HealthState::RouteMissing,
                    _ => HealthState::Unreachable,
                };
                NodeHealth {
                    state,
                    latency_ms: Some(started.elapsed().as_millis() as u64),
                    checked_secs_ago: None,
                    error: (state != HealthState::Healthy)
                        .then(|| format!("status {}", status.as_u16())),
                }
            }
            Err(error) => NodeHealth {
                state: HealthState::Unreachable,
                latency_ms: None,
                checked_secs_ago: None,
                error: Some(error.to_string()),
            },
        };
        node.set_health(health);
    }

    /// Resolve a file ID through the central juiceback, with a short cache.
    pub async fn resolve(&self, id: &str) -> Result<Option<ResolveRecord>, ResolveError> {
        if let Some(entry) = self.cache.get(id) {
            let ttl = if entry.1.status == "uploading" {
                Duration::from_secs(2)
            } else {
                Duration::from_secs(self.config.router.resolve_cache_secs)
            };
            if entry.0.elapsed() < ttl {
                return Ok(Some(entry.1.clone()));
            }
        }
        let url = format!(
            "{}/internal/resolve/{id}",
            self.config.router.juiceback_url.trim_end_matches('/')
        );
        let mut request = self.client.get(url);
        if !self.api_key.is_empty() {
            request = request.header("x-juicehost-api-key", &self.api_key);
        }
        let response = request.send().await.map_err(|error| {
            tracing::warn!("resolve {id}: {error}");
            ResolveError::Unavailable
        })?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            tracing::warn!("resolve {id}: status {}", response.status());
            return Err(ResolveError::Unavailable);
        }
        let record = response.json::<ResolveRecord>().await.map_err(|error| {
            tracing::warn!("resolve {id}: bad body: {error}");
            ResolveError::Unavailable
        })?;
        if self.cache.len() > 20_000 {
            self.cache.retain(|_, entry| {
                entry.0.elapsed() < Duration::from_secs(self.config.router.resolve_cache_secs)
            });
            if self.cache.len() > 20_000 {
                self.cache.clear();
            }
        }
        self.cache
            .insert(id.to_string(), (Instant::now(), record.clone()));
        Ok(Some(record))
    }

    /// Build the routing decision for a request path. `kind == Uploading`
    /// means the caller should not redirect yet.
    #[must_use]
    pub fn decision(
        &self,
        id: &str,
        record: &ResolveRecord,
        route: &str,
        path: &str,
    ) -> RouteDecision {
        let r2_url = record
            .r2_url
            .as_deref()
            .map(str::trim)
            .filter(|url| is_http_url(url));
        let storage_host = record
            .storage_host
            .as_deref()
            .map(str::trim)
            .filter(|host| !host.is_empty() && is_http_url(host));

        let kind = if record.status == "uploading" {
            RouteKind::Uploading
        } else if r2_url.is_some() {
            RouteKind::R2
        } else if storage_host.is_some() {
            RouteKind::Remote
        } else {
            RouteKind::Local
        };

        let relative = format!("{}/{}", route, path.trim_start_matches('/'));
        let target = match kind {
            RouteKind::Uploading => None,
            RouteKind::R2 => r2_url.map(ToOwned::to_owned),
            RouteKind::Remote => {
                storage_host.map(|host| format!("{}/{relative}", host.trim_end_matches('/')))
            }
            RouteKind::Local => Some(format!(
                "{}/{relative}",
                self.config.router.home_url.trim_end_matches('/')
            )),
        };

        RouteDecision {
            id: id.to_string(),
            kind,
            status: record.status.clone(),
            target,
            storage_host: record.storage_host.clone(),
            r2_url: record.r2_url.clone(),
            expires_at: record.expires_at,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveError {
    Unavailable,
}

fn is_http_url(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|url| url.scheme() == "http" || url.scheme() == "https")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        let mut config = Config::default();
        config.router.home_url = "https://f.example".to_string();
        config
            .regions
            .public_juicehosts
            .insert("eu1".to_string(), "https://f-eu1.example".to_string());
        config
            .regions
            .public_juicehosts
            .insert("us1".to_string(), "https://f-us1.example".to_string());
        config
    }

    #[test]
    fn registry_includes_home_node_once() {
        let registry = Registry::new(&config());
        assert_eq!(registry.nodes.len(), 3);
        assert!(registry.get("home").is_some());
    }

    #[test]
    fn country_prefers_its_region() {
        let config = config();
        let registry = Registry::new(&config);
        let mut geo = GeoSettings::default();
        geo.countries.insert("US".to_string(), "us1".to_string());
        let selection = registry
            .select(&geo, None, Some("US"), None)
            .expect("selection");
        assert_eq!(selection.region, "us1");
        assert_eq!(selection.reason, "country");
    }

    #[test]
    fn explicit_region_wins_over_country() {
        let config = config();
        let registry = Registry::new(&config);
        let mut geo = GeoSettings::default();
        geo.countries.insert("US".to_string(), "us1".to_string());
        let selection = registry
            .select(&geo, Some("eu1"), Some("US"), None)
            .expect("selection");
        assert_eq!(selection.region, "eu1");
        assert_eq!(selection.reason, "requested");
    }

    #[test]
    fn falls_back_to_home_when_no_region_matches() {
        let config = config();
        let registry = Registry::new(&config);
        let geo = GeoSettings::default();
        let selection = registry
            .select(&geo, None, Some("ZZ"), None)
            .expect("selection");
        assert_eq!(selection.reason, "fallback");
    }

    #[test]
    fn uploading_files_do_not_redirect() {
        let state = AppState::new(config());
        let record = ResolveRecord {
            status: "uploading".to_string(),
            storage_host: None,
            tier: None,
            r2_url: None,
            r2_key: None,
            expires_at: None,
        };
        let decision = state.decision("abc12345", &record, "v", "abc12345.mp4");
        assert_eq!(decision.kind, RouteKind::Uploading);
        assert!(decision.target.is_none());
    }

    #[test]
    fn local_files_go_to_home_node() {
        let state = AppState::new(config());
        let record = ResolveRecord {
            status: "ready".to_string(),
            storage_host: None,
            tier: None,
            r2_url: None,
            r2_key: None,
            expires_at: None,
        };
        let decision = state.decision("abc12345", &record, "f", "abc12345.mp4");
        assert_eq!(decision.kind, RouteKind::Local);
        assert_eq!(
            decision.target.as_deref(),
            Some("https://f.example/f/abc12345.mp4")
        );
    }

    #[test]
    fn remote_and_r2_targets() {
        let state = AppState::new(config());
        let remote = ResolveRecord {
            status: "ready".to_string(),
            storage_host: Some("https://f-us1.example/".to_string()),
            tier: None,
            r2_url: None,
            r2_key: None,
            expires_at: None,
        };
        let decision = state.decision("abc12345", &remote, "d", "abc12345.zip");
        assert_eq!(decision.kind, RouteKind::Remote);
        assert_eq!(
            decision.target.as_deref(),
            Some("https://f-us1.example/d/abc12345.zip")
        );

        let hot = ResolveRecord {
            r2_url: Some("https://r.example/hot/abc12345.zip".to_string()),
            ..remote
        };
        let decision = state.decision("abc12345", &hot, "v", "abc12345.zip");
        assert_eq!(decision.kind, RouteKind::R2);
        assert_eq!(
            decision.target.as_deref(),
            Some("https://r.example/hot/abc12345.zip")
        );
    }
}
