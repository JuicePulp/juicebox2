mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use tower::ServiceExt;

#[tokio::test]
async fn health_returns_ok() {
    let app = common::mock_router(common::test_state());

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "ok");
}

#[tokio::test]
async fn config_returns_json_without_secrets() {
    let app = common::mock_router(common::test_state());

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/config")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["max_file_size_bytes"], 524_288_000);
    assert!(json.get("juicehost_api_key").is_none());
    assert!(json.get("jwt_secret").is_none());
}

#[tokio::test]
async fn anonymous_scrapes_create_no_sessions_and_set_no_cookie() {
    let state = common::test_state();
    let app = common::mock_router(state.clone());

    for uri in ["/api/health", "/api/config", "/api/openapi.json"] {
        let resp = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(
            resp.headers().get(axum::http::header::SET_COOKIE).is_none(),
            "{uri} must not set a session cookie"
        );
    }

    let sessions: i64 = state
        .db
        .get()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(sessions, 0);
}
