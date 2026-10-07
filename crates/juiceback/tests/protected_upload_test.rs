mod common;

use std::sync::{Arc, Mutex};

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use juiceback::{config::Config, state::AppState};
use serde_json::Value;
use tower::ServiceExt;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path_regex},
};

const TEST_KEY_HEX: &str =
    "0000000000000000000000000000000000000000000000000000000000000001";
const PASSWORD: &str = "correct horse 999";
const PLAINTEXT: &[u8] = b"hello protected world, this is a secret file";

fn test_config(juicehost_url: String) -> Config {
    let mut cfg = common::test_config();
    cfg.juicehost_url = juicehost_url;
    cfg.storage_encryption_key = TEST_KEY_HEX.into();
    cfg
}

fn test_state(juicehost_url: String) -> Arc<AppState> {
    common::state_from_config(test_config(juicehost_url))
}

/// Captures every chunked push body the mock juicehost receives.
fn captured_pushes() -> Arc<Mutex<Vec<Vec<u8>>>> {
    Arc::new(Mutex::new(Vec::new()))
}

async fn mock_push_endpoint(server: &MockServer, captured: Arc<Mutex<Vec<Vec<u8>>>>) {
    Mock::given(method("POST"))
        .and(path_regex("/internal/file/stream/.*"))
        .respond_with(move |req: &wiremock::Request| {
            captured
                .lock()
                .unwrap()
                .push(req.body.clone());
            ResponseTemplate::new(200)
        })
        .mount(server)
        .await;
}

fn multipart_body(boundary: &str, texts: &[(&str, &str)], file: (&str, &[u8])) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, value) in texts {
        out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        out.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
        );
        out.extend_from_slice(value.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    let (filename, bytes) = file;
    out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    out.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: text/plain\r\n\r\n"
        )
        .as_bytes(),
    );
    out.extend_from_slice(bytes);
    out.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    out
}

