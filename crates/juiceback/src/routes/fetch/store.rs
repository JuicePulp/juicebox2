use std::sync::Arc;

use futures::StreamExt;

use super::{
    meta::clean_meta_value,
    types::{ByteSource, FetchMeta, FetchResult},
    validation::{empty_stream_message, sanitize_filename},
};
use crate::{
    cobalt::FetchOptions,
    db::{self, FileRecord},
    state::AppState,
};

/// Reserves the pending record first; deletes it on error and flips live only
/// after transfer checks.
#[expect(
    clippy::too_many_arguments,
    reason = "pipeline fns thread established context (state, ids, tokens); bundling params churns callers for no behavior gain"
)]
pub(crate) async fn download_and_store(
    state: &Arc<AppState>,
    user_id: &str,
    job_id: &str,
    source_url: &str,
    byte_source: ByteSource,
    filename: Option<&str>,
    meta: FetchMeta,
    opts: &FetchOptions,
    max_size: u64,
    ttl_hours: f64,
    encrypted_ip: Option<String>,
    password: Option<String>,
) -> FetchResult {
    let transcode_flac = opts.audio_only && opts.audio_format == "flac";
    let filename = if transcode_flac {
        with_extension(sanitize_filename(filename, opts.audio_only), "flac")
    } else {
        sanitize_filename(filename, opts.audio_only)
    };
    let fallback_mime: &str = if transcode_flac {
        "audio/flac"
    } else if opts.audio_only {
        "audio/mpeg"
    } else {
        "video/mp4"
    };
    let mime_type = if transcode_flac {
        "audio/flac".to_string()
    } else {
        mime_guess::from_path(&filename)
            .first()
            .map(|m| m.to_string())
            .unwrap_or_else(|| fallback_mime.to_string())
    };

    let file_id = nanoid::nanoid!(8);
    let delete_token = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp();
    let expires_at = now + (ttl_hours * crate::constants::SECONDS_PER_HOUR_F64).round() as i64;

    let mut record = FileRecord::new(
        file_id.clone(),
        filename.clone(),
        mime_type.clone(),
        0,
        delete_token.clone(),
        now,
        expires_at,
        encrypted_ip,
        state.config.default_storage_host.clone(),
    );
    record.title = meta
        .title
        .as_deref()
        .and_then(clean_meta_value)
        .unwrap_or_default();
    record.author = meta
        .author
        .as_deref()
        .and_then(clean_meta_value)
        .unwrap_or_default();

    let setup = match password.filter(|p| !p.is_empty()) {
        Some(password) => Some(
            crate::routes::upload::protected::prepare_protection(state, &password)
                .await
                .map_err(|e| format!("protection setup failed: {e:?}"))?,
        ),
        None => None,
    };

    if let Some(ref setup) = setup {
        record.password_hash = Some(setup.password_hash.clone());
        record.is_encrypted = true;
        record.dek_wrapped = Some(setup.wrapped_b64.clone());
        record.dek_salt = Some(setup.salt_b64.clone());
        record.dek_escrow = Some(setup.escrow_hex.clone());
        record.key_version = crate::crypto_file::KEY_VERSION_V1;
    }

    if let Err(e) = db::insert_pending_file(state, record.clone()).await {
        return Err(format!("failed to reserve file slot: {e:?}"));
    }

    let mut cleanup = CleanupOnDrop {
        state: Arc::clone(state),
        file_id: file_id.clone(),
        delete_token: delete_token.clone(),
        host: state.config.default_storage_host.clone(),
        active: true,
    };

    let (byte_source, _flac_guard) = if transcode_flac {
        let input = spool_for_transcode(state, job_id, source_url, byte_source, max_size).await?;
        let _ = state
            .db_call("update_fetch_job_progress", {
                let job_id = job_id.to_string();
                move |db| {
                    db::update_fetch_job_progress(db, &job_id, "converting", "processing", 0, 0)
                }
            })
            .await;
        let output = transcode_to_flac(input.path(), max_size).await?;
        (
            ByteSource::LocalFile(output.path().to_path_buf()),
            Some(output),
        )
    } else {
        (byte_source, None)
    };

    let result = transfer_to_juicehost(
        state,
        job_id,
        &file_id,
        &filename,
        &mime_type,
        &delete_token,
        source_url,
        byte_source,
        max_size,
        setup.as_ref().map(|setup| setup.dek.clone()),
    )
    .await;

    let (size_bytes, enc_header, detected) = result?;
    // The object is durable now, so stop rollback; a later finalize failure leaves
    // a pending row.
    cleanup.active = false;
    let (filename, mime_type) = reconcile_media(filename, mime_type, detected);
    let protection = setup
        .as_ref()
        .zip(enc_header.as_deref())
        .map(|(setup, header)| setup.material(Some(header.to_string())));
    finalize_stored_file(
        state,
        user_id,
        file_id,
        filename,
        mime_type,
        size_bytes,
        delete_token,
        protection,
    )
    .await
}

