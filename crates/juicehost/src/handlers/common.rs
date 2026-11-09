use axum::{
    body::Body,
    http::{HeaderMap, StatusCode},
};
use futures::StreamExt;

use crate::{error::JuicehostError, state::AppState};

#[must_use]
pub(crate) fn optional_file_capability(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-juicehost-file-capability")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

pub(crate) fn required_file_capability(
    headers: &HeaderMap,
    api_key: &str,
) -> Result<Option<String>, JuicehostError> {
    let has_api_key = headers
        .get("x-juicehost-api-key")
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
        request.header("x-juicehost-api-key", &state.api_key)
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

pub(crate) async fn backend_file_status(state: &AppState, id: &str) -> BackendStatus {
    let Some(backend_url) = state.backend_url.as_ref() else {
        return BackendStatus::Unavailable;
    };
    let status_url = format!("{backend_url}/internal/file/{id}/status");
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
        Ok(body) => BackendStatus::Known(body),
        Err(_) => BackendStatus::Unavailable,
    }
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
