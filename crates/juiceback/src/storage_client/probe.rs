use std::sync::Arc;

use super::target::{require_juicehost_url, resolve_juicehost_target};
use crate::state::AppState;

/// Query juicehost to confirm a file physically exists on disk and report its
/// stored size. Returns `Ok(Some(size_bytes))` when present, `Ok(None)` when
/// the file is not on juicehost, and `Err(String)` when juicehost is
/// unreachable or returned an unexpected status. Used to verify ultrafast
/// uploads actually landed before marking them ready.
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
    let target = resolve_juicehost_target(state, custom_host)
        .await
        .map_err(|e| e.to_string())?;

    let mut headers = target.headers.clone();
    if let Some(cap) = capability {
        headers.insert(
            "x-juicehost-file-capability",
            cap.parse::<reqwest::header::HeaderValue>()
                .map_err(|_| "invalid file capability".to_string())?,
        );
    }

    let resp = target
        .client
        .get(format!("{}/internal/file/{}/stat", target.base_url, id))
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
