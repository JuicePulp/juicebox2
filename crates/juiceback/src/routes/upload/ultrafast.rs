use std::{net::SocketAddr, sync::Arc};

use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::HeaderMap,
};
use jsonwebtoken::{EncodingKey, Header, encode};
use serde::Serialize;
use utoipa::ToSchema;

use super::common::{dte_ticket_ttl_secs, enforce_mint_limit};
use crate::{
    db::{self, FileRecord},
    error::AppError,
    routes::UserId,
    state::AppState,
};

#[derive(serde::Deserialize, ToSchema)]
pub struct UltrafastReserveRequest {
    pub filename: String,
    pub mime_type: String,
    pub file_size: u64,
    pub ttl_hours: Option<f64>,
    pub host: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct UltrafastReserveResponse {
    pub ticket: String,
    pub file_id: String,
    pub url: String,
    pub delete_token: String,
    pub juicehost_url: String,
}

#[utoipa::path(
    post,
    path = "/upload/ultrafast/reserve",
    request_body = UltrafastReserveRequest,
    responses(
        (status = 200, description = "Ticket issued", body = UltrafastReserveResponse),
        (status = 401, description = "Cloudflare Access auth required"),
        (status = 403, description = "No connected devices"),
    ),
    tag = "Uploads",
)]
#[tracing::instrument(skip_all)]
pub async fn ultrafast_reserve_handler(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    UserId(user_id): UserId,
    Json(body): Json<UltrafastReserveRequest>,
) -> Result<Json<UltrafastReserveResponse>, AppError> {
    let has_device = state
        .connected_devices
        .get(&user_id)
        .map(|d| !d.value().is_empty())
        .unwrap_or(false);
    if !has_device {
        return Err(AppError::BadRequest(
            "No connected devices. Pair juicebox-plus first.".into(),
        ));
    }

    let (default_ttl, allowed_ttl, danger, max_file_size) = {
        let jh = state.juicehost_config()?;
        (
            jh.default_ttl_hours,
            jh.allowed_ttl_hours.clone(),
            jh.danger_level,
            jh.max_file_size_bytes,
        )
    };
    if body.file_size == 0 || body.file_size > max_file_size {
        return Err(AppError::PayloadTooLarge);
    }

    enforce_mint_limit(&state, &headers, addr)?;

    let now = chrono::Utc::now().timestamp();
    let ticket_ttl_secs = if state.config.dte_enabled {
        dte_ticket_ttl_secs(&state, body.file_size)
    } else {
        300
    };
    let delete_token = uuid::Uuid::new_v4().to_string();

    let mut ttl_seconds = default_ttl as i64 * crate::constants::SECONDS_PER_HOUR;
    if let Some(hours) = body.ttl_hours {
        let clamped = crate::constants::clamp_to_nearest(hours, &allowed_ttl);
        ttl_seconds = (clamped * crate::constants::SECONDS_PER_HOUR_F64).round() as i64;
    }

    if danger != crate::file_validation::ProtectionLevel::None {
        if let crate::file_validation::FileValidation::BlockedExtension { tier, .. } =
            crate::file_validation::validate_filename(&body.filename, danger)
        {
            return Err(AppError::BlockedFileType(
                crate::file_validation::friendly_block_reason(tier),
            ));
        }
    }

    let file_id = nanoid::nanoid!(8);

    let body_clone_filename = body.filename.clone();
    let body_clone_mime = body.mime_type.clone();
    let body_clone_size = body.file_size;

    let encrypted_ip = {
        let raw_ip = crate::utils::client_ip(&headers, addr.ip(), &state).to_string();
        crate::utils::encrypt_ip(&raw_ip, &state.config.ip_encryption_key)
    };

    let record = FileRecord::new(
        file_id.clone(),
        body.filename.clone(),
        body.mime_type.clone(),
        body.file_size as i64,
        delete_token.clone(),
        now,
        now + ttl_seconds,
        encrypted_ip,
        None,
    );

    let record = db::insert_pending_file(&state, record).await?;
    let owned_record = record.clone();
    let owner = user_id.clone();
    state
        .db_call("own_ultrafast_reservation", move |db| {
            db::add_client_file(db, &owner, &owned_record)
        })
        .await?;

    let device_id = {
        let guard = state
            .connected_devices
            .get(&user_id)
            .ok_or_else(|| AppError::BadRequest("No connected devices".into()))?;
        let first = guard
            .value()
            .first()
            .ok_or_else(|| AppError::BadRequest("No connected devices".into()))?;
        let locked = first
            .try_lock()
            .map_err(|_| AppError::Internal("Device lock poisoned".into()))?;
        locked.device_id.clone()
    };

    let ticket_claims = serde_json::json!({
        "sub": device_id,
        "user_id": user_id,
        "iss": "juiceback-ticket",
        "file_id": file_id,
        "filename": body.filename,
        "mime_type": body.mime_type,
        "file_size": body.file_size,
        "file_capability": delete_token,
        "iat": now as usize,
        "exp": (now + ticket_ttl_secs) as usize,
    });

    let ticket = encode(
        &Header::default(),
        &ticket_claims,
        &EncodingKey::from_secret(state.config.ticket_jwt_secret.as_bytes()),
    )
    .map_err(|e| AppError::Internal(format!("JWT encoding failed: {e}")))?;

    let public_url = crate::utils::public_url(
        &state.config.public_base_url,
        &record.storage_host,
        &file_id,
        &record.filename,
    );

    tracing::info!("ultrafast reserve: id={file_id} user={user_id}");

    let upload_request = serde_json::json!({
        "type": "upload_request",
        "file_id": file_id,
        "filename": body_clone_filename,
        "mime_type": body_clone_mime,
        "file_size": body_clone_size,
        "ticket": ticket.clone(),
        "juicehost_url": state.config.public_juicehost_url,
        "juiceback_url": state.config.public_base_url,
        "download_url": public_url,
    });

    {
        let guard = state
            .connected_devices
            .get(&user_id)
            .ok_or_else(|| AppError::BadRequest("No connected devices".into()))?;
        let first = guard
            .value()
            .first()
            .ok_or_else(|| AppError::BadRequest("No connected devices".into()))?;
        let locked = first
            .try_lock()
            .map_err(|_| AppError::Internal("Device lock poisoned".into()))?;
        let _ = locked.sender.send(upload_request.to_string()).await;
    }

    Ok(Json(UltrafastReserveResponse {
        ticket,
        file_id,
        url: public_url,
        delete_token,
        juicehost_url: state.config.public_juicehost_url.clone(),
    }))
}

