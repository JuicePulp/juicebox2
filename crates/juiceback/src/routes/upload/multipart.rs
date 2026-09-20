use std::{net::SocketAddr, sync::Arc};

use async_compression::tokio::bufread::GzipDecoder;
use axum::{
    Json,
    extract::{ConnectInfo, Multipart, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use serde::Serialize;
use tokio::{
    io::{AsyncReadExt, BufReader},
    sync::mpsc,
};
use tokio_util::io::StreamReader;
use utoipa::ToSchema;

use super::common::{UploadResponse, sanitize_filename};
use crate::{
    db::{self, FileRecord},
    error::AppError,
    routes::{UserId, noscript},
    state::AppState,
    upload_mode::UploadMode,
};

/// Parsed multipart upload parameters, extracted from the multipart stream.
struct UploadParams {
    file_id: String,
    filename: String,
    mime_type: String,
    total_bytes: i64,
    selected_host: Option<String>,
    selected_upload_mode: UploadMode,
    ttl_seconds: i64,
    reservation: Option<FileRecord>,
    file_capability: String,
}

/// Spawn the background task that streams channel chunks to juicehost.
/// Shared by the plain and gzip streaming upload paths so both get identical
/// backpressure behavior (`STREAM_CHANNEL_CAPACITY` in-flight chunks).
fn spawn_upload_push(
    state: &Arc<AppState>,
    file_id: &str,
    filename: &str,
    mime_type: &str,
    host: &Option<String>,
    upload_mode: UploadMode,
    capability: &str,
) -> (
    mpsc::Sender<Result<Bytes, String>>,
    tokio::task::JoinHandle<Result<(), String>>,
) {
    let (tx, rx) =
        mpsc::channel::<Result<Bytes, String>>(crate::constants::STREAM_CHANNEL_CAPACITY);

    let state_push = Arc::clone(state);
    let push_id = file_id.to_string();
    let push_filename = filename.to_string();
    let push_mime = mime_type.to_string();
    let push_host = host.clone();
    let push_mode = upload_mode;
    let push_capability = capability.to_string();

    let push_handle = tokio::spawn(async move {
        crate::storage_client::push_file_streaming(
            &state_push,
            &push_id,
            &push_filename,
            &push_mime,
            rx,
            push_host,
            &push_mode,
            Some(&push_capability),
        )
        .await
    });
    (tx, push_handle)
}

async fn stream_upload_to_juicehost(
    mut field: axum::extract::multipart::Field<'_>,
    state: &Arc<AppState>,
    file_id: &str,
    filename: &str,
    mime_type: &str,
    host: &Option<String>,
    upload_mode: UploadMode,
    max_size: i64,
    capability: &str,
) -> Result<(i64, u64, std::time::Duration, std::time::Duration), AppError> {
    let (tx, push_handle) = spawn_upload_push(
        state,
        file_id,
        filename,
        mime_type,
        host,
        upload_mode,
        capability,
    );

    let read_start = std::time::Instant::now();
    let mut total_bytes: i64 = 0;
    let mut chunk_count: u64 = 0;
    let mut push_backpressure: std::time::Duration = std::time::Duration::default();

    let first_chunk = field
        .chunk()
        .await
        .map_err(|e| {
            tracing::warn!("stream upload chunk read error: {:?}", e);
            AppError::InvalidMultipart
        })?
        .unwrap_or_default();

    if !first_chunk.is_empty() {
        let level = state
            .jh_config
            .read()
            .unwrap()
            .as_ref()
            .map(|c| c.danger_level)
            .unwrap_or(crate::file_validation::ProtectionLevel::High);
        if let Some(err) = crate::file_validation::blocked_file_error(
            crate::file_validation::validate_file(filename, &first_chunk, level),
        ) {
            return Err(err);
        }
    }

    total_bytes += first_chunk.len() as i64;
    if total_bytes > max_size {
        return Err(AppError::PayloadTooLarge);
    }
    chunk_count += 1;
    let send_start = std::time::Instant::now();
    if tx.send(Ok(first_chunk)).await.is_err() {
        // Channel closed, push task panicked
        return Err(AppError::TaskPanicked(
            "streaming push task panicked".into(),
        ));
    }
    push_backpressure += send_start.elapsed();

    while let Some(chunk) = field.chunk().await.map_err(|e| {
        tracing::warn!("stream upload chunk read error: {:?}", e);
        AppError::InvalidMultipart
    })? {
        total_bytes += chunk.len() as i64;
        if total_bytes > max_size {
            return Err(AppError::PayloadTooLarge);
        }
        chunk_count += 1;
        let send_start = std::time::Instant::now();
        if tx.send(Ok(chunk)).await.is_err() {
            break;
        }
        push_backpressure += send_start.elapsed();
    }
    drop(tx);
    let read_time = read_start.elapsed();

    let stream_start = std::time::Instant::now();
    push_handle
        .await
        .map_err(|_| AppError::TaskPanicked("streaming push task panicked".into()))?
        .map_err(AppError::from_juicehost_error)?;
    let push_wait = stream_start.elapsed();

    let read_bps = if read_time.as_secs_f64() > 0.0 {
        total_bytes as f64 / read_time.as_secs_f64()
    } else {
        0.0
    };
    tracing::debug!(
        "  stream: id={} bytes={} chunks={} read={:?} ({:.2} MB/s) \
         backpressure={:?} push_wait={:?}",
        file_id,
        total_bytes,
        chunk_count,
        read_time,
        read_bps / crate::constants::BYTES_PER_MIB,
        push_backpressure,
        push_wait,
    );

    Ok((total_bytes, chunk_count, push_backpressure, push_wait))
}

/// Incrementally gunzip a chunk stream into the storage push channel.
///
/// Wire bytes are already capped by the caller; decoded output aborts at
/// `max_size` (zip-bomb guard). File-type validation runs on a small sniff
/// prefix before the first byte is forwarded, mirroring the old buffered
/// path. Returns total decoded bytes. Pure apart from the channel send, so
/// unit tests can drive it with synthetic streams (no `AppState` needed).
async fn pipe_gunzip_to_sender<S>(
    stream: S,
    tx: &mpsc::Sender<Result<Bytes, String>>,
    filename: &str,
    danger: crate::file_validation::ProtectionLevel,
    max_size: i64,
) -> Result<i64, AppError>
where
    S: futures::Stream<Item = Result<Bytes, std::io::Error>>,
{
    let decoder = GzipDecoder::new(BufReader::new(StreamReader::new(Box::pin(stream))));
    // StreamReader is !Unpin, so pin the decoder stack-local before AsyncRead use.
    tokio::pin!(decoder);
    let mut buf = [0_u8; crate::constants::GZIP_READ_BUFFER_SIZE];
    let mut sniff: Vec<u8> = Vec::new();
    let mut validated = false;
    let mut forwarded_any = false;
    let mut decoded_total: i64 = 0;
    loop {
        let n = decoder
            .read(&mut buf)
            .await
            .map_err(|_| AppError::GzipDecodeFailed)?;
        if n == 0 {
            break;
        }
        decoded_total += n as i64;
        if decoded_total > max_size {
            return Err(AppError::PayloadTooLarge);
        }
        if !validated {
            sniff.extend_from_slice(&buf[..n]);
            if sniff.len() < crate::constants::FILE_SNIFF_PREFIX_LEN {
                continue;
            }
            validate_decoded_prefix(&sniff, filename, danger)?;
            if tx
                .send(Ok(Bytes::from(std::mem::take(&mut sniff))))
                .await
                .is_err()
            {
                return Err(AppError::TaskPanicked(
                    "streaming push task panicked".into(),
                ));
            }
            forwarded_any = true;
            validated = true;
        } else if tx
            .send(Ok(Bytes::copy_from_slice(&buf[..n])))
            .await
            .is_err()
        {
            if forwarded_any {
                break;
            }
            return Err(AppError::TaskPanicked(
                "streaming push task panicked".into(),
            ));
        } else {
            forwarded_any = true;
        }
    }
    if !validated {
        // Short (or empty) stream: validate whatever prefix arrived, so empty
        // and truncated inputs follow the same path as the buffered code did.
        validate_decoded_prefix(&sniff, filename, danger)?;
        if !sniff.is_empty() {
            tx.send(Ok(Bytes::from(sniff)))
                .await
                .map_err(|_| AppError::TaskPanicked("streaming push task panicked".into()))?;
        }
    }
    Ok(decoded_total)
}

fn validate_decoded_prefix(
    data: &[u8],
    filename: &str,
    danger: crate::file_validation::ProtectionLevel,
) -> Result<(), AppError> {
    if let Some(err) = crate::file_validation::blocked_file_error(
        crate::file_validation::validate_file(filename, data, danger),
    ) {
        return Err(err);
    }
    Ok(())
}

/// Streaming counterpart of `stream_upload_to_juicehost` for
/// `X-File-Encoding: gzip` fields: network chunks are gunzipped incrementally
/// and forwarded decoded, so memory stays at channel capacity instead of
/// 2x file size. Wire bytes are capped at `max_size` while reading (parity
/// with the old raw buffer cap); decoded bytes are capped inside
/// `pipe_gunzip_to_sender`.
#[expect(
    clippy::too_many_arguments,
    reason = "pipeline fns thread established context (state, ids, tokens); bundling params churns callers for no behavior gain"
)]
async fn stream_gzip_upload_to_juicehost(
    field: axum::extract::multipart::Field<'_>,
    state: &Arc<AppState>,
    file_id: &str,
    filename: &str,
    mime_type: &str,
    host: &Option<String>,
    upload_mode: UploadMode,
    max_size: i64,
    capability: &str,
    danger: crate::file_validation::ProtectionLevel,
) -> Result<(i64, std::time::Duration, std::time::Duration), AppError> {
    let (tx, push_handle) = spawn_upload_push(
        state,
        file_id,
        filename,
        mime_type,
        host,
        upload_mode,
        capability,
    );

    let read_start = std::time::Instant::now();
    // Own the field and the running wire total in the unfold state: no
    // borrows, so the resulting stream feeds StreamReader directly.
    let wire_cap = max_size;
    let chunk_stream = futures::stream::unfold((field, 0_i64), |(mut f, mut total)| async move {
        match f.chunk().await {
            Ok(Some(chunk)) => {
                total += chunk.len() as i64;
                if total > wire_cap {
                    let err =
                        std::io::Error::new(std::io::ErrorKind::Other, "wire size limit exceeded");
                    return Some((Err(err), (f, total)));
                }
                Some((Ok(chunk), (f, total)))
            }
            Ok(None) => None,
            Err(e) => {
                let err = std::io::Error::new(std::io::ErrorKind::Other, e.body_text());
                Some((Err(err), (f, total)))
            }
        }
    });

    let decode_result =
        pipe_gunzip_to_sender(Box::pin(chunk_stream), &tx, filename, danger, max_size).await;
    drop(tx);
    let read_time = read_start.elapsed();
    let total_bytes = decode_result?;

    let stream_start = std::time::Instant::now();
    push_handle
        .await
        .map_err(|_| AppError::TaskPanicked("streaming push task panicked".into()))?
        .map_err(AppError::from_juicehost_error)?;
    let push_wait = stream_start.elapsed();

    tracing::debug!(
        "  stream (gzip): id={} bytes={} read={:?} push_wait={:?}",
        file_id,
        total_bytes,
        read_time,
        push_wait,
    );

    Ok((total_bytes, read_time, push_wait))
}

