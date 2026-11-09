use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    body::Body,
    extract::{Extension, Path, Query, State},
    http::{HeaderMap, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use futures::StreamExt;

use super::{
    common::{
        NO_STORE, RangeResult, backend_file_status, backend_request, file_cache_control_value,
        parse_range, prevent_file_caching,
    },
    preview::is_bot,
    shell::{BackendFileMeta, GateDecision, ShellMode, gate_decision, remaining_ttl_secs},
    stats::{ViewerIp, report_file_hit},
};
use crate::{
    error::{JuicehostError, StorageError, frozen_html, not_found_html, teapot_html},
    state::AppState,
    storage,
    storage::valid_component as is_valid_id,
};

/// Builds the `Content-Disposition` filename, preferring the backend name and
/// stripping directory components and control characters to keep the header
/// well formed.
fn download_filename(id: &str, extension: &str, backend_meta: Option<&BackendFileMeta>) -> String {
    if let Some(name) = backend_meta
        .and_then(|meta| meta.filename.as_deref())
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        let name = name
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(name)
            .chars()
            .filter(|c| !c.is_control() && *c != '"' && *c != '\\')
            .take(255)
            .collect::<String>();
        let name = name.trim();
        if !name.is_empty() {
            return name.to_string();
        }
    }
    if extension.is_empty() {
        id.to_string()
    } else {
        format!("{id}.{extension}")
    }
}

fn encode_rfc5987(value: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'!'
            | b'#'
            | b'$'
            | b'&'
            | b'+'
            | b'-'
            | b'.'
            | b'^'
            | b'_'
            | b'`'
            | b'|'
            | b'~' => out.push(char::from(byte)),
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

fn content_disposition_value(disposition: &str, name: &str) -> String {
    if name.is_ascii() {
        return format!("{disposition}; filename=\"{name}\"");
    }
    let fallback = name
        .chars()
        .map(|c| if c.is_ascii() { c } else { '_' })
        .collect::<String>();
    format!(
        "{disposition}; filename=\"{fallback}\"; filename*=UTF-8''{}",
        encode_rfc5987(name)
    )
}

fn disposition_for(force_download: bool) -> &'static str {
    if force_download {
        "attachment"
    } else {
        "inline"
    }
}

struct LiveReadState {
    offset: u64,
    end: u64,
    idle: Instant,
    wait: bool,
    // Held for the lifetime of the live read to bound concurrent downloads.
    #[expect(dead_code, reason = "held for the lifetime of the live read")]
    permit: tokio::sync::OwnedSemaphorePermit,
}

/// Reads up to `max_len` bytes at `offset`; `Ok(None)` when the data is not yet
/// available.
async fn read_live_at(
    state: &AppState,
    id: &str,
    offset: u64,
    max_len: u64,
) -> Result<Option<Bytes>, String> {
    if max_len == 0 {
        return Ok(None);
    }

    if let Ok(meta) = state.storage.stat(id).await
        && offset < meta.size
    {
        let end = offset.saturating_add(max_len - 1).min(meta.size - 1);
        let mut stream = state
            .storage
            .get_range_stream(id, offset, end)
            .await
            .map_err(|e| e.to_string())?;
        return match stream.next().await {
            Some(Ok(bytes)) => Ok(Some(bytes)),
            Some(Err(e)) => Err(e.to_string()),
            None => Ok(None),
        };
    }

    let Some(source) = state.live_uploads.source_at(id, offset) else {
        return Ok(None);
    };
    let available = source.end.saturating_sub(offset);
    let len = available.min(max_len);
    if len == 0 {
        return Ok(None);
    }

    if source.ready {
        let local_start = offset.saturating_sub(source.base_offset);
        let local_end = local_start.saturating_add(len - 1);
        let Ok(mut stream) = state
            .storage
            .get_range_stream(&source.upload_id, local_start, local_end)
            .await
        else {
            return Ok(None);
        };
        return match stream.next().await {
            Some(Ok(bytes)) => Ok(Some(bytes)),
            Some(Err(e)) => Err(e.to_string()),
            None => Ok(None),
        };
    }

    let Some(path) = state.storage.live_path(&source.upload_id) else {
        return Ok(None);
    };
    let Ok(mut file) = tokio::fs::File::open(&path).await else {
        return Ok(None);
    };
    let Ok(meta) = file.metadata().await else {
        return Ok(None);
    };
    let local_start = offset.saturating_sub(source.base_offset);
    if local_start >= meta.len() {
        return Ok(None);
    }
    let read_len = len.min(meta.len() - local_start);
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    file.seek(std::io::SeekFrom::Start(local_start))
        .await
        .map_err(|e| format!("live seek failed: {e}"))?;
    let mut buf = vec![0u8; read_len as usize];
    let n = file
        .read(&mut buf)
        .await
        .map_err(|e| format!("live read failed: {e}"))?;
    if n == 0 {
        return Ok(None);
    }
    buf.truncate(n);
    Ok(Some(Bytes::from(buf)))
}

