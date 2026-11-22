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
    #[serde(default = "default_juiceback_url")]
    pub juiceback_url: String,
    #[serde(default = "default_public_base_url")]
    pub public_base_url: String,
    #[serde(default = "default_home_url")]
    pub home_url: String,
    #[serde(default = "default_juiceback_origin")]
    pub juiceback_origin: String,
    /// Browser origins allowed to call the routing/status endpoints.
    #[serde(default)]
    pub allowed_origins: Vec<String>,
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
    #[serde(default)]
    pub local_mode: LocalMode,
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
            allowed_origins: Vec::new(),
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
    "https://f-r.juicey.dev".to_string()
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
    #[serde(default)]
    pub public_juicehosts: HashMap<String, String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct GeoSettings {
    #[serde(default)]
    pub default_region: Option<String>,
    #[serde(default)]
    pub countries: HashMap<String, String>,
    #[serde(default)]
    pub colos: HashMap<String, String>,
}

impl Config {
    /// Loads a TOML config. A missing file is documented to use defaults;
    /// unreadable, malformed or invalid files are fatal so the router never
    /// silently starts with defaults (and an empty API key) on a bad deploy.
    ///
    /// # Errors
    ///
    /// Returns a description when the file cannot be read, cannot be parsed,
    /// or contains invalid URLs or non-positive timeouts/ports.
    pub fn load(path: &Path) -> Result<Self, String> {
        let config = match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str::<Self>(&text)
                .map_err(|error| format!("config file {} invalid: {error}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                tracing::info!("config: {} not found, using defaults", path.display());
                Self::default()
            }
            Err(error) => {
                return Err(format!(
                    "config file {} unreadable: {error}",
                    path.display()
                ));
            }
        };
        config.validate()?;
        Ok(config)
    }

    /// Rejects values that would leave routing or health probes misconfigured.
    ///
    /// # Errors
    ///
    /// Returns a description of the first invalid setting.
    fn validate(&self) -> Result<(), String> {
        if self.server.port == 0 {
            return Err("server.port must be non-zero".to_owned());
        }
        for (label, value, allow_empty) in [
            ("juiceback_url", self.router.juiceback_url.as_str(), false),
            (
                "public_base_url",
                self.router.public_base_url.as_str(),
                true,
            ),
            ("home_url", self.router.home_url.as_str(), true),
            (
                "juiceback_origin",
                self.router.juiceback_origin.as_str(),
                false,
            ),
            ("local_upstream", self.router.local_upstream.as_str(), false),
        ] {
            if value.trim().is_empty() {
                if allow_empty {
                    continue;
                }
                return Err(format!("router.{label} must not be empty"));
            }
            let parsed = url::Url::parse(value)
                .map_err(|error| format!("router.{label} is invalid: {error}"))?;
            if !matches!(parsed.scheme(), "http" | "https") || parsed.host().is_none() {
                return Err(format!("router.{label} must be an absolute http(s) URL"));
            }
        }
        if self.router.backend_timeout_ms == 0
            || self.router.resolve_cache_secs == 0
            || self.router.probe_interval_secs == 0
            || self.router.probe_timeout_ms == 0
        {
            return Err("router timeouts and resolve_cache_secs must be positive".to_owned());
        }
        Ok(())
    }

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

    /// Falls back to `JUICEHOST_API_KEY` so a host install can share its key.
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

    #[test]
    fn missing_config_uses_defaults() {
        let path =
            std::env::temp_dir().join(format!("juicerouter-missing-{}.toml", std::process::id()));
        let _ = std::fs::remove_file(&path);
        assert_eq!(Config::load(&path).unwrap().server.port, 6404);
    }

    #[test]
    fn malformed_config_is_an_error() {
        let path =
            std::env::temp_dir().join(format!("juicerouter-invalid-{}.toml", std::process::id()));
        std::fs::write(&path, "[router]\njuiceback_url = 42\n").unwrap();
        let error = Config::load(&path).unwrap_err();
        assert!(error.contains("invalid"), "{error}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn invalid_values_are_rejected() {
        let mut config = Config::default();
        config.router.backend_timeout_ms = 0;
        assert!(config.validate().is_err());
        config = Config::default();
        config.router.home_url = "ftp://example".to_owned();
        assert!(config.validate().is_err());
    }
}
