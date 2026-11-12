use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use bytes::Bytes;
use chrono::{Datelike, Timelike};
use flate2::Crc;
use futures::StreamExt;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

use super::{
    store::{download_and_store, fetch_tunnel_response},
    types::{ByteSource, FetchMeta, FetchResult},
};
use crate::{
    cobalt::{CobaltPickerItem, FetchOptions},
    db,
    state::AppState,
};

const MAX_BUNDLE_ITEMS: usize = 50;
const ITEM_TIMEOUT_SECS: u64 = 120;
const RESERVED_FINALIZE_SECS: u64 = 180;
const MAX_CLASSIC_ZIP_BYTES: u64 = 3_500_000_000;
const PREFIX_BYTES: usize = 512;
const INITIAL_READ_LIMIT: usize = 1024 * 1024;

struct CentralEntry {
    name: String,
    crc: u32,
    size: u32,
    offset: u32,
}

struct ItemKind {
    extension: String,
}

/// Best-effort filename for a single picker item before content sniffing.
#[must_use]
pub(crate) fn picker_item_filename(item: &CobaltPickerItem) -> &'static str {
    match item.r#type.trim().to_ascii_lowercase().as_str() {
        "image" | "photo" => "image.jpg",
        "gif" => "image.gif",
        "audio" | "music" => "audio.mp3",
        _ => "video.mp4",
    }
}

/// Bundles Cobalt picker results into a stored ZIP and uploads it through the
/// normal file pipeline.
pub(crate) async fn bundle_picker(
    state: &Arc<AppState>,
    user_id: &str,
    job_id: &str,
    source_url: &str,
    items: &[CobaltPickerItem],
    opts: &FetchOptions,
    max_size: u64,
    default_ttl_hours: f64,
    encrypted_ip: Option<String>,
    password: Option<String>,
) -> FetchResult {
    if items.is_empty() {
        return Err("picker returned no usable media URLs".into());
    }
    if items.len() > MAX_BUNDLE_ITEMS {
        return Err(format!(
            "picker returned {} media items; the maximum is {MAX_BUNDLE_ITEMS}",
            items.len()
        ));
    }

    let bundle_cap = max_size.min(MAX_CLASSIC_ZIP_BYTES);
    let deadline = Instant::now() + Duration::from_secs(crate::constants::FETCH_JOB_TIMEOUT_SECS)
        - Duration::from_secs(RESERVED_FINALIZE_SECS);

    let temp = tempfile::NamedTempFile::new()
        .map_err(|e| format!("could not create bundle temp file: {e}"))?;
    let mut zip = tokio::fs::File::create(temp.path())
        .await
        .map_err(|e| format!("could not create bundle temp file: {e}"))?;

    let mut central: Vec<CentralEntry> = Vec::with_capacity(items.len());
    let mut bundle_bytes: u64 = 0;
    let mut last_progress = Instant::now() - Duration::from_secs(1);
    let item_count = items.len() as i64;

    let result = async {
        for (position, item) in items.iter().enumerate() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("bundle download timed out".to_string());
            }
            let timeout = remaining.min(Duration::from_secs(ITEM_TIMEOUT_SECS));
            let index = position + 1;

            let _ = state
                .db_call("update_fetch_job_progress", {
                    let job_id = job_id.to_string();
                    let stage = format!("bundle-{index}");
                    move |db| {
                        db::update_fetch_job_progress(
                            db,
                            &job_id,
                            &stage,
                            "downloading",
                            index as i64,
                            item_count,
                        )
                    }
                })
                .await;

            let entry = tokio::time::timeout(
                timeout,
                download_item(
                    state,
                    &mut zip,
                    item,
                    position,
                    bundle_cap,
                    &mut bundle_bytes,
                ),
            )
            .await
            .map_err(|_| format!("bundle item {index} timed out"))??;

            central.push(entry);

            if last_progress.elapsed() >= Duration::from_millis(750) {
                last_progress = Instant::now();
                let _ = state
                    .db_call("update_fetch_job_progress", {
                        let job_id = job_id.to_string();
                        move |db| {
                            db::update_fetch_job_progress(
                                db,
                                &job_id,
                                "bundle",
                                "downloading",
                                bundle_bytes as i64,
                                bundle_cap as i64,
                            )
                        }
                    })
                    .await;
            }
        }

        write_central_directory(&mut zip, &central, &mut bundle_bytes, bundle_cap).await?;
        zip.flush()
            .await
            .map_err(|e| format!("could not finalize bundle: {e}"))?;
        drop(zip);

        let filename = bundle_filename(source_url);
        let meta = FetchMeta {
            title: Some("Media bundle".to_string()),
            author: None,
        };
        download_and_store(
            state,
            user_id,
            job_id,
            source_url,
            ByteSource::LocalFile(temp.path().to_path_buf()),
            Some(&filename),
            meta,
            opts,
            max_size,
            default_ttl_hours,
            encrypted_ip,
            password,
        )
        .await
    }
    .await;

    result
}