fn live_body_stream(
    state: Arc<AppState>,
    id: String,
    start: u64,
    end: u64,
    wait: bool,
    permit: tokio::sync::OwnedSemaphorePermit,
) -> Body {
    let stream = futures::stream::unfold(
        LiveReadState {
            offset: start,
            end,
            idle: Instant::now(),
            wait,
            permit,
        },
        move |mut read| {
            let state = Arc::clone(&state);
            let id = id.clone();
            async move {
                if read.offset > read.end {
                    return None;
                }
                loop {
                    let remaining = read.end.saturating_sub(read.offset).saturating_add(1);
                    match read_live_at(&state, &id, read.offset, remaining.min(256 * 1024)).await {
                        Ok(Some(bytes)) if !bytes.is_empty() => {
                            read.offset = read.offset.saturating_add(bytes.len() as u64);
                            read.idle = Instant::now();
                            return Some((Ok(bytes), read));
                        }
                        Ok(_) => {
                            let progress = state.live_uploads.progress(&id);
                            if progress.as_ref().is_some_and(|p| p.complete)
                                && progress
                                    .as_ref()
                                    .is_some_and(|p| p.available <= read.offset)
                            {
                                return None;
                            }
                            if !read.wait {
                                return None;
                            }
                            if read.idle.elapsed() > Duration::from_secs(120) {
                                return Some((
                                    Err(std::io::Error::new(
                                        std::io::ErrorKind::TimedOut,
                                        "upload stalled",
                                    )),
                                    read,
                                ));
                            }
                            state
                                .live_uploads
                                .wait_for_change(&id, Duration::from_millis(250))
                                .await;
                        }
                        Err(error) => {
                            return Some((Err(std::io::Error::other(error)), read));
                        }
                    }
                }
            }
        },
    );
    Body::from_stream(stream)
}

