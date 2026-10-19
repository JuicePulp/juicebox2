//! Tiny in-memory micro-cache for the public, cookie-free HTML pages.
//!
//! The layer sits *outside* the compression layer, so a cache entry stores the
//! already-compressed bytes and a hit costs one hash-map lookup plus a body
//! memcpy. A background task keeps the hottest pages (home page and friends)
//! warm for the exact `Accept-Encoding` variants browsers and Cloudflare send,
//! so even the very first visitor after a deploy gets a pre-rendered page.
//!
//! Only allowlisted, non-personalised pages are cached. Anything that depends on
//! per-IP state (`/banned`, `/files`) or carries a query string bypasses the
//! cache entirely.

use std::{sync::Arc, time::Duration, time::Instant};

use axum::{
    body::Body,
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::Response,
};
use bytes::Bytes;
use dashmap::DashMap;
use http_body_util::BodyExt;

use crate::{i18n, state::AppState};

/// How long an entry may be served. The refresher re-populates every
/// [`REFRESH_INTERVAL`], so steady-state hits never see an expired entry.
const TTL: Duration = Duration::from_secs(10);
/// Backstop against unbounded growth (12 pages x 4 encodings x hosts).
const MAX_ENTRIES: usize = 256;
/// How often the background task re-renders the hot pages.
const REFRESH_INTERVAL: Duration = Duration::from_secs(4);

const PUBLIC_HOST: &str = "box.juicey.dev";

#[derive(Debug)]
struct CachedPage {
    stored: Instant,
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
}
impl CachedPage {
    fn to_response(&self) -> Response<Body> {
        let mut response = Response::new(Body::from(self.body.clone()));
        *response.status_mut() = self.status;
        *response.headers_mut() = self.headers.clone();
        set_content_length(response.headers_mut(), self.body.len());
        response
    }
}

fn set_content_length(headers: &mut HeaderMap, len: usize) {
    headers.remove(header::CONTENT_LENGTH);
    if let Ok(value) = HeaderValue::from_str(&len.to_string()) {
        headers.insert(header::CONTENT_LENGTH, value);
    }
}

/// Paths whose body only depends on the language (and the global caches the
/// SSR code already keeps fresh). `/banned` is per-IP and `/files` is
/// per-cookie, so both are deliberately absent.
fn is_cacheable_path(path: &str) -> bool {
    let (_, stripped) = i18n::split_locale(path);
    matches!(
        stripped.as_str(),
        "/" | "/index.html"
            | "/faq"
            | "/faq/"
            | "/docs"
            | "/docs/"
            | "/terms"
            | "/terms/"
            | "/privacy"
            | "/privacy/"
            | "/report"
            | "/report/"
            | "/feedback"
            | "/feedback/"
            | "/download"
            | "/download/"
    )
}

fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let header = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in header.split(';') {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        if key.trim() == name {
            return Some(value.trim());
        }
    }
    None
}

/// Exact `Accept-Encoding` variant, so the cached body always matches what the
/// client asked for. Cloudflare sends a stable `gzip, br`, browsers a stable
/// `gzip, deflate, br, zstd`, so the variant count stays tiny.
fn encoding_variant(headers: &HeaderMap) -> String {
    match headers.get(header::ACCEPT_ENCODING) {
        Some(value) if !value.is_empty() => {
            value.to_str().unwrap_or_default().trim().to_ascii_lowercase()
        }
        _ => "identity".to_owned(),
    }
}

fn is_cacheable_request(req: &Request<Body>) -> bool {
    req.method() == Method::GET
        && req.uri().query().is_none()
        && is_cacheable_path(req.uri().path())
}

fn cache_key(req: &Request<Body>, banned: bool) -> Option<String> {
    let path = req.uri().path();
    let headers = req.headers();
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())?;
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("http");
    let selected_host = cookie_value(headers, "juicebox_host").unwrap_or("");
    let adtesting = cookie_value(headers, "ADTESTING").unwrap_or("");
    Some(format!(
        "{scheme}\x1f{host}\x1f{path}\x1f{}\x1f{selected_host}\x1f{adtesting}\x1f{banned}",
        encoding_variant(headers)
    ))
}

