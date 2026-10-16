

use axum::{
    body::Body,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};

use super::common::backend_request;
use crate::{error::not_found_html, state::AppState};

const SHELL_TEMPLATE: &str = include_str!("../templates/unlock_shell.html");
const SHELL_APP_JS: &str = include_str!("../templates/shell_app.js");
const LOGO_DATA_URI: &str = include_str!("../templates/logo_b64.txt");
const TITLE_FONT_DATA_URI: &str = include_str!("../templates/title_b64.txt");

pub(crate) enum BackendStatus {
    
    Known(serde_json::Value),
    
    Unknown,
    
    Unavailable,
}

impl BackendStatus {
    pub(crate) fn into_known(self) -> Option<serde_json::Value> {
        match self {
            Self::Known(body) => Some(body),
            _ => None,
        }
    }
}

pub(crate) async fn backend_file_status(state: &AppState, id: &str) -> BackendStatus {
    let Some(backend_url) = state.backend_url.as_ref() else {
        return BackendStatus::Unavailable;
    };
    let status_url = format!("{backend_url}/internal/file/{id}/status");
    let Ok(resp) = backend_request(state, status_url).send().await else {
        return BackendStatus::Unavailable;
    };
    if resp.status() == StatusCode::NOT_FOUND {
        return BackendStatus::Unknown;
    }
    if !resp.status().is_success() {
        return BackendStatus::Unavailable;
    }
    match resp.json::<serde_json::Value>().await {
        Ok(body) => BackendStatus::Known(body),
        Err(_) => BackendStatus::Unavailable,
    }
}

pub(crate) async fn remaining_ttl_secs(state: &AppState, id: &str) -> Option<u64> {
    if !state.file_cache_enabled {
        return None;
    }
    backend_file_status(state, id)
        .await
        .into_known()
        .and_then(|body| body.get("expires_at")?.as_i64())
        .map(ttl_from_expires_at)
}

fn ttl_from_expires_at(expires_at: i64) -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    (expires_at - now).max(0) as u64
}

#[derive(Clone, Copy)]
pub(crate) enum ShellMode {
    View,
    Download,
    Preview,
}

impl ShellMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::View => "view",
            Self::Download => "download",
            Self::Preview => "preview",
        }
    }
}

pub(crate) enum GateDecision {
    Respond(Response<Body>),
    HotServe { key: String },
    ServeLocal,
}

fn hot_r2_key(body: &serde_json::Value) -> Option<String> {
    if body.get("tier").and_then(|v| v.as_str()) != Some("hot") {
        return None;
    }
    let key = body.get("r2_key").and_then(|v| v.as_str()).unwrap_or("");
    if key.is_empty() || key.len() > 256 || key.starts_with('/') || key.contains("..") {
        return None;
    }
    if !key.starts_with("hot/") {
        return None;
    }
    if !key.split('/').all(|seg| {
        !seg.is_empty()
            && seg.len() <= 128
            && seg
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    }) {
        return None;
    }
    Some(key.to_string())
}

pub(crate) async fn gate_decision(
    state: &AppState,
    id: &str,
    mode: ShellMode,
    page_url: Option<&str>,
) -> GateDecision {
    if state.backend_url.is_none() {
        return GateDecision::ServeLocal;
    }
    match backend_file_status(state, id).await {
        BackendStatus::Known(body)
            if body.get("protected").and_then(|v| v.as_bool()) == Some(true) =>
        {
            let filename = body
                .get("filename")
                .and_then(|v| v.as_str())
                .map(hashed_filename)
                .unwrap_or_else(|| id.to_string());
            let gateway_origin = body
                .get("gateway_origin")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let key_version = body
                .get("key_version")
                .and_then(|v| v.as_i64())
                .unwrap_or(0)
                .to_string();
            let size_label = body
                .get("size_bytes")
                .and_then(|v| v.as_u64())
                .map(super::preview::human_size)
                .unwrap_or_default();
            let ciphertext_url = format!("/c/{id}");
            GateDecision::Respond(render_shell(
                &filename,
                id,
                gateway_origin,
                &ciphertext_url,
                &key_version,
                &size_label,
                mode,
                page_url,
            ))
        }
        BackendStatus::Known(body) => match hot_r2_key(&body) {
            Some(key) => GateDecision::HotServe { key },
            None => GateDecision::ServeLocal,
        },
        BackendStatus::Unknown => GateDecision::ServeLocal,
        BackendStatus::Unavailable => {
            let mut resp = not_found_html().into_response();
            resp.headers_mut().insert(
                header::CACHE_CONTROL,
                header::HeaderValue::from_static("no-store"),
            );
            GateDecision::Respond(resp)
        }
    }
}