async fn serve_live_upload(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    method: &Method,
    id: &str,
    force_download: bool,
    viewer_ip: &Option<String>,
) -> Result<Option<Response<Body>>, JuicehostError> {
    let Some(snapshot) = state.live_uploads.snapshot(id) else {
        return Ok(None);
    };
    if snapshot.total_size == 0 {
        return Ok(None);
    }

    let extension = storage::extract_extension(&snapshot.filename).to_ascii_lowercase();
    let mime = if snapshot.mime.trim().is_empty() {
        storage::guess_mime(&extension)
    } else {
        snapshot.mime.clone()
    };
    let backend_meta = BackendFileMeta {
        filename: Some(snapshot.filename.clone()),
        title: None,
        author: None,
    };
    let download_name = download_filename(id, &extension, Some(&backend_meta));
    let with_headers = |mut builder: axum::http::response::Builder| {
        builder = builder.header(header::CACHE_CONTROL, NO_STORE);
        builder = builder.header(header::X_CONTENT_TYPE_OPTIONS, "nosniff");
        builder = builder.header(header::CONTENT_SECURITY_POLICY, "sandbox allow-downloads");
        builder = builder.header(juiceproto::headers::UPLOADING, "1");
        builder = builder.header(juiceproto::headers::UPLOADED, snapshot.uploaded.to_string());
        builder = builder.header(juiceproto::headers::TOTAL, snapshot.total_size.to_string());
        builder = builder.header(
            header::CONTENT_DISPOSITION,
            content_disposition_value(disposition_for(force_download), &download_name),
        );
        builder
    };

    let is_head = *method == Method::HEAD;
    let count_hit = |viewer_ip: &Option<String>| {
        if *method != Method::GET || is_bot(headers) {
            return;
        }
        if let Some(ip) = viewer_ip.as_deref() {
            let surface = if force_download { "download" } else { "view" };
            report_file_hit(state, id, surface, ip, surface);
        }
    };

    if let Some(range_header) = headers.get(header::RANGE)
        && let Ok(range_val) = range_header.to_str()
    {
        let available = snapshot.available.max(1);
        match parse_range(range_val, available, state.max_range_response_bytes) {
            RangeResult::Satisfiable(start, end) => {
                let end = end.min(snapshot.available.saturating_sub(1));
                if snapshot.available == 0 || end < start {
                    return Ok(Some(
                        with_headers(
                            Response::builder()
                                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                                .header(
                                    header::CONTENT_RANGE,
                                    format!("bytes */{}", snapshot.total_size),
                                )
                                .header(header::ACCEPT_RANGES, "bytes"),
                        )
                        .body(Body::empty())
                        .map_err(|_| JuicehostError::Internal)?,
                    ));
                }
                let content_len = end - start + 1;
                let body = if is_head {
                    Body::empty()
                } else {
                    let permit = Arc::clone(&state.download_semaphore)
                        .try_acquire_owned()
                        .map_err(|_| JuicehostError::ServiceUnavailable)?;
                    live_body_stream(Arc::clone(state), id.to_string(), start, end, false, permit)
                };
                count_hit(viewer_ip);
                return Ok(Some(
                    with_headers(
                        Response::builder()
                            .status(StatusCode::PARTIAL_CONTENT)
                            .header(header::CONTENT_TYPE, &mime)
                            .header(header::CONTENT_LENGTH, content_len)
                            .header(
                                header::CONTENT_RANGE,
                                format!("bytes {start}-{end}/{}", snapshot.total_size),
                            )
                            .header(header::ACCEPT_RANGES, "bytes"),
                    )
                    .body(body)
                    .map_err(|_| JuicehostError::Internal)?,
                ));
            }
            RangeResult::Unsatisfiable => {
                return Ok(Some(
                    with_headers(
                        Response::builder()
                            .status(StatusCode::RANGE_NOT_SATISFIABLE)
                            .header(
                                header::CONTENT_RANGE,
                                format!("bytes */{}", snapshot.total_size),
                            )
                            .header(header::ACCEPT_RANGES, "bytes"),
                    )
                    .body(Body::empty())
                    .map_err(|_| JuicehostError::Internal)?,
                ));
            }
            RangeResult::Ignore => {}
        }
    }

    let body = if is_head {
        Body::empty()
    } else {
        let permit = Arc::clone(&state.download_semaphore)
            .try_acquire_owned()
            .map_err(|_| JuicehostError::ServiceUnavailable)?;
        live_body_stream(
            Arc::clone(state),
            id.to_string(),
            0,
            snapshot.total_size.saturating_sub(1),
            true,
            permit,
        )
    };
    count_hit(viewer_ip);
    Ok(Some(
        with_headers(
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, &mime)
                .header(header::CONTENT_LENGTH, snapshot.total_size)
                .header(header::ACCEPT_RANGES, "bytes"),
        )
        .body(body)
        .map_err(|_| JuicehostError::Internal)?,
    ))
}