async fn post_upload(
    app: &axum::Router,
    body: Vec<u8>,
    boundary: &str,
    delete_token: Option<&str>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/upload")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        );
    if let Some(token) = delete_token {
        builder = builder.header("x-delete-token", token);
    }
    let resp = app
        .clone()
        .oneshot(builder.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

async fn db_record(state: &Arc<AppState>, id: &str) -> juiceback::db::FileRecord {
    let lookup = id.to_string();
    state
        .db_call("test_get_file", move |db| {
            juiceback::db::get_file(db, &lookup)
        })
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn protected_relay_roundtrip_stores_ciphertext_only() {
    let server = MockServer::start().await;
    let captured = captured_pushes();
    mock_push_endpoint(&server, Arc::clone(&captured)).await;

    let state = test_state(server.uri());
    let app = common::mock_router(Arc::clone(&state));

    let boundary = "testboundary123";
    let body = multipart_body(
        boundary,
        &[("password", PASSWORD)],
        ("secret.txt", PLAINTEXT),
    );
    let (status, json) = post_upload(&app, body, boundary, None).await;
    assert_eq!(status, StatusCode::OK, "upload failed: {json}");
    assert_eq!(json["protected"], true);
    assert_eq!(json["is_encrypted"], true);
    let id = json["id"].as_str().unwrap().to_string();

    // What reached juicehost must be a valid container, not plaintext.
    let pushes = captured.lock().unwrap();
    assert_eq!(pushes.len(), 1);
    let stored: Vec<u8> = pushes.concat();
    assert!(stored.len() > PLAINTEXT.len());
    assert_eq!(&stored[..4], b"JBC1");
    assert!(
        !stored
            .windows(PLAINTEXT.len())
            .any(|w| w == PLAINTEXT),
        "plaintext must not appear in stored bytes"
    );
    let key = juiceback::crypto_file::FileKey::from_hex(TEST_KEY_HEX).unwrap();
    let mut plain = Vec::new();
    juiceback::crypto_file::decrypt_to_writer(&key, &stored, &mut plain).unwrap();
    assert_eq!(plain, PLAINTEXT);

    // DB carries verifier + flags + header, never the password.
    let record = db_record(&state, &id).await;
    assert!(record.is_protected());
    assert!(record.is_encrypted);
    let hash = record.password_hash.clone().unwrap();
    assert_ne!(hash, PASSWORD);
    let ok = tokio::task::spawn_blocking({
        let hash = hash.clone();
        move || juiceback::auth::verify_password(PASSWORD, &hash).unwrap()
    })
    .await
    .unwrap();
    assert!(ok);
    let wrong = tokio::task::spawn_blocking({
        let hash = hash.clone();
        move || juiceback::auth::verify_password("wrong password", &hash).unwrap()
    })
    .await
    .unwrap();
    assert!(!wrong);
    assert_eq!(
        record.enc_header.as_deref().unwrap(),
        hex::encode(&stored[..13])
    );
}

#[tokio::test]
async fn direct_reserve_with_password_is_rejected() {
    let state = test_state("http://127.0.0.1:1".into());
    let app = common::mock_router(state);
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/upload/direct/reserve")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "filename": "secret.txt",
                        "mime_type": "text/plain",
                        "file_size": 100,
                        "password": PASSWORD,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("USE_RELAY_FOR_PROTECTED"), "{text}");
}

#[tokio::test]
async fn reserve_with_password_completes_via_relay() {
    let server = MockServer::start().await;
    let captured = captured_pushes();
    mock_push_endpoint(&server, Arc::clone(&captured)).await;

    let state = test_state(server.uri());
    let app = common::mock_router(Arc::clone(&state));

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/upload/reserve")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "filename": "secret.txt", "password": PASSWORD })
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let reserve: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(reserve["protected"], true);
    assert_eq!(reserve["is_encrypted"], true);
    let id = reserve["id"].as_str().unwrap().to_string();
    let token = reserve["delete_token"].as_str().unwrap().to_string();

    let boundary = "reserveboundary1";
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"reserve_id\"\r\n\r\n");
    body.extend_from_slice(id.as_bytes());
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"file\"; filename=\"secret.txt\"\r\nContent-Type: text/plain\r\n\r\n",
    );
    body.extend_from_slice(PLAINTEXT);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let (status, json) = post_upload(&app, body, boundary, Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "completion failed: {json}");
    assert_eq!(json["protected"], true);

    let record = db_record(&state, &id).await;
    assert!(record.is_protected());
    assert!(record.enc_header.is_some());
    let pushes = captured.lock().unwrap();
    assert_eq!(juiceback::crypto_file::parse_header(&pushes.concat()).unwrap() as usize, PLAINTEXT.len());
}

#[tokio::test]
async fn short_password_rejected_at_reserve() {    let state = test_state("http://127.0.0.1:1".into());
    let app = common::mock_router(state);
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/upload/reserve")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "filename": "a.txt", "password": "short" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn plain_relay_still_streams_plaintext() {
    let server = MockServer::start().await;
    let captured = captured_pushes();
    mock_push_endpoint(&server, Arc::clone(&captured)).await;

    let state = test_state(server.uri());
    let app = common::mock_router(Arc::clone(&state));

    let boundary = "plainboundary1";
    let body = multipart_body(boundary, &[], ("a.txt", PLAINTEXT));
    let (status, json) = post_upload(&app, body, boundary, None).await;
    assert_eq!(status, StatusCode::OK, "upload failed: {json}");
    assert_eq!(json["protected"], false);
    assert_eq!(json["is_encrypted"], false);

    let pushes = captured.lock().unwrap();
    assert_eq!(pushes.concat(), PLAINTEXT);

    let id = json["id"].as_str().unwrap().to_string();
    let record = db_record(&state, &id).await;
    assert!(!record.is_protected());
    assert!(!record.is_encrypted);
    assert_eq!(record.enc_header, None);
}
