use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Multipart, Path, State},
    http::{HeaderMap, header},
    response::Json,
};
use bytes::Bytes;
use futures::StreamExt;

use super::common::optional_file_capability;
use crate::{
    error::{JuicehostError, StorageError},
    state::AppState,
    storage,
    storage::valid_component as is_valid_id,
};

#[utoipa::path(
    post,
    path = "/internal/file",
    request_body(content_type = "multipart/form-data", description = "Multipart form with id, filename, and file fields"),
    responses(
        (status = 200, description = "File stored successfully", body = serde_json::Value),
        (status = 400, description = "Missing or invalid file ID"),
        (status = 401, description = "Missing or invalid credentials"),
        (status = 409, description = "File with this ID already exists"),
        (status = 413, description = "File exceeds size limit"),
        (status = 507, description = "Insufficient storage space"),
    ),
    tag = "Internal",
)]
/// Multipart upload expecting `id`, `filename` and `file` fields. The body is
/// buffered in memory, so large files belong on the streaming route.
#[tracing::instrument(skip_all)]
pub async fn store_file(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, JuicehostError> {
    let _permit = Arc::clone(&state.upload_semaphore)
        .try_acquire_owned()
        .map_err(|_| JuicehostError::ServiceUnavailable)?;
    let mut file_id = String::new();
    let mut filename = String::new();
    let mut file_data: Option<Bytes> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| JuicehostError::BadRequest)?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "id" => {
                file_id = field.text().await.map_err(|_| JuicehostError::BadRequest)?;
            }
            "filename" => {
                filename = field.text().await.map_err(|_| JuicehostError::BadRequest)?;
            }
            "file" => {
                let data = field
                    .bytes()
                    .await
                    .map_err(|_| JuicehostError::BadRequest)?;
                if data.len() as u64 > state.max_file_size_bytes {
                    return Err(JuicehostError::PayloadTooLarge);
                }
                file_data = Some(data);
            }
            _ => {}
        }
    }

    let Some(data) = file_data else {
        return Err(JuicehostError::BadRequest);
    };

    if file_id.is_empty() || !is_valid_id(&file_id) || filename.is_empty() {
        return Err(JuicehostError::BadRequest);
    }

    validate_or_block(
        &filename,
        &data,
        state.danger_level,
        &format!("(id={file_id})"),
    )?;

    let capability = optional_file_capability(&headers);

    state
        .storage
        .put(
            &file_id,
            &content_filename(&filename, infer::get(&data).as_ref()),
            data,
            capability.as_deref(),
        )
        .await
        .map_err(JuicehostError::from)?;

    tracing::info!("stored file {file_id} ({filename})");

    Ok(Json(serde_json::json!({"status": "ok", "id": file_id})))
}

pub(crate) fn validate_or_block(
    filename: &str,
    bytes: &[u8],
    danger_level: juiceutils::file_validation::ProtectionLevel,
    context: &str,
) -> Result<(), JuicehostError> {
    use juiceutils::file_validation::{FileValidation, friendly_block_reason};
    match juiceutils::file_validation::validate_file(filename, bytes, danger_level) {
        FileValidation::Allowed => Ok(()),
        FileValidation::BlockedExtension { ext, tier } => {
            tracing::warn!(
                "blocked file upload: extension .{} ({:?} tier) {}",
                ext,
                tier,
                context
            );
            Err(JuicehostError::BlockedFileType(friendly_block_reason(tier)))
        }
        FileValidation::BlockedMagic { description, tier } => {
            tracing::warn!(
                "blocked file upload: {} ({:?} tier) {}",
                description,
                tier,
                context
            );
            Err(JuicehostError::BlockedFileType(friendly_block_reason(tier)))
        }
        FileValidation::Empty => Err(JuicehostError::BlockedFileType(
            "Empty files are not allowed".into(),
        )),
    }
}

pub(crate) async fn sniff_and_validate(
    body: axum::body::Body,
    filename: &str,
    danger_level: juiceutils::file_validation::ProtectionLevel,
) -> Result<(axum::body::Body, Option<infer::Type>), JuicehostError> {
    use futures::StreamExt;

    const SNIFF_BYTES: usize = 512;

    let mut stream = body.into_data_stream();
    let mut prefix: Vec<u8> = Vec::with_capacity(SNIFF_BYTES);
    let mut tail: Option<Bytes> = None;

    while prefix.len() < SNIFF_BYTES {
        match stream.next().await {
            Some(Ok(chunk)) => {
                if chunk.len() >= SNIFF_BYTES - prefix.len() {
                    let take = SNIFF_BYTES - prefix.len();
                    prefix.extend_from_slice(&chunk[..take]);
                    tail = Some(chunk.slice(take..));
                    break;
                }
                prefix.extend_from_slice(&chunk);
            }
            Some(Err(_)) => return Err(JuicehostError::BadRequest),
            None => break,
        }
    }

    validate_or_block(filename, &prefix, danger_level, "")?;

    let mut chunks: Vec<Result<Bytes, axum::Error>> = vec![Ok(Bytes::from(prefix.clone()))];
    if let Some(t) = tail {
        chunks.push(Ok(t));
    }
    let body = axum::body::Body::from_stream(futures::stream::iter(chunks).chain(stream));

    let detected = infer::get(&prefix);
    Ok((body, detected))
}

