use crate::config::{ConfigError, security::SecuritySettings};

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

    pub fn load(security: &SecuritySettings) -> Result<Self, ConfigError> {
        let ticket_jwt_secret = juiceutils::config::optional_secret("TICKET_JWT_SECRET")
            .or_else(|| juiceutils::config::optional_secret("JWT_SECRET"))
            .filter(|s| !s.is_empty())
            .or_else(|| {
                let key = security.api_key().trim().to_owned();
                (!key.is_empty()).then_some(key)
            })
            .ok_or(ConfigError::MissingSecret {
                name: "TICKET_JWT_SECRET",
            })?;
        let ip_pepper = juiceutils::config::optional_secret("IP_PEPPER").unwrap_or_default();
        Ok(Self {
            ticket_jwt_secret,
            ip_pepper,
        })
    }
}
