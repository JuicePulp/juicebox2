use juiceutils::{
    config::{SentrySettings, optional_secret},
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
    juiceutils::config::ports::FRONTEND
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
    #[serde(default)]
    pub router_url: String,
}

impl Default for ProxyFile {
    fn default() -> Self {
        Self {
            juiceback_url: default_juiceback_url(),
            juicehost_url: default_juicehost_url(),
            api_internal_url: default_api_internal_url(),
            trusted_proxy_cidrs: String::new(),
            router_url: String::new(),
        }
    }
}

fn default_juiceback_url() -> String {
    format!("http://127.0.0.1:{}", juiceutils::config::ports::BACKEND)
}

fn default_juicehost_url() -> String {
    format!("http://127.0.0.1:{}", juiceutils::config::ports::HOST)
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
    pub router_url: Option<String>,
    pub trusted_proxies: Vec<IpCidr>,
    pub sentry_environment: String,
}

impl Config {
    pub fn load() -> Self {
        let file: FileConfig = juiceutils::config::load_service_config(
            "juicefront",
            "JUICEFRONT_CONFIG",
            &["juicefront.toml", "config.toml"],
        );
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
        let router_url = optional_secret("JUICEROUTER_URL")
            .filter(|v| !v.is_empty())
            .or_else(|| (!proxy.router_url.is_empty()).then(|| proxy.router_url.clone()));
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
            router_url,
            trusted_proxies,
            sentry_environment: SentrySettings::from_env_or(&file.sentry).environment,
        }
    }

    pub fn upstream_host_port(base_url: &str) -> Option<(String, u16)> {
        let candidate = if base_url.contains("://") {
            base_url.to_string()
        } else {
            format!("http://{base_url}")
        };
        let parsed = url::Url::parse(&candidate).ok()?;
        let host = match parsed.host()? {
            url::Host::Domain(domain) => domain.to_owned(),
            url::Host::Ipv4(v4) => v4.to_string(),
            url::Host::Ipv6(v6) => v6.to_string(),
        };
        let port = parsed.port_or_known_default()?;
        Some((host, port))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_host_port_uses_scheme_default_and_ipv6() {
        assert_eq!(
            Config::upstream_host_port("https://box.juicey.dev"),
            Some(("box.juicey.dev".to_owned(), 443))
        );
        assert_eq!(
            Config::upstream_host_port("http://juiceback:6401"),
            Some(("juiceback".to_owned(), 6401))
        );
        assert_eq!(
            Config::upstream_host_port("juiceback:6401"),
            Some(("juiceback".to_owned(), 6401))
        );
        assert_eq!(
            Config::upstream_host_port("https://[::1]:6401"),
            Some(("::1".to_owned(), 6401))
        );
        assert_eq!(Config::upstream_host_port("not a url"), None);
    }
}
