//! Password-gated downloads for at-rest encrypted files.
//!
//! juicehost stores ciphertext only and redirects protected public links
//! here. juiceback verifies the password (Argon2id, rate-limited), then
//! fetches ciphertext over the API-key-authenticated internal endpoint and
//! decrypts locally — streaming, never buffering a whole file.
//!
//! The cleartext password is accepted in memory only: it is never logged,
//! never persisted, and never sent to juicehost.

use std::{net::SocketAddr, sync::Arc};

use axum::{
    Json,
    body::{Body, Bytes},
    extract::{ConnectInfo, Path, Query, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
};
use futures::StreamExt;
use serde::{Deserialize, Serialize};

use crate::{
    crypto_file::{self, FileKey},
    db,
    error::AppError,
    state::AppState,
};

pub const UNLOCK_ISSUER: &str = "juiceback-unlock";
const UNLOCK_TTL_SECS: usize = 300;
const FORBIDDEN_BODY: &str = "invalid password";

/// cuelume interaction sounds, bundled at compile time (the unlock page has
/// no static-file server, so the bundle is inlined). Regenerate per
/// `static/README.md` when upgrading cuelume.
const CUELUME_BUNDLE: &str = include_str!("../../static/cuelume.bundle.js");

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UnlockClaims {
    sub: String,
    iss: String,
    exp: usize,
    iat: usize,
}

fn unlock_cookie_name(id: &str) -> String {
    format!("jb_unlock_{id}")
}

fn mint_unlock_token(
    jwt_secret: &str,
    file_id: &str,
    now: i64,
) -> Result<String, AppError> {
    use jsonwebtoken::{EncodingKey, Header, encode};
    let claims = UnlockClaims {
        sub: file_id.to_string(),
        iss: UNLOCK_ISSUER.to_string(),
        iat: now as usize,
        exp: now as usize + UNLOCK_TTL_SECS,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(jwt_secret.as_bytes()),
    )
    .map_err(|e| AppError::Internal(format!("unlock token failed: {e}")))
}

fn valid_unlock_cookie(headers: &HeaderMap, file_id: &str, jwt_secret: &str) -> bool {
    use jsonwebtoken::{DecodingKey, Validation, Algorithm, decode};
    let name = unlock_cookie_name(file_id);
    let Some(token) = crate::auth::cookie_value(headers, &name) else {
        return false;
    };
    let mut validation = Validation::new(Algorithm::HS256);
    validation.set_issuer(&[UNLOCK_ISSUER]);
    match decode::<UnlockClaims>(
        &token,
        &DecodingKey::from_secret(jwt_secret.as_bytes()),
        &validation,
    ) {
        Ok(data) => data.claims.sub == file_id,
        Err(_) => false,
    }
}

fn set_unlock_cookie(
    headers: &mut HeaderMap,
    file_id: &str,
    token: &str,
    secure: bool,
) -> Result<(), AppError> {
    let mut value = format!(
        "{}={}; Path=/; Max-Age={}; HttpOnly; SameSite=Strict",
        unlock_cookie_name(file_id),
        token,
        UNLOCK_TTL_SECS
    );
    if secure {
        value.push_str("; Secure");
    }
    headers.insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&value)
            .map_err(|_| AppError::Internal("cookie encoding failed".into()))?,
    );
    Ok(())
}

fn escape_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

async fn protected_record(
    state: &Arc<AppState>,
    id: &str,
) -> Result<db::FileRecord, AppError> {
    let lookup = id.to_string();
    let record = state
        .db_call("get_unlock_file", move |db| db::get_file(db, &lookup))
        .await?
        .ok_or(AppError::NotFound)?;
    if !record.is_protected() {
        return Err(AppError::NotFound);
    }
    Ok(record)
}

fn public_url_for(state: &Arc<AppState>, record: &db::FileRecord) -> String {
    crate::utils::public_url(
        &state.config.public_base_url,
        record.storage_host.as_deref(),
        &record.id,
        &record.filename,
    )
}