async fn download_item(
    state: &Arc<AppState>,
    zip: &mut tokio::fs::File,
    item: &CobaltPickerItem,
    position: usize,
    bundle_cap: u64,
    bundle_bytes: &mut u64,
) -> Result<CentralEntry, String> {
    let resp = fetch_tunnel_response(
        &state.http,
        state,
        &item.url,
        state.config.allow_private_fetch,
    )
    .await
    .map_err(|e| format!("could not download item {}: {e}", position + 1))?;
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    if let Some(len) = resp.content_length()
        && len == 0
    {
        return Err(format!("item {} was empty", position + 1));
    }

    let mut stream = resp.bytes_stream();
    let mut initial: Vec<Bytes> = Vec::new();
    let mut prefix = Vec::with_capacity(PREFIX_BYTES);
    let mut initial_len = 0usize;
    while prefix.len() < PREFIX_BYTES && initial_len < INITIAL_READ_LIMIT {
        match stream.next().await {
            Some(Ok(chunk)) => {
                if chunk.is_empty() {
                    continue;
                }
                let need = PREFIX_BYTES.saturating_sub(prefix.len());
                let take = need.min(chunk.len());
                prefix.extend_from_slice(&chunk[..take]);
                initial_len += chunk.len();
                initial.push(chunk);
            }
            Some(Err(e)) => {
                return Err(format!(
                    "item {} failed while downloading: {e}",
                    position + 1
                ));
            }
            None => break,
        }
    }
    if prefix.is_empty() {
        return Err(format!("item {} was empty", position + 1));
    }

    let kind = detect_kind(content_type.as_deref(), &prefix, &item.r#type);
    let name = entry_name(position + 1, &kind.extension);
    let offset = zip
        .stream_position()
        .await
        .map_err(|e| format!("could not write bundle: {e}"))?;
    if offset > u64::from(u32::MAX) {
        return Err("bundle exceeds the classic ZIP size limit".into());
    }

    write_local_header(zip, &name, bundle_bytes, bundle_cap).await?;

    let mut crc = Crc::new();
    let mut size: u64 = 0;
    for chunk in initial {
        size += chunk.len() as u64;
        if size > u64::from(u32::MAX) {
            return Err("bundle item exceeds the classic ZIP size limit".into());
        }
        *bundle_bytes = bundle_bytes
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| "bundle size overflow".to_string())?;
        if *bundle_bytes > bundle_cap {
            return Err("bundle exceeds this server's maximum file size".into());
        }
        crc.update(&chunk);
        zip.write_all(&chunk)
            .await
            .map_err(|e| format!("could not write bundle: {e}"))?;
    }
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|e| format!("item {} failed while downloading: {e}", position + 1))?;
        if chunk.is_empty() {
            continue;
        }
        size += chunk.len() as u64;
        if size > u64::from(u32::MAX) {
            return Err("bundle item exceeds the classic ZIP size limit".into());
        }
        *bundle_bytes = bundle_bytes
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| "bundle size overflow".to_string())?;
        if *bundle_bytes > bundle_cap {
            return Err("bundle exceeds this server's maximum file size".into());
        }
        crc.update(&chunk);
        zip.write_all(&chunk)
            .await
            .map_err(|e| format!("could not write bundle: {e}"))?;
    }

    if size == 0 {
        return Err(format!("item {} was empty", position + 1));
    }

    let end = zip
        .stream_position()
        .await
        .map_err(|e| format!("could not finalize bundle: {e}"))?;
    zip.seek(std::io::SeekFrom::Start(offset + 14))
        .await
        .map_err(|e| format!("could not finalize bundle: {e}"))?;
    let mut patch = Vec::with_capacity(12);
    put_u32(&mut patch, crc.sum());
    put_u32(&mut patch, size as u32);
    put_u32(&mut patch, size as u32);
    zip.write_all(&patch)
        .await
        .map_err(|e| format!("could not finalize bundle: {e}"))?;
    zip.seek(std::io::SeekFrom::Start(end))
        .await
        .map_err(|e| format!("could not finalize bundle: {e}"))?;

    Ok(CentralEntry {
        name,
        crc: crc.sum(),
        size: size as u32,
        offset: offset as u32,
    })
}

