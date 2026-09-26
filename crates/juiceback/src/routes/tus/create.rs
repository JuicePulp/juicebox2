use std::{net::SocketAddr, sync::Arc};

use axum::{
    body::Bytes,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use dashmap::mapref::entry::Entry;
use tokio::sync::mpsc;

use super::metadata::{
    find_meta, parse_parallel_number, parse_tus_metadata, validate_parallel_metadata,
};
use crate::{db, error::AppError, state::AppState, tus::TusUpload, upload_mode::UploadMode};

#[utoipa::path(
    post,
    path = "/api/tus",
    request_body(content_type = "application/json", description = "Empty body. File metadata is sent via headers: Upload-Length, Upload-Metadata (base64-encoded pairs)"),
    responses(
        (status = 201, description = "TUS upload session created", headers(
            ("Location" = String, description = "URL of the created upload resource"),
            ("Tus-Resumable" = String, description = "TUS protocol version"),
        )),
        (status = 413, description = "File too large"),
        (status = 429, description = "Rate limit exceeded or too many TUS sessions"),
    ),
    tag = "Uploads",
)]
#[tracing::instrument(skip_all)]
pub async fn create_upload_handler(
    State(state): State<Arc<AppState>>,
    crate::routes::UserId(user_id): crate::routes::UserId,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let raw_ip = crate::utils::client_ip(&headers, addr.ip(), &state).to_string();
    let hashed_ip = crate::utils::hash_ip_for_ban(&raw_ip, &state.config.ip_pepper);
    let encrypted_ip =
        crate::utils::encrypt_ip(&raw_ip, &state.config.ip_encryption_key).unwrap_or_default();

    let total_length = headers
        .get("upload-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .ok_or_else(|| AppError::TusMissingLength)?;

    let (max_size, default_ttl, allowed_ttl, _danger) = {
        let jh = state.juicehost_config()?;
        (
            jh.max_file_size_bytes,
            jh.default_ttl_hours,
            jh.allowed_ttl_hours.clone(),
            jh.danger_level,
        )
    };

    if total_length > max_size {
        return Err(AppError::PayloadTooLarge);
    }

    if state.tus.len() >= crate::constants::MAX_TUS_SESSIONS {
        return Err(AppError::RateLimited);
    }

    let ip_sessions = state
        .tus
        .iter()
        .filter(|entry| entry.hashed_ip == hashed_ip)
        .count();
    if ip_sessions >= crate::constants::MAX_TUS_PER_IP {
        let ip_preview = crate::utils::truncate_hash(&hashed_ip);
        let per_ip_cap = crate::constants::MAX_TUS_PER_IP;
        tracing::warn!("tus: ip={ip_preview} hit per-IP session limit ({per_ip_cap})");
        return Err(AppError::RateLimited);
    }

    let metadata = parse_tus_metadata(headers.get("upload-metadata"));
    let filename = find_meta(&metadata, "filename")
        .unwrap_or("upload")
        .to_string();
    let mime_type = find_meta(&metadata, "mimetype")
        .unwrap_or("application/octet-stream")
        .to_string();
    let ttl_hours = find_meta(&metadata, "ttl")
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(default_ttl);
    let ttl_hours = crate::constants::clamp_to_nearest(ttl_hours, &allowed_ttl);
    let mut storage_host = match find_meta(&metadata, "host")
        .filter(|v| !v.is_empty())
        .map(ToString::to_string)
    {
        Some(h) => Some(
            crate::storage_client::check_storage_host(&h, state.config.allow_private_fetch)
                .await
                .map_err(|e| AppError::BadRequest(e.to_string()))?,
        ),
        None => None,
    };
    let storage_upload_mode = find_meta(&metadata, "upload_mode")
        .map(UploadMode::from)
        .unwrap_or(UploadMode::Standard);

    let session_id = find_meta(&metadata, "session_id").map(ToString::to_string);
    let part_index = parse_parallel_number(find_meta(&metadata, "part_index"))?;
    let total_parts = parse_parallel_number(find_meta(&metadata, "total_parts"))?;
    let parallel = validate_parallel_metadata(session_id.as_deref(), part_index, total_parts)?;

    let reserve_id = find_meta(&metadata, "reserve_id")
        .filter(|v| !v.is_empty())
        .map(|value| {
            if crate::utils::is_valid_id(value) {
                Ok(value.to_string())
            } else {
                Err(AppError::BadRequest("invalid reserve_id".into()))
            }
        })
        .transpose()?;
    let reservation_token = find_meta(&metadata, "delete_token")
        .filter(|v| !v.is_empty())
        .map(ToString::to_string);
    if let Some(ref reserve_id) = reserve_id {
        let token = reservation_token
            .as_deref()
            .ok_or_else(|| AppError::Forbidden("reservation delete token required".into()))?;
        let lookup_id = reserve_id.clone();
        let reservation = state
            .db_call("get_tus_reservation", move |db| {
                db::get_file(db, &lookup_id)
            })
            .await?
            .ok_or_else(|| AppError::BadRequest("invalid reserve_id".into()))?;
        if reservation.status != "uploading" {
            return Err(AppError::BadRequest("reservation is not uploading".into()));
        }
        if !crate::utils::constant_time_eq(&reservation.delete_token, token) {
            return Err(AppError::Forbidden(
                "invalid reservation delete token".into(),
            ));
        }
        storage_host = reservation.storage_host;
    }

    if storage_host.is_none() {
        storage_host = crate::routes::upload::region_host(&state, &headers, addr);
    }

    let id = nanoid::nanoid!(8);
    let mut file_capability = reservation_token
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    if let Some((sid, pi, tp)) = parallel {
        let session = state
            .part_sessions
            .entry(sid.to_string())
            .or_insert_with(|| {
                Arc::new(crate::tus::PartSession {
                    session_id: sid.to_string(),
                    total_parts: tp,
                    filename: filename.clone(),
                    mime_type: mime_type.clone(),
                    hashed_ip: hashed_ip.clone(),
                    storage_host: storage_host.clone(),
                    ttl_hours,
                    reserve_id: reserve_id.clone(),
                    reservation_token: reservation_token.clone(),
                    capability: file_capability.clone(),
                    user_id: user_id.clone(),
                    upload_mode: storage_upload_mode,
                    declared_size: std::sync::atomic::AtomicU64::new(0),
                    completed: std::sync::atomic::AtomicUsize::new(0),
                    part_ids: dashmap::DashMap::new(),
                    part_lengths: dashmap::DashMap::new(),
                    completion_reserve_id: std::sync::Mutex::new(None),
                })
            });
        if session.total_parts != tp
            || session.filename != filename
            || session.mime_type != mime_type
            || session.storage_host != storage_host
            || session.ttl_hours != ttl_hours
            || session.reserve_id != reserve_id
            || session.reservation_token != reservation_token
            || session.upload_mode != storage_upload_mode
        {
            return Err(AppError::BadRequest(
                "parallel upload session metadata mismatch".into(),
            ));
        }
        file_capability = session.capability.clone();
        match session.part_ids.entry(pi) {
            Entry::Occupied(_) => {
                return Err(AppError::BadRequest(
                    "parallel upload part index already exists".into(),
                ));
            }
            Entry::Vacant(entry) => {
                entry.insert(id.clone());
            }
        }
        if session
            .declared_size
            .fetch_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |size| {
                    size.checked_add(total_length)
                        .filter(|sum| *sum <= max_size)
                },
            )
            .is_err()
        {
            session.part_ids.remove(&pi);
            return Err(AppError::PayloadTooLarge);
        }
        session.part_lengths.insert(pi, total_length);
        let part_no = pi + 1;
        tracing::debug!("parallel part registered: session={sid} part={part_no}/{tp} id={id}");
    }

    let (tx, rx) =
        mpsc::channel::<Result<Bytes, String>>(crate::constants::STREAM_CHANNEL_CAPACITY);

    let upload = TusUpload {
        id: id.clone(),
        offset: 0,
        total_length,
        filename: filename.clone(),
        mime_type: mime_type.clone(),
        ttl_hours,
        created_at: chrono::Utc::now().timestamp(),
        delete_token: file_capability.clone(),
        hashed_ip,
        encrypted_ip,
        storage_host: storage_host.clone(),
        session_id,
        part_index,
        reserve_id,
        reservation_token,
        capability: file_capability.clone(),
        patch_lock: Arc::new(tokio::sync::Mutex::new(())),
        user_id,
        upload_mode: storage_upload_mode,
        push_rx: Some(rx),
    };

    state.tus.insert(id.clone(), upload);
    state.tus_senders.insert(id.clone(), tx);

    let location = format!("/api/tus/{id}");
    Ok(Response::builder()
        .status(StatusCode::CREATED)
        .header("Location", &location)
        .header("Tus-Resumable", "1.0.0")
        .body(axum::body::Body::empty())
        .map_err(|_| AppError::Internal("response build failed".into()))?)
}
