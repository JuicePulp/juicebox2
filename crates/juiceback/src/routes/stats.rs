use std::sync::Arc;

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use serde::Deserialize;
use utoipa::ToSchema;

use crate::{db, error::AppError, state::AppState};

fn check_internal_key(state: &AppState, headers: &HeaderMap) -> Result<(), AppError> {
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
    Ok(())
}

#[derive(Deserialize, ToSchema)]
pub struct HitRequest {
    /// File id that was served.
    pub file_id: String,
    /// `"view"` (inline bytes, preview page, protected content) or
    /// `"download"` (forced attachment).
    pub kind: String,
    /// Viewer IP as seen by the serving edge. Hashed with the ip_pepper
    /// HMAC on receipt; the raw value is never stored.
    pub ip: String,
}

#[utoipa::path(
    post,
    path = "/internal/stats/hit",
    request_body = HitRequest,
    responses(
        (status = 204, description = "Hit recorded"),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Invalid API key"),
    ),
    security(("api_key" = [])),
    tag = "Internal",
)]
pub async fn hit_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<HitRequest>,
) -> Result<StatusCode, AppError> {
    check_internal_key(&state, &headers)?;
    let kind =
        db::HitKind::from_str(&body.kind).ok_or(AppError::BadRequest("unknown hit kind".into()))?;
    if body.file_id.is_empty() || body.file_id.len() > 64 || body.ip.is_empty() {
        return Err(AppError::BadRequest("invalid hit payload".into()));
    }
    let ip_hash = crate::utils::hash_ip_for_ban(&body.ip, &state.config.ip_pepper);
    let now = chrono::Utc::now().timestamp();
    state
        .db_call("record_file_hit", move |db| {
            db::record_file_hit(db, &body.file_id, &ip_hash, kind, now)
        })
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, ToSchema)]
pub struct VisitRequest {
    /// Visitor IP as seen by the edge. Hashed on receipt, never stored.
    pub ip: String,
}

#[utoipa::path(
    post,
    path = "/internal/stats/visit",
    request_body = VisitRequest,
    responses(
        (status = 204, description = "Visit recorded"),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Invalid API key"),
    ),
    security(("api_key" = [])),
    tag = "Internal",
)]
pub async fn visit_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<VisitRequest>,
) -> Result<StatusCode, AppError> {
    check_internal_key(&state, &headers)?;
    if body.ip.is_empty() {
        return Err(AppError::BadRequest("invalid visit payload".into()));
    }
    let ip_hash = crate::utils::hash_ip_for_ban(&body.ip, &state.config.ip_pepper);
    let now = chrono::Utc::now().timestamp();
    state
        .db_call("record_site_visit", move |db| {
            db::record_site_visit(db, &ip_hash, now)
        })
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
