use std::sync::Arc;

use futures::StreamExt;

use super::{
    types::{ByteSource, FetchResult},
    validation::{empty_stream_message, sanitize_filename},
};
use crate::{
    cobalt::FetchOptions,
    db::{self, FileRecord},
    state::AppState,
    upload_mode::UploadMode,
};

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
    opts: &FetchOptions,
    max_size: u64,
    ttl_hours: f64,
    encrypted_ip: Option<String>,
) -> FetchResult {
    let filename = sanitize_filename(filename, opts.audio_only);
    let fallback_mime: &str = if opts.audio_only {
        "audio/mpeg"
    } else {
        "video/mp4"
    };
    let mime_type = mime_guess::from_path(&filename)
        .first()
        .map(|m| m.to_string())
        .unwrap_or_else(|| fallback_mime.to_string());

    let file_id = nanoid::nanoid!(8);
    let delete_token = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp();
    let expires_at = now + (ttl_hours * crate::constants::SECONDS_PER_HOUR_F64).round() as i64;

    let record = FileRecord::new(
        file_id.clone(),
        filename.clone(),
        mime_type.clone(),
        0,
        delete_token.clone(),
        now,
        expires_at,
        encrypted_ip,
        None,
    );

    if let Err(e) = db::insert_pending_file(state, record.clone()).await {
        return Err(format!("failed to reserve file slot: {e:?}"));
    }

    let mut cleanup = CleanupOnDrop {
        state: Arc::clone(state),
        file_id: file_id.clone(),
        delete_token: delete_token.clone(),
        active: true,
    };

    let result = transfer_to_juicehost(
        state,
        job_id,
        &file_id,
        &filename,
        &mime_type,
        &delete_token,
        source_url,
        &byte_source,
        max_size,
    )
    .await;

    let size_bytes = result?;
    cleanup.active = false;
    finalize_stored_file(
        state,
        user_id,
        file_id,
        filename,
        mime_type,
        size_bytes,
        delete_token,
    )
    .await
}

struct CleanupOnDrop {
    state: Arc<AppState>,
    file_id: String,
    delete_token: String,
    active: bool,
}

impl Drop for CleanupOnDrop {
    fn drop(&mut self) {
        if self.active {
            let state = Arc::clone(&self.state);
            let file_id = self.file_id.clone();
            let token = self.delete_token.clone();
            tokio::spawn(async move {
                let _ = db::delete_pending_file(&state, file_id.clone()).await;
                let _ = crate::storage_client::delete_file_on_juicehost(
                    &state,
                    &file_id,
                    None,
                    Some(&token),
                )
                .await;
            });
        }
    }
}

async fn fetch_tunnel_response(
    client: &reqwest::Client,
    initial_url: &str,
    allow_private: bool,
) -> Result<reqwest::Response, String> {
    let mut url = crate::storage_client::check_outbound_url(initial_url, allow_private)
        .await
        .map_err(|e| format!("refusing to fetch media from unsafe URL: {e}"))?;
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

async fn transfer_to_juicehost(
    state: &Arc<AppState>,
    job_id: &str,
    file_id: &str,
    filename: &str,
    mime_type: &str,
    capability: &str,
    source_url: &str,
    byte_source: &ByteSource,
    max_size: u64,
) -> Result<u64, String> {
    use tokio::io::AsyncReadExt as _;
    let (mut stream, _precheck_len): (
        std::pin::Pin<Box<dyn futures::Stream<Item = Result<bytes::Bytes, String>> + Send>>,
        Option<u64>,
    ) = match &byte_source {
        ByteSource::Tunnel(url) => {
            let client = state.http.clone();
            let resp =
                fetch_tunnel_response(&client, url, state.config.allow_private_fetch).await?;
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
            let meta = tokio::fs::metadata(path)
                .await
                .map_err(|e| format!("yt-dlp output missing: {e}"))?;
            let size = meta.len();
            if size == 0 {
                return Err(empty_stream_message(source_url));
            }
            if size > max_size {
                return Err("file exceeds this server's maximum file size".into());
            }
            let file = tokio::fs::File::open(path)
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
                None,
            )
        }
    };

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, String>>(
        crate::constants::STREAM_CHANNEL_CAPACITY,
    );
    let push_state = Arc::clone(state);
    let push_file_id = file_id.to_string();
    let push_filename = filename.to_string();
    let push_mime = mime_type.to_string();
    let push_capability = capability.to_string();
    let push_task = tokio::spawn(async move {
        crate::storage_client::push_file_streaming(
            &push_state,
            &push_file_id,
            &push_filename,
            &push_mime,
            rx,
            None,
            &UploadMode::Standard,
            Some(&push_capability),
        )
        .await
    });

    let mut total: u64 = 0;
    let mut last_progress = std::time::Instant::now() - std::time::Duration::from_secs(1);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("media download failed mid-stream: {e}"))?;
        total += chunk.len() as u64;
        if total > max_size {
            return Err("file exceeds this server's maximum file size".into());
        }

        if last_progress.elapsed() >= std::time::Duration::from_millis(750) {
            last_progress = std::time::Instant::now();
            let job_id_owned = job_id.to_string();
            let _ = state
                .db_call("update_fetch_job_progress", move |db| {
                    db::update_fetch_job_progress(
                        db,
                        &job_id_owned,
                        "transfer",
                        "downloading",
                        total as i64,
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
        let _ = state
            .db_call("update_fetch_job_progress_final", move |db| {
                db::update_fetch_job_progress(
                    db,
                    &job_id_owned,
                    "transfer",
                    "downloading",
                    total as i64,
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

    Ok(total)
}

async fn finalize_stored_file(
    state: &Arc<AppState>,
    user_id: &str,
    file_id: String,
    filename: String,
    mime_type: String,
    size_bytes: u64,
    delete_token: String,
) -> FetchResult {
    let completed = state
        .db_call("complete_fetch_reservation", move |db| {
            db::finish_reservation(
                db,
                &filename,
                &mime_type,
                size_bytes as i64,
                &file_id,
                &delete_token,
                None,
            )
        })
        .await
        .map_err(|e| format!("failed to finalize fetched file: {e:?}"))?;

    match completed {
        db::FinishReservationResult::Completed(record) => {
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
        _ => Err("reservation vanished during finalization".into()),
    }
}
