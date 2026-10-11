use std::sync::Arc;

use axum::{
    extract::{Path, State},
    response::Json,
};

use crate::{
    error::JuicehostError,
    state::AppState,
    storage::{R2Store, valid_component as is_valid_id},
};

#[utoipa::path(
    post,
    path = "/internal/file/{id}/promote",
    params(
        ("id" = String, Path, description = "The file ID to copy to R2 hot-tier"),
    ),
    responses(
        (status = 200, description = "File copied to R2", body = serde_json::Value),
        (status = 400, description = "Invalid file ID"),
        (status = 401, description = "Missing or invalid API key"),
        (status = 404, description = "File not found"),
        (status = 503, description = "R2 not configured or busy"),
    ),
    tag = "Internal",
)]
pub async fn promote_file(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, JuicehostError> {
    if !is_valid_id(&id) {
        return Err(JuicehostError::BadRequest);
    }
    let Some(ref r2) = state.r2 else {
        return Err(JuicehostError::ServiceUnavailable);
    };
    let Ok(_permit) = state.upload_semaphore.try_acquire() else {
        return Err(JuicehostError::ServiceUnavailable);
    };

    let data = state.storage.get(&id).await.map_err(|e| {
        tracing::warn!("promote: local read failed for id={id}: {e}");
        JuicehostError::from(e)
    })?;
    let key = R2Store::hot_key(&id, &data.meta.extension);
    let content_type = crate::storage::guess_mime(&data.meta.extension);
    r2.put_bytes(&key, data.data, &content_type).await.map_err(|e| {
        tracing::warn!("promote: R2 put failed for id={id}: {e}");
        JuicehostError::from(e)
    })?;

    tracing::info!("promoted id={id} to R2 ({key}, {} bytes)", data.meta.size);
    Ok(Json(serde_json::json!({"status": "ok", "id": id, "key": key, "size": data.meta.size})))
}

#[utoipa::path(
    post,
    path = "/internal/file/{id}/demote",
    params(
        ("id" = String, Path, description = "The file ID to remove from R2 hot-tier"),
    ),
    responses(
        (status = 200, description = "R2 copy removed", body = serde_json::Value),
        (status = 400, description = "Invalid file ID"),
        (status = 401, description = "Missing or invalid API key"),
    ),
    tag = "Internal",
)]
pub async fn demote_file(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, JuicehostError> {
    if !is_valid_id(&id) {
        return Err(JuicehostError::BadRequest);
    }
    let removed = if let Some(ref r2) = state.r2 {
        r2.delete_prefix(&R2Store::hot_prefix(&id))
            .await
            .map_err(|e| {
                tracing::warn!("demote: R2 delete failed for id={id}: {e}");
                JuicehostError::from(e)
            })?
    } else {
        0
    };
    Ok(Json(serde_json::json!({"status": "ok", "id": id, "removed": removed})))
}
