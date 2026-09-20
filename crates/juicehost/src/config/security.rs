use juiceutils::{
    file_validation::ProtectionLevel,
    proxy::{self, parse_trusted_proxy_cidrs},
};

use crate::config::{ConfigError, DirectorySettings, SecurityFile};

/// Internal API authentication, origin, and validation settings.
#[derive(Debug)]
pub struct SecuritySettings {
    api_key: String,
    allow_no_auth: bool,
    allowed_origins: Vec<String>,
    danger_level: ProtectionLevel,
    trusted_proxy_cidrs: Vec<proxy::IpCidr>,
}

impl SecuritySettings {
    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    pub const fn allow_no_auth(&self) -> bool {
        self.allow_no_auth
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
        let api_key = juiceutils::config::optional_secret("JUICEHOST_API_KEY").unwrap_or_default();
        // Explicit opt-out for running without an API key (default false).
        let allow_no_auth = file.allow_no_auth;

        let allowed_origins = file.allowed_origins.clone().unwrap_or_else(|| {
            directories
                .backend_url()
                .map(|b| vec![b.clone()])
                .unwrap_or_default()
        });

        let danger_level = ProtectionLevel::parse(&file.danger_level);
        let trusted_proxy_cidrs =
            parse_trusted_proxy_cidrs(&file.trusted_proxy_cidrs.clone().unwrap_or_default())
                .map_err(ConfigError::InvalidTrustedProxyCidrs)?;
        Ok(Self {
            api_key,
            allow_no_auth,
            allowed_origins,
            danger_level,
            trusted_proxy_cidrs,
        })
    }
}
