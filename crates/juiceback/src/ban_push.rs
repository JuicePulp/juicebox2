use std::{sync::Arc, time::Duration};

use crate::state::AppState;

/// No-op without the internal API key; per-host failures are only logged.
async fn post_to_hosts(state: &Arc<AppState>, path: &str, body: serde_json::Value) {
    if state.config.juicehost_api_key.is_empty() {
        return;
    }
    for host in state.hosts.ban_targets() {
        let url = format!("{host}{path}");
        let resp = state
            .http
            .post(&url)
            .headers(state.juicehost_headers.clone())
            .timeout(Duration::from_secs(5))
            .json(&body)
            .send()
            .await;
        match resp {
            Ok(resp) if resp.status().is_success() => {}
            Ok(resp) => {
                tracing::warn!("ban push to {url} failed: {}", resp.status());
            }
            Err(e) => {
                tracing::warn!("ban push to {url} failed: {e}");
            }
        }
    }
}

/// Fire-and-forget, so a ban request never waits on the network.
pub fn push_ban(state: &Arc<AppState>, hash: &str) {
    let state = Arc::clone(state);
    let body = serde_json::json!({
        "pepper": state.config.ip_pepper,
        "hash": hash,
    });
    tokio::spawn(async move {
        post_to_hosts(&state, "/internal/bans/ban", body).await;
    });
}

pub fn push_unban(state: &Arc<AppState>, hash: &str) {
    let state = Arc::clone(state);
    let body = serde_json::json!({
        "pepper": state.config.ip_pepper,
        "hash": hash,
    });
    tokio::spawn(async move {
        post_to_hosts(&state, "/internal/bans/unban", body).await;
    });
}

pub fn push_snapshot(state: &Arc<AppState>) {
    let state = Arc::clone(state);
    tokio::spawn(async move {
        let hashes: Vec<String> = state.banned_ips.iter().map(|e| e.key().clone()).collect();
        let body = serde_json::json!({
            "pepper": state.config.ip_pepper,
            "hashes": hashes,
        });
        post_to_hosts(&state, "/internal/bans/snapshot", body).await;
    });
}