#[must_use]
pub(crate) fn content_filename(filename: &str, detected: Option<&infer::Type>) -> String {
    match detected {
        Some(typ) => {
            let stem = std::path::Path::new(filename)
                .file_stem()
                .and_then(|s| s.to_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("file");
            format!("{}.{}", stem, typ.extension())
        }
        None => filename.to_string(),
    }
}

#[utoipa::path(
    post,
    path = "/internal/file/stream/{id}/{filename}",
    params(
        ("id" = String, Path, description = "The file ID"),
        ("filename" = String, Path, description = "The original filename"),
    ),
    request_body(content_type = "application/octet-stream", description = "Raw file bytes"),
    responses(
        (status = 200, description = "File stored successfully", body = serde_json::Value),
        (status = 400, description = "Invalid file ID or blocked file type"),
        (status = 401, description = "Missing or invalid credentials"),
        (status = 409, description = "File with this ID already exists"),
        (status = 413, description = "File exceeds size limit"),
        (status = 507, description = "Insufficient storage space"),
    ),
    tag = "Internal",
)]
/// Raw-body upload streamed straight into storage, keeping memory flat.
/// `x-mime-type` overrides the guessed MIME and `x-juicebox-live-total` opts
/// the upload into progress tracking.
#[tracing::instrument(skip_all)]
pub async fn store_file_streaming(
    State(state): State<Arc<AppState>>,
    Path((id, filename)): Path<(String, String)>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<Json<serde_json::Value>, JuicehostError> {
    if !is_valid_id(&id) {
        return Err(JuicehostError::BadRequest);
    }

    let _permit = Arc::clone(&state.upload_semaphore)
        .try_acquire_owned()
        .map_err(|_| JuicehostError::ServiceUnavailable)?;
    let handler_start = std::time::Instant::now();

    let capability = optional_file_capability(&headers);

    let (body, detected) = sniff_and_validate(body, &filename, state.danger_level).await?;
    let max_size = state.max_file_size_bytes;
    let stream = sized_stream(body, max_size, None);
    let storage_filename = content_filename(&filename, detected.as_ref());
    let mime = headers
        .get(juiceproto::headers::MIME_TYPE)
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| storage::guess_mime(storage::extract_extension(&storage_filename)));
    let live_total = headers
        .get(juiceproto::headers::LIVE_TOTAL)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let is_part = state.live_uploads.is_part(&id);
    let should_track = is_part || live_total > 0;
    if should_track && !is_part {
        state
            .live_uploads
            .ensure_single(&id, &storage_filename, &mime, live_total);
    }
    let live_writer = should_track
        .then(|| state.live_uploads.track(&id))
        .flatten();
    let stream = stream.map(move |item| {
        if let Ok(ref bytes) = item
            && let Some(ref writer) = live_writer
        {
            writer.add(bytes.len());
        }
        item
    });

    let total = state
        .storage
        .put_stream(
            &id,
            &storage_filename,
            Box::pin(stream),
            capability.as_deref(),
        )
        .await
        .map_err(JuicehostError::from)?;
    state.live_uploads.mark_ready(&id);

    let total_time = handler_start.elapsed();
    let bytes_per_sec = if total_time.as_secs_f64() > 0.0 {
        total as f64 / total_time.as_secs_f64()
    } else {
        0.0
    };
    tracing::info!(
        "stored file (streaming): {id} ({filename}) bytes={total} {:.2} MB/s",
        bytes_per_sec / (1024.0 * 1024.0),
    );

    Ok(Json(serde_json::json!({"status": "ok", "id": id})))
}