pub(crate) async fn protected_shell_response(
    state: &AppState,
    id: &str,
    mode: ShellMode,
    page_url: Option<&str>,
) -> Option<Response<Body>> {
    match gate_decision(state, id, mode, page_url).await {
        GateDecision::Respond(resp) => Some(resp),
        GateDecision::HotServe { .. } | GateDecision::ServeLocal => None,
    }
}

fn hashed_filename(filename: &str) -> String {
    use sha2::Digest as _;
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"juicebox-filename-v1:");
    hasher.update(filename.as_bytes());
    hex::encode(hasher.finalize())[..16].to_string()
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn render_shell(
    filename: &str,
    id: &str,
    gateway_origin: &str,
    ciphertext_url: &str,
    key_version: &str,
    size_label: &str,
    mode: ShellMode,
    page_url: Option<&str>,
) -> Response<Body> {
    
    
    let content_url = format!(
        "{}/file/{}/content",
        gateway_origin.trim_end_matches('/'),
        id
    );
    
    
    let enc_meta = format!("Encrypted file - {size_label}");
    let mut og_tags = format!(
        "<meta property=\"og:title\" content=\"{}\">\n<meta property=\"og:description\" content=\"{}\">\n<meta name=\"twitter:card\" content=\"summary\">",
        escape_html(filename),
        escape_html(&enc_meta),
    );
    let mut discord_embed = String::new();
    if let Some(url) = page_url.filter(|u| !u.is_empty()) {
        og_tags.push_str(&format!(
            "\n<meta property=\"og:url\" content=\"{}\">",
            escape_html(url)
        ));
        if !filename.is_empty() {
            let name: String = filename.chars().take(80).collect();
            let heading = format!("## [{name}]({url})\n{enc_meta}");
            let payload = serde_json::json!({
                "component": {
                    "type": 17,
                    "accent_color": 16760154,
                    "components": [
                        { "type": 10, "content": heading },
                        { "type": 1, "components": [{ "type": 2, "style": 5, "url": url, "label": "Open" }] },
                    ],
                },
            });
            let serialized =
                serde_json::to_string(&payload).unwrap_or_default().replace("</", "<\\/");
            if !serialized.is_empty() {
                discord_embed = format!(
                    "<script id=\"discord:component-embed\" type=\"application/json\">\n{serialized}\n</script>"
                );
            }
        }
    }
    let html = SHELL_TEMPLATE
        .replace("__HEAD_META__", juiceutils::web::HEAD_META.trim_end())
        .replace("__OG_TAGS__", &og_tags)
        .replace("__DISCORD_EMBED__", &discord_embed)
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
        .replace("__DL_ICON_SVG__", juiceutils::icons::DOWNLOAD_ART)
        .replace("__APP_JS__", SHELL_APP_JS.trim_end())
        .replace("__PREVIEW_CSS__", juiceutils::web::PREVIEW_CSS.trim_end())
        .replace("__FILE_ID__", &escape_html(id))
        .replace("__FILENAME__", &escape_html(filename))
        .replace("__SIZE__", &escape_html(size_label))
        .replace("__GATEWAY_ORIGIN__", &escape_html(gateway_origin))
        .replace("__CIPHERTEXT_URL__", &escape_html(ciphertext_url))
        .replace("__CONTENT_URL__", &escape_html(&content_url))
        .replace("__MODE__", mode.as_str())
        .replace("__KEY_VERSION__", &escape_html(key_version))
        .replace("__MIME__", "application/octet-stream");
    
    
    
    
    
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .header(
            header::CONTENT_SECURITY_POLICY,
            "default-src 'none'; script-src 'unsafe-inline' https://static.cloudflareinsights.com; style-src 'unsafe-inline'; font-src data:; img-src data: blob:; media-src blob:; frame-src blob:; connect-src http: https:; frame-ancestors 'none'; base-uri 'none'; form-action 'none'",
        )
        .body(Body::from(html))
        .unwrap_or_else(|_| not_found_html().into_response())
}
