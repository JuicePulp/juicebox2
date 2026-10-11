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
    extract::{Extension, Path, State},
    http::{HeaderMap, Method, header},
    response::{IntoResponse, Response},
};

use super::{
    shell::{ShellMode, protected_shell_response},
    stats::{ViewerIp, report_file_hit},
};
use crate::{
    error::{frozen_html, not_found_html},
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
        )
    {
        PreviewKind::Text
    } else {
        PreviewKind::Download
    }
}

pub(crate) fn human_size(bytes: u64) -> String {
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
    "curl",
    "wget",
    "httpie",
    "aria2",
    "axel",
    "python-urllib",
    "python-requests",
    "go-http-client",
    "okhttp",
    "libwww-perl",
    "scrapy",
    "node-fetch",
    "undici",
    "got",
    "axios",
    "httpclient",
    "perl",
    "ruby",
    "php",
    "powershell",
    "discordbot",
    "telegram",
    "twitterbot",
    "facebookexternalhit",
    "facebookcatalog",
    "slackbot",
    "linkedinbot",
    "whatsapp",
    "skypeuripreview",
    "mastodon",
    "misskey",
    "pleroma",
    "pixelfed",
    "matrix",
    "signal",
    "line",
    "kakao",
    "viber",
    "pinterest",
    "redditbot",
    "tumblr",
    "bitlybot",
    "embedly",
    "iframely",
    "microlink",
    "outbrain",
    "quora",
    "mj12bot",
    "ahrefsbot",
    "semrushbot",
    "dotbot",
    "coccoc",
    "petalbot",
    "bytespider",
    "gptbot",
    "claudebot",
    "ccbot",
    "anthropic-ai",
    "perplexitybot",
    "applebot",
    "bingbot",
    "googlebot",
    "adsbot",
    "mediapartners-google",
    "slurp",
    "duckduckbot",
    "bravebot",
    "mojeek",
    "yandex",
    "baidu",
    "sogou",
    "exabot",
    "facebot",
    "ia_archiver",
    "bot",
    "crawl",
    "spider",
    "scrape",
    "archiver",
];








fn is_discordbot(headers: &HeaderMap) -> bool {
    headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .contains("discordbot")
}

fn request_base_url(headers: &HeaderMap) -> Option<String> {
    let host = headers
        .get("x-forwarded-host")
        .and_then(|v| v.to_str().ok())
        .or_else(|| headers.get(header::HOST).and_then(|v| v.to_str().ok()))?;
    let host = host.split(',').next()?.trim().trim_end_matches('/');
    if host.is_empty() {
        return None;
    }
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| s == "http" || s == "https")
        .unwrap_or_else(|| "https".to_owned());
    Some(format!("{proto}://{host}"))
}

fn absolute_for(base: Option<&str>, path: &str) -> String {
    match base {
        Some(b) => format!("{}{path}", b.trim_end_matches('/')),
        None => path.to_string(),
    }
}

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
    page_url: String,
    mime: String,
    extension: String,
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let truncated: String = value.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{truncated}…")
}

fn embed_gallery_supported(kind: PreviewKind, extension: &str) -> bool {
    let ext = extension.to_ascii_lowercase();
    match kind {
        PreviewKind::Image => matches!(
            ext.as_str(),
            "png" | "gif" | "jpg" | "jpeg" | "webp" | "avif"
        ),
        PreviewKind::Video => matches!(ext.as_str(), "mp4" | "webm" | "mov"),
        _ => false,
    }
}