/// Deletes the pending row/object unless disarmed; spawned because `Drop`
/// cannot await.
struct CleanupOnDrop {
    state: Arc<AppState>,
    file_id: String,
    delete_token: String,
    host: Option<String>,
    active: bool,
}

impl Drop for CleanupOnDrop {
    fn drop(&mut self) {
        if self.active {
            let state = Arc::clone(&self.state);
            let file_id = self.file_id.clone();
            let token = self.delete_token.clone();
            let host = self.host.clone();
            tokio::spawn(async move {
                let _ = db::delete_pending_file(&state, file_id.clone()).await;
                let _ = crate::storage_client::delete_file_on_juicehost(
                    &state,
                    &file_id,
                    host.as_deref(),
                    Some(&token),
                )
                .await;
            });
        }
    }
}

fn configured_cobalt_hosts(config: &crate::config::Config) -> Vec<(String, u16)> {
    let mut out = Vec::new();
    for endpoint in &config.cobalt_endpoints {
        if let Ok(parsed) = url::Url::parse(endpoint.url.trim())
            && let Some(host) = parsed.host_str()
            && let Some(port) = parsed.port_or_known_default()
        {
            let host_port = (host.to_ascii_lowercase(), port);
            if !out.contains(&host_port) {
                out.push(host_port);
            }
        }
    }
    out
}

/// http(s) only; other schemes or unparsable input are untrusted.
fn tunnel_url_host_port(url: &str) -> Option<(String, u16)> {
    let parsed = url::Url::parse(url.trim()).ok()?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return None;
    }
    Some((
        parsed.host_str()?.to_ascii_lowercase(),
        parsed.port_or_known_default()?,
    ))
}

/// Redirects are followed by hand so every hop passes the SSRF check; cobalt
/// hosts are trusted because tunnels point at localhost.
async fn fetch_tunnel_response(
    client: &reqwest::Client,
    state: &Arc<AppState>,
    initial_url: &str,
    allow_private: bool,
) -> Result<reqwest::Response, String> {
    let trusted_cobalt = tunnel_url_host_port(initial_url)
        .map(|hp| configured_cobalt_hosts(&state.config).contains(&hp))
        .unwrap_or(false);
    let mut url = if trusted_cobalt {
        initial_url.trim().to_string()
    } else {
        crate::storage_client::check_outbound_url(initial_url, allow_private)
            .await
            .map_err(|e| format!("refusing to fetch media from unsafe URL: {e}"))?
    };
    for _ in 0..=5 {
        let resp = client
            .get(&url)
            .timeout(std::time::Duration::from_secs(
                crate::constants::FETCH_JOB_TIMEOUT_SECS,
            ))
            .send()
            .await
            .map_err(|e| format!("media download failed: {e}"))?;
        if !resp.status().is_redirection() {
            if !resp.status().is_success() {
                return Err(format!(
                    "media download failed: server returned status {}",
                    resp.status()
                ));
            }
            return Ok(resp);
        }
        let location = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| "media redirect without Location".to_string())?;
        let next = url::Url::parse(&url)
            .map_err(|_| "media redirect from invalid URL".to_string())?
            .join(location)
            .map_err(|_| "media redirect to invalid URL".to_string())?;
        url = crate::storage_client::check_outbound_url(next.as_str(), allow_private)
            .await
            .map_err(|e| format!("refusing to follow media redirect: {e}"))?;
    }
    Err("media redirect limit exceeded".into())
}

