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
async fn renew_with_same_id_renames_filename() {
    let state = common::test_state();
    insert_ready_file(&state, "reg00001", None).await;
    let app = common::mock_router(std::sync::Arc::clone(&state));
    let payload = serde_json::json!({
        "custom_id": "reg00001",
        "filename": "renamed clip.mkv"
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/file/reg00001/renew")
                .header("content-type", "application/json")
                .header("x-delete-token", "tok-reg00001")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["id"], "reg00001");
    assert_eq!(json["filename"], "renamed clip.mkv");
    assert_eq!(json["url"], "http://localhost:6402/v/reg00001.mkv");

    let record = get_file(&state.db.get().unwrap(), "reg00001")
        .unwrap()
        .unwrap();
    assert_eq!(record.filename, "renamed clip.mkv");
}

#[tokio::test]
async fn renew_with_new_id_renames_filename_and_url() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/internal/file/reg00002/rename"))
        .respond_with(wiremock::ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let mut config = common::test_config();
    config.juicehost_url = server.uri();
    let state = common::state_from_config(config);
    insert_ready_file(&state, "reg00002", None).await;
    let app = common::mock_router(std::sync::Arc::clone(&state));
    let payload = serde_json::json!({
        "custom_id": "freshname",
        "filename": "renamed.txt"
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/file/reg00002/renew")
                .header("content-type", "application/json")
                .header("x-delete-token", "tok-reg00002")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["id"], "freshname");
    assert_eq!(json["filename"], "renamed.txt");
    assert_eq!(json["url"], "http://localhost:6402/v/freshname.txt");

    let db = state.db.get().unwrap();
    assert!(get_file(&db, "reg00002").unwrap().is_none());
    let record = get_file(&db, "freshname").unwrap().unwrap();
    assert_eq!(record.filename, "renamed.txt");
    assert_eq!(record.storage_path, "remote-freshname");
}

#[tokio::test]
async fn resolve_returns_host_and_tier() {
    let state = common::test_state();
    insert_ready_file(&state, "reg00001", Some("https://f-eu1.juicey.dev")).await;
    let app = common::mock_router(state);
    let (status, json) = resolve_body(app, resolve_req("reg00001", Some("test-key"))).await;
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
        .db_call("promote", |db| {
            set_file_tier(db, "hot00001", "hot", "hot/hot00001.mp4")
        })
        .await
        .unwrap();
    let app = common::mock_router(state);
    let (status, json) = resolve_body(app, resolve_req("hot00001", Some("test-key"))).await;
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
    config.region_public_juicehosts = std::collections::HashMap::from([(
        "eu".to_string(),
        "https://f-eu1.juicey.dev".to_string(),
    )]);

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
