use axum::{
    body::Body,
    http::{HeaderMap, StatusCode, header},
    response::Response,
};

use super::common::backend_request;
use crate::state::AppState;

/// Backend's view of where a file lives; `r2_url` wins over `storage_host`.
pub(crate) struct RemoteLocation {
    pub(crate) status: String,
    pub(crate) storage_host: Option<String>,
    pub(crate) r2_url: Option<String>,
    pub(crate) filename: Option<String>,
    pub(crate) mime_type: Option<String>,
    pub(crate) size_bytes: Option<u64>,
}

pub(crate) async fn backend_location(state: &AppState, id: &str) -> Option<RemoteLocation> {
    let backend_url = state.backend_url.as_ref()?;
    let url = format!("{backend_url}/internal/resolve/{id}");
    let response = backend_request(state, url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let body: serde_json::Value = response.json().await.ok()?;
    let status = body.get("status")?.as_str()?.to_owned();
    let string_field = |key: &str| {
        body.get(key)
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    };
    Some(RemoteLocation {
        status,
        storage_host: string_field("storage_host"),
        r2_url: string_field("r2_url"),
        filename: string_field("filename"),
        mime_type: string_field("mime_type"),
        size_bytes: body.get("size_bytes").and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_i64().and_then(|n| u64::try_from(n).ok()))
        }),
    })
}

/// Rewrites the URL extension to the recorded one; route `e` serves raw media
/// and is untouched.
fn canonical_route_path(route: &str, path: &str, filename: Option<&str>) -> String {
    if route == "e" {
        return path.to_string();
    }
    let Some(filename) = filename else {
        return path.to_string();
    };
    let Some(extension) = std::path::Path::new(filename)
        .extension()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return path.to_string();
    };
    let id = path
        .trim_start_matches('/')
        .split(['.', '/'])
        .next()
        .unwrap_or(path);
    if id.is_empty() {
        return path.to_string();
    }
    format!("{id}.{extension}")
}

fn host_only(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let with_scheme = if value.contains("://") {
        value.to_owned()
    } else {
        format!("https://{value}")
    };
    let parsed = url::Url::parse(&with_scheme).ok()?;
    Some(parsed.host_str()?.to_ascii_lowercase())
}

/// Gate on backend-supplied redirect targets so only `http`/`https` can be
/// emitted.
pub(crate) fn is_http_url(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|url| url.scheme() == "http" || url.scheme() == "https")
}

pub(crate) async fn remote_redirect(
    state: &AppState,
    headers: &HeaderMap,
    id: &str,
    route: &str,
    path: &str,
) -> Option<Response<Body>> {
    let location = backend_location(state, id).await?;
    remote_redirect_with(headers, route, path, &location)
}

/// Emits a 302 to the hot copy, or `None` while uploading, when the location is
/// missing/non-HTTP, or when it points back at this host (which would loop).
pub(crate) fn remote_redirect_with(
    headers: &HeaderMap,
    route: &str,
    path: &str,
    location: &RemoteLocation,
) -> Option<Response<Body>> {
    if location.status == "uploading" {
        return None;
    }

    let target_path = canonical_route_path(route, path, location.filename.as_deref());
    let target = if let Some(r2_url) = location.r2_url.as_deref() {
        if !is_http_url(r2_url) {
            return None;
        }
        r2_url.to_owned()
    } else {
        let storage_host = location.storage_host.as_deref()?;
        if !is_http_url(storage_host) {
            return None;
        }
        let request_host = headers
            .get(header::HOST)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        if !request_host.is_empty() {
            match (host_only(storage_host), host_only(request_host)) {
                (Some(storage), Some(current)) if storage == current => return None,
                _ => {}
            }
        }
        format!(
            "{}/{}/{}",
            storage_host.trim_end_matches('/'),
            route,
            target_path.trim_start_matches('/')
        )
    };

    Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, target)
        .header(header::CACHE_CONTROL, super::common::NO_STORE)
        .body(Body::empty())
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_only_normalizes_urls_and_bare_hosts() {
        assert_eq!(
            host_only("https://F-EU2.juicey.dev/"),
            Some("f-eu2.juicey.dev".to_string())
        );
        assert_eq!(
            host_only("f-eu2.juicey.dev:443"),
            Some("f-eu2.juicey.dev".to_string())
        );
        assert_eq!(host_only(""), None);
    }

    #[test]
    fn remote_paths_follow_the_recorded_extension() {
        assert_eq!(
            canonical_route_path("v", "abc12345.mp4", Some("Song.mp3")),
            "abc12345.mp3"
        );
        assert_eq!(
            canonical_route_path("f", "abc12345", Some("Song.mp3")),
            "abc12345.mp3"
        );
        assert_eq!(
            canonical_route_path("d", "abc12345.mp4", Some("movie.mp4")),
            "abc12345.mp4"
        );
        assert_eq!(
            canonical_route_path("e", "abc12345.mp4", Some("Song.mp3")),
            "abc12345.mp4"
        );
        assert_eq!(
            canonical_route_path("v", "abc12345.mp4", Some("no-extension")),
            "abc12345.mp4"
        );
    }

    #[test]
    fn rejects_non_http_locations() {
        assert!(!is_http_url("ftp://example.com/file"));
        assert!(!is_http_url("javascript:alert(1)"));
        assert!(!is_http_url("mailto:a@b"));
        assert!(is_http_url("https://f-eu2.juicey.dev"));
        assert!(is_http_url("http://127.0.0.1:6402"));
    }
}