/// Serves from the R2 hot tier with ETag and range support; `None` when R2 is
/// unconfigured or fails, so the caller can fall back to local storage.
#[expect(clippy::too_many_arguments)]
async fn serve_r2_response(
    state: &AppState,
    headers: &HeaderMap,
    method: &Method,
    id: &str,
    key: &str,
    file_meta: &storage::FileMetadata,
    backend_meta: Option<&BackendFileMeta>,
    force_download: bool,
    viewer_ip: Option<String>,
) -> Option<Response<Body>> {
    let r2 = state.r2.as_ref()?.clone();
    let etag = file_meta.etag.clone();
    let total_size = file_meta.size;
    let mime_str = storage::guess_mime(&file_meta.extension);
    let download_name = download_filename(id, &file_meta.extension, backend_meta);

    let ttl_remaining = remaining_ttl_secs(state, id).await;
    let cache_control = file_cache_control_value(state, ttl_remaining);
    let cache_tag = if cache_control == NO_STORE {
        None
    } else {
        Some(format!("juicebox:{id}"))
    };
    let with_cache_headers = |mut builder: axum::http::response::Builder| {
        builder = builder.header(header::CACHE_CONTROL, &cache_control);
        builder = builder.header(header::X_CONTENT_TYPE_OPTIONS, "nosniff");
        builder = builder.header(header::CONTENT_SECURITY_POLICY, "sandbox allow-downloads");
        if let Some(tag) = &cache_tag {
            builder = builder.header(header::HeaderName::from_static("cache-tag"), tag);
        }
        builder
    };
    let with_disposition = |mut builder: axum::http::response::Builder| {
        builder = builder.header(
            header::CONTENT_DISPOSITION,
            content_disposition_value(disposition_for(force_download), &download_name),
        );
        builder
    };
    let count_hit = |viewer_ip: &Option<String>| {
        if *method != Method::GET || is_bot(headers) {
            return;
        }
        if let Some(ip) = viewer_ip.as_deref() {
            let surface = if force_download { "download" } else { "view" };
            report_file_hit(
                state,
                id,
                if force_download { "download" } else { "view" },
                ip,
                surface,
            );
        }
    };

    if let Some(if_none_match) = headers.get(header::IF_NONE_MATCH)
        && let Ok(val) = if_none_match.to_str()
        && val.trim_matches('"') == etag.trim_matches('"')
    {
        count_hit(&viewer_ip);
        return with_cache_headers(
            Response::builder()
                .status(StatusCode::NOT_MODIFIED)
                .header(header::ETAG, &etag),
        )
        .body(Body::empty())
        .ok();
    }

    let permit = Arc::clone(&state.download_semaphore)
        .try_acquire_owned()
        .ok()?;
    if let Some(range_header) = headers.get(header::RANGE)
        && let Ok(range_val) = range_header.to_str()
    {
        match parse_range(range_val, total_size, state.max_range_response_bytes) {
            RangeResult::Satisfiable(start, end) => {
                let range_stream = r2.get_range_stream(key, start, end).await.ok()?;
                let content_len = end - start + 1;
                let stream = range_stream.map(move |item| {
                    // Keep the permit alive until the response stream is dropped.
                    let _ = &permit;
                    item.map_err(|_| {
                        axum::Error::new(std::io::Error::other("r2 range stream failed"))
                    })
                });
                count_hit(&viewer_ip);
                return with_disposition(with_cache_headers(
                    Response::builder()
                        .status(StatusCode::PARTIAL_CONTENT)
                        .header(header::CONTENT_TYPE, &mime_str)
                        .header(header::CONTENT_LENGTH, content_len)
                        .header(
                            header::CONTENT_RANGE,
                            format!("bytes {start}-{end}/{total_size}"),
                        )
                        .header(header::ETAG, &etag)
                        .header(header::ACCEPT_RANGES, "bytes"),
                ))
                .body(Body::from_stream(stream))
                .ok();
            }
            RangeResult::Unsatisfiable => {
                return with_cache_headers(
                    Response::builder()
                        .status(StatusCode::RANGE_NOT_SATISFIABLE)
                        .header(header::CONTENT_RANGE, format!("bytes */{total_size}"))
                        .header(header::ACCEPT_RANGES, "bytes"),
                )
                .body(Body::empty())
                .ok();
            }
            RangeResult::Ignore => {}
        }
    }

    let stream = r2.get_stream(key).await.ok()?;
    let stream = stream.map(move |item| {
        // Keep the permit alive until the response stream is dropped.
        let _ = &permit;
        item.map_err(|_| axum::Error::new(std::io::Error::other("r2 stream failed")))
    });
    count_hit(&viewer_ip);
    with_disposition(with_cache_headers(
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, &mime_str)
            .header(header::CONTENT_LENGTH, total_size)
            .header(header::ETAG, &etag)
            .header(header::ACCEPT_RANGES, "bytes"),
    ))
    .body(Body::from_stream(stream))
    .ok()
}