async fn parse_multipart(
    multipart: &mut Multipart,
    state: &Arc<AppState>,
    is_gzip: bool,
    reservation_token: Option<&str>,
    default_capability: String,
) -> Result<UploadParams, AppError> {
    let (default_ttl, allowed_ttl, max_size, danger) = {
        let jh = state.juicehost_config()?;
        (
            jh.default_ttl_hours,
            jh.allowed_ttl_hours.clone(),
            jh.max_file_size_bytes,
            jh.danger_level,
        )
    };
    let mut ttl_seconds = default_ttl as i64 * crate::constants::SECONDS_PER_HOUR;
    let mut file_id = String::new();
    let mut sanitized_filename = String::new();
    let mut mime_type = String::new();
    let mut total_bytes: i64 = 0;
    let mut selected_host: Option<String> = None;
    let mut selected_upload_mode = UploadMode::Standard;
    let mut reservation = None;
    let mut file_capability = default_capability;

    while let Some(field) = multipart.next_field().await.map_err(|e| {
        tracing::warn!("multipart field parse error: {:?}", e);
        AppError::InvalidMultipart
    })? {
        let name = field.name().unwrap_or("").to_string();
        let field_start = std::time::Instant::now();

        if name == "ttl_hours" {
            let text = field.text().await.map_err(|_| AppError::InvalidMultipart)?;
            if let Ok(val) = text.parse::<f64>() {
                ttl_seconds = (crate::constants::clamp_to_nearest(val, &allowed_ttl)
                    * crate::constants::SECONDS_PER_HOUR_F64)
                    .round() as i64;
            }
            tracing::debug!("  field ttl_hours took {:?}", field_start.elapsed());
        } else if name == "upload_mode" {
            let text = field.text().await.map_err(|_| AppError::InvalidMultipart)?;
            selected_upload_mode = UploadMode::from(text.trim());
            tracing::debug!("  field upload_mode took {:?}", field_start.elapsed());
        } else if name == "host" {
            let text = field.text().await.map_err(|_| AppError::InvalidMultipart)?;
            let text = text.trim().trim_end_matches('/').to_string();
            if !text.is_empty() {
                let checked = crate::storage_client::check_storage_host(
                    &text,
                    state.config.allow_private_fetch,
                )
                .await
                .map_err(|_| AppError::InvalidMultipart)?;
                selected_host = Some(checked);
            }
            tracing::debug!("  field host took {:?}", field_start.elapsed());
        } else if name == "reserve_id" {
            let text = field.text().await.map_err(|_| AppError::InvalidMultipart)?;
            let text = text.trim().to_string();
            if !text.is_empty() {
                if !crate::utils::is_valid_id(&text) {
                    return Err(AppError::BadRequest("invalid reserve_id".into()));
                }
                let token = reservation_token.ok_or_else(|| {
                    AppError::Forbidden("reservation delete token required".into())
                })?;
                let lookup_id = text.clone();
                let existing = state
                    .db_call("get_reserved_file", move |db| db::get_file(db, &lookup_id))
                    .await?
                    .ok_or_else(|| AppError::BadRequest("invalid reserve_id".into()))?;
                if existing.status != "uploading" {
                    return Err(AppError::BadRequest("reservation is not uploading".into()));
                }
                if !crate::utils::constant_time_eq(&existing.delete_token, token) {
                    return Err(AppError::Forbidden(
                        "invalid reservation delete token".into(),
                    ));
                }
                file_id = text;
                file_capability.clone_from(&existing.delete_token);
                reservation = Some(existing);
            }
            tracing::debug!("  field reserve_id took {:?}", field_start.elapsed());
        } else if name == "file" {
            let filename = field.file_name().unwrap_or("upload").to_string();
            sanitized_filename = sanitize_filename(&filename);

            if let Some(ref existing) = reservation {
                selected_host = existing.storage_host.clone();
            }

            mime_type = field
                .content_type()
                .map(ToString::to_string)
                .unwrap_or_else(|| {
                    mime_guess::from_path(&sanitized_filename)
                        .first_or_octet_stream()
                        .to_string()
                });

            if file_id.is_empty() {
                file_id = nanoid::nanoid!(8);
            }

            if is_gzip {
                let (bytes, read_time, push_wait) = stream_gzip_upload_to_juicehost(
                    field,
                    state,
                    &file_id,
                    &sanitized_filename,
                    &mime_type,
                    &selected_host,
                    selected_upload_mode,
                    max_size as i64,
                    &file_capability,
                    danger,
                )
                .await?;
                total_bytes = bytes;
                let gzip_bps = if read_time.as_secs_f64() > 0.0 {
                    total_bytes as f64 / read_time.as_secs_f64()
                } else {
                    0.0
                };
                tracing::debug!(
                    "  field file (gzip): read={:?} bytes={} {:.2} MB/s push_wait={:?}",
                    read_time,
                    total_bytes,
                    gzip_bps / crate::constants::BYTES_PER_MIB,
                    push_wait,
                );
            } else {
                let (bytes, _chunks, _backpressure, _push_wait) = stream_upload_to_juicehost(
                    field,
                    state,
                    &file_id,
                    &sanitized_filename,
                    &mime_type,
                    &selected_host,
                    selected_upload_mode,
                    max_size as i64,
                    &file_capability,
                )
                .await?;
                total_bytes = bytes;
            }
            break;
        }
    }

    Ok(UploadParams {
        file_id,
        filename: sanitized_filename,
        mime_type,
        total_bytes,
        selected_host,
        selected_upload_mode,
        ttl_seconds,
        reservation,
        file_capability,
    })
}

