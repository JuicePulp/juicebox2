mod common;

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use juiceback::{config::Config, db, state::AppState};
use serde_json::{Value, json};
use tower::ServiceExt;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path as exact_path, path_regex},
};

fn state_with_juicehost(url: &str) -> Arc<AppState> {
    let mut config: Config = common::test_config();
    config.juicehost_url = url.to_string();
    config.public_juicehost_url = url.to_string();
    config.direct_upload_enabled = true;
    common::state_from_config(config)
}

fn session_cookie(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get_all(axum::http::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|value| {
            let pair = value.split(';').next()?.trim();
            pair.starts_with("jb_session=").then(|| pair.to_string())
        })
}

struct Reply {
    status: StatusCode,
    body: Value,
    cookie: Option<String>,
}

async fn send(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    cookie: Option<&str>,
    delete_token: Option<&str>,
) -> Reply {
    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", cookie);
    }
    if let Some(token) = delete_token {
        builder = builder.header("x-delete-token", token);
    }
    let request = builder
        .body(body.map_or_else(Body::empty, |value| Body::from(value.to_string())))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let cookie = session_cookie(response.headers());
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Reply {
        status,
        body,
        cookie,
    }
}

async fn record(state: &Arc<AppState>, id: &str) -> db::FileRecord {
    let lookup = id.to_string();
    state
        .db_call("test_get_file", move |db| db::get_file(db, &lookup))
        .await
        .unwrap()
        .unwrap()
}

async fn mock_live_registration(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path_regex("/internal/live/.*/register"))
        .respond_with(ResponseTemplate::new(200))
        .mount(server)
        .await;
}

#[tokio::test]
async fn direct_reserve_returns_its_persisted_delete_token() {
    let state = common::test_state();
    let app = common::mock_router(Arc::clone(&state));

    let first = send(
        &app,
        "POST",
        "/upload/direct/reserve",
        Some(json!({
            "filename": "a.txt",
            "mime_type": "text/plain",
            "file_size": 64,
        })),
        None,
        None,
    )
    .await;
    assert_eq!(first.status, StatusCode::OK, "{:?}", first.body);
    let token = first.body["delete_token"].as_str().unwrap().to_string();
    assert!(uuid::Uuid::parse_str(&token).is_ok(), "token={token}");
    let file_id = first.body["file_id"].as_str().unwrap().to_string();
    assert_eq!(record(&state, &file_id).await.delete_token, token);

    let second = send(
        &app,
        "POST",
        "/upload/direct/reserve",
        Some(json!({
            "filename": "b.txt",
            "mime_type": "text/plain",
            "file_size": 64,
        })),
        first.cookie.as_deref(),
        None,
    )
    .await;
    assert_eq!(second.status, StatusCode::OK, "{:?}", second.body);
    let other_token = second.body["delete_token"].as_str().unwrap().to_string();
    assert!(uuid::Uuid::parse_str(&other_token).is_ok());
    assert_ne!(token, other_token);
    let other_file_id = second.body["file_id"].as_str().unwrap().to_string();
    assert_eq!(
        record(&state, &other_file_id).await.delete_token,
        other_token
    );
}