#[tracing::instrument(skip_all)]
pub async fn serve_file_wildcard(
    State(state): State<Arc<AppState>>,
    Extension(viewer): Extension<ViewerIp>,
    method: Method,
    headers: HeaderMap,
    Path(path): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Response<Body>, JuicehostError> {
    let id = path.split('.').next().unwrap_or(&path).to_string();
    if params.get("inline").is_some_and(|value| value == "1") {
        return super::serve_stored_bytes_inline(&state, &headers, &id).await;
    }
    serve_file_inner(state, headers, method, id, false, viewer.0, &path).await
}

#[tracing::instrument(skip_all)]
pub async fn serve_file_download(
    State(state): State<Arc<AppState>>,
    Extension(viewer): Extension<ViewerIp>,
    method: Method,
    headers: HeaderMap,
    Path(path): Path<String>,
) -> Result<Response<Body>, JuicehostError> {
    let id = path.split('.').next().unwrap_or(&path).to_string();
    serve_file_inner(state, headers, method, id, true, viewer.0, &path).await
}

pub(crate) async fn serve_file_inner(
    state: Arc<AppState>,
    headers: HeaderMap,
    method: Method,
    id: String,
    force_download: bool,
    viewer_ip: Option<String>,
    request_path: &str,
) -> Result<Response<Body>, JuicehostError> {
    if !is_valid_id(&id) {
        return Err(JuicehostError::BadRequest);
    }

    let file_meta = match state.storage.stat(&id).await {
        Ok(meta) => meta,
        Err(StorageError::Frozen) => {
            return Ok(prevent_file_caching(frozen_html().into_response()));
        }
        Err(StorageError::NotFound) => {
            if let Some(response) =
                serve_live_upload(&state, &headers, &method, &id, force_download, &viewer_ip)
                    .await?
            {
                return Ok(response);
            }

            if let Some(response) = super::route::remote_redirect(
                &state,
                &headers,
                &id,
                if force_download { "d" } else { "f" },
                request_path,
            )
            .await
            {
                return Ok(response);
            }

            if let crate::handlers::common::BackendStatus::Known(body) =
                backend_file_status(&state, &id).await
                && body.get("status").and_then(|s| s.as_str()) == Some("uploading")
            {
                let filename = body
                    .get("filename")
                    .and_then(|s| s.as_str())
                    .unwrap_or("upload");
                return Ok(prevent_file_caching(
                    teapot_html(filename, "", "").into_response(),
                ));
            }

            if let Some(ref backend_url) = state.backend_url {
                let alias_url = format!("{backend_url}/internal/alias/{id}");
                if let Ok(resp) = backend_request(&state, alias_url).send().await
                    && resp.status().is_success()
                    && let Ok(body) = resp.json::<serde_json::Value>().await
                    && let Some(new_url) = body.get("url").and_then(|u| u.as_str())
                {
                    // Only absolute http(s) redirect targets; reject anything else.
                    let valid = new_url
                        .parse::<url::Url>()
                        .is_ok_and(|u| u.scheme() == "http" || u.scheme() == "https");
                    if !valid {
                        return Ok(prevent_file_caching(not_found_html().into_response()));
                    }
                    return Response::builder()
                        .status(StatusCode::MOVED_PERMANENTLY)
                        .header(header::LOCATION, new_url)
                        .header(header::CACHE_CONTROL, NO_STORE)
                        .body(Body::empty())
                        .map_err(|_| JuicehostError::Internal);
                }
            }

            return Ok(prevent_file_caching(not_found_html().into_response()));
        }

        Err(other) => return Err(JuicehostError::from(other)),
    };

    let mode = if force_download {
        ShellMode::Download
    } else {
        ShellMode::View
    };
    let backend_meta = match gate_decision(&state, &id, mode, None, is_bot(&headers)).await {
        GateDecision::Respond(resp) => return Ok(resp),
        GateDecision::Ciphertext => {
            return super::general::serve_stored_bytes(&state, &headers, &id).await;
        }
        GateDecision::HotServe { key, meta } => {
            if let Some(resp) = serve_r2_response(
                &state,
                &headers,
                &method,
                &id,
                &key,
                &file_meta,
                meta.as_ref(),
                force_download,
                viewer_ip.clone(),
            )
            .await
            {
                return Ok(resp);
            }
            meta
        }
        GateDecision::ServeLocal { meta } => meta,
    };

    let etag = &file_meta.etag;

    let ttl_remaining = remaining_ttl_secs(&state, &id).await;
    let cache_control = file_cache_control_value(&state, ttl_remaining);
    let cache_tag = if cache_control == NO_STORE {
        None
    } else {
        Some(format!("juicebox:{id}"))
    };
    let with_cache_headers = |mut builder: axum::http::response::Builder| {
        builder = builder.header(header::CACHE_CONTROL, &cache_control);

        builder = builder.header(header::X_CONTENT_TYPE_OPTIONS, "nosniff");
        builder = builder.header(header::CONTENT_SECURITY_POLICY, "sandbox allow-downloads");
        if let Some(tag) = &cache_tag {
            builder = builder.header(header::HeaderName::from_static("cache-tag"), tag);
        }
        builder
    };

    let count_hit = |viewer_ip: &Option<String>| {
        if method != Method::GET {
            return;
        }

        if is_bot(&headers) {
            return;
        }
        let Some(ip) = viewer_ip.as_deref() else {
            return;
        };
        report_file_hit(
            &state,
            &id,
            if force_download { "download" } else { "view" },
            ip,
            if force_download { "download" } else { "view" },
        );
    };

    if let Some(if_none_match) = headers.get(header::IF_NONE_MATCH)
        && let Ok(val) = if_none_match.to_str()
        && val.trim_matches('"') == etag.trim_matches('"')
    {
        count_hit(&viewer_ip);
        return with_cache_headers(
            Response::builder()
                .status(StatusCode::NOT_MODIFIED)
                .header(header::ETAG, etag),
        )
        .body(Body::empty())
        .map_err(|_| JuicehostError::Internal);
    }

    let mime_str = storage::guess_mime(&file_meta.extension);
    let total_size = file_meta.size;
    let download_name = download_filename(&id, &file_meta.extension, backend_meta.as_ref());
    let with_disposition = |mut builder: axum::http::response::Builder| {
        builder = builder.header(
            header::CONTENT_DISPOSITION,
            content_disposition_value(disposition_for(force_download), &download_name),
        );
        builder
    };

    let permit = Arc::clone(&state.download_semaphore)
        .try_acquire_owned()
        .map_err(|_| JuicehostError::ServiceUnavailable)?;
    if let Some(range_header) = headers.get(header::RANGE)
        && let Ok(range_val) = range_header.to_str()
    {
        match parse_range(range_val, total_size, state.max_range_response_bytes) {
            RangeResult::Satisfiable(start, end) => {
                let range_stream = state
                    .storage
                    .get_range_stream(&id, start, end)
                    .await
                    .map_err(JuicehostError::from)?;
                let content_len = end - start + 1;
                let stream = range_stream.map(move |item| {
                    let _ = &permit;
                    item
                });
                count_hit(&viewer_ip);
                return with_disposition(with_cache_headers(
                    Response::builder()
                        .status(StatusCode::PARTIAL_CONTENT)
                        .header(header::CONTENT_TYPE, &mime_str)
                        .header(header::CONTENT_LENGTH, content_len)
                        .header(
                            header::CONTENT_RANGE,
                            format!("bytes {start}-{end}/{total_size}"),
                        )
                        .header(header::ETAG, etag)
                        .header(header::ACCEPT_RANGES, "bytes"),
                ))
                .body(Body::from_stream(stream))
                .map_err(|_| JuicehostError::Internal);
            }
            RangeResult::Unsatisfiable => {
                return with_cache_headers(
                    Response::builder()
                        .status(StatusCode::RANGE_NOT_SATISFIABLE)
                        .header(header::CONTENT_RANGE, format!("bytes */{total_size}"))
                        .header(header::ACCEPT_RANGES, "bytes"),
                )
                .body(Body::empty())
                .map_err(|_| JuicehostError::Internal);
            }
            RangeResult::Ignore => {}
        }
    }

    let stream = state
        .storage
        .get_stream(&id)
        .await
        .map_err(JuicehostError::from)?;
    let stream = stream.map(move |item| {
        let _ = &permit;
        item
    });
    let body = Body::from_stream(stream);

    count_hit(&viewer_ip);
    with_disposition(with_cache_headers(
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, &mime_str)
            .header(header::CONTENT_LENGTH, total_size)
            .header(header::ETAG, etag)
            .header(header::ACCEPT_RANGES, "bytes"),
    ))
    .body(body)
    .map_err(|_| JuicehostError::Internal)
}
