//! Peer metadata endpoints: registration, internal authorization, share links
//! and lifecycle. Every test gets a fresh in-memory database and paired device.

mod common;

use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use jsonwebtoken::{EncodingKey, Header, encode};
use juiceback::{
    auth::{DeviceClaims, ISS_DEVICE},
    db::{self, FileRecord},
    state::AppState,
};
use serde_json::{Value, json};
use tower::ServiceExt;

fn device_token(sub: &str, user_id: &str) -> String {
    let now = usize::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    )
    .unwrap();
    let claims = DeviceClaims {
        sub: sub.to_owned(),
        user_id: user_id.to_owned(),
        iss: ISS_DEVICE.to_owned(),
        iat: now,
        exp: now + 3600,
        file_id: None,
        filename: None,
        mime_type: None,
        file_size: None,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(b"secret"), // test_config().ticket_jwt_secret
    )
    .unwrap()
}

fn pair_device(state: &Arc<AppState>, device_id: &str, user_id: &str) {
    let conn = state.db.get().unwrap();
    conn.execute(
        "INSERT INTO devices (id, user_id, device_name, paired_at, last_seen_at) \
         VALUES (?1, ?2, 'test device', 1, 1)",
        rusqlite::params![device_id, user_id],
    )
    .unwrap();
}

