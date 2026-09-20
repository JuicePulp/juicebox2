use axum::{body::Body, http::HeaderMap};
use futures::StreamExt;

use crate::{error::JuicehostError, state::AppState};

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