fn unlock_page_html(id: &str, filename: &str) -> String {
    let safe_name = escape_html(filename);
    let page = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Unlock {safe_name} - Juicebox</title></head>
<body>
<main>
<h1>This file is password-protected</h1>
<p>Enter the password for <strong>{safe_name}</strong> to download it.</p>
<form method="post" action="/file/{id}/unlock">
<label for="password">Password</label>
<input id="password" name="password" type="password" data-cuelume-type autocomplete="current-password" required>
<button type="submit" data-cuelume-tap>Unlock</button>
</form>
<p>Fetching with curl? Append <code>?password=...</code> to
<code>/file/{id}/content</code>, or send <code>X-File-Password</code>.</p>
</main>
<script>__CUELUME_BUNDLE__</script>
<script>
try {{ window.Cuelume && Cuelume.bind(); }} catch (e) {{}}
function sfx(name, opts) {{
  try {{ window.Cuelume && Cuelume.play(name, opts); }} catch (e) {{}}
}}
document.querySelector("form").addEventListener("submit", async (e) => {{
  e.preventDefault();
  const password = document.getElementById("password").value;
  const res = await fetch("/file/{id}/unlock", {{
    method: "POST",
    headers: {{ "Content-Type": "application/json" }},
    body: JSON.stringify({{ password }}),
  }});
  if (res.ok) {{
    sfx("success", {{ emphasis: "subtle" }});
    location.href = "/file/{id}/content";
  }} else {{
    sfx("error", {{ emphasis: "subtle" }});
    alert("Invalid password");
  }}
}});
</script>
</body>
</html>"#
    );
    page.replace("__CUELUME_BUNDLE__", CUELUME_BUNDLE)
}

#[utoipa::path(
    get,
    path = "/file/{id}/unlock",
    params(("id" = String, Path, description = "File ID")),
    responses(
        (status = 200, description = "Password prompt"),
        (status = 302, description = "Unprotected file redirects to its public URL"),
        (status = 404, description = "File not found"),
    ),
    tag = "Files",
)]
#[tracing::instrument(skip_all)]
pub async fn unlock_page_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let lookup = id.clone();
    let record = state
        .db_call("get_unlock_page", move |db| db::get_file(db, &lookup))
        .await?
        .ok_or(AppError::NotFound)?;
    if !record.is_protected() {
        return Ok(Redirect::to(&public_url_for(&state, &record)).into_response());
    }
    Ok(Html(unlock_page_html(&record.id, &record.filename)).into_response())
}

#[derive(Deserialize)]
pub struct UnlockQuery {
    password: Option<String>,
}

/// Extract a candidate password from query, `X-File-Password`, or HTTP Basic
/// (password part). Returns `None` when no credential was supplied.
fn request_password(headers: &HeaderMap, query: &UnlockQuery) -> Option<String> {
    if let Some(pw) = query.password.as_deref().filter(|p| !p.is_empty()) {
        return Some(pw.to_string());
    }
    if let Some(pw) = headers
        .get("x-file-password")
        .and_then(|v| v.to_str().ok())
        .filter(|p| !p.is_empty())
    {
        return Some(pw.to_string());
    }
    let auth = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let b64 = auth.strip_prefix("Basic ")?.trim();
    let decoded = base64_decode(b64)?;
    let (_, pass) = decoded.split_once(':')?;
    (!pass.is_empty()).then(|| pass.to_string())
}

fn base64_decode(input: &str) -> Option<String> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(input.as_bytes())
        .ok()?;
    String::from_utf8(bytes).ok()
}

async fn verify_password_blocking(password: String, hash: String) -> Result<bool, AppError> {
    tokio::task::spawn_blocking(move || crate::auth::verify_password(&password, &hash))
        .await
        .map_err(|_| AppError::Internal("password check panicked".into()))?
        .map_err(AppError::Internal)
}

