use std::{collections::HashMap, sync::Arc};

use axum::{
    body::Body,
    extract::{Extension, Path, Query, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::Response,
};
use bytes::Bytes;
use futures::StreamExt;

use super::{
    common::{
        ByteResponseHeaders, NO_STORE, RangeResult, apply_byte_response_headers, parse_range,
    },
    serve::raw_content_security_policy,
    shell::{GateDecision, ShellMode, gate_decision},
    stats::ViewerIp,
    zip_index::{self, IndexedEntry, Resolved, ZipIndex},
    zip_list,
};
use crate::{
    error::JuicehostError,
    state::AppState,
    storage::{self, FileMetadata, valid_component},
};

/// Cap on both compressed and decompressed entry size, applied to stored
/// (method 0) entries as well as deflated ones.
const MAX_ENTRY_BYTES: u64 = 32 * 1024 * 1024;

fn valid_entry_path(path: &str) -> bool {
    zip_index::valid_selector(path)
}

/// Inflates deflate data, aborting once the output exceeds the entry cap.
fn inflate_deflated(data: &[u8]) -> Option<Vec<u8>> {
    use std::io::Read;
    let decoder = flate2::read::DeflateDecoder::new(data);
    let mut out = Vec::new();
    decoder
        .take(MAX_ENTRY_BYTES + 1)
        .read_to_end(&mut out)
        .ok()?;
    if out.len() as u64 > MAX_ENTRY_BYTES {
        return None;
    }
    Some(out)
}

/// Serves `?path=` or `?index=` from the ZIP at `id` via the central directory.
#[tracing::instrument(skip_all)]
pub async fn zip_entry_file(
    State(state): State<Arc<AppState>>,
    Extension(viewer): Extension<ViewerIp>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response<Body>, JuicehostError> {
    if let Some(delay) = state.zip_rate_limiter.check(viewer.0.as_deref()) {
        return Ok(rate_limited(delay));
    }
    let path = params.get("path");
    let index = params.get("index");
    if path.is_some() == index.is_some() {
        return Err(JuicehostError::BadRequest);
    }
    let selector = path
        .map(String::as_str)
        .or_else(|| index.map(String::as_str))
        .unwrap_or("");
    if !valid_component(&id) || !valid_entry_path(selector) {
        return Err(JuicehostError::BadRequest);
    }

    let meta = state
        .storage
        .stat(&id)
        .await
        .map_err(|_| JuicehostError::NotFound)?;
    if !is_zip(&meta) {
        return Err(JuicehostError::NotFound);
    }
    match gate_decision(&state, &id, ShellMode::View, None, false).await {
        GateDecision::Respond(_) | GateDecision::Ciphertext => {
            return Err(JuicehostError::Forbidden);
        }
        GateDecision::HotServe { .. } | GateDecision::ServeLocal { .. } => {}
    }
    let index_data = zip_index::load_index(&state, &id, &meta)
        .await
        .ok_or(JuicehostError::NotFound)?;

    let resolved = if path.is_some() {
        match index_data
            .files
            .iter()
            .find(|entry| entry.entry.name == selector)
        {
            Some(entry) => Resolved::File(entry.clone()),
            None => zip_index::resolve(&index_data, selector),
        }
    } else {
        zip_index::resolve(&index_data, selector)
    };

    match resolved {
        Resolved::File(entry) => serve_entry(&state, &id, &meta, &entry, false, None, false).await,
        Resolved::Ambiguous(entries) => Ok(ambiguous_response(&entries, &index_data)),
        _ => Err(JuicehostError::NotFound),
    }
}

/// JSON manifest for `/api/zipindex/{id}`.
pub async fn zip_index_json(
    State(state): State<Arc<AppState>>,
    Extension(viewer): Extension<ViewerIp>,
    Path(id): Path<String>,
) -> Result<Response<Body>, JuicehostError> {
    if let Some(delay) = state.zip_rate_limiter.check(viewer.0.as_deref()) {
        return Ok(rate_limited(delay));
    }
    if !valid_component(&id) {
        return Err(JuicehostError::BadRequest);
    }
    let meta = state
        .storage
        .stat(&id)
        .await
        .map_err(|_| JuicehostError::NotFound)?;
    if !is_zip(&meta) {
        return Err(JuicehostError::NotFound);
    }
    match gate_decision(&state, &id, ShellMode::View, None, false).await {
        GateDecision::Respond(_) | GateDecision::Ciphertext => {
            return Err(JuicehostError::Forbidden);
        }
        GateDecision::HotServe { .. } | GateDecision::ServeLocal { .. } => {}
    }
    let index = zip_index::load_index(&state, &id, &meta)
        .await
        .ok_or(JuicehostError::NotFound)?;
    let entries: Vec<serde_json::Value> = index
        .files
        .iter()
        .map(|entry| {
            let (preview, raw, download, index_url) = zip_index::entry_urls(&index, entry);
            serde_json::json!({
                "index": entry.index,
                "path": entry.entry.name,
                "name": entry.basename,
                "is_dir": false,
                "size": entry.entry.size,
                "mime": storage::guess_mime(
                    entry.entry.name.rsplit_once('.').map_or("", |(_, ext)| ext)
                ),
                "preview_url": preview,
                "raw_url": raw,
                "download_url": download,
                "index_url": index_url,
            })
        })
        .collect();
    let body = serde_json::json!({
        "archive_id": id,
        "archive_name": index.archive_name,
        "entry_count": index.files.len(),
        "total_size": index.total_size(),
        "folders": index.folders,
        "entries": entries,
    });
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, NO_STORE)
        .body(Body::from(body.to_string()))
        .map_err(|_| JuicehostError::Internal)
}

/// Shared byte response for `/f/<archive>/z/<selector>` and `/d/...`.
pub(crate) async fn zip_entry_bytes(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    method: axum::http::Method,
    id: &str,
    selector: &str,
    force_download: bool,
    viewer_ip: Option<String>,
) -> Result<Response<Body>, JuicehostError> {
    if let Some(delay) = state.zip_rate_limiter.check(viewer_ip.as_deref()) {
        return Ok(rate_limited(delay));
    }
    if !valid_entry_path(selector) {
        return Err(JuicehostError::BadRequest);
    }
    let meta = state
        .storage
        .stat(id)
        .await
        .map_err(|_| JuicehostError::NotFound)?;
    if !is_zip(&meta) {
        return Err(JuicehostError::NotFound);
    }

    let mode = if force_download {
        ShellMode::Download
    } else {
        ShellMode::View
    };
    match gate_decision(state, id, mode, None, false).await {
        GateDecision::Respond(_) | GateDecision::Ciphertext => {
            return Err(JuicehostError::Forbidden);
        }
        GateDecision::HotServe { .. } | GateDecision::ServeLocal { .. } => {}
    }

    let index = zip_index::load_index(state, id, &meta)
        .await
        .ok_or(JuicehostError::NotFound)?;
    match zip_index::resolve(&index, selector) {
        Resolved::File(entry) => {
            serve_entry(
                state,
                id,
                &meta,
                &entry,
                force_download,
                Some(headers),
                method == Method::HEAD,
            )
            .await
        }
        Resolved::Ambiguous(entries) => Ok(ambiguous_response(&entries, &index)),
        _ => Err(JuicehostError::NotFound),
    }
}

fn is_zip(meta: &FileMetadata) -> bool {
    meta.extension.eq_ignore_ascii_case("zip")
}

/// Builds the full entry response, then drops the body for HEAD requests so the
/// headers stay identical to GET without streaming any entry bytes.
async fn serve_entry(
    state: &Arc<AppState>,
    id: &str,
    meta: &FileMetadata,
    entry: &IndexedEntry,
    force_download: bool,
    headers: Option<&HeaderMap>,
    is_head: bool,
) -> Result<Response<Body>, JuicehostError> {
    let response = serve_entry_inner(state, id, meta, entry, force_download, headers).await?;
    if is_head {
        let (parts, _) = response.into_parts();
        return Ok(Response::from_parts(parts, Body::empty()));
    }
    Ok(response)
}

async fn serve_entry_inner(
    state: &Arc<AppState>,
    id: &str,
    meta: &FileMetadata,
    entry: &IndexedEntry,
    force_download: bool,
    headers: Option<&HeaderMap>,
) -> Result<Response<Body>, JuicehostError> {
    if entry.entry.encrypted {
        return Ok(text_response(
            StatusCode::FORBIDDEN,
            "Encrypted ZIP entries are not supported",
        ));
    }
    if entry.entry.method != 0 && entry.entry.method != 8 {
        return Ok(text_response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Unsupported ZIP compression method",
        ));
    }
    let uncomp_size = entry.entry.size.ok_or(JuicehostError::NotFound)?;
    // A stored entry is served directly from a range read, so it needs the same
    // size cap as the deflate path before any bytes are streamed to the client.
    if uncomp_size > MAX_ENTRY_BYTES || entry.entry.comp_size > MAX_ENTRY_BYTES {
        return Err(JuicehostError::PayloadTooLarge);
    }
    let data_start = local_data_start(state, id, &entry.entry, meta.size)
        .await
        .ok_or(JuicehostError::NotFound)?;
    let mime = storage::guess_mime(
        &entry
            .entry
            .name
            .rsplit_once('.')
            .map_or(String::new(), |(_, ext)| ext.to_ascii_lowercase()),
    );
    let disposition = crate::handlers::serve::content_disposition_value(
        if force_download {
            "attachment"
        } else {
            "inline"
        },
        &entry.basename,
    );

    if entry.entry.method == 0 {
        if uncomp_size != entry.entry.comp_size {
            return Err(JuicehostError::NotFound);
        }
        return serve_stored(
            state,
            id,
            &entry.entry,
            data_start,
            &mime,
            &disposition,
            headers,
        )
        .await;
    }

    if entry.entry.comp_size == 0 {
        return Response::builder()
            .status(StatusCode::OK)
            .header(
                header::CONTENT_TYPE,
                HeaderValue::from_str(&mime).map_err(|_| JuicehostError::Internal)?,
            )
            .header(header::CONTENT_LENGTH, 0)
            .header(header::CONTENT_DISPOSITION, disposition)
            .header(header::CACHE_CONTROL, NO_STORE)
            .body(Body::empty())
            .map_err(|_| JuicehostError::Internal);
    }
    let permit = Arc::clone(&state.zip_decode_semaphore)
        .try_acquire_owned()
        .map_err(|_| JuicehostError::ServiceUnavailable)?;
    let data = zip_list::read_range_capped(
        &state.storage,
        id,
        data_start,
        data_start + entry.entry.comp_size - 1,
        MAX_ENTRY_BYTES,
    )
    .await
    .ok_or(JuicehostError::NotFound)?;
    let decoded = tokio::task::spawn_blocking(move || inflate_deflated(&data))
        .await
        .map_err(|_| JuicehostError::Internal)?;
    drop(permit);
    let bytes = decoded.ok_or(JuicehostError::NotFound)?;
    Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_str(&mime).map_err(|_| JuicehostError::Internal)?,
        )
        .header(header::CONTENT_LENGTH, bytes.len())
        .header(header::CONTENT_DISPOSITION, disposition)
        .header(header::CACHE_CONTROL, NO_STORE)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(
            header::CONTENT_SECURITY_POLICY,
            raw_content_security_policy(&mime),
        )
        .body(Body::from(bytes))
        .map_err(|_| JuicehostError::Internal)
}

