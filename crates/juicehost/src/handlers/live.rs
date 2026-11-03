use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;

use super::common::backend_request;
use crate::{error::JuicehostError, state::AppState, storage::valid_component};

#[derive(Debug, Deserialize)]
pub struct LivePartRequest {
    pub part_id: String,
    pub offset: u64,
    pub length: u64,
}

#[derive(Debug, Deserialize)]
pub struct LiveRegisterRequest {
    pub filename: String,
    pub mime_type: String,
    pub total_size: u64,
    #[serde(default)]
    pub parts: Vec<LivePartRequest>,
}

pub async fn register_live_upload(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<LiveRegisterRequest>,
) -> Result<StatusCode, JuicehostError> {
    if !valid_component(&id)
        || body.filename.is_empty()
        || body.total_size == 0
        || body.parts.is_empty()
    {
        return Err(JuicehostError::BadRequest);
    }

    let mut parts = Vec::with_capacity(body.parts.len());
    for part in body.parts {
        if !valid_component(&part.part_id)
            || part.length == 0
            || part
                .offset
                .checked_add(part.length)
                .is_none_or(|end| end > body.total_size)
        {
            return Err(JuicehostError::BadRequest);
        }
        parts.push((part.part_id, part.offset, part.length));
    }

    state.live_uploads.register_parts(
        &id,
        &body.filename,
        &body.mime_type,
        body.total_size,
        &parts,
    );
    tracing::info!(
        "live upload registered: id={} total={} parts={}",
        id,
        body.total_size,
        parts.len()
    );
    Ok(StatusCode::NO_CONTENT)
}

pub async fn remove_live_upload(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> StatusCode {
    state.live_uploads.remove_target(&id);
    StatusCode::NO_CONTENT
}

pub async fn upload_progress(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    if !valid_component(&id) {
        return JuicehostError::BadRequest.into_response();
    }

    if let Ok(meta) = state.storage.stat(&id).await {
        return Json(serde_json::json!({
            "ready": true,
            "uploading": false,
            "total": meta.size,
            "uploaded": meta.size,
            "available": meta.size,
            "percent": 100,
        }))
        .into_response();
    }

    if let Some(progress) = state.live_uploads.progress(&id) {
        let percent = if progress.total_size == 0 {
            0
        } else {
            progress.uploaded.saturating_mul(100) / progress.total_size
        };
        return Json(serde_json::json!({
            "ready": false,
            "uploading": true,
            "complete": progress.complete,
            "total": progress.total_size,
            "uploaded": progress.uploaded,
            "available": progress.available,
            "percent": percent,
            "filename": progress.filename,
            "mime_type": progress.mime,
        }))
        .into_response();
    }

    if let Some(ref backend_url) = state.backend_url {
        let status_url = format!("{backend_url}/internal/file/{id}/status");
        if let Ok(resp) = backend_request(&state, status_url).send().await
            && resp.status().is_success()
            && let Ok(body) = resp.json::<serde_json::Value>().await
            && body.get("status").and_then(|s| s.as_str()) == Some("uploading")
        {
            let total = body
                .get("size_bytes")
                .and_then(|s| s.as_i64())
                .unwrap_or(0)
                .max(0) as u64;
            return Json(serde_json::json!({
                "ready": false,
                "uploading": true,
                "complete": false,
                "total": total,
                "uploaded": 0,
                "available": 0,
                "percent": 0,
                "filename": body.get("filename").and_then(|s| s.as_str()),
            }))
            .into_response();
        }
    }

    Json(serde_json::json!({
        "ready": false,
        "uploading": false,
        "complete": false,
        "total": 0,
        "uploaded": 0,
        "available": 0,
        "percent": 0,
    }))
    .into_response()
}
