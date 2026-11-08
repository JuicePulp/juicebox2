use std::sync::Arc;

use crate::state::AppState;

use super::client::StorageClient;

/// Tell a content host about a quick-link upload before its bytes arrive.
/// This is best-effort: if the host is older or unavailable the upload still
/// proceeds and the preview falls back to the normal uploading notice.
pub async fn register_live_upload(
    state: &Arc<AppState>,
    target_id: &str,
    filename: &str,
    mime_type: &str,
    total_size: u64,
    parts: &[(String, u64, u64)],
    host: Option<&str>,
    capability: Option<&str>,
) -> Result<(), String> {
    if total_size == 0 || parts.is_empty() {
        return Ok(());
    }

    let client = StorageClient::resolve(state, host).await?;
    let url = format!(
        "{}/internal/live/{}/register",
        client.base_url().trim_end_matches('/'),
        target_id
    );
    let parts: Vec<serde_json::Value> = parts
        .iter()
        .map(|(part_id, offset, length)| {
            serde_json::json!({
                "part_id": part_id,
                "offset": offset,
                "length": length,
            })
        })
        .collect();
    let body = serde_json::json!({
        "filename": filename,
        "mime_type": mime_type,
        "total_size": total_size,
        "parts": parts,
    });

    let resp = client
        .http()
        .post(&url)
        .headers(client.headers_with(capability)?)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("register live upload failed: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!(
            "register live upload rejected: status={} body={text}",
            status.as_u16()
        ));
    }
    Ok(())
}

/// Ask juicehost how many bytes it has accepted for one TUS resource. This is
/// intentionally best-effort: a missing live record does not mean the upload
/// is broken, so callers can keep transferring while they wait for a result.
pub async fn live_part_status(
    state: &Arc<AppState>,
    id: &str,
    host: Option<&str>,
    capability: Option<&str>,
) -> Result<serde_json::Value, String> {
    let client = StorageClient::resolve(state, host).await?;
    let url = format!(
        "{}/internal/live/part/{}",
        client.base_url().trim_end_matches('/'),
        id
    );
    let resp = client
        .http()
        .get(&url)
        .headers(client.headers_with(capability)?)
        .send()
        .await
        .map_err(|e| format!("juicehost live status request error: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "juicehost live status failed: status={status} body={body}"
        ));
    }
    resp.json::<serde_json::Value>()
        .await
        .map_err(|e| format!("juicehost live status parse error: {e}"))
}