enum SpoolInput {
    Temp(tempfile::NamedTempFile),
    Existing(std::path::PathBuf),
}

impl SpoolInput {
    fn path(&self) -> &std::path::Path {
        match self {
            Self::Temp(file) => file.path(),
            Self::Existing(path) => path,
        }
    }
}

fn with_extension(filename: String, extension: &str) -> String {
    let mut path = std::path::PathBuf::from(filename);
    path.set_extension(extension);
    path.to_string_lossy().into_owned()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DetectedMedia {
    extension: String,
    mime_type: String,
}

const SNIFF_PREFIX_BYTES: usize = 512;

fn sniff_media(prefix: &[u8]) -> Option<DetectedMedia> {
    if prefix.is_empty() {
        return None;
    }
    let kind = infer::get(prefix)?;
    let mime_type = kind.mime_type();
    if !mime_type.starts_with("audio/") && !mime_type.starts_with("video/") {
        return None;
    }
    Some(DetectedMedia {
        extension: kind.extension().to_string(),
        mime_type: mime_type.to_string(),
    })
}

fn reconcile_media(
    filename: String,
    mime_type: String,
    detected: Option<DetectedMedia>,
) -> (String, String) {
    let Some(detected) = detected else {
        return (filename, mime_type);
    };
    let old_kind = mime_type.split('/').next().unwrap_or("").trim();
    let new_kind = detected.mime_type.split('/').next().unwrap_or("").trim();
    if old_kind == new_kind {
        return (filename, mime_type);
    }
    (
        with_extension(filename, &detected.extension),
        detected.mime_type,
    )
}

/// ffmpeg needs a seekable input, so remote sources are spooled first.
async fn spool_for_transcode(
    state: &Arc<AppState>,
    job_id: &str,
    source_url: &str,
    byte_source: ByteSource,
    max_size: u64,
) -> Result<SpoolInput, String> {
    match byte_source {
        ByteSource::LocalFile(path) => Ok(SpoolInput::Existing(path)),
        ByteSource::Tunnel(url) => {
            let resp =
                fetch_tunnel_response(&state.http, state, &url, state.config.allow_private_fetch)
                    .await?;
            spool_response(state, job_id, source_url, resp, max_size).await
        }
        ByteSource::BunResponse(resp) => {
            spool_response(state, job_id, source_url, resp, max_size).await
        }
    }
}

/// `Content-Length` is only a hint, so the cap is enforced per chunk.
async fn spool_response(
    state: &Arc<AppState>,
    job_id: &str,
    source_url: &str,
    resp: reqwest::Response,
    max_size: u64,
) -> Result<SpoolInput, String> {
    let total_hint = resp.content_length().map(|l| l as u64);
    if let Some(len) = total_hint {
        if len == 0 {
            return Err(empty_stream_message(source_url));
        }
        if len > max_size {
            return Err("file exceeds this server's maximum file size".into());
        }
    }

    let _ = state
        .db_call("update_fetch_job_progress", {
            let job_id = job_id.to_string();
            let total = total_hint.unwrap_or(0) as i64;
            move |db| {
                db::update_fetch_job_progress(db, &job_id, "transcode", "downloading", 0, total)
            }
        })
        .await;

    let temp = tempfile::NamedTempFile::new()
        .map_err(|e| format!("audio conversion spool failed: {e}"))?;
    let file = temp
        .reopen()
        .map_err(|e| format!("audio conversion spool failed: {e}"))?;
    let mut out = tokio::fs::File::from_std(file);
    let mut stream = resp.bytes_stream();
    let mut total: u64 = 0;
    let mut last_progress = std::time::Instant::now() - std::time::Duration::from_secs(1);

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("media download failed mid-stream: {e}"))?;
        total += chunk.len() as u64;
        if total > max_size {
            return Err("file exceeds this server's maximum file size".into());
        }

        tokio::io::AsyncWriteExt::write_all(&mut out, &chunk)
            .await
            .map_err(|e| format!("audio conversion spool failed: {e}"))?;

        if last_progress.elapsed() >= std::time::Duration::from_millis(750) {
            last_progress = std::time::Instant::now();
            let job_id_owned = job_id.to_string();
            let total_bytes = total_hint.unwrap_or(0);
            let _ = state
                .db_call("update_fetch_job_progress", move |db| {
                    db::update_fetch_job_progress(
                        db,
                        &job_id_owned,
                        "transcode",
                        "downloading",
                        total as i64,
                        total_bytes as i64,
                    )
                })
                .await;
        }
    }

    tokio::io::AsyncWriteExt::flush(&mut out)
        .await
        .map_err(|e| format!("audio conversion spool failed: {e}"))?;
    if total == 0 {
        return Err(empty_stream_message(source_url));
    }
    Ok(SpoolInput::Temp(temp))
}

