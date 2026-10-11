use crate::config::{ConfigError, R2File};

#[derive(Debug)]
pub struct R2Settings {
    bucket: Option<String>,
    endpoint: Option<String>,
    region: String,
    access_key: Option<String>,
    secret_key: Option<String>,
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
        let access_key = juiceutils::config::optional_secret("R2_ACCESS_KEY");
        let secret_key = juiceutils::config::optional_secret("R2_SECRET_KEY");
        Ok(Self {
            bucket: file.bucket.clone(),
            endpoint: file.endpoint.clone(),
            region: file.region.clone(),
            access_key,
            secret_key,
        })
    }
}