#[utoipa::path(
    post,
    path = "/internal/file/upload/{id}",
    params(
        ("id" = String, Path, description = "The file ID from the ticket"),
    ),
    request_body(content_type = "application/octet-stream", description = "Raw file bytes"),
    responses(
        (status = 200, description = "File stored successfully", body = serde_json::Value),
        (status = 400, description = "Invalid file ID"),
        (status = 401, description = "Missing or invalid ticket JWT"),
        (status = 409, description = "File with this ID already exists"),
        (status = 413, description = "File exceeds size limit"),
        (status = 507, description = "Insufficient storage space"),
    ),
    tag = "Internal",
)]
/// Upload authorized by a signed ticket: the path id must match the ticket, a
/// declared Content-Length must equal the signed size, and the ticket supplies
/// the capability and filename. The body must arrive in full.
#[tracing::instrument(skip_all)]
pub async fn store_file_ticket(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<Json<serde_json::Value>, JuicehostError> {
    if !is_valid_id(&id) {
        return Err(JuicehostError::BadRequest);
    }

    let token = juiceutils::extract_bearer_token(&headers).ok_or(JuicehostError::Unauthorized)?;

    let ticket = juiceproto::ticket::verify(token, &state.ticket_jwt_secret).map_err(|e| {
        tracing::warn!("ticket JWT validation failed: {e}");
        JuicehostError::Unauthorized
    })?;

    if ticket.file_id != id {
        tracing::warn!(
            "ticket file_id mismatch: ticket={} path={}",
            ticket.file_id,
            id
        );
        return Err(JuicehostError::Forbidden);
    }
    if ticket.file_size > state.max_file_size_bytes {
        return Err(JuicehostError::PayloadTooLarge);
    }
    if let Some(content_length) = headers.get(header::CONTENT_LENGTH) {
        let declared = content_length
            .to_str()
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or(JuicehostError::BadRequest)?;
        if declared != ticket.file_size {
            return Err(JuicehostError::SizeMismatch);
        }
    }
    let real_filename = ticket.filename.clone();
    let _permit = Arc::clone(&state.upload_semaphore)
        .try_acquire_owned()
        .map_err(|_| JuicehostError::ServiceUnavailable)?;

    let handler_start = std::time::Instant::now();

    let capability = Some(ticket.file_capability.clone());

    let (body, detected) = sniff_and_validate(body, &real_filename, state.danger_level).await?;
    let stream = sized_stream(body, ticket.file_size, Some(ticket.file_size));
    let storage_filename = content_filename(&real_filename, detected.as_ref());
    let mime = storage::guess_mime(storage::extract_extension(&storage_filename));
    if !state.live_uploads.is_part(&id) {
        state
            .live_uploads
            .ensure_single(&id, &storage_filename, &mime, ticket.file_size);
    }
    let live_writer = state.live_uploads.track(&id);
    let stream = stream.map(move |item| {
        if let Ok(ref bytes) = item
            && let Some(ref writer) = live_writer
        {
            writer.add(bytes.len());
        }
        item
    });

    let total = state
        .storage
        .put_stream(
            &id,
            &storage_filename,
            Box::pin(stream),
            capability.as_deref(),
        )
        .await
        .map_err(JuicehostError::from)?;
    state.live_uploads.mark_ready(&id);

    let total_time = handler_start.elapsed();
    let bytes_per_sec = if total_time.as_secs_f64() > 0.0 {
        total as f64 / total_time.as_secs_f64()
    } else {
        0.0
    };
    tracing::info!(
        "stored file (ticket): {id} ({real_filename}) bytes={total} {:.2} MB/s device={sub}",
        bytes_per_sec / (1024.0 * 1024.0),
        sub = ticket.sub,
    );

    Ok(Json(serde_json::json!({"status": "ok", "id": id})))
}

/// Enforces the size cap as chunks flow; with `exact_size`, overflow and short
/// bodies both fail as `SizeMismatch` so a ticket upload cannot be silently
/// truncated.
#[must_use]
pub(crate) fn sized_stream(
    body: Body,
    max_size: u64,
    exact_size: Option<u64>,
) -> storage::ByteStream {
    let stream = body.into_data_stream();
    Box::pin(futures::stream::try_unfold(
        (stream, 0u64),
        move |(mut stream, total)| async move {
            match stream.next().await {
                Some(Ok(chunk)) => {
                    let next = total
                        .checked_add(chunk.len() as u64)
                        .ok_or(StorageError::PayloadTooLarge)?;
                    if next > max_size {
                        return Err(if exact_size.is_some() {
                            StorageError::SizeMismatch
                        } else {
                            StorageError::PayloadTooLarge
                        });
                    }
                    Ok(Some((chunk, (stream, next))))
                }
                Some(Err(error)) => Err(StorageError::BodyRead(format!(
                    "request body failed: {error}"
                ))),
                None if exact_size.is_some_and(|expected| expected != total) => {
                    Err(StorageError::SizeMismatch)
                }
                None => Ok(None),
            }
        },
    ))
}
