use std::{net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    body::Body,
    extract::{ConnectInfo, State},
    http::Request,
    middleware,
    response::Response,
};

use crate::state::AppState;

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

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct HitKey {
    file_id: String,
    kind: &'static str,
    ip: String,
    surface: &'static str,
}

const FLUSH_INTERVAL: Duration = Duration::from_secs(1);
const FLUSH_BATCH_LIMIT: usize = 1_000;

pub(crate) fn report_file_hit(
    state: &AppState,
    file_id: &str,
    kind: &'static str,
    ip: &str,
    surface: &'static str,
) {
    if state.backend_url.is_none() {
        return;
    }
    let key = HitKey {
        file_id: file_id.to_owned(),
        kind,
        ip: ip.to_owned(),
        surface,
    };
    *state.hit_buffer.entry(key).or_insert(0) += 1;
}

pub fn start_flusher(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(FLUSH_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            flush_once(&state).await;
        }
    });
}

async fn flush_once(state: &AppState) {
    let Some(backend_url) = state.backend_url.as_deref() else {
        state.hit_buffer.clear();
        return;
    };
    let keys: Vec<HitKey> = state
        .hit_buffer
        .iter()
        .take(FLUSH_BATCH_LIMIT)
        .map(|entry| entry.key().clone())
        .collect();
    let mut drained = Vec::with_capacity(keys.len());
    for key in keys {
        if let Some((_, count)) = state.hit_buffer.remove(&key)
            && count > 0
        {
            drained.push((key, count));
        }
    }
    if drained.is_empty() {
        return;
    }

    let hits: Vec<serde_json::Value> = drained
        .iter()
        .map(|(key, count)| {
            serde_json::json!({
                "file_id": &key.file_id,
                "kind": key.kind,
                "ip": &key.ip,
                "surface": key.surface,
                "count": count,
            })
        })
        .collect();
    let url = format!("{backend_url}/internal/stats/hit-batch");
    let mut request = state
        .backend_client
        .post(url)
        .json(&serde_json::json!({ "hits": hits }))
        .timeout(Duration::from_secs(3));
    if !state.api_key.is_empty() {
        request = request.header("x-juicehost-api-key", &state.api_key);
    }
    match request.send().await {
        Ok(resp) if resp.status().is_success() => {}
        Ok(resp) => {
            tracing::debug!(status = %resp.status(), "stats hit batch not recorded");
        }
        Err(error) => {
            tracing::debug!(error = %error, "stats hit batch not recorded; retrying");
            for (key, count) in drained {
                *state.hit_buffer.entry(key).or_insert(0) += count;
            }
        }
    }
}
