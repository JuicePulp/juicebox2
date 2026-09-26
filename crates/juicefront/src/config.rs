use std::path::PathBuf;

use juiceutils::{
    config::{SentrySettings, load_toml_or_default, optional_secret},
    proxy::{IpCidr, parse_trusted_proxy_cidrs},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct FileConfig {
    #[serde(default)]
    pub ui: UiFile,
    #[serde(default)]
    pub proxy: ProxyFile,
    #[serde(default)]
    pub sentry: SentrySettings,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct UiFile {
    #[serde(default = "default_ui_host")]
    pub host: String,
    #[serde(default = "default_ui_port")]
    pub port: u16,
}

impl Default for UiFile {
    fn default() -> Self {
        Self {
            host: default_ui_host(),
            port: default_ui_port(),
        }
    }
}

fn default_ui_host() -> String {
    "0.0.0.0".into()
}

const fn default_ui_port() -> u16 {
    6400
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ProxyFile {
    #[serde(default = "default_juiceback_url")]
    pub juiceback_url: String,
    #[serde(default = "default_juicehost_url")]
    pub juicehost_url: String,
    #[serde(default = "default_api_internal_url")]
    pub api_internal_url: String,
    #[serde(default)]
    pub trusted_proxy_cidrs: String,
}

impl Default for ProxyFile {
    fn default() -> Self {
        Self {
            juiceback_url: default_juiceback_url(),
            juicehost_url: default_juicehost_url(),
            api_internal_url: default_api_internal_url(),
            trusted_proxy_cidrs: String::new(),
        }
    }
}

fn default_juiceback_url() -> String {
    "http://127.0.0.1:6401".into()
}

fn default_juicehost_url() -> String {
    "http://127.0.0.1:6402".into()
}

fn default_api_internal_url() -> String {
    default_juiceback_url()
}

#[derive(Debug, Clone)]
pub struct Config {
    pub ui_host: String,
    pub ui_port: u16,
    pub juiceback_url: String,
    pub juicehost_url: String,
    pub api_internal_url: String,
    pub trusted_proxies: Vec<IpCidr>,
    pub sentry_environment: String,
}

impl Config {
    pub fn load() -> Self {
        let file = load_file_config();
        let proxy = &file.proxy;

        let juiceback_url = optional_secret("JUICEBACK_URL")
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| proxy.juiceback_url.clone());
        let juicehost_url = optional_secret("JUICEHOST_URL")
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| proxy.juicehost_url.clone());
        let api_internal_url = optional_secret("API_INTERNAL_URL")
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| proxy.api_internal_url.clone());
        let cidrs = optional_secret("TRUSTED_PROXY_CIDRS")
            .unwrap_or_else(|| proxy.trusted_proxy_cidrs.clone());
        let trusted_proxies = match parse_trusted_proxy_cidrs(&cidrs) {
            Ok(list) => list,
            Err(err) => {
                tracing::warn!("invalid trusted_proxy_cidrs ({err}), trusting none");
                Vec::new()
            }
        };

        Self {
            ui_host: file.ui.host,
            ui_port: file.ui.port,
            juiceback_url,
            juicehost_url,
            api_internal_url,
            trusted_proxies,
            sentry_environment: file.sentry.environment,
        }
    }

    pub fn upstream_host_port(base_url: &str) -> Option<(String, u16)> {
        let without_scheme = base_url.split("://").nth(1).unwrap_or(base_url);
        let authority = without_scheme.split('/').next().unwrap_or(without_scheme);
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (host, port.parse::<u16>().ok()?),
            None => (authority, 80),
        };
        if host.is_empty() {
            return None;
        }
        Some((host.to_owned(), port))
    }
}

fn candidate_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(path) = optional_secret("JUICEFRONT_CONFIG") {
        paths.push(PathBuf::from(path));
    }
    for name in ["juicefront.toml", "config.toml"] {
        paths.push(PathBuf::from(name));
        paths.push(PathBuf::from("/etc/juicebox").join(name));
    }
    paths
}

fn load_file_config() -> FileConfig {
    for path in candidate_paths() {
        if path.exists() {
            return load_toml_or_default(&path);
        }
    }
    tracing::warn!("no juicefront config file found, using defaults");
    FileConfig::default()
}