/// Check unlock credentials: valid cookie, or a verifiable password from
/// query/header. Returns `true` when the download may proceed. Wrong and
/// missing credentials are indistinguishable to callers.
async fn credentials_ok(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    query: &UnlockQuery,
    record: &db::FileRecord,
) -> Result<bool, AppError> {
    if valid_unlock_cookie(headers, &record.id, &state.config.jwt_secret) {
        return Ok(true);
    }
    let Some(password) = request_password(headers, query) else {
        return Ok(false);
    };
    let hash = record.password_hash.clone().unwrap_or_default();
    if hash.is_empty() {
        return Ok(false);
    }
    Ok(verify_password_blocking(password, hash).await?)
}

#[tracing::instrument(skip_all)]
pub async fn unlock_submit_handler(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<UnlockQuery>,
    body: axum::body::Bytes,
) -> Result<Response, AppError> {
    let record = protected_record(&state, &id).await?;
    // POST bodies carry the password either as JSON (JS) or urlencoded (no-JS).
    let mut password = query.password.clone().filter(|p| !p.is_empty());
    if password.is_none() && !body.is_empty() {
        let ctype = headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if ctype.contains("application/json") {
            password = serde_json::from_slice::<UnlockQuery>(&body)
                .ok()
                .and_then(|q| q.password)
                .filter(|p| !p.is_empty());
        } else {
            let text = String::from_utf8_lossy(&body);
            for pair in text.split('&') {
                if let Some((k, v)) = pair.split_once('=')
                    && k.trim() == "password"
                {
                    let decoded: String = url::form_urlencoded::parse(v.as_bytes())
                        .map(|(key, val)| format!("{key}{val}"))
                        .collect();
                    if !decoded.is_empty() {
                        password = Some(decoded);
                    }
                    break;
                }
            }
        }
    }

    let ip = crate::utils::client_ip(&headers, addr.ip(), &state).to_string();
    if !state.unlock_limiter.allow(&ip) {
        return Err(AppError::TooManyRequests(
            "Too many unlock attempts. Please wait and try again.".into(),
        ));
    }

    let ok = match password {
        Some(password) => {
            let hash = record.password_hash.clone().unwrap_or_default();
            verify_password_blocking(password, hash).await?
        }
        None => false,
    };
    if !ok {
        return Err(AppError::Forbidden(FORBIDDEN_BODY.into()));
    }

    let now = chrono::Utc::now().timestamp();
    let token = mint_unlock_token(&state.config.jwt_secret, &record.id, now)?;
    let wants_html = crate::routes::noscript::wants_html(&headers);
    if wants_html {
        let mut headers_out = HeaderMap::new();
        set_unlock_cookie(&mut headers_out, &record.id, &token, state.config.secure_cookies)?;
        let redirect = Redirect::to(&format!("/file/{}/content", record.id)).into_response();
        let (mut parts, body) = redirect.into_parts();
        parts.headers.extend(headers_out);
        return Ok(Response::from_parts(parts, body));
    }

    let mut resp = Json(serde_json::json!({ "ok": true })).into_response();
    set_unlock_cookie(
        resp.headers_mut(),
        &record.id,
        &token,
        state.config.secure_cookies,
    )?;
    Ok(resp)
}

