mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use juiceback::routes::build_router;
use tower::ServiceExt;

#[tokio::test]
async fn banned_ip_returns_403() {
    let state = common::test_state_with_proxies();
    let hashed = juiceback::utils::hash_ip_for_ban("192.168.1.100", &state.config.ip_pepper);
    state.banned_ips.insert(hashed, ());

    let app = build_router(state);
    let resp = app
        .oneshot(common::with_connect_info(
            Request::builder()
                .uri("/api/health")
                .header("x-forwarded-for", "192.168.1.100")
                .body(Body::empty())
                .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn unban_ip_allows_access() {
    let state = common::test_state_with_proxies();
    let hashed = juiceback::utils::hash_ip_for_ban("192.168.1.100", &state.config.ip_pepper);
    state.banned_ips.insert(hashed.clone(), ());
    state.banned_ips.remove(&hashed);

    let app = build_router(state);
    let resp = app
        .oneshot(common::with_connect_info(
            Request::builder()
                .uri("/api/health")
                .header("x-forwarded-for", "192.168.1.100")
                .body(Body::empty())
                .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn banned_ip_can_still_check_ban_status() {
    let state = common::test_state_with_proxies();
    let hashed = juiceback::utils::hash_ip_for_ban("192.168.1.100", &state.config.ip_pepper);
    state
        .ban_ip(&hashed, "spam", "admin")
        .await
        .expect("ban_ip should succeed");

    let app = build_router(state);
    // The /banned page depends on /api/ban-status, so it must stay reachable
    // even for banned IPs (other endpoints still 403).
    let resp = app
        .oneshot(common::with_connect_info(
            Request::builder()
                .uri("/api/ban-status?ip=192.168.1.100")
                .header("x-forwarded-for", "192.168.1.100")
                .body(Body::empty())
                .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["banned"], true);
}

#[tokio::test]
async fn ban_snapshot_requires_api_key_and_returns_hashes() {
    let state = common::test_state_with_proxies();
    let hashed = juiceback::utils::hash_ip_for_ban("192.168.1.100", &state.config.ip_pepper);
    state.ban_ip(&hashed, "spam", "admin").await.unwrap();

    let app = build_router(state);

    // No API key -> 401 (unauthenticated).
    let resp = app
        .clone()
        .oneshot(common::with_connect_info(
            Request::builder()
                .uri("/internal/ban-snapshot")
                .body(Body::empty())
                .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Valid API key -> pepper + hashes.
    let resp = app
        .oneshot(common::with_connect_info(
            Request::builder()
                .uri("/internal/ban-snapshot")
                .header("x-juicehost-api-key", "test-key")
                .body(Body::empty())
                .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 8192).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["pepper"], "test_pepper");
    assert_eq!(json["count"], 1);
    assert_eq!(json["hashes"][0], hashed);
}
