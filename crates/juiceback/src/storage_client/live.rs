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
        .headers(client.headers_with(None)?)
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
