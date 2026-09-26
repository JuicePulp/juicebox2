use crate::config::{ConfigError, LimitsFile};

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
    #[must_use]
    pub const fn min_free_space_bytes(&self) -> u64 {
        self.min_free_space_bytes
    }

    #[must_use]
    pub const fn max_file_size_bytes(&self) -> u64 {
        self.max_file_size_bytes
    }

    #[must_use]
    pub const fn max_range_response_bytes(&self) -> u64 {
        self.max_range_response_bytes
    }

    #[must_use]
    pub const fn max_concurrent_uploads(&self) -> usize {
        self.max_concurrent_uploads
    }

    #[must_use]
    pub const fn max_concurrent_downloads(&self) -> usize {
        self.max_concurrent_downloads
    }

    #[must_use]
    pub const fn max_concat_parts(&self) -> usize {
        self.max_concat_parts
    }

    #[must_use]
    pub const fn tcp_body_inactivity_seconds(&self) -> u64 {
        self.tcp_body_inactivity_seconds
    }

    #[must_use]
    pub const fn tcp_request_total_seconds(&self) -> u64 {
        self.tcp_request_total_seconds
    }

    #[must_use]
    pub const fn tcp_max_concurrent_requests(&self) -> usize {
        self.tcp_max_concurrent_requests
    }

    pub fn load(file: &LimitsFile) -> Result<Self, ConfigError> {
        let min_free_space_bytes = file.min_free_space_gb * 1024 * 1024 * 1024;
        let max_file_size_bytes = file.max_file_size_mb * 1024 * 1024;
        let max_range_response_bytes = file.max_range_response_mb * 1024 * 1024;
        let max_concurrent_uploads = file.max_concurrent_uploads;
        let max_concurrent_downloads = file.max_concurrent_downloads;
        let max_concat_parts = file.max_concat_parts;
        let tcp_body_inactivity_seconds = file.tcp_body_inactivity_seconds;
        let tcp_request_total_seconds = file.tcp_request_total_seconds;
        let tcp_max_concurrent_requests = file.tcp_max_concurrent_requests;
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
