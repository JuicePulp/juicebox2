use std::{net::SocketAddr, sync::Arc};

use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::HeaderMap,
    response::Response,
};
use serde::Serialize;
use utoipa::ToSchema;

use super::{
    common::{UploadResponse, enforce_mint_limit, region_host, upload_json_response},
    ticket::{
        clamp_ttl_seconds, device_connected, encrypted_client_ip, insert_owned_pending,
        mint_ticket, notify_device, public_urls_for, reject_blocked_filename, ticket_ttl_secs,
    },
};
use crate::{
    db::{self, FileRecord},
    error::AppError,
    routes::UserId,
    state::AppState,
};

#[derive(Serialize, ToSchema)]
pub struct DirectUploadReserveResponse {
    pub ticket: String,
    pub file_id: String,
    pub upload_url: String,
    pub url: String,
    pub raw_url: String,
    pub download_url: String,
    pub expires_at: i64,
}

#[derive(serde::Deserialize, ToSchema)]
pub struct DirectUploadReserveRequest {
    pub filename: String,
    pub mime_type: String,
    pub file_size: u64,
    pub ttl_hours: Option<f64>,
    pub host: Option<String>,

    #[serde(default)]
    pub device_id: Option<String>,

    #[serde(default)]
    pub password: Option<String>,
}

#[derive(serde::Deserialize, ToSchema)]
pub struct DirectUploadCompleteRequest {
    pub file_id: String,
    pub ticket: String,
}

#[utoipa::path(
    post,
    path = "/upload/direct/reserve",
    request_body = DirectUploadReserveRequest,
    responses(
        (status = 200, description = "Direct upload ticket issued", body = DirectUploadReserveResponse),
        (status = 413, description = "File exceeds configured limit"),
    ),
    tag = "Uploads",
)]
#[tracing::instrument(skip_all)]
pub async fn direct_upload_reserve_handler(
    State(state): State<Arc<AppState>>,
    UserId(user_id): UserId,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<DirectUploadReserveRequest>,
) -> Result<Json<DirectUploadReserveResponse>, AppError> {
    let (
        default_ttl,
        allowed_ttl,
        danger,
        max_file_size,
        public_juicehost_url,
        default_storage_host,
    ) = {
        let jh = state.juicehost_config_or_refresh().await?;
        (
            jh.default_ttl_hours,
            jh.allowed_ttl_hours.clone(),
            jh.danger_level,
            jh.max_file_size_bytes,
            state.config.public_juicehost_url.clone(),
            state.config.default_storage_host.clone(),
        )
    };

    if body.file_size == 0 || body.file_size > max_file_size {
        return Err(AppError::PayloadTooLarge);
    }
    if body.password.as_deref().is_some_and(|p| !p.is_empty()) {
        return Err(super::protected::relay_required());
    }
    reject_blocked_filename(&body.filename, danger)?;

    let requested_device_id = body
        .device_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    if let Some(ref wanted) = requested_device_id {
        if !device_connected(&state, &user_id, wanted) {
            return Err(AppError::BadRequest(
                "Unknown device. Pair juicebox-plus first.".into(),
            ));
        }
    }

    let selected_host = match body
        .host
        .as_deref()
        .map(str::trim)
        .filter(|host| !host.is_empty())
    {
        Some(host) => Some(
            crate::storage_client::check_storage_host(host, state.config.allow_private_fetch)
                .await
                .map_err(|e| AppError::BadRequest(e.to_string()))?,
        ),
        None => None,
    };

    enforce_mint_limit(&state, &headers, addr)?;

    let now = chrono::Utc::now().timestamp();
    let ttl_secs = ticket_ttl_secs(&state, body.file_size, 900);
    let ttl_seconds = clamp_ttl_seconds(&allowed_ttl, default_ttl, body.ttl_hours);
    let file_id = nanoid::nanoid!(8);
    let delete_token = uuid::Uuid::new_v4().to_string();
    let encrypted_ip = encrypted_client_ip(&state, &headers, addr);

    let region_juicehost = region_host(&state, &headers, addr);
    let storage_host = selected_host
        .clone()
        .or(region_juicehost.clone())
        .or(default_storage_host);
    let record = FileRecord::new(
        file_id.clone(),
        body.filename.clone(),
        body.mime_type.clone(),
        body.file_size as i64,
        delete_token.clone(),
        now,
        now + ttl_seconds,
        encrypted_ip,
        storage_host.clone(),
    );
    let record = insert_owned_pending(&state, &user_id, record, "own_direct_upload").await?;

    let ticket_sub = requested_device_id
        .clone()
        .unwrap_or_else(|| "browser".to_string());
    let ticket = mint_ticket(
        &state,
        &ticket_sub,
        &user_id,
        &file_id,
        &body.filename,
        &body.mime_type,
        body.file_size,
        &delete_token,
        now,
        ttl_secs,
    )?;

    let (public_url, raw_url, download_url) = public_urls_for(&state, &record);

    if let Some(ref wanted) = requested_device_id {
        notify_device(
            &state,
            &user_id,
            wanted,
            &file_id,
            &body.filename,
            &body.mime_type,
            body.file_size,
            &ticket,
            &public_url,
        )
        .await;
    }

    let upload_base = storage_host
        .as_deref()
        .unwrap_or_else(|| public_juicehost_url.trim_end_matches('/'));
    let upload_url = format!("{upload_base}/internal/file/upload/{file_id}");

    Ok(Json(DirectUploadReserveResponse {
        ticket,
        file_id,
        upload_url,
        url: public_url,
        raw_url,
        download_url,
        expires_at: record.expires_at,
    }))
}