async fn write_local_header(
    zip: &mut tokio::fs::File,
    name: &str,
    bundle_bytes: &mut u64,
    bundle_cap: u64,
) -> Result<(), String> {
    let mut header = Vec::with_capacity(30 + name.len());
    header.extend_from_slice(b"PK\x03\x04");
    put_u16(&mut header, 20);
    put_u16(&mut header, 0x0800);
    put_u16(&mut header, 0);
    let (time, date) = dos_datetime();
    put_u16(&mut header, time);
    put_u16(&mut header, date);
    put_u32(&mut header, 0);
    put_u32(&mut header, 0);
    put_u32(&mut header, 0);
    put_u16(&mut header, name.len() as u16);
    put_u16(&mut header, 0);
    header.extend_from_slice(name.as_bytes());

    *bundle_bytes = bundle_bytes
        .checked_add(header.len() as u64)
        .ok_or_else(|| "bundle size overflow".to_string())?;
    if *bundle_bytes > bundle_cap {
        return Err("bundle exceeds this server's maximum file size".into());
    }
    zip.write_all(&header)
        .await
        .map_err(|e| format!("could not write bundle: {e}"))
}

async fn write_central_directory(
    zip: &mut tokio::fs::File,
    entries: &[CentralEntry],
    bundle_bytes: &mut u64,
    bundle_cap: u64,
) -> Result<(), String> {
    let cd_offset = zip
        .stream_position()
        .await
        .map_err(|e| format!("could not finalize bundle: {e}"))?;
    if cd_offset > u64::from(u32::MAX) {
        return Err("bundle exceeds the classic ZIP size limit".into());
    }

    let mut cd = Vec::new();
    for entry in entries {
        cd.extend_from_slice(b"PK\x01\x02");
        put_u16(&mut cd, 20);
        put_u16(&mut cd, 20);
        put_u16(&mut cd, 0x0800);
        put_u16(&mut cd, 0);
        let (time, date) = dos_datetime();
        put_u16(&mut cd, time);
        put_u16(&mut cd, date);
        put_u32(&mut cd, entry.crc);
        put_u32(&mut cd, entry.size);
        put_u32(&mut cd, entry.size);
        put_u16(&mut cd, entry.name.len() as u16);
        put_u16(&mut cd, 0);
        put_u16(&mut cd, 0);
        put_u16(&mut cd, 0);
        put_u16(&mut cd, 0);
        put_u32(&mut cd, 0);
        put_u32(&mut cd, entry.offset);
        cd.extend_from_slice(entry.name.as_bytes());
    }

    let mut eocd = Vec::with_capacity(22);
    eocd.extend_from_slice(b"PK\x05\x06");
    put_u16(&mut eocd, 0);
    put_u16(&mut eocd, 0);
    put_u16(&mut eocd, entries.len() as u16);
    put_u16(&mut eocd, entries.len() as u16);
    put_u32(&mut eocd, cd.len() as u32);
    put_u32(&mut eocd, cd_offset as u32);
    put_u16(&mut eocd, 0);

    let overhead = (cd.len() + eocd.len()) as u64;
    *bundle_bytes = bundle_bytes
        .checked_add(overhead)
        .ok_or_else(|| "bundle size overflow".to_string())?;
    if *bundle_bytes > bundle_cap {
        return Err("bundle exceeds this server's maximum file size".into());
    }

    zip.write_all(&cd)
        .await
        .map_err(|e| format!("could not finalize bundle: {e}"))?;
    zip.write_all(&eocd)
        .await
        .map_err(|e| format!("could not finalize bundle: {e}"))
}

