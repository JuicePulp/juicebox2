use crate::config::{ConfigError, S3File};

#[derive(Debug)]
pub struct S3Settings {
    bucket: Option<String>,
    region: Option<String>,
    endpoint: Option<String>,
    allow_http: bool,
    access_key: Option<String>,
    secret_key: Option<String>,
}

impl S3Settings {
    #[must_use]
    pub fn bucket(&self) -> Option<&str> {
        self.bucket.as_deref()
    }

    #[must_use]
    pub fn region(&self) -> Option<&str> {
        self.region.as_deref()
    }

    #[must_use]
    pub fn endpoint(&self) -> Option<&str> {
        self.endpoint.as_deref()
    }

    #[must_use]
    pub const fn allow_http(&self) -> bool {
        self.allow_http
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

impl S3Settings {
    pub fn load(file: &S3File) -> Result<Self, ConfigError> {
        let bucket = file.bucket.clone();
        let region = file.region.clone();
        let endpoint = file.endpoint.clone();
        let allow_http = file.allow_http;
        let access_key = juiceutils::config::optional_secret("S3_ACCESS_KEY");
        let secret_key = juiceutils::config::optional_secret("S3_SECRET_KEY");
        Ok(Self {
            bucket,
            region,
            endpoint,
            allow_http,
            access_key,
            secret_key,
        })
    }
}
