use std::sync::Arc;

use axum::{
    Json,
    body::{Body, Bytes},
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use futures::StreamExt;
use tokio::io::AsyncWriteExt;

use super::{
    completion::complete_tus_upload,
    metadata::checked_upload_offset,
    parallel::complete_parallel_part,
    state::{await_storage_push, spawn_push_task},
    types::TusUploadMeta,
};
use crate::{error::AppError, state::AppState};

/// `204` with the new offset, `200` with the file JSON at the end, or `202`
/// when only a parallel part finished.
#[utoipa::path(
    patch,
    path = "/api/tus/{id}",
    params(
        ("id" = String, Path, description = "The TUS upload session ID"),
    ),
    request_body(content_type = "application/offset+octet-stream", description = "Binary chunk of file data; may be gzip-compressed when the X-File-Encoding: gzip header is set (offsets remain in decompressed bytes)"),
    responses(
        (status = 204, description = "Chunk accepted, upload in progress", headers(
            ("Upload-Offset" = String, description = "New byte offset after this chunk"),
            ("Tus-Resumable" = String, description = "TUS protocol version"),
        )),
        (status = 200, description = "Upload complete, file is now live", body = serde_json::Value),
        (status = 404, description = "TUS session not found"),
        (status = 409, description = "Upload offset mismatch"),
        (status = 413, description = "PATCH body exceeds the per-chunk size limit"),
    ),
    tag = "Uploads",
)]
pub async fn patch_upload_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, AppError> {
    let req_offset = headers
        .get("upload-offset")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let content_length = headers
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let started = std::time::Instant::now();
    let res = patch_upload_handler_impl(State(state), Path(id.clone()), headers, body).await;
    let elapsed = started.elapsed();
    match &res {
        Ok(r) => {
            tracing::info!(
                id = %id,
                req_offset,
                content_length,
                resp_offset = r.headers().get("upload-offset").and_then(|v| v.to_str().ok().map(str::to_string)),
                status = r.status().as_u16(),
                elapsed_ms = elapsed.as_millis() as u64,
                "tus_patch ok"
            );
        }
        Err(e) => {
            let code: u16 = match e {
                AppError::TusOffsetMismatch => 409,
                AppError::TusSessionNotFound => 404,
                AppError::TusMissingOffset => 400,
                AppError::PayloadTooLarge => 413,
                AppError::BlockedFileType(_) => 403,
                AppError::GzipDecodeFailed => 400,
                _ => 500,
            };
            tracing::info!(
                id = %id,
                req_offset,
                content_length,
                status = code,
                error = ?e,
                elapsed_ms = elapsed.as_millis() as u64,
                "tus_patch err"
            );
        }
    }
    res
}

