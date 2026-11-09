use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Json,
};

use super::common::required_file_capability;
use crate::{
    error::JuicehostError,
    state::AppState,
    storage::{R2Store, valid_component as is_valid_id},
};

fn freeze_auth(headers: &HeaderMap, api_key: &str) -> Result<Option<String>, JuicehostError> {
    required_file_capability(headers, api_key).map_err(|e| {
        tracing::warn!("auth: rejected freeze request - invalid API key or capability");
        e
    })
}

#[utoipa::path(
    post,
    path = "/internal/file/{id}/freeze",
    params(
        ("id" = String, Path, description = "The file ID to freeze"),
    ),
    responses(
        (status = 200, description = "File frozen", body = serde_json::Value),
        (status = 400, description = "Invalid file ID"),
        (status = 401, description = "Missing or invalid API key"),
        (status = 404, description = "File not found"),
    ),
    tag = "Internal",
)]
pub async fn freeze_file(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, JuicehostError> {
    let capability = freeze_auth(&headers, &state.api_key)?;

    if !is_valid_id(&id) {
        return Err(JuicehostError::BadRequest);
    }

    state
        .storage
        .freeze(&id, capability.as_deref())
        .await
        .map_err(JuicehostError::from)?;

    if let Some(ref r2) = state.r2
        && let Err(e) = r2.delete_prefix(&R2Store::hot_prefix(&id)).await
    {
        tracing::warn!("freeze: R2 demote failed for id={id}: {e}");
    }

    tracing::info!("froze file: {id}");
    Ok(Json(
        serde_json::json!({"status": "ok", "id": id, "frozen": true}),
    ))
}

#[utoipa::path(
    post,
    path = "/internal/file/{id}/unfreeze",
    params(
        ("id" = String, Path, description = "The file ID to unfreeze"),
    ),
    responses(
        (status = 200, description = "File unfrozen", body = serde_json::Value),
        (status = 400, description = "Invalid file ID"),
        (status = 401, description = "Missing or invalid API key"),
        (status = 404, description = "File not found"),
    ),
    tag = "Internal",
)]
pub async fn unfreeze_file(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, JuicehostError> {
    let capability = freeze_auth(&headers, &state.api_key)?;

    if !is_valid_id(&id) {
        return Err(JuicehostError::BadRequest);
    }

    state
        .storage
        .unfreeze(&id, capability.as_deref())
        .await
        .map_err(JuicehostError::from)?;

    tracing::info!("unfroze file: {id}");
    Ok(Json(
        serde_json::json!({"status": "ok", "id": id, "frozen": false}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_headers(key: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("x-juicehost-api-key", key.parse().unwrap());
        headers
    }

    #[test]
    fn freeze_auth_rejects_missing_key_and_capability() {
        assert!(freeze_auth(&HeaderMap::new(), "secret").is_err());
    }

    #[test]
    fn freeze_auth_rejects_wrong_key_without_capability() {
        assert!(freeze_auth(&key_headers("wrong"), "secret").is_err());
    }

    #[test]
    fn freeze_auth_accepts_exact_key() {
        assert!(matches!(
            freeze_auth(&key_headers("secret"), "secret"),
            Ok(None)
        ));
    }

    #[test]
    fn freeze_auth_accepts_capability_with_wrong_key() {
        let mut headers = key_headers("wrong");
        headers.insert("x-juicehost-file-capability", "tok".parse().unwrap());
        assert!(matches!(
            freeze_auth(&headers, "secret"),
            Ok(Some(cap)) if cap == "tok"
        ));
    }

    #[test]
    fn freeze_auth_fails_closed_without_configured_key_or_capability() {
        assert!(freeze_auth(&HeaderMap::new(), "").is_err());
    }

    #[test]
    fn freeze_auth_accepts_capability_without_configured_key() {
        let mut headers = HeaderMap::new();
        headers.insert("x-juicehost-file-capability", "tok".parse().unwrap());
        assert!(matches!(
            freeze_auth(&headers, ""),
            Ok(Some(cap)) if cap == "tok"
        ));
    }
}
