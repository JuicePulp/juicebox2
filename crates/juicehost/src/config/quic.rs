use std::path::PathBuf;

use crate::config::{ConfigError, QuicFile, public::PublicSettings};

/// QUIC/HTTP/3 listener and limit settings.
#[derive(Debug)]
pub struct QuicSettings {
    host: String,
    port: u16,
    cert_path: PathBuf,
    max_connections: usize,
    max_requests: usize,
    handshake_seconds: u64,
    idle_seconds: u64,
    request_total_seconds: u64,
}

impl QuicSettings {
    pub fn host(&self) -> &str {
        &self.host
    }

    pub const fn port(&self) -> u16 {
        self.port
    }

    pub const fn cert_path(&self) -> &PathBuf {
        &self.cert_path
    }

    pub const fn max_connections(&self) -> usize {
        self.max_connections
    }

    pub const fn max_requests(&self) -> usize {
        self.max_requests
    }

    pub const fn handshake_seconds(&self) -> u64 {
        self.handshake_seconds
    }

    pub const fn idle_seconds(&self) -> u64 {
        self.idle_seconds
    }

    pub const fn request_total_seconds(&self) -> u64 {
        self.request_total_seconds
    }

    pub fn load(file: &QuicFile, public: &PublicSettings) -> Result<Self, ConfigError> {
        let host = file
            .host
            .clone()
            .unwrap_or_else(|| public.host().to_owned());
        let port = file.port.unwrap_or(
            public
                .port()
                .checked_add(1)
                .ok_or(ConfigError::MissingQuicPort)?,
        );
        let cert_path = file.cert_path.clone();
        let max_connections = file.max_connections;
        let max_requests = file.max_requests;
        let handshake_seconds = file.handshake_seconds;
        let idle_seconds = file.idle_seconds;
        let request_total_seconds = file.request_total_seconds;
        Ok(Self {
            host,
            port,
            cert_path,
            max_connections,
            max_requests,
            handshake_seconds,
            idle_seconds,
            request_total_seconds,
        })
    }
}