#[derive(serde::Deserialize, ToSchema)]
pub struct ReserveUploadRequest {
    pub filename: Option<String>,
    pub mime_type: Option<String>,
    pub ttl_hours: Option<f64>,
    pub host: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct ReserveResponse {
    pub id: String,
    pub url: String,
    pub delete_token: String,
    pub status: String,
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

    let mut ttl_seconds = default_ttl as i64 * crate::constants::SECONDS_PER_HOUR;
    if let Some(hours) = body.ttl_hours {
        let clamped = crate::constants::clamp_to_nearest(hours, &allowed_ttl);
        ttl_seconds = (clamped * crate::constants::SECONDS_PER_HOUR_F64).round() as i64;
    }

    let filename = body.filename.unwrap_or_else(|| "upload".to_string());
    let mime_type = body
        .mime_type
        .unwrap_or_else(|| "application/octet-stream".to_string());

    // Validate filename extension before reserving (use a dummy empty slice for
    // magic byte check; we only have the filename at reserve time). Full
    // validation happens on upload.
    if danger != crate::file_validation::ProtectionLevel::None {
        if let crate::file_validation::FileValidation::BlockedExtension { tier, .. } =
            crate::file_validation::validate_filename(&filename, danger)
        {
            return Err(AppError::BlockedFileType(
                crate::file_validation::friendly_block_reason(tier),
            ));
        }
    }

    let selected_host = body
        .host
        .map(|h| h.trim().trim_end_matches('/').to_string())
        .filter(|h| !h.is_empty());

    let file_id = nanoid::nanoid!(8);

    let encrypted_ip = {
        let raw_ip = crate::utils::client_ip(&headers, addr.ip(), &state).to_string();
        crate::utils::encrypt_ip(&raw_ip, &state.config.ip_encryption_key)
    };

    let record = FileRecord::new(
        file_id.clone(),
        filename,
        mime_type,
        0, // size unknown at reserve time
        delete_token.clone(),
        now,
        now + ttl_seconds,
        encrypted_ip,
        selected_host.clone(),
    );

    let record = db::insert_pending_file(&state, record).await?;
    let owned_record = record.clone();
    state
        .db_call("own_reserved_file", move |db| {
            db::add_client_file(db, &user_id, &owned_record)
        })
        .await?;

    let public_url = crate::utils::public_url(
        &state.config.public_base_url,
        &record.storage_host,
        &file_id,
        &record.filename,
    );

    tracing::info!("reserve: id={} host={:?}", file_id, selected_host);

    Ok((
        StatusCode::OK,
        Json(ReserveResponse {
            id: file_id,
            url: public_url,
            delete_token,
            status: "uploading".to_string(),
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
    let params = parse_multipart(
        &mut multipart,
        &state,
        is_gzip,
        reservation_token,
        fresh_capability,
    )
    .await?;
    tracing::debug!("multipart parse total took {:?}", parse_start.elapsed());

    let now = chrono::Utc::now().timestamp();

    let record = if let Some(ref existing) = params.reservation {
        // Bytes were already streamed to juicehost during parse; only the
        // metadata record remains to be completed.
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
        completed
    } else {
        // Fresh upload (no reservation)
        let record = FileRecord::new(
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

    let public_url = crate::utils::public_url(
        &state.config.public_base_url,
        &record.storage_host,
        &record.id,
        &record.filename,
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

#[cfg(test)]
mod tests {
    use futures::stream;

    use super::*;

    fn gzip_wire(data: &[u8]) -> Vec<u8> {
        use std::io::Write as _;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    fn io_stream(chunks: Vec<Bytes>) -> impl futures::Stream<Item = Result<Bytes, std::io::Error>> {
        stream::iter(chunks.into_iter().map(Ok))
    }

    async fn drain(
        tx: mpsc::Sender<Result<Bytes, String>>,
        mut rx: mpsc::Receiver<Result<Bytes, String>>,
    ) -> Vec<u8> {
        drop(tx);
        let mut out = Vec::new();
        while let Some(item) = rx.recv().await {
            out.extend_from_slice(&item.unwrap());
        }
        out
    }

    #[tokio::test]
    async fn gunzip_roundtrip_multi_chunk() {
        let original = b"The quick brown fox jumps over the lazy dog. ".repeat(5000);
        let wire = gzip_wire(&original);
        // Split wire bytes into odd-sized frames to exercise partial reads.
        let mut frames = Vec::new();
        let mut i = 0;
        while i < wire.len() {
            let end = (i + 7919).min(wire.len());
            frames.push(Bytes::copy_from_slice(&wire[i..end]));
            i = end;
        }
        let (tx, rx) = mpsc::channel(16);
        let total = pipe_gunzip_to_sender(
            io_stream(frames),
            &tx,
            "notes.txt",
            crate::file_validation::ProtectionLevel::High,
            100 * 1024 * 1024,
        )
        .await
        .unwrap();
        assert_eq!(total as usize, original.len());
        assert_eq!(drain(tx, rx).await, original);
    }

    #[tokio::test]
    async fn gunzip_rejects_zip_bomb() {
        let original = vec![0_u8; 4 * 1024 * 1024];
        let wire = gzip_wire(&original);
        assert!(wire.len() < 64 * 1024);
        let (tx, rx) = mpsc::channel(16);
        let err = pipe_gunzip_to_sender(
            io_stream(vec![Bytes::from(wire)]),
            &tx,
            "zeros.bin",
            crate::file_validation::ProtectionLevel::High,
            1024,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, AppError::PayloadTooLarge));
        drop(rx);
    }

    #[tokio::test]
    async fn gunzip_rejects_corrupt_input() {
        let (tx, rx) = mpsc::channel(16);
        let err = pipe_gunzip_to_sender(
            io_stream(vec![Bytes::from_static(b"definitely not gzip")]),
            &tx,
            "notes.txt",
            crate::file_validation::ProtectionLevel::High,
            1024 * 1024,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, AppError::GzipDecodeFailed));
        drop(rx);
    }

    #[tokio::test]
    async fn gunzip_blocks_dangerous_type_before_forwarding() {
        let mut payload = b"MZ".to_vec();
        payload.extend_from_slice(&[0_u8; 2048]);
        let wire = gzip_wire(&payload);
        let (tx, rx) = mpsc::channel(16);
        let err = pipe_gunzip_to_sender(
            io_stream(vec![Bytes::from(wire)]),
            &tx,
            "evil.exe",
            crate::file_validation::ProtectionLevel::High,
            1024 * 1024,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, AppError::BlockedFileType(_)));
        drop(rx);
    }

    #[tokio::test]
    async fn gunzip_rejects_empty_input() {
        let (tx, rx) = mpsc::channel(16);
        let err = pipe_gunzip_to_sender(
            io_stream(vec![]),
            &tx,
            "notes.txt",
            crate::file_validation::ProtectionLevel::High,
            1024 * 1024,
        )
        .await
        .unwrap_err();
        // An empty body claiming gzip encoding is corrupt gzip (truncated
        // stream), not an empty file.
        assert!(matches!(err, AppError::GzipDecodeFailed));
        drop(rx);
    }
}