async fn transcode_to_flac(
    input: &std::path::Path,
    max_size: u64,
) -> Result<tempfile::NamedTempFile, String> {
    let output = tempfile::NamedTempFile::new()
        .map_err(|e| format!("audio conversion temp file failed: {e}"))?;
    let ffmpeg = std::env::var("FFMPEG_BIN").unwrap_or_else(|_| "ffmpeg".into());
    let result = tokio::process::Command::new(&ffmpeg)
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-nostdin")
        .arg("-y")
        .arg("-i")
        .arg(input)
        .arg("-vn")
        .arg("-map_metadata")
        .arg("0")
        .arg("-c:a")
        .arg("flac")
        .arg("-f")
        .arg("flac")
        .arg(output.path())
        .output()
        .await
        .map_err(|e| format!("audio conversion failed to start: {e}"))?;

    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        let detail: String = stderr
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("unknown ffmpeg error")
            .trim()
            .chars()
            .take(300)
            .collect();
        return Err(format!("audio conversion failed: {detail}"));
    }

    let size = tokio::fs::metadata(output.path())
        .await
        .map_err(|e| format!("audio conversion output unreadable: {e}"))?
        .len();
    if size == 0 {
        return Err("audio conversion produced an empty file".into());
    }
    if size > max_size {
        return Err("converted file exceeds this server's maximum file size".into());
    }
    Ok(output)
}

