use std::{net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    body::Body,
    extract::{ConnectInfo, State},
    http::Request,
    middleware,
    response::Response,
};

use crate::state::AppState;

/// Viewer IP resolved once per file-serving request and stashed as an
/// extension. `None` when there is no socket peer (in-process calls):
/// without a peer the trusted-proxy rules cannot apply, and stats must
/// never fall back to trivially spoofable headers, so those hits are
/// simply not counted.
#[derive(Clone)]
pub struct ViewerIp(pub Option<String>);

pub(crate) async fn viewer_ip_middleware(
    State(state): State<Arc<AppState>>,
    mut req: Request<Body>,
    next: middleware::Next,
) -> Response {
    let ip = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|peer| peer.0.ip())
        .map(|peer| {
            juiceutils::proxy::client_ip(req.headers(), peer, &state.trusted_proxy_cidrs)
                .to_string()
        });
    req.extensions_mut().insert(ViewerIp(ip));
    next.run(req).await
}

/// Best-effort visitor hit report to juiceback (`POST
/// /internal/stats/hit`). Fire-and-forget: never blocks or fails the
/// response being served; unreachable backends just drop the hit (debug
/// log). No-op when no backend is configured (backendless mode).
pub(crate) fn report_file_hit(state: &AppState, file_id: &str, kind: &'static str, ip: &str) {
    let Some(ref backend_url) = state.backend_url else {
        return;
    };
    let url = format!("{backend_url}/internal/stats/hit");
    let client = state.backend_client.clone();
    let api_key = state.api_key.clone();
    let body = serde_json::json!({
        "file_id": file_id,
        "kind": kind,
        "ip": ip,
    });
    tokio::spawn(async move {
        let mut request = client.post(url).json(&body).timeout(Duration::from_secs(2));
        if !api_key.is_empty() {
            request = request.header("x-juicehost-api-key", api_key);
        }
        match request.send().await {
            Ok(resp) if resp.status().is_success() => {}
            Ok(resp) => tracing::debug!(status = %resp.status(), "stats hit not recorded"),
            Err(error) => tracing::debug!(error = %error, "stats hit not recorded"),
        }
    });
}
