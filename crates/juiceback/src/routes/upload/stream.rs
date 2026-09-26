use std::sync::Arc;

use async_compression::tokio::bufread::GzipDecoder;
use bytes::Bytes;
use tokio::{
    io::{AsyncReadExt, BufReader},
    sync::mpsc,
};
use tokio_util::io::StreamReader;

use crate::{error::AppError, state::AppState, upload_mode::UploadMode};

type PushChannels = (
    mpsc::Sender<Result<Bytes, String>>,
    tokio::task::JoinHandle<Result<(), String>>,
);

pub fn spawn_upload_push(
    state: &Arc<AppState>,
    file_id: &str,
    filename: &str,
    mime_type: &str,
    host: Option<&str>,
    upload_mode: UploadMode,
    capability: &str,
) -> PushChannels {
    let (tx, rx) =
        mpsc::channel::<Result<Bytes, String>>(crate::constants::STREAM_CHANNEL_CAPACITY);

    let state_push = Arc::clone(state);
    let push_id = file_id.to_string();
    let push_filename = filename.to_string();
    let push_mime = mime_type.to_string();
    let push_host = host.map(str::to_owned);
    let push_mode = upload_mode;
    let push_capability = capability.to_string();

    let push_handle = tokio::spawn(async move {
        crate::storage_client::push_file_streaming(
            &state_push,
            &push_id,
            &push_filename,
            &push_mime,
            rx,
            push_host,
            &push_mode,
            Some(&push_capability),
        )
        .await
    });
    (tx, push_handle)
}

#[expect(clippy::too_many_arguments, reason = "pipeline context threading")]
pub async fn stream_upload_to_juicehost(
    mut field: axum::extract::multipart::Field<'_>,
    state: &Arc<AppState>,
    file_id: &str,
    filename: &str,
    mime_type: &str,
    host: Option<&str>,
    upload_mode: UploadMode,
    max_size: i64,
    capability: &str,
) -> Result<(i64, u64, std::time::Duration, std::time::Duration), AppError> {
    let (tx, push_handle) = spawn_upload_push(
        state,
        file_id,
        filename,
        mime_type,
        host,
        upload_mode,
        capability,
    );

    let read_start = std::time::Instant::now();
    let mut total_bytes: i64 = 0;
    let mut chunk_count: u64 = 0;
    let mut push_backpressure: std::time::Duration = std::time::Duration::default();

    let first_chunk = field
        .chunk()
        .await
        .map_err(|e| {
            tracing::warn!("stream upload chunk read error: {:?}", e);
            AppError::InvalidMultipart
        })?
        .unwrap_or_default();

    if !first_chunk.is_empty() {
        let level = state
            .jh_config
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map_or(crate::file_validation::ProtectionLevel::High, |c| {
                c.danger_level
            });
        if let Some(err) = crate::file_validation::blocked_file_error(
            crate::file_validation::validate_file(filename, &first_chunk, level),
        ) {
            return Err(err);
        }
    }

    total_bytes += first_chunk.len() as i64;
    if total_bytes > max_size {
        return Err(AppError::PayloadTooLarge);
    }
    chunk_count += 1;
    let send_start = std::time::Instant::now();
    if tx.send(Ok(first_chunk)).await.is_err() {
        return Err(AppError::TaskPanicked(
            "streaming push task panicked".into(),
        ));
    }
    push_backpressure += send_start.elapsed();

    while let Some(chunk) = field.chunk().await.map_err(|e| {
        tracing::warn!("stream upload chunk read error: {:?}", e);
        AppError::InvalidMultipart
    })? {
        total_bytes += chunk.len() as i64;
        if total_bytes > max_size {
            return Err(AppError::PayloadTooLarge);
        }
        chunk_count += 1;
        let send_start = std::time::Instant::now();
        if tx.send(Ok(chunk)).await.is_err() {
            break;
        }
        push_backpressure += send_start.elapsed();
    }
    drop(tx);
    let read_time = read_start.elapsed();

    let stream_start = std::time::Instant::now();
    push_handle
        .await
        .map_err(|_| AppError::TaskPanicked("streaming push task panicked".into()))?
        .map_err(AppError::from_juicehost_error)?;
    let push_wait = stream_start.elapsed();

    let read_bps = if read_time.as_secs_f64() > 0.0 {
        total_bytes as f64 / read_time.as_secs_f64()
    } else {
        0.0
    };
    tracing::debug!(
        "  stream: id={} bytes={} chunks={} read={:?} ({:.2} MB/s) \
         backpressure={:?} push_wait={:?}",
        file_id,
        total_bytes,
        chunk_count,
        read_time,
        read_bps / crate::constants::BYTES_PER_MIB,
        push_backpressure,
        push_wait,
    );

    Ok((total_bytes, chunk_count, push_backpressure, push_wait))
}

