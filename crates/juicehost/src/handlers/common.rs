use axum::{
    body::Body,
    http::{HeaderMap, StatusCode, header},
    response::Response,
};
use futures::StreamExt;

use crate::{error::JuicehostError, state::AppState};

pub(crate) const NO_STORE: &str = "no-store";

const FILE_STATUS_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(5);
const FILE_STATUS_CACHE_MAX_ENTRIES: usize = 8_192;

#[must_use]
pub(crate) fn optional_file_capability(headers: &HeaderMap) -> Option<String> {
    headers
        .get(juiceproto::headers::FILE_CAPABILITY)
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// `Ok(None)` for a matching API key (list-wide access); otherwise a capability
/// scopes the caller to one file. An empty configured key disables key auth,
/// leaving capabilities as the only way in.
pub(crate) fn required_file_capability(
    headers: &HeaderMap,
    api_key: &str,
) -> Result<Option<String>, JuicehostError> {
    let has_api_key = headers
        .get(juiceproto::headers::HOST_API_KEY)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|provided| juiceutils::constant_time_eq(api_key, provided));
    if !api_key.is_empty() && has_api_key {
        return Ok(None);
    }
    optional_file_capability(headers)
        .ok_or(JuicehostError::Unauthorized)
        .map(Some)
}

pub(crate) fn backend_request(state: &AppState, url: String) -> reqwest::RequestBuilder {
    let request = state.backend_client.get(url);
    if state.api_key.is_empty() {
        request
    } else {
        request.header(juiceproto::headers::HOST_API_KEY, &state.api_key)
    }
}

pub(crate) fn deadline_body(
    body: Body,
    inactivity: std::time::Duration,
    total: std::time::Duration,
) -> Body {
    let deadline = tokio::time::Instant::now() + total;
    let stream = futures::stream::unfold(Some(body.into_data_stream()), move |state| async move {
        let mut stream = state?;
        let wait = inactivity.min(deadline.saturating_duration_since(tokio::time::Instant::now()));
        if wait.is_zero() {
            return Some((
                Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "request body total deadline exceeded",
                )),
                None,
            ));
        }
        match tokio::time::timeout(wait, stream.next()).await {
            Ok(Some(Ok(chunk))) => Some((Ok(chunk), Some(stream))),
            Ok(Some(Err(error))) => Some((Err(std::io::Error::other(error)), None)),
            Ok(None) => None,
            Err(_) => Some((
                Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "request body inactivity deadline exceeded",
                )),
                None,
            )),
        }
    })
    .fuse();
    Body::from_stream(stream)
}

pub(crate) enum BackendStatus {
    Known(serde_json::Value),

    Unknown,

    Unavailable,
}

impl BackendStatus {
    pub(crate) fn into_known(self) -> Option<serde_json::Value> {
        match self {
            Self::Known(body) => Some(body),
            _ => None,
        }
    }
}

/// Successful status bodies are cached five seconds; the cache is pruned on
/// overflow, then cleared.
pub(crate) async fn backend_file_status(state: &AppState, id: &str) -> BackendStatus {
    if let Some(entry) = state.file_status_cache.get(id)
        && entry.0.elapsed() < FILE_STATUS_CACHE_TTL
    {
        return BackendStatus::Known(entry.1.clone());
    }
    let Some(backend_url) = state.backend_url.as_ref() else {
        return BackendStatus::Unavailable;
    };
    let status_url = juiceproto::config::file_status_url(backend_url, id);
    let Ok(resp) = backend_request(state, status_url).send().await else {
        return BackendStatus::Unavailable;
    };
    if resp.status() == StatusCode::NOT_FOUND {
        return BackendStatus::Unknown;
    }
    if !resp.status().is_success() {
        return BackendStatus::Unavailable;
    }
    match resp.json::<serde_json::Value>().await {
        Ok(body) => {
            if state.file_status_cache.len() >= FILE_STATUS_CACHE_MAX_ENTRIES {
                state
                    .file_status_cache
                    .retain(|_, (at, _)| at.elapsed() < FILE_STATUS_CACHE_TTL);
                if state.file_status_cache.len() >= FILE_STATUS_CACHE_MAX_ENTRIES {
                    state.file_status_cache.clear();
                }
            }
            state
                .file_status_cache
                .insert(id.to_owned(), (std::time::Instant::now(), body.clone()));
            BackendStatus::Known(body)
        }
        Err(_) => BackendStatus::Unavailable,
    }
}

pub(crate) fn file_cache_control_value(
    state: &AppState,
    ttl_remaining_secs: Option<u64>,
) -> String {
    if !state.file_cache_enabled {
        return NO_STORE.to_string();
    }
    match ttl_remaining_secs {
        Some(secs) if secs > 5 => {
            let age = secs.saturating_sub(5).min(state.file_cache_max_age_secs);
            format!("public, max-age={age}, s-maxage={age}, immutable")
        }
        _ => NO_STORE.to_string(),
    }
}

pub(crate) fn prevent_file_caching(mut response: Response<Body>) -> Response<Body> {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static(NO_STORE),
    );
    response
}

/// `Ignore` means send the full body; `Unsatisfiable` is valid but outside the
/// file and must map to 416.
#[derive(Debug, PartialEq)]
pub(crate) enum RangeResult {
    Satisfiable(u64, u64),

    Unsatisfiable,

    Ignore,
}

/// Parses a single `bytes=` range, clamping to `max_len` and the file size;
/// multi-range is ignored.
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
