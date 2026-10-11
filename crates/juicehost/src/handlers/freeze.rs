use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Json,
};

use crate::{error::JuicehostError, state::AppState, storage::valid_component as is_valid_id};

/// Admin-only gate for freeze management.
///
/// Deliberately stricter than the router's `require_api_key` layer: per-file
/// capability headers are *ignored* here. Freezing is a moderation/legal
/// action, not an owner action (owners can simply delete their own file),
/// so only the instance API key authorizes it. Fail-closed: when no API key
/// is configured, nobody can freeze anything.
fn require_freeze_admin(headers: &HeaderMap, api_key: &str) -> Result<(), JuicehostError> {
    let provided = headers
        .get("x-juicehost-api-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if api_key.is_empty() || !juiceutils::constant_time_eq(api_key, provided) {
        tracing::warn!("auth: rejected freeze request - invalid or missing API key");
        return Err(JuicehostError::Unauthorized);
    }
    Ok(())
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
    require_freeze_admin(&headers, &state.api_key)?;

    if !is_valid_id(&id) {
        return Err(JuicehostError::BadRequest);
    }

    state
        .storage
        .freeze(&id)
        .await
        .map_err(JuicehostError::from)?;

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
    require_freeze_admin(&headers, &state.api_key)?;

    if !is_valid_id(&id) {
        return Err(JuicehostError::BadRequest);
    }

    state
        .storage
        .unfreeze(&id)
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
    fn freeze_admin_rejects_missing_key() {
        assert!(require_freeze_admin(&HeaderMap::new(), "secret").is_err());
    }

    #[test]
    fn freeze_admin_rejects_wrong_key() {
        assert!(require_freeze_admin(&key_headers("wrong"), "secret").is_err());
    }

    #[test]
    fn freeze_admin_accepts_exact_key() {
        assert!(require_freeze_admin(&key_headers("secret"), "secret").is_ok());
    }

    #[test]
    fn freeze_admin_fails_closed_without_configured_key() {
        // No configured key: even an "empty matches empty" comparison must
        // not authorize — and capability headers must not help either.
        assert!(require_freeze_admin(&HeaderMap::new(), "").is_err());
        let mut headers = HeaderMap::new();
        headers.insert("x-juicehost-file-capability", "anything".parse().unwrap());
        assert!(require_freeze_admin(&headers, "").is_err());
    }

    #[test]
    fn freeze_admin_ignores_capability_headers() {
        let mut headers = key_headers("wrong");
        headers.insert("x-juicehost-file-capability", "anything".parse().unwrap());
        assert!(require_freeze_admin(&headers, "secret").is_err());
    }
}
