mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use juiceback::routes::build_router;
use tower::ServiceExt;

fn api_key_header() -> (&'static str, &'static str) {
    ("x-juicehost-api-key", "test-key")
}

async fn seed_admin_and_login(
    state: &std::sync::Arc<juiceback::state::AppState>,
    app: &axum::Router,
) -> String {
    let hash = juiceback::auth::hash_password("correct-horse").unwrap();
    {
        let conn = state.db.get().unwrap();
        conn.execute(
            "INSERT INTO admins (username, password_hash) VALUES (?1, ?2)",
            rusqlite::params!["admin", hash],
        )
        .unwrap();
    }
    let resp = app
        .clone()
        .oneshot(common::with_connect_info(
            Request::builder()
                .method("POST")
                .uri("/api/admin/login")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("username=admin&password=correct-horse"))
                .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let set_cookie = resp
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    set_cookie.split(';').next().unwrap().trim().to_owned()
}

fn json_post(uri: &str, body: serde_json::Value, extra: Option<(&str, &str)>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json");
    if let Some((k, v)) = extra {
        builder = builder.header(k, v);
    }
    common::with_connect_info(builder.body(Body::from(body.to_string())).unwrap())
}

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn stats_hit_requires_api_key() {
    let state = common::test_state();
    let app = build_router(state);
    let (key, val) = api_key_header();

    // No key at all.
    let resp = app
        .clone()
        .oneshot(json_post(
            "/internal/stats/hit",
            serde_json::json!({"file_id": "abc", "kind": "view", "ip": "10.0.0.1"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Wrong key.
    let resp = app
        .clone()
        .oneshot(json_post(
            "/internal/stats/hit",
            serde_json::json!({"file_id": "abc", "kind": "view", "ip": "10.0.0.1"}),
            Some((key, "wrong")),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Unknown kind rejected even with a valid key.
    let resp = app
        .clone()
        .oneshot(json_post(
            "/internal/stats/hit",
            serde_json::json!({"file_id": "abc", "kind": "explode", "ip": "10.0.0.1"}),
            Some((key, val)),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn stats_hits_count_unique_people_not_range_chunks() {
    let state = common::test_state();
    let app = build_router(state.clone());
    let (key, val) = api_key_header();

    // Same person viewing repeatedly (e.g. video range chunks) = 1 viewer, N hits.
    for _ in 0..3 {
        let resp = app
            .clone()
            .oneshot(json_post(
                "/internal/stats/hit",
                serde_json::json!({"file_id": "file1", "kind": "view", "ip": "10.0.0.1"}),
                Some((key, val)),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }
    // A second person views + downloads.
    let resp = app
        .clone()
        .oneshot(json_post(
            "/internal/stats/hit",
            serde_json::json!({"file_id": "file1", "kind": "view", "ip": "10.0.0.2"}),
            Some((key, val)),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = app
        .clone()
        .oneshot(json_post(
            "/internal/stats/hit",
            serde_json::json!({"file_id": "file1", "kind": "download", "ip": "10.0.0.2"}),
            Some((key, val)),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let cookie = seed_admin_and_login(&state, &app).await;
    let resp = app
        .clone()
        .oneshot(common::with_connect_info(
            Request::builder()
                .uri("/api/admin/stats/overview")
                .header("cookie", cookie.clone())
                .body(Body::empty())
                .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["file_viewers"], 2);
    assert_eq!(body["file_views"], 4);
    assert_eq!(body["file_downloads"], 1);
}

#[tokio::test]
async fn stats_overview_counts_visitors_uploaders_and_file_views() {
    let state = common::test_state();
    let app = build_router(state.clone());
    let (key, val) = api_key_header();

    // Overview requires admin auth.
    let resp = app
        .clone()
        .oneshot(common::with_connect_info(
            Request::builder()
                .uri("/api/admin/stats/overview")
                .body(Body::empty())
                .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Two site visitors (one visits twice), two file viewers.
    for (ip, times) in [("192.0.2.1", 2), ("192.0.2.2", 1)] {
        for _ in 0..times {
            let resp = app
                .clone()
                .oneshot(json_post(
                    "/internal/stats/visit",
                    serde_json::json!({"ip": ip}),
                    Some((key, val)),
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        }
    }
    for ip in ["192.0.2.1", "192.0.2.3"] {
        let resp = app
            .clone()
            .oneshot(json_post(
                "/internal/stats/hit",
                serde_json::json!({"file_id": "vid1", "kind": "view", "ip": ip}),
                Some((key, val)),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    // One uploader identity with a file on record.
    {
        let conn = state.db.get().unwrap();
        conn.execute(
            "INSERT INTO files (id, filename, mime_type, size_bytes, storage_path, delete_token, uploaded_at, expires_at)
             VALUES ('vid1', 'a.mp4', 'video/mp4', 10, 'remote-vid1', 'tok', 1, 9999999999)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO client_files (client_key, file_id, delete_token) VALUES ('user-1', 'vid1', 'tok')",
            [],
        )
        .unwrap();
    }

    let cookie = seed_admin_and_login(&state, &app).await;
    let resp = app
        .clone()
        .oneshot(common::with_connect_info(
            Request::builder()
                .uri("/api/admin/stats/overview")
                .header("cookie", cookie.clone())
                .body(Body::empty())
                .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["site_visitors"], 2);
    assert_eq!(body["site_visits"], 3);
    assert_eq!(body["uploaders"], 1);
    assert_eq!(body["file_viewers"], 2);
    assert_eq!(body["file_views"], 2);
    assert_eq!(body["file_downloads"], 0);

    // The files list carries per-file counters.
    let resp = app
        .clone()
        .oneshot(common::with_connect_info(
            Request::builder()
                .uri("/api/admin/files?limit=50")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let item = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == "vid1")
        .unwrap();
    assert_eq!(item["viewers"], 2);
    assert_eq!(item["views"], 2);
    assert_eq!(item["downloaders"], 0);
    assert!(item["last_viewed_at"].as_i64().unwrap() > 0);
}
