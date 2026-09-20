use std::sync::Arc;

use super::{
    errors::format_error_response,
    target::{
        check_storage_host, custom_juicehost_target, require_juicehost_url,
        resolve_juicehost_target,
    },
};
use crate::state::AppState;

#[test]
fn format_error_response_with_json() {
    let body = r#"{"error": "STORAGE_FULL", "message": "disk is full"}"#.to_string();
    let result = format_error_response(507, body);
    assert!(result.contains("[STORAGE_FULL]"));
    assert!(result.contains("disk is full"));
    assert!(result.contains("status=507"));
}

#[test]
fn format_error_response_empty_body() {
    let result = format_error_response(500, String::new());
    assert!(result.contains("UNKNOWN_ERROR"));
    assert!(result.contains("body=<empty>"));
    assert!(result.contains("status=500"));
}

#[test]
fn format_error_response_invalid_json() {
    let result = format_error_response(403, "forbidden".to_string());
    assert!(result.contains("forbidden"));
    assert!(result.contains("status=403"));
}

fn test_state() -> Arc<AppState> {
    let config = crate::config::Config {
        host: "127.0.0.1".into(),
        port: 6401,
        quic_port: 6402,
        database_path: "./test.db".into(),
        rate_limit_per_minute: 10,
        db_pool_size: 8,
        max_concurrent_uploads: 16,
        public_base_url: "http://localhost:6402".into(),
        log_level: "info".into(),
        cleanup_interval_minutes: 30,
        juicehost_api_key: "my_api_key".into(),
        juicehost_url: "http://127.0.0.1:6402".into(),
        public_juicehost_url: "http://localhost:6402".into(),
        juiceback_origin: "http://127.0.0.1:6401".into(),
        jwt_secret: "secret".into(),
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
        cobalt_enabled: false,
        cobalt_api_url: "http://localhost:7272".into(),
        cobalt_api_key: "cobalt_key".into(),
        cobalt_session_api_url: None,
        cobalt_session_api_key: None,
        fetch_empty_retry_delay_secs: 0,
        dte_enabled: false,
        dte_assumed_bps: crate::constants::DTE_ASSUMED_BPS,
        dte_safety_mult: crate::constants::DTE_SAFETY_MULT,
        dte_base_overhead_secs: crate::constants::DTE_BASE_OVERHEAD_SECS,
        dte_min_ttl_secs: crate::constants::DTE_MIN_TTL_SECS,
        dte_max_ttl_secs: crate::constants::DTE_MAX_TTL_SECS,
        dte_mint_limit: crate::constants::DTE_MINT_LIMIT,
        dte_mint_window_secs: crate::constants::DTE_MINT_WINDOW_SECS,
        dte_mint_burst: crate::constants::DTE_MINT_BURST,
        region_public_juicehosts: std::collections::HashMap::new(),
        allow_private_fetch: true,
    };
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("x-juicehost-api-key", "my_api_key".parse().unwrap());
    headers.insert(
        "x-juiceback-origin",
        "http://127.0.0.1:6401".parse().unwrap(),
    );
    crate::state::AppState::new(
        r2d2::Pool::builder()
            .max_size(1)
            .build(r2d2_sqlite::SqliteConnectionManager::memory())
            .unwrap(),
        config,
        reqwest::Client::new(),
        headers,
        None,
    )
}

#[tokio::test]
async fn custom_target_does_not_forward_internal_headers() {
    let target = resolve_juicehost_target(&test_state(), Some("https://1.1.1.1:8443"))
        .await
        .unwrap();
    assert!(target.headers.is_empty());
}

#[tokio::test]
async fn configured_target_preserves_internal_headers_and_url() {
    let target = resolve_juicehost_target(&test_state(), None).await.unwrap();
    assert_eq!(target.base_url, "http://127.0.0.1:6402");
    assert_eq!(target.headers["x-juicehost-api-key"], "my_api_key");
    assert_eq!(
        target.headers["x-juiceback-origin"],
        "http://127.0.0.1:6401"
    );
}

#[test]
fn require_juicehost_url_empty() {
    assert!(require_juicehost_url("").is_err());
}

#[test]
fn require_juicehost_url_valid() {
    assert!(require_juicehost_url("http://127.0.0.1:6402").is_ok());
}

#[tokio::test]
async fn custom_target_normalizes_bare_public_ip() {
    let target = custom_juicehost_target("1.1.1.1").await.unwrap();
    assert_eq!(target.base_url, "https://1.1.1.1");
}

#[tokio::test]
async fn custom_target_accepts_root_slash_and_keeps_port() {
    let target = custom_juicehost_target(" https://1.1.1.1:8443/ ")
        .await
        .unwrap();
    assert_eq!(target.base_url, "https://1.1.1.1:8443");
}

#[tokio::test]
async fn custom_target_rejects_credentials_and_extra_url_components() {
    assert!(
        custom_juicehost_target("https://user:pass@1.1.1.1")
            .await
            .is_err()
    );
    assert!(
        custom_juicehost_target("https://1.1.1.1/api")
            .await
            .is_err()
    );
    assert!(
        custom_juicehost_target("https://1.1.1.1/?next=x")
            .await
            .is_err()
    );
    assert!(
        custom_juicehost_target("https://1.1.1.1/#fragment")
            .await
            .is_err()
    );
    assert!(
        custom_juicehost_target("https://localhost./")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn custom_target_rejects_local_and_metadata_names() {
    assert!(
        custom_juicehost_target("http://localhost:6402")
            .await
            .is_err()
    );
    assert!(
        custom_juicehost_target("http://service.internal")
            .await
            .is_err()
    );
    assert!(
        custom_juicehost_target("http://metadata.google.internal")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn custom_target_rejects_bad_scheme() {
    assert!(custom_juicehost_target("ftp://1.1.1.1").await.is_err());
    assert!(custom_juicehost_target("file:///etc/passwd").await.is_err());
}

#[tokio::test]
async fn check_storage_host_private_flag() {
    assert!(check_storage_host("http://127.0.0.1", false).await.is_err());
    assert!(
        check_storage_host("http://10.0.0.1:8080", false)
            .await
            .is_err()
    );
    assert!(check_storage_host("http://[::1]", false).await.is_err());
    assert!(check_storage_host("http://127.0.0.1", true).await.is_ok());
    assert!(check_storage_host("files.example.com", true).await.is_ok());
}

#[tokio::test]
async fn custom_target_rejects_non_public_addresses() {
    for host in [
        "http://10.0.0.1",
        "http://192.168.1.1",
        "http://172.16.5.5",
        "http://169.254.169.254",
        "http://127.0.0.1",
        "http://224.0.0.1",
        "http://[::1]",
        "http://[fe80::1]",
        "http://[fc00::1]",
        "http://[ff02::1]",
        "http://[::ffff:127.0.0.1]",
    ] {
        assert!(
            custom_juicehost_target(host).await.is_err(),
            "accepted {host}"
        );
    }
}
