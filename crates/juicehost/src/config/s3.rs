use crate::config::{ConfigError, S3File};

/// S3-compatible backend settings.
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
    pub const fn bucket(&self) -> Option<&String> {
        self.bucket.as_ref()
    }

    pub const fn region(&self) -> Option<&String> {
        self.region.as_ref()
    }

    pub const fn endpoint(&self) -> Option<&String> {
        self.endpoint.as_ref()
    }

    pub const fn allow_http(&self) -> bool {
        self.allow_http
    }

    pub const fn access_key(&self) -> Option<&String> {
        self.access_key.as_ref()
    }

    pub const fn secret_key(&self) -> Option<&String> {
        self.secret_key.as_ref()
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
