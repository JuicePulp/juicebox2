use crate::config::security::SecuritySettings;

/// Signing secrets and ban pepper settings.
#[derive(Debug)]
pub struct SecretSettings {
    ticket_jwt_secret: String,
    ip_pepper: String,
}

impl SecretSettings {
    pub fn ticket_jwt_secret(&self) -> &str {
        &self.ticket_jwt_secret
    }

    pub fn ip_pepper(&self) -> &str {
        &self.ip_pepper
    }

    pub fn load(security: &SecuritySettings) -> Self {
        let ticket_jwt_secret = juicebox_config::optional_secret("TICKET_JWT_SECRET")
            .or_else(|| juicebox_config::optional_secret("JWT_SECRET"))
            .unwrap_or_else(|| security.api_key().to_owned());
        let ip_pepper = juicebox_config::optional_secret("IP_PEPPER").unwrap_or_default();
        Self {
            ticket_jwt_secret,
            ip_pepper,
        }
    }
}
