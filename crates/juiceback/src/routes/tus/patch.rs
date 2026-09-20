use std::sync::Arc;

use axum::{
    Json,
    body::{Body, Bytes},
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use futures::StreamExt;

use super::{
    completion::complete_tus_upload,
    metadata::checked_upload_offset,
    parallel::complete_parallel_part,
    state::{await_storage_push, spawn_push_task},
    types::TusUploadMeta,
};
use crate::{db, error::AppError, state::AppState};

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
    let res = patch_upload_handler_impl(State(state), Path(id.clone()), headers, body).await;
    match &res {
        Ok(r) => {
            tracing::info!(
                id = %id,
                req_offset,
                resp_offset = r.headers().get("upload-offset").and_then(|v| v.to_str().ok().map(str::to_string)),
                status = r.status().as_u16(),
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
                status = code,
                "tus_patch err"
            );
        }
    }
    res
}

/// Stream a request body into memory with a hard cap, aborting with
/// `PayloadTooLarge` as soon as the cap is exceeded instead of buffering an
/// unbounded body. A mid-stream client disconnect surfaces as `BadRequest`;
/// the stored upload offset is never bumped, so the client can retry the
/// same offset.
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

/// Blocking gzip decode for `spawn_blocking`: decodes one self-contained gzip
/// member with a running output cap (zip-bomb guard). Pure function so unit
/// tests can cover it without a runtime.
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

    // Stream the PATCH body with a hard per-request cap instead of buffering
    // an unbounded `Bytes`: memory per PATCH stays under TUS_PATCH_MAX_BYTES
    // no matter how large the client claims the chunk is.
    let raw = collect_bounded_body(body, crate::constants::TUS_PATCH_MAX_BYTES).await?;

    // Chunks may arrive gzip-compressed (X-File-Encoding: gzip, one
    // self-contained gzip member per PATCH). Decode up front so first-chunk
    // magic validation sniffs real content and offsets stay logical.
    let body_bytes = if headers.get("x-file-encoding").and_then(|v| v.to_str().ok()) == Some("gzip")
        && !raw.is_empty()
    {
        let cap = crate::constants::TUS_PATCH_MAX_BYTES;
        tokio::task::spawn_blocking(move || decode_gzip_bounded(raw, cap))
            .await
            .map_err(|_| AppError::TaskPanicked("decompression panicked".into()))??
    } else {
        Bytes::from(raw)
    };
    let chunk_len = body_bytes.len() as u64;

    let reserve_override = headers
        .get("x-reserve-id")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty())
        .map(|value| {
            if crate::utils::is_valid_id(value) {
                Ok(value.to_string())
            } else {
                Err(AppError::BadRequest("invalid reserve_id".into()))
            }
        })
        .transpose()?;
    let reservation_token_override = headers
        .get("x-delete-token")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty())
        .map(ToString::to_string);
    if reserve_override.is_some() && reservation_token_override.is_none() {
        return Err(AppError::Forbidden(
            "reservation delete token required".into(),
        ));
    }
    if let Some(ref reserve_id) = reserve_override {
        let token = reservation_token_override.as_deref().unwrap_or_default();
        let lookup_id = reserve_id.clone();
        let reservation = state
            .db_call("get_tus_override_reservation", move |db| {
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
    }

    let patch_lock = state
        .tus
        .get(&id)
        .ok_or(AppError::TusSessionNotFound)?
        .patch_lock
        .clone();
    let _patch_guard = patch_lock.lock().await;

    let (session_id, part_index, filename, new_offset) = {
        let upload = state.tus.get(&id).ok_or(AppError::TusSessionNotFound)?;
        if upload_offset != upload.offset {
            return Err(AppError::TusOffsetMismatch);
        }
        let new_offset = checked_upload_offset(upload_offset, chunk_len, upload.total_length)?;
        (
            upload.session_id.clone(),
            upload.part_index,
            upload.filename.clone(),
            new_offset,
        )
    };

    if upload_offset == 0 && !body_bytes.is_empty() {
        let level = state
            .juicehost_config()
            .map(|jh| jh.danger_level)
            .unwrap_or(crate::file_validation::ProtectionLevel::High);
        if let Some(err) = crate::file_validation::blocked_file_error(
            crate::file_validation::validate_file(&filename, &body_bytes, level),
        ) {
            state.tus.remove(&id);
            return Err(err);
        }
    }

    // First chunk for this session: hand the receiver to a freshly spawned
    // storage push. Sessions that never reach this point hold no juicehost
    // connection and cannot starve its inactivity deadline.
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
        .ok_or(AppError::TusSessionNotFound)?;

    let send_start = std::time::Instant::now();
    if sender.send(Ok(body_bytes)).await.is_err() {
        drop(sender);
        state.tus_senders.remove(&id);
        await_storage_push(&state, &id).await?;
        return Err(AppError::TusSessionNotFound);
    }
    tracing::debug!(
        "tus patch: chunk_stream took {:?} ({} bytes)",
        send_start.elapsed(),
        chunk_len
    );
    drop(sender);

    let (is_complete, completed_meta) = {
        let mut upload = state.tus.get_mut(&id).ok_or(AppError::TusSessionNotFound)?;
        upload.offset = new_offset;
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
            };
            (true, Some(meta))
        } else {
            (false, None)
        }
    };
    // Release the per-upload lock before the slow completion (push drain +
    // concat + DB): offset validation, send, and bump above are done, and a
    // concurrent PATCH now fails fast on the removed session instead of
    // stalling behind the whole completion.
    drop(_patch_guard);

    if is_complete {
        if let Some(meta) = completed_meta {
            if let (Some(sid), Some(pi)) = (&session_id, part_index) {
                state.tus.remove(&id);
                let response = complete_parallel_part(&state, meta, sid, pi).await?;
                let status = if response.get("url").is_some() {
                    StatusCode::OK
                } else {
                    StatusCode::ACCEPTED
                };
                return Ok((status, Json(response)).into_response());
            } else {
                state.tus.remove(&id);
                let response = complete_tus_upload(&state, meta).await?;
                return Ok(Json(response).into_response());
            }
        }
    }

    Ok(Response::builder()
        .status(StatusCode::NO_CONTENT)
        .header("Upload-Offset", new_offset.to_string())
        .header("Tus-Resumable", "1.0.0")
        .body(axum::body::Body::empty())
        .map_err(|_| AppError::Internal("response build failed".into()))?)
}
