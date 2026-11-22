//! Peer-file metadata endpoints. juiceback only stores metadata for these
//! files: the bytes stay on the announcing device and are served through the
//! relay, so none of these handlers ever call `storage_client`.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;

use crate::{
    auth::{DeviceAuth, InternalApiKey},
    db::{self, FileRecord, PeerRevokeResult},
    error::AppError,
    state::AppState,
};

/// A single announce. `password`/`encrypted` are accepted only so they can be
/// rejected explicitly: peer sharing is for public files only.
#[derive(Debug, Deserialize, ToSchema)]
pub struct PeerRegisterRequest {
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub ttl_hours: f64,
    #[serde(default)]
    pub delete_token: Option<String>,
    #[serde(default, alias = "file_password")]
    pub password: Option<String>,
    #[serde(
        default,
        alias = "encrypted",
        alias = "is_encrypted",
        alias = "protected",
        alias = "is_protected"
    )]
    pub is_encrypted: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PeerRegisterResponse {
    pub id: String,
    pub delete_token: String,
    pub expires_at: i64,
    pub url: String,
    pub raw_url: String,
    pub download_url: String,
}

#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct PeerHeartbeatRequest {
    /// When omitted, every peer row for the device is touched.
    #[serde(default)]
    pub ids: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PeerAuthorizeRequest {
    pub device_id: String,
    pub user_id: String,
    pub ids: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PeerAuthorizeResponse {
    pub accepted: Vec<String>,
    pub rejected: Vec<PeerRejectionResponse>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PeerRejectionResponse {
    pub id: String,
    pub reason: String,
}

/// Bounds the relay's batch so one announce cannot pin a DB worker.
const MAX_AUTHORIZE_IDS: usize = 1000;
/// Bounds the heartbeat's id list so one request cannot build a huge query.
const MAX_HEARTBEAT_IDS: usize = 1000;

fn validate_register(payload: &PeerRegisterRequest) -> Result<String, AppError> {
    if !crate::utils::is_valid_id(&payload.id) {
        return Err(AppError::BadRequest(format!(
            "invalid file ID: must be {}-{} chars, alphanumeric, hyphens, or underscores",
            crate::constants::MIN_CUSTOM_ID_LEN,
            crate::constants::MAX_CUSTOM_ID_LEN,
        )));
    }
    if payload
        .password
        .as_deref()
        .is_some_and(|p| !p.trim().is_empty())
        || payload.is_encrypted
    {
        return Err(AppError::BadRequest(
            "peer files must be public and unencrypted".into(),
        ));
    }
    let filename = payload.filename.trim();
    if filename.is_empty() || filename.len() > crate::constants::MAX_FILENAME_LEN {
        return Err(AppError::BadRequest("invalid filename".into()));
    }
    if payload.mime_type.trim().is_empty() {
        return Err(AppError::BadRequest("mime_type is required".into()));
    }
    if payload.size_bytes < 0 {
        return Err(AppError::BadRequest(
            "size_bytes must be non-negative".into(),
        ));
    }
    if !payload.ttl_hours.is_finite() || payload.ttl_hours <= 0.0 {
        return Err(AppError::BadRequest("ttl_hours must be positive".into()));
    }
    Ok(crate::routes::upload::sanitize_filename(filename))
}

/// Rejects attempts to claim an id taken by a different device or by a
/// protected file; a device may re-announce its own public peer row.
fn check_existing(
    existing: Option<FileRecord>,
    device_id: &str,
) -> Result<Option<String>, AppError> {
    let Some(existing) = existing else {
        return Ok(None);
    };
    if existing.is_protected() || existing.is_encrypted {
        return Err(AppError::Forbidden(
            "protected files cannot be shared by peers".into(),
        ));
    }
    if !existing.is_peer() {
        return Err(AppError::Conflict("file ID already exists".into()));
    }
    if !crate::utils::constant_time_eq(&existing.peer_device_id, device_id) {
        return Err(AppError::Forbidden(
            "file is announced by another device".into(),
        ));
    }
    // Re-announcing our own row must keep the existing delete token, otherwise
    // previously stored /api/owned-files id+token pairs stop resolving.
    Ok(Some(existing.delete_token))
}

#[utoipa::path(
    post,
    path = "/api/peer/register",
    request_body(content = PeerRegisterRequest, description = "Metadata for a file served by the device"),
    responses(
        (status = 200, description = "Peer file registered", body = PeerRegisterResponse),
        (status = 400, description = "Invalid metadata or protected file"),
        (status = 401, description = "Missing or invalid device token"),
        (status = 409, description = "File ID already used by another file"),
        (status = 503, description = "Peer relay not configured"),
    ),
    tag = "Peers",
)]
pub async fn register_handler(
    State(state): State<Arc<AppState>>,
    DeviceAuth(claims): DeviceAuth,
    Json(payload): Json<PeerRegisterRequest>,
) -> Result<Json<PeerRegisterResponse>, AppError> {
    let filename = validate_register(&payload)?;
    let relay = state
        .config
        .peer_relay_url
        .as_deref()
        .ok_or_else(|| AppError::ServiceUnavailable("peer relay is not configured".into()))?;

    let device_id = claims.sub.clone();
    let existing_id = payload.id.clone();
    let existing = state
        .db_call("peer_get_existing", move |db| {
            db::get_file(db, &existing_id)
        })
        .await?;
    let existing_token = check_existing(existing, &device_id)?;

    let now = chrono::Utc::now().timestamp();
    let delete_token = payload
        .delete_token
        .filter(|token| !token.trim().is_empty())
        .or(existing_token)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let expires_at =
        now + (payload.ttl_hours * crate::constants::SECONDS_PER_HOUR_F64).round() as i64;
    let record = FileRecord::new_peer(
        payload.id.clone(),
        filename,
        payload.mime_type.trim().to_string(),
        payload.size_bytes,
        delete_token.clone(),
        now,
        expires_at,
        relay,
        device_id,
    );

    let to_insert = record.clone();
    state
        .db_call("replace_peer_file", move |db| {
            db::replace_peer_file(db, &to_insert)
        })
        .await?;

    let (url, raw_url, download_url) = crate::utils::share_links(
        &state.config,
        record.storage_host.as_deref(),
        &record.id,
        &record.filename,
    );
    Ok(Json(PeerRegisterResponse {
        id: record.id,
        delete_token,
        expires_at,
        url,
        raw_url,
        download_url,
    }))
}

