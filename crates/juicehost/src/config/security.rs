use juiceutils::{
    file_validation::ProtectionLevel,
    proxy::{self, parse_trusted_proxy_cidrs},
};

use crate::config::{ConfigError, DirectorySettings, SecurityFile};

/// Internal API authentication, origin, and validation settings.
#[derive(Debug)]
pub struct SecuritySettings {
    api_key: String,
    allowed_origins: Vec<String>,
    danger_level: ProtectionLevel,
    trusted_proxy_cidrs: Vec<proxy::IpCidr>,
}

impl SecuritySettings {
    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    pub fn allowed_origins(&self) -> &[String] {
        &self.allowed_origins
    }

    pub const fn danger_level(&self) -> ProtectionLevel {
        self.danger_level
    }

    pub fn trusted_proxy_cidrs(&self) -> &[proxy::IpCidr] {
        &self.trusted_proxy_cidrs
    }

    pub fn load(file: &SecurityFile, directories: &DirectorySettings) -> Result<Self, ConfigError> {
        let api_key = juicebox_config::optional_secret("JUICEHOST_API_KEY").unwrap_or_default();

        let allowed_origins = std::env::var("ALLOWED_ORIGINS")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map_or_else(
                || {
                    file.allowed_origins.clone().unwrap_or_else(|| {
                        directories
                            .backend_url()
                            .map(|b| vec![b.clone()])
                            .unwrap_or_default()
                    })
                },
                |s| {
                    s.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                },
            );

        let danger_level = ProtectionLevel::parse(
            &std::env::var("DANGER_LEVEL").unwrap_or_else(|_| file.danger_level.clone()),
        );
        let trusted_proxy_cidrs = parse_trusted_proxy_cidrs(
            &std::env::var("TRUSTED_PROXY_CIDRS")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .or_else(|| file.trusted_proxy_cidrs.clone())
                .unwrap_or_default(),
        )
        .map_err(ConfigError::InvalidTrustedProxyCidrs)?;
        Ok(Self {
            api_key,
            allowed_origins,
            danger_level,
            trusted_proxy_cidrs,
        })
    }
}
