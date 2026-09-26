use std::sync::Arc;

use super::{client::StorageClient, target::require_juicehost_url};
use crate::state::AppState;

pub async fn stat_file_on_juicehost(
    state: &Arc<AppState>,
    id: &str,
    host: Option<&str>,
    capability: Option<&str>,
) -> Result<Option<u64>, String> {
    let custom_host = host.map(str::trim).filter(|h| !h.is_empty());
    if custom_host.is_none() {
        require_juicehost_url(&state.config.juicehost_url).map_err(|e| e.to_string())?;
    }
    let client = StorageClient::resolve(state, custom_host).await?;

    let headers = client.headers_with(capability)?;

    let resp = client
        .http()
        .get(format!("{}/internal/file/{}/stat", client.base_url(), id))
        .headers(headers)
        .send()
        .await
        .map_err(|e| format!("juicehost stat request error: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "juicehost stat failed: status={status} body={body}"
        ));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("juicehost stat parse error: {e}"))?;
    match json.get("exists").and_then(serde_json::Value::as_bool) {
        Some(true) => Ok(json.get("size_bytes").and_then(serde_json::Value::as_u64)),
        _ => Ok(None),
    }
}