#[utoipa::path(
    post,
    path = "/upload/direct/complete",
    request_body = DirectUploadCompleteRequest,
    responses(
        (status = 200, description = "Direct upload completed", body = UploadResponse),
        (status = 401, description = "Invalid upload ticket"),
        (status = 409, description = "Upload is already complete"),
    ),
    tag = "Uploads",
)]
#[tracing::instrument(skip_all)]
pub async fn direct_upload_complete_handler(
    State(state): State<Arc<AppState>>,
    UserId(user_id): UserId,
    headers: HeaderMap,
    Json(body): Json<DirectUploadCompleteRequest>,
) -> Result<Response, AppError> {
    let claims = juiceproto::ticket::verify(&body.ticket, &state.config.ticket_jwt_secret)
        .map_err(|_| AppError::Unauthorized("Invalid upload ticket".into()))?;
    if claims.file_id != body.file_id || claims.user_id != user_id {
        return Err(AppError::Unauthorized(
            "Upload ticket does not match session".into(),
        ));
    }

    let file_id = body.file_id;
    let lookup_id = file_id.clone();
    let file = state
        .db_call("get_direct_upload", move |db| db::get_file(db, &lookup_id))
        .await?
        .ok_or(AppError::NotFound)?;
    let owner_id = user_id.clone();
    let owned_id = file_id.clone();
    let owned = state
        .db_call("verify_direct_upload_owner", move |db| {
            db::client_owns_file(db, &owner_id, &owned_id)
        })
        .await?;
    if !owned {
        return Err(AppError::Unauthorized(
            "Upload is not owned by this session".into(),
        ));
    }
    if file.status != FileRecord::STATUS_UPLOADING {
        return Err(AppError::Conflict("upload is already complete".into()));
    }

    if file.is_protected() {
        return Err(super::protected::relay_required());
    }

    let stored_size = crate::storage_client::stat_file_on_juicehost(
        &state,
        &file_id,
        file.storage_host.as_deref(),
        Some(&file.delete_token),
    )
    .await
    .map_err(AppError::JuicehostUnreachable)?
    .ok_or_else(|| {
        AppError::JuicehostRejected("Uploaded file was not found on juicehost".into())
    })?;
    if stored_size != file.size_bytes as u64 {
        return Err(AppError::JuicehostRejected(format!(
            "Uploaded file size mismatch: expected {} bytes, got {stored_size}",
            file.size_bytes
        )));
    }

    let completed = crate::reservation::finish(
        &state,
        crate::reservation::FinishRequest {
            id: file_id,
            delete_token: file.delete_token.clone(),
            filename: file.filename.clone(),
            mime_type: file.mime_type.clone(),
            size_bytes: file.size_bytes,
            uploader_ip: None,
        },
        crate::reservation::FinishPolicy::Strict,
    )
    .await?;

    Ok(upload_json_response(&state, &completed, false, &headers))
}
