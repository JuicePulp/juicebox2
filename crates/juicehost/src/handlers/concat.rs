use std::sync::Arc;

use axum::{extract::State, http::HeaderMap, response::Json};

use super::common::required_file_capability;
use crate::{error::JuicehostError, state::AppState, storage::valid_component as is_valid_id};

#[derive(serde::Deserialize)]
pub struct ConcatRequest {
    target_id: String,

    #[serde(default = "default_concat_filename")]
    filename: String,

    parts: Vec<String>,
}

fn default_concat_filename() -> String {
    "upload.bin".into()
}

#[utoipa::path(
    post,
    path = "/internal/file/concat",
    request_body(content = serde_json::Value),
    responses(
        (status = 200, description = "Files concatenated", body = serde_json::Value),
        (status = 400, description = "Invalid target or parts"),
        (status = 401, description = "Missing or invalid credentials"),
        (status = 413, description = "Combined file exceeds size limit"),
    ),
    tag = "Internal",
)]
pub async fn concat_files(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<ConcatRequest>,
) -> Result<Json<serde_json::Value>, JuicehostError> {
    let target_id = payload.target_id;
    let filename = payload.filename;
    let parts: Vec<&str> = payload.parts.iter().map(String::as_str).collect();

    let unique: std::collections::HashSet<_> = parts.iter().copied().collect();
    if parts.is_empty()
        || parts.len() > state.max_concat_parts
        || unique.len() != parts.len()
        || !is_valid_id(&target_id)
        || parts.iter().any(|id| !is_valid_id(id) || *id == target_id)
    {
        return Err(JuicehostError::BadRequest);
    }

    let mut aggregate = 0u64;
    for part in &parts {
        let size = state
            .storage
            .stat(part)
            .await
            .map_err(JuicehostError::from)?
            .size;
        aggregate = aggregate
            .checked_add(size)
            .ok_or(JuicehostError::PayloadTooLarge)?;
        if aggregate > state.max_file_size_bytes {
            return Err(JuicehostError::PayloadTooLarge);
        }
    }
    let _permit = Arc::clone(&state.upload_semaphore)
        .try_acquire_owned()
        .map_err(|_| JuicehostError::ServiceUnavailable)?;

    let capability = required_file_capability(&headers, &state.api_key)?;

    state
        .storage
        .concat(&target_id, &filename, &parts, capability.as_deref())
        .await
        .map_err(JuicehostError::from)?;

    tracing::info!("concat: {target_id} <- {parts:?} ({filename})");

    Ok(Json(serde_json::json!({"status": "ok", "id": target_id})))
}
