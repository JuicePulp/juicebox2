use std::{sync::Arc, time::Duration};

use axum::{
    body::Body,
    extract::{Request, State},
    middleware::Next,
    response::Response,
};

use crate::{
    proxy::{inbound_cookie, redirect_to},
    state::AppState,
};

pub async fn admin_guard(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
    next: Next,
) -> Response<Body> {
    let path = req.uri().path().to_owned();
    let is_admin = path == "/admin" || path.starts_with("/admin/");
    let is_login = path == "/admin/login" || path.starts_with("/admin/login");
    if !is_admin || is_login {
        return next.run(req).await;
    }

    if check_admin_session(&state, inbound_cookie(req.headers())).await {
        next.run(req).await
    } else {
        redirect_to("/admin/login")
    }
}

async fn check_admin_session(state: &AppState, cookie: Option<String>) -> bool {
    let url = format!("{}/api/admin/check", state.config.api_internal_url);
    let mut request = state.http.get(&url);
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    match tokio::time::timeout(Duration::from_secs(3), request.send()).await {
        Ok(Ok(response)) => response.status().is_success(),
        _ => false,
    }
}
