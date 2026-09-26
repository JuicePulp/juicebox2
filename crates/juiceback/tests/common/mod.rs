#![allow(dead_code)]

use std::{net::SocketAddr, sync::Arc};

use axum::{
    body::Body,
    extract::connect_info::{ConnectInfo, MockConnectInfo},
    http::Request,
};
use juiceback::{
    config::Config, routes::build_router, state::AppState, storage_client::JuicehostConfig,
};

#[must_use]
pub fn mock_addr() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 0))
}

#[must_use]
pub fn with_connect_info(mut req: Request<Body>) -> Request<Body> {
    req.extensions_mut().insert(ConnectInfo(mock_addr()));
    req
}

#[must_use]
pub fn mock_router(state: Arc<AppState>) -> axum::Router {
    build_router(state).layer(MockConnectInfo(mock_addr()))
}

#[must_use]
pub fn test_config() -> Config {
    Config {
        host: "127.0.0.1".into(),
        port: 6401,
        quic_port: 6402,
        database_path: ":memory:".into(),
        rate_limit_per_minute: 100,
        db_pool_size: 1,
        max_concurrent_uploads: 16,
        public_base_url: "http://localhost:6402".into(),
        log_level: "info".into(),
        cleanup_interval_minutes: 30,
        juicehost_api_key: "test-key".into(),
        juicehost_url: "http://127.0.0.1:6402".into(),
        public_juicehost_url: "http://localhost:6402".into(),
        juiceback_origin: "http://127.0.0.1:6401".into(),
        jwt_secret: "test-secret".into(),
        cors_origins: vec!["http://localhost:6400".into()],
        report_webhook_url: None,
        smtp_host: None,
        smtp_port: None,
        smtp_username: None,
        smtp_password: None,
        report_email_recipient: None,
        report_email_sender: None,
        quic_cert_path: None,
        ip_encryption_key: "0000000000000000000000000000000000000000000000000000000000000001"
            .into(),
        ip_pepper: "test_pepper".into(),
        trusted_proxy_cidrs: vec![],
        report_retention_days: 90,
        feedback_retention_days: 90,
        cf_api_token: None,
        cf_zone_id: None,
        ticket_jwt_secret: "secret".into(),
        secure_cookies: false,
        sentry_environment: "test".into(),
        direct_upload_enabled: false,
        cobalt_enabled: false,
        cobalt_api_url: "http://localhost:7272".into(),
        cobalt_api_key: "cobalt_key".into(),
        cobalt_session_api_url: None,
        cobalt_session_api_key: None,
        fetch_empty_retry_delay_secs: 0,
        dte_enabled: false,
        dte_assumed_bps: juiceback::constants::DTE_ASSUMED_BPS,
        dte_safety_mult: juiceback::constants::DTE_SAFETY_MULT,
        dte_base_overhead_secs: juiceback::constants::DTE_BASE_OVERHEAD_SECS,
        dte_min_ttl_secs: juiceback::constants::DTE_MIN_TTL_SECS,
        dte_max_ttl_secs: juiceback::constants::DTE_MAX_TTL_SECS,
        dte_mint_limit: juiceback::constants::DTE_MINT_LIMIT,
        dte_mint_window_secs: juiceback::constants::DTE_MINT_WINDOW_SECS,
        dte_mint_burst: juiceback::constants::DTE_MINT_BURST,
        region_public_juicehosts: std::collections::HashMap::new(),
        allow_private_fetch: true,
    }
}

#[must_use]
pub fn test_config_with_proxies() -> Config {
    let mut config = test_config();
    config.trusted_proxy_cidrs =
        juiceutils::proxy::parse_trusted_proxy_cidrs("127.0.0.1/32").unwrap();
    config
}

#[must_use]
pub fn fetch_config_with_session(
    cobalt_api_url: String,
    session_api_url: Option<String>,
    session_api_key: Option<String>,
) -> Config {
    fetch_config_with_delay(cobalt_api_url, session_api_url, session_api_key, 1)
}