/// Parse `Range: bytes=S-`, `bytes=S-E`, or `bytes=-N` against `total`,
/// returning `(start, end_exclusive)`. `None` means no usable header.
fn parse_range(value: &str, total: u64) -> Result<Option<(u64, u64)>, AppError> {
    let spec = value.trim().strip_prefix("bytes=").unwrap_or("");
    if spec.is_empty() {
        return Ok(None);
    }
    if total == 0 {
        return Err(AppError::RangeNotSatisfiable("empty file".into()));
    }
    if let Some(suffix) = spec.strip_prefix('-') {
        let n: u64 = suffix
            .parse()
            .map_err(|_| AppError::RangeNotSatisfiable("bad range".into()))?;
        if n == 0 {
            return Err(AppError::RangeNotSatisfiable("bad range".into()));
        }
        let start = total.saturating_sub(n);
        return Ok(Some((start, total)));
    }
    let (start_s, end_s) = spec.split_once('-').unwrap_or((spec, ""));
    let start: u64 = start_s
        .parse()
        .map_err(|_| AppError::RangeNotSatisfiable("bad range".into()))?;
    if start >= total {
        return Err(AppError::RangeNotSatisfiable("range past end".into()));
    }
    let end = if end_s.is_empty() {
        total
    } else {
        let last: u64 = end_s
            .parse()
            .map_err(|_| AppError::RangeNotSatisfiable("bad range".into()))?;
        (last + 1).min(total)
    };
    if end <= start {
        return Err(AppError::RangeNotSatisfiable("bad range".into()));
    }
    Ok(Some((start, end)))
}

/// Decrypt a ciphertext byte stream chunk-by-chunk, yielding only plaintext
/// `[plain_start, plain_end)`.
///
/// `cipher_skip` leading cipher bytes are dropped first (used when juicehost
/// ignored our Range and answered 200); afterwards the stream must begin
/// exactly at stored chunk `first_idx`. Chunk boundaries after that are
/// deterministic, so framing needs no lookahead beyond one stored chunk.
fn decrypt_cipher_stream(
    key: FileKey,
    cipher: impl futures::Stream<Item = Result<Bytes, String>> + Send + Unpin + 'static,
    plain_start: u64,
    plain_end: u64,
    plain_len: u64,
    first_idx: u64,
    mut cipher_skip: u64,
) -> impl futures::Stream<Item = Result<Bytes, String>> + Send {
    async_stream::stream! {
        let chunk = crate::crypto_file::PLAINTEXT_CHUNK_LEN as u64;
        let mut buf: Vec<u8> = Vec::new();
        let mut cipher = cipher;
        while cipher_skip > 0 {
            match cipher.next().await {
                Some(Ok(bytes)) => {
                    if (bytes.len() as u64) <= cipher_skip {
                        cipher_skip -= bytes.len() as u64;
                    } else {
                        buf.extend_from_slice(&bytes[(cipher_skip as usize)..]);
                        cipher_skip = 0;
                    }
                }
                Some(Err(e)) => { yield Err(e); return; }
                None => { yield Err("ciphertext truncated".to_string()); return; }
            }
        }
        let mut plain_off = first_idx * chunk;
        while plain_off < plain_end {
            let want = (plain_len - plain_off).min(chunk) as usize;
            let take = crate::crypto_file::NONCE_LEN + want + crate::crypto_file::TAG_LEN;
            while buf.len() < take {
                match cipher.next().await {
                    Some(Ok(bytes)) => buf.extend_from_slice(&bytes),
                    Some(Err(e)) => { yield Err(e); return; }
                    None => { yield Err("ciphertext truncated".to_string()); return; }
                }
            }
            let stored: Vec<u8> = buf.drain(..take).collect();
            let plain = match crate::crypto_file::decrypt_chunk(&key, &stored) {
                Ok(plain) => plain,
                Err(e) => { yield Err(format!("decryption failed: {e}")); return; }
            };
            let from = plain_start.saturating_sub(plain_off) as usize;
            let to = ((plain_end - plain_off).min(want as u64)) as usize;
            if from < to {
                yield Ok(Bytes::from(plain[from..to].to_vec()));
            }
            plain_off += want as u64;
        }
    }
}