pub async fn pipe_gunzip_to_sender<S>(
    stream: S,
    tx: &mpsc::Sender<Result<Bytes, String>>,
    filename: &str,
    danger: crate::file_validation::ProtectionLevel,
    max_size: i64,
) -> Result<i64, AppError>
where
    S: futures::Stream<Item = Result<Bytes, std::io::Error>>,
{
    let decoder = GzipDecoder::new(BufReader::new(StreamReader::new(Box::pin(stream))));

    tokio::pin!(decoder);
    let mut buf = [0_u8; crate::constants::GZIP_READ_BUFFER_SIZE];
    let mut sniff: Vec<u8> = Vec::new();
    let mut validated = false;
    let mut forwarded_any = false;
    let mut decoded_total: i64 = 0;
    loop {
        let n = decoder
            .read(&mut buf)
            .await
            .map_err(|_| AppError::GzipDecodeFailed)?;
        if n == 0 {
            break;
        }
        decoded_total += n as i64;
        if decoded_total > max_size {
            return Err(AppError::PayloadTooLarge);
        }
        if !validated {
            sniff.extend_from_slice(&buf[..n]);
            if sniff.len() < crate::constants::FILE_SNIFF_PREFIX_LEN {
                continue;
            }
            validate_decoded_prefix(&sniff, filename, danger)?;
            if tx
                .send(Ok(Bytes::from(std::mem::take(&mut sniff))))
                .await
                .is_err()
            {
                return Err(AppError::TaskPanicked(
                    "streaming push task panicked".into(),
                ));
            }
            forwarded_any = true;
            validated = true;
        } else if tx
            .send(Ok(Bytes::copy_from_slice(&buf[..n])))
            .await
            .is_err()
        {
            if forwarded_any {
                break;
            }
            return Err(AppError::TaskPanicked(
                "streaming push task panicked".into(),
            ));
        } else {
            forwarded_any = true;
        }
    }
    if !validated {
        validate_decoded_prefix(&sniff, filename, danger)?;
        if !sniff.is_empty() {
            tx.send(Ok(Bytes::from(sniff)))
                .await
                .map_err(|_| AppError::TaskPanicked("streaming push task panicked".into()))?;
        }
    }
    Ok(decoded_total)
}

fn validate_decoded_prefix(
    data: &[u8],
    filename: &str,
    danger: crate::file_validation::ProtectionLevel,
) -> Result<(), AppError> {
    if let Some(err) = crate::file_validation::blocked_file_error(
        crate::file_validation::validate_file(filename, data, danger),
    ) {
        return Err(err);
    }
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "pipeline fns thread established context (state, ids, tokens); bundling params churns callers for no behavior gain"
)]
pub async fn stream_gzip_upload_to_juicehost(
    field: axum::extract::multipart::Field<'_>,
    state: &Arc<AppState>,
    file_id: &str,
    filename: &str,
    mime_type: &str,
    host: Option<&str>,
    upload_mode: UploadMode,
    max_size: i64,
    capability: &str,
    danger: crate::file_validation::ProtectionLevel,
) -> Result<(i64, std::time::Duration, std::time::Duration), AppError> {
    let (tx, push_handle) = spawn_upload_push(
        state,
        file_id,
        filename,
        mime_type,
        host,
        upload_mode,
        capability,
    );

    let read_start = std::time::Instant::now();

    let wire_cap = max_size;
    let chunk_stream = futures::stream::unfold((field, 0_i64), |(mut f, mut total)| async move {
        match f.chunk().await {
            Ok(Some(chunk)) => {
                total += chunk.len() as i64;
                if total > wire_cap {
                    let err =
                        std::io::Error::new(std::io::ErrorKind::Other, "wire size limit exceeded");
                    return Some((Err(err), (f, total)));
                }
                Some((Ok(chunk), (f, total)))
            }
            Ok(None) => None,
            Err(e) => {
                let err = std::io::Error::new(std::io::ErrorKind::Other, e.body_text());
                Some((Err(err), (f, total)))
            }
        }
    });

    let decode_result =
        pipe_gunzip_to_sender(Box::pin(chunk_stream), &tx, filename, danger, max_size).await;
    drop(tx);
    let read_time = read_start.elapsed();
    let total_bytes = decode_result?;

    let stream_start = std::time::Instant::now();
    push_handle
        .await
        .map_err(|_| AppError::TaskPanicked("streaming push task panicked".into()))?
        .map_err(AppError::from_juicehost_error)?;
    let push_wait = stream_start.elapsed();

    tracing::debug!(
        "  stream (gzip): id={} bytes={} read={:?} push_wait={:?}",
        file_id,
        total_bytes,
        read_time,
        push_wait,
    );

    Ok((total_bytes, read_time, push_wait))
}

