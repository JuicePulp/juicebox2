use std::{net::IpAddr, sync::Arc, time::Duration};

use axum::{
    body::Body,
    extract::{ConnectInfo, Request, State},
    http::{
        HeaderMap, HeaderName, HeaderValue, StatusCode,
        header::{COOKIE, LOCATION},
    },
    response::Response,
};
use futures::StreamExt;
use http_body_util::BodyStream;
use thiserror::Error;

use crate::state::AppState;

#[derive(Debug, Error)]
pub enum ProxyError {
    #[error("request to {url} failed: {source}")]
    Request {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("request body failed: {0}")]
    Body(#[source] axum::Error),
    #[error("upstream body failed: {0}")]
    UpstreamBody(#[source] reqwest::Error),
    #[error("failed to build proxy response: {0}")]
    Response(#[source] axum::http::Error),
}

const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailers",
    "transfer-encoding",
    "upgrade",
    "host",
    "content-length",
];

pub fn proxy_target(path: &str) -> Option<Upstream> {
    if path.starts_with("/api/")
        || path == "/upload"
        || path.starts_with("/upload/")
        || path.starts_with("/file/")
    {
        Some(Upstream::Juiceback)
    } else if path.starts_with("/f/") || path.starts_with("/v/") || path.starts_with("/d/") {
        Some(Upstream::Juicehost)
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Upstream {
    Juiceback,
    Juicehost,
}

pub fn clean_headers(input: &HeaderMap) -> Vec<(HeaderName, HeaderValue)> {
    let mut out = Vec::with_capacity(input.len());
    for (name, value) in input {
        let lower = name.as_str();
        if HOP_BY_HOP.contains(&lower) || lower == "set-cookie" {
            continue;
        }
        out.push((name.clone(), value.clone()));
    }
    for value in input.get_all("set-cookie") {
        if let Ok(name) = HeaderName::from_bytes(b"set-cookie") {
            out.push((name, value.clone()));
        }
    }
    out
}

pub fn clean_ws_headers(input: &HeaderMap) -> Vec<(HeaderName, HeaderValue)> {
    const WS_STRIPPED: &[&str] = &[
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailers",
        "transfer-encoding",
        "host",
        "content-length",
    ];
    input
        .iter()
        .filter(|(name, _)| !WS_STRIPPED.contains(&name.as_str()))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

pub fn outgoing_forwarded_for(
    incoming: Option<&str>,
    peer: Option<IpAddr>,
    is_peer_trusted: bool,
) -> Option<String> {
    let peer = peer?;
    if incoming.is_some() && is_peer_trusted {
        incoming.map(str::to_owned)
    } else {
        Some(peer.to_string())
    }
}

pub async fn proxy_handler(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    req: Request<Body>,
) -> Response<Body> {
    let started = std::time::Instant::now();
    let path = req.uri().path().to_owned();
    let method = req.method().clone();
    if proxy_target(&path).is_none() {
        let headers = req.headers().clone();
        return crate::handlers::not_found_page(state, headers, peer.ip(), path).await;
    }
    let upstream = proxy_target(&path).unwrap_or(Upstream::Juiceback);

    match proxy_request(&state, req, peer.ip(), upstream).await {
        Ok(response) => {
            tracing::debug!(
                %method,
                path = %path,
                status = %response.status(),
                elapsed_ms = started.elapsed().as_millis() as u64,
                "proxied request"
            );
            response
        }
        Err(err) => {
            tracing::warn!(%method, path = %path, error = %err, "proxy error: upstream unavailable");
            Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .body(Body::from("Proxy error: upstream unavailable"))
                .unwrap_or_default()
        }
    }
}

async fn proxy_request(
    state: &AppState,
    req: Request<Body>,
    peer: IpAddr,
    upstream: Upstream,
) -> Result<Response<Body>, ProxyError> {
    let base = match upstream {
        Upstream::Juiceback => &state.config.juiceback_url,
        Upstream::Juicehost => &state.config.juicehost_url,
    };
    let (method, uri, headers, body) = (
        req.method().clone(),
        req.uri().clone(),
        req.headers().clone(),
        req.into_body(),
    );

    let mut url = format!("{base}{}", uri.path());
    if let Some(query) = uri.query() {
        url.push('?');
        url.push_str(query);
    }

    let is_peer_trusted = juiceutils::proxy::is_trusted(peer, &state.config.trusted_proxies);
    let mut outgoing: HeaderMap = HeaderMap::new();
    for (name, value) in clean_headers(&headers) {
        outgoing.append(name, value);
    }
    if let Some(xff) = outgoing_forwarded_for(
        headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()),
        Some(peer),
        is_peer_trusted,
    ) && let Ok(value) = xff.parse()
    {
        outgoing.insert("x-forwarded-for", value);
    }
    if let Ok(value) = peer.to_string().parse() {
        outgoing.insert("x-real-ip", value);
    }
    if let Some(host) = uri.host()
        && let Ok(value) = host.parse()
    {
        outgoing.insert("x-forwarded-host", value);
    }
    if let Some(scheme) = uri.scheme_str()
        && let Ok(value) = scheme.parse()
    {
        outgoing.insert("x-forwarded-proto", value);
    }

    let stream = BodyStream::new(body).filter_map(|result| async move {
        match result {
            Ok(frame) => frame.into_data().ok().map(Ok::<_, ProxyError>),
            Err(err) => Some(Err(ProxyError::Body(err))),
        }
    });

    let upstream_response = state
        .http
        .request(method, &url)
        .headers(outgoing)
        .body(reqwest::Body::wrap_stream(stream))
        .send()
        .await
        .map_err(|source| ProxyError::Request {
            url: url.clone(),
            source,
        })?;

    let status = upstream_response.status();
    let cleaned = clean_headers(upstream_response.headers());
    let shutdown = std::sync::Arc::clone(&state.shutdown);
    let stop = async move { shutdown.notified().await };
    let body_stream = upstream_response
        .bytes_stream()
        .map(|result| result.map_err(ProxyError::UpstreamBody))
        .take_until(stop);

    let mut builder = Response::builder().status(status);
    if let Some(headers) = builder.headers_mut() {
        headers.reserve(cleaned.len());
        for (name, value) in cleaned {
            headers.append(name, value);
        }
    }
    builder
        .body(Body::from_stream(body_stream))
        .map_err(ProxyError::Response)
}

pub fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(16)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("failed to build reqwest client")
}

pub fn redirect_to(location: &str) -> Response<Body> {
    Response::builder()
        .status(StatusCode::FOUND)
        .header(LOCATION, location)
        .body(Body::empty())
        .unwrap_or_default()
}

pub fn inbound_cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get(COOKIE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_target_routes_match_old_middleware() {
        assert!(matches!(
            proxy_target("/api/health"),
            Some(Upstream::Juiceback)
        ));
        assert!(matches!(proxy_target("/upload"), Some(Upstream::Juiceback)));
        assert!(matches!(
            proxy_target("/upload/reserve"),
            Some(Upstream::Juiceback)
        ));
        assert!(matches!(
            proxy_target("/file/abc/info"),
            Some(Upstream::Juiceback)
        ));
        assert!(matches!(proxy_target("/f/xyz"), Some(Upstream::Juicehost)));
        assert!(matches!(proxy_target("/v/xyz"), Some(Upstream::Juicehost)));
        assert!(matches!(proxy_target("/d/xyz"), Some(Upstream::Juicehost)));
        assert!(proxy_target("/c/xyz").is_none());
        assert!(proxy_target("/").is_none());
        assert!(proxy_target("/admin").is_none());
        assert!(proxy_target("/files").is_none());
    }

    #[test]
    fn clean_headers_strips_hop_by_hop_but_keeps_cookies() {
        let mut headers = HeaderMap::new();
        headers.insert("connection", "keep-alive".parse().unwrap());
        headers.insert("content-length", "42".parse().unwrap());
        headers.insert("authorization", "Bearer x".parse().unwrap());
        headers.append("set-cookie", "a=1".parse().unwrap());
        headers.append("set-cookie", "b=2".parse().unwrap());

        let cleaned = clean_headers(&headers);
        let names: Vec<String> = cleaned.iter().map(|(n, _)| n.to_string()).collect();
        assert!(!names.contains(&"connection".to_owned()));
        assert!(!names.contains(&"content-length".to_owned()));
        assert!(names.contains(&"authorization".to_owned()));
        assert_eq!(cleaned.iter().filter(|(n, _)| n == "set-cookie").count(), 2);
    }

    #[test]
    fn clean_ws_headers_keeps_upgrade_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("connection", "Upgrade".parse().unwrap());
        headers.insert("upgrade", "websocket".parse().unwrap());
        headers.insert("sec-websocket-key", "abc".parse().unwrap());
        headers.insert("content-length", "0".parse().unwrap());

        let cleaned = clean_ws_headers(&headers);
        let names: Vec<String> = cleaned.iter().map(|(n, _)| n.to_string()).collect();
        assert!(names.contains(&"connection".to_owned()));
        assert!(names.contains(&"upgrade".to_owned()));
        assert!(names.contains(&"sec-websocket-key".to_owned()));
        assert!(!names.contains(&"content-length".to_owned()));
    }

    #[test]
    fn forwarded_for_trust_matches_old_middleware() {
        let peer: IpAddr = "10.0.0.1".parse().unwrap();
        assert_eq!(
            outgoing_forwarded_for(Some("198.51.100.7"), Some(peer), true),
            Some("198.51.100.7".to_owned())
        );
        assert_eq!(
            outgoing_forwarded_for(Some("198.51.100.7"), Some(peer), false),
            Some("10.0.0.1".to_owned())
        );
        assert_eq!(
            outgoing_forwarded_for(Some("198.51.100.7"), None, true),
            None
        );
    }
}
