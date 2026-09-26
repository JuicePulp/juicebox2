use crate::config::{ConfigError, PublicFile};

#[derive(Debug)]
pub struct PublicSettings {
    host: String,
    port: u16,
}

impl PublicSettings {
    pub const DEFAULT_IP: &str = "127.0.0.1";

    pub const DEFAULT_PORT: u16 = 6402;

    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    #[must_use]
    pub const fn port(&self) -> u16 {
        self.port
    }

    pub fn load(file: &PublicFile) -> Result<Self, ConfigError> {
        let host = file.host.clone();
        let port = file.port;

        Ok(Self { host, port })
    }
}
