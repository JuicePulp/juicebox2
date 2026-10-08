use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};

use crate::{db, error::AppError, state::AppState};

#[utoipa::path(
    get,
    path = "/internal/file/{id}/status",
    params(
        ("id" = String, Path, description = "File ID")
    ),
    responses(
        (status = 200, description = "File status"),
        (status = 401, description = "Invalid or missing API key"),
        (status = 404, description = "File not found"),
    ),
    tag = "Internal",
)]
#[tracing::instrument(skip_all)]
pub async fn file_status_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    if state.config.juicehost_api_key.is_empty() {
        return Err(AppError::Unauthorized("API key not configured".into()));
    }
    let provided = headers
        .get("x-juicehost-api-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !crate::utils::constant_time_eq(provided, &state.config.juicehost_api_key) {
        return Err(AppError::Unauthorized("invalid API key".into()));
    }

    let id_clone = id.clone();
    let file = state
        .db_call("get_file", move |db| db::get_file(db, &id_clone))
        .await;

    match file {
        Ok(Some(record)) => {
            // The unlock shell lives on juicehost: this URL is the public
            // `/f/` page, never a juiceback address.
            let unlock_url = record.is_protected().then(|| {
                crate::utils::public_url(
                    &state.config.public_base_url,
                    record.storage_host.as_deref(),
                    &id,
                    &record.filename,
                )
            });
            Ok(Json(serde_json::json!({
                "id": id,
                "status": record.status,
                "filename": record.filename,
                "expires_at": record.expires_at,
                "protected": record.is_protected(),
                "is_encrypted": record.is_encrypted,
                "unlock_url": unlock_url,
                "key_version": record.key_version,
                "gateway_origin": state.config.juiceback_origin,
            })))
        }
        Ok(None) => Err(AppError::NotFound),
        Err(e) => Err(e),
    }
}
