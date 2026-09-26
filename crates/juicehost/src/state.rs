use std::{sync::Arc, time::Duration};

use crate::{config::Config, storage::StorageBackend};

#[derive(Clone)]
pub struct AppState {
    pub storage: Arc<dyn StorageBackend>,

    pub api_key: String,

    pub allow_no_auth: bool,

    pub allowed_origins: Vec<String>,

    pub min_free_space_bytes: u64,

    pub max_file_size_bytes: u64,

    pub backend_url: Option<String>,

    pub frontend_url: Option<String>,

    pub danger_level: juiceutils::file_validation::ProtectionLevel,

    pub default_ttl_hours: f64,

    pub allowed_ttl_hours: Vec<f64>,

    pub quick_link: bool,

    pub custom_id: bool,
    pub file_cache_enabled: bool,
    pub file_cache_max_age_secs: u64,

    pub ticket_jwt_secret: String,

    pub ban_list: Arc<juiceutils::ban::BanList>,

    pub ban_list_file: Option<std::path::PathBuf>,

    pub ban_sync_url: Option<String>,

    pub ban_sync_interval: u64,
    pub trusted_proxy_cidrs: Vec<juiceutils::proxy::IpCidr>,
    pub max_range_response_bytes: u64,
    pub max_concat_parts: usize,
    pub tcp_body_inactivity: std::time::Duration,
    pub tcp_request_total: std::time::Duration,
    pub upload_semaphore: Arc<tokio::sync::Semaphore>,
    pub download_semaphore: Arc<tokio::sync::Semaphore>,

    pub backend_client: reqwest::Client,
}

impl AppState {
    #[must_use]
    pub fn new(config: &Config, storage: Arc<dyn StorageBackend>) -> Self {
        Self {
            storage,
            api_key: config.api_key.clone(),
            allow_no_auth: config.allow_no_auth,
            allowed_origins: config.allowed_origins.clone(),
            min_free_space_bytes: config.min_free_space_bytes,
            max_file_size_bytes: config.max_file_size_bytes,
            backend_url: config.backend_url.clone(),
            frontend_url: config.frontend_url.clone(),
            danger_level: config.danger_level,
            default_ttl_hours: config.default_ttl_hours,
            allowed_ttl_hours: config.allowed_ttl_hours.clone(),
            quick_link: config.quick_link,
            custom_id: config.custom_id,
            file_cache_enabled: config.file_cache_enabled,
            file_cache_max_age_secs: config.file_cache_max_age_secs,
            ticket_jwt_secret: config.ticket_jwt_secret.clone(),
            ban_list: Arc::new(juiceutils::ban::BanList::new(config.ip_pepper.clone())),
            ban_list_file: config.ban_list_file.clone(),
            ban_sync_url: config.ban_sync_url.clone(),
            ban_sync_interval: config.ban_sync_interval,
            trusted_proxy_cidrs: config.trusted_proxy_cidrs.clone(),
            max_range_response_bytes: config.max_range_response_bytes,
            max_concat_parts: config.max_concat_parts,
            tcp_body_inactivity: Duration::from_secs(config.tcp_body_inactivity_seconds),
            tcp_request_total: Duration::from_secs(config.tcp_request_total_seconds),
            upload_semaphore: Arc::new(tokio::sync::Semaphore::new(config.max_concurrent_uploads)),
            download_semaphore: Arc::new(tokio::sync::Semaphore::new(
                config.max_concurrent_downloads,
            )),
            backend_client: reqwest::Client::builder()
                .timeout(Duration::from_secs(2))
                .build()
                .expect("fixed backend HTTP client configuration must be valid"),
        }
    }
}