#[derive(serde::Deserialize, ToSchema)]
pub struct UltrafastCompleteRequest {
    pub file_id: String,
    /// Real filename from the device (may differ from the placeholder used
    /// during reserve).
    #[serde(default)]
    pub filename: Option<String>,
    /// MIME type from the device.
    #[serde(default)]
    pub mime_type: Option<String>,
    /// File size in bytes from the device.
    #[serde(default)]
    pub file_size: Option<i64>,
    /// JWT ticket that proves the device is authorized for ultrafast
    #[serde(default)]
    pub ticket: Option<String>,
    /// Reservation capability returned by the reserve endpoint.
    #[serde(default)]
    pub delete_token: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct UltrafastCompleteResponse {
    pub url: String,
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub status: String,
    pub delete_token: String,
    pub expires_at: i64,
}

#[utoipa::path(
    post,
    path = "/api/device/upload",
    request_body = UltrafastReserveRequest,
    responses(
        (status = 200, description = "Ticket issued", body = UltrafastReserveResponse),
        (status = 401, description = "Device JWT required"),
        (status = 403, description = "Missing or invalid reservation delete token"),
    ),
    tag = "Uploads",
)]
#[tracing::instrument(skip_all)]
pub async fn device_upload_handler(
    State(state): State<Arc<AppState>>,
    crate::auth::DeviceAuth(claims): crate::auth::DeviceAuth,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<UltrafastReserveRequest>,
) -> Result<Json<UltrafastReserveResponse>, AppError> {
    let device_id = claims.sub;
    let user_id = claims.user_id;

    let (default_ttl, allowed_ttl, danger, max_file_size) = {
        let jh = state.juicehost_config()?;
        (
            jh.default_ttl_hours,
            jh.allowed_ttl_hours.clone(),
            jh.danger_level,
            jh.max_file_size_bytes,
        )
    };
    if body.file_size == 0 || body.file_size > max_file_size {
        return Err(AppError::PayloadTooLarge);
    }

    let now = chrono::Utc::now().timestamp();
    let delete_token = uuid::Uuid::new_v4().to_string();

    let mut ttl_seconds = default_ttl as i64 * crate::constants::SECONDS_PER_HOUR;
    if let Some(hours) = body.ttl_hours {
        let clamped = crate::constants::clamp_to_nearest(hours, &allowed_ttl);
        ttl_seconds = (clamped * crate::constants::SECONDS_PER_HOUR_F64).round() as i64;
    }

    if danger != crate::file_validation::ProtectionLevel::None {
        if let crate::file_validation::FileValidation::BlockedExtension { tier, .. } =
            crate::file_validation::validate_filename(&body.filename, danger)
        {
            return Err(AppError::BlockedFileType(
                crate::file_validation::friendly_block_reason(tier),
            ));
        }
    }

    let file_id = nanoid::nanoid!(8);

    let selected_host = match body
        .host
        .as_ref()
        .map(|h| h.trim().trim_end_matches('/').to_string())
        .filter(|h| !h.is_empty())
    {
        Some(h) => Some(
            crate::storage_client::check_storage_host(&h, state.config.allow_private_fetch)
                .await
                .map_err(|e| AppError::BadRequest(e.to_string()))?,
        ),
        None => None,
    };

    let encrypted_ip = {
        let raw_ip = crate::utils::client_ip(&headers, addr.ip(), &state).to_string();
        crate::utils::encrypt_ip(&raw_ip, &state.config.ip_encryption_key)
    };

    let record = FileRecord::new(
        file_id.clone(),
        body.filename.clone(),
        body.mime_type.clone(),
        body.file_size as i64,
        delete_token.clone(),
        now,
        now + ttl_seconds,
        encrypted_ip,
        selected_host.clone(),
    );

    let record = db::insert_pending_file(&state, record).await?;

    let owned_record = record.clone();
    let owner = user_id.clone();
    state
        .db_call("own_device_reservation", move |db| {
            db::add_client_file(db, &owner, &owned_record)
        })
        .await?;

    let ticket_claims = serde_json::json!({
        "sub": device_id,
        "user_id": user_id,
        "iss": "juiceback-ticket",
        "file_id": file_id,
        "filename": body.filename,
        "mime_type": body.mime_type,
        "file_size": body.file_size,
        "file_capability": delete_token,
        "iat": now as usize,
        "exp": (now + 300) as usize,
    });

    let ticket = encode(
        &Header::default(),
        &ticket_claims,
        &EncodingKey::from_secret(state.config.ticket_jwt_secret.as_bytes()),
    )
    .map_err(|e| AppError::Internal(format!("JWT encoding failed: {e}")))?;

    let public_url = crate::utils::public_url(
        &state.config.public_base_url,
        &selected_host,
        &file_id,
        &record.filename,
    );

    tracing::info!("device upload reserve: id={file_id} device={device_id}");

    Ok(Json(UltrafastReserveResponse {
        ticket,
        file_id,
        url: public_url,
        delete_token,
        juicehost_url: state.config.public_juicehost_url.clone(),
    }))
}