async fn serve_stored(
    state: &Arc<AppState>,
    id: &str,
    entry: &zip_list::ZipEntry,
    data_start: u64,
    mime: &str,
    disposition: &str,
    headers: Option<&HeaderMap>,
) -> Result<Response<Body>, JuicehostError> {
    let total = entry.comp_size;
    if total == 0 {
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime)
            .header(header::CONTENT_LENGTH, 0)
            .header(header::CONTENT_DISPOSITION, disposition)
            .header(header::CACHE_CONTROL, NO_STORE)
            .body(Body::empty())
            .map_err(|_| JuicehostError::Internal);
    }
    let permit = Arc::clone(&state.download_semaphore)
        .try_acquire_owned()
        .map_err(|_| JuicehostError::ServiceUnavailable)?;
    if let Some(value) = headers.and_then(|headers| headers.get(header::RANGE))
        && let Ok(value) = value.to_str()
    {
        match parse_range(value, total, state.max_range_response_bytes) {
            RangeResult::Satisfiable(start, end) => {
                let stream = state
                    .storage
                    .get_range_stream(id, data_start + start, data_start + end)
                    .await
                    .map_err(JuicehostError::from)?;
                let stream = stream.map(move |item| {
                    let _ = &permit;
                    item
                });
                let content_len = end - start + 1;
                return Response::builder()
                    .status(StatusCode::PARTIAL_CONTENT)
                    .header(header::CONTENT_TYPE, mime)
                    .header(header::CONTENT_LENGTH, content_len)
                    .header(
                        header::CONTENT_RANGE,
                        format!("bytes {start}-{end}/{total}"),
                    )
                    .header(header::ACCEPT_RANGES, "bytes")
                    .header(header::CONTENT_DISPOSITION, disposition)
                    .header(header::CACHE_CONTROL, NO_STORE)
                    .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
                    .header(
                        header::CONTENT_SECURITY_POLICY,
                        raw_content_security_policy(mime),
                    )
                    .body(Body::from_stream(stream))
                    .map_err(|_| JuicehostError::Internal);
            }
            RangeResult::Unsatisfiable => {
                let mut builder = Response::builder().status(StatusCode::RANGE_NOT_SATISFIABLE);
                apply_byte_response_headers(
                    &mut builder,
                    ByteResponseHeaders {
                        csp_mime: Some(mime),
                        etag: None,
                        content_range: Some(format!("bytes */{total}")),
                        content_length: None,
                        accept_ranges: true,
                    },
                );
                return builder
                    .body(Body::empty())
                    .map_err(|_| JuicehostError::Internal);
            }
            RangeResult::Ignore => {}
        }
    }

    let stream = state
        .storage
        .get_range_stream(id, data_start, data_start + total - 1)
        .await
        .map_err(JuicehostError::from)?;
    let stream = stream.map(move |item| {
        let _ = &permit;
        item
    });
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_LENGTH, total)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_DISPOSITION, disposition)
        .header(header::CACHE_CONTROL, NO_STORE)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(
            header::CONTENT_SECURITY_POLICY,
            raw_content_security_policy(mime),
        )
        .body(Body::from_stream(stream))
        .map_err(|_| JuicehostError::Internal)
}