#[cfg(test)]
mod tests {
    use futures::stream;

    use super::*;

    fn gzip_wire(data: &[u8]) -> Vec<u8> {
        use std::io::Write as _;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    fn io_stream(chunks: Vec<Bytes>) -> impl futures::Stream<Item = Result<Bytes, std::io::Error>> {
        stream::iter(chunks.into_iter().map(Ok))
    }

    async fn drain(
        tx: mpsc::Sender<Result<Bytes, String>>,
        mut rx: mpsc::Receiver<Result<Bytes, String>>,
    ) -> Vec<u8> {
        drop(tx);
        let mut out = Vec::new();
        while let Some(item) = rx.recv().await {
            out.extend_from_slice(&item.unwrap());
        }
        out
    }

    #[tokio::test]
    async fn gunzip_roundtrip_multi_chunk() {
        let original = b"The quick brown fox jumps over the lazy dog. ".repeat(5000);
        let wire = gzip_wire(&original);

        let mut frames = Vec::new();
        let mut i = 0;
        while i < wire.len() {
            let end = (i + 7919).min(wire.len());
            frames.push(Bytes::copy_from_slice(&wire[i..end]));
            i = end;
        }
        let (tx, rx) = mpsc::channel(16);
        let total = pipe_gunzip_to_sender(
            io_stream(frames),
            &tx,
            "notes.txt",
            crate::file_validation::ProtectionLevel::High,
            100 * 1024 * 1024,
        )
        .await
        .unwrap();
        assert_eq!(total as usize, original.len());
        assert_eq!(drain(tx, rx).await, original);
    }

    #[tokio::test]
    async fn gunzip_rejects_zip_bomb() {
        let original = vec![0_u8; 4 * 1024 * 1024];
        let wire = gzip_wire(&original);
        assert!(wire.len() < 64 * 1024);
        let (tx, rx) = mpsc::channel(16);
        let err = pipe_gunzip_to_sender(
            io_stream(vec![Bytes::from(wire)]),
            &tx,
            "zeros.bin",
            crate::file_validation::ProtectionLevel::High,
            1024,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, AppError::PayloadTooLarge));
        drop(rx);
    }

    #[tokio::test]
    async fn gunzip_rejects_corrupt_input() {
        let (tx, rx) = mpsc::channel(16);
        let err = pipe_gunzip_to_sender(
            io_stream(vec![Bytes::from_static(b"definitely not gzip")]),
            &tx,
            "notes.txt",
            crate::file_validation::ProtectionLevel::High,
            1024 * 1024,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, AppError::GzipDecodeFailed));
        drop(rx);
    }

    #[tokio::test]
    async fn gunzip_blocks_dangerous_type_before_forwarding() {
        let mut payload = b"MZ".to_vec();
        payload.extend_from_slice(&[0_u8; 2048]);
        let wire = gzip_wire(&payload);
        let (tx, rx) = mpsc::channel(16);
        let err = pipe_gunzip_to_sender(
            io_stream(vec![Bytes::from(wire)]),
            &tx,
            "evil.exe",
            crate::file_validation::ProtectionLevel::High,
            1024 * 1024,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, AppError::BlockedFileType(_)));
        drop(rx);
    }

    #[tokio::test]
    async fn gunzip_rejects_empty_input() {
        let (tx, rx) = mpsc::channel(16);
        let err = pipe_gunzip_to_sender(
            io_stream(vec![]),
            &tx,
            "notes.txt",
            crate::file_validation::ProtectionLevel::High,
            1024 * 1024,
        )
        .await
        .unwrap_err();

        assert!(matches!(err, AppError::GzipDecodeFailed));
        drop(rx);
    }
}
