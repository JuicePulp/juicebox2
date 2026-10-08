//! Fullscreen file previews (`/v/`).
//!
//! A self-contained immersive page in the Juicebox aesthetic: the media fills
//! the viewport with minimal chrome (filename, download, raw, fullscreen).
//! No card layouts - previews get the whole screen. Works without JS for
//! video/audio/image/PDF (native elements); text preview and the fullscreen
//! button progressively enhance.

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
};

use super::shell::{ShellMode, protected_shell_response};
use crate::{
    error::not_found_html,
    state::AppState,
    storage,
    storage::valid_component as is_valid_id,
};

const NO_STORE: &str = "no-store";
const TEXT_PREVIEW_MAX_BYTES: u64 = 262_144;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewKind {
    Video,
    Audio,
    Image,
    Pdf,
    Text,
    Download,
}

fn preview_kind(mime: &str) -> PreviewKind {
    let base = mime.split(';').next().unwrap_or("").trim();
    if base.starts_with("video/") {
        PreviewKind::Video
    } else if base.starts_with("audio/") {
        PreviewKind::Audio
    } else if base.starts_with("image/") {
        PreviewKind::Image
    } else if base == "application/pdf" {
        PreviewKind::Pdf
    } else if base.starts_with("text/")
        || base.ends_with("+json")
        || base.ends_with("+xml")
        || matches!(
            base,
            "application/json"
                | "application/javascript"
                | "application/x-javascript"
                | "application/xml"
        ) {
        PreviewKind::Text
    } else {
        PreviewKind::Download
    }
}

