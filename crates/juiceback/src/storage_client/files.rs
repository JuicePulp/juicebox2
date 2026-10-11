use std::sync::Arc;

use super::client::StorageClient;
use crate::{db, state::AppState};

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
    let client = StorageClient::resolve(state, custom_host).await?;

    let body = serde_json::json!({ "new_id": new_id });

    let headers = client.headers_with(capability)?;

    let resp = client
        .http()
        .post(format!(
            "{}/internal/file/{}/rename",
            client.base_url(),
            old_id
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

pub async fn demote_file_on_juicehost(
    state: &Arc<AppState>,
    id: &str,
    host: Option<&str>,
) -> Result<(), String> {
    let custom_host = host.map(str::trim).filter(|h| !h.is_empty());
    if custom_host.is_none() && state.config.juicehost_url.is_empty() {
        return Ok(());
    }
    let client = StorageClient::resolve(state, custom_host).await?;

    let headers = client.headers_with(None)?;

    let resp = client
        .http()
        .post(format!("{}/internal/file/{}/demote", client.base_url(), id))
        .headers(headers)
        .send()
        .await
        .map_err(|e| format!("juicehost demote request failed: {e}"))?;

    if resp.status().is_success() {
        tracing::info!("juicehost demote: id={id} ok");
        Ok(())
    } else if resp.status() == reqwest::StatusCode::NOT_FOUND {
        tracing::warn!("juicehost demote: id={id} not found (already gone)");
        Ok(())
    } else {
        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();
        Err(format!(
            "juicehost demote failed: status={status} body={body_text}"
        ))
    }
}

pub async fn promote_file_on_juicehost(
    state: &Arc<AppState>,
    id: &str,
    host: Option<&str>,
    capability: Option<&str>,
) -> Result<String, String> {
    let custom_host = host.map(str::trim).filter(|h| !h.is_empty());
    if custom_host.is_none() && state.config.juicehost_url.is_empty() {
        return Err("JUICEHOST_URL is not set".into());
    }
    let client = StorageClient::resolve(state, custom_host).await?;

    let headers = client.headers_with(capability)?;

    let resp = client
        .http()
        .post(format!(
            "{}/internal/file/{}/promote",
            client.base_url(),
            id
        ))
        .headers(headers)
        .send()
        .await
        .map_err(|e| format!("juicehost promote request failed: {e}"))?;

    if resp.status().is_success() {
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("juicehost promote invalid response: {e}"))?;
        body.get("key")
            .and_then(|v| v.as_str())
            .filter(|k| !k.is_empty())
            .map(str::to_string)
            .ok_or_else(|| "juicehost promote response missing key".to_string())
    } else {
        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();
        Err(format!(
            "juicehost promote failed: status={status} body={body_text}"
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

    let id_owned = id.to_string();
    let hot = state
        .db_call("get_file_tier", move |db| db::get_file(db, &id_owned))
        .await
        .ok()
        .flatten()
        .is_some_and(|record| record.tier == "hot");
    if hot && let Err(e) = demote_file_on_juicehost(state, id, host).await {
        tracing::warn!("juicehost delete: R2 demote failed for id={id}: {e}");
    }

    let client = StorageClient::resolve(state, custom_host).await?;

    let headers = client.headers_with(capability)?;

    let resp = client
        .http()
        .delete(format!("{}/internal/file/{}", client.base_url(), id))
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

pub async fn concat_files(
    state: &Arc<AppState>,
    target_id: &str,
    filename: &str,
    part_ids: &[String],
    host: Option<&str>,
    capability: Option<&str>,
) -> Result<(), String> {
    let client = StorageClient::resolve(state, host).await?;

    let body = serde_json::json!({
        "target_id": target_id,
        "filename": filename,
        "parts": part_ids,
    });

    let headers = client.headers_with(capability)?;

    let resp = client
        .http()
        .post(format!("{}/internal/file/concat", client.base_url()))
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
