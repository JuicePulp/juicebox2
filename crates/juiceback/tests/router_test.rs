mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use juiceback::db::{FileRecord, get_file, insert_file, set_file_tier};
use tower::ServiceExt;

async fn insert_ready_file(
    state: &std::sync::Arc<juiceback::state::AppState>,
    id: &str,
    host: Option<&str>,
) {
    let record = FileRecord::new(
        id.to_string(),
        "movie.mp4".to_string(),
        "video/mp4".to_string(),
        1024,
        format!("tok-{id}"),
        1000,
        9_999_999_999,
        None,
        host.map(str::to_string),
    );
    let rec = record.clone();
    state
        .db_call("insert_file", move |db| insert_file(db, &rec))
        .await
        .unwrap();
    assert!(get_file(&state.db.get().unwrap(), id).unwrap().is_some());
}

fn resolve_req(id: &str, key: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().uri(format!("/internal/resolve/{id}"));
    if let Some(key) = key {
        builder = builder.header("x-juicehost-api-key", key);
    }
    builder.body(Body::empty()).unwrap()
}

async fn resolve_body(app: axum::Router, req: Request<Body>) -> (StatusCode, serde_json::Value) {
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&body).unwrap()
    };
    (status, json)
}

#[tokio::test]
async fn resolve_needs_api_key() {
    let app = common::mock_router(common::test_state());
    let (status, _) = resolve_body(app, resolve_req("abc12345", None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn resolve_missing_file_is_404() {
    let app = common::mock_router(common::test_state());
    let (status, _) = resolve_body(app, resolve_req("nosuchid", Some("test-key"))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn resolve_returns_host_and_tier() {
    let state = common::test_state();
    insert_ready_file(&state, "reg00001", Some("https://f-eu1.juicey.dev")).await;
    let app = common::mock_router(state);
    let (status, json) =
        resolve_body(app, resolve_req("reg00001", Some("test-key"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["storage_host"], "https://f-eu1.juicey.dev");
    assert_eq!(json["tier"], "standard");
    assert!(json["r2_url"].is_null());
}

#[tokio::test]
async fn resolve_hot_file_reports_r2_url() {
    let mut config = common::test_config();
    config.r2_public_base_url = "https://r.juicey.dev".to_string();
    let state = common::state_from_config(config);
    insert_ready_file(&state, "hot00001", Some("https://f-eu1.juicey.dev")).await;
    state
        .db_call("promote", |db| set_file_tier(db, "hot00001", "hot", "hot/hot00001.mp4"))
        .await
        .unwrap();
    let app = common::mock_router(state);
    let (status, json) =
        resolve_body(app, resolve_req("hot00001", Some("test-key"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["tier"], "hot");
    assert_eq!(json["r2_url"], "https://r.juicey.dev/hot/hot00001.mp4");
    assert_eq!(json["r2_key"], "hot/hot00001.mp4");
}

#[tokio::test]
async fn share_link_uses_router_for_official_hosts() {
    let mut config = common::test_config();
    config.router_base_url = Some("https://f.juicey.dev".to_string());
    config.juicehost_url = "http://127.0.0.1:6402".to_string();
    config.region_public_juicehosts = std::collections::HashMap::from([
        ("eu".to_string(), "https://f-eu1.juicey.dev".to_string()),
    ]);

    assert_eq!(
        juiceback::utils::share_link(
            &config,
            Some("https://f-eu1.juicey.dev"),
            "abc12345",
            "movie.mp4",
        ),
        "https://f.juicey.dev/v/abc12345.mp4"
    );
    assert_eq!(
        juiceback::utils::share_link(&config, None, "abc12345", "movie.mp4"),
        "https://f.juicey.dev/v/abc12345.mp4"
    );
    assert_eq!(
        juiceback::utils::share_link(
            &config,
            Some("https://thirdparty.example"),
            "abc12345",
            "movie.mp4",
        ),
        "https://thirdparty.example/v/abc12345.mp4"
    );

    config.router_base_url = None;
    assert_eq!(
        juiceback::utils::share_link(
            &config,
            Some("https://f-eu1.juicey.dev"),
            "abc12345",
            "movie.mp4",
        ),
        "https://f-eu1.juicey.dev/v/abc12345.mp4"
    );
}