#[utoipa::path(
    get,
    path = "/file/{id}/content",
    params(("id" = String, Path, description = "File ID")),
    responses(
        (status = 200, description = "Decrypted file bytes"),
        (status = 206, description = "Decrypted byte range"),
        (status = 302, description = "Unprotected file redirects to its public URL"),
        (status = 403, description = "Missing or wrong password"),
        (status = 404, description = "File not found"),
        (status = 416, description = "Range not satisfiable"),
    ),
    tag = "Files",
)]
#[tracing::instrument(skip_all)]
pub async fn content_handler(
    State(state): State<Arc<AppState>>,
    method: Method,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<UnlockQuery>,
) -> Result<Response, AppError> {
    let lookup = id.clone();
    let record = state
        .db_call("get_content_file", move |db| db::get_file(db, &lookup))
        .await?
        .ok_or(AppError::NotFound)?;
    if !record.is_protected() {
        return Ok(Redirect::to(&public_url_for(&state, &record)).into_response());
    }
    if !credentials_ok(&state, &headers, &query, &record).await? {
        return Err(AppError::Forbidden(FORBIDDEN_BODY.into()));
    }

    let plain_len: u64 = u64::try_from(record.size_bytes.max(0))
        .map_err(|_| AppError::Internal("bad file size".into()))?;

    let (start, end) = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(|v| parse_range(v, plain_len))
        .transpose()?
        .flatten()
        .unwrap_or((0, plain_len));

    serve_protected_bytes(&state, &record, start, end, &method).await
}

/// Fetch ciphertext for `record`, decrypt the `[start, end)` plaintext span,
/// and build the 200/206 response. Shared by the gated download and the
/// admin decrypt preview (which bypasses the gate with the stored key).
///
/// # Errors
///
/// Returns [`AppError::RangeNotSatisfiable`] for out-of-bounds spans,
/// [`AppError::JuicehostUnreachable`] when the backend fetch fails, and
/// [`AppError::Internal`] when the storage key is unavailable.
pub(crate) async fn serve_protected_bytes(
    state: &Arc<AppState>,
    record: &db::FileRecord,
    start: u64,
    end: u64,
    method: &Method,
) -> Result<Response, AppError> {
    let plain_len: u64 = u64::try_from(record.size_bytes.max(0))
        .map_err(|_| AppError::Internal("bad file size".into()))?;
    if start > end || end > plain_len {
        return Err(AppError::RangeNotSatisfiable("bad range".into()));
    }
    let enc_header_hex = record.enc_header.clone().unwrap_or_default();
    let partial = start != 0 || end != plain_len;

    let key = state
        .config
        .storage_file_key()
        .map_err(|_| AppError::Internal("storage encryption unavailable".into()))?;
    let (cipher_start, cipher_len) =
        crypto_file::cipher_range_for_plain(start, end, plain_len)
            .map_err(|_| AppError::RangeNotSatisfiable("bad range".into()))?;
    let (cipher_stream, ranged) = crate::storage_client::download_ciphertext(
        &state,
        &record.id,
        record.storage_host.as_deref(),
        Some(&record.delete_token),
        Some((cipher_start, cipher_len)),
    )
    .await
    .map_err(AppError::JuicehostUnreachable)?;

    let plain_stream = decrypt_cipher_stream(
        key,
        cipher_stream,
        start,
        end,
        plain_len,
        start / crate::crypto_file::PLAINTEXT_CHUNK_LEN as u64,
        if ranged { 0 } else { cipher_start },
    );

    let mime = record
        .mime_type
        .parse::<HeaderValue>()
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
    let mut builder = Response::builder()
        .header(header::CONTENT_TYPE, mime)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CACHE_CONTROL, "no-store")
        .header(
            header::CONTENT_LENGTH,
            (end - start).to_string(),
        );
    if !enc_header_hex.is_empty() {
        builder = builder.header(
            header::ETAG,
            format!("\"{enc_header_hex}\""),
        );
    }
    if partial {
        builder = builder
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_RANGE, format!("bytes {start}-{}/{plain_len}", end - 1));
    }
    if method == Method::HEAD {
        return builder
            .body(Body::empty())
            .map_err(|_| AppError::Internal("response build failed".into()));
    }
    builder
        .body(Body::from_stream(plain_stream))
        .map_err(|_| AppError::Internal("response build failed".into()))
}
