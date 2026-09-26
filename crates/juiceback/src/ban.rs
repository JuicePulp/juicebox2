use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
};

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::Request,
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::{
    error::AppError,
    state::AppState,
    utils::{ClientIp, client_ip, hash_ip_for_ban},
};

pub const STATUS_PATH: &str = "/api/ban-status";

pub fn check(state: &Arc<AppState>, raw_ip: &str) -> bool {
    state.is_banned(&hash_ip_for_ban(raw_ip, &state.config.ip_pepper))
}

pub fn snapshot(state: &Arc<AppState>) -> (String, Vec<String>) {
    let hashes: Vec<String> = state.banned_ips.iter().map(|e| e.key().clone()).collect();
    (state.config.ip_pepper.clone(), hashes)
}

pub async fn middleware(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if req.uri().path() == STATUS_PATH {
        return next.run(req).await;
    }

    let raw_ip = req
        .extensions()
        .get::<ClientIp>()
        .map(|value| value.0)
        .filter(|ip| !ip.is_unspecified())
        .unwrap_or_else(|| {
            let peer = req
                .extensions()
                .get::<ConnectInfo<SocketAddr>>()
                .map(|value| value.0.ip())
                .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
            client_ip(req.headers(), peer, &state)
        })
        .to_string();
    if check(&state, &raw_ip) {
        return AppError::Forbidden("you are banned".into()).into_response();
    }

    next.run(req).await
}