fn component_embed_payload(
    page: &PreviewPage,
    abs_raw: &str,
    abs_page: &str,
    abs_download: &str,
) -> serde_json::Value {
    let name = truncate_chars(&page.filename, 80);
    let meta = truncate_chars(&format!("{} · {}", page.mime, page.size_label), 160);
    let heading = format!("## [{name}]({abs_page})\n{meta}");
    let mut container: Vec<serde_json::Value> = Vec::new();
    container.push(serde_json::json!({
        "type": 10,
        "content": heading,
    }));
    if embed_gallery_supported(page.kind, &page.extension) {
        container.push(serde_json::json!({
            "type": 12,
            "items": [{ "media": { "url": abs_raw }, "description": name }],
        }));
    } else {
        let hint = match page.kind {
            PreviewKind::Audio => "Audio file — open the preview page to listen.",
            PreviewKind::Pdf => "PDF file — open the preview page to read it.",
            PreviewKind::Text => "Text file — open the preview page to read it.",
            _ => "File ready — open the preview page or download it.",
        };
        container.push(serde_json::json!({
            "type": 10,
            "content": hint,
        }));
    }
    if abs_page.len() <= 512 && abs_download.len() <= 512 {
        container.push(serde_json::json!({
            "type": 1,
            "components": [
                { "type": 2, "style": 5, "url": abs_page, "label": "Open" },
                { "type": 2, "style": 5, "url": abs_download, "label": "Download" },
            ],
        }));
    }
    serde_json::json!({
        "component": {
            "type": 17,
            "accent_color": 5793266,
            "components": container,
        }
    })
}

fn component_embed_script(
    page: &PreviewPage,
    abs_raw: &str,
    abs_page: &str,
    abs_download: &str,
) -> String {
    let mut payload = component_embed_payload(page, abs_raw, abs_page, abs_download);
    let mut serialized = serde_json::to_string(&payload).unwrap_or_default();
    if serialized.len() > 3000 {
        let name = truncate_chars(&page.filename, 40);
        let meta = truncate_chars(&format!("{} · {}", page.mime, page.size_label), 80);
        payload = serde_json::json!({
            "component": {
                "type": 17,
                "components": [{ "type": 10, "content": format!("## [{name}]({abs_page})\n{meta}") }],
            }
        });
        serialized = serde_json::to_string(&payload).unwrap_or_default();
    }
    if serialized.is_empty() {
        return String::new();
    }
    format!(
        "<script id=\"discord:component-embed\" type=\"application/json\">\n{serialized}\n</script>"
    )
}

