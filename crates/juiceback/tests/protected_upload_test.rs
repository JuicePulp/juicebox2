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
    matchers::{method, path as p, path_regex},
};

fn b64(s: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(s)
}

const TEST_KEY_HEX: &str = "0000000000000000000000000000000000000000000000000000000000000001";
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

/// Serves captured pushes back as the internal ciphertext endpoint,
/// honoring a single `Range: bytes=A-B` header like juicehost does.
async fn mock_ciphertext_endpoint(server: &MockServer, captured: Arc<Mutex<Vec<Vec<u8>>>>) {
    Mock::given(method("GET"))
        .and(path_regex("/internal/file/.*/ciphertext"))
        .respond_with(move |req: &wiremock::Request| {
            let stored: Vec<u8> = captured.lock().unwrap().concat();
            let total = stored.len() as u64;
            let range = req
                .headers
                .get("range")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("bytes="))
                .and_then(|spec| {
                    let (s, e) = spec.split_once('-')?;
                    let start: u64 = s.parse().ok()?;
                    let end: u64 = if e.is_empty() {
                        total.saturating_sub(1)
                    } else {
                        e.parse().ok()?
                    };
                    (start <= end && start < total).then(|| (start, end.min(total - 1)))
                });
            match range {
                Some((start, end)) => ResponseTemplate::new(206)
                    .insert_header("content-range", format!("bytes {start}-{end}/{total}"))
                    .set_body_bytes(stored[start as usize..=end as usize].to_vec()),
                None => ResponseTemplate::new(200).set_body_bytes(stored),
            }
        })
        .mount(server)
        .await;
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
    let mut builder = Request::builder().method("POST").uri("/upload").header(
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

    mock_ciphertext_endpoint(&server, Arc::clone(&captured)).await;

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
        !stored.windows(PLAINTEXT.len()).any(|w| w == PLAINTEXT),
        "plaintext must not appear in stored bytes"
    );
    let key = juiceback::crypto_file::FileKey::from_hex(TEST_KEY_HEX).unwrap();
    // New uploads use a per-file data key: recover it through the escrow
    // copy (what the server does) and decrypt client-side with it.
    let record = db_record(&state, &id).await;
    let escrow = record.dek_escrow.clone().expect("v1 escrow material");
    let escrow_raw = hex::decode(&escrow).unwrap();
    let dek_bytes = juiceback::crypto_file::decrypt_chunk(&key, &escrow_raw).unwrap();
    let dek = juiceback::crypto_file::FileKey::from_bytes(&dek_bytes).unwrap();
    let mut plain = Vec::new();
    juiceback::crypto_file::decrypt_to_writer(&dek, &stored, &mut plain).unwrap();
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

    mock_ciphertext_endpoint(&server, Arc::clone(&captured)).await;

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
    assert_eq!(
        juiceback::crypto_file::parse_header(&pushes.concat()).unwrap() as usize,
        PLAINTEXT.len()
    );
}

#[tokio::test]
async fn short_password_rejected_at_reserve() {
    let state = test_state("http://127.0.0.1:1".into());
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

    mock_ciphertext_endpoint(&server, Arc::clone(&captured)).await;

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

async fn tus_create(
    app: &axum::Router,
    metadata: &str,
    total_length: u64,
) -> (StatusCode, Option<String>) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/tus")
                .header("Tus-Resumable", "1.0.0")
                .header("Upload-Length", total_length.to_string())
                .header("Upload-Metadata", metadata)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    (status, location)
}

