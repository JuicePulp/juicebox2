use std::sync::Arc;

use super::{
    errors::format_error_response,
    target::{require_juicehost_url, resolve_juicehost_target},
};
use crate::{state::AppState, upload_mode::UploadMode};

/// Bounded replay buffer for the QUIC→HTTP fallback path.
///
/// Retains stream chunks as refcounted `Bytes` (cheap clones, no memcpy) up
/// to `QUIC_SPILL_THRESHOLD_BYTES`, then spills to a temp file and drops the
/// retained chunks. The QUIC happy path therefore pins at most ~8 MiB no
/// matter the file size; only a QUIC failure pays for disk-backed replay.
/// The temp file deletes itself on drop, so no cleanup path can leak it.
///
/// Spill writes are synchronous page-cache writes of network-sized chunks
/// (no fsync, no large copies): same class as the blocking `statvfs`/`fs`
/// calls already on juicehost's hot paths, far below the executor's
/// blocking tolerance.
pub struct QuicFallbackBuffer {
    ram: Vec<bytes::Bytes>,
    ram_bytes: usize,
    spilled_bytes: u64,
    spill: Option<tempfile::NamedTempFile>,
}

impl QuicFallbackBuffer {
    pub fn new() -> Self {
        Self {
            ram: Vec::new(),
            ram_bytes: 0,
            spilled_bytes: 0,
            spill: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.ram.is_empty() && self.spilled_bytes == 0
    }

    pub fn push(&mut self, chunk: &bytes::Bytes) -> Result<(), String> {
        if let Some(spill) = self.spill.as_mut() {
            use std::io::Write as _;
            spill
                .write_all(chunk)
                .map_err(|e| format!("fallback spill failed: {e}"))?;
            self.spilled_bytes += chunk.len() as u64;
            return Ok(());
        }
        if self.ram_bytes + chunk.len() > crate::constants::QUIC_SPILL_THRESHOLD_BYTES {
            let mut spill = tempfile::NamedTempFile::new()
                .map_err(|e| format!("fallback spill failed: {e}"))?;
            use std::io::Write as _;
            for c in self.ram.drain(..) {
                spill
                    .write_all(&c)
                    .map_err(|e| format!("fallback spill failed: {e}"))?;
                self.spilled_bytes += c.len() as u64;
            }
            spill
                .write_all(chunk)
                .map_err(|e| format!("fallback spill failed: {e}"))?;
            self.spilled_bytes += chunk.len() as u64;
            self.spill = Some(spill);
            return Ok(());
        }
        self.ram_bytes += chunk.len();
        self.ram.push(chunk.clone());
        Ok(())
    }

    /// Concatenate everything for the HTTP fallback POST. Only runs when
    /// QUIC already failed, so the copy here replaces the failure path's
    /// cost instead of adding to the happy path.
    pub fn materialize(mut self) -> Result<Vec<bytes::Bytes>, String> {
        if let Some(spill) = self.spill.take() {
            let data =
                std::fs::read(spill.path()).map_err(|e| format!("fallback replay failed: {e}"))?;
            return Ok(vec![bytes::Bytes::from(data)]);
        }
        Ok(std::mem::take(&mut self.ram))
    }
}

/// Stream a file to juicehost over HTTP or QUIC. channel-based, no full
/// buffering. Close the sender = EOF. QUIC falls back to TCP.
#[expect(
    clippy::too_many_arguments,
    reason = "pipeline fns thread established context (state, ids, tokens); bundling params churns callers for no behavior gain"
)]
#[tracing::instrument(skip_all)]
pub async fn push_file_streaming(
    state: &Arc<AppState>,
    id: &str,
    filename: &str,
    mime_type: &str,
    chunk_rx: tokio::sync::mpsc::Receiver<Result<bytes::Bytes, String>>,
    host: Option<String>,
    mode: &UploadMode,
    capability: Option<&str>,
) -> Result<(), String> {
    let custom_host = host.as_deref().map(str::trim).filter(|h| !h.is_empty());
    let target = resolve_juicehost_target(state, custom_host)
        .await
        .map_err(|e| e.to_string())?;

    if *mode == UploadMode::Quic
        && custom_host.is_none()
        && !state.config.juicehost_api_key.is_empty()
    {
        // QUIC is internal-only!!
        // always use the direct juicehost URL and never the public-facing host override
        // (which may point to Cloudflare/Nginx)
        require_juicehost_url(&state.config.juicehost_url).map_err(|e| e.to_string())?;

        let mut fallback = QuicFallbackBuffer::new();
        let mut rx = chunk_rx;

        match crate::quic::push_file_streaming_quic_streamed(
            state,
            id,
            filename,
            mime_type,
            &mut rx,
            &mut fallback,
        )
        .await
        {
            Ok(()) => {
                drop(fallback);
                return Ok(());
            }
            Err(e) => {
                // If juicehost itself returned a non-2xx (e.g. 507 disk full),
                // return that error directly as we kinda have no point retrying over TCP.
                if e.contains("(status=") {
                    return Err(e);
                }
                tracing::warn!("QUIC push failed for {id}, falling back to HTTP: {e}");
            }
        }

        // QUIC failed partway.. drain leftovers into the bounded buffer.
        while let Some(chunk) = rx.recv().await {
            if let Ok(data) = chunk {
                fallback.push(&data)?;
            }
        }

        if fallback.is_empty() {
            return Err("no data received for QUIC push".into());
        }

        return push_buffered_to_http(
            state,
            id,
            filename,
            mime_type,
            fallback.materialize()?,
            None,
            capability,
        )
        .await;
    }

    let encoded_fn =
        percent_encoding::utf8_percent_encode(filename, percent_encoding::NON_ALPHANUMERIC)
            .to_string();
    let url = format!(
        "{}/internal/file/stream/{}/{}",
        target.base_url, id, encoded_fn
    );
    let stream = tokio_stream::wrappers::ReceiverStream::new(chunk_rx);
    let body = reqwest::Body::wrap_stream(stream);

    let req_start = std::time::Instant::now();
    let mut request = target
        .client
        .post(&url)
        .header("x-mime-type", mime_type)
        .headers(target.headers);
    if let Some(capability) = capability {
        request = request.header("x-juicehost-file-capability", capability);
    }
    let resp = request
        .body(body)
        .send()
        .await
        .map_err(|e| format!("juicehost streaming push error: {e}"))?;
    let send_time = req_start.elapsed();

    if resp.status().is_success() {
        tracing::debug!("push_stream: id={} url={} send={:?}", id, url, send_time,);
        tracing::info!("juicehost streaming push: id={id} ok");
        Ok(())
    } else {
        let status = resp.status();
        let resp_start = std::time::Instant::now();
        let body_text = resp.text().await.unwrap_or_default();
        let resp_time = resp_start.elapsed();
        tracing::warn!(
            "push_stream error: id={} url={} send={:?} resp_read={:?} status={}",
            id,
            url,
            send_time,
            resp_time,
            status.as_u16(),
        );
        Err(format!(
            "{} (url={})",
            format_error_response(status.as_u16(), body_text),
            url
        ))
    }
}

