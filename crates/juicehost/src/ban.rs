use std::sync::Arc;

use crate::state::AppState;

pub async fn load_ban_file(state: &Arc<AppState>) {
    let pepper = state.ban_list.pepper();
    if let Some(ref path) = state.ban_list_file {
        state.ban_list.load_file(path, &pepper);
    }
}

pub async fn fetch_snapshot_once(state: &Arc<AppState>) {
    let Some(ref backend) = state.backend_url else {
        return;
    };
    if state.api_key.is_empty() {
        return;
    }
    let url = format!("{backend}/internal/ban-snapshot");
    let resp = match state
        .backend_client
        .get(&url)
        .header("x-juicehost-api-key", &state.api_key)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            tracing::warn!("ban snapshot fetch from {url} failed: {e}");
            return;
        }
    };
    if !resp.status().is_success() {
        tracing::warn!("ban snapshot fetch from {url} failed: {}", resp.status());
        return;
    }
    let body: serde_json::Value = match resp.json().await {
        Ok(body) => body,
        Err(e) => {
            tracing::warn!("ban snapshot fetch from {url} invalid: {e}");
            return;
        }
    };
    let pepper = body
        .get("pepper")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if pepper.is_empty() {
        tracing::warn!("ban snapshot fetch from {url} returned empty pepper");
        return;
    }
    let hashes: Vec<String> = body
        .get("hashes")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|h| h.as_str().map(std::string::ToString::to_string))
                .collect()
        })
        .unwrap_or_default();
    state.ban_list.set_snapshot(&pepper, hashes);
    tracing::info!("loaded {} bans from {url}", state.ban_list.len());
}
