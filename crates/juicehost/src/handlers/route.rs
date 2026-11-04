use axum::{
    body::Body,
    http::{HeaderMap, StatusCode, header},
    response::Response,
};

use super::common::backend_request;
use crate::state::AppState;

/// What the backend knows about a file that is not stored on this node.
struct RemoteLocation {
    status: String,
    storage_host: Option<String>,
    r2_url: Option<String>,
    filename: Option<String>,
}

async fn backend_location(state: &AppState, id: &str) -> Option<RemoteLocation> {
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
    })
}

/// Use the extension juicebox recorded for the file, not the extension in the
/// shared URL. A link copied before an audio file was correctly re-labelled
/// can say `.mp4`; forwarding that extension would make the storage node
/// render a video preview around MP3 bytes. The `/e/` route is generated
/// media (`.mp4` visualiser, `.jpg` poster), so it keeps its own extension.
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

fn is_http_url(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|url| url.scheme() == "http" || url.scheme() == "https")
}

/// Redirect a request for a file this node does not have to the node that
/// does (or to R2 when it has been promoted).
///
/// `route` is `f`, `d`, `v`, or `e`; `path` is the part after the route
/// (usually `<id>` or `<id>.<ext>`). For raw/preview routes the extension is
/// replaced with the one juicebox recorded, so stale links keep working.
///
/// Returns `None` when the file is still uploading here, when the backend has
/// no record, when the storage host is this same node (no redirect loop), or
/// when the stored URL is not http(s).
pub(crate) async fn remote_redirect(
    state: &AppState,
    headers: &HeaderMap,
    id: &str,
    route: &str,
    path: &str,
) -> Option<Response<Body>> {
    let location = backend_location(state, id).await?;
    // The upload is still in flight on this node; the caller should render
    // the live progress page instead of bouncing the viewer.
    if location.status == "uploading" {
        return None;
    }

    let target_path = canonical_route_path(route, path, location.filename.as_deref());
    let target = if let Some(r2_url) = location.r2_url {
        if !is_http_url(&r2_url) {
            return None;
        }
        r2_url
    } else {
        let storage_host = location.storage_host?;
        if !is_http_url(&storage_host) {
            return None;
        }
        let request_host = headers
            .get(header::HOST)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        if !request_host.is_empty() {
            match (host_only(&storage_host), host_only(request_host)) {
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
        .header(header::CACHE_CONTROL, "no-store")
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
