use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use futures::StreamExt;

use super::common::backend_request;
use crate::{
    error::{JuicehostError, StorageError, not_found_html, teapot_html},
    state::AppState,
    storage,
    storage::valid_component as is_valid_id,
};

const FILE_CACHE_CONTROL: &str = "no-store";

#[must_use]
pub(crate) fn file_cache_control_value(
    state: &AppState,
    ttl_remaining_secs: Option<u64>,
) -> String {
    if !state.file_cache_enabled {
        return FILE_CACHE_CONTROL.to_string();
    }
    match ttl_remaining_secs {
        Some(secs) if secs > 0 => {
            let age = secs.min(state.file_cache_max_age_secs);
            format!("public, max-age={age}, s-maxage={age}")
        }
        _ => FILE_CACHE_CONTROL.to_string(),
    }
}

pub(crate) async fn remaining_ttl_secs(state: &AppState, id: &str) -> Option<u64> {
    if !state.file_cache_enabled {
        return None;
    }
    let backend_url = state.backend_url.as_ref()?;
    let status_url = format!("{backend_url}/internal/file/{id}/status");
    let resp = backend_request(state, status_url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body = resp.json::<serde_json::Value>().await.ok()?;
    let expires_at = body.get("expires_at")?.as_i64()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    Some((expires_at - now).max(0) as u64)
}

fn prevent_file_caching(mut response: Response<Body>) -> Response<Body> {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static(FILE_CACHE_CONTROL),
    );
    response
}

#[tracing::instrument(skip_all)]
pub async fn serve_file_wildcard(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(path): Path<String>,
) -> Result<Response<Body>, JuicehostError> {
    let id = path.split('.').next().unwrap_or(&path).to_string();
    serve_file_inner(state, headers, id).await
}

async fn serve_file_inner(
    state: Arc<AppState>,
    headers: HeaderMap,
    id: String,
) -> Result<Response<Body>, JuicehostError> {
    if !is_valid_id(&id) {
        return Err(JuicehostError::BadRequest);
    }

    let file_meta = match state.storage.stat(&id).await {
        Ok(meta) => meta,
        Err(StorageError::NotFound) => {
            if let Some(ref backend_url) = state.backend_url {
                let status_url = format!("{backend_url}/internal/file/{id}/status");
                match backend_request(&state, status_url).send().await {
                    Ok(resp) if resp.status().is_success() => {
                        if let Ok(body) = resp.json::<serde_json::Value>().await
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
                    }
                    _ => {}
                }
            }

            if let Some(ref backend_url) = state.backend_url {
                let alias_url = format!("{backend_url}/internal/alias/{id}");
                if let Ok(resp) = backend_request(&state, alias_url).send().await
                    && resp.status().is_success()
                    && let Ok(body) = resp.json::<serde_json::Value>().await
                    && let Some(new_url) = body.get("url").and_then(|u| u.as_str())
                {
                    let valid = new_url
                        .parse::<url::Url>()
                        .is_ok_and(|u| u.scheme() == "http" || u.scheme() == "https");
                    if !valid {
                        return Ok(prevent_file_caching(not_found_html().into_response()));
                    }
                    return Response::builder()
                        .status(StatusCode::MOVED_PERMANENTLY)
                        .header(header::LOCATION, new_url)
                        .header(header::CACHE_CONTROL, FILE_CACHE_CONTROL)
                        .body(Body::empty())
                        .map_err(|_| JuicehostError::Internal);
                }
            }

            return Ok(prevent_file_caching(not_found_html().into_response()));
        }

        Err(other) => return Err(JuicehostError::from(other)),
    };

    let etag = &file_meta.etag;

    let ttl_remaining = remaining_ttl_secs(&state, &id).await;
    let cache_control = file_cache_control_value(&state, ttl_remaining);
    let cache_tag = if cache_control == FILE_CACHE_CONTROL {
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

    if let Some(if_none_match) = headers.get(header::IF_NONE_MATCH)
        && let Ok(val) = if_none_match.to_str()
        && val.trim_matches('"') == etag.trim_matches('"')
    {
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
                return with_cache_headers(
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
                )
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

    with_cache_headers(
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, &mime_str)
            .header(header::CONTENT_LENGTH, total_size)
            .header(header::ETAG, etag)
            .header(header::ACCEPT_RANGES, "bytes"),
    )
    .body(body)
    .map_err(|_| JuicehostError::Internal)
}

#[derive(Debug, PartialEq)]
pub(crate) enum RangeResult {
    Satisfiable(u64, u64),

    Unsatisfiable,

    Ignore,
}

#[must_use]
pub(crate) fn parse_range(range_val: &str, total_size: u64, max_len: u64) -> RangeResult {
    let range_val = range_val.trim();
    let Some(range_val) = range_val.strip_prefix("bytes=") else {
        return RangeResult::Ignore;
    };
    let range_val = range_val.trim();
    if range_val.contains(',') {
        return RangeResult::Ignore;
    }
    if total_size == 0 {
        return RangeResult::Unsatisfiable;
    }

    if let Some((start_str, end_str)) = range_val.split_once('-') {
        let start_str = start_str.trim();
        let end_str = end_str.trim();

        if start_str.is_empty() {
            let Ok(suffix_len) = end_str.parse::<u64>() else {
                return RangeResult::Ignore;
            };
            if suffix_len == 0 {
                return RangeResult::Unsatisfiable;
            }
            let suffix_len = suffix_len.min(total_size).min(max_len);
            RangeResult::Satisfiable(total_size - suffix_len, total_size - 1)
        } else {
            let Ok(start) = start_str.parse::<u64>() else {
                return RangeResult::Ignore;
            };
            if start >= total_size {
                return RangeResult::Unsatisfiable;
            }
            let requested_end = if end_str.is_empty() {
                total_size - 1
            } else {
                let Ok(end) = end_str.parse::<u64>() else {
                    return RangeResult::Ignore;
                };
                if end < start {
                    return RangeResult::Ignore;
                }
                end.min(total_size - 1)
            };
            let end = requested_end.min(start.saturating_add(max_len - 1));
            RangeResult::Satisfiable(start, end)
        }
    } else {
        RangeResult::Ignore
    }
}