#[derive(Debug, Default)]
pub struct PageCache {
    entries: DashMap<String, Arc<CachedPage>>,
}

impl PageCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn get(&self, key: &str) -> Option<Arc<CachedPage>> {
        let fresh = {
            let entry = self.entries.get(key)?;
            (entry.stored.elapsed() < TTL).then(|| Arc::clone(&entry))
        };
        if fresh.is_none() {
            self.entries.remove(key);
        }
        fresh
    }

    fn insert(&self, key: String, page: Arc<CachedPage>) {
        if self.entries.len() >= MAX_ENTRIES {
            self.entries
                .retain(|_, entry| entry.stored.elapsed() < TTL);
            if self.entries.len() >= MAX_ENTRIES {
                self.entries.clear();
            }
        }
        self.entries.insert(key, page);
    }
}

pub async fn middleware(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
    next: Next,
) -> Response<Body> {
    if !is_cacheable_request(&req) {
        return next.run(req).await;
    }
    // The page embeds a per-IP ban flag, so it has to be part of the key.
    // This also warms the ban cache the renderer will read moments later.
    let Some(peer) = req
        .extensions()
        .get::<ConnectInfo<std::net::SocketAddr>>()
        .map(|info| info.0)
    else {
        return next.run(req).await;
    };
    let banned = crate::handlers::cached_ban(&state, req.headers(), peer.ip())
        .await
        .banned;
    // Only fully anonymous, non-banned visitors get an edge-cacheable header.
    // Everyone else keeps `private`, so a poisoned/edge-cached page can never
    // reach them (Cloudflare's rule also bypasses any request with cookies).
    let anonymous = !req.headers().contains_key(header::COOKIE) && !banned;
    let Some(key) = cache_key(&req, banned) else {
        return next.run(req).await;
    };
    if let Some(page) = state.page_cache.get(&key) {
        return page.to_response();
    }

    let response = next.run(req).await;
    let (parts, body) = response.into_parts();
    let status = parts.status;
    let mut headers = parts.headers;
    let cacheable = status == StatusCode::OK
        && !headers.contains_key(header::SET_COOKIE)
        && headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/html"))
        && !headers
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("no-store"));
    if cacheable && anonymous {
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=30, s-maxage=30"),
        );
    }

    let Ok(collected) = body.collect().await else {
        return Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .body(Body::empty())
            .unwrap_or_default();
    };
    let body = collected.to_bytes();
    headers.remove(header::CONTENT_LENGTH);
    let page = Arc::new(CachedPage {
        stored: Instant::now(),
        status,
        headers,
        body,
    });
    if cacheable {
        state.page_cache.insert(key, Arc::clone(&page));
    }
    page.to_response()
}

/// Keep the hot pages warm so the first request after a deploy is served from
/// memory. Requests go through the loopback listener with the public `Host`
/// header, so cache keys and canonical URLs match real traffic.
pub fn spawn_refresh(state: Arc<AppState>) {
    let port = state.config.ui_port;
    tokio::spawn(async move {
        let Ok(client) = reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .no_proxy()
            .build()
        else {
            return;
        };
        let base = format!("http://127.0.0.1:{port}");
        let paths = [
            "/",
            "/fr",
            "/ru",
            "/es",
            "/ja",
            "/faq",
            "/docs",
            "/terms",
            "/privacy",
            "/download",
            "/report",
            "/feedback",
        ];
        let encodings = [
            "gzip, br",
            "gzip, deflate, br, zstd",
            "gzip, deflate, br",
            "gzip, deflate",
        ];
        loop {
            for path in paths {
                for encoding in encodings {
                    let request = client
                        .get(format!("{base}{path}"))
                        .header(reqwest::header::HOST, PUBLIC_HOST)
                        .header("x-forwarded-proto", "https")
                        .header(reqwest::header::ACCEPT_ENCODING, encoding)
                        .header(reqwest::header::ACCEPT, "text/html");
                    let _ = request.send().await;
                }
            }
            tokio::time::sleep(REFRESH_INTERVAL).await;
        }
    });
}
