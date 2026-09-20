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
    let app = build_router(state);

    let resp = app
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
