use std::{net::SocketAddr, sync::Arc};

use axum::{
    Json,
    extract::{ConnectInfo, Multipart, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use utoipa::ToSchema;

use super::{common::UploadResponse, parse::parse_multipart};
use crate::{
    db::{self, FileRecord},
    error::AppError,
    routes::{UserId, noscript},
    state::AppState,
    upload_mode::UploadMode,
};
#[derive(serde::Deserialize, ToSchema)]
pub struct ReserveUploadRequest {
    pub filename: Option<String>,
    pub mime_type: Option<String>,
    pub ttl_hours: Option<f64>,
    pub host: Option<String>,
    /// Optional password gate. Relay-only: complete via `POST /upload`,
    /// never via direct/ticket PUT.
    pub password: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct ReserveResponse {
    pub id: String,
    pub url: String,
    pub delete_token: String,
    pub status: String,
    pub protected: bool,
    pub is_encrypted: bool,
}

#[utoipa::path(
    post,
    path = "/upload/reserve",
    request_body = ReserveUploadRequest,
    responses(
        (status = 200, description = "Upload slot reserved", body = ReserveResponse),
        (status = 429, description = "Rate limit exceeded"),
    ),
    tag = "Uploads",
)]
#[tracing::instrument(skip_all)]
pub async fn reserve_upload_handler(
    State(state): State<Arc<AppState>>,
    UserId(user_id): UserId,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<ReserveUploadRequest>,
) -> Result<(StatusCode, Json<ReserveResponse>), AppError> {
    let (default_ttl, allowed_ttl, danger) = {
        let jh = state.juicehost_config()?;
        (
            jh.default_ttl_hours,
            jh.allowed_ttl_hours.clone(),
            jh.danger_level,
        )
    };

    let now = chrono::Utc::now().timestamp();
    let delete_token = uuid::Uuid::new_v4().to_string();

    let ttl_seconds = super::ticket::clamp_ttl_seconds(&allowed_ttl, default_ttl, body.ttl_hours);

    let filename = body.filename.unwrap_or_else(|| "upload".to_string());
    let mime_type = body
        .mime_type
        .unwrap_or_else(|| "application/octet-stream".to_string());

    super::ticket::reject_blocked_filename(&filename, danger)?;

    let selected_host = body
        .host
        .map(|h| h.trim().trim_end_matches('/').to_string())
        .filter(|h| !h.is_empty());

    let file_id = nanoid::nanoid!(8);

    let encrypted_ip = super::ticket::encrypted_client_ip(&state, &headers, addr);

    // A protected reservation mints its per-file data key now, while the
    // password is in memory. Completion reuses the row material, so the
    // form never needs to resend the password.
    let setup = match body.password.filter(|p| !p.is_empty()) {
        Some(password) => Some(super::protected::prepare_protection(&state, &password).await?),
        None => None,
    };

    let mut record = FileRecord::new(
        file_id.clone(),
        filename,
        mime_type,
        0,
        delete_token.clone(),
        now,
        now + ttl_seconds,
        encrypted_ip,
        selected_host.clone(),
    );
    if let Some(ref setup) = setup {
        record.password_hash = Some(setup.password_hash.clone());
        record.is_encrypted = true;
        record.dek_wrapped = Some(setup.wrapped_b64.clone());
        record.dek_salt = Some(setup.salt_b64.clone());
        record.dek_escrow = Some(setup.escrow_hex.clone());
        record.key_version = crate::crypto_file::KEY_VERSION_V1;
    }

    let record =
        super::ticket::insert_owned_pending(&state, &user_id, record, "own_reserved_file").await?;

    let public_url = super::ticket::public_url_for(&state, &record);

    tracing::info!("reserve: id={} host={:?}", file_id, selected_host);

    Ok((
        StatusCode::OK,
        Json(ReserveResponse {
            id: file_id,
            url: public_url,
            delete_token,
            status: "uploading".to_string(),
            protected: record.is_protected(),
            is_encrypted: record.is_encrypted,
        }),
    ))
}

fn build_upload_response(
    headers: &HeaderMap,
    record: &FileRecord,
    public_url: &str,
    wants_html: bool,
    ttl_seconds: i64,
    now: i64,
) -> Result<Response, AppError> {
    if wants_html {
        return Ok(noscript::upload_redirect(
            headers,
            public_url,
            &record.filename,
            &record.delete_token,
            &record.id,
            &record.mime_type,
            record.size_bytes,
            now,
            now + ttl_seconds,
        ));
    }

    let entry = noscript::FileCookieEntry {
        id: record.id.clone(),
        token: record.delete_token.clone(),
    };
    let cookie_hdr = noscript::file_cookie_header(headers, &entry);

    let mut resp = Json(UploadResponse {
        url: public_url.to_string(),
        id: record.id.clone(),
        filename: record.filename.clone(),
        size_bytes: record.size_bytes,
        mime_type: record.mime_type.clone(),
        expires_at: record.expires_at,
        delete_token: record.delete_token.clone(),
        status: record.status.clone(),
        protected: record.is_protected(),
        is_encrypted: record.is_encrypted,
    })
    .into_response();

    resp.headers_mut().insert(header::SET_COOKIE, cookie_hdr);
    Ok(resp)
}

#[utoipa::path(
    post,
    path = "/upload",
    request_body(content_type = "multipart/form-data", description = "File upload with optional metadata fields (host, upload_mode, ttl_hours). Completing reserve_id requires x-delete-token."),
    responses(
        (status = 200, description = "File uploaded successfully", body = UploadResponse),
        (status = 413, description = "File too large"),
        (status = 403, description = "Missing or invalid reservation delete token"),
        (status = 429, description = "Rate limit exceeded"),
    ),
    tag = "Uploads",
)]
#[tracing::instrument(skip_all)]
pub async fn upload_handler(
    State(state): State<Arc<AppState>>,
    UserId(user_id): UserId,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Response, AppError> {
    let handler_start = std::time::Instant::now();
    let raw_ip = crate::utils::client_ip(&headers, addr.ip(), &state).to_string();
    let encrypted_ip = crate::utils::encrypt_ip(&raw_ip, &state.config.ip_encryption_key);
    let hashed_ip = crate::utils::hash_ip_for_ban(&raw_ip, &state.config.ip_pepper);

    let _permit = state
        .upload_semaphore
        .acquire()
        .await
        .map_err(|_| AppError::TaskPanicked("upload semaphore closed".into()))?;

    let wants_html = noscript::wants_html(&headers);
    let is_gzip = headers.get("x-file-encoding").and_then(|v| v.to_str().ok()) == Some("gzip");

    let parse_start = std::time::Instant::now();
    let reservation_token = headers
        .get("x-delete-token")
        .and_then(|value| value.to_str().ok());
    let fresh_capability = uuid::Uuid::new_v4().to_string();
    // The multipart state machine is large; keep it on the heap so debug
    // builds (2 MiB worker stacks) don't overflow while polling it.
    let params = Box::pin(parse_multipart(
        &mut multipart,
        &state,
        is_gzip,
        reservation_token,
        fresh_capability,
    ))
    .await?;
    tracing::debug!("multipart parse total took {:?}", parse_start.elapsed());

    let now = chrono::Utc::now().timestamp();

    // Protection was resolved while parsing (fresh setup from the form
    // password, or the reservation row's material). The reservation's
    // verifier wins when both exist - never mix a form password in.
    let protection = params.protection;
    let protected = protection.is_some();
    if protected && params.enc_header.is_none() {
        return Err(AppError::Internal("protected upload missing ciphertext".into()));
    }

    let record = if let Some(ref existing) = params.reservation {
        let update_id = existing.id.clone();
        let delete_token = existing.delete_token.clone();
        let fname = params.filename.clone();
        let mtype = params.mime_type.clone();
        let enc_ip = encrypted_ip.clone();
        let completed = match state
            .db_call("finish_reservation", move |db| {
                crate::db::finish_reservation(
                    db,
                    &fname,
                    &mtype,
                    params.total_bytes,
                    &update_id,
                    &delete_token,
                    enc_ip.as_deref(),
                )
            })
            .await?
        {
            db::FinishReservationResult::Completed(record) => record,
            db::FinishReservationResult::NotFound => {
                return Err(AppError::BadRequest("invalid reserve_id".into()));
            }
            db::FinishReservationResult::NotUploading => {
                return Err(AppError::BadRequest("reservation is not uploading".into()));
            }
            db::FinishReservationResult::InvalidToken => {
                return Err(AppError::Forbidden(
                    "invalid reservation delete token".into(),
                ));
            }
        };

        tracing::info!(
            "reserved upload completed: id={} size={}",
            existing.id,
            params.total_bytes
        );
        let mut completed = completed;
        if let Some(ref upload_protection) = protection {
            let header = params.enc_header.ok_or_else(|| {
                AppError::Internal("protected upload missing ciphertext".into())
            })?;
            let material = upload_protection.material(hex::encode(header));
            super::protected::mark_protected(&state, &completed.id, &material).await?;
            completed.password_hash = material.password_hash.clone();
            completed.is_encrypted = material.is_encrypted;
            completed.enc_header = material.enc_header.clone();
            completed.dek_wrapped = material.dek_wrapped.clone();
            completed.dek_salt = material.dek_salt.clone();
            completed.dek_escrow = material.dek_escrow.clone();
            completed.key_version = material.key_version;
        }
        completed
    } else {
        let mut record = FileRecord::new(
            params.file_id.clone(),
            params.filename.clone(),
            params.mime_type.clone(),
            params.total_bytes,
            params.file_capability.clone(),
            now,
            now + params.ttl_seconds,
            encrypted_ip.clone(),
            params.selected_host.clone(),
        );
        if let Some(ref upload_protection) = protection {
            let header = params.enc_header.ok_or_else(|| {
                AppError::Internal("protected upload missing ciphertext".into())
            })?;
            let material = upload_protection.material(hex::encode(header));
            record.password_hash = material.password_hash.clone();
            record.is_encrypted = material.is_encrypted;
            record.enc_header = material.enc_header.clone();
            record.dek_wrapped = material.dek_wrapped.clone();
            record.dek_salt = material.dek_salt.clone();
            record.dek_escrow = material.dek_escrow.clone();
            record.key_version = material.key_version;
        }

        let db_start = std::time::Instant::now();
        let record = db::insert_new_file(&state, record).await?;
        tracing::debug!("db insert took {:?}", db_start.elapsed());
        Box::new(record)
    };

    let owned_record = record.clone();
    state
        .db_call("own_uploaded_file", move |db| {
            db::add_client_file(db, &user_id, &owned_record)
        })
        .await?;

    tracing::info!(
        "upload: id={} size={} mime={} expires_at={} ip={}",
        record.id,
        record.size_bytes,
        record.mime_type,
        record.expires_at,
        crate::utils::truncate_hash(&hashed_ip)
    );

    if params.selected_upload_mode == UploadMode::Quic {
        crate::utils::log_quic_throughput(
            &record.id,
            record.size_bytes as u64,
            handler_start.elapsed(),
            parse_start.elapsed(),
        );
    }

    let public_url = crate::utils::share_url(
        &state.config.public_base_url,
        record.storage_host.as_deref(),
        &record.id,
        &record.filename,
        record.is_protected(),
    );

    build_upload_response(
        &headers,
        &record,
        &public_url,
        wants_html,
        params.ttl_seconds,
        now,
    )
}
