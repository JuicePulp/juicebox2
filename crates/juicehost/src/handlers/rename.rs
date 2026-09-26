use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Json,
};

use super::common::required_file_capability;
use crate::{error::JuicehostError, state::AppState, storage::valid_component as is_valid_id};

#[derive(serde::Deserialize)]
pub struct RenameRequest {
    new_id: String,
}

#[utoipa::path(
    post,
    path = "/internal/file/{id}/rename",
    params(
        ("id" = String, Path, description = "Current file ID"),
    ),
    request_body(content = serde_json::Value, description = "JSON with `new_id` field"),
    responses(
        (status = 200, description = "File renamed", body = serde_json::Value),
        (status = 400, description = "Invalid ID or missing body"),
        (status = 401, description = "Missing or invalid credentials"),
        (status = 404, description = "Source file not found"),
        (status = 409, description = "Target file ID already exists"),
    ),
    tag = "Internal",
)]
pub async fn rename_file(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(payload): Json<RenameRequest>,
) -> Result<Json<serde_json::Value>, JuicehostError> {
    let new_id = payload.new_id;

    if !is_valid_id(&id) || !is_valid_id(&new_id) {
        return Err(JuicehostError::BadRequest);
    }

    let capability = required_file_capability(&headers, &state.api_key)?;

    state
        .storage
        .rename(&id, &new_id, capability.as_deref())
        .await
        .map_err(JuicehostError::from)?;

    tracing::info!("renamed file: {id} -> {new_id}");

    Ok(Json(
        serde_json::json!({"status": "ok", "old_id": id, "new_id": new_id}),
    ))
}