fn og_tags(page: &PreviewPage, abs_raw: &str, abs_page: &str) -> String {
    use std::fmt::Write as _;
    let description = format!("{} · {}", page.mime, page.size_label);
    let mut tags = format!(
        "<meta property=\"og:title\" content=\"{title}\">\n<meta property=\"og:description\" content=\"{desc}\">\n<meta property=\"og:url\" content=\"{url}\">\n<meta property=\"og:type\" content=\"website\">",
        title = escape_html(&page.filename),
        desc = escape_html(&description),
        url = escape_html(abs_page),
    );
    match page.kind {
        PreviewKind::Video => {
            let _ = write!(
                tags,
                "\n<meta property=\"og:video\" content=\"{url}\">\n<meta property=\"og:video:type\" content=\"{mime}\">",
                url = escape_html(abs_raw),
                mime = escape_html(&page.mime),
            );
        }
        PreviewKind::Audio => {
            let _ = write!(
                tags,
                "\n<meta property=\"og:audio\" content=\"{url}\">\n<meta property=\"og:audio:type\" content=\"{mime}\">",
                url = escape_html(abs_raw),
                mime = escape_html(&page.mime),
            );
        }
        PreviewKind::Image => {
            let _ = write!(
                tags,
                "\n<meta property=\"og:image\" content=\"{url}\">",
                url = escape_html(abs_raw),
            );
        }
        _ => {}
    }
    let card = match page.kind {
        PreviewKind::Image | PreviewKind::Video => "summary_large_image",
        _ => "summary",
    };
    let _ = write!(tags, "\n<meta name=\"twitter:card\" content=\"{card}\">");
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

fn render_preview(page: &PreviewPage, headers: &HeaderMap) -> String {
    let notice = format!(
        " Juicebox preview page for {} - this HTML is a preview, NOT the raw file. Raw bytes: {}. Bots are redirected to the raw file. ",
        page.filename, page.raw_url,
    );
    let base = request_base_url(headers);
    let abs_raw = absolute_for(base.as_deref(), &page.raw_url);
    let abs_page = absolute_for(base.as_deref(), &page.page_url);
    let abs_download = absolute_for(base.as_deref(), &page.download_url);
    PREVIEW_TEMPLATE
        .replace("__CURL_NOTICE__", &notice)
        .replace("__HEAD_META__", juiceutils::web::HEAD_META.trim_end())
        .replace(
            "__FONT_FACE__",
            &juiceutils::web::font_face_css(TITLE_FONT_DATA_URI.trim()),
        )
        .replace("__BASE_CSS__", juiceutils::web::BASE_CSS.trim_end())
        .replace("__BRAND_CSS__", juiceutils::web::BRAND_CSS.trim_end())
        .replace(
            "__BRAND__",
            &juiceutils::web::brand_html(LOGO_DATA_URI.trim()),
        )
        .replace("__PREVIEW_CSS__", juiceutils::web::PREVIEW_CSS.trim_end())
        .replace("__APP_JS__", PREVIEW_APP_JS.trim_end())
        .replace("__TITLE__", &escape_html(&page.title))
        .replace("__TEXT_MAX__", &TEXT_PREVIEW_MAX_BYTES.to_string())
        .replace("__OG_TAGS__", &og_tags(page, &abs_raw, &abs_page))
        .replace(
            "__DISCORD_EMBED__",
            &component_embed_script(page, &abs_raw, &abs_page, &abs_download),
        )
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
    Extension(viewer): Extension<ViewerIp>,
    method: Method,
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
    preview_file_inner(state, headers, method, &id, &ext, viewer.0).await
}

async fn preview_file_inner(
    state: Arc<AppState>,
    headers: HeaderMap,
    method: Method,
    id: &str,
    ext: &str,
    viewer_ip: Option<String>,
) -> Result<Response<Body>, crate::error::JuicehostError> {
    use crate::error::JuicehostError;

    let meta = match state.storage.stat(id).await {
        Ok(meta) => meta,
        Err(crate::error::StorageError::Frozen) => {
            // Same visibility rules as the serve path: 451, never cached.
            let (status, html) = frozen_html();
            return Response::builder()
                .status(status)
                .header(header::CACHE_CONTROL, NO_STORE)
                .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                .body(Body::from(html.0))
                .map_err(|_| JuicehostError::Internal);
        }
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
    let page_url = format!("/v/{filename}");
    let download_url = format!("/d/{filename}");

    if is_bot(&headers) && !is_discordbot(&headers) {
        return Response::builder()
            .status(axum::http::StatusCode::FOUND)
            .header(header::LOCATION, &raw_url)
            .header(header::CACHE_CONTROL, NO_STORE)
            .body(Body::empty())
            .map_err(|_| JuicehostError::Internal);
    }
    let mime = storage::guess_mime(&extension);
    let title = format!("{filename} - Juicebox");
    let page = PreviewPage {
        kind: preview_kind(&mime),
        title,
        filename,
        size_label: human_size(meta.size),
        raw_url,
        download_url,
        page_url,
        mime,
        extension,
    };
    let response = Response::builder()
        .status(axum::http::StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, NO_STORE)
        .body(Body::from(render_preview(&page, &headers)))
        .map_err(|_| JuicehostError::Internal)?;
    
    
    
    if method == Method::GET
        && let Some(ip) = viewer_ip.as_deref()
    {
        report_file_hit(&state, id, "view", ip);
    }
    Ok(response)
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
        assert_eq!(
            preview_kind("application/octet-stream"),
            PreviewKind::Download
        );
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
            page_url: "/v/a.mp4".into(),
            mime: "video/mp4".into(),
            extension: "mp4".into(),
        };
        let html = render_preview(&page, &HeaderMap::new());
        assert!(html.contains("<video"));
        assert!(html.contains("/f/a.mp4"));
        assert!(!html.contains("preview-card"));
        assert!(html.contains("discord:component-embed"));
        assert!(html.contains("twitter:card"));
    }
}