/// Rejects as soon as the cap is exceeded rather than buffering everything.
pub(super) async fn collect_bounded_body(body: Body, cap: usize) -> Result<Vec<u8>, AppError> {
    let mut buf = Vec::new();
    let mut stream = body.into_data_stream();
    while let Some(frame) = stream.next().await {
        let chunk = frame.map_err(|_| AppError::BadRequest("patch body stream error".into()))?;
        if buf.len() + chunk.len() > cap {
            return Err(AppError::PayloadTooLarge);
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

/// Caps the decoded size, stopping a small body from expanding into a bomb.
pub(super) fn decode_gzip_bounded(raw: Vec<u8>, cap: usize) -> Result<Bytes, AppError> {
    use std::io::Read as _;
    let mut decoder = flate2::read::GzDecoder::new(raw.as_slice());
    let mut out = Vec::new();
    let mut buf = [0_u8; crate::constants::GZIP_READ_BUFFER_SIZE];
    loop {
        let n = decoder
            .read(&mut buf)
            .map_err(|_| AppError::GzipDecodeFailed)?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
        if out.len() > cap {
            return Err(AppError::PayloadTooLarge);
        }
    }
    Ok(Bytes::from(out))
}

#[tracing::instrument(skip_all)]
async fn patch_upload_handler_impl(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, AppError> {
    let upload_offset = headers
        .get("upload-offset")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .ok_or_else(|| AppError::TusMissingOffset)?;
    let content_length = headers
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let gzip = headers
        .get(juiceproto::headers::FILE_ENCODING)
        .and_then(|v| v.to_str().ok())
        == Some("gzip");

    let reserve_override = headers
        .get("x-reserve-id")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty())
        .map(ToString::to_string);
    let reservation_token_override = headers
        .get(juiceproto::headers::DELETE_TOKEN)
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty())
        .map(ToString::to_string);
    if reserve_override.is_some() && reservation_token_override.is_none() {
        return Err(AppError::Forbidden(
            "reservation delete token required".into(),
        ));
    }

    let patch_lock = state
        .tus
        .get(&id)
        .ok_or(AppError::TusSessionNotFound)?
        .patch_lock
        .clone();
    let _patch_guard = patch_lock.lock().await;

    let (session_id, part_index, filename, total_length, protected) = {
        let upload = state.tus.get(&id).ok_or(AppError::TusSessionNotFound)?;
        if upload_offset != upload.offset {
            return Err(AppError::TusOffsetMismatch);
        }
        if let Some(len) = content_length {
            let _ = checked_upload_offset(upload_offset, len, upload.total_length)?;
        }
        (
            upload.session_id.clone(),
            upload.part_index,
            upload.filename.clone(),
            upload.total_length,
            upload.password_hash.is_some(),
        )
    };

    if let Some(ref reserve_id) = reserve_override {
        // A session can be adopted into a reservation created after it.
        let token = reservation_token_override.as_deref().unwrap_or_default();
        let reservation = crate::reservation::verify_by_id(&state, reserve_id, token).await?;
        let upload_host = state
            .tus
            .get(&id)
            .ok_or(AppError::TusSessionNotFound)?
            .storage_host
            .clone();
        if reservation.storage_host != upload_host {
            return Err(AppError::BadRequest(
                "reservation storage host does not match TUS session".into(),
            ));
        }
        if let Some(mut upload) = state.tus.get_mut(&id)
            && upload.reserve_id.is_none()
        {
            upload.reserve_id = Some(reserve_id.clone());
        }
    }

    if content_length == Some(0) && upload_offset >= total_length {
        // Empty PATCH at the end retries completion without resending bytes.
        return advance_and_maybe_complete(
            &state,
            &id,
            upload_offset,
            &headers,
            session_id.as_deref(),
            part_index,
            reserve_override,
            reservation_token_override,
        )
        .await;
    }

    if gzip || protected {
        // Both paths need the whole chunk in memory; the body cap bounds it.
        let raw = collect_bounded_body(body, crate::constants::TUS_PATCH_MAX_BYTES).await?;
        let body_bytes = if gzip && !raw.is_empty() {
            let cap = crate::constants::TUS_PATCH_MAX_BYTES;
            tokio::task::spawn_blocking(move || decode_gzip_bounded(raw, cap))
                .await
                .map_err(|_| AppError::TaskPanicked("decompression panicked".into()))??
        } else {
            Bytes::from(raw)
        };
        let chunk_len = body_bytes.len() as u64;
        let new_offset = checked_upload_offset(upload_offset, chunk_len, total_length)?;

        if upload_offset == 0
            && !body_bytes.is_empty()
            && let Err(err) = validate_first_chunk(&state, &filename, &body_bytes)
        {
            state.tus.remove(&id);
            state.tus_senders.remove(&id);
            let spool = crate::routes::upload::protected::tus_spool_path(&id);
            let _ = tokio::fs::remove_file(&spool).await;
            return Err(err);
        }

        if protected {
            let spool = crate::routes::upload::protected::tus_spool_path(&id);
            tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&spool)
                .await
                .map_err(|e| AppError::Internal(format!("spool failed: {e}")))?
                .write_all(&body_bytes)
                .await
                .map_err(|e| AppError::Internal(format!("spool failed: {e}")))?;
        } else {
            {
                let mut upload = state.tus.get_mut(&id).ok_or(AppError::TusSessionNotFound)?;
                if let Some(rx) = upload.push_rx.take() {
                    drop(upload);
                    spawn_push_task(&state, &id, rx);
                }
            }
            let sender = state
                .tus_senders
                .get(&id)
                .ok_or(AppError::TusSessionNotFound)?
                .clone();
            let send_start = std::time::Instant::now();
            if sender.send(Ok(body_bytes)).await.is_err() {
                drop(sender);
                abort_session(&state, &id).await;
                return Err(AppError::TusSessionNotFound);
            }
            tracing::debug!(
                "tus patch: chunk_stream took {:?} ({chunk_len} bytes)",
                send_start.elapsed()
            );
            drop(sender);
        }

        return advance_and_maybe_complete(
            &state,
            &id,
            new_offset,
            &headers,
            session_id.as_deref(),
            part_index,
            reserve_override,
            reservation_token_override,
        )
        .await;
    }

    let received = stream_patch_body(
        &state,
        &id,
        body,
        upload_offset,
        total_length,
        content_length,
        &filename,
    )
    .await?;
    let new_offset = checked_upload_offset(upload_offset, received, total_length)?;

    advance_and_maybe_complete(
        &state,
        &id,
        new_offset,
        &headers,
        session_id.as_deref(),
        part_index,
        reserve_override,
        reservation_token_override,
    )
    .await
}

fn validate_first_chunk(
    state: &Arc<AppState>,
    filename: &str,
    bytes: &[u8],
) -> Result<(), AppError> {
    // Missing config fails closed at the strictest level.
    let level = state
        .juicehost_config()
        .map_or(crate::file_validation::ProtectionLevel::High, |jh| {
            jh.danger_level
        });
    crate::file_validation::blocked_file_error(crate::file_validation::validate_file(
        filename, bytes, level,
    ))
    .map_or(Ok(()), Err)
}

/// Records `upload.offset` after each accepted frame, so a reconnect resumes at
/// the last frame handed to the push task.
async fn stream_patch_body(
    state: &Arc<AppState>,
    id: &str,
    body: Body,
    upload_offset: u64,
    total_length: u64,
    content_length: Option<u64>,
    filename: &str,
) -> Result<u64, AppError> {
    let mut stream = body.into_data_stream();
    let mut received: u64 = 0;
    let mut pending: Option<Bytes> = None;

    if upload_offset == 0 {
        // Peek one frame so a blocked file is rejected before the push task starts.
        while let Some(frame) = stream.next().await {
            let chunk =
                frame.map_err(|_| AppError::BadRequest("patch body stream error".into()))?;
            if chunk.is_empty() {
                continue;
            }
            if let Err(err) = validate_first_chunk(state, filename, &chunk) {
                state.tus.remove(id);
                state.tus_senders.remove(id);
                return Err(err);
            }
            pending = Some(chunk);
            break;
        }
    }

    {
        let mut upload = state.tus.get_mut(id).ok_or(AppError::TusSessionNotFound)?;
        if let Some(rx) = upload.push_rx.take() {
            drop(upload);
            spawn_push_task(state, id, rx);
        }
    }
    let sender = state
        .tus_senders
        .get(id)
        .ok_or(AppError::TusSessionNotFound)?
        .clone();

    loop {
        let chunk = match pending.take() {
            Some(chunk) => chunk,
            None => match stream.next().await {
                Some(Ok(chunk)) => chunk,
                Some(Err(_)) => {
                    tracing::info!(
                        upload_id = %id,
                        received,
                        "tus patch: body stream ended early; session kept for resume"
                    );
                    return Err(AppError::BadRequest("patch body stream error".into()));
                }
                None => break,
            },
        };
        if chunk.is_empty() {
            continue;
        }

        received = received.saturating_add(chunk.len() as u64);
        let new_offset = upload_offset
            .checked_add(received)
            .ok_or(AppError::PayloadTooLarge)?;
        if new_offset > total_length || content_length.is_some_and(|declared| received > declared) {
            drop(sender);
            abort_session(state, id).await;
            return Err(AppError::PayloadTooLarge);
        }

        if sender.send(Ok(chunk)).await.is_err() {
            drop(sender);
            abort_session(state, id).await;
            return Err(AppError::TusSessionNotFound);
        }
        if let Some(mut upload) = state.tus.get_mut(id) {
            // Recorded per frame so an aborted stream keeps a resumable offset.
            upload.offset = new_offset;
            upload.last_activity = chrono::Utc::now().timestamp();
        }
    }

    if let Some(declared) = content_length
        && received < declared
    {
        tracing::info!(upload_id = %id, received, declared, "tus patch: body ended short; session kept for resume");
        return Err(AppError::BadRequest(format!(
            "patch body ended at {received} of {declared} bytes"
        )));
    }

    drop(sender);
    Ok(received)
}

async fn abort_session(state: &Arc<AppState>, id: &str) {
    state.tus_senders.remove(id);
    // Let the push task settle before releasing the session.
    let _ = await_storage_push(state, id).await;
    state.tus.remove(id);
}

/// A failed completion keeps the session so the client can retry.
#[expect(clippy::too_many_arguments, reason = "handler context threading")]
async fn advance_and_maybe_complete(
    state: &Arc<AppState>,
    id: &str,
    new_offset: u64,
    headers: &HeaderMap,
    session_id: Option<&str>,
    part_index: Option<usize>,
    reserve_override: Option<String>,
    reservation_token_override: Option<String>,
) -> Result<Response, AppError> {
    let (is_complete, completed_meta) = {
        let mut upload = state.tus.get_mut(id).ok_or(AppError::TusSessionNotFound)?;
        upload.offset = new_offset;
        upload.last_activity = chrono::Utc::now().timestamp();
        let done = upload.offset >= upload.total_length;
        if done {
            let meta = TusUploadMeta {
                id: upload.id.clone(),
                filename: upload.filename.clone(),
                mime_type: upload.mime_type.clone(),
                total_length: upload.total_length,
                delete_token: upload.delete_token.clone(),
                encrypted_ip: upload.encrypted_ip.clone(),
                ttl_hours: upload.ttl_hours,
                storage_host: upload.storage_host.clone(),
                reserve_id: reserve_override
                    .clone()
                    .or_else(|| upload.reserve_id.clone()),
                reservation_token: reservation_token_override
                    .clone()
                    .or_else(|| upload.reservation_token.clone()),
                user_id: upload.user_id.clone(),
                password_hash: upload.password_hash.clone(),
                dek: upload.dek.clone(),
                dek_wrapped: upload.dek_wrapped.clone(),
                dek_salt: upload.dek_salt.clone(),
                dek_escrow: upload.dek_escrow.clone(),
            };
            (true, Some(meta))
        } else {
            (false, None)
        }
    };

    if !is_complete {
        return Ok(offset_response(new_offset));
    }
    let Some(meta) = completed_meta else {
        return Ok(offset_response(new_offset));
    };

    let cookie_hdr = crate::routes::noscript::file_cookie_header(
        headers,
        &crate::routes::noscript::FileCookieEntry {
            id: meta.id.clone(),
            token: meta.delete_token.clone(),
        },
    );
    let response = if let (Some(sid), Some(pi)) = (session_id, part_index) {
        complete_parallel_part(state, meta, sid, pi).await
    } else {
        complete_tus_upload(state, meta).await
    };

    match response {
        Ok(data) => {
            state.tus.remove(id);
            state.tus_senders.remove(id);
            // A body without a url is only a part-complete marker.
            let status = if data.get("url").is_some() {
                StatusCode::OK
            } else {
                StatusCode::ACCEPTED
            };
            let mut resp = (status, Json(data)).into_response();
            if status == StatusCode::OK {
                // Only a live file gets the download cookie.
                resp.headers_mut()
                    .insert(axum::http::header::SET_COOKIE, cookie_hdr);
            }
            Ok(resp)
        }
        Err(err) => {
            tracing::warn!(
                id = %id,
                error = ?err,
                "tus completion failed; session kept for retry"
            );
            Err(err)
        }
    }
}

fn offset_response(new_offset: u64) -> Response {
    Response::builder()
        .status(StatusCode::NO_CONTENT)
        .header("Upload-Offset", new_offset.to_string())
        .header("Tus-Resumable", "1.0.0")
        .body(axum::body::Body::empty())
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}