async fn local_data_start(
    state: &Arc<AppState>,
    id: &str,
    entry: &zip_list::ZipEntry,
    archive_size: u64,
) -> Option<u64> {
    let offset = u64::from(entry.header_offset);
    let header = zip_list::read_range_capped(&state.storage, id, offset, offset + 29, 30).await?;
    if header.len() < 30 || &header[0..4] != b"PK\x03\x04" {
        return None;
    }
    let name_len = u16::from_le_bytes([header[26], header[27]]) as u64;
    let extra_len = u16::from_le_bytes([header[28], header[29]]) as u64;
    let data_start = offset
        .checked_add(30)?
        .checked_add(name_len)?
        .checked_add(extra_len)?;
    if data_start.checked_add(entry.comp_size)? > archive_size {
        return None;
    }
    if name_len > 0 {
        let name = zip_list::read_range_capped(
            &state.storage,
            id,
            offset + 30,
            offset + 30 + name_len - 1,
            name_len,
        )
        .await?;
        if name.as_ref() != entry.name.as_bytes() {
            return None;
        }
    }
    Some(data_start)
}

fn ambiguous_response(entries: &[IndexedEntry], index: &ZipIndex) -> Response<Body> {
    let mut body = String::from("Multiple files match. Use one of these paths:\n\n");
    for entry in entries {
        let (preview, _, _, _) = zip_index::entry_urls(index, entry);
        body.push_str(&format!("{}  {}\n", entry.entry.name, preview));
    }
    text_response(StatusCode::CONFLICT, &body)
}