#[allow(clippy::needless_pass_by_value)] // callers pass owned `json!` literals
fn json_request(
    uri: &str,
    token: Option<&str>,
    api_key: Option<&str>,
    body: Value,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if let Some(api_key) = api_key {
        builder = builder.header("x-juicehost-api-key", api_key);
    }
    builder
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

async fn json_response(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    (status, value)
}

fn register_body(id: &str, filename: &str) -> Value {
    json!({
        "id": id,
        "filename": filename,
        "mime_type": "video/mp4",
        "size_bytes": 1_234,
        "ttl_hours": 24.0,
    })
}

async fn register(app: &Router, device_id: &str, body: Value) -> (StatusCode, Value) {
    json_response(
        app,
        json_request(
            "/api/peer/register",
            Some(&device_token(device_id, "user-1")),
            None,
            body,
        ),
    )
    .await
}

#[tokio::test]
async fn register_creates_metadata_only_row_with_relay_share_links() {
    let state = common::test_state();
    pair_device(&state, "device-1", "user-1");
    let app = common::mock_router(state.clone());

    let (status, body) = register(&app, "device-1", register_body("peer-file-1", "clip.mp4")).await;
    assert_eq!(status, StatusCode::OK, "unexpected response: {body}");
    assert_eq!(body["id"], "peer-file-1");
    assert!(body["expires_at"].as_i64().unwrap() > chrono::Utc::now().timestamp());
    assert_eq!(body["url"], "https://ff.juicey.dev/v/peer-file-1.mp4");
    assert_eq!(body["raw_url"], "https://ff.juicey.dev/f/peer-file-1.mp4");
    assert_eq!(
        body["download_url"],
        "https://ff.juicey.dev/d/peer-file-1.mp4"
    );
    assert_ne!(body["delete_token"].as_str().unwrap(), "");

    let conn = state.db.get().unwrap();
    let row: (String, String, String, String, String, i64) = conn
        .query_row(
            "SELECT tier, status, storage_path, storage_host, peer_device_id, last_seen \
             FROM files WHERE id = 'peer-file-1'",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(row.0, FileRecord::TIER_PEER);
    assert_eq!(row.1, FileRecord::STATUS_PEER);
    assert_eq!(row.2, "peer:peer-file-1");
    assert_eq!(row.3, "https://ff.juicey.dev");
    assert_eq!(row.4, "device-1");
    assert!(row.5 > 0, "last_seen should be initialized");
}

#[tokio::test]
async fn reannounce_keeps_the_existing_delete_token() {
    let state = common::test_state();
    pair_device(&state, "device-1", "user-1");
    let app = common::mock_router(state.clone());

    let (status, first) =
        register(&app, "device-1", register_body("peer-file-1", "clip.mp4")).await;
    assert_eq!(status, StatusCode::OK, "unexpected response: {first}");
    let token = first["delete_token"].as_str().unwrap().to_owned();

    // Re-announcing our own row with updated metadata must not rotate the
    // delete token, or previously saved owned-files pairs stop resolving.
    let mut body = register_body("peer-file-1", "clip.mp4");
    body["size_bytes"] = json!(999);
    let (status, second) = register(&app, "device-1", body).await;
    assert_eq!(status, StatusCode::OK, "unexpected response: {second}");
    assert_eq!(second["delete_token"].as_str().unwrap(), token);

    let conn = state.db.get().unwrap();
    let stored: String = conn
        .query_row(
            "SELECT delete_token FROM files WHERE id = 'peer-file-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored, token);
}

#[tokio::test]
async fn register_requires_a_paired_device_token() {
    let state = common::test_state();
    let app = common::mock_router(state.clone());

    // Valid signature, but the device was never paired.
    let (status, _) = register(
        &app,
        "ghost-device",
        register_body("peer-file-1", "clip.mp4"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // No bearer token at all.
    let (status, _) = json_response(
        &app,
        json_request(
            "/api/peer/register",
            None,
            None,
            register_body("peer-file-2", "clip.mp4"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn register_rejects_protected_files_and_invalid_metadata() {
    let state = common::test_state();
    pair_device(&state, "device-1", "user-1");
    let app = common::mock_router(state.clone());

    let mut protected = register_body("peer-file-1", "clip.mp4");
    protected["password"] = json!("secret");
    assert_eq!(
        register(&app, "device-1", protected).await.0,
        StatusCode::BAD_REQUEST
    );

    let mut encrypted = register_body("peer-file-2", "clip.mp4");
    encrypted["is_encrypted"] = json!(true);
    assert_eq!(
        register(&app, "device-1", encrypted).await.0,
        StatusCode::BAD_REQUEST
    );

    let mut negative_size = register_body("peer-file-3", "clip.mp4");
    negative_size["size_bytes"] = json!(-1);
    assert_eq!(
        register(&app, "device-1", negative_size).await.0,
        StatusCode::BAD_REQUEST
    );

    let mut bad_ttl = register_body("peer-file-4", "clip.mp4");
    bad_ttl["ttl_hours"] = json!(0.0);
    assert_eq!(
        register(&app, "device-1", bad_ttl).await.0,
        StatusCode::BAD_REQUEST
    );

    assert_eq!(
        register(&app, "device-1", register_body("x", "clip.mp4"))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn register_rejects_ids_claimed_by_another_device_or_by_a_standard_file() {
    let state = common::test_state();
    pair_device(&state, "device-1", "user-1");
    // `register` mints tokens for user-1; both devices share that user here.
    pair_device(&state, "device-2", "user-1");
    let app = common::mock_router(state.clone());

    assert_eq!(
        register(&app, "device-1", register_body("peer-file-1", "clip.mp4"))
            .await
            .0,
        StatusCode::OK
    );
    let (status, body) = register(&app, "device-2", register_body("peer-file-1", "clip.mp4")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "unexpected response: {body}");

    // A non-peer file id cannot be repurposed as a peer file.
    {
        let conn = state.db.get().unwrap();
        let now = chrono::Utc::now().timestamp();
        db::insert_file(
            &conn,
            &FileRecord::new(
                "standard-file".into(),
                "clip.mp4".into(),
                "video/mp4".into(),
                10,
                "token".into(),
                now,
                now + 3600,
                None,
                None,
            ),
        )
        .unwrap();
    }
    let (status, _) = register(&app, "device-1", register_body("standard-file", "clip.mp4")).await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn authorize_accepts_owned_peer_files_and_rejects_foreign_or_missing_ones() {
    let state = common::test_state();
    pair_device(&state, "device-1", "user-1");
    pair_device(&state, "device-2", "user-2");
    let app = common::mock_router(state.clone());
    assert_eq!(
        register(&app, "device-1", register_body("peer-file-1", "clip.mp4"))
            .await
            .0,
        StatusCode::OK
    );

    let (status, body) = json_response(
        &app,
        json_request(
            "/internal/peer/authorize",
            None,
            Some("test-key"),
            json!({
                "device_id": "device-1",
                "user_id": "user-1",
                "ids": ["peer-file-1", "missing-file"],
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "unexpected response: {body}");
    assert_eq!(body["accepted"], json!(["peer-file-1"]));
    assert_eq!(body["rejected"][0]["id"], "missing-file");
    assert_eq!(body["rejected"][0]["reason"], "not found");

    let (status, body) = json_response(
        &app,
        json_request(
            "/internal/peer/authorize",
            None,
            Some("test-key"),
            json!({
                "device_id": "device-2",
                "user_id": "user-2",
                "ids": ["peer-file-1"],
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["accepted"], json!([]));
    assert_eq!(body["rejected"][0]["reason"], "owned by another device");
}

#[tokio::test]
async fn authorize_requires_the_internal_api_key_and_a_well_formed_body() {
    let state = common::test_state();
    let app = common::mock_router(state.clone());
    let body = json!({ "device_id": "device-1", "user_id": "user-1", "ids": [] });

    assert_eq!(
        json_response(
            &app,
            json_request("/internal/peer/authorize", None, None, body.clone()),
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        json_response(
            &app,
            json_request(
                "/internal/peer/authorize",
                None,
                Some("wrong-key"),
                body.clone()
            ),
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );

    let malformed = json!({ "device_id": "", "user_id": "user-1", "ids": [] });
    assert_eq!(
        json_response(
            &app,
            json_request(
                "/internal/peer/authorize",
                None,
                Some("test-key"),
                malformed,
            ),
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn owned_files_expose_peer_status_and_relay_urls() {
    let state = common::test_state();
    pair_device(&state, "device-1", "user-1");
    let app = common::mock_router(state.clone());
    let (status, registered) =
        register(&app, "device-1", register_body("peer-file-1", "clip.mp4")).await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = json_response(
        &app,
        json_request(
            "/api/owned-files",
            None,
            None,
            json!({
                "pairs": [{
                    "id": "peer-file-1",
                    "token": registered["delete_token"],
                }],
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "unexpected response: {body}");
    assert_eq!(body["files"].as_array().unwrap().len(), 1);
    let file = &body["files"][0];
    assert_eq!(file["id"], "peer-file-1");
    assert_eq!(file["status"], "peer");
    assert_eq!(file["storage_host"], "https://ff.juicey.dev");
    assert_eq!(file["url"], "https://ff.juicey.dev/v/peer-file-1.mp4");
    assert_eq!(
        file["download_url"],
        "https://ff.juicey.dev/d/peer-file-1.mp4"
    );
}

#[tokio::test]
async fn heartbeat_and_revoke_manage_peer_rows() {
    let state = common::test_state();
    pair_device(&state, "device-1", "user-1");
    pair_device(&state, "device-2", "user-2");
    let app = common::mock_router(state.clone());
    assert_eq!(
        register(&app, "device-1", register_body("peer-file-1", "clip.mp4"))
            .await
            .0,
        StatusCode::OK
    );

    let (status, body) = json_response(
        &app,
        json_request(
            "/api/peer/heartbeat",
            Some(&device_token("device-1", "user-1")),
            None,
            json!({ "ids": ["peer-file-1"] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "unexpected response: {body}");
    assert!(body["updated"].as_u64().unwrap() >= 1);

    // A different device cannot revoke this file.
    let other_revoke = Request::builder()
        .method("POST")
        .uri("/api/peer/revoke/peer-file-1")
        .header(
            "authorization",
            format!("Bearer {}", device_token("device-2", "user-2")),
        )
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(other_revoke).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );

    let revoke = Request::builder()
        .method("POST")
        .uri("/api/peer/revoke/peer-file-1")
        .header(
            "authorization",
            format!("Bearer {}", device_token("device-1", "user-1")),
        )
        .body(Body::empty())
        .unwrap();
    let (status, body) = json_response(&app, revoke).await;
    assert_eq!(status, StatusCode::OK, "unexpected response: {body}");
    assert_eq!(body["revoked"], true);

    let conn = state.db.get().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM files WHERE id = 'peer-file-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}
