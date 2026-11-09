use std::sync::Arc;

use super::client::StorageClient;
use crate::state::AppState;

/// Lets juicehost serve the file during upload; empty input is a no-op.
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