fn rate_limited(delay: u64) -> Response<Body> {
    Response::builder()
        .status(StatusCode::TOO_MANY_REQUESTS)
        .header(header::RETRY_AFTER, delay.to_string())
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(header::CACHE_CONTROL, NO_STORE)
        .body(Body::from("Too many ZIP requests; slow down.\n"))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

fn text_response(status: StatusCode, message: &str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(header::CACHE_CONTROL, NO_STORE)
        .body(Body::from(format!("{message}\n")))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

/// Reads the first bytes of a ZIP entry for text previews, respecting the
/// deflate cap. Stored entries use a direct range read.
pub(crate) async fn read_entry_head(
    state: &Arc<AppState>,
    id: &str,
    meta: &FileMetadata,
    entry: &IndexedEntry,
    max_bytes: u64,
) -> Option<(String, u64, String)> {
    let uncomp_size = entry.entry.size?;
    let data_start = local_data_start(state, id, &entry.entry, meta.size).await?;
    let take = uncomp_size.min(max_bytes);
    if take == 0 {
        return None;
    }
    let bytes = if entry.entry.method == 0 {
        zip_list::read_range_capped(
            &state.storage,
            id,
            data_start,
            data_start + take - 1,
            max_bytes,
        )
        .await?
    } else {
        if entry.entry.comp_size > MAX_ENTRY_BYTES {
            return None;
        }
        let data = zip_list::read_range_capped(
            &state.storage,
            id,
            data_start,
            data_start + entry.entry.comp_size - 1,
            MAX_ENTRY_BYTES,
        )
        .await?;
        Bytes::from(inflate_deflated(&data)?)
    };
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(take as usize)]).into_owned();
    Some((head, uncomp_size, "utf-8".to_string()))
}