fn detect_kind(content_type: Option<&str>, prefix: &[u8], item_type: &str) -> ItemKind {
    if let Some(kind) = infer::get(prefix) {
        let mime = kind.mime_type();
        if mime.starts_with("image/") || mime.starts_with("video/") || mime.starts_with("audio/") {
            return ItemKind {
                extension: kind.extension().to_string(),
            };
        }
    }

    if let Some(raw) = content_type {
        let mime = raw
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if mime.starts_with("image/") || mime.starts_with("video/") || mime.starts_with("audio/") {
            if let Some(exts) = mime_guess::get_mime_extensions_str(&mime)
                && let Some(ext) = exts.first()
            {
                return ItemKind {
                    extension: (*ext).to_string(),
                };
            }
        }
    }

    let kind = item_type.trim().to_ascii_lowercase();
    let extension = match kind.as_str() {
        "image" | "photo" => "jpg",
        "video" => "mp4",
        "audio" => "mp3",
        "gif" => "gif",
        _ => "bin",
    };
    ItemKind {
        extension: extension.to_string(),
    }
}

fn entry_name(position: usize, extension: &str) -> String {
    let category = extension_category(extension);
    format!("{category}_{position:02}.{extension}")
}

fn extension_category(extension: &str) -> &'static str {
    match extension.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif" | "heic" | "bmp" | "tif" | "tiff" => {
            "photo"
        }
        "mp4" | "webm" | "mov" | "mkv" | "m4v" => "video",
        "mp3" | "m4a" | "aac" | "ogg" | "wav" | "flac" | "opus" => "audio",
        _ => "file",
    }
}

fn bundle_filename(source_url: &str) -> String {
    let parsed = url::Url::parse(source_url.trim()).ok();
    let host = parsed
        .as_ref()
        .and_then(|url| url.host_str())
        .unwrap_or("media")
        .trim_start_matches("www.")
        .replace('.', "_");
    let host = if host == "instagram_com" {
        "instagram".to_string()
    } else {
        host
    };
    let id = parsed
        .as_ref()
        .and_then(|url| url.path_segments()?.filter(|s| !s.is_empty()).last())
        .unwrap_or("bundle");
    let safe: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(48)
        .collect();
    let safe_host: String = host
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(32)
        .collect();
    let safe = if safe.is_empty() { "bundle" } else { &safe };
    let safe_host = if safe_host.is_empty() {
        "media"
    } else {
        &safe_host
    };
    format!("{safe_host}_{safe}.zip")
}

fn dos_datetime() -> (u16, u16) {
    let now = chrono::Utc::now();
    let time =
        ((now.hour() as u16) << 11) | ((now.minute() as u16) << 5) | ((now.second() as u16) / 2);
    let year = now.year().clamp(1980, 2107) as u16;
    let date = ((year - 1980) << 9) | ((now.month() as u16) << 5) | now.day() as u16;
    (time, date)
}

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_flat_and_indexed() {
        assert_eq!(entry_name(1, "jpg"), "photo_01.jpg");
        assert_eq!(entry_name(2, "mp4"), "video_02.mp4");
        assert_eq!(entry_name(3, "m4a"), "audio_03.m4a");
        assert_eq!(entry_name(4, "bin"), "file_04.bin");
    }

    #[test]
    fn detects_infer_kind_before_fallback() {
        let kind = detect_kind(
            Some("application/octet-stream"),
            b"\xff\xd8\xff\xe0",
            "video",
        );
        assert_eq!(kind.extension, "jpg");
    }

    #[test]
    fn filename_is_sanitized() {
        assert_eq!(
            bundle_filename("https://www.instagram.com/p/DflAq__CbnK/"),
            "instagram_DflAq__CbnK.zip"
        );
        assert_eq!(
            bundle_filename("https://example.com/watch/abc"),
            "example_com_abc.zip"
        );
    }
}
