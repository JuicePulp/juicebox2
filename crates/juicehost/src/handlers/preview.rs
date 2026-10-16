

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
const ZIP_APP_JS: &str = include_str!("../templates/zip_app.js");
const LOGO_DATA_URI: &str = include_str!("../templates/logo_b64.txt");
const TITLE_FONT_DATA_URI: &str = include_str!("../templates/title_b64.txt");

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
    
    
    zip_html: Option<String>,
    
    
    text_head: Option<(String, u64, String)>,
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

fn component_embed_payload(
    page: &PreviewPage,
    abs_raw: &str,
    abs_page: &str,
    abs_download: &str,
) -> serde_json::Value {
    let name = truncate_chars(&page.filename, 80);
    let meta = truncate_chars(&format!("{} - {}", kind_label(page.kind), page.size_label), 160);
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
        container.push(serde_json::json!({
            "type": 12,
            "items": [{ "media": { "url": abs_raw }, "description": name }],
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
        let meta = truncate_chars(&format!("{} - {}", page.mime, page.size_label), 80);
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
    let description = format!("{} - {}", page.mime, page.size_label);
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
    
    
    let valid = match std::str::from_utf8(&bytes) {
        Ok(_) => bytes.len(),
        Err(e) => e.valid_up_to(),
    };
    if valid == 0 {
        return None;
    }
    let raw = String::from_utf8_lossy(&bytes[..valid]);
    let snippet: String = raw.chars().take(800).collect();
    Some((escape_html(&raw), valid as u64, snippet.replace("```", "'''")))
}

fn download_hero(page: &PreviewPage) -> String {
    format!(
        "<div class=\"dl-hero\"><div class=\"dl-icon\" aria-hidden=\"true\">{}</div><p class=\"dl-name\">{name}</p><p class=\"dl-meta\">{mime} - {size}</p><a class=\"dl-btn\" href=\"{dl}\">Download</a></div>",
        juiceutils::icons::DOWNLOAD_ART,
        name = escape_html(&page.filename),
        mime = escape_html(&page.mime),
        size = escape_html(&page.size_label),
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
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" | "bmp" | "ico"
                | "svg" => "image",
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

fn stage_html(page: &PreviewPage) -> String {
    let raw = escape_html(&page.raw_url);
    match page.kind {
        PreviewKind::Video => format!(
            "<video id=\"media\" controls playsinline preload=\"metadata\" src=\"{raw}\" aria-label=\"{name}\"></video>",
            name = escape_html(&page.filename),
        ),
        PreviewKind::Audio => format!(
            "<div class=\"audio-hero\" aria-hidden=\"true\">{}</div><audio id=\"media\" controls preload=\"metadata\" src=\"{raw}\"></audio>",
            juiceutils::icons::AUDIO,
        ),
        PreviewKind::Image => format!(
            "<img id=\"media\" src=\"{raw}\" alt=\"{name}\" loading=\"eager\">",
            name = escape_html(&page.filename),
        ),
        PreviewKind::Pdf => format!(
            "<iframe id=\"media\" class=\"pdf-frame\" src=\"{raw}\" title=\"{name}\"></iframe><noscript><p class=\"fallback-note\"><a href=\"{raw}\">Open the PDF</a></p></noscript>",
            name = escape_html(&page.filename),
        ),
        PreviewKind::Text => {
            let (content, loaded, _) = page.text_head.clone().unwrap_or_else(|| {
                (
                    String::from("Loading preview..."),
                    0,
                    String::new(),
                )
            });
            format!(
                "<pre id=\"textview\" class=\"text-view\" data-raw=\"{raw}\" data-loaded=\"{loaded}\" aria-live=\"polite\">{content}</pre><noscript><p class=\"fallback-note\"><a href=\"{raw}\">Open the raw file</a> (text preview needs JavaScript).</p></noscript>",
            )
        }
        PreviewKind::Download => download_hero(page),
        PreviewKind::Zip => page.zip_html.clone().unwrap_or_else(|| download_hero(page)),
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
            return Ok(not_found_html().into_response());
        }
        Err(_) => return Err(JuicehostError::Internal),
    };

    let abs_self = request_base_url(&headers)
        .map(|base| absolute_for(Some(base.as_str()), &format!("/v/{page_path}")));

    if let Some(shelled) =
        protected_shell_response(&state, id, ShellMode::Preview, abs_self.as_deref()).await
    {
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
    let title = format!("{filename} - Juicebox²");
    let kind = preview_kind(&mime);
    
    
    let zip_html = if kind == PreviewKind::Zip {
        super::zip_list::load_archive_entries(&state.storage, id, &filename, meta.size)
            .await
            .map(|listing| {
                zip_listing_html(
                    id,
                    &download_url,
                    &listing.entries,
                    listing.locked,
                )
            })
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
        size_label: human_size(meta.size),
        raw_url,
        download_url,
        page_url,
        mime,
        extension,
        zip_html,
        text_head,
    };
    let response = Response::builder()
        .status(axum::http::StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, NO_STORE)
        .body(Body::from(render_preview(&page, &headers)))
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
        assert_eq!(preview_kind("application/x-7z-compressed"), PreviewKind::Zip);
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
            title: "a.mp4 - Juicebox²".into(),
            filename: "a.mp4".into(),
            size_label: "1.0 MB".into(),
            raw_url: "/f/a.mp4".into(),
            download_url: "/d/a.mp4".into(),
            page_url: "/v/a.mp4".into(),
            mime: "video/mp4".into(),
            extension: "mp4".into(),
            zip_html: None,
            text_head: None,
        };
        let html = render_preview(&page, &HeaderMap::new());
        assert!(html.contains("<video"));
        assert!(html.contains("/f/a.mp4"));
        assert!(!html.contains("preview-card"));
        assert!(html.contains("discord:component-embed"));
        assert!(html.contains("twitter:card"));
    }
}