fn human_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    const TB: u64 = 1024 * GB;
    if bytes >= TB {
        format!("{:.1} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

const PREVIEW_TEMPLATE: &str = include_str!("../templates/preview.html");
const PREVIEW_APP_JS: &str = include_str!("../templates/preview_app.js");
const LOGO_DATA_URI: &str = include_str!("../templates/logo_b64.txt");
const TITLE_FONT_DATA_URI: &str = include_str!("../templates/title_b64.txt");

/// User-Agent fragments identifying non-interactive clients (curl-like
/// tools, social unfurlers, crawlers). Matched case-insensitively against
/// the whole header; real browsers never contain these tokens.
const BOT_UA_TOKENS: &[&str] = &[
    "curl", "wget", "httpie", "aria2", "axel", "python-urllib", "python-requests",
    "go-http-client", "okhttp", "libwww-perl", "scrapy", "node-fetch", "undici",
    "got", "axios", "httpclient", "perl", "ruby", "php", "powershell",
    "discordbot", "telegram", "twitterbot", "facebookexternalhit", "facebookcatalog",
    "slackbot", "linkedinbot", "whatsapp", "skypeuripreview", "mastodon", "misskey",
    "pleroma", "pixelfed", "matrix", "signal", "line", "kakao", "viber", "pinterest",
    "redditbot", "tumblr", "bitlybot", "embedly", "iframely", "microlink", "outbrain",
    "quora", "mj12bot", "ahrefsbot", "semrushbot", "dotbot", "coccoc", "petalbot",
    "bytespider", "gptbot", "claudebot", "ccbot", "anthropic-ai", "perplexitybot",
    "applebot", "bingbot", "googlebot", "adsbot", "mediapartners-google", "slurp",
    "duckduckbot", "bravebot", "mojeek", "yandex", "baidu", "sogou", "exabot",
    "facebot", "ia_archiver", "bot", "crawl", "spider", "scrape", "archiver",
];

/// Layered bot check for the preview page (humans keep the HTML, anything
/// else gets the raw bytes):
/// 1. `Sec-Fetch-Mode: navigate` is only ever sent by browsers on page
///    loads - always a user.
/// 2. Known bot/crawler/curler User-Agent tokens.
/// 3. `Accept` without `text/html` (curl `*/*`, API clients), or neither
///    header at all (browsers navigating always send both).
fn is_bot(headers: &HeaderMap) -> bool {
    if headers
        .get("sec-fetch-mode")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|mode| mode.trim().eq_ignore_ascii_case("navigate"))
    {
        return false;
    }
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if BOT_UA_TOKENS.iter().any(|token| ua.contains(token)) {
        return true;
    }
    match headers.get(header::ACCEPT).and_then(|v| v.to_str().ok()) {
        Some(accept) => !accept.contains("text/html"),
        None => ua.is_empty(),
    }
}

struct PreviewPage {
    kind: PreviewKind,
    title: String,
    filename: String,
    size_label: String,
    raw_url: String,
    download_url: String,
    mime: String,
}

fn og_tags(page: &PreviewPage) -> String {
    use std::fmt::Write as _;
    let mut tags = format!(
        "<meta property=\"og:title\" content=\"{title}\">\n<meta property=\"og:description\" content=\"Preview on Juicebox\">",
        title = escape_html(&page.filename),
    );
    match page.kind {
        PreviewKind::Video => {
            let _ = write!(
                tags,
                "\n<meta property=\"og:video\" content=\"{url}\">\n<meta property=\"og:video:type\" content=\"{mime}\">",
                url = escape_html(&page.raw_url),
                mime = escape_html(&page.mime),
            );
        }
        PreviewKind::Audio => {
            let _ = write!(
                tags,
                "\n<meta property=\"og:audio\" content=\"{url}\">\n<meta property=\"og:audio:type\" content=\"{mime}\">",
                url = escape_html(&page.raw_url),
                mime = escape_html(&page.mime),
            );
        }
        PreviewKind::Image => {
            let _ = write!(
                tags,
                "\n<meta property=\"og:image\" content=\"{url}\">",
                url = escape_html(&page.raw_url),
            );
        }
        _ => {}
    }
    tags
}

fn stage_html(page: &PreviewPage) -> String {
    let raw = escape_html(&page.raw_url);
    match page.kind {
        PreviewKind::Video => format!(
            "<video id=\"media\" controls playsinline preload=\"metadata\" src=\"{raw}\" aria-label=\"{name}\"></video>",
            name = escape_html(&page.filename),
        ),
        PreviewKind::Audio => format!(
            "<div class=\"audio-hero\" aria-hidden=\"true\">&#9835;</div><audio id=\"media\" controls preload=\"metadata\" src=\"{raw}\"></audio>",
        ),
        PreviewKind::Image => format!(
            "<img id=\"media\" src=\"{raw}\" alt=\"{name}\" loading=\"eager\">",
            name = escape_html(&page.filename),
        ),
        PreviewKind::Pdf => format!(
            "<iframe id=\"media\" class=\"pdf-frame\" src=\"{raw}\" title=\"{name}\"></iframe><noscript><p class=\"fallback-note\"><a href=\"{raw}\">Open the PDF</a></p></noscript>",
            name = escape_html(&page.filename),
        ),
        PreviewKind::Text => format!(
            "<pre id=\"textview\" class=\"text-view\" data-raw=\"{raw}\" aria-live=\"polite\">Loading preview...</pre><noscript><p class=\"fallback-note\"><a href=\"{raw}\">Open the raw file</a> (text preview needs JavaScript).</p></noscript>",
        ),
        PreviewKind::Download => format!(
            "<div class=\"dl-hero\"><div class=\"dl-icon\" aria-hidden=\"true\">&#8681;</div><p class=\"dl-name\">{name}</p><p class=\"dl-meta\">{mime} &middot; {size}</p><a class=\"dl-btn\" href=\"{dl}\">Download</a></div>",
            name = escape_html(&page.filename),
            mime = escape_html(&page.mime),
            size = escape_html(&page.size_label),
            dl = escape_html(&page.download_url),
        ),
    }
}

fn render_preview(page: &PreviewPage) -> String {
    let notice = format!(
        " Juicebox preview page for {} - this HTML is a preview, NOT the raw file. Raw bytes: {}. Bots are redirected to the raw file. ",
        page.filename,
        page.raw_url,
    );
    PREVIEW_TEMPLATE
        .replace("__CURL_NOTICE__", &notice)
        .replace("__HEAD_META__", juiceutils::web::HEAD_META.trim_end())
        .replace(
            "__FONT_FACE__",
            &juiceutils::web::font_face_css(TITLE_FONT_DATA_URI.trim()),
        )
        .replace("__BASE_CSS__", juiceutils::web::BASE_CSS.trim_end())
        .replace("__BRAND_CSS__", juiceutils::web::BRAND_CSS.trim_end())
        .replace("__BRAND__", &juiceutils::web::brand_html(LOGO_DATA_URI.trim()))
        .replace("__APP_JS__", PREVIEW_APP_JS.trim_end())
        .replace("__TITLE__", &escape_html(&page.title))
        .replace("__TEXT_MAX__", &TEXT_PREVIEW_MAX_BYTES.to_string())
        .replace("__OG_TAGS__", &og_tags(page))
        .replace("__FILENAME__", &escape_html(&page.filename))
        .replace("__SIZE__", &escape_html(&page.size_label))
        .replace("__RAW_URL__", &escape_html(&page.raw_url))
        .replace("__DOWNLOAD_URL__", &escape_html(&page.download_url))
        .replace("__STAGE__", &stage_html(page))
}

#[tracing::instrument(skip_all)]
/// Render the fullscreen preview page for a stored file.
///
/// # Errors
///
/// Returns [`JuicehostError::BadRequest`] for invalid ids and
/// [`JuicehostError::Internal`] on storage failure; missing files render the
/// HTML 404 page instead of erroring.
pub async fn preview_file_wildcard(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(path): Path<String>,
) -> Result<Response<Body>, crate::error::JuicehostError> {
    use crate::error::JuicehostError;

    let (id, ext) = match path.rsplit_once('.') {
        Some((id, ext)) if !id.is_empty() && !ext.is_empty() => (id.to_string(), ext.to_string()),
        _ => (path.clone(), String::new()),
    };
    if !is_valid_id(&id) {
        return Err(JuicehostError::BadRequest);
    }
    preview_file_inner(state, headers, &id, &ext).await
}

async fn preview_file_inner(
    state: Arc<AppState>,
    headers: HeaderMap,
    id: &str,
    ext: &str,
) -> Result<Response<Body>, crate::error::JuicehostError> {
    use crate::error::JuicehostError;

    let meta = match state.storage.stat(id).await {
        Ok(meta) => meta,
        Err(crate::error::StorageError::NotFound) => {
            return Ok(not_found_html().into_response());
        }
        Err(_) => return Err(JuicehostError::Internal),
    };

    if let Some(shelled) = protected_shell_response(&state, id, ShellMode::Preview).await {
        return Ok(shelled);
    }

    let extension = if ext.is_empty() {
        meta.extension.clone()
    } else {
        ext.to_ascii_lowercase()
    };
    let filename = if extension.is_empty() {
        id.to_string()
    } else {
        format!("{id}.{extension}")
    };
    let raw_url = format!("/f/{filename}");
    // Non-interactive clients (curl, unfurlers, crawlers) get the raw bytes
    // instead of the preview page. Humans keep the HTML.
    if is_bot(&headers) {
        return Response::builder()
            .status(axum::http::StatusCode::FOUND)
            .header(header::LOCATION, &raw_url)
            .header(header::CACHE_CONTROL, NO_STORE)
            .body(Body::empty())
            .map_err(|_| JuicehostError::Internal);
    }
    let mime = storage::guess_mime(&extension);
    let title = format!("{filename} - Juicebox");
    let download_url = format!("/d/{filename}");
    let page = PreviewPage {
        kind: preview_kind(&mime),
        title,
        filename,
        size_label: human_size(meta.size),
        raw_url,
        download_url,
        mime,
    };
    Response::builder()
        .status(axum::http::StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, NO_STORE)
        .body(Body::from(render_preview(&page)))
        .map_err(|_| JuicehostError::Internal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_by_mime() {
        assert_eq!(preview_kind("video/mp4"), PreviewKind::Video);
        assert_eq!(preview_kind("video/webm; codecs=vp9"), PreviewKind::Video);
        assert_eq!(preview_kind("audio/mpeg"), PreviewKind::Audio);
        assert_eq!(preview_kind("audio/wav"), PreviewKind::Audio);
        assert_eq!(preview_kind("image/png"), PreviewKind::Image);
        assert_eq!(preview_kind("image/svg+xml"), PreviewKind::Image);
        assert_eq!(preview_kind("application/pdf"), PreviewKind::Pdf);
        assert_eq!(preview_kind("text/plain; charset=utf-8"), PreviewKind::Text);
        assert_eq!(preview_kind("application/json"), PreviewKind::Text);
        assert_eq!(preview_kind("application/javascript"), PreviewKind::Text);
        assert_eq!(preview_kind("application/ld+json"), PreviewKind::Text);
        assert_eq!(preview_kind("application/zip"), PreviewKind::Download);
        assert_eq!(preview_kind("application/octet-stream"), PreviewKind::Download);
    }

    #[test]
    fn sizes_are_human() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2.0 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
    }

    #[test]
    fn stage_has_no_card_chrome() {
        let page = PreviewPage {
            kind: PreviewKind::Video,
            title: "a.mp4 - Juicebox".into(),
            filename: "a.mp4".into(),
            size_label: "1.0 MB".into(),
            raw_url: "/f/a.mp4".into(),
            download_url: "/d/a.mp4".into(),
            mime: "video/mp4".into(),
        };
        let html = render_preview(&page);
        assert!(html.contains("<video"));
        assert!(html.contains("/f/a.mp4"));
        assert!(!html.contains("card"));
    }
}
