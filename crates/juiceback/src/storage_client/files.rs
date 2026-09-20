use std::sync::Arc;

use super::target::resolve_juicehost_target;
use crate::state::AppState;

pub async fn rename_file_on_juicehost(
    state: &Arc<AppState>,
    old_id: &str,
    new_id: &str,
    host: Option<&str>,
    capability: Option<&str>,
) -> Result<(), String> {
    let custom_host = host.map(str::trim).filter(|h| !h.is_empty());
    if custom_host.is_none() && state.config.juicehost_url.is_empty() {
        return Ok(());
    }
    let target = resolve_juicehost_target(state, custom_host)
        .await
        .map_err(|e| e.to_string())?;

    let body = serde_json::json!({ "new_id": new_id });

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
        .post(format!(
            "{}/internal/file/{}/rename",
            target.base_url, old_id
        ))
        .headers(headers)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("juicehost rename request failed: {e}"))?;

    if resp.status().is_success() {
        tracing::info!("juicehost rename: {old_id} -> {new_id} ok");
        Ok(())
    } else {
        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();
        if status == reqwest::StatusCode::NOT_FOUND {
            tracing::error!("juicehost rename: {old_id} not found on disk");
            return Err(format!(
                "juicehost rename failed: source file {old_id} not found on disk (status=404)"
            ));
        }
        Err(format!(
            "juicehost rename failed: status={status} body={body_text}"
        ))
    }
}

pub async fn delete_file_on_juicehost(
    state: &Arc<AppState>,
    id: &str,
    host: Option<&str>,
    capability: Option<&str>,
) -> Result<(), String> {
    let custom_host = host.map(str::trim).filter(|h| !h.is_empty());
    if custom_host.is_none() && state.config.juicehost_url.is_empty() {
        return Ok(());
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
        .delete(format!("{}/internal/file/{}", target.base_url, id))
        .headers(headers)
        .send()
        .await
        .map_err(|e| format!("juicehost delete request failed: {e}"))?;

    if resp.status().is_success() {
        tracing::info!("juicehost delete: id={id} ok");
        Ok(())
    } else if resp.status() == reqwest::StatusCode::NOT_FOUND {
        tracing::warn!("juicehost delete: id={id} not found (already gone)");
        Ok(())
    } else {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        Err(format!(
            "juicehost delete failed: status={status} body={body}"
        ))
    }
}

/// Concatenate multiple part files on juicehost into a single target file.
/// Used by parallel TUS uploads to assemble split files.
pub async fn concat_files(
    state: &Arc<AppState>,
    target_id: &str,
    filename: &str,
    part_ids: &[String],
    host: Option<&str>,
    capability: Option<&str>,
) -> Result<(), String> {
    let target = resolve_juicehost_target(state, host)
        .await
        .map_err(|e| e.to_string())?;

    let body = serde_json::json!({
        "target_id": target_id,
        "filename": filename,
        "parts": part_ids,
    });

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
        .post(format!("{}/internal/file/concat", target.base_url))
        .headers(headers)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("juicehost concat request failed: {e}"))?;

    if resp.status().is_success() {
        tracing::info!("juicehost concat: {} <- {:?} ok", target_id, part_ids);
        Ok(())
    } else {
        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();
        Err(format!(
            "juicehost concat failed: status={status} body={body_text}"
        ))
    }
}
