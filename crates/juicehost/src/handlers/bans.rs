use std::sync::Arc;

use axum::{
    extract::State,
    response::Json,
};
use serde::Deserialize;
use utoipa::ToSchema;

use crate::{error::JuicehostError, state::AppState};

#[derive(Debug, Deserialize, ToSchema)]
pub struct BanHashRequest {
    pub pepper: String,
    pub hash: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct BanSnapshotRequest {
    pub pepper: String,
    #[serde(default)]
    pub hashes: Vec<String>,
}

fn ensure_pepper(state: &Arc<AppState>, pepper: &str) {
    if state.ban_list.pepper().is_empty() && !pepper.is_empty() {
        state.ban_list.set_pepper(pepper);
    }
}

#[utoipa::path(
    post,
    path = "/internal/bans/ban",
    request_body(content = BanHashRequest, description = "Ban a pre-hashed IP (pushed by juiceback)"),
    responses(
        (status = 200, description = "IP banned", body = serde_json::Value),
        (status = 400, description = "Missing pepper or hash"),
        (status = 401, description = "Missing or invalid API key"),
    ),
    tag = "Internal",
)]
pub async fn push_ban(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<BanHashRequest>,
) -> Result<Json<serde_json::Value>, JuicehostError> {
    let pepper = payload.pepper.trim();
    let hash = payload.hash.trim();
    if pepper.is_empty() || hash.is_empty() {
        return Err(JuicehostError::BadRequest);
    }
    ensure_pepper(&state, pepper);
    state.ban_list.add_hash(hash);
    Ok(Json(serde_json::json!({"status": "ok", "banned": true})))
}

#[utoipa::path(
    post,
    path = "/internal/bans/unban",
    request_body(content = BanHashRequest, description = "Unban a pre-hashed IP (pushed by juiceback)"),
    responses(
        (status = 200, description = "IP unbanned", body = serde_json::Value),
        (status = 400, description = "Missing pepper or hash"),
        (status = 401, description = "Missing or invalid API key"),
    ),
    tag = "Internal",
)]
pub async fn push_unban(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<BanHashRequest>,
) -> Result<Json<serde_json::Value>, JuicehostError> {
    let hash = payload.hash.trim();
    if hash.is_empty() {
        return Err(JuicehostError::BadRequest);
    }
    if !payload.pepper.trim().is_empty() {
        ensure_pepper(&state, payload.pepper.trim());
    }
    let removed = state.ban_list.remove_hash(hash);
    Ok(Json(serde_json::json!({"status": "ok", "removed": removed})))
}

#[utoipa::path(
    post,
    path = "/internal/bans/snapshot",
    request_body(content = BanSnapshotRequest, description = "Replace the full ban snapshot (pushed by juiceback after imports)"),
    responses(
        (status = 200, description = "Snapshot applied", body = serde_json::Value),
        (status = 400, description = "Missing pepper"),
        (status = 401, description = "Missing or invalid API key"),
    ),
    tag = "Internal",
)]
pub async fn push_snapshot(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<BanSnapshotRequest>,
) -> Result<Json<serde_json::Value>, JuicehostError> {
    let pepper = payload.pepper.trim();
    if pepper.is_empty() {
        return Err(JuicehostError::BadRequest);
    }
    state.ban_list.set_snapshot(pepper, payload.hashes);
    tracing::info!("applied pushed ban snapshot ({} bans)", state.ban_list.len());
    Ok(Json(serde_json::json!({"status": "ok", "count": state.ban_list.len()})))
}