#[utoipa::path(
    post,
    path = "/api/peer/revoke/{id}",
    responses(
        (status = 200, description = "Peer file revoked"),
        (status = 401, description = "Missing or invalid device token"),
        (status = 403, description = "File belongs to another device"),
        (status = 404, description = "Peer file not found"),
    ),
    tag = "Peers",
)]
pub async fn revoke_handler(
    State(state): State<Arc<AppState>>,
    DeviceAuth(claims): DeviceAuth,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let device_id = claims.sub.clone();
    let revoked_id = id.clone();
    let result = state
        .db_call("revoke_peer_file", move |db| {
            db::revoke_peer_file(db, &id, &device_id)
        })
        .await?;
    match result {
        PeerRevokeResult::Revoked => Ok(Json(json!({ "revoked": true, "id": revoked_id }))),
        PeerRevokeResult::NotFound => Err(AppError::NotFound),
        PeerRevokeResult::NotOwner => Err(AppError::Forbidden("not your peer file".into())),
    }
}

#[utoipa::path(
    post,
    path = "/api/peer/heartbeat",
    request_body(content = PeerHeartbeatRequest, description = "Optional list of announced ids"),
    responses(
        (status = 200, description = "last_seen updated"),
        (status = 401, description = "Missing or invalid device token"),
    ),
    tag = "Peers",
)]
pub async fn heartbeat_handler(
    State(state): State<Arc<AppState>>,
    DeviceAuth(claims): DeviceAuth,
    Json(payload): Json<PeerHeartbeatRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    if payload
        .ids
        .as_ref()
        .is_some_and(|ids| ids.len() > MAX_HEARTBEAT_IDS)
    {
        return Err(AppError::BadRequest(format!(
            "too many ids (max {MAX_HEARTBEAT_IDS})"
        )));
    }
    let now = chrono::Utc::now().timestamp();
    let device_id = claims.sub.clone();
    let ids = payload.ids;
    let updated = state
        .db_call("touch_peer_files", move |db| {
            db::delete_expired_peer_files(db, now)?;
            db::touch_peer_files(db, &device_id, ids.as_deref(), now)
        })
        .await?;
    Ok(Json(json!({ "updated": updated, "now": now })))
}

/// Approval is based only on the row's tier, device id and expiry. The relay
/// supplies `user_id` from its verified token; the `files` table has no user
/// column, so the device id is the ownership key.
fn authorize_ids(
    db: &Connection,
    device_id: &str,
    ids: &[String],
    now: i64,
) -> Result<PeerAuthorizeResponse, rusqlite::Error> {
    let mut accepted = Vec::new();
    let mut rejected = Vec::new();
    for id in ids {
        let reason = match db::get_file(db, id)? {
            None => Some("not found"),
            Some(record) if !record.is_peer() => Some("not a peer file"),
            Some(record) if !crate::utils::constant_time_eq(&record.peer_device_id, device_id) => {
                Some("owned by another device")
            }
            Some(record) if record.expires_at < now => Some("expired"),
            Some(record) if record.is_protected() || record.is_encrypted => Some("protected file"),
            Some(_) => None,
        };
        match reason {
            None => accepted.push(id.clone()),
            Some(reason) => rejected.push(PeerRejectionResponse {
                id: id.clone(),
                reason: reason.to_string(),
            }),
        }
    }
    Ok(PeerAuthorizeResponse { accepted, rejected })
}

#[utoipa::path(
    post,
    path = "/internal/peer/authorize",
    request_body(content = PeerAuthorizeRequest, description = "Ids announced by a connected peer"),
    responses(
        (status = 200, description = "Accepted and rejected ids", body = PeerAuthorizeResponse),
        (status = 400, description = "Malformed request"),
        (status = 401, description = "Invalid or missing x-juicehost-api-key header"),
    ),
    tag = "Internal",
    security(),
)]
pub async fn authorize_handler(
    State(state): State<Arc<AppState>>,
    _key: InternalApiKey,
    Json(payload): Json<PeerAuthorizeRequest>,
) -> Result<Json<PeerAuthorizeResponse>, AppError> {
    if payload.device_id.trim().is_empty() || payload.user_id.trim().is_empty() {
        return Err(AppError::BadRequest(
            "device_id and user_id are required".into(),
        ));
    }
    if payload.ids.len() > MAX_AUTHORIZE_IDS {
        return Err(AppError::BadRequest(format!(
            "too many ids (max {MAX_AUTHORIZE_IDS})"
        )));
    }
    let now = chrono::Utc::now().timestamp();
    let device_id = payload.device_id;
    let ids = payload.ids;
    let response = state
        .db_call("authorize_peer_files", move |db| {
            authorize_ids(db, &device_id, &ids, now)
        })
        .await?;
    Ok(Json(response))
}
