mod common;

use std::sync::{Arc, Mutex};

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use juiceback::state::AppState;
use serde_json::Value;
use tower::ServiceExt;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path_regex},
};

const PAYLOAD: &[u8] = b"the quick brown fox jumps over the lazy dog";

fn captured_pushes() -> Arc<Mutex<Vec<Vec<u8>>>> {
    Arc::new(Mutex::new(Vec::new()))
}

async fn mock_push_endpoint(server: &MockServer, captured: Arc<Mutex<Vec<Vec<u8>>>>) {
    Mock::given(method("POST"))
        .and(path_regex("/internal/file/stream/.*"))
        .respond_with(move |req: &wiremock::Request| {
            captured.lock().unwrap().push(req.body.clone());
            ResponseTemplate::new(200)
        })
        .mount(server)
        .await;
}

fn state(server: &MockServer) -> Arc<AppState> {
    let mut cfg = common::test_config();
    cfg.juicehost_url = server.uri();
    common::state_from_config(cfg)
}

fn meta(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| {
            use base64::Engine as _;
            format!(
                "{k} {}",
                base64::engine::general_purpose::STANDARD.encode(v)
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

async fn create(app: &axum::Router, total: u64) -> String {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/tus")
                .header("Tus-Resumable", "1.0.0")
                .header("Upload-Length", total.to_string())
                .header(
                    "Upload-Metadata",
                    meta(&[("filename", "payload.bin"), ("mimetype", "text/plain")]),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    resp.headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .expect("location")
}

async fn patch(app: &axum::Router, uri: &str, offset: u64, body: Vec<u8>, declared: u64) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(uri)
                .header("Tus-Resumable", "1.0.0")
                .header("Upload-Offset", offset.to_string())
                .header("content-length", declared.to_string())
                .header("content-type", "application/offset+octet-stream")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn offset(app: &axum::Router, uri: &str) -> Option<u64> {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .header("Tus-Resumable", "1.0.0")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    resp.headers()
        .get("upload-offset")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
}

#[tokio::test]
async fn streaming_patch_concatenates_in_order() {
    let server = MockServer::start().await;
    let captured = captured_pushes();
    mock_push_endpoint(&server, Arc::clone(&captured)).await;
    let state = state(&server);
    let app = common::mock_router(Arc::clone(&state));

    let uri = create(&app, PAYLOAD.len() as u64).await;
    let mid = PAYLOAD.len() / 2;

    let (status, _) = patch(&app, &uri, 0, PAYLOAD[..mid].to_vec(), mid as u64).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(offset(&app, &uri).await, Some(mid as u64));

    let (status, json) = patch(
        &app,
        &uri,
        mid as u64,
        PAYLOAD[mid..].to_vec(),
        (PAYLOAD.len() - mid) as u64,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "completion failed: {json}");
    assert_eq!(json["status"], "ready");

    let pushes = captured.lock().unwrap();
    assert_eq!(pushes.len(), 1, "expected a single streaming push");
    assert_eq!(pushes[0], PAYLOAD);
}

#[tokio::test]
async fn streaming_patch_without_content_length_is_capped_by_total() {
    let server = MockServer::start().await;
    let captured = captured_pushes();
    mock_push_endpoint(&server, Arc::clone(&captured)).await;
    let state = state(&server);
    let app = common::mock_router(Arc::clone(&state));

    let uri = create(&app, PAYLOAD.len() as u64).await;

    // No content-length header at all: the stream is bounded by Upload-Length.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(&uri)
                .header("Tus-Resumable", "1.0.0")
                .header("Upload-Offset", "0")
                .header("content-type", "application/offset+octet-stream")
                .body(Body::from(PAYLOAD.to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(captured.lock().unwrap().concat(), PAYLOAD);
}

#[tokio::test]
async fn short_body_keeps_session_at_accepted_offset() {
    let server = MockServer::start().await;
    let captured = captured_pushes();
    mock_push_endpoint(&server, Arc::clone(&captured)).await;
    let state = state(&server);
    let app = common::mock_router(Arc::clone(&state));

    let total = PAYLOAD.len() as u64;
    let uri = create(&app, total).await;
    let cut = 12;

    // Declares the whole file but delivers only the first 12 bytes.
    let (status, _) = patch(&app, &uri, 0, PAYLOAD[..cut].to_vec(), total).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(offset(&app, &uri).await, Some(cut as u64));

    // Resume exactly where the push left off.
    let (status, json) = patch(
        &app,
        &uri,
        cut as u64,
        PAYLOAD[cut..].to_vec(),
        (PAYLOAD.len() - cut) as u64,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "completion failed: {json}");
    assert_eq!(captured.lock().unwrap().concat(), PAYLOAD);
}

#[tokio::test]
async fn stale_offset_is_rejected_without_closing_the_session() {
    let server = MockServer::start().await;
    let captured = captured_pushes();
    mock_push_endpoint(&server, Arc::clone(&captured)).await;
    let state = state(&server);
    let app = common::mock_router(Arc::clone(&state));

    let uri = create(&app, PAYLOAD.len() as u64).await;
    let (status, _) = patch(&app, &uri, 0, PAYLOAD[..5].to_vec(), 5).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = patch(&app, &uri, 0, PAYLOAD[..5].to_vec(), 5).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(offset(&app, &uri).await, Some(5));

    let (status, _) = patch(
        &app,
        &uri,
        5,
        PAYLOAD[5..].to_vec(),
        (PAYLOAD.len() - 5) as u64,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(captured.lock().unwrap().concat(), PAYLOAD);
}

#[tokio::test]
async fn empty_finalize_patch_completes_after_bytes_were_accepted() {
    let server = MockServer::start().await;
    let captured = captured_pushes();
    mock_push_endpoint(&server, Arc::clone(&captured)).await;
    let state = state(&server);
    let app = common::mock_router(Arc::clone(&state));

    let total = PAYLOAD.len() as u64;
    let uri = create(&app, total).await;
    let (status, _) = patch(&app, &uri, 0, PAYLOAD.to_vec(), total).await;
    assert_eq!(status, StatusCode::OK);

    // A lost 200 makes the client retry with a zero-length finalize PATCH.
    // The session is gone by then, which is the signal to confirm via the
    // file record instead of re-uploading.
    let (status, _) = patch(&app, &uri, total, Vec::new(), 0).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
