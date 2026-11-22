//! HTTP routes: health, the peer WebSocket endpoint and public file fetches.

use std::{io::ErrorKind, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, State, WebSocketUpgrade},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use juiceproto::peer::PeerServerMessage;
use serde::Deserialize;
use tokio::sync::{mpsc, oneshot};

use crate::relay::{
    RelayState,
    session::{ChunkItem, StreamGuard, serve_peer_socket},
};

const CHUNK_QUEUE: usize = 32;
const REQUEST_HEADERS: &[&str] = &["range", "if-none-match", "accept", "user-agent"];
const RESPONSE_HEADERS: &[&str] = &[
    // `content-length`/`content-range` are deliberately omitted: the peer
    // controls them, and `Body` computes the real length for the browser.
    "content-type",
    "accept-ranges",
    "etag",
    "cache-control",
    "content-disposition",
    "content-security-policy",
    "x-content-type-options",
];

/// All responses are non-cacheable so revoked peer files disappear at once.
pub fn build(state: Arc<RelayState>) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/peer/ws", get(peer_ws))
        .route("/{route}/{file}", get(fetch))
        .with_state(state)
}

async fn health(State(state): State<Arc<RelayState>>) -> Response {
    let (sessions, files, requests) = state.registry.stats();
    let payload = serde_json::json!({
        "status": "ok",
        "base_url": state.config.relay_base_url,
        "sessions": sessions,
        "files": files,
        "active_requests": requests,
    });
    (
        StatusCode::OK,
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
        Json(payload),
    )
        .into_response()
}

async fn peer_ws(ws: WebSocketUpgrade, State(state): State<Arc<RelayState>>) -> Response {
    ws.max_frame_size(state.config.max_frame_bytes)
        .max_message_size(state.config.max_frame_bytes)
        .on_upgrade(move |socket| serve_peer_socket(socket, state))
}

#[derive(Debug, Deserialize)]
struct FetchPath {
    route: String,
    file: String,
}

struct FetchTarget {
    route: &'static str,
    id: String,
    filename: String,
}

fn parse_target(path: &FetchPath) -> Result<FetchTarget, FetchError> {
    let route: &'static str = match path.route.as_str() {
        "v" => "v",
        "f" => "f",
        "d" => "d",
        "e" => "e",
        _ => return Err(FetchError::BadRequest),
    };
    let (id, extension) = path.file.rsplit_once('.').ok_or(FetchError::BadRequest)?;
    if !crate::relay::session::valid_file_id(id)
        || extension.is_empty()
        || extension.len() > 16
        || !extension
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(FetchError::BadRequest);
    }
    Ok(FetchTarget {
        route,
        id: id.to_owned(),
        filename: path.file.clone(),
    })
}

fn forwarded_request_headers(headers: &HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .filter(|(name, _)| REQUEST_HEADERS.contains(&name.as_str()))
        .filter_map(|(name, value)| {
            Some((name.as_str().to_owned(), value.to_str().ok()?.to_owned()))
        })
        .collect()
}

fn apply_response_headers(
    mut builder: axum::http::response::Builder,
    headers: &[(String, String)],
) -> axum::http::response::Builder {
    for (name, value) in headers {
        if !RESPONSE_HEADERS.contains(&name.as_str()) || name.eq_ignore_ascii_case("cache-control")
        {
            continue;
        }
        let Ok(name) = HeaderName::from_bytes(name.as_bytes()) else {
            continue;
        };
        let Ok(value) = HeaderValue::from_str(value) else {
            continue;
        };
        builder = builder.header(name, value);
    }
    builder
}

async fn fetch(
    State(state): State<Arc<RelayState>>,
    Path(path): Path<FetchPath>,
    headers: HeaderMap,
) -> Result<Response, FetchError> {
    let target = parse_target(&path)?;
    let session = state
        .registry
        .lookup(&target.id)
        .ok_or(FetchError::NotFound)?;
    let config = &state.config;
    let (request_id, guard) = state
        .registry
        .begin_request(Arc::clone(&session), config.max_concurrent_requests)?;

    let (head_tx, head_rx) = oneshot::channel();
    let (chunk_tx, chunk_rx) = mpsc::channel(CHUNK_QUEUE);
    if !session.insert_pending(request_id, head_tx, chunk_tx) {
        return Err(FetchError::Unavailable);
    }
    let request = PeerServerMessage::Request {
        request_id,
        route: target.route.to_owned(),
        id: target.id.clone(),
        filename: target.filename,
        headers: forwarded_request_headers(&headers),
    };
    session
        .send(request, Duration::from_secs(5))
        .await
        .map_err(|_| FetchError::Unavailable)?;

    let head = tokio::time::timeout(config.request_timeout(), head_rx)
        .await
        .map_err(|_| FetchError::GatewayTimeout)?
        .map_err(|_| FetchError::Unavailable)?
        .map_err(|reason| {
            tracing::debug!("peer request failed: {reason}");
            if reason == "peer disconnected" {
                FetchError::Unavailable
            } else {
                FetchError::BadGateway
            }
        })?;
    let status = StatusCode::from_u16(head.status).map_err(|_| FetchError::BadGateway)?;

    let builder = apply_response_headers(Response::builder().status(status), &head.headers);
    let builder = builder.header(header::CACHE_CONTROL, "no-store");
    let body = chunk_body(chunk_rx, guard, config.request_timeout());
    builder.body(body).map_err(|_| FetchError::Internal)
}