async fn transfer_to_juicehost(
    state: &Arc<AppState>,
    job_id: &str,
    file_id: &str,
    filename: &str,
    mime_type: &str,
    capability: &str,
    source_url: &str,
    byte_source: ByteSource,
    max_size: u64,
    dek: Option<crate::crypto_file::FileKey>,
) -> Result<(u64, Option<String>, Option<DetectedMedia>), String> {
    use tokio::io::AsyncReadExt as _;
    let (mut stream, total_hint): (
        std::pin::Pin<Box<dyn futures::Stream<Item = Result<bytes::Bytes, String>> + Send>>,
        Option<u64>,
    ) = match byte_source {
        ByteSource::Tunnel(url) => {
            let client = state.http.clone();
            let resp =
                fetch_tunnel_response(&client, state, &url, state.config.allow_private_fetch)
                    .await?;
            let len = resp.content_length().map(|l| l as u64);
            if len == Some(0) {
                return Err(empty_stream_message(source_url));
            }
            if let Some(l) = len {
                if l > max_size {
                    return Err("file exceeds this server's maximum file size".into());
                }
            }
            (
                Box::pin(
                    resp.bytes_stream()
                        .map(|c| c.map_err(|e| format!("media download failed mid-stream: {e}"))),
                ),
                len,
            )
        }
        ByteSource::BunResponse(resp) => {
            let len = resp.content_length().map(|l| l as u64);
            if len == Some(0) {
                return Err(empty_stream_message(source_url));
            }
            if let Some(l) = len {
                if l > max_size {
                    return Err("file exceeds this server's maximum file size".into());
                }
            }
            (
                Box::pin(
                    resp.bytes_stream()
                        .map(|c| c.map_err(|e| format!("media download failed mid-stream: {e}"))),
                ),
                len,
            )
        }
        ByteSource::LocalFile(path) => {
            let meta = tokio::fs::metadata(&path)
                .await
                .map_err(|e| format!("yt-dlp output missing: {e}"))?;
            let size = meta.len();
            if size == 0 {
                return Err(empty_stream_message(source_url));
            }
            if size > max_size {
                return Err("file exceeds this server's maximum file size".into());
            }
            let file = tokio::fs::File::open(&path)
                .await
                .map_err(|e| format!("failed to open yt-dlp output: {e}"))?;
            (
                Box::pin(futures::stream::unfold(file, async move |mut file| {
                    let mut buf = bytes::BytesMut::with_capacity(256 * 1024);
                    match file.read_buf(&mut buf).await {
                        Ok(0) => None,
                        Ok(_) => Some((Ok(buf.freeze()), file)),
                        Err(e) => Some((Err(format!("failed reading yt-dlp output: {e}")), file)),
                    }
                })),
                Some(size),
            )
        }
    };

    if let Some(dek) = dek {
        return transfer_protected_to_juicehost(
            state,
            job_id,
            file_id,
            filename,
            mime_type,
            capability,
            &mut stream,
            max_size,
            source_url,
            total_hint,
            &dek,
        )
        .await;
    }

    let _ = state
        .db_call("update_fetch_job_progress", {
            let job_id = job_id.to_string();
            let total = total_hint.unwrap_or(0) as i64;
            move |db| {
                db::update_fetch_job_progress(db, &job_id, "transfer", "downloading", 0, total)
            }
        })
        .await;

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, String>>(
        crate::constants::STREAM_CHANNEL_CAPACITY,
    );
    let push_state = Arc::clone(state);
    let push_file_id = file_id.to_string();
    let push_filename = filename.to_string();
    let push_mime = mime_type.to_string();
    let push_capability = capability.to_string();
    let push_host = state.config.default_storage_host.clone();
    let push_task = tokio::spawn(async move {
        crate::storage_client::push_file_streaming(
            &push_state,
            &push_file_id,
            &push_filename,
            &push_mime,
            rx,
            push_host,
            Some(&push_capability),
            None,
        )
        .await
    });

    let mut total: u64 = 0;
    let mut prefix: Vec<u8> = Vec::with_capacity(SNIFF_PREFIX_BYTES);
    let mut last_progress = std::time::Instant::now() - std::time::Duration::from_secs(1);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("media download failed mid-stream: {e}"))?;
        if prefix.len() < SNIFF_PREFIX_BYTES {
            let take = (SNIFF_PREFIX_BYTES - prefix.len()).min(chunk.len());
            prefix.extend_from_slice(&chunk[..take]);
        }
        total += chunk.len() as u64;
        if total > max_size {
            return Err("file exceeds this server's maximum file size".into());
        }

        if last_progress.elapsed() >= std::time::Duration::from_millis(750) {
            last_progress = std::time::Instant::now();
            let job_id_owned = job_id.to_string();
            let total_bytes = total_hint.unwrap_or(0);
            let _ = state
                .db_call("update_fetch_job_progress", move |db| {
                    db::update_fetch_job_progress(
                        db,
                        &job_id_owned,
                        "transfer",
                        "downloading",
                        total as i64,
                        total_bytes as i64,
                    )
                })
                .await;
        }
        if tx.send(Ok(chunk)).await.is_err() {
            return Err("storage backend rejected the transfer".into());
        }
    }

    {
        let job_id_owned = job_id.to_string();
        let total_bytes = total_hint.unwrap_or(0);
        let _ = state
            .db_call("update_fetch_job_progress_final", move |db| {
                db::update_fetch_job_progress(
                    db,
                    &job_id_owned,
                    "transfer",
                    "downloading",
                    total as i64,
                    total_bytes as i64,
                )
            })
            .await;
    }

    if total == 0 {
        push_task.abort();
        return Err(empty_stream_message(source_url));
    }
    drop(tx);

    push_task
        .await
        .map_err(|_| "storage task panicked".to_string())??;

    Ok((total, None, sniff_media(&prefix)))
}