#[must_use]
pub fn fetch_config_with_delay(
    cobalt_api_url: String,
    session_api_url: Option<String>,
    session_api_key: Option<String>,
    delay: u64,
) -> Config {
    Config {
        host: "127.0.0.1".into(),
        port: 6401,
        quic_port: 6402,
        database_path: ":memory:".into(),
        rate_limit_per_minute: 100,
        db_pool_size: 1,
        max_concurrent_uploads: 16,
        public_base_url: "http://localhost:6402".into(),
        log_level: "info".into(),
        cleanup_interval_minutes: 30,
        juicehost_api_key: "test-key".into(),

        juicehost_url: cobalt_api_url.clone(),
        public_juicehost_url: "http://localhost:6402".into(),
        juiceback_origin: "http://127.0.0.1:6401".into(),
        jwt_secret: "test-secret".into(),
        cors_origins: vec![],
        report_webhook_url: None,
        smtp_host: None,
        smtp_port: None,
        smtp_username: None,
        smtp_password: None,
        report_email_recipient: None,
        report_email_sender: None,
        quic_cert_path: None,
        ip_encryption_key: "0000000000000000000000000000000000000000000000000000000000000001"
            .into(),
        ip_pepper: "test_pepper".into(),
        trusted_proxy_cidrs: vec![],
        report_retention_days: 90,
        feedback_retention_days: 90,
        cf_api_token: None,
        cf_zone_id: None,
        ticket_jwt_secret: "secret".into(),
        secure_cookies: false,
        sentry_environment: "test".into(),
        direct_upload_enabled: false,
        cobalt_enabled: true,
        cobalt_api_url,
        cobalt_api_key: "cobalt_key".into(),
        cobalt_session_api_url: session_api_url,
        cobalt_session_api_key: session_api_key,
        fetch_empty_retry_delay_secs: delay,
        dte_enabled: false,
        dte_assumed_bps: juiceback::constants::DTE_ASSUMED_BPS,
        dte_safety_mult: juiceback::constants::DTE_SAFETY_MULT,
        dte_base_overhead_secs: juiceback::constants::DTE_BASE_OVERHEAD_SECS,
        dte_min_ttl_secs: juiceback::constants::DTE_MIN_TTL_SECS,
        dte_max_ttl_secs: juiceback::constants::DTE_MAX_TTL_SECS,
        dte_mint_limit: juiceback::constants::DTE_MINT_LIMIT,
        dte_mint_window_secs: juiceback::constants::DTE_MINT_WINDOW_SECS,
        dte_mint_burst: juiceback::constants::DTE_MINT_BURST,
        region_public_juicehosts: std::collections::HashMap::new(),
        allow_private_fetch: true,
    }
}

#[must_use]
pub fn test_jh_config() -> JuicehostConfig {
    JuicehostConfig {
        max_file_size_bytes: 524_288_000,
        default_ttl_hours: 72.0,
        allowed_ttl_hours: vec![0.5, 1.0, 6.0, 12.0, 24.0, 72.0, 168.0],
        danger_level: juiceback::file_validation::ProtectionLevel::High,
        quick_link: false,
        custom_id_enabled: true,
        ultrafast: false,
    }
}

#[must_use]
pub fn state_from_config(config: Config) -> Arc<AppState> {
    state_from_config_with_jh(config, test_jh_config())
}

#[must_use]
pub fn state_from_config_with_jh(config: Config, jh: JuicehostConfig) -> Arc<AppState> {
    let manager = r2d2_sqlite::SqliteConnectionManager::memory();
    let pool = r2d2::Pool::builder().max_size(1).build(manager).unwrap();
    juiceback::db::init_db(&pool.get().unwrap()).unwrap();

    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("x-juicehost-api-key", "test-key".parse().unwrap());
    let http = reqwest::Client::new();

    AppState::new(pool, config, http, headers, Some(jh))
}

#[must_use]
pub fn test_state() -> Arc<AppState> {
    state_from_config(test_config())
}

#[must_use]
pub fn test_state_with_proxies() -> Arc<AppState> {
    state_from_config(test_config_with_proxies())
}
