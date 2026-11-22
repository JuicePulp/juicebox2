use std::ops::Deref;

use crate::config::{ConfigError, security::SecuritySettings};

/// A credential that never prints its value through `Debug`.
#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct SecretString(String);

impl SecretString {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretString(<redacted>)")
    }
}

impl Deref for SecretString {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl AsRef<str> for SecretString {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl From<String> for SecretString {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for SecretString {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

/// The ticket signing key is guaranteed non-empty and non-placeholder.
#[derive(Debug)]
pub struct SecretSettings {
    ticket_jwt_secret: SecretString,
    ip_pepper: SecretString,
}

impl SecretSettings {
    #[must_use]
    pub fn ticket_jwt_secret(&self) -> &str {
        self.ticket_jwt_secret.as_str()
    }

    #[must_use]
    pub fn ip_pepper(&self) -> &str {
        self.ip_pepper.as_str()
    }

    /// Tries `TICKET_JWT_SECRET`, `JWT_SECRET`, then the API key; fails if none
    /// is usable.
    pub fn load(security: &SecuritySettings) -> Result<Self, ConfigError> {
        let ticket_jwt_secret = juiceutils::config::ticket_secret(security.api_key())
            .map_err(|e| ConfigError::InvalidSecret(e.to_string()))?;
        let ip_pepper = juiceutils::config::optional_secret("IP_PEPPER").unwrap_or_default();
        Ok(Self {
            ticket_jwt_secret: SecretString::from(ticket_jwt_secret),
            ip_pepper: SecretString::from(ip_pepper),
        })
    }
}