/// Push pre-buffered chunks to juicehost over HTTP.
async fn push_buffered_to_http(
    state: &Arc<AppState>,
    id: &str,
    filename: &str,
    mime_type: &str,
    chunks: Vec<bytes::Bytes>,
    host: Option<&str>,
    capability: Option<&str>,
) -> Result<(), String> {
    let data = tokio::task::spawn_blocking(move || {
        let total: usize = chunks.iter().map(bytes::Bytes::len).sum();
        let mut data = Vec::with_capacity(total);
        for c in chunks {
            data.extend_from_slice(&c);
        }
        data
    })
    .await
    .map_err(|_| "data concatenation panicked".to_string())?;
    push_file_to_juicehost(state, id, filename, mime_type, data, host, capability).await
}

/// Push raw bytes to juicehost and it will stay on juicehost's disk
#[tracing::instrument(skip_all)]
pub async fn push_file_to_juicehost(
    state: &Arc<AppState>,
    id: &str,
    filename: &str,
    mime_type: &str,
    data: Vec<u8>,
    host: Option<&str>,
    capability: Option<&str>,
) -> Result<(), String> {
    let target = resolve_juicehost_target(state, host)
        .await
        .map_err(|e| e.to_string())?;

    let build_start = std::time::Instant::now();
    let file_part = reqwest::multipart::Part::bytes(data)
        .file_name(filename.to_string())
        .mime_str(mime_type)
        .map_err(|e| format!("mime error: {e}"))?;

    let form = reqwest::multipart::Form::new()
        .text("id", id.to_string())
        .text("filename", filename.to_string())
        .part("file", file_part);
    let build_time = build_start.elapsed();

    let send_start = std::time::Instant::now();
    let url = format!("{}/internal/file", target.base_url);
    let mut request = target.client.post(&url).headers(target.headers);
    if let Some(capability) = capability {
        request = request.header("x-juicehost-file-capability", capability);
    }
    let resp = request
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("juicehost push error: {e}"))?;
    let send_time = send_start.elapsed();

    tracing::debug!(
        "juicehost push: id={} build={:?} send={:?} status={}",
        id,
        build_time,
        send_time,
        resp.status()
    );

    if resp.status().is_success() {
        tracing::info!("juicehost push: id={id} ok");
        Ok(())
    } else {
        let body_start = std::time::Instant::now();
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        let body_time = body_start.elapsed();
        tracing::warn!(
            "juicehost push error: id={} url={} body_read={:?} status={}",
            id,
            url,
            body_time,
            status.as_u16(),
        );
        Err(format!(
            "{} (url={})",
            format_error_response(status.as_u16(), body),
            url
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_buffer_ram_roundtrip() {
        let mut buf = QuicFallbackBuffer::new();
        assert!(buf.is_empty());
        buf.push(&bytes::Bytes::from_static(b"hello ")).unwrap();
        buf.push(&bytes::Bytes::from_static(b"world")).unwrap();
        assert!(!buf.is_empty());
        assert!(buf.spill.is_none());
        let parts = buf.materialize().unwrap();
        let total: Vec<u8> = parts.concat();
        assert_eq!(total, b"hello world");
    }

    #[test]
    fn fallback_buffer_spills_and_replays() {
        let mut buf = QuicFallbackBuffer::new();
        // 10 MiB in 64 KiB pieces forces a spill past the 8 MiB threshold.
        let piece = bytes::Bytes::from(vec![0xABu8; 64 * 1024]);
        for _ in 0..160 {
            buf.push(&piece).unwrap();
        }
        assert!(buf.spill.is_some());
        assert!(buf.ram.is_empty());
        let parts = buf.materialize().unwrap();
        let total: Vec<u8> = parts.concat();
        assert_eq!(total.len(), 10 * 1024 * 1024);
        assert!(total.iter().all(|&b| b == 0xAB));
    }

    #[test]
    fn fallback_buffer_empty_materializes_empty() {
        let buf = QuicFallbackBuffer::new();
        assert!(buf.materialize().unwrap().is_empty());
    }
}