fn chunk_body(receiver: mpsc::Receiver<ChunkItem>, guard: StreamGuard, timeout: Duration) -> Body {
    // One deadline for the whole stream: a peer that trickles one byte per
    // chunk must not pin a browser request slot forever.
    let deadline = tokio::time::Instant::now() + timeout;
    let stream = futures::stream::unfold(
        (receiver, guard, deadline, false),
        move |(mut receiver, mut guard, deadline, done)| async move {
            if done {
                return None;
            }
            match tokio::time::timeout_at(deadline, receiver.recv()).await {
                Ok(Some(ChunkItem::Data(bytes))) => {
                    Some((Ok(bytes), (receiver, guard, deadline, false)))
                }
                Ok(Some(ChunkItem::Fail(reason))) => {
                    guard.abandon();
                    let error = axum::Error::new(std::io::Error::other(reason));
                    Some((Err(error), (receiver, guard, deadline, true)))
                }
                Ok(None) => {
                    guard.complete();
                    None
                }
                Err(_) => {
                    guard.abandon();
                    let error = axum::Error::new(std::io::Error::new(
                        ErrorKind::TimedOut,
                        "peer chunk timed out",
                    ));
                    Some((Err(error), (receiver, guard, deadline, true)))
                }
            }
        },
    );
    Body::from_stream(stream)
}

#[derive(Debug)]
enum FetchError {
    BadRequest,
    NotFound,
    Busy,
    Unavailable,
    BadGateway,
    GatewayTimeout,
    Internal,
}

impl From<&'static str> for FetchError {
    fn from(code: &'static str) -> Self {
        match code {
            "busy" => Self::Busy,
            _ => Self::Unavailable,
        }
    }
}

impl IntoResponse for FetchError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::BadRequest => (StatusCode::BAD_REQUEST, "invalid peer file path"),
            Self::NotFound => (StatusCode::NOT_FOUND, "peer file not found"),
            Self::Busy => (StatusCode::SERVICE_UNAVAILABLE, "relay is busy"),
            Self::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "peer is offline"),
            Self::BadGateway => (StatusCode::BAD_GATEWAY, "invalid peer response"),
            Self::GatewayTimeout => (StatusCode::GATEWAY_TIMEOUT, "peer request timed out"),
            Self::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "internal error"),
        };
        (
            status,
            [
                (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
                (
                    header::CONTENT_SECURITY_POLICY,
                    HeaderValue::from_static("sandbox"),
                ),
            ],
            Json(serde_json::json!({ "error": message })),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(route: &str, file: &str) -> FetchPath {
        FetchPath {
            route: route.to_owned(),
            file: file.to_owned(),
        }
    }

    #[test]
    fn parses_each_public_route() {
        for route in ["v", "f", "d", "e"] {
            let target = parse_target(&path(route, "abc-123.mp4")).unwrap();
            assert_eq!(target.route, route);
            assert_eq!(target.id, "abc-123");
            assert_eq!(target.filename, "abc-123.mp4");
        }
    }

    #[test]
    fn rejects_unknown_routes_and_paths() {
        assert!(parse_target(&path("x", "abc.mp4")).is_err());
        assert!(parse_target(&path("v", "abc")).is_err());
        assert!(parse_target(&path("v", "abc.bad/ext")).is_err());
        assert!(parse_target(&path("v", "../secret.txt")).is_err());
    }

    #[test]
    fn only_allowlisted_request_headers_are_forwarded() {
        let mut headers = HeaderMap::new();
        headers.insert(header::RANGE, "bytes=0-7".parse().unwrap());
        headers.insert(header::IF_NONE_MATCH, "\"etag\"".parse().unwrap());
        headers.insert(header::HOST, "example".parse().unwrap());
        let forwarded = forwarded_request_headers(&headers);
        assert_eq!(forwarded.len(), 2);
        assert!(
            forwarded
                .iter()
                .all(|(name, _)| REQUEST_HEADERS.contains(&name.as_str()))
        );
    }

    #[test]
    fn response_headers_forward_and_force_no_store() {
        let builder = apply_response_headers(
            Response::builder(),
            &[
                ("content-type".to_owned(), "video/mp4".to_owned()),
                ("etag".to_owned(), "\"v1\"".to_owned()),
                ("cache-control".to_owned(), "public, max-age=999".to_owned()),
                ("content-length".to_owned(), "999999".to_owned()),
                ("content-range".to_owned(), "bytes 0-1/999999".to_owned()),
                ("set-cookie".to_owned(), "a=b".to_owned()),
            ],
        );
        let response = builder
            .header(header::CACHE_CONTROL, "no-store")
            .body(Body::empty())
            .unwrap();
        assert_eq!(response.headers()[header::CONTENT_TYPE], "video/mp4");
        assert_eq!(response.headers()[header::ETAG], "\"v1\"");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert!(!response.headers().contains_key("set-cookie"));
        assert!(!response.headers().contains_key(header::CONTENT_LENGTH));
        assert!(!response.headers().contains_key("content-range"));
    }
}
