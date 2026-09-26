use std::{net::SocketAddr, sync::Arc};

use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::HeaderMap,
};
use serde::Serialize;
use utoipa::ToSchema;

use super::{
    common::UploadResponse,
    ticket::{
        check_mint_limit, clamp_ttl_seconds, device_connected, encrypted_client_ip,
        insert_owned_pending, mint_ticket, notify_device, pinned_region_host, public_url_for,
        reject_blocked_filename, ticket_ttl_secs,
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
    let (default_ttl, allowed_ttl, danger, max_file_size, public_juicehost_url) = {
        let jh = state.juicehost_config()?;
        (
            jh.default_ttl_hours,
            jh.allowed_ttl_hours.clone(),
            jh.danger_level,
            jh.max_file_size_bytes,
            state.config.public_juicehost_url.clone(),
        )
    };

    if body.file_size == 0 || body.file_size > max_file_size {
        return Err(AppError::PayloadTooLarge);
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

    let selected_host = body
        .host
        .as_deref()
        .map(str::trim)
        .filter(|host| !host.is_empty());
    if selected_host.is_some() && requested_device_id.is_none() {
        return Err(AppError::BadRequest(
            "direct browser upload does not support custom hosts yet".into(),
        ));
    }

    let selected_host = if requested_device_id.is_some() {
        match selected_host {
            Some(h) => Some(
                crate::storage_client::check_storage_host(h, state.config.allow_private_fetch)
                    .await
                    .map_err(|e| AppError::BadRequest(e.to_string()))?,
            ),
            None => None,
        }
    } else {
        None
    };

    check_mint_limit(&state, &headers, addr)?;

    let now = chrono::Utc::now().timestamp();
    let ttl_secs = ticket_ttl_secs(&state, body.file_size, 900);
    let ttl_seconds = clamp_ttl_seconds(&allowed_ttl, default_ttl, body.ttl_hours);
    let file_id = nanoid::nanoid!(8);
    let delete_token = uuid::Uuid::new_v4().to_string();
    let encrypted_ip = encrypted_client_ip(&state, &headers, addr);

    let region_juicehost = pinned_region_host(&state, &headers, addr);
    let storage_host = selected_host.clone().or(region_juicehost.clone());
    let record = FileRecord::new(
        file_id.clone(),
        body.filename.clone(),
        body.mime_type.clone(),
        body.file_size as i64,
        delete_token.clone(),
        now,
        now + ttl_seconds,
        encrypted_ip,
        storage_host,
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

    if let Some(ref wanted) = requested_device_id {
        let download_url = public_url_for(&state, &record);
        notify_device(
            &state,
            &user_id,
            wanted,
            &file_id,
            &body.filename,
            &body.mime_type,
            body.file_size,
            &ticket,
            &download_url,
        )
        .await;
    }

    let public_url = public_url_for(&state, &record);
    let upload_url = format!(
        "{}/internal/file/upload/{}",
        region_juicehost
            .as_deref()
            .unwrap_or_else(|| public_juicehost_url.trim_end_matches('/')),
        file_id
    );

    Ok(Json(DirectUploadReserveResponse {
        ticket,
        file_id,
        upload_url,
        url: public_url,
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
    Json(body): Json<DirectUploadCompleteRequest>,
) -> Result<Json<UploadResponse>, AppError> {
    use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode as jwt_decode};
    let mut validation = Validation::new(Algorithm::HS256);
    validation.set_required_spec_claims(&["exp"]);
    validation.set_issuer(&["juiceback-ticket"]);
    let ticket = jwt_decode::<serde_json::Value>(
        &body.ticket,
        &DecodingKey::from_secret(state.config.ticket_jwt_secret.as_bytes()),
        &validation,
    )
    .map_err(|_| AppError::Unauthorized("Invalid upload ticket".into()))?
    .claims;
    let ticket_file_id = ticket.get("file_id").and_then(|v| v.as_str()).unwrap_or("");
    let ticket_user_id = ticket.get("user_id").and_then(|v| v.as_str()).unwrap_or("");
    if ticket_file_id != body.file_id || ticket_user_id != user_id {
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
    if file.status != "uploading" {
        return Err(AppError::Conflict("upload is already complete".into()));
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

    let filename = file.filename.clone();
    let mime_type = file.mime_type.clone();
    let size_bytes = file.size_bytes;
    let reservation_token = file.delete_token.clone();
    let update_id = file_id.clone();
    match state
        .db_call("complete_direct_upload", move |db| {
            db::finish_reservation(
                db,
                &filename,
                &mime_type,
                size_bytes,
                &update_id,
                &reservation_token,
                None,
            )
        })
        .await?
    {
        db::FinishReservationResult::Completed(_) => {}
        db::FinishReservationResult::NotFound => return Err(AppError::NotFound),
        db::FinishReservationResult::NotUploading => {
            return Err(AppError::Conflict("upload is already complete".into()));
        }
        db::FinishReservationResult::InvalidToken => {
            return Err(AppError::Unauthorized("invalid upload ownership".into()));
        }
    }

    let public_url = public_url_for(&state, &file);
    Ok(Json(UploadResponse {
        id: file_id,
        url: public_url,
        filename: file.filename,
        size_bytes,
        mime_type: file.mime_type,
        expires_at: file.expires_at,
        delete_token: String::new(),
        status: "ready".into(),
    }))
}
