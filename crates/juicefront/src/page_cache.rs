use std::{
    sync::Arc,
    time::{Duration, Instant},
};

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

const TTL: Duration = Duration::from_secs(10);
/// Cap before expired entries are swept, then the whole map is cleared.
const MAX_ENTRIES: usize = 256;
const REFRESH_INTERVAL: Duration = Duration::from_secs(4);

/// Host header the refresher sends so it fills the keys real traffic reads.
const PUBLIC_HOST: &str = "box.juicey.dev";

/// A rendered response body; `to_response` rebuilds `Content-Length`.
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

/// Public pages served for every locale; the cache and the prewarmer both read
/// this list.
const PUBLIC_PAGES: &[&str] = &[
    "/",
    "/faq",
    "/docs",
    "/terms",
    "/privacy",
    "/report",
    "/feedback",
    "/download",
];

fn is_cacheable_path(path: &str) -> bool {
    let (_, stripped) = i18n::split_locale(path);
    let stripped = stripped.trim_end_matches('/');
    stripped.is_empty()
        || stripped == "/index.html"
        || PUBLIC_PAGES
            .iter()
            .any(|page| *page != "/" && *page == stripped)
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

fn encoding_variant(headers: &HeaderMap) -> String {
    match headers.get(header::ACCEPT_ENCODING) {
        Some(value) if !value.is_empty() => value
            .to_str()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase(),
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
    // Unit separators cannot appear in header values, so fields cannot forge a key.
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
            self.entries.retain(|_, entry| entry.stored.elapsed() < TTL);
            if self.entries.len() >= MAX_ENTRIES {
                self.entries.clear();
            }
        }
        self.entries.insert(key, page);
    }
}

/// Only anonymous 200 HTML responses are cached; others are returned as-is.
pub async fn middleware(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
    next: Next,
) -> Response<Body> {
    if !is_cacheable_request(&req) {
        return next.run(req).await;
    }
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

/// Prewarm re-requests pages locally to cover the encodings browsers send.
pub fn spawn_refresh(state: Arc<AppState>) {
    let port = state.config.ui_port;
    tokio::spawn(async move {
        let Ok(client) = reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .no_proxy()
            .build()
        else {
            tracing::warn!("page cache refresh disabled: could not build HTTP client");
            return;
        };
        tracing::debug!("page cache refresh started");
        let base = format!("http://127.0.0.1:{port}");
        let mut paths = Vec::new();
        for locale in i18n::LOCALES {
            let prefix = i18n::locale_prefix(locale.code);
            paths.push(if prefix.is_empty() {
                "/".to_owned()
            } else {
                prefix.to_owned()
            });
        }
        paths.extend(
            PUBLIC_PAGES
                .iter()
                .filter(|page| **page != "/")
                .map(|page| (*page).to_owned()),
        );
        let encodings = [
            "gzip, br",
            "gzip, deflate, br, zstd",
            "gzip, deflate, br",
            "gzip, deflate",
        ];
        loop {
            let mut refreshed = 0usize;
            for path in &paths {
                for encoding in encodings {
                    let request = client
                        .get(format!("{base}{path}"))
                        .header(reqwest::header::HOST, PUBLIC_HOST)
                        .header("x-forwarded-proto", "https")
                        .header(reqwest::header::ACCEPT_ENCODING, encoding)
                        .header(reqwest::header::ACCEPT, "text/html");
                    if request
                        .send()
                        .await
                        .is_ok_and(|response| response.status().is_success())
                    {
                        refreshed += 1;
                    }
                }
            }
            tracing::debug!(
                pages = refreshed,
                total = paths.len() * encodings.len(),
                "page cache refreshed"
            );
            tokio::time::sleep(REFRESH_INTERVAL).await;
        }
    });
}
