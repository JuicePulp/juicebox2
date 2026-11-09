use crate::config::{ConfigError, security::SecuritySettings};

/// The ticket signing key is guaranteed non-empty and non-placeholder.
#[derive(Debug)]
pub struct SecretSettings {
    ticket_jwt_secret: String,
    ip_pepper: String,
}

impl SecretSettings {
    #[must_use]
    pub fn ticket_jwt_secret(&self) -> &str {
        &self.ticket_jwt_secret
    }

    #[must_use]
    pub fn ip_pepper(&self) -> &str {
        &self.ip_pepper
    }

    /// Tries `TICKET_JWT_SECRET`, `JWT_SECRET`, then the API key; fails if none
    /// is usable.
    pub fn load(security: &SecuritySettings) -> Result<Self, ConfigError> {
        let ticket_jwt_secret = juiceutils::config::ticket_secret(security.api_key())
            .map_err(|e| ConfigError::InvalidSecret(e.to_string()))?;
        let ip_pepper = juiceutils::config::optional_secret("IP_PEPPER").unwrap_or_default();
        Ok(Self {
            ticket_jwt_secret,
            ip_pepper,
        })
    }
}