#[tokio::test]
async fn parallel_reserve_returns_the_parent_delete_token() {
    let server = MockServer::start().await;
    mock_live_registration(&server).await;
    let state = state_with_juicehost(&server.uri());
    let app = common::mock_router(Arc::clone(&state));

    let reply = send(
        &app,
        "POST",
        "/upload/direct/parallel/reserve",
        Some(json!({
            "filename": "big.bin",
            "mime_type": "application/octet-stream",
            "file_size": 1024,
            "num_parts": 2,
        })),
        None,
        None,
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{:?}", reply.body);
    let token = reply.body["delete_token"].as_str().unwrap().to_string();
    assert!(uuid::Uuid::parse_str(&token).is_ok(), "token={token}");
    let file_id = reply.body["file_id"].as_str().unwrap().to_string();
    assert_eq!(record(&state, &file_id).await.delete_token, token);

    let parts = reply.body["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 2);
    assert!(parts.iter().all(|part| {
        part["ticket"]
            .as_str()
            .is_some_and(|ticket| !ticket.is_empty())
            && part["part_id"].as_str().is_some_and(|id| !id.is_empty())
            && part["upload_url"]
                .as_str()
                .is_some_and(|url| !url.is_empty())
    }));
    let total: u64 = parts
        .iter()
        .map(|part| part["part_size"].as_u64().unwrap())
        .sum();
    assert_eq!(total, 1024);
}

#[tokio::test]
async fn direct_complete_returns_the_token_and_the_token_can_delete() {
    let server = MockServer::start().await;
    let state = state_with_juicehost(&server.uri());
    let app = common::mock_router(Arc::clone(&state));

    let reserve = send(
        &app,
        "POST",
        "/upload/direct/reserve",
        Some(json!({
            "filename": "a.txt",
            "mime_type": "text/plain",
            "file_size": 64,
        })),
        None,
        None,
    )
    .await;
    assert_eq!(reserve.status, StatusCode::OK, "{:?}", reserve.body);
    let file_id = reserve.body["file_id"].as_str().unwrap().to_string();
    let token = reserve.body["delete_token"].as_str().unwrap().to_string();
    let ticket = reserve.body["ticket"].as_str().unwrap().to_string();
    let cookie = reserve.cookie.expect("reserve response sets a session");

    Mock::given(method("GET"))
        .and(exact_path(format!("/internal/file/{file_id}/stat")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"exists": true, "size_bytes": 64})),
        )
        .mount(&server)
        .await;

    let complete = send(
        &app,
        "POST",
        "/upload/direct/complete",
        Some(json!({"file_id": file_id, "ticket": ticket})),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(complete.status, StatusCode::OK, "{:?}", complete.body);
    assert_eq!(complete.body["id"], file_id);
    assert_eq!(complete.body["delete_token"], token);

    let wrong = send(
        &app,
        "DELETE",
        &format!("/file/{file_id}"),
        None,
        None,
        Some("00000000-0000-0000-0000-000000000000"),
    )
    .await;
    assert_eq!(wrong.status, StatusCode::FORBIDDEN);

    Mock::given(method("DELETE"))
        .and(exact_path(format!("/internal/file/{file_id}")))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let deleted = send(
        &app,
        "DELETE",
        &format!("/file/{file_id}"),
        None,
        None,
        Some(&token),
    )
    .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT, "{:?}", deleted.body);
    assert!(
        db::get_file(&state.db.get().unwrap(), &file_id)
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn parallel_complete_returns_the_token_and_the_token_can_delete() {
    let server = MockServer::start().await;
    mock_live_registration(&server).await;
    let state = state_with_juicehost(&server.uri());
    let app = common::mock_router(Arc::clone(&state));

    let reserve = send(
        &app,
        "POST",
        "/upload/direct/parallel/reserve",
        Some(json!({
            "filename": "big.bin",
            "mime_type": "application/octet-stream",
            "file_size": 1024,
            "num_parts": 2,
        })),
        None,
        None,
    )
    .await;
    assert_eq!(reserve.status, StatusCode::OK, "{:?}", reserve.body);
    let file_id = reserve.body["file_id"].as_str().unwrap().to_string();
    let token = reserve.body["delete_token"].as_str().unwrap().to_string();
    let ticket = reserve.body["ticket"].as_str().unwrap().to_string();
    let cookie = reserve.cookie.expect("reserve response sets a session");
    let part_ids: Vec<String> = reserve.body["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|part| part["part_id"].as_str().unwrap().to_string())
        .collect();

    Mock::given(method("GET"))
        .and(exact_path(format!("/internal/file/{file_id}/stat")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"exists": true, "size_bytes": 1024})),
        )
        .mount(&server)
        .await;

    let complete = send(
        &app,
        "POST",
        "/upload/direct/parallel/complete",
        Some(json!({
            "file_id": file_id,
            "ticket": ticket,
            "part_ids": part_ids,
        })),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(complete.status, StatusCode::OK, "{:?}", complete.body);
    assert_eq!(complete.body["id"], file_id);
    assert_eq!(complete.body["delete_token"], token);

    let wrong = send(
        &app,
        "DELETE",
        &format!("/file/{file_id}"),
        None,
        None,
        Some("00000000-0000-0000-0000-000000000000"),
    )
    .await;
    assert_eq!(wrong.status, StatusCode::FORBIDDEN);

    Mock::given(method("DELETE"))
        .and(exact_path(format!("/internal/file/{file_id}")))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let deleted = send(
        &app,
        "DELETE",
        &format!("/file/{file_id}"),
        None,
        None,
        Some(&token),
    )
    .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT, "{:?}", deleted.body);
}