#[expect(
    clippy::too_many_arguments,
    reason = "pipeline fns thread established context (state, ids, tokens); bundling params churns callers for no behavior gain"
)]
/// Encrypting uploads need the plaintext length up front, so the body is
/// spooled.
async fn transfer_protected_to_juicehost(
    state: &Arc<AppState>,
    job_id: &str,
    file_id: &str,
    filename: &str,
    mime_type: &str,
    capability: &str,
    stream: &mut std::pin::Pin<
        Box<dyn futures::Stream<Item = Result<bytes::Bytes, String>> + Send>,
    >,
    max_size: u64,
    source_url: &str,
    total_hint: Option<u64>,
    dek: &crate::crypto_file::FileKey,
) -> Result<(u64, Option<String>, Option<DetectedMedia>), String> {
    let _ = state
        .db_call("update_fetch_job_progress", {
            let job_id = job_id.to_string();
            let total = total_hint.unwrap_or(0) as i64;
            move |db| {
                db::update_fetch_job_progress(db, &job_id, "transfer", "downloading", 0, total)
            }
        })
        .await;
    let (spool_tx, spool_handle) =
        crate::routes::upload::protected::spawn_spool_task(max_size as i64);
    let mut received: u64 = 0;
    let mut prefix: Vec<u8> = Vec::with_capacity(SNIFF_PREFIX_BYTES);
    let mut last_progress = std::time::Instant::now() - std::time::Duration::from_secs(1);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("media download failed mid-stream: {e}"))?;
        if prefix.len() < SNIFF_PREFIX_BYTES {
            let take = (SNIFF_PREFIX_BYTES - prefix.len()).min(chunk.len());
            prefix.extend_from_slice(&chunk[..take]);
        }
        received += chunk.len() as u64;
        if received > max_size {
            return Err("file exceeds this server's maximum file size".into());
        }
        if last_progress.elapsed() >= std::time::Duration::from_millis(750) {
            last_progress = std::time::Instant::now();
            let job_id_owned = job_id.to_string();
            let total_bytes = total_hint.unwrap_or(0);
            let _ = state
                .db_call("update_fetch_job_progress", move |db| {
                    db::update_fetch_job_progress(
                        db,
                        &job_id_owned,
                        "transfer",
                        "downloading",
                        received as i64,
                        total_bytes as i64,
                    )
                })
                .await;
        }
        if spool_tx.send(Ok(chunk)).await.is_err() {
            break;
        }
    }
    drop(spool_tx);
    let (spool_file, total) = spool_handle
        .await
        .map_err(|_| "spool task panicked".to_string())?
        .map_err(|e| format!("spool failed: {e:?}"))?;
    if total == 0 {
        return Err(empty_stream_message(source_url));
    }
    {
        let job_id_owned = job_id.to_string();
        let total_bytes = total_hint.unwrap_or(0);
        let _ = state
            .db_call("update_fetch_job_progress_final", move |db| {
                db::update_fetch_job_progress(
                    db,
                    &job_id_owned,
                    "transfer",
                    "downloading",
                    total as i64,
                    total_bytes as i64,
                )
            })
            .await;
    }
    let header = crate::routes::upload::protected::push_encrypted_file(
        state,
        file_id,
        filename,
        mime_type,
        state.config.default_storage_host.as_deref(),
        capability,
        spool_file.path(),
        total,
        dek,
    )
    .await
    .map_err(|e| format!("encrypted push failed: {e:?}"))?;
    Ok((total, Some(hex::encode(header)), sniff_media(&prefix)))
}