async fn tus_patch(
    app: &axum::Router,
    location: &str,
    offset: u64,
    chunk: &[u8],
) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(location)
                .header("Tus-Resumable", "1.0.0")
                .header("Upload-Offset", offset.to_string())
                .header("content-type", "application/offset+octet-stream")
                .body(Body::from(chunk.to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

fn tus_meta(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{k} {}", b64(v)))
        .collect::<Vec<_>>()
        .join(",")
}

#[tokio::test]
async fn tus_protected_roundtrip_spools_then_encrypts() {
    let server = MockServer::start().await;
    let captured = captured_pushes();
    mock_push_endpoint(&server, Arc::clone(&captured)).await;

    mock_ciphertext_endpoint(&server, Arc::clone(&captured)).await;

    let state = test_state(server.uri());
    let app = common::mock_router(Arc::clone(&state));

    let meta = tus_meta(&[
        ("filename", "secret.txt"),
        ("mimetype", "text/plain"),
        ("password", PASSWORD),
    ]);
    let (status, location) = tus_create(&app, &meta, PLAINTEXT.len() as u64).await;
    assert_eq!(status, StatusCode::CREATED);
    let location = location.unwrap();

    // Two PATCHes to prove offset bookkeeping works while spooling.
    let mid = PLAINTEXT.len() / 2;
    let (status, _) = tus_patch(&app, &location, 0, &PLAINTEXT[..mid]).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, json) = tus_patch(&app, &location, mid as u64, &PLAINTEXT[mid..]).await;
    assert_eq!(status, StatusCode::OK, "completion failed: {json}");
    assert_eq!(json["protected"], true);
    assert_eq!(json["is_encrypted"], true);

    let pushes = captured.lock().unwrap();
    assert_eq!(pushes.len(), 1);
    let stored: Vec<u8> = pushes.concat();
    assert_eq!(&stored[..4], b"JBC1");
    assert!(
        !stored.windows(PLAINTEXT.len()).any(|w| w == PLAINTEXT),
        "plaintext must not reach juicehost"
    );

    let id = json["id"].as_str().unwrap().to_string();
    let record = db_record(&state, &id).await;
    assert!(record.is_protected());
    assert!(record.is_encrypted);
    assert!(record.enc_header.is_some());
}

#[tokio::test]
async fn tus_parallel_with_password_is_rejected() {
    let state = test_state("http://127.0.0.1:1".into());
    let app = common::mock_router(state);
    let meta = format!(
        "filename {},mimetype {},password {},session_id {},part_index {},total_parts {}",
        b64("big.bin"),
        b64("application/octet-stream"),
        b64(PASSWORD),
        b64("sess1"),
        b64("0"),
        b64("2"),
    );
    let (status, _) = tus_create(&app, &meta, 100).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn tus_adopts_protected_reservation() {
    let server = MockServer::start().await;
    let captured = captured_pushes();
    mock_push_endpoint(&server, Arc::clone(&captured)).await;

    mock_ciphertext_endpoint(&server, Arc::clone(&captured)).await;

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
    let id = reserve["id"].as_str().unwrap();
    let token = reserve["delete_token"].as_str().unwrap();

    // No password in TUS metadata: the reservation carries the verifier.
    let meta = tus_meta(&[
        ("filename", "secret.txt"),
        ("mimetype", "text/plain"),
        ("reserve_id", id),
        ("delete_token", token),
    ]);
    let (status, location) = tus_create(&app, &meta, PLAINTEXT.len() as u64).await;
    assert_eq!(status, StatusCode::CREATED, "{location:?}");
    let location = location.unwrap();
    let (status, json) = tus_patch(&app, &location, 0, PLAINTEXT).await;
    assert_eq!(status, StatusCode::OK, "completion failed: {json}");
    assert_eq!(json["protected"], true);

    let record = db_record(&state, id).await;
    assert!(record.is_protected());
    assert!(record.enc_header.is_some());
    let pushes = captured.lock().unwrap();
    assert_eq!(&pushes.concat()[..4], b"JBC1");
}

#[tokio::test]
async fn fetch_with_password_stores_ciphertext_only() {
    let server = MockServer::start().await;
    let captured = captured_pushes();

    Mock::given(method("POST"))
        .and(p("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "status": "tunnel",
            "url": format!("{}/tunnel?id=abc123", server.uri()),
            "filename": "clip.mp4",
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(p("/tunnel"))
        .respond_with(ResponseTemplate::new(200).set_body_string("fake-media-bytes"))
        .mount(&server)
        .await;
    let cap = Arc::clone(&captured);
    Mock::given(method("POST"))
        .and(path_regex("/internal/file/stream/.*"))
        .respond_with(move |req: &wiremock::Request| {
            cap.lock().unwrap().push(req.body.clone());
            ResponseTemplate::new(200)
        })
        .mount(&server)
        .await;
    let mut cfg = common::fetch_config_with_delay(server.uri(), None, None, 1);
    cfg.juicehost_url = server.uri();
    cfg.storage_encryption_key = TEST_KEY_HEX.into();
    let state = common::state_from_config(cfg);
    let app = common::mock_router(Arc::clone(&state));

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/fetch")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "url": "https://example.com/v/1", "password": PASSWORD })
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let cookie = resp
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .map(|h| h.split(';').next().unwrap_or_default().to_string());
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let started: Value = serde_json::from_slice(&bytes).unwrap();
    let job_id = started["job_id"].as_str().unwrap().to_string();

    let mut file_id = String::new();
    for _ in 0..200 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let mut builder = Request::builder().uri(format!("/api/fetch/{job_id}"));
        if let Some(ref cookie) = cookie {
            builder = builder.header("cookie", cookie);
        }
        let resp = app
            .clone()
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let status: Value = serde_json::from_slice(&bytes).unwrap();
        if status["status"] == "done" {
            file_id = status["file"]["id"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            break;
        }
        assert_ne!(status["status"], "failed", "fetch job failed: {status}");
    }
    assert!(!file_id.is_empty(), "fetch job did not complete");

    let record = db_record(&state, &file_id).await;
    assert!(record.is_protected());
    assert!(record.is_encrypted);

    let pushes = captured.lock().unwrap();
    let stored: Vec<u8> = pushes.concat();
    assert!(!stored.is_empty());
    assert_eq!(&stored[..4], b"JBC1");
    assert!(
        !stored
            .windows(b"fake-media-bytes".len())
            .any(|w| w == b"fake-media-bytes"),
        "plaintext must not reach juicehost"
    );
}

/// Upload a protected file through the relay and return its id.
async fn upload_protected(app: &axum::Router, state: &Arc<AppState>, boundary: &str) -> String {
    let body = multipart_body(
        boundary,
        &[("password", PASSWORD)],
        ("secret.txt", PLAINTEXT),
    );
    let (status, json) = post_upload(app, body, boundary, None).await;
    assert_eq!(status, StatusCode::OK, "setup upload failed: {json}");
    let id = json["id"].as_str().unwrap().to_string();
    let record = db_record(state, &id).await;
    assert!(record.is_protected());
    id
}

async fn post_unlock_json(
    app: &axum::Router,
    id: &str,
    password: &str,
) -> (StatusCode, axum::http::HeaderMap, Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/file/{id}/unlock"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "password": password }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, headers, json)
}

fn unlock_cookie(headers: &axum::http::HeaderMap, id: &str) -> Option<String> {
    let name = format!("jb_unlock_{id}=");
    headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with(&name))
        .map(|v| v.split(';').next().unwrap_or_default().to_string())
}

#[tokio::test]
async fn unlock_page_and_cookie_flow() {
    let server = MockServer::start().await;
    let captured = captured_pushes();
    mock_push_endpoint(&server, Arc::clone(&captured)).await;

    mock_ciphertext_endpoint(&server, Arc::clone(&captured)).await;

    let state = test_state(server.uri());
    let app = common::mock_router(Arc::clone(&state));
    let id = upload_protected(&app, &state, "unlockpage1").await;

    // The backend unlock page is retired: old links bounce to the host shell.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/file/{id}/unlock"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        location.ends_with(&format!("/v/{id}.txt")),
        "unexpected redirect: {location}"
    );

    
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/file/{id}/content"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let no_auth_body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();

    // Wrong password: identical 403 body (no oracle).
    let (status, _, _) = post_unlock_json(&app, &id, "wrong password").await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Correct password: cookie + JSON ok.
    let (status, headers, json) = post_unlock_json(&app, &id, PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let cookie = unlock_cookie(&headers, &id).expect("unlock cookie");
    assert!(cookie.starts_with(&format!("jb_unlock_{id}=")));

    // Cookie grants the bytes.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/file/{id}/content"))
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("accept-ranges")
            .and_then(|v| v.to_str().ok()),
        Some("bytes")
    );
    assert_eq!(
        resp.headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(bytes.as_ref(), PLAINTEXT);

    // The 403 bodies for missing vs wrong credentials match.
    let (status, _, _) = post_unlock_json(&app, &id, "wrong password").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let _ = no_auth_body;
}

#[tokio::test]
async fn content_password_transports_and_ranges() {
    let server = MockServer::start().await;
    let captured = captured_pushes();
    mock_push_endpoint(&server, Arc::clone(&captured)).await;

    mock_ciphertext_endpoint(&server, Arc::clone(&captured)).await;

    let state = test_state(server.uri());
    let app = common::mock_router(Arc::clone(&state));
    let id = upload_protected(&app, &state, "unlockrange1").await;

    // ?password= query transport.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/file/{id}/content?password={}",
                    urlencode(PASSWORD)
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(bytes.as_ref(), PLAINTEXT);

    // X-File-Password transport.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/file/{id}/content"))
                .header("X-File-Password", PASSWORD)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Byte range over the decrypted representation.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/file/{id}/content"))
                .header("X-File-Password", PASSWORD)
                .header("Range", "bytes=0-9")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    let total = PLAINTEXT.len();
    assert_eq!(
        resp.headers()
            .get("content-range")
            .and_then(|v| v.to_str().ok()),
        Some(format!("bytes 0-9/{total}").as_str())
    );
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(bytes.as_ref(), &PLAINTEXT[..10]);

    // Open-ended range.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/file/{id}/content"))
                .header("X-File-Password", PASSWORD)
                .header("Range", format!("bytes={}-", total - 5))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(bytes.as_ref(), &PLAINTEXT[total - 5..]);

    // Unsatisfiable range.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/file/{id}/content"))
                .header("X-File-Password", PASSWORD)
                .header("Range", format!("bytes={}-{}", total + 100, total + 200))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::RANGE_NOT_SATISFIABLE);

    // HEAD mirrors headers with no body.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("HEAD")
                .uri(format!("/file/{id}/content"))
                .header("X-File-Password", PASSWORD)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok()),
        Some(total.to_string().as_str())
    );
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(bytes.is_empty());

    // Unprotected files redirect to their public URL instead.
    let boundary = "plainunlock1";
    let body = multipart_body(boundary, &[], ("a.txt", PLAINTEXT));
    let (status, json) = post_upload(&app, body, boundary, None).await;
    assert_eq!(status, StatusCode::OK);
    let plain_id = json["id"].as_str().unwrap().to_string();
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/file/{plain_id}/content"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-.~_".contains(&b) {
            out.push(b as char);
        } else if b == b' ' {
            out.push_str("%20");
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[tokio::test]
async fn unlock_rate_limits_guessing() {
    let server = MockServer::start().await;
    let captured = captured_pushes();
    mock_push_endpoint(&server, Arc::clone(&captured)).await;

    let mut cfg = common::test_config();
    cfg.juicehost_url = server.uri();
    cfg.storage_encryption_key = TEST_KEY_HEX.into();
    cfg.password_try_limit = 3;
    let state = common::state_from_config(cfg);
    let app = common::mock_router(Arc::clone(&state));
    let id = upload_protected(&app, &state, "unlocklimit1").await;

    for _ in 0..3 {
        let (status, _, _) = post_unlock_json(&app, &id, "wrong password").await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
    let (status, _, _) = post_unlock_json(&app, &id, "wrong password").await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
}

async fn seed_admin(state: &Arc<AppState>) -> String {
    state
        .db_call("seed_test_admin", move |db| {
            db.execute(
                "INSERT INTO admins (username, password_hash) VALUES ('testadmin', 'x')",
                [],
            )
        })
        .await
        .unwrap();
    juiceback::auth::create_jwt("testadmin", "test-secret").unwrap()
}

fn admin_cookie(token: &str) -> String {
    format!("token={token}")
}

#[tokio::test]
async fn report_password_enables_admin_preview() {
    let server = MockServer::start().await;
    let captured = captured_pushes();
    mock_push_endpoint(&server, Arc::clone(&captured)).await;
    mock_ciphertext_endpoint(&server, Arc::clone(&captured)).await;

    let state = test_state(server.uri());
    let app = common::mock_router(Arc::clone(&state));
    let id = upload_protected(&app, &state, "reportpreview1").await;
    let file_url = format!("http://localhost:6402/v/{id}.txt");

    
    let body = format!(
        "file_url={}&reason=spam&details=test&password={}",
        urlencode(&file_url),
        urlencode(PASSWORD)
    );
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/report")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    let reports = state
        .db_call("list_reports", move |db| juiceback::db::list_reports(db))
        .await
        .unwrap();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].password.as_deref(), Some(PASSWORD));

    // Admin list exposes availability, never the password.
    let token = seed_admin(&state).await;
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/admin/reports")
                .header("cookie", admin_cookie(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let list: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(list["items"][0]["has_password"], true);
    assert!(list["items"][0].get("password").is_none());

    // Preview streams decrypted bytes using the stored password.
    let report_id = reports[0].id;
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/admin/report/{report_id}/preview"))
                .header("cookie", admin_cookie(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(bytes.as_ref(), PLAINTEXT);

    // Preview without auth is rejected.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/admin/report/{report_id}/preview"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn deleting_protected_file_removes_ciphertext_and_row() {
    let server = MockServer::start().await;
    let captured = captured_pushes();
    mock_push_endpoint(&server, Arc::clone(&captured)).await;
    mock_ciphertext_endpoint(&server, Arc::clone(&captured)).await;

    let deleted: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let deleted_clone = Arc::clone(&deleted);
    Mock::given(method("DELETE"))
        .and(path_regex("/internal/file/.*"))
        .respond_with(move |req: &wiremock::Request| {
            deleted_clone
                .lock()
                .unwrap()
                .push(req.url.path().to_string());
            ResponseTemplate::new(200)
        })
        .mount(&server)
        .await;

    let state = test_state(server.uri());
    let app = common::mock_router(Arc::clone(&state));
    let id = upload_protected(&app, &state, "deletecleanup1").await;

    // The completion response already points at the unlock page.
    let record = db_record(&state, &id).await;
    assert!(record.is_protected());

    // Info reports the unlock URL, not the raw storage URL.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/file/{id}/info"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let info: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(info["protected"], true);
    assert!(
        info["url"]
            .as_str()
            .unwrap_or_default()
            .ends_with(&format!("/v/{id}.txt")),
        "unexpected url: {}",
        info["url"]
    );

    let token = record.delete_token.clone();
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/file/{id}"))
                .header("x-delete-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let lookup = id.clone();
    let gone = state
        .db_call("test_get_deleted", move |db| {
            juiceback::db::get_file(db, &lookup)
        })
        .await
        .unwrap();
    assert!(gone.is_none());
    let hits = deleted.lock().unwrap();
    assert!(
        hits.iter().any(|p| p.contains(&id)),
        "host ciphertext was not deleted: {hits:?}"
    );
}

async fn get_gateway_params(app: &axum::Router, id: &str) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/gateway/params/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

async fn post_gateway_unlock(
    app: &axum::Router,
    id: &str,
    password: &str,
) -> (StatusCode, axum::http::HeaderMap, Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/gateway/unlock")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "id": id, "password": password }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, headers, json)
}

#[tokio::test]
async fn gateway_params_and_unlock_release_working_dek() {
    let server = MockServer::start().await;
    let captured = captured_pushes();
    mock_push_endpoint(&server, Arc::clone(&captured)).await;
    mock_ciphertext_endpoint(&server, Arc::clone(&captured)).await;

    let state = test_state(server.uri());
    let app = common::mock_router(Arc::clone(&state));
    let id = upload_protected(&app, &state, "gateway1").await;

    let (status, params) = get_gateway_params(&app, &id).await;
    assert_eq!(status, StatusCode::OK, "{params}");
    assert_eq!(params["key_version"], "1");
    assert!(!params["salt"].as_str().unwrap_or_default().is_empty());
    assert_eq!(params["chunk_shift"], 16);
    assert_eq!(params["plain_len"], PLAINTEXT.len() as u64);
    assert!(
        params["ciphertext_url"]
            .as_str()
            .unwrap()
            .ends_with(&format!("/c/{id}"))
    );
    assert_eq!(params["filename"], "secret.txt");

    // Wrong password: 403, no key.
    let (status, _, _) = post_gateway_unlock(&app, &id, "wrong password").await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Correct password: data key, no-store, decrypts the stored bytes.
    let (status, headers, json) = post_gateway_unlock(&app, &id, PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(
        headers.get("cache-control").and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    let dek_b64 = json["dek"].as_str().unwrap().to_string();
    assert!(!dek_b64.is_empty());
    assert!(!json.to_string().contains(PASSWORD));

    use base64::Engine as _;
    let dek_raw = base64::engine::general_purpose::STANDARD
        .decode(&dek_b64)
        .unwrap();
    let dek = juiceback::crypto_file::FileKey::from_bytes(&dek_raw).unwrap();
    let stored: Vec<u8> = captured.lock().unwrap().concat();
    let mut plain = Vec::new();
    juiceback::crypto_file::decrypt_to_writer(&dek, &stored, &mut plain).unwrap();
    assert_eq!(plain, PLAINTEXT);

    // Unknown ids and public files: 404, never a key.
    let (status, _) = get_gateway_params(&app, "nope1234").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = post_gateway_unlock(&app, "nope1234", PASSWORD).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let boundary = "gatewaypublic1";
    let body = multipart_body(boundary, &[], ("a.txt", PLAINTEXT));
    let (status, json) = post_upload(&app, body, boundary, None).await;
    assert_eq!(status, StatusCode::OK);
    let plain_id = json["id"].as_str().unwrap().to_string();
    let (status, _) = get_gateway_params(&app, &plain_id).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = post_gateway_unlock(&app, &plain_id, PASSWORD).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// Seed a legacy (global-key, `key_version` 0) protected row with matching
/// mock-host bytes, proving old files keep working read-only.
async fn seed_legacy(state: &Arc<AppState>, captured: Arc<Mutex<Vec<Vec<u8>>>>) -> String {
    let key = juiceback::crypto_file::FileKey::from_hex(TEST_KEY_HEX).unwrap();
    let stored =
        juiceback::crypto_file::encrypt_reader(&key, PLAINTEXT.len() as u64, &PLAINTEXT[..])
            .unwrap();
    captured.lock().unwrap().push(stored.clone());
    let hash = juiceback::auth::hash_password(PASSWORD).unwrap();
    let id = "legacy01".to_string();
    let record = juiceback::db::FileRecord {
        storage_path: format!("remote-{id}"),
        id: id.clone(),
        filename: "secret.txt".into(),
        mime_type: "text/plain".into(),
        size_bytes: PLAINTEXT.len() as i64,
        delete_token: uuid::Uuid::new_v4().to_string(),
        uploaded_at: chrono::Utc::now().timestamp(),
        expires_at: chrono::Utc::now().timestamp() + 3600,
        uploader_ip: None,
        storage_host: None,
        status: "ready".into(),
        password_hash: Some(hash),
        is_encrypted: true,
        enc_header: Some(hex::encode(&stored[..13])),
        dek_wrapped: None,
        dek_salt: None,
        dek_escrow: None,
        key_version: 0,
    };
    state
        .db_call("seed_legacy", move |db| {
            juiceback::db::insert_file(db, &record)
        })
        .await
        .unwrap();
    id
}

#[tokio::test]
async fn legacy_files_stay_readable_without_key_release() {
    let server = MockServer::start().await;
    let captured = captured_pushes();
    mock_ciphertext_endpoint(&server, Arc::clone(&captured)).await;

    let state = test_state(server.uri());
    let id = seed_legacy(&state, Arc::clone(&captured)).await;
    let app = common::mock_router(Arc::clone(&state));

    let (status, params) = get_gateway_params(&app, &id).await;
    assert_eq!(status, StatusCode::OK, "{params}");
    assert_eq!(params["key_version"], "0");

    // Legacy rows never release keys here.
    let (status, _, _) = post_gateway_unlock(&app, &id, PASSWORD).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Server-side password transports still decrypt legacy bytes.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/file/{id}/content"))
                .header("X-File-Password", PASSWORD)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(bytes.as_ref(), PLAINTEXT);

    // Old unlock links bounce to the host shell too.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/file/{id}/unlock"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
}
