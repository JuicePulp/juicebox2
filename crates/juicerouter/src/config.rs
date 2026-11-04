use std::{collections::HashMap, path::Path};

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub server: ServerSettings,
    #[serde(default)]
    pub router: RouterSettings,
    #[serde(default)]
    pub regions: RegionSettings,
    #[serde(default)]
    pub geo: GeoSettings,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: ServerSettings::default(),
            router: RouterSettings::default(),
            regions: RegionSettings::default(),
            geo: GeoSettings::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerSettings {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
        }
    }
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u16 {
    6404
}

#[derive(Debug, Clone, Deserialize)]
pub struct RouterSettings {
    /// Central juiceback used for `/internal/resolve/{id}`.
    #[serde(default = "default_juiceback_url")]
    pub juiceback_url: String,
    /// Public URL of this router, used only for display/links.
    #[serde(default = "default_public_base_url")]
    pub public_base_url: String,
    /// Node that owns files with no `storage_host` (the local juicehost).
    #[serde(default = "default_home_url")]
    pub home_url: String,
    /// Sent as `x-juiceback-origin`; storage nodes reject requests without a
    /// matching origin when they have `[security] allowed_origins` set.
    #[serde(default = "default_juiceback_origin")]
    pub juiceback_origin: String,
    /// API key for `/internal/*`. Prefer the JUICEHOST_API_KEY env var.
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default = "default_backend_timeout_ms")]
    pub backend_timeout_ms: u64,
    #[serde(default = "default_resolve_cache_secs")]
    pub resolve_cache_secs: u64,
    #[serde(default = "default_probe_interval_secs")]
    pub probe_interval_secs: u64,
    #[serde(default = "default_probe_timeout_ms")]
    pub probe_timeout_ms: u64,
    /// What to do with files that have no `storage_host` (stored on the home
    /// node). `redirect` is correct when the router has its own hostname;
    /// `proxy` is correct when the router fronts `home_url` directly.
    #[serde(default)]
    pub local_mode: LocalMode,
    /// Internal juicehost address used by `local_mode = "proxy"`.
    #[serde(default = "default_local_upstream")]
    pub local_upstream: String,
}

impl Default for RouterSettings {
    fn default() -> Self {
        Self {
            juiceback_url: default_juiceback_url(),
            public_base_url: default_public_base_url(),
            home_url: default_home_url(),
            juiceback_origin: default_juiceback_origin(),
            api_key: None,
            backend_timeout_ms: default_backend_timeout_ms(),
            resolve_cache_secs: default_resolve_cache_secs(),
            probe_interval_secs: default_probe_interval_secs(),
            probe_timeout_ms: default_probe_timeout_ms(),
            local_mode: LocalMode::Redirect,
            local_upstream: default_local_upstream(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalMode {
    #[default]
    Redirect,
    Proxy,
}

fn default_local_upstream() -> String {
    "http://127.0.0.1:6402".to_string()
}

fn default_juiceback_url() -> String {
    "http://127.0.0.1:6401".to_string()
}

fn default_public_base_url() -> String {
    "https://r.juicey.dev".to_string()
}

fn default_home_url() -> String {
    "https://f.juicey.dev".to_string()
}

fn default_juiceback_origin() -> String {
    "https://box.juicey.dev".to_string()
}

fn default_backend_timeout_ms() -> u64 {
    2_000
}

fn default_resolve_cache_secs() -> u64 {
    30
}

fn default_probe_interval_secs() -> u64 {
    30
}

fn default_probe_timeout_ms() -> u64 {
    5_000
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RegionSettings {
    /// Same shape as juiceback's `[regions.public_juicehosts]`.
    #[serde(default)]
    pub public_juicehosts: HashMap<String, String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct GeoSettings {
    /// Used when the country/colo is unknown or maps to no configured node.
    #[serde(default)]
    pub default_region: Option<String>,
    /// Cloudflare `CF-IPCountry` → region key (ISO-3166 alpha-2, uppercase).
    #[serde(default)]
    pub countries: HashMap<String, String>,
    /// Cloudflare `CF-Ray` colo code (first three letters) → region key.
    #[serde(default)]
    pub colos: HashMap<String, String>,
}

impl Config {
    /// Load the TOML file, falling back to defaults on any error.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            tracing::warn!("config file {} not found, using defaults", path.display());
            return Self::default();
        };
        match toml::from_str(&text) {
            Ok(config) => config,
            Err(error) => {
                tracing::error!("config file {} invalid: {error}", path.display());
                Self::default()
            }
        }
    }

    /// Environment overrides for secrets only.
    #[must_use]
    pub fn with_env(mut self) -> Self {
        if let Ok(value) = std::env::var("JUICEROUTER_API_KEY")
            && !value.trim().is_empty()
        {
            self.router.api_key = Some(value.trim().to_string());
        }
        if let Ok(value) = std::env::var("JUICEBACK_URL")
            && !value.trim().is_empty()
        {
            self.router.juiceback_url = value.trim().to_string();
        }
        self
    }

    #[must_use]
    pub fn api_key(&self) -> String {
        self.router
            .api_key
            .clone()
            .or_else(|| std::env::var("JUICEHOST_API_KEY").ok())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_regions_and_geo() {
        let toml = r#"
[router]
juiceback_url = "http://127.0.0.1:6401"

[regions.public_juicehosts]
eu1 = "https://f-eu1.example"
us1 = "https://f-us1.example"

[geo]
default_region = "eu1"

[geo.countries]
US = "us1"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(
            config
                .regions
                .public_juicehosts
                .get("us1")
                .map(String::as_str),
            Some("https://f-us1.example")
        );
        assert_eq!(config.geo.default_region.as_deref(), Some("eu1"));
        assert_eq!(
            config.geo.countries.get("US").map(String::as_str),
            Some("us1")
        );
        assert_eq!(config.server.port, 6404);
    }
}
