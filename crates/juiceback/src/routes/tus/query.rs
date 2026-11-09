use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};

use super::state::release_parallel_slot;
use crate::{error::AppError, state::AppState};

#[utoipa::path(
    get,
    path = "/api/tus/{id}",
    params(
        ("id" = String, Path, description = "The TUS upload session ID"),
    ),
    responses(
        (status = 200, description = "Upload progress", headers(
            ("Upload-Offset" = String, description = "Current byte offset"),
            ("Upload-Length" = String, description = "Total expected size"),
            ("Tus-Resumable" = String, description = "TUS protocol version"),
        )),
        (status = 404, description = "TUS session not found"),
    ),
    tag = "Uploads",
)]
pub async fn get_upload_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let (offset, total_length) = {
        let mut upload = state.tus.get_mut(&id).ok_or(AppError::TusSessionNotFound)?;
        upload.last_activity = chrono::Utc::now().timestamp();
        (upload.offset, upload.total_length)
    };

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("Upload-Offset", offset.to_string())
        .header("Upload-Length", total_length.to_string())
        .header("Tus-Resumable", "1.0.0")
        .body(axum::body::Body::empty())
        .map_err(|_| AppError::Internal("response build failed".into()))?)
}

#[utoipa::path(
    get,
    path = "/api/tus/{id}/health",
    params(
        ("id" = String, Path, description = "The TUS upload session ID"),
    ),
    responses(
        (status = 200, description = "Backend and storage progress"),
        (status = 404, description = "TUS session not found"),
    ),
    tag = "Uploads",
)]
pub async fn health_upload_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let (offset, total_length, storage_host, capability) = {
        let mut upload = state.tus.get_mut(&id).ok_or(AppError::TusSessionNotFound)?;
        upload.last_activity = chrono::Utc::now().timestamp();
        (
            upload.offset,
            upload.total_length,
            upload.storage_host.clone(),
            upload.capability.clone(),
        )
    };

    let storage = match crate::storage_client::live_part_status(
        &state,
        &id,
        storage_host.as_deref(),
        Some(capability.as_str()),
    )
    .await
    {
        Ok(status) => status,
        Err(error) => serde_json::json!({
            "found": false,
            "error": error,
        }),
    };

    Ok(Json(serde_json::json!({
        "id": id,
        "offset": offset,
        "length": total_length,
        "complete": offset >= total_length,
        "storage": storage,
    })))
}

#[utoipa::path(
    options,
    path = "/api/tus",
    responses(
        (status = 204, description = "TUS capabilities", headers(
            ("Tus-Resumable" = String, description = "TUS protocol version"),
            ("Tus-Version" = String, description = "Supported TUS version"),
            ("Tus-Extension" = String, description = "Supported TUS extensions"),
        )),
    ),
    tag = "Uploads",
)]
pub async fn options_handler() -> Response {
    Response::builder()
        .status(StatusCode::NO_CONTENT)
        .header("Tus-Resumable", "1.0.0")
        .header("Tus-Version", "1.0.0")
        .header("Tus-Extension", "creation,termination")
        .body(axum::body::Body::empty())
        .unwrap_or_else(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "tus capability check failed",
            )
                .into_response()
        })
}

#[utoipa::path(
    delete,
    path = "/api/tus/{id}",
    params(
        ("id" = String, Path, description = "The TUS upload session ID"),
    ),
    responses(
        (status = 204, description = "TUS session deleted and temp file removed"),
        (status = 404, description = "TUS session not found"),
    ),
    tag = "Uploads",
)]
pub async fn delete_upload_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let existing = state.tus.get(&id).ok_or(AppError::TusSessionNotFound)?;
    let sid = existing.session_id.clone();
    let pi = existing.part_index;
    let len = existing.total_length;
    drop(existing);
    release_parallel_slot(&state, &id, sid.as_deref(), pi, len);
    Ok(Response::builder()
        .status(StatusCode::NO_CONTENT)
        .header("Tus-Resumable", "1.0.0")
        .body(axum::body::Body::empty())
        .map_err(|_| AppError::Internal("response build failed".into()))?)
}