#[utoipa::path(
    post,
    path = "/upload/ultrafast/complete",
    request_body = UltrafastCompleteRequest,
    responses(
        (status = 200, description = "File marked ready", body = UltrafastCompleteResponse),
        (status = 401, description = "Device JWT required"),
        (status = 404, description = "File not found"),
    ),
    tag = "Uploads",
)]
#[tracing::instrument(skip_all)]
pub async fn ultrafast_complete_handler(
    State(state): State<Arc<AppState>>,
    crate::auth::DeviceAuth(claims): crate::auth::DeviceAuth,
    Json(body): Json<UltrafastCompleteRequest>,
) -> Result<Json<UltrafastCompleteResponse>, AppError> {
    let device_id = claims.sub.clone();
    let device_user_id = claims.user_id.clone();

    // Validate ticket JWT for ownership check (proves this device was authorized
    // for this file).
    if let Some(ref ticket_str) = body.ticket {
        use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode as jwt_decode};

        let mut ticket_validation = Validation::new(Algorithm::HS256);
        ticket_validation.set_required_spec_claims(&["exp"]);
        ticket_validation.set_issuer(&["juiceback-ticket"]);

        let ticket = jwt_decode::<serde_json::Value>(
            ticket_str,
            &DecodingKey::from_secret(state.config.ticket_jwt_secret.as_bytes()),
            &ticket_validation,
        )
        .map_err(|e| {
            tracing::warn!("ticket JWT validation failed in complete: {e}");
            AppError::Unauthorized("Invalid upload ticket".into())
        })?;

        let ticket_file_id = ticket
            .claims
            .get("file_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let ticket_user_id = ticket
            .claims
            .get("user_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if ticket_file_id != body.file_id {
            tracing::warn!(
                "ticket file_id mismatch in complete: ticket={ticket_file_id} body={}",
                body.file_id
            );
            return Err(AppError::Unauthorized("Ticket file_id mismatch".into()));
        }
        if ticket_user_id != device_user_id {
            tracing::warn!(
                "ticket user_id mismatch in complete: ticket={ticket_user_id} device={device_user_id}"
            );
            return Err(AppError::Unauthorized("Ticket user_id mismatch".into()));
        }
    }

    let file_id = body.file_id;
    let reservation_token = body
        .delete_token
        .as_deref()
        .ok_or_else(|| AppError::Forbidden("reservation delete token required".into()))?;

    let fid = file_id.clone();
    let file = state
        .db_call("get_file_for_complete", move |db| db::get_file(db, &fid))
        .await?
        .ok_or_else(|| AppError::NotFound)?;

    // Prefer body metadata (real values from the device) over JWT claims (which may
    // be placeholders from reserve).
    let real_filename = body
        .filename
        .or(claims.filename)
        .unwrap_or_else(|| file.filename.clone());
    let real_mime = body
        .mime_type
        .or(claims.mime_type)
        .unwrap_or_else(|| file.mime_type.clone());
    let real_size = body
        .file_size
        .or_else(|| claims.file_size.map(|s| s as i64))
        .unwrap_or(file.size_bytes);

    // Re-validate the real filename (device-provided, may differ from the reserve
    // placeholder) against the configured danger level before marking ready.
    let danger = state
        .jh_config
        .read()
        .unwrap()
        .as_ref()
        .map(|c| c.danger_level)
        .unwrap_or(crate::file_validation::ProtectionLevel::High);
    if danger != crate::file_validation::ProtectionLevel::None {
        if let crate::file_validation::FileValidation::BlockedExtension { tier, .. } =
            crate::file_validation::validate_filename(&real_filename, danger)
        {
            return Err(AppError::BlockedFileType(
                crate::file_validation::friendly_block_reason(tier),
            ));
        }
    }

    // Verify the bytes actually landed on juicehost before marking ready.
    // The device uploads to `public_juicehost_url`, which routes to the same
    // juicehost instance that juiceback talks to via `juicehost_url`, so a stat
    // here reflects what the device uploaded.
    match crate::storage_client::stat_file_on_juicehost(
        &state,
        &file_id,
        file.storage_host.as_deref(),
        Some(&file.delete_token),
    )
    .await
    {
        Ok(Some(stored_size)) if stored_size == real_size as u64 => {}
        Ok(Some(stored_size)) => {
            tracing::warn!(
                "ultrafast complete size mismatch: id={} device={} stored={}",
                file_id,
                real_size,
                stored_size
            );
            return Err(AppError::JuicehostRejected(format!(
                "Uploaded file size mismatch: device reported {real_size} bytes but juicehost stored {stored_size}"
            )));
        }
        Ok(None) => {
            tracing::warn!("ultrafast complete: id={file_id} not found on juicehost");
            return Err(AppError::JuicehostRejected(
                "Uploaded file was not found on juicehost".into(),
            ));
        }
        Err(e) => {
            tracing::warn!("ultrafast complete: id={file_id} stat failed: {e}");
            return Err(AppError::JuicehostUnreachable(format!(
                "Could not verify upload on juicehost: {e}"
            )));
        }
    }

    let update_id = file_id.clone();
    let reservation_token = reservation_token.to_string();
    let ufname = real_filename.clone();
    let umime = real_mime.clone();
    let usize = real_size;
    match state
        .db_call("complete_ultrafast", move |db| {
            crate::db::finish_reservation(
                db,
                &ufname,
                &umime,
                usize,
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
            return Err(AppError::Conflict("reservation is not uploading".into()));
        }
        db::FinishReservationResult::InvalidToken => {
            return Err(AppError::Forbidden(
                "invalid reservation delete token".into(),
            ));
        }
    }

    let public_url = crate::utils::public_url(
        &state.config.public_base_url,
        &file.storage_host,
        &file_id,
        &real_filename,
    );

    tracing::info!("ultrafast complete: id={file_id} device={device_id}");

    Ok(Json(UltrafastCompleteResponse {
        url: public_url,
        id: file_id,
        filename: real_filename,
        mime_type: real_mime,
        size_bytes: real_size,
        status: "ready".to_string(),
        delete_token: file.delete_token,
        expires_at: file.expires_at,
    }))
}
