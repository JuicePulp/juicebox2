mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use juiceback::routes::build_router;
use tower::ServiceExt;

#[tokio::test]
async fn admin_login_wrong_password() {
    let state = common::test_state();
    let hash = juiceback::auth::hash_password("correct-horse").unwrap();
    {
        let conn = state.db.get().unwrap();
        conn.execute(
            "INSERT INTO admins (username, password_hash) VALUES (?1, ?2)",
            rusqlite::params!["admin", hash],
        )
        .unwrap();
    }
    let app = build_router(state);

    let resp = app
        .clone()
        .oneshot(common::with_connect_info(
            Request::builder()
                .method("POST")
                .uri("/api/admin/login")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("username=admin&password=wrong"))
                .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // The correct password proves the account exists, so the 401 above came
    // from the real password-verification branch and not the unknown-user path.
    let resp = app
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
}

#[tokio::test]
async fn admin_login_no_user() {
    let state = common::test_state();
    let app = build_router(state);

    let resp = app
        .oneshot(common::with_connect_info(
            Request::builder()
                .method("POST")
                .uri("/api/admin/login")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("username=nonexistent&password=pass"))
                .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
