use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};

use super::common::required_file_capability;
use crate::{error::JuicehostError, state::AppState, storage::valid_component as is_valid_id};

#[utoipa::path(
    delete,
    path = "/internal/file/{id}",
    params(
        ("id" = String, Path, description = "The file ID to delete"),
    ),
    responses(
        (status = 204, description = "File deleted"),
        (status = 400, description = "Invalid file ID"),
        (status = 401, description = "Missing or invalid credentials"),
        (status = 404, description = "File not found"),
    ),
    tag = "Internal",
)]
pub async fn delete_file(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, JuicehostError> {
    let capability = required_file_capability(&headers, &state.api_key)?;

    if !is_valid_id(&id) {
        return Err(JuicehostError::BadRequest);
    }

    let deleted = state
        .storage
        .delete(&id, capability.as_deref())
        .await
        .map_err(JuicehostError::from)?;

    // A completed direct upload can still have a live-upload entry. If it
    // outlives the stored file, serving the deleted ID waits on that entry
    // instead of returning 404.
    state.live_uploads.remove_target(&id);

    if !deleted {
        return Err(JuicehostError::NotFound);
    }

    tracing::info!("deleted file {id}");
    Ok(StatusCode::NO_CONTENT)
}
