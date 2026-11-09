use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Extension, Path, State},
    http::{HeaderMap, Method, header},
    response::{IntoResponse, Response},
};
use juiceutils::text::escape_html;

use super::{
    audio_embed::{self, AudioEmbedInfo},
    common::{NO_STORE, backend_request},
    shell::{BackendFileMeta, ShellMode, protected_shell_response},
    stats::{ViewerIp, report_file_hit},
};
use crate::{
    error::{frozen_html, not_found_html},
    state::AppState,
    storage,
    storage::valid_component as is_valid_id,
};

// Only the head of a text file is loaded for the inline preview.
const TEXT_PREVIEW_MAX_BYTES: u64 = 262_144;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewKind {
    Video,
    Audio,
    Image,
    Pdf,
    Text,
    Zip,
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
    } else if base == "application/zip"
        || base == "application/x-7z-compressed"
        || base == "application/x-tar"
        || base == "application/gzip"
    {
        PreviewKind::Zip
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

const PREVIEW_TEMPLATE: &str = include_str!("../templates/preview.html");
const PREVIEW_APP_JS: &str = include_str!("../templates/preview_app.js");
const ZIP_APP_JS: &str = include_str!("../templates/zip_app.js");
const LIVE_UPLOAD_JS: &str = include_str!("../templates/live_upload.js");
const LOGO_DATA_URI: &str = include_str!("../templates/logo_b64.txt");
const TITLE_FONT_DATA_URI: &str = include_str!("../templates/title_b64.txt");

const BOT_UA_TOKENS: &[&str] = &[
    "curl",
    "wget",
    "computercraft",
    "cc-tweaked",
    "craftos",
    "opencomputers",
    "lua-http",
    "luasocket",
    "lua-socket",
    "httpie",
    "aria2",
    "axel",
    "python-urllib",
    "python-requests",
    "httpx",
    "aiohttp",
    "go-http-client",
    "okhttp",
    "reqwest",
    "hyper",
    "libwww-perl",
    "scrapy",
    "node-fetch",
    "node/",
    "undici",
    "bun/",
    "deno/",
    "got",
    "axios",
    "httpclient",
    "java/",
    "java-http-client",
    "postmanruntime",
    "insomnia",
    "restclient",
    "http.rb",
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

const BROWSER_UA_TOKENS: &[&str] = &[
    "mozilla",
    "applewebkit",
    "gecko/",
    "trident/",
    "presto/",
    "edg/",
    "opr/",
];

fn looks_like_browser(ua: &str) -> bool {
    BROWSER_UA_TOKENS.iter().any(|token| ua.contains(token))
}

fn is_discordbot(headers: &HeaderMap) -> bool {
    headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .contains("discordbot")
}

/// Best-effort absolute origin from proxy headers, defaulting to https; a
/// spoofed host only affects this response.
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

/// True when the request wants raw bytes rather than HTML. `sec-fetch-mode:
/// navigate` counts as a browser; otherwise UA tokens win, then `Accept`, with
/// a missing `Accept` falling back to the UA.
pub(crate) fn is_bot(headers: &HeaderMap) -> bool {
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
    match headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
    {
        Some(accept) if !accept.is_empty() => !accept.contains("text/html"),
        _ => !looks_like_browser(&ua),
    }
}

#[derive(Debug, Clone, Copy)]
struct UploadInfo {
    total: u64,
    uploaded: u64,
    complete: bool,
}

struct PreviewPage {
    kind: PreviewKind,
    title: String,
    filename: String,
    display_name: String,
    author: Option<String>,
    size_label: String,
    raw_url: String,
    download_url: String,
    page_url: String,
    file_id: String,
    mime: String,
    extension: String,

    zip_html: Option<String>,

    text_head: Option<(String, u64, String)>,

    uploading: Option<UploadInfo>,
}

fn canonical_url_name(id: &str, extension: &str) -> String {
    if extension.is_empty() {
        id.to_string()
    } else {
        format!("{id}.{extension}")
    }
}

fn resolved_display_name(meta: Option<&BackendFileMeta>, filename: &str) -> String {
    meta.and_then(BackendFileMeta::display_name).map_or_else(
        || {
            if filename.trim().is_empty() {
                "file".to_string()
            } else {
                filename.to_string()
            }
        },
        |name| truncate_chars(name, 120),
    )
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let truncated: String = value.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{truncated}...")
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

fn kind_label(kind: PreviewKind) -> &'static str {
    match kind {
        PreviewKind::Video => "Video file",
        PreviewKind::Audio => "Audio file",
        PreviewKind::Image => "Image file",
        PreviewKind::Pdf => "PDF file",
        PreviewKind::Text => "Text file",
        PreviewKind::Zip => "Archive",
        PreviewKind::Download => "File",
    }
}

/// Builds a Discord Components V2 payload; component type numbers follow
/// Discord's schema.
fn component_embed_payload(
    page: &PreviewPage,
    abs_raw: &str,
    abs_page: &str,
    abs_download: &str,
    audio_video_url: Option<&str>,
) -> serde_json::Value {
    let name = truncate_chars(&page.display_name, 80);
    let meta = truncate_chars(
        &format!("{} - {}", kind_label(page.kind), page.size_label),
        160,
    );
    let heading = format!("## [{name}]({abs_page})\n{meta}");
    let mut container: Vec<serde_json::Value> = Vec::new();
    container.push(serde_json::json!({
        "type": 10,
        "content": heading,
    }));

    if page.kind == PreviewKind::Text
        && let Some(snippet) = page
            .text_head
            .as_ref()
            .map(|t| t.2.as_str())
            .filter(|s| !s.trim().is_empty())
    {
        container.push(serde_json::json!({
            "type": 10,
            "content": format!("```\n{}\n```", truncate_chars(snippet, 1000)),
        }));
    }
    if embed_gallery_supported(page.kind, &page.extension) {
        let mut item = serde_json::json!({ "media": { "url": abs_raw } });
        if let Some(author) = page.author.as_deref() {
            item["description"] = serde_json::Value::String(truncate_chars(author, 80));
        }
        container.push(serde_json::json!({
            "type": 12,
            "items": [item],
        }));
    } else if page.kind == PreviewKind::Audio
        && let Some(url) = audio_video_url
    {
        let mut item = serde_json::json!({ "media": { "url": url } });
        if let Some(author) = page.author.as_deref() {
            item["description"] = serde_json::Value::String(truncate_chars(author, 80));
        }
        container.push(serde_json::json!({
            "type": 12,
            "items": [item],
        }));
        container.push(serde_json::json!({
            "type": 10,
            "content": "Audio preview, or download it here.",
        }));
    } else {
        let hint = match page.kind {
            PreviewKind::Audio => "Press play to listen, or download it here.",
            PreviewKind::Pdf => "Press the open button to read more, or download it here.",
            PreviewKind::Text => "Press the open button to see more, or download it here.",
            PreviewKind::Zip => "Open the archive to browse what is inside, or download it here.",
            _ => "Press the open button to see more, or download it here.",
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

// `<`, `>` and `&` are escaped so the JSON cannot terminate the surrounding
// script element.
fn json_script_payload(value: &serde_json::Value) -> String {
    serde_json::to_string(value)
        .unwrap_or_default()
        .replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
}

/// Wraps the embed in its script tag, degrading to a compact heading above 3000
/// bytes.
fn component_embed_script(
    page: &PreviewPage,
    abs_raw: &str,
    abs_page: &str,
    abs_download: &str,
    audio_video_url: Option<&str>,
) -> String {
    let mut payload =
        component_embed_payload(page, abs_raw, abs_page, abs_download, audio_video_url);
    let mut serialized = json_script_payload(&payload);
    if serialized.len() > 3000 {
        let name = truncate_chars(&page.display_name, 40);
        let meta = truncate_chars(&format!("{} - {}", page.mime, page.size_label), 80);
        payload = serde_json::json!({
            "component": {
                "type": 17,
                "components": [{ "type": 10, "content": format!("## [{name}]({abs_page})\n{meta}") }],
            }
        });
        serialized = json_script_payload(&payload);
    }
    if serialized.is_empty() {
        return String::new();
    }
    format!(
        "<script id=\"discord:component-embed\" type=\"application/json\">\n{serialized}\n</script>"
    )
}

struct AudioEmbedUrls {
    video: String,
    poster: String,
    width: u32,
    height: u32,
}

fn og_tags(
    page: &PreviewPage,
    abs_raw: &str,
    abs_page: &str,
    audio_urls: Option<&AudioEmbedUrls>,
) -> String {
    use std::fmt::Write as _;
    let description = format!("{} - {}", page.mime, page.size_label);
    let mut tags = format!(
        "<meta property=\"og:title\" content=\"{title}\">\n<meta property=\"og:description\" content=\"{desc}\">\n<meta property=\"og:url\" content=\"{url}\">\n<meta property=\"og:type\" content=\"website\">",
        title = escape_html(&page.display_name),
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
            if let Some(urls) = audio_urls {
                let _ = write!(
                    tags,
                    "\n<meta property=\"og:video\" content=\"{url}\">\n<meta property=\"og:video:secure_url\" content=\"{url}\">\n<meta property=\"og:video:type\" content=\"video/mp4\">\n<meta property=\"og:video:width\" content=\"{width}\">\n<meta property=\"og:video:height\" content=\"{height}\">\n<meta property=\"og:image\" content=\"{poster}\">",
                    url = escape_html(&urls.video),
                    width = urls.width,
                    height = urls.height,
                    poster = escape_html(&urls.poster),
                );
            } else {
                let _ = write!(
                    tags,
                    "\n<meta property=\"og:audio\" content=\"{url}\">\n<meta property=\"og:audio:type\" content=\"{mime}\">",
                    url = escape_html(abs_raw),
                    mime = escape_html(&page.mime),
                );
            }
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
        PreviewKind::Audio if audio_urls.is_some() => "player",
        _ => "summary",
    };
    let _ = write!(tags, "\n<meta name=\"twitter:card\" content=\"{card}\">");
    tags
}

fn text_head_from_bytes(bytes: &[u8]) -> Option<(String, u64, String)> {
    if bytes.is_empty() {
        return None;
    }
    let valid = match std::str::from_utf8(bytes) {
        Ok(_) => bytes.len(),
        Err(e) => e.valid_up_to(),
    };
    if valid == 0 {
        return None;
    }
    let raw = String::from_utf8_lossy(&bytes[..valid]);
    let snippet: String = raw.chars().take(800).collect();
    // Fence markers are neutralized so the snippet cannot escape its code block.
    Some((
        escape_html(&raw),
        valid as u64,
        snippet.replace("```", "'''"),
    ))
}

async fn load_text_head(
    storage: &Arc<dyn storage::StorageBackend>,
    id: &str,
    size: u64,
) -> Option<(String, u64, String)> {
    let end = size.min(TEXT_PREVIEW_MAX_BYTES);
    if end == 0 {
        return None;
    }
    let bytes =
        super::zip_list::read_range_capped(storage, id, 0, end - 1, TEXT_PREVIEW_MAX_BYTES).await?;
    text_head_from_bytes(&bytes)
}

async fn load_remote_text_head(
    state: &AppState,
    location: &super::route::RemoteLocation,
    id: &str,
    size: u64,
) -> Option<(String, u64, String)> {
    let end = if size == 0 {
        TEXT_PREVIEW_MAX_BYTES
    } else {
        size.min(TEXT_PREVIEW_MAX_BYTES)
    };
    let url = if let Some(r2_url) = location.r2_url.as_deref() {
        if !super::route::is_http_url(r2_url) {
            return None;
        }
        r2_url.to_owned()
    } else {
        let storage_host = location.storage_host.as_deref()?;
        if !super::route::is_http_url(storage_host) {
            return None;
        }
        format!("{}/f/{}", storage_host.trim_end_matches('/'), id)
    };
    let response = state
        .backend_client
        .get(url)
        .header(header::RANGE, format!("bytes=0-{}", end.saturating_sub(1)))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() && response.status() != reqwest::StatusCode::PARTIAL_CONTENT
    {
        return None;
    }
    let bytes = response.bytes().await.ok()?;
    let capped = &bytes[..bytes.len().min(end as usize)];
    text_head_from_bytes(capped)
}

async fn backend_uploading_meta(state: &AppState, id: &str) -> Option<(String, u64)> {
    let backend_url = state.backend_url.as_ref()?;
    let status_url = juiceproto::config::file_status_url(backend_url, id);
    let resp = backend_request(state, status_url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body = resp.json::<serde_json::Value>().await.ok()?;
    if body.get("status").and_then(|s| s.as_str()) != Some("uploading") {
        return None;
    }
    let filename = body
        .get("filename")
        .and_then(|s| s.as_str())
        .unwrap_or("upload")
        .to_string();
    let total = body
        .get("size_bytes")
        .and_then(|s| s.as_i64())
        .unwrap_or(0)
        .max(0) as u64;
    Some((filename, total))
}

fn download_hero(page: &PreviewPage) -> String {
    let upload_note = page.uploading.map_or_else(String::new, |_| {
        "<p class=\"dl-upload-note\">This file is currently uploading! I can't show you a preview of the file while it happens for good measures.</p>".to_string()
    });
    format!(
        "<div class=\"dl-hero\"><div class=\"dl-icon\" aria-hidden=\"true\">{}</div><p class=\"dl-name\">{name}</p><p class=\"dl-meta\">{mime} - {size}</p>{upload_note}<a class=\"dl-btn\" href=\"{dl}\">Download</a></div>",
        juiceutils::icons::DOWNLOAD_ART,
        name = escape_html(&page.filename),
        mime = escape_html(&page.mime),
        size = escape_html(&page.size_label),
        upload_note = upload_note,
        dl = escape_html(&page.download_url),
    )
}

fn zip_listing_html(
    archive_id: &str,
    download_url: &str,
    entries: &[super::zip_list::ZipEntry],
    locked: bool,
) -> String {
    if entries.is_empty() && locked {
        return format!(
            "<div class=\"dl-hero\"><div class=\"dl-icon\" aria-hidden=\"true\">{}</div><a class=\"dl-btn\" href=\"{}\">Download</a></div>",
            juiceutils::icons::DOWNLOAD_ART,
            escape_html(download_url),
        );
    }
    const SHOWN: usize = 500;
    let mut rows = String::with_capacity(entries.len().min(SHOWN) * 96);
    for entry in entries.iter().take(SHOWN) {
        let size = if entry.is_dir {
            String::from("-")
        } else {
            entry
                .size
                .map(human_size)
                .unwrap_or_else(|| String::from("?"))
        };
        let modified = entry.modified.as_deref().unwrap_or("-");
        let cls = if entry.is_dir { "zip-dir" } else { "zip-file" };
        let kind_attr = if entry.is_dir {
            "dir"
        } else {
            match entry
                .name
                .rsplit('.')
                .next()
                .unwrap_or("")
                .to_ascii_lowercase()
                .as_str()
            {
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" | "bmp" | "ico" | "svg" => "image",
                _ => "file",
            }
        };
        let dir_attr = if entry.is_dir { "1" } else { "0" };
        rows.push_str(&format!(
            "<tr class=\"{cls}\" data-path=\"{}\" data-dir=\"{dir_attr}\" data-kind=\"{kind_attr}\"><td class=\"zip-ico\">{}</td><td class=\"zip-name\">{}</td><td class=\"zip-size\">{}</td><td class=\"zip-mod\">{}</td></tr>",
            escape_html(&entry.name),
            juiceutils::icons::file_icon(&entry.name, entry.is_dir),
            escape_html(&entry.name),
            escape_html(&size),
            escape_html(modified),
        ));
    }
    let more = entries.len().saturating_sub(SHOWN);
    let more_html = if more > 0 {
        format!("<p class=\"zip-more\">+{more} more files</p>")
    } else {
        String::new()
    };
    let total: u64 = entries
        .iter()
        .filter(|e| !e.is_dir)
        .filter_map(|e| e.size)
        .sum();
    format!(
        "<div class=\"zip-list\" id=\"zipBrowser\"><nav class=\"zip-crumbs\" id=\"zipCrumbs\"></nav><p class=\"zip-head\">{} files - {}</p><div class=\"zip-scroll\"><table class=\"zip-table\" id=\"zipTable\" data-archive=\"{}\"><thead><tr><th></th><th>Name</th><th>Size</th><th>Modified</th></tr></thead><tbody>{}</tbody></table></div>{}<div class=\"zip-lightbox\" id=\"zipLightbox\" hidden><img id=\"zipLightboxImg\" alt=\"\"><p class=\"zip-lightbox-err\" id=\"zipLightboxErr\" hidden></p><button type=\"button\" class=\"zip-lightbox-close\" data-close=\"1\">Close</button></div><a class=\"dl-btn\" href=\"{}\">Download archive</a></div>",
        entries.len(),
        human_size(total),
        escape_html(archive_id),
        rows,
        more_html,
        escape_html(download_url),
    )
}

fn author_attr(page: &PreviewPage, attr: &str) -> String {
    page.author.as_deref().map_or_else(String::new, |author| {
        format!(" {attr}=\"{}\"", escape_html(author))
    })
}

fn stage_html(page: &PreviewPage) -> String {
    let raw = escape_html(&page.raw_url);
    match page.kind {
        PreviewKind::Video => format!(
            "<video id=\"media\" controls playsinline preload=\"metadata\" src=\"{raw}\"{label}></video>",
            label = author_attr(page, "aria-label"),
        ),
        PreviewKind::Audio => format!(
            "<div class=\"audio-hero\" aria-hidden=\"true\">{}</div><audio id=\"media\" controls preload=\"metadata\" src=\"{raw}\"></audio>",
            juiceutils::icons::AUDIO,
        ),
        PreviewKind::Image => format!(
            "<img id=\"media\" src=\"{raw}\"{label} loading=\"eager\">",
            label = author_attr(page, "alt"),
        ),
        PreviewKind::Pdf => format!(
            "<iframe id=\"media\" class=\"pdf-frame\" src=\"{raw}\"{label}></iframe><noscript><p class=\"fallback-note\"><a href=\"{raw}\">Open the PDF</a></p></noscript>",
            label = author_attr(page, "title"),
        ),
        PreviewKind::Text => {
            let (content, loaded, _) = page
                .text_head
                .clone()
                .unwrap_or_else(|| (String::from("Loading preview..."), 0, String::new()));
            let uploading_attr = if page.uploading.is_some() {
                " data-uploading=\"1\""
            } else {
                ""
            };
            format!(
                "<pre id=\"textview\" class=\"text-view\" data-raw=\"{raw}\" data-loaded=\"{loaded}\"{uploading_attr} aria-live=\"polite\">{content}</pre><noscript><p class=\"fallback-note\"><a href=\"{raw}\">Open the raw file</a> (text preview needs JavaScript).</p></noscript>",
            )
        }
        PreviewKind::Download => download_hero(page),
        PreviewKind::Zip => page.zip_html.clone().unwrap_or_else(|| download_hero(page)),
    }
}

/// Fills `preview.html`; every dynamic value is escaped before substitution.
fn render_preview(
    page: &PreviewPage,
    headers: &HeaderMap,
    audio_embed: Option<&AudioEmbedInfo>,
) -> String {
    let notice = format!(
        " Juicebox preview page for {} - this HTML is a preview, NOT the raw file. Raw bytes: {}. Bots are redirected to the raw file. ",
        page.filename, page.raw_url,
    );
    let base = request_base_url(headers);
    let abs_raw = absolute_for(base.as_deref(), &page.raw_url);
    let abs_page = absolute_for(base.as_deref(), &page.page_url);
    let abs_download = absolute_for(base.as_deref(), &page.download_url);
    let audio_urls = audio_embed.and_then(|info| {
        // Crawlers need absolute media URLs, so embeds are skipped without a request
        // origin.
        let base = base.as_deref()?;
        Some(AudioEmbedUrls {
            video: absolute_for(Some(base), &info.video_path),
            poster: absolute_for(Some(base), &info.poster_path),
            width: info.width,
            height: info.height,
        })
    });
    let (upload_hidden, upload_label, upload_width) = page.uploading.map_or_else(
        || ("hidden".to_string(), String::new(), "0".to_string()),
        |upload| {
            let percent = if upload.total == 0 {
                0
            } else {
                upload.uploaded.saturating_mul(100) / upload.total
            };
            let label = if upload.complete {
                "Finalizing upload".to_string()
            } else {
                format!("Uploading {percent}%")
            };
            (String::new(), label, percent.min(100).to_string())
        },
    );
    PREVIEW_TEMPLATE
        .replace("__CURL_NOTICE__", &escape_html(&notice))
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
        .replace(
            "__APP_JS__",
            if page.kind == PreviewKind::Text {
                PREVIEW_APP_JS.trim_end()
            } else if page.kind == PreviewKind::Zip {
                ZIP_APP_JS.trim_end()
            } else {
                ""
            },
        )
        .replace("__TITLE__", &escape_html(&page.title))
        .replace("__TEXT_MAX__", &TEXT_PREVIEW_MAX_BYTES.to_string())
        .replace(
            "__OG_TAGS__",
            &og_tags(page, &abs_raw, &abs_page, audio_urls.as_ref()),
        )
        .replace(
            "__DISCORD_EMBED__",
            &component_embed_script(
                page,
                &abs_raw,
                &abs_page,
                &abs_download,
                audio_urls.as_ref().map(|urls| urls.video.as_str()),
            ),
        )
        .replace("__FILENAME__", &escape_html(&page.filename))
        .replace("__SIZE__", &escape_html(&page.size_label))
        .replace("__RAW_URL__", &escape_html(&page.raw_url))
        .replace("__DOWNLOAD_URL__", &escape_html(&page.download_url))
        .replace("__STAGE__", &stage_html(page))
        .replace("__UPLOAD_HIDDEN__", &upload_hidden)
        .replace("__UPLOAD_LABEL__", &escape_html(&upload_label))
        .replace("__UPLOAD_WIDTH__", &upload_width)
        .replace("__FILE_ID__", &escape_html(&page.file_id))
        .replace("__LIVE_JS__", LIVE_UPLOAD_JS.trim_end())
}

#[tracing::instrument(skip_all)]

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
    preview_file_inner(state, headers, method, &id, &ext, &path, viewer.0).await
}

/// Text preview for a remote-only file; only Discord's crawler gets HTML, other
/// bots get raw bytes.
async fn remote_text_preview(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    method: &Method,
    id: &str,
    ext: &str,
    page_path: &str,
    viewer_ip: Option<&str>,
    location: &super::route::RemoteLocation,
) -> Result<Option<Response<Body>>, crate::error::JuicehostError> {
    use crate::error::JuicehostError;

    if location.status != "ready" {
        return Ok(None);
    }
    if is_bot(headers) && !is_discordbot(headers) {
        return Ok(None);
    }

    let extension = location
        .filename
        .as_deref()
        .map(storage::extract_extension)
        .map(|value| value.to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| ext.to_ascii_lowercase());
    let mime = location
        .mime_type
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| storage::guess_mime(&extension));
    if preview_kind(&mime) != PreviewKind::Text {
        return Ok(None);
    }

    let size = location.size_bytes.unwrap_or(0);
    let filename = canonical_url_name(id, &extension);
    let raw_url = format!("/f/{filename}");
    let download_url = format!("/d/{filename}");
    let page_url = format!("/v/{filename}");

    let abs_self = request_base_url(headers)
        .map(|base| absolute_for(Some(base.as_str()), &format!("/v/{page_path}")));
    let (shelled, backend_meta) =
        protected_shell_response(state, id, ShellMode::Preview, abs_self.as_deref()).await;
    if let Some(shelled) = shelled {
        return Ok(Some(shelled));
    }

    let display_name = resolved_display_name(backend_meta.as_ref(), &filename);
    let author = backend_meta.as_ref().and_then(|meta| meta.author.clone());
    let text_head = load_remote_text_head(state, location, id, size).await;
    let page = PreviewPage {
        kind: PreviewKind::Text,
        title: format!("{display_name} - Juicebox²"),
        filename,
        display_name,
        author,
        size_label: human_size(size),
        raw_url,
        download_url,
        page_url,
        file_id: id.to_string(),
        mime,
        extension,
        zip_html: None,
        text_head,
        uploading: None,
    };
    let response = Response::builder()
        .status(axum::http::StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, NO_STORE)
        .body(Body::from(render_preview(&page, headers, None)))
        .map_err(|_| JuicehostError::Internal)?;
    if *method == Method::GET
        && let Some(ip) = viewer_ip
    {
        report_file_hit(state, id, "view", ip, "preview");
    }
    Ok(Some(response))
}

async fn preview_file_inner(
    state: Arc<AppState>,
    headers: HeaderMap,
    method: Method,
    id: &str,
    ext: &str,
    page_path: &str,
    viewer_ip: Option<String>,
) -> Result<Response<Body>, crate::error::JuicehostError> {
    use crate::error::JuicehostError;

    let meta = match state.storage.stat(id).await {
        Ok(meta) => meta,
        Err(crate::error::StorageError::Frozen) => {
            let (status, html) = frozen_html();
            return Response::builder()
                .status(status)
                .header(header::CACHE_CONTROL, NO_STORE)
                .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                .body(Body::from(html.0))
                .map_err(|_| JuicehostError::Internal);
        }
        Err(crate::error::StorageError::NotFound) => {
            if let Some(location) = super::route::backend_location(&state, id).await {
                if let Some(response) = remote_text_preview(
                    &state,
                    &headers,
                    &method,
                    id,
                    ext,
                    page_path,
                    viewer_ip.as_deref(),
                    &location,
                )
                .await?
                {
                    return Ok(response);
                }
                if let Some(response) =
                    super::route::remote_redirect_with(&headers, "v", page_path, &location)
                {
                    return Ok(response);
                }
            }
            let live = state.live_uploads.progress(id);
            let backend = if live.is_none() {
                backend_uploading_meta(&state, id).await
            } else {
                None
            };
            if live.is_none() && backend.is_none() {
                return Ok(not_found_html().into_response());
            }

            let (mut filename, mut mime, total, uploaded, complete) = match live {
                Some(progress) => (
                    progress.filename,
                    progress.mime,
                    progress.total_size,
                    progress.uploaded,
                    progress.complete,
                ),
                None => {
                    let (name, size) = backend.unwrap_or_default();
                    (name, String::new(), size, 0, false)
                }
            };
            if filename.trim().is_empty() {
                filename = "upload".to_string();
            }
            let extension = if ext.is_empty() {
                storage::extract_extension(&filename).to_ascii_lowercase()
            } else {
                ext.to_ascii_lowercase()
            };
            if mime.trim().is_empty() {
                mime = storage::guess_mime(&extension);
            }
            let canonical = canonical_url_name(id, &extension);
            let raw_url = format!("/f/{canonical}");
            let download_url = format!("/d/{canonical}");
            let page_url = format!("/v/{canonical}");

            if is_bot(&headers) && !is_discordbot(&headers) {
                let mut response = super::serve::serve_file_inner(
                    Arc::clone(&state),
                    headers.clone(),
                    method.clone(),
                    id.to_string(),
                    false,
                    viewer_ip.clone(),
                    &canonical,
                )
                .await?;
                response.headers_mut().insert(
                    header::CACHE_CONTROL,
                    axum::http::HeaderValue::from_static(NO_STORE),
                );
                response.headers_mut().insert(
                    axum::http::HeaderName::from_static("cdn-cache-control"),
                    axum::http::HeaderValue::from_static(NO_STORE),
                );
                return Ok(response);
            }

            let detected = preview_kind(&mime);
            let kind = if detected == PreviewKind::Text {
                PreviewKind::Text
            } else {
                PreviewKind::Download
            };
            let page = PreviewPage {
                kind,
                title: format!("{} - Juicebox²", truncate_chars(&filename, 120)),
                filename: filename.clone(),
                display_name: truncate_chars(&filename, 120),
                author: None,
                size_label: if total == 0 {
                    "uploading".to_string()
                } else {
                    human_size(total)
                },
                raw_url,
                download_url,
                page_url,
                file_id: id.to_string(),
                mime,
                extension,
                zip_html: None,
                text_head: None,
                uploading: Some(UploadInfo {
                    total,
                    uploaded,
                    complete,
                }),
            };
            let response = Response::builder()
                .status(axum::http::StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                .header(header::CACHE_CONTROL, NO_STORE)
                .body(Body::from(render_preview(&page, &headers, None)))
                .map_err(|_| JuicehostError::Internal)?;
            if method == Method::GET
                && (!is_bot(&headers) || is_discordbot(&headers))
                && let Some(ip) = viewer_ip.as_deref()
            {
                report_file_hit(&state, id, "view", ip, "preview");
            }
            return Ok(response);
        }
        Err(_) => return Err(JuicehostError::Internal),
    };

    let extension = if meta.extension.is_empty() {
        ext.to_ascii_lowercase()
    } else {
        meta.extension.to_ascii_lowercase()
    };
    let filename = canonical_url_name(id, &extension);
    let raw_url = format!("/f/{filename}");

    if is_bot(&headers) && !is_discordbot(&headers) {
        let mut response = super::serve::serve_file_inner(
            Arc::clone(&state),
            headers.clone(),
            method.clone(),
            id.to_string(),
            false,
            viewer_ip.clone(),
            &filename,
        )
        .await?;
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static(NO_STORE),
        );
        response.headers_mut().insert(
            axum::http::HeaderName::from_static("cdn-cache-control"),
            axum::http::HeaderValue::from_static(NO_STORE),
        );
        return Ok(response);
    }

    let abs_self = request_base_url(&headers)
        .map(|base| absolute_for(Some(base.as_str()), &format!("/v/{page_path}")));
    let (shelled, backend_meta) =
        protected_shell_response(&state, id, ShellMode::Preview, abs_self.as_deref()).await;
    if let Some(shelled) = shelled {
        return Ok(shelled);
    }

    let display_name = resolved_display_name(backend_meta.as_ref(), &filename);
    let author = backend_meta.as_ref().and_then(|meta| meta.author.clone());
    let page_url = format!("/v/{filename}");
    let download_url = format!("/d/{filename}");
    let mime = storage::guess_mime(&extension);
    let title = format!("{display_name} - Juicebox²");
    let kind = preview_kind(&mime);
    let audio_embed = if kind == PreviewKind::Audio && is_discordbot(&headers) {
        audio_embed::prepare_for_discord(
            &state,
            id,
            &meta,
            &display_name,
            author.as_deref(),
            backend_meta
                .as_ref()
                .and_then(|meta| meta.filename.as_deref()),
        )
        .await
    } else {
        None
    };

    let zip_html = if kind == PreviewKind::Zip {
        super::zip_list::load_archive_entries(&state.storage, id, &filename, meta.size)
            .await
            .map(|listing| zip_listing_html(id, &download_url, &listing.entries, listing.locked))
    } else {
        None
    };
    let text_head = if kind == PreviewKind::Text {
        load_text_head(&state.storage, id, meta.size).await
    } else {
        None
    };
    let page = PreviewPage {
        kind,
        title,
        filename,
        display_name,
        author,
        size_label: human_size(meta.size),
        raw_url,
        download_url,
        page_url,
        file_id: id.to_string(),
        mime,
        extension,
        zip_html,
        text_head,
        uploading: None,
    };
    let response = Response::builder()
        .status(axum::http::StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, NO_STORE)
        .body(Body::from(render_preview(
            &page,
            &headers,
            audio_embed.as_ref(),
        )))
        .map_err(|_| JuicehostError::Internal)?;

    if method == Method::GET
        && (!is_bot(&headers) || is_discordbot(&headers))
        && let Some(ip) = viewer_ip.as_deref()
    {
        report_file_hit(&state, id, "view", ip, "preview");
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computer_craft_is_treated_as_non_browser() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::USER_AGENT,
            "computercraft/1.109.2".parse().expect("ua"),
        );
        assert!(is_bot(&headers));

        headers.insert(
            header::USER_AGENT,
            "CraftOS-PC/2.8.0 (Windows)".parse().expect("ua"),
        );
        assert!(is_bot(&headers));

        headers.insert(
            header::USER_AGENT,
            "Mozilla/5.0 (X11; Linux x86_64) Gecko/20100101 Firefox/130.0"
                .parse()
                .expect("ua"),
        );
        headers.insert(
            header::ACCEPT,
            "text/html,application/xhtml+xml".parse().expect("accept"),
        );
        assert!(!is_bot(&headers));
    }

    #[test]
    fn missing_or_empty_accept_defaults_to_raw_for_non_browsers() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::USER_AGENT,
            "some-lua-client/1.0".parse().expect("ua"),
        );
        assert!(is_bot(&headers));

        headers.insert(header::ACCEPT, "".parse().expect("accept"));
        assert!(is_bot(&headers));

        headers.insert(
            header::USER_AGENT,
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/130.0 Safari/537.36"
                .parse()
                .expect("ua"),
        );
        headers.remove(header::ACCEPT);
        assert!(!is_bot(&headers));
    }

    #[test]
    fn known_clients_win_over_text_html_accept() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ACCEPT,
            "text/html,application/xhtml+xml".parse().expect("accept"),
        );
        for ua in [
            "curl/8.5.0",
            "OpenComputers/1.8",
            "LuaSocket/3.1",
            "python-httpx/0.27",
            "Java/17.0.2",
            "PostmanRuntime/7.39",
            "node/20.0",
        ] {
            headers.insert(header::USER_AGENT, ua.parse().expect("ua"));
            assert!(is_bot(&headers), "{ua} should be treated as a bot");
        }
    }

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
        assert_eq!(preview_kind("application/zip"), PreviewKind::Zip);
        assert_eq!(
            preview_kind("application/x-7z-compressed"),
            PreviewKind::Zip
        );
        assert_eq!(preview_kind("application/x-tar"), PreviewKind::Zip);
        assert_eq!(preview_kind("application/gzip"), PreviewKind::Zip);
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
            title: "My Clip.mp4 - Juicebox²".into(),
            filename: "a.mp4".into(),
            display_name: "My Clip.mp4".into(),
            author: Some("jawed".into()),
            size_label: "1.0 MB".into(),
            raw_url: "/f/a.mp4".into(),
            download_url: "/d/a.mp4".into(),
            page_url: "/v/a.mp4".into(),
            file_id: "a".into(),
            mime: "video/mp4".into(),
            extension: "mp4".into(),
            zip_html: None,
            text_head: None,
            uploading: None,
        };
        let html = render_preview(&page, &HeaderMap::new(), None);
        assert!(html.contains("<video"));
        assert!(html.contains("/f/a.mp4"));
        assert!(!html.contains("preview-card"));
        assert!(html.contains("discord:component-embed"));
        assert!(html.contains("twitter:card"));
        assert!(html.contains("## [My Clip.mp4]"));
        assert!(html.contains("\"description\":\"jawed\""));
        assert!(html.contains("aria-label=\"jawed\""));
    }

    #[test]
    fn missing_author_omits_alt_and_description() {
        let page = PreviewPage {
            kind: PreviewKind::Video,
            title: "a.mp4 - Juicebox²".into(),
            filename: "a.mp4".into(),
            display_name: "a.mp4".into(),
            author: None,
            size_label: "1.0 MB".into(),
            raw_url: "/f/a.mp4".into(),
            download_url: "/d/a.mp4".into(),
            page_url: "/v/a.mp4".into(),
            file_id: "a".into(),
            mime: "video/mp4".into(),
            extension: "mp4".into(),
            zip_html: None,
            text_head: None,
            uploading: None,
        };
        let html = render_preview(&page, &HeaderMap::new(), None);
        assert!(html.contains("<video"));
        assert!(!html.contains("aria-label"));
        assert!(!html.contains("\"description\":"));
    }

    #[test]
    fn audio_with_generated_video_uses_video_metadata() {
        let page = PreviewPage {
            kind: PreviewKind::Audio,
            title: "Song.mp3 - Juicebox²".into(),
            filename: "a.mp3".into(),
            display_name: "Song.mp3".into(),
            author: Some("artist".into()),
            size_label: "4.0 MB".into(),
            raw_url: "/f/a.mp3".into(),
            download_url: "/d/a.mp3".into(),
            page_url: "/v/a.mp3".into(),
            file_id: "a".into(),
            mime: "audio/mpeg".into(),
            extension: "mp3".into(),
            zip_html: None,
            text_head: None,
            uploading: None,
        };
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "f.juicey.dev".parse().expect("host"));
        headers.insert("x-forwarded-proto", "https".parse().expect("proto"));
        let embed = AudioEmbedInfo {
            video_path: "/e/a.mp4".into(),
            poster_path: "/e/a.jpg".into(),
            width: 640,
            height: 360,
        };
        let html = render_preview(&page, &headers, Some(&embed));
        assert!(html.contains("og:video\" content=\"https://f.juicey.dev/e/a.mp4\""));
        assert!(html.contains("og:video:type\" content=\"video/mp4\""));
        assert!(html.contains("og:image\" content=\"https://f.juicey.dev/e/a.jpg\""));
        assert!(html.contains("twitter:card\" content=\"player\""));
        assert!(!html.contains("og:audio"));
        assert!(html.contains("\"type\":12"));
        assert!(html.contains("\"media\":{\"url\":\"https://f.juicey.dev/e/a.mp4\"}"));
    }

    #[test]
    fn text_preview_cannot_break_out_of_component_script() {
        let page = PreviewPage {
            kind: PreviewKind::Text,
            title: "page.html - Juicebox²".into(),
            filename: "a.html".into(),
            display_name: "page.html".into(),
            author: None,
            size_label: "42 B".into(),
            raw_url: "/f/a.html".into(),
            download_url: "/d/a.html".into(),
            page_url: "/v/a.html".into(),
            file_id: "a".into(),
            mime: "text/html".into(),
            extension: "html".into(),
            zip_html: None,
            text_head: Some((
                "&lt;script&gt;alert(1)&lt;/script&gt;".into(),
                42,
                "</script><script>alert(1)</script>".into(),
            )),
            uploading: None,
        };
        let html = render_preview(&page, &HeaderMap::new(), None);

        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));

        let marker = "discord:component-embed";
        let marker_at = html.find(marker).expect("component payload");
        let body_start = html[marker_at..].find('>').expect("script open tag") + marker_at + 1;
        let body_end = html[body_start..]
            .find("</script>")
            .expect("script close tag")
            + body_start;
        let body = html[body_start..body_end].trim();
        let parsed: serde_json::Value =
            serde_json::from_str(body).expect("component payload stays valid JSON");
        assert!(parsed.to_string().contains("</script>"));
    }

    #[test]
    fn canonical_url_name_always_starts_with_id() {
        assert_eq!(canonical_url_name("sd6Q83tz", "mp4"), "sd6Q83tz.mp4");
        assert_eq!(canonical_url_name("abc123", ""), "abc123");
    }

    #[test]
    fn display_name_prefers_title_then_filename_then_canonical() {
        let meta = BackendFileMeta {
            filename: Some("Me at the zoo-jNQXAC9IVRw.mp4".into()),
            title: Some("Me at the zoo".into()),
            author: Some("jawed".into()),
        };
        assert_eq!(
            resolved_display_name(Some(&meta), "X8yBKzsg.mp4"),
            "Me at the zoo"
        );

        let meta = BackendFileMeta {
            filename: Some("My Clip.mp4".into()),
            ..BackendFileMeta::default()
        };
        assert_eq!(
            resolved_display_name(Some(&meta), "abc123.mp4"),
            "My Clip.mp4"
        );

        let meta = BackendFileMeta::default();
        assert_eq!(
            resolved_display_name(Some(&meta), "abc123.mp4"),
            "abc123.mp4"
        );
        assert_eq!(resolved_display_name(None, "abc123.mp4"), "abc123.mp4");
        assert_eq!(resolved_display_name(None, ""), "file");
    }
}
