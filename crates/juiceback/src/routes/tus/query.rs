use std::sync::Arc;

use axum::{
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
    let upload = state.tus.get(&id).ok_or(AppError::TusSessionNotFound)?;

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("Upload-Offset", upload.offset.to_string())
        .header("Upload-Length", upload.total_length.to_string())
        .header("Tus-Resumable", "1.0.0")
        .body(axum::body::Body::empty())
        .map_err(|_| AppError::Internal("response build failed".into()))?)
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
