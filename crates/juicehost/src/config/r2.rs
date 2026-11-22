use crate::config::{ConfigError, R2File, secret::SecretString};

/// Missing bucket or credentials disables the hot tier.
#[derive(Debug)]
pub struct R2Settings {
    bucket: Option<String>,
    endpoint: Option<String>,
    /// R2 region; `auto` is the correct value for Cloudflare.
    region: String,
    access_key: Option<SecretString>,
    secret_key: Option<SecretString>,
}

impl R2Settings {
    #[must_use]
    pub fn bucket(&self) -> Option<&str> {
        self.bucket.as_deref()
    }

    #[must_use]
    pub fn endpoint(&self) -> Option<&str> {
        self.endpoint.as_deref()
    }

    #[must_use]
    pub fn region(&self) -> &str {
        &self.region
    }

    #[must_use]
    pub fn access_key(&self) -> Option<&str> {
        self.access_key.as_deref()
    }

    #[must_use]
    pub fn secret_key(&self) -> Option<&str> {
        self.secret_key.as_deref()
    }
}

impl R2Settings {
    pub fn load(file: &R2File) -> Result<Self, ConfigError> {
        let access_key =
            juiceutils::config::optional_secret("R2_ACCESS_KEY").map(SecretString::from);
        let secret_key =
            juiceutils::config::optional_secret("R2_SECRET_KEY").map(SecretString::from);
        Ok(Self {
            bucket: file.bucket.clone(),
            endpoint: file.endpoint.clone(),
            region: file.region.clone(),
            access_key,
            secret_key,
        })
    }
}
