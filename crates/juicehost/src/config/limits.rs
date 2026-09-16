use crate::config::{ConfigError, LimitsFile, bounded_env};

/// Concurrency and timeout limit settings.
#[derive(Debug)]
pub struct LimitsSettings {
    min_free_space_bytes: u64,
    max_file_size_bytes: u64,
    max_range_response_bytes: u64,
    max_concurrent_uploads: usize,
    max_concurrent_downloads: usize,
    max_concat_parts: usize,
    tcp_body_inactivity_seconds: u64,
    tcp_request_total_seconds: u64,
    tcp_max_concurrent_requests: usize,
}

impl LimitsSettings {
    pub const fn min_free_space_bytes(&self) -> u64 {
        self.min_free_space_bytes
    }

    pub const fn max_file_size_bytes(&self) -> u64 {
        self.max_file_size_bytes
    }

    pub const fn max_range_response_bytes(&self) -> u64 {
        self.max_range_response_bytes
    }

    pub const fn max_concurrent_uploads(&self) -> usize {
        self.max_concurrent_uploads
    }

    pub const fn max_concurrent_downloads(&self) -> usize {
        self.max_concurrent_downloads
    }

    pub const fn max_concat_parts(&self) -> usize {
        self.max_concat_parts
    }

    pub const fn tcp_body_inactivity_seconds(&self) -> u64 {
        self.tcp_body_inactivity_seconds
    }

    pub const fn tcp_request_total_seconds(&self) -> u64 {
        self.tcp_request_total_seconds
    }

    pub const fn tcp_max_concurrent_requests(&self) -> usize {
        self.tcp_max_concurrent_requests
    }

    // I KNOW THERE'S A BETTER WAY TO DO THIS DON'T BLAME ME FOR THIS.
    pub fn load(file: &LimitsFile) -> Result<Self, ConfigError> {
        let min_free_space_bytes = std::env::var("MIN_FREE_SPACE_GB")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(file.min_free_space_gb)
            * 1024
            * 1024
            * 1024;
        let max_file_size_bytes =
            bounded_env("MAX_FILE_SIZE_MB", file.max_file_size_mb, 1, 1024 * 1024)? * 1024 * 1024;
        let max_range_response_bytes =
            bounded_env("MAX_RANGE_RESPONSE_MB", file.max_range_response_mb, 1, 1024)?
                * 1024
                * 1024;
        let max_concurrent_uploads = bounded_env(
            "MAX_CONCURRENT_UPLOADS",
            file.max_concurrent_uploads,
            1,
            4096,
        )?;
        let max_concurrent_downloads = bounded_env(
            "MAX_CONCURRENT_DOWNLOADS",
            file.max_concurrent_downloads,
            1,
            4096,
        )?;
        let max_concat_parts = bounded_env("MAX_CONCAT_PARTS", file.max_concat_parts, 1, 4096)?;
        let tcp_body_inactivity_seconds = bounded_env(
            "TCP_BODY_INACTIVITY_SECONDS",
            file.tcp_body_inactivity_seconds,
            1,
            3600,
        )?;
        let tcp_request_total_seconds = bounded_env(
            "TCP_REQUEST_TOTAL_SECONDS",
            file.tcp_request_total_seconds,
            1,
            86_400,
        )?;
        let tcp_max_concurrent_requests = bounded_env(
            "TCP_MAX_CONCURRENT_REQUESTS",
            file.tcp_max_concurrent_requests,
            1,
            65_536,
        )?;
        Ok(Self {
            min_free_space_bytes,
            max_file_size_bytes,
            max_range_response_bytes,
            max_concurrent_uploads,
            max_concurrent_downloads,
            max_concat_parts,
            tcp_body_inactivity_seconds,
            tcp_request_total_seconds,
            tcp_max_concurrent_requests,
        })
    }
}