/// Protection/ownership failures are logged but not fatal; the bytes are
/// already durable.
async fn finalize_stored_file(
    state: &Arc<AppState>,
    user_id: &str,
    file_id: String,
    filename: String,
    mime_type: String,
    size_bytes: u64,
    delete_token: String,
    protection: Option<db::ProtectionMaterial>,
) -> FetchResult {
    let record = crate::reservation::finish(
        state,
        crate::reservation::FinishRequest {
            id: file_id,
            delete_token,
            filename,
            mime_type,
            size_bytes: size_bytes as i64,
            uploader_ip: None,
        },
        crate::reservation::FinishPolicy::Strict,
    )
    .await
    .map_err(|e| format!("failed to finalize fetched file: {e:?}"))?;

    if let Some(material) = protection {
        let rid = record.id.clone();
        if let Err(e) = state
            .db_call("set_fetch_protection", move |db| {
                db::set_protection(db, &rid, &material).map(|_| ())
            })
            .await
        {
            tracing::warn!("fetch: could not set protection: {e:?}");
        }
    }
    let owned = record.clone();
    let owner = user_id.to_string();
    if let Err(e) = state
        .db_call("own_fetched_file", move |db| {
            db::add_client_file(db, &owner, &owned)
        })
        .await
    {
        tracing::warn!("fetch: could not register ownership: {e:?}");
    }
    Ok(record.id)
}

#[cfg(test)]
mod tunnel_host_tests {
    use super::{reconcile_media, sniff_media, tunnel_url_host_port, with_extension};

    #[test]
    fn audio_bytes_override_a_video_fetch_label() {
        let detected = sniff_media(b"ID3\x03\x00\x00\x00\x00\x00\x00").expect("mp3");
        assert_eq!(detected.mime_type, "audio/mpeg");
        let (filename, mime) = reconcile_media(
            "Song.mp4".to_string(),
            "video/mp4".to_string(),
            Some(detected),
        );
        assert_eq!(filename, "Song.mp3");
        assert_eq!(mime, "audio/mpeg");
    }

    #[test]
    fn matching_media_is_left_alone() {
        let detected = sniff_media(b"\x00\x00\x00\x18ftypmp42\x00\x00\x00\x00").expect("mp4");
        assert_eq!(detected.mime_type, "video/mp4");
        let (filename, mime) = reconcile_media(
            "Movie.mp4".to_string(),
            "video/mp4".to_string(),
            Some(detected),
        );
        assert_eq!(filename, "Movie.mp4");
        assert_eq!(mime, "video/mp4");
    }

    #[test]
    fn unknown_bytes_do_not_change_metadata() {
        let (filename, mime) =
            reconcile_media("notes.txt".to_string(), "text/plain".to_string(), None);
        assert_eq!(filename, "notes.txt");
        assert_eq!(mime, "text/plain");
    }

    #[test]
    fn flac_extension_replaces_container_extension() {
        assert_eq!(with_extension("song.wav".into(), "flac"), "song.flac");
        assert_eq!(with_extension("song".into(), "flac"), "song.flac");
    }

    #[test]
    fn parses_tunnel_host_port() {
        assert_eq!(
            tunnel_url_host_port("http://127.0.0.1:7272/tunnel?id=x"),
            Some(("127.0.0.1".to_string(), 7272))
        );
        assert_eq!(
            tunnel_url_host_port("https://cobalt.example.com/stream"),
            Some(("cobalt.example.com".to_string(), 443))
        );
        assert!(tunnel_url_host_port("ftp://127.0.0.1:7272/x").is_none());
        assert!(tunnel_url_host_port("not a url").is_none());
    }
}
