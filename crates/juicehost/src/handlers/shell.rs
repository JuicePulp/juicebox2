//! Password-gate shells for public routes (`/f/`, `/d/`, `/v/`).
//!
//! Protected files store ciphertext only. Instead of redirecting to
//! juiceback, the host serves an unlock shell: a password form plus a
//! self-contained decrypt app. The browser POSTs the password directly to
//! the juiceback key-release gateway and decrypts `/c/` ciphertext locally
//! - juicehost never sees passwords, keys, or plaintext.

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

/// Backend file-status probe outcome.
pub(crate) enum BackendStatus {
    /// Backend answered with a status document.
    Known(serde_json::Value),
    /// Backend answered but knows nothing about this id (legacy/direct file).
    Unknown,
    /// No backend configured, or the probe failed. Callers fail closed.
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

/// Seconds until `expires_at` (unix), saturating at zero.
fn ttl_from_expires_at(expires_at: i64) -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    (expires_at - now).max(0) as u64
}

/// Which public route asked for the shell (drives the post-unlock action).
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

/// Shell check for public byte/page routes.
///
/// Returns `Some(response)` when the request must not proceed to bytes: the
/// unlock shell for protected files, or a 404 when protection cannot be
/// ruled out (unreachable backend). Returns `None` when serving may
/// proceed - including standalone hosts without a backend (nothing can be
/// protected there) and ids the backend does not know (legacy files).
pub(crate) async fn protected_shell_response(
    state: &AppState,
    id: &str,
    mode: ShellMode,
) -> Option<Response<Body>> {
    if state.backend_url.is_none() {
        return None;
    }
    match backend_file_status(state, id).await {
        BackendStatus::Known(body)
            if body.get("protected").and_then(|v| v.as_bool()) == Some(true) =>
        {
            let filename = body.get("filename").and_then(|v| v.as_str()).unwrap_or(id);
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
            Some(render_shell(
                filename,
                id,
                gateway_origin,
                &key_version,
                &size_label,
                mode,
            ))
        }
        BackendStatus::Known(_) | BackendStatus::Unknown => None,
        // Fail closed: an unreachable backend must not leak bytes that
        // might be ciphertext for a protected file.
        BackendStatus::Unavailable => {
            let mut resp = not_found_html().into_response();
            resp.headers_mut().insert(
                header::CACHE_CONTROL,
                header::HeaderValue::from_static("no-store"),
            );
            Some(resp)
        }
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

fn render_shell(
    filename: &str,
    id: &str,
    gateway_origin: &str,
    key_version: &str,
    size_label: &str,
    mode: ShellMode,
) -> Response<Body> {
    let extension = filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let mime = crate::storage::guess_mime(&extension);
    let content_url = format!(
        "{}/file/{}/content",
        gateway_origin.trim_end_matches('/'),
        id
    );
    let html = SHELL_TEMPLATE
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
        .replace("__APP_JS__", SHELL_APP_JS.trim_end())
        .replace("__PREVIEW_CSS__", juiceutils::web::PREVIEW_CSS.trim_end())
        .replace("__FILE_ID__", &escape_html(id))
        .replace("__FILENAME__", &escape_html(filename))
        .replace("__SIZE__", &escape_html(size_label))
        .replace("__GATEWAY_ORIGIN__", &escape_html(gateway_origin))
        .replace("__CIPHERTEXT_URL__", &escape_html(&format!("/c/{id}")))
        .replace("__CONTENT_URL__", &escape_html(&content_url))
        .replace("__MODE__", mode.as_str())
        .replace("__KEY_VERSION__", &escape_html(key_version))
        .replace("__MIME__", &escape_html(&mime));
    // Own CSP: the shell is a self-contained app (inline script + styles,
    // data:/blob: media, cross-origin gateway fetch). Blob subresources
    // (decrypted media, PDF frames) are explicitly allowed; top-level blob
    // navigation is avoided entirely. The middleware keeps handler-set
    // policies, so byte responses stay scriptless while the shell runs.
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .header(
            header::CONTENT_SECURITY_POLICY,
            "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; font-src data:; img-src data: blob:; media-src blob:; frame-src blob:; connect-src http: https:; frame-ancestors 'none'; base-uri 'none'; form-action 'none'",
        )
        .body(Body::from(html))
        .unwrap_or_else(|_| not_found_html().into_response())
}
