use std::{net::SocketAddr, sync::Arc};

use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::HeaderMap,
};
use jsonwebtoken::{EncodingKey, Header, encode};
use serde::Serialize;
use utoipa::ToSchema;

use super::common::{UploadResponse, dte_ticket_ttl_secs, enforce_mint_limit, region_host};
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
    if danger != crate::file_validation::ProtectionLevel::None {
        if let crate::file_validation::FileValidation::BlockedExtension { tier, .. } =
            crate::file_validation::validate_filename(&body.filename, danger)
        {
            return Err(AppError::BlockedFileType(
                crate::file_validation::friendly_block_reason(tier),
            ));
        }
    }

    let selected_host = body
        .host
        .as_deref()
        .map(str::trim)
        .filter(|host| !host.is_empty());
    if selected_host.is_some() {
        return Err(AppError::BadRequest(
            "direct browser upload does not support custom hosts yet".into(),
        ));
    }

    enforce_mint_limit(&state, &headers, addr)?;

    let now = chrono::Utc::now().timestamp();
    let ticket_ttl_secs = if state.config.dte_enabled {
        dte_ticket_ttl_secs(&state, body.file_size)
    } else {
        900
    };
    let ttl_seconds = body
        .ttl_hours
        .map(|hours| crate::constants::clamp_to_nearest(hours, &allowed_ttl))
        .map(|hours| (hours * crate::constants::SECONDS_PER_HOUR_F64).round() as i64)
        .unwrap_or_else(|| (default_ttl * crate::constants::SECONDS_PER_HOUR_F64).round() as i64);
    let file_id = nanoid::nanoid!(8);
    let delete_token = uuid::Uuid::new_v4().to_string();
    let encrypted_ip = {
        let raw_ip = crate::utils::client_ip(&headers, addr.ip(), &state).to_string();
        crate::utils::encrypt_ip(&raw_ip, &state.config.ip_encryption_key)
    };

    let region_juicehost = region_host(&state, &headers, addr);
    let record = FileRecord::new(
        file_id.clone(),
        body.filename.clone(),
        body.mime_type.clone(),
        body.file_size as i64,
        delete_token.clone(),
        now,
        now + ttl_seconds,
        encrypted_ip,
        region_juicehost.clone(),
    );
    let record = db::insert_pending_file(&state, record).await?;
    let owned_record = record.clone();
    let owner = user_id.clone();
    state
        .db_call("own_direct_upload", move |db| {
            db::add_client_file(db, &owner, &owned_record)
        })
        .await?;

    let ticket_claims = serde_json::json!({
        "sub": "browser",
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

    let public_url = crate::utils::public_url(
        &state.config.public_base_url,
        &file.storage_host,
        &file_id,
        &file.filename,
    );
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
