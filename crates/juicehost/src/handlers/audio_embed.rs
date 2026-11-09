use std::{
    collections::HashMap,
    ffi::OsString,
    path::{Path as FsPath, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime},
};

use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::Response,
};
use bytes::Bytes;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use super::{
    serve::{RangeResult, file_cache_control_value, parse_range},
    shell::{GateDecision, ShellMode, gate_decision, remaining_ttl_secs},
};
use crate::{
    error::{JuicehostError, StorageError},
    state::AppState,
    storage,
    storage::valid_component as is_valid_id,
};

const CACHE_VERSION: u32 = 14;
const PREVIEW_WAIT: Duration = Duration::from_secs(3);
const CACHE_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const TEMP_MAX_AGE: Duration = Duration::from_secs(60 * 60);
const THUMBNAIL_FETCH_TIMEOUT: Duration = Duration::from_secs(8);
const THUMBNAIL_MAX_BYTES: u64 = 8 * 1024 * 1024;
const TITLE_MAX_CHARS: usize = 96;
const TITLE_MAX_CHARS_WIDE: usize = 48;
const BRAND_ORANGE: &str = "0xF97316";
const BRAND_ORANGE_RGB: (u8, u8, u8) = (0xF9, 0x73, 0x16);
const BRAND_CREAM: &str = "0xD5C7BC";
const BRAND_MUTED: &str = "0xA8988B";
const BRAND_BORDER: &str = "0x3A302A";
const BACKGROUND: &str = "0x141110";
const THUMB_SIZE: u32 = 168;
const THUMB_X: u32 = 32;
const THUMB_Y: u32 = 92;
const TEXT_X: u32 = 224;
const META_VALUE_X: u32 = TEXT_X + 112;
const META_LABEL_SIZE: u32 = 16;
const META_VALUE_SIZE: u32 = 17;
const META_ROW_HEIGHT: u32 = 23;
const META_MAX_ROWS: usize = 6;
const TIME_FONT_SIZE: u32 = 20;
const TIME_Y_FROM_BOTTOM: u32 = 62;
const PROGRESS_HEIGHT: u32 = 10;
const PROGRESS_BOTTOM_MARGIN: u32 = 20;
const PROGRESS_SIDE_MARGIN: u32 = THUMB_X;
const PROGRESS_TRACK: &str = "0x555555";
const VIDEO_FPS: u32 = 2;
const FRAME_DURATION_TOLERANCE: f64 = 1.0;
const LOGO_PNG: &[u8] = include_bytes!("../../../juicefront/static/assets/logo.png");
const TITLE_FONT: &[u8] = include_bytes!("../../../juicefront/static/assets/legacy/title.ttf");
const BODY_FONT: &[u8] = include_bytes!("../../../juicefront/static/assets/legacy/secondary.ttf");
const WIDE_FONT: &[u8] = include_bytes!("../../../juicefront/static/assets/legacy/unifont.ttf");
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct AudioEmbedInfo {
    pub video_path: String,
    pub poster_path: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug)]
pub struct GeneratedAudioEmbed {
    pub info: AudioEmbedInfo,
    pub video_file: PathBuf,
    pub poster_file: PathBuf,
    pub seconds: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheMeta {
    version: u32,
    etag: String,
    size: u64,
    extension: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    thumb_key: String,
    seconds: u64,
    limit: u64,
    width: u32,
    height: u32,
}

struct CachePaths {
    video: PathBuf,
    poster: PathBuf,
    meta: PathBuf,
}

fn cache_paths(state: &AppState, id: &str) -> CachePaths {
    let dir = state.audio_embed.cache_dir();
    CachePaths {
        video: dir.join(format!("{id}.mp4")),
        poster: dir.join(format!("{id}.jpg")),
        meta: dir.join(format!("{id}.json")),
    }
}

fn eligible(state: &AppState, meta: &storage::FileMetadata) -> bool {
    state.audio_embed.enabled()
        && meta.size > 0
        && meta.size <= state.audio_embed.max_source_bytes()
        && storage::guess_mime(&meta.extension).starts_with("audio/")
}

fn sanitize_text(raw: &str) -> String {
    let normalized: String = raw
        .chars()
        .filter(|c| !is_emoji_supplement(*c))
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    normalized.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn title_for_video(raw: &str, extension: &str) -> String {
    let mut title = sanitize_text(raw);
    if !extension.is_empty() {
        let suffix = format!(".{extension}");
        if title.len() >= suffix.len()
            && title
                .get(title.len() - suffix.len()..)
                .is_some_and(|tail| tail.eq_ignore_ascii_case(&suffix))
        {
            title.truncate(title.len() - suffix.len());
            title = title.trim_end().to_string();
        }
    }
    let max_chars = if title_needs_wide_font(&title) {
        TITLE_MAX_CHARS_WIDE
    } else {
        TITLE_MAX_CHARS
    };
    if title.chars().count() > max_chars {
        title = title
            .chars()
            .take(max_chars)
            .collect::<String>()
            .trim_end()
            .to_string();
        title.push_str("...");
    }
    title
}

fn title_needs_wide_font(title: &str) -> bool {
    title.chars().any(|c| !c.is_ascii() && c != '\u{2026}')
}

fn is_emoji_supplement(c: char) -> bool {
    matches!(c, '\u{200D}' | '\u{20E3}' | '\u{FE0E}' | '\u{FE0F}')
        || ('\u{1F000}'..='\u{1FAFF}').contains(&c)
}

fn author_for_video(raw: &str) -> String {
    let value = sanitize_text(raw);
    value
        .strip_suffix("- Topic")
        .map_or(value.clone(), |author| author.trim_end().to_string())
}

fn char_units(c: char) -> usize {
    if c.is_ascii() || c == '\u{2026}' {
        1
    } else {
        2
    }
}

fn text_units(value: &str) -> usize {
    value.chars().map(char_units).sum()
}

fn truncate_units(value: &str, max_units: usize) -> String {
    if text_units(value) <= max_units {
        return value.to_string();
    }
    let budget = max_units.saturating_sub(3);
    let mut out = String::new();
    let mut used = 0;
    for ch in value.chars() {
        let units = char_units(ch);
        if used + units > budget {
            break;
        }
        out.push(ch);
        used += units;
    }
    out = out.trim_end().to_string();
    out.push_str("...");
    out
}

fn wrap_text(value: &str, max_units: usize, max_lines: usize) -> Vec<String> {
    if value.trim().is_empty() || max_units == 0 || max_lines == 0 {
        return Vec::new();
    }
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut used = 0;
    let mut truncated = false;

    for word in value.split_whitespace() {
        let word_units = text_units(word);
        if !current.is_empty() {
            if used + 1 + word_units <= max_units {
                current.push(' ');
                current.push_str(word);
                used += 1 + word_units;
                continue;
            }
            lines.push(current);
            current = String::new();
            used = 0;
            if lines.len() >= max_lines {
                truncated = true;
                break;
            }
        }
        if word_units <= max_units {
            current.push_str(word);
            used = word_units;
            continue;
        }
        for ch in word.chars() {
            let units = char_units(ch);
            if used + units > max_units && !current.is_empty() {
                lines.push(current);
                current = String::new();
                used = 0;
                if lines.len() >= max_lines {
                    truncated = true;
                    break;
                }
            }
            current.push(ch);
            used += units;
        }
        if truncated {
            break;
        }
    }

    if !current.is_empty() {
        if lines.len() < max_lines {
            lines.push(current);
        } else {
            truncated = true;
        }
    }
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        truncated = true;
    }
    if truncated && let Some(last) = lines.last_mut() {
        while text_units(last) + 3 > max_units {
            last.pop();
        }
        *last = last.trim_end().to_string();
        last.push_str("...");
    }
    lines
}

fn youtube_video_id_from_filename(filename: &str) -> Option<String> {
    let stem = filename.rsplit_once('.').map_or(filename, |(stem, _)| stem);
    let (_, id) = stem.rsplit_once('-')?;
    if id.len() == 11
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        Some(id.to_string())
    } else {
        None
    }
}

fn escape_filter_path(path: &std::path::Path) -> String {
    let value = path.to_string_lossy();
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        if matches!(ch, '\\' | ':' | '\'' | ',' | '[' | ']' | ';') {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

async fn read_cache(
    state: &AppState,
    id: &str,
    meta: &storage::FileMetadata,
    title: Option<&str>,
    author: Option<&str>,
    thumb_key: Option<&str>,
) -> Option<GeneratedAudioEmbed> {
    let paths = cache_paths(state, id);
    let raw = tokio::fs::read(&paths.meta).await.ok()?;
    let cached: CacheMeta = serde_json::from_slice(&raw).ok()?;
    let settings = &state.audio_embed;
    if cached.version != CACHE_VERSION
        || cached.etag != meta.etag
        || cached.size != meta.size
        || cached.extension != meta.extension
        || title.is_some_and(|title| cached.title != title)
        || author.is_some_and(|author| cached.author != author)
        || thumb_key.is_some_and(|key| cached.thumb_key != key)
        || cached.limit != settings.max_seconds()
        || cached.width != settings.width()
        || cached.height != settings.height()
    {
        return None;
    }
    let video_meta = tokio::fs::metadata(&paths.video).await.ok()?;
    let poster_meta = tokio::fs::metadata(&paths.poster).await.ok()?;
    if !video_meta.is_file()
        || video_meta.len() == 0
        || !poster_meta.is_file()
        || poster_meta.len() == 0
    {
        return None;
    }
    Some(GeneratedAudioEmbed {
        info: AudioEmbedInfo {
            video_path: format!("/e/{id}.mp4"),
            poster_path: format!("/e/{id}.jpg"),
            width: settings.width(),
            height: settings.height(),
        },
        video_file: paths.video,
        poster_file: paths.poster,
        seconds: cached.seconds,
    })
}

pub async fn prepare_for_discord(
    state: &Arc<AppState>,
    id: &str,
    meta: &storage::FileMetadata,
    title: &str,
    author: Option<&str>,
    source_filename: Option<&str>,
) -> Option<AudioEmbedInfo> {
    if !is_valid_id(id) || !eligible(state, meta) {
        return None;
    }
    let title = title_for_video(title, &meta.extension);
    let author = author_for_video(author.unwrap_or(""));
    let thumb_key = source_filename
        .and_then(youtube_video_id_from_filename)
        .map(|video_id| format!("yt:{video_id}"));
    if let Some(cached) = read_cache(
        state,
        id,
        meta,
        Some(&title),
        Some(&author),
        thumb_key.as_deref(),
    )
    .await
    {
        return Some(cached.info);
    }
    let mut task = tokio::spawn(ensure_video(
        Arc::clone(state),
        id.to_string(),
        meta.clone(),
        Some(title),
        Some(author),
        thumb_key,
    ));
    match tokio::time::timeout(PREVIEW_WAIT, &mut task).await {
        Ok(Ok(Some(generated))) => Some(generated.info),
        Ok(Ok(None)) | Ok(Err(_)) => None,
        Err(_) => {
            tracing::info!(
                "audio embed generation for {id} is still running after preview wait; advertising media"
            );
            Some(AudioEmbedInfo {
                video_path: format!("/e/{id}.mp4"),
                poster_path: format!("/e/{id}.jpg"),
                width: state.audio_embed.width(),
                height: state.audio_embed.height(),
            })
        }
    }
}

pub async fn ensure_video(
    state: Arc<AppState>,
    id: String,
    meta: storage::FileMetadata,
    title: Option<String>,
    author: Option<String>,
    thumb_key: Option<String>,
) -> Option<GeneratedAudioEmbed> {
    if !is_valid_id(&id) || !eligible(&state, &meta) {
        return None;
    }
    let lock = state
        .audio_embed_locks
        .entry(id.clone())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone();
    let _guard = lock.lock().await;
    if let Some(cached) = read_cache(
        &state,
        &id,
        &meta,
        title.as_deref(),
        author.as_deref(),
        thumb_key.as_deref(),
    )
    .await
    {
        return Some(cached);
    }

    let _permit = state.audio_embed_semaphore.acquire().await.ok()?;
    let timeout = Duration::from_secs(state.audio_embed.timeout_seconds());
    match tokio::time::timeout(
        timeout,
        generate(
            &state,
            &id,
            &meta,
            title.as_deref(),
            author.as_deref(),
            thumb_key.as_deref(),
        ),
    )
    .await
    {
        Ok(Ok(generated)) => {
            tracing::info!(
                "generated audio embed for {id} ({}x{}, {}s)",
                state.audio_embed.width(),
                state.audio_embed.height(),
                generated.seconds
            );
            Some(generated)
        }
        Ok(Err(error)) => {
            tracing::warn!("audio embed generation failed for {id}: {error}");
            None
        }
        Err(_) => {
            tracing::warn!(
                "audio embed generation timed out for {id} after {}s",
                state.audio_embed.timeout_seconds()
            );
            None
        }
    }
}

struct TempCleanup {
    paths: Vec<PathBuf>,
}

impl TempCleanup {
    fn new(paths: Vec<PathBuf>) -> Self {
        Self { paths }
    }
}

impl Drop for TempCleanup {
    fn drop(&mut self) {
        for path in &self.paths {
            let _ = std::fs::remove_file(path);
        }
    }
}

enum Input {
    Local(PathBuf),
    Temp(PathBuf),
}

async fn resolve_input(state: &AppState, id: &str, temp: &PathBuf) -> Result<Input, String> {
    let gate = gate_decision(state, id, ShellMode::Preview, None, false).await;
    let (local, r2_key) = match gate {
        GateDecision::Respond(_) | GateDecision::Ciphertext => {
            return Err(String::from("file is not publicly previewable"));
        }
        GateDecision::HotServe { key, .. } => (state.storage.local_path(id), Some(key)),
        GateDecision::ServeLocal { .. } => (state.storage.local_path(id), None),
    };

    if let Some(path) = local
        && tokio::fs::metadata(&path).await.is_ok()
    {
        return Ok(Input::Local(path));
    }

    let stream = if let Some(key) = r2_key {
        match state.r2.as_ref() {
            Some(r2) => match r2.get_stream(&key).await {
                Ok(stream) => stream,
                Err(error) => {
                    tracing::debug!("audio embed R2 read failed for {id}: {error}; using storage");
                    state
                        .storage
                        .get_stream(id)
                        .await
                        .map_err(|e| format!("open source: {e}"))?
                }
            },
            None => state
                .storage
                .get_stream(id)
                .await
                .map_err(|e| format!("open source: {e}"))?,
        }
    } else {
        state
            .storage
            .get_stream(id)
            .await
            .map_err(|e| format!("open source: {e}"))?
    };

    let mut file = tokio::fs::File::create(temp)
        .await
        .map_err(|e| format!("create source temp: {e}"))?;
    let mut written = 0u64;
    let mut stream = stream;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("read source: {e}"))?;
        written = written.saturating_add(chunk.len() as u64);
        if written > state.audio_embed.max_source_bytes() {
            return Err(String::from("source exceeds audio embed limit"));
        }
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("write source temp: {e}"))?;
    }
    file.flush()
        .await
        .map_err(|e| format!("flush source temp: {e}"))?;
    if written == 0 {
        return Err(String::from("source is empty"));
    }
    Ok(Input::Temp(temp.clone()))
}

#[derive(Debug, Default)]
struct SourceInfo {
    duration: f64,
    audio_codec: String,
    has_cover: bool,
    tags: Vec<(String, String)>,
}

const TAG_PRIORITY: &[&str] = &[
    "title",
    "artist",
    "album_artist",
    "album",
    "date",
    "genre",
    "track",
    "disc",
    "composer",
    "lyricist",
    "producer",
    "comment",
    "performer",
    "publisher",
    "copyright",
];

fn ffprobe_path(ffmpeg: &str) -> PathBuf {
    let path = FsPath::new(ffmpeg);
    if let Some(parent) = path.parent() {
        let candidate = parent.join("ffprobe");
        if candidate.is_file() {
            return candidate;
        }
    }
    PathBuf::from("ffprobe")
}

fn json_f64(value: &serde_json::Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|text| text.parse::<f64>().ok()))
}

fn collect_tags(value: &serde_json::Value, tags: &mut Vec<(String, String)>) {
    let Some(object) = value.as_object() else {
        return;
    };
    for (key, value) in object {
        let Some(text) = value.as_str() else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let text = sanitize_text(text);
        if key.is_empty() || text.is_empty() {
            continue;
        }
        if let Some(existing) = tags.iter_mut().find(|(name, _)| name == &key) {
            existing.1 = text;
        } else {
            tags.push((key, text));
        }
    }
}

async fn probe_source(ffmpeg: &str, input: &FsPath) -> Result<SourceInfo, String> {
    let mut cmd = tokio::process::Command::new(ffprobe_path(ffmpeg));
    cmd.kill_on_drop(true)
        .arg("-v")
        .arg("error")
        .arg("-print_format")
        .arg("json")
        .arg("-show_format")
        .arg("-show_streams")
        .arg(input);
    let output = tokio::time::timeout(Duration::from_secs(15), cmd.output())
        .await
        .map_err(|_| String::from("ffprobe timed out"))?
        .map_err(|error| format!("run ffprobe: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "ffprobe failed: {}",
            stderr.chars().take(400).collect::<String>()
        ));
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("parse ffprobe json: {error}"))?;
    let format = &value["format"];
    let streams = value["streams"].as_array().cloned().unwrap_or_default();
    let audio = streams
        .iter()
        .find(|stream| stream["codec_type"].as_str() == Some("audio"));
    let duration = json_f64(&format["duration"])
        .or_else(|| audio.and_then(|stream| json_f64(&stream["duration"])))
        .unwrap_or(0.0);
    let audio_codec = audio
        .and_then(|stream| stream["codec_name"].as_str())
        .unwrap_or("")
        .to_string();
    let has_cover = streams.iter().any(|stream| {
        stream["disposition"]["attached_pic"].as_u64() == Some(1)
            || (stream["codec_type"].as_str() == Some("video")
                && matches!(
                    stream["codec_name"].as_str(),
                    Some("mjpeg" | "png" | "bmp" | "webp")
                ))
    });
    let mut tags = Vec::new();
    collect_tags(&format["tags"], &mut tags);
    if let Some(audio) = audio {
        collect_tags(&audio["tags"], &mut tags);
    }
    Ok(SourceInfo {
        duration,
        audio_codec,
        has_cover,
        tags,
    })
}

fn is_technical_tag(key: &str) -> bool {
    matches!(
        key,
        "encoder"
            | "major_brand"
            | "minor_version"
            | "compatible_brands"
            | "handler_name"
            | "vendor_id"
            | "language"
            | "duration"
            | "size"
            | "bit_rate"
            | "codec_name"
            | "codec_type"
            | "filename"
            | "mimetype"
            | "creation_time"
            | "media_type"
    )
}

fn tag_label(key: &str) -> String {
    match key {
        "album_artist" => String::from("Album artist"),
        "date" => String::from("Released"),
        "track" => String::from("Track"),
        "disc" => String::from("Disc"),
        "composer" => String::from("Composer"),
        "comment" => String::from("Comment"),
        "performer" => String::from("Performer"),
        "publisher" => String::from("Publisher"),
        "copyright" => String::from("Copyright"),
        "artist" => String::from("Artist"),
        "album" => String::from("Album"),
        "genre" => String::from("Genre"),
        "title" => String::from("Title"),
        other => {
            let mut chars = other.replace('_', " ").chars().collect::<Vec<_>>();
            if let Some(first) = chars.first_mut() {
                *first = first.to_ascii_uppercase();
            }
            chars.into_iter().collect()
        }
    }
}

fn metadata_rows(
    source: &SourceInfo,
    author_line: &str,
    display_title: &str,
    max_value_units: usize,
) -> Vec<(String, String)> {
    let mut entries: Vec<(&str, String)> = Vec::new();
    for key in TAG_PRIORITY {
        if let Some((_, value)) = source.tags.iter().find(|(name, _)| name == key) {
            entries.push((key, value.clone()));
        }
    }
    for (key, value) in &source.tags {
        if !TAG_PRIORITY.contains(&key.as_str()) && !is_technical_tag(key) {
            entries.push((key.as_str(), value.clone()));
        }
    }

    let mut rows = Vec::new();
    for (key, value) in entries {
        if rows.len() >= META_MAX_ROWS {
            break;
        }
        let value = sanitize_text(&value);
        if value.is_empty() {
            continue;
        }
        if key == "title" && (value == display_title || value.eq_ignore_ascii_case(display_title)) {
            continue;
        }
        if (key == "artist" || key == "album_artist")
            && !author_line.is_empty()
            && value.eq_ignore_ascii_case(author_line)
        {
            continue;
        }
        let value = truncate_units(&value, max_value_units);
        if value.is_empty() {
            continue;
        }
        rows.push((tag_label(key), value));
    }
    rows
}

fn format_clock(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

fn format_metadata_date(value: &str) -> String {
    let digits = value
        .chars()
        .filter(char::is_ascii_digit)
        .collect::<String>();
    match digits.len() {
        8.. => format!("{}-{}-{}", &digits[0..4], &digits[4..6], &digits[6..8]),
        4 => digits,
        _ => value.trim().to_string(),
    }
}

fn add_metadata_tag(tags: &mut Vec<(String, String)>, key: &str, value: Option<&str>) {
    let Some(value) = value.map(sanitize_text).filter(|value| !value.is_empty()) else {
        return;
    };
    if !tags.iter().any(|(existing, _)| existing == key) {
        tags.push((key.to_string(), value));
    }
}

fn parse_youtube_metadata(value: &serde_json::Value) -> Vec<(String, String)> {
    let pick = |key: &str| {
        value
            .get(key)
            .and_then(|item| item.as_str())
            .map(str::trim)
            .filter(|item| !item.is_empty())
    };
    let mut tags = Vec::new();
    add_metadata_tag(
        &mut tags,
        "artist",
        pick("artist")
            .or_else(|| pick("uploader"))
            .or_else(|| pick("channel")),
    );
    add_metadata_tag(&mut tags, "album", pick("album"));
    add_metadata_tag(&mut tags, "track", pick("track"));
    add_metadata_tag(&mut tags, "genre", pick("genre"));
    add_metadata_tag(&mut tags, "comment", pick("comment"));
    add_metadata_tag(&mut tags, "copyright", pick("copyright"));

    let date = pick("release_date").map(format_metadata_date).or_else(|| {
        value
            .get("release_year")
            .and_then(|item| item.as_u64())
            .map(|year| year.to_string())
    });
    add_metadata_tag(&mut tags, "date", date.as_deref());

    if let Some(description) = pick("description") {
        for line in description.lines().map(str::trim) {
            if let Some(rest) = line.strip_prefix("Released on:") {
                add_metadata_tag(&mut tags, "date", Some(&format_metadata_date(rest)));
            } else if let Some(rest) = line.strip_prefix('℗') {
                add_metadata_tag(&mut tags, "copyright", Some(&format!("℗{rest}")));
            } else if let Some(rest) = line.strip_prefix("Composer Lyricist:") {
                add_metadata_tag(&mut tags, "composer", Some(rest));
            } else if let Some(rest) = line.strip_prefix("Composer:") {
                add_metadata_tag(&mut tags, "composer", Some(rest));
            } else if let Some(rest) = line.strip_prefix("Lyricist:") {
                add_metadata_tag(&mut tags, "lyricist", Some(rest));
            } else if let Some(rest) = line.strip_prefix("Songwriter:") {
                add_metadata_tag(&mut tags, "composer", Some(rest));
            } else if let Some(rest) = line.strip_prefix("Producer:") {
                add_metadata_tag(&mut tags, "producer", Some(rest));
            }
        }
    }
    tags
}

async fn fetch_youtube_metadata(video_id: &str) -> Result<Vec<(String, String)>, String> {
    let bin = std::env::var("YTDLP_BIN").unwrap_or_else(|_| String::from("yt-dlp"));
    let mut cmd = tokio::process::Command::new(bin);
    cmd.kill_on_drop(true)
        .arg("--skip-download")
        .arg("--dump-single-json")
        .arg("--no-warnings")
        .arg("--no-progress")
        .arg("--no-playlist")
        .arg("--socket-timeout")
        .arg("10")
        .arg("--retries")
        .arg("1");
    if let Ok(proxy) = std::env::var("YTDLP_PROXY") {
        if !proxy.trim().is_empty() {
            cmd.arg("--proxy").arg(proxy);
        }
    }
    if let Ok(pot) = std::env::var("YTDLP_POT_BASE_URL") {
        if !pot.trim().is_empty() {
            cmd.arg("--extractor-args")
                .arg(format!("youtubepot-bgutilhttp:base_url={pot}"));
        }
    }
    if let Ok(runtime) = std::env::var("YTDLP_JS_RUNTIME") {
        if !runtime.trim().is_empty() {
            cmd.arg("--js-runtimes").arg(runtime);
        }
    }
    if let Ok(cookies) = std::env::var("YTDLP_COOKIES") {
        if FsPath::new(&cookies).is_file() {
            cmd.arg("--cookies").arg(cookies);
        }
    }
    cmd.arg("--remote-components")
        .arg("ejs:github")
        .arg(format!("https://www.youtube.com/watch?v={video_id}"));

    let output = tokio::time::timeout(Duration::from_secs(25), cmd.output())
        .await
        .map_err(|_| String::from("yt-dlp metadata timed out"))?
        .map_err(|error| format!("run yt-dlp metadata: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "yt-dlp metadata failed: {}",
            stderr.chars().take(400).collect::<String>()
        ));
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("parse yt-dlp metadata: {error}"))?;
    Ok(parse_youtube_metadata(&value))
}

async fn enrich_source_metadata(source: &mut SourceInfo, thumb_key: Option<&str>) {
    let Some(video_id) = thumb_key.and_then(|key| key.strip_prefix("yt:")) else {
        return;
    };
    match fetch_youtube_metadata(video_id).await {
        Ok(tags) => {
            for (key, value) in tags {
                if !source.tags.iter().any(|(existing, _)| existing == &key) {
                    source.tags.push((key, value));
                }
            }
        }
        Err(error) => {
            tracing::debug!("YouTube metadata unavailable for {video_id}: {error}");
        }
    }
}

fn drawtext(font: &FsPath, text: &FsPath, color: &str, size: u32, x: u32, y: u32) -> String {
    format!(
        "drawtext=fontfile={}:textfile={}:expansion=none:fontcolor={color}:fontsize={size}:x={x}:y={y}",
        escape_filter_path(font),
        escape_filter_path(text)
    )
}

async fn run_ffmpeg(ffmpeg: &str, args: Vec<OsString>) -> Result<(), String> {
    let mut cmd = tokio::process::Command::new(ffmpeg);
    cmd.kill_on_drop(true)
        .arg("-nostdin")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-y")
        .args(args);
    let output = cmd
        .output()
        .await
        .map_err(|error| format!("run ffmpeg: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(format!(
        "ffmpeg failed: {}",
        stderr.chars().take(800).collect::<String>()
    ))
}

async fn download_source_thumbnail(video_id: &str, out: &FsPath) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(THUMBNAIL_FETCH_TIMEOUT)
        .build()
        .map_err(|error| format!("build thumbnail client: {error}"))?;
    for quality in ["maxresdefault", "hqdefault"] {
        let url = format!("https://i.ytimg.com/vi/{video_id}/{quality}.jpg");
        let Ok(response) = client.get(&url).send().await else {
            continue;
        };
        if !response.status().is_success() {
            continue;
        }
        if response
            .content_length()
            .is_some_and(|length| length > THUMBNAIL_MAX_BYTES)
        {
            continue;
        }
        let Ok(bytes) = response.bytes().await else {
            continue;
        };
        if bytes.len() < 1024 || bytes.len() as u64 > THUMBNAIL_MAX_BYTES {
            continue;
        }
        tokio::fs::write(out, &bytes)
            .await
            .map_err(|error| format!("write source thumbnail: {error}"))?;
        return Ok(());
    }
    Err(String::from("source thumbnail unavailable"))
}

async fn extract_cover(ffmpeg: &str, input: &FsPath, out: &FsPath) -> Result<(), String> {
    let args = vec![
        OsString::from("-i"),
        input.as_os_str().to_owned(),
        OsString::from("-map"),
        OsString::from("0:v:0"),
        OsString::from("-frames:v"),
        OsString::from("1"),
        OsString::from("-q:v"),
        OsString::from("2"),
        out.as_os_str().to_owned(),
    ];
    run_ffmpeg(ffmpeg, args).await
}

async fn render_thumbnail(ffmpeg: &str, image: &FsPath, out: &FsPath) -> Result<(), String> {
    let filter = format!(
        "[0:v]scale={THUMB_SIZE}:{THUMB_SIZE}:force_original_aspect_ratio=increase:flags=lanczos,crop={THUMB_SIZE}:{THUMB_SIZE},format=rgb24[out]"
    );
    let args = vec![
        OsString::from("-i"),
        image.as_os_str().to_owned(),
        OsString::from("-filter_complex"),
        OsString::from(filter),
        OsString::from("-map"),
        OsString::from("[out]"),
        OsString::from("-frames:v"),
        OsString::from("1"),
        OsString::from("-q:v"),
        OsString::from("2"),
        out.as_os_str().to_owned(),
    ];
    run_ffmpeg(ffmpeg, args).await
}

async fn make_placeholder_thumbnail(
    ffmpeg: &str,
    logo: &FsPath,
    out: &FsPath,
) -> Result<(), String> {
    let filter = String::from(
        "[1:v]scale=112:112:flags=neighbor[lg];[0:v][lg]overlay=(W-w)/2:(H-h)/2,format=rgb24[out]",
    );
    let args = vec![
        OsString::from("-f"),
        OsString::from("lavfi"),
        OsString::from("-i"),
        OsString::from(format!("color=c=0x1F1A17:s={THUMB_SIZE}x{THUMB_SIZE}")),
        OsString::from("-i"),
        logo.as_os_str().to_owned(),
        OsString::from("-filter_complex"),
        OsString::from(filter),
        OsString::from("-map"),
        OsString::from("[out]"),
        OsString::from("-frames:v"),
        OsString::from("1"),
        out.as_os_str().to_owned(),
    ];
    run_ffmpeg(ffmpeg, args).await
}

async fn convert_poster(ffmpeg: &str, background: &FsPath, out: &FsPath) -> Result<(), String> {
    let args = vec![
        OsString::from("-i"),
        background.as_os_str().to_owned(),
        OsString::from("-frames:v"),
        OsString::from("1"),
        OsString::from("-q:v"),
        OsString::from("3"),
        out.as_os_str().to_owned(),
    ];
    run_ffmpeg(ffmpeg, args).await
}

async fn make_video(
    ffmpeg: &str,
    background: &FsPath,
    input: &FsPath,
    out: &FsPath,
    font: &FsPath,
    width: u32,
    height: u32,
    play_seconds: f64,
    video_seconds: f64,
    total_label: &str,
    copied_audio: bool,
) -> Result<(), String> {
    let total_label = total_label.replace(':', "\\:");
    let time_y = height.saturating_sub(TIME_Y_FROM_BOTTOM);
    let progress_y = height.saturating_sub(PROGRESS_BOTTOM_MARGIN + PROGRESS_HEIGHT);
    let progress_width = width.saturating_sub(PROGRESS_SIDE_MARGIN * 2);
    let (orange_r, orange_g, orange_b) = BRAND_ORANGE_RGB;
    let filter = format!(
        "[2:v]format=rgba,geq=r={orange_r}:g={orange_g}:b={orange_b}:a='if(lt(X,{progress_width}*min(1,T/{play_seconds:.6})),255,0)'[fill];\
         [0:v]drawtext=fontfile={}:text='%{{pts\\:gmtime\\:0\\:%M\\\\\\:%S}} / {total_label}':fontcolor={BRAND_MUTED}:fontsize={TIME_FONT_SIZE}:borderw=1:bordercolor=0x000000:x={THUMB_X}:y={time_y}[base];\
         [base][fill]overlay=x={PROGRESS_SIDE_MARGIN}:y={progress_y}:format=rgb,format=yuv420p[v]",
        escape_filter_path(font)
    );
    let mut args = vec![
        OsString::from("-loop"),
        OsString::from("1"),
        OsString::from("-framerate"),
        OsString::from(VIDEO_FPS.to_string()),
        OsString::from("-t"),
        OsString::from(format!("{video_seconds:.3}")),
        OsString::from("-i"),
        background.as_os_str().to_owned(),
        OsString::from("-i"),
        input.as_os_str().to_owned(),
        OsString::from("-f"),
        OsString::from("lavfi"),
        OsString::from("-i"),
        OsString::from(format!(
            "color=c=black:s={progress_width}x{PROGRESS_HEIGHT}:r={VIDEO_FPS}"
        )),
        OsString::from("-filter_complex"),
        OsString::from(filter),
        OsString::from("-map"),
        OsString::from("[v]"),
        OsString::from("-map"),
        OsString::from("1:a:0"),
        OsString::from("-c:v"),
        OsString::from("libx264"),
        OsString::from("-preset"),
        OsString::from("veryfast"),
        OsString::from("-crf"),
        OsString::from("18"),
        OsString::from("-tune"),
        OsString::from("stillimage"),
        OsString::from("-profile:v"),
        OsString::from("baseline"),
        OsString::from("-level"),
        OsString::from("3.1"),
        OsString::from("-g"),
        OsString::from("10"),
        OsString::from("-keyint_min"),
        OsString::from("2"),
        OsString::from("-sc_threshold"),
        OsString::from("0"),
        OsString::from("-r"),
        OsString::from(VIDEO_FPS.to_string()),
        OsString::from("-pix_fmt"),
        OsString::from("yuv420p"),
    ];
    if copied_audio {
        args.extend([OsString::from("-c:a"), OsString::from("copy")]);
    } else {
        args.extend([
            OsString::from("-c:a"),
            OsString::from("aac"),
            OsString::from("-b:a"),
            OsString::from("320k"),
            OsString::from("-ar"),
            OsString::from("48000"),
            OsString::from("-ac"),
            OsString::from("2"),
        ]);
    }
    args.extend([
        OsString::from("-movflags"),
        OsString::from("+faststart"),
        OsString::from("-shortest"),
        OsString::from("-t"),
        OsString::from(format!("{video_seconds:.3}")),
        OsString::from("-threads"),
        OsString::from("1"),
        out.as_os_str().to_owned(),
    ]);
    run_ffmpeg(ffmpeg, args).await
}

async fn generate(
    state: &Arc<AppState>,
    id: &str,
    meta: &storage::FileMetadata,
    title: Option<&str>,
    author: Option<&str>,
    thumb_key: Option<&str>,
) -> Result<GeneratedAudioEmbed, String> {
    let settings = &state.audio_embed;
    let dir = settings.cache_dir().clone();
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("create cache dir: {e}"))?;

    let token = format!(
        "{}-{}",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let source_temp = dir.join(format!(".src-{id}-{token}.{}", meta.extension));
    let video_temp = dir.join(format!(".embed-{id}-{token}.mp4"));
    let poster_temp = dir.join(format!(".poster-{id}-{token}.jpg"));
    let meta_temp = dir.join(format!(".meta-{id}-{token}.json"));
    let logo_temp = dir.join(format!(".logo-{id}-{token}.png"));
    let title_font_temp = dir.join(format!(".font-title-{id}-{token}.ttf"));
    let body_font_temp = dir.join(format!(".font-body-{id}-{token}.ttf"));
    let time_font_temp = dir.join(format!(".font-time-{id}-{token}.ttf"));
    let thumb_temp = dir.join(format!(".thumb-{id}-{token}.jpg"));
    let thumb_source_temp = dir.join(format!(".thumbsrc-{id}-{token}.jpg"));
    let cover_temp = dir.join(format!(".cover-{id}-{token}.jpg"));
    let background_temp = dir.join(format!(".bg-{id}-{token}.png"));
    let text_temps = (0..20)
        .map(|index| dir.join(format!(".text-{id}-{token}-{index}.txt")))
        .collect::<Vec<_>>();
    let mut cleanup = vec![
        source_temp.clone(),
        video_temp.clone(),
        poster_temp.clone(),
        meta_temp.clone(),
        logo_temp.clone(),
        title_font_temp.clone(),
        body_font_temp.clone(),
        time_font_temp.clone(),
        thumb_temp.clone(),
        thumb_source_temp.clone(),
        cover_temp.clone(),
        background_temp.clone(),
    ];
    cleanup.extend(text_temps.iter().cloned());
    let _cleanup = TempCleanup::new(cleanup);

    let display_title = sanitize_text(title.unwrap_or(""));
    let raw_author = author_for_video(author.unwrap_or(""));

    tokio::fs::write(&logo_temp, LOGO_PNG)
        .await
        .map_err(|e| format!("write logo asset: {e}"))?;
    tokio::fs::write(&title_font_temp, TITLE_FONT)
        .await
        .map_err(|e| format!("write title font asset: {e}"))?;

    let input = resolve_input(state, id, &source_temp).await?;
    let input_path = match &input {
        Input::Local(path) | Input::Temp(path) => path,
    };

    let mut source = probe_source(settings.ffmpeg_bin(), input_path).await?;
    enrich_source_metadata(&mut source, thumb_key).await;
    if !source.duration.is_finite() || source.duration <= 0.0 {
        return Err(String::from("could not determine source duration"));
    }
    let limit = settings.max_seconds();
    let play_seconds = if limit == 0 {
        source.duration
    } else {
        source.duration.min(limit as f64)
    };
    if play_seconds <= 0.0 {
        return Err(String::from("source duration is empty"));
    }
    let capped = limit > 0 && source.duration > play_seconds + 0.000_1;
    let video_seconds = if capped {
        play_seconds
    } else {
        play_seconds + FRAME_DURATION_TOLERANCE
    };
    let copied_audio = matches!(source.audio_codec.as_str(), "mp3" | "aac");

    let width = settings.width();
    let height = settings.height();
    let text_width = width.saturating_sub(TEXT_X + 32).max(64);
    let title_units = (text_width / 11).max(16) as usize;
    let author_units = (text_width / 9).max(20) as usize;
    let meta_width = width.saturating_sub(META_VALUE_X + 24);
    let meta_units = (meta_width / 9).max(16) as usize;
    let title_lines = wrap_text(&display_title, title_units, 2);
    let fallback_author = source
        .tags
        .iter()
        .find(|(key, _)| key == "artist")
        .map(|(_, value)| sanitize_text(value))
        .unwrap_or_default();
    let author_line = if raw_author.is_empty() {
        fallback_author
    } else {
        raw_author.clone()
    };
    let author_line = truncate_units(&author_line, author_units);
    let meta_rows = metadata_rows(&source, &author_line, &display_title, meta_units);

    for (index, line) in title_lines.iter().enumerate() {
        if index >= 2 {
            break;
        }
        tokio::fs::write(&text_temps[index], line.as_bytes())
            .await
            .map_err(|e| format!("write title text: {e}"))?;
    }
    tokio::fs::write(&text_temps[2], author_line.as_bytes())
        .await
        .map_err(|e| format!("write author text: {e}"))?;
    for (index, (label, value)) in meta_rows.iter().enumerate() {
        let label_slot = 3 + index * 2;
        let value_slot = label_slot + 1;
        if value_slot >= text_temps.len() {
            break;
        }
        tokio::fs::write(&text_temps[label_slot], label.as_bytes())
            .await
            .map_err(|e| format!("write metadata label: {e}"))?;
        tokio::fs::write(&text_temps[value_slot], value.as_bytes())
            .await
            .map_err(|e| format!("write metadata value: {e}"))?;
    }

    let needs_wide = title_lines
        .iter()
        .chain(std::iter::once(&author_line))
        .chain(meta_rows.iter().flat_map(|(label, value)| [label, value]))
        .any(|line| title_needs_wide_font(line));
    let body_font = if needs_wide { WIDE_FONT } else { BODY_FONT };
    tokio::fs::write(&body_font_temp, body_font)
        .await
        .map_err(|e| format!("write body font asset: {e}"))?;
    tokio::fs::write(&time_font_temp, BODY_FONT)
        .await
        .map_err(|e| format!("write time font asset: {e}"))?;

    let mut thumbnail_image = None;
    if let Some(video_id) = thumb_key.and_then(|key| key.strip_prefix("yt:")) {
        if download_source_thumbnail(video_id, &thumb_source_temp)
            .await
            .is_ok()
        {
            thumbnail_image = Some(thumb_source_temp.as_path());
        }
    }
    if thumbnail_image.is_none()
        && source.has_cover
        && extract_cover(settings.ffmpeg_bin(), input_path, &cover_temp)
            .await
            .is_ok()
    {
        thumbnail_image = Some(cover_temp.as_path());
    }
    if let Some(image) = thumbnail_image {
        if render_thumbnail(settings.ffmpeg_bin(), image, &thumb_temp)
            .await
            .is_err()
        {
            make_placeholder_thumbnail(settings.ffmpeg_bin(), &logo_temp, &thumb_temp).await?;
        }
    } else {
        make_placeholder_thumbnail(settings.ffmpeg_bin(), &logo_temp, &thumb_temp).await?;
    }

    let mut text_entries: Vec<(PathBuf, u32, &'static str, u32, u32)> = Vec::new();
    let mut y = 96u32;
    for (index, line) in title_lines.iter().enumerate() {
        if line.is_empty() || y + 22 > height.saturating_sub(20) {
            continue;
        }
        text_entries.push((text_temps[index].clone(), 22, BRAND_CREAM, y, TEXT_X));
        y += 28;
    }
    if !author_line.is_empty() {
        y += 6;
        if y + 18 <= height.saturating_sub(20) {
            text_entries.push((text_temps[2].clone(), 18, BRAND_ORANGE, y, TEXT_X));
            y += 24;
        }
    }
    y += 8;
    for (index, _) in meta_rows.iter().enumerate() {
        let label_slot = 3 + index * 2;
        let value_slot = label_slot + 1;
        if value_slot >= text_temps.len() || y + META_VALUE_SIZE > height.saturating_sub(8) {
            break;
        }
        text_entries.push((
            text_temps[label_slot].clone(),
            META_LABEL_SIZE,
            BRAND_MUTED,
            y,
            TEXT_X,
        ));
        text_entries.push((
            text_temps[value_slot].clone(),
            META_VALUE_SIZE,
            BRAND_CREAM,
            y,
            META_VALUE_X,
        ));
        y += META_ROW_HEIGHT;
    }

    let mut filters = vec![
        String::from("[1:v]scale=40:40:flags=neighbor[logo]"),
        String::from("[0:v][logo]overlay=32:28[brand]"),
        format!(
            "[brand]drawtext=fontfile={}:text='JUICEBOX²':fontcolor={BRAND_ORANGE}:fontsize=30:borderw=2:bordercolor=0x000000:x=84:y=30[word]",
            escape_filter_path(&title_font_temp)
        ),
        format!("[word][2:v]overlay={THUMB_X}:{THUMB_Y}[thumb]"),
        format!(
            "[thumb]drawbox=x={}:y={}:w={}:h={}:color={BRAND_BORDER}:t=2[border]",
            THUMB_X.saturating_sub(2),
            THUMB_Y.saturating_sub(2),
            THUMB_SIZE + 4,
            THUMB_SIZE + 4
        ),
        format!(
            "[border]drawbox=x={PROGRESS_SIDE_MARGIN}:y={}:w={}:h={PROGRESS_HEIGHT}:color={PROGRESS_TRACK}:t=fill[track]",
            height.saturating_sub(PROGRESS_BOTTOM_MARGIN + PROGRESS_HEIGHT),
            width.saturating_sub(PROGRESS_SIDE_MARGIN * 2)
        ),
    ];
    let mut last = String::from("track");
    for (index, (path, size, color, text_y, text_x)) in text_entries.iter().enumerate() {
        let label = format!("text{index}");
        filters.push(format!(
            "[{last}]{}[{label}]",
            drawtext(&body_font_temp, path, color, *size, *text_x, *text_y)
        ));
        last = label;
    }
    filters.push(format!("[{last}]format=rgb24[out]"));
    let filter = filters.join(";");
    let background_args = vec![
        OsString::from("-f"),
        OsString::from("lavfi"),
        OsString::from("-i"),
        OsString::from(format!("color=c={BACKGROUND}:s={width}x{height}")),
        OsString::from("-i"),
        logo_temp.as_os_str().to_owned(),
        OsString::from("-i"),
        thumb_temp.as_os_str().to_owned(),
        OsString::from("-filter_complex"),
        OsString::from(filter),
        OsString::from("-map"),
        OsString::from("[out]"),
        OsString::from("-frames:v"),
        OsString::from("1"),
        OsString::from("-pix_fmt"),
        OsString::from("rgb24"),
        background_temp.as_os_str().to_owned(),
    ];
    run_ffmpeg(settings.ffmpeg_bin(), background_args).await?;

    let total_label = format_clock(play_seconds);
    make_video(
        settings.ffmpeg_bin(),
        &background_temp,
        input_path,
        &video_temp,
        &time_font_temp,
        width,
        height,
        play_seconds,
        video_seconds,
        &total_label,
        copied_audio,
    )
    .await?;

    let video_meta = tokio::fs::metadata(&video_temp)
        .await
        .map_err(|e| format!("stat generated video: {e}"))?;
    if video_meta.len() == 0 {
        return Err(String::from("ffmpeg produced an empty video"));
    }

    convert_poster(settings.ffmpeg_bin(), &background_temp, &poster_temp).await?;

    let seconds = play_seconds.ceil() as u64;
    let cache_meta = CacheMeta {
        version: CACHE_VERSION,
        etag: meta.etag.clone(),
        size: meta.size,
        extension: meta.extension.clone(),
        title: display_title,
        author: raw_author,
        thumb_key: thumb_key.unwrap_or("").to_string(),
        seconds,
        limit,
        width,
        height,
    };
    let serialized =
        serde_json::to_vec(&cache_meta).map_err(|e| format!("serialize cache meta: {e}"))?;
    tokio::fs::write(&meta_temp, serialized)
        .await
        .map_err(|e| format!("write cache meta: {e}"))?;

    let paths = cache_paths(state, id);
    tokio::fs::rename(&video_temp, &paths.video)
        .await
        .map_err(|e| format!("publish video: {e}"))?;
    tokio::fs::rename(&poster_temp, &paths.poster)
        .await
        .map_err(|e| format!("publish poster: {e}"))?;
    tokio::fs::rename(&meta_temp, &paths.meta)
        .await
        .map_err(|e| format!("publish cache meta: {e}"))?;

    tracing::info!(
        "audio embed source {id}: codec={} copied={copied_audio} duration={:.3}s video={:.3}s",
        source.audio_codec,
        source.duration,
        video_seconds
    );

    Ok(GeneratedAudioEmbed {
        info: AudioEmbedInfo {
            video_path: format!("/e/{id}.mp4"),
            poster_path: format!("/e/{id}.jpg"),
            width,
            height,
        },
        video_file: paths.video,
        poster_file: paths.poster,
        seconds,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmbedKind {
    Video,
    Poster,
}

fn strip_media_version(path: &str) -> &str {
    let Some((prefix, rest)) = path.split_once('/') else {
        return path;
    };
    let Some(version) = prefix
        .strip_prefix('v')
        .and_then(|part| part.split_once('-'))
    else {
        return path;
    };
    if !version.0.is_empty() && version.0.chars().all(|c| c.is_ascii_digit()) {
        rest
    } else {
        path
    }
}

fn parse_embed_path(path: &str) -> Option<(String, EmbedKind)> {
    let path = strip_media_version(path);
    let (id, ext) = path.rsplit_once('.')?;
    if !is_valid_id(id) {
        return None;
    }
    let kind = match ext {
        "mp4" => EmbedKind::Video,
        "jpg" => EmbedKind::Poster,
        _ => return None,
    };
    Some((id.to_string(), kind))
}

fn media_not_found() -> Response<Body> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::empty())
        .expect("static media error response is valid")
}

pub async fn audio_embed_media(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(path): Path<String>,
) -> Result<Response<Body>, JuicehostError> {
    let Some((id, kind)) = parse_embed_path(&path) else {
        return Ok(media_not_found());
    };
    let meta = match state.storage.stat(&id).await {
        Ok(meta) => meta,
        Err(StorageError::NotFound | StorageError::Frozen) => {
            if let Some(response) =
                super::route::remote_redirect(&state, &headers, &id, "e", &path).await
            {
                return Ok(response);
            }
            return Ok(media_not_found());
        }
        Err(_) => return Err(JuicehostError::Internal),
    };
    if !storage::guess_mime(&meta.extension).starts_with("audio/") {
        return Ok(media_not_found());
    }

    match gate_decision(&state, &id, ShellMode::Preview, None, false).await {
        GateDecision::Respond(_) | GateDecision::Ciphertext => return Ok(media_not_found()),
        GateDecision::HotServe { .. } | GateDecision::ServeLocal { .. } => {}
    }

    let Some(generated) = ensure_video(
        Arc::clone(&state),
        id.clone(),
        meta.clone(),
        None,
        None,
        None,
    )
    .await
    else {
        return Ok(media_not_found());
    };
    let file_path = match kind {
        EmbedKind::Video => &generated.video_file,
        EmbedKind::Poster => &generated.poster_file,
    };
    let data = match tokio::fs::read(file_path).await {
        Ok(data) => Bytes::from(data),
        Err(error) => {
            tracing::warn!("audio embed cache read failed for {id}: {error}");
            return Ok(media_not_found());
        }
    };
    let total = data.len() as u64;
    if total == 0 {
        return Ok(media_not_found());
    }
    let content_type = match kind {
        EmbedKind::Video => "video/mp4",
        EmbedKind::Poster => "image/jpeg",
    };
    let etag = format!(
        "\"{}-{}-{}\"",
        id,
        meta.etag.trim_matches('"'),
        match kind {
            EmbedKind::Video => "v",
            EmbedKind::Poster => "p",
        }
    );
    let cache_control = embed_cache_control(&state, &id).await;
    let cache_tag = (cache_control != "no-store").then(|| format!("juicebox:{id}"));
    if let Some(value) = headers.get(header::IF_NONE_MATCH)
        && let Ok(value) = value.to_str()
        && value
            .split(',')
            .any(|candidate| candidate.trim().trim_matches('"') == etag.trim_matches('"'))
    {
        let mut builder = Response::builder()
            .status(StatusCode::NOT_MODIFIED)
            .header(header::ETAG, &etag)
            .header(header::CACHE_CONTROL, &cache_control);
        if let Some(tag) = &cache_tag {
            builder = builder.header("cache-tag", tag.as_str());
        }
        return builder
            .body(Body::empty())
            .map_err(|_| JuicehostError::Internal);
    }

    let mut builder = Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .header(header::ETAG, &etag)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CACHE_CONTROL, &cache_control)
        .header(header::CONTENT_DISPOSITION, "inline");
    if let Some(tag) = &cache_tag {
        builder = builder.header("cache-tag", tag.as_str());
    }

    if let Some(range) = headers.get(header::RANGE).and_then(|v| v.to_str().ok()) {
        match parse_range(range, total, state.max_range_response_bytes) {
            RangeResult::Satisfiable(start, end) => {
                let slice = data.slice(start as usize..=end as usize);
                return builder
                    .status(StatusCode::PARTIAL_CONTENT)
                    .header(
                        header::CONTENT_RANGE,
                        format!("bytes {start}-{end}/{total}"),
                    )
                    .header(header::CONTENT_LENGTH, end - start + 1)
                    .body(Body::from(slice))
                    .map_err(|_| JuicehostError::Internal);
            }
            RangeResult::Unsatisfiable => {
                return builder
                    .status(StatusCode::RANGE_NOT_SATISFIABLE)
                    .header(header::CONTENT_RANGE, format!("bytes */{total}"))
                    .body(Body::empty())
                    .map_err(|_| JuicehostError::Internal);
            }
            RangeResult::Ignore => {}
        }
    }

    builder
        .status(StatusCode::OK)
        .header(header::CONTENT_LENGTH, total)
        .body(Body::from(data))
        .map_err(|_| JuicehostError::Internal)
}

async fn embed_cache_control(state: &AppState, id: &str) -> String {
    let ttl = remaining_ttl_secs(state, id).await;
    file_cache_control_value(state, ttl)
}

pub fn spawn_pruner(state: Arc<AppState>) {
    if !state.audio_embed.enabled() {
        return;
    }
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60 * 60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            prune_cache(&state).await;
        }
    });
}

#[derive(Default)]
struct CacheGroup {
    files: Vec<PathBuf>,
    bytes: u64,
    newest: Option<SystemTime>,
}

async fn prune_cache(state: &AppState) {
    let dir = state.audio_embed.cache_dir();
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(_) => return,
    };
    let now = SystemTime::now();
    let mut groups: HashMap<String, CacheGroup> = HashMap::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        let path = entry.path();
        let metadata = match entry.metadata().await {
            Ok(metadata) if metadata.is_file() => metadata,
            _ => continue,
        };
        let modified = metadata.modified().unwrap_or(now);
        if name.starts_with('.') {
            if now.duration_since(modified).unwrap_or_default() > TEMP_MAX_AGE {
                let _ = tokio::fs::remove_file(&path).await;
            }
            continue;
        }
        let stem = name
            .rsplit_once('.')
            .map_or_else(|| name.clone(), |(stem, _)| stem.to_string());
        let group = groups.entry(stem).or_default();
        group.files.push(path);
        group.bytes = group.bytes.saturating_add(metadata.len());
        group.newest = Some(group.newest.map_or(modified, |newest| newest.max(modified)));
    }

    let mut expired = 0usize;
    let mut evicted = 0usize;
    let mut freed = 0u64;
    let mut live: Vec<(String, CacheGroup)> = Vec::with_capacity(groups.len());
    for (id, group) in groups {
        let too_old = group
            .newest
            .and_then(|newest| now.duration_since(newest).ok())
            .is_some_and(|age| age > CACHE_MAX_AGE);
        if too_old {
            for path in &group.files {
                let _ = tokio::fs::remove_file(path).await;
            }
            expired += 1;
            freed = freed.saturating_add(group.bytes);
        } else {
            live.push((id, group));
        }
    }

    let cap = state.audio_embed.cache_max_bytes();
    let mut total = live.iter().map(|(_, group)| group.bytes).sum::<u64>();
    if total > cap {
        live.sort_by_key(|(_, group)| group.newest);
        let target = cap.saturating_mul(4) / 5;
        for (_, group) in live {
            if total <= target {
                break;
            }
            for path in &group.files {
                let _ = tokio::fs::remove_file(path).await;
            }
            total = total.saturating_sub(group.bytes);
            freed = freed.saturating_add(group.bytes);
            evicted += 1;
        }
    }

    state
        .audio_embed_locks
        .retain(|_, lock| Arc::strong_count(lock) > 1);

    if expired > 0 || evicted > 0 {
        tracing::info!(
            "audio embed cache pruned: {expired} expired, {evicted} evicted, {} bytes freed",
            freed
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_media_paths() {
        assert_eq!(
            parse_embed_path("abc123.mp4"),
            Some(("abc123".to_string(), EmbedKind::Video))
        );
        assert_eq!(
            parse_embed_path("abc_123.jpg"),
            Some(("abc_123".to_string(), EmbedKind::Poster))
        );
        assert_eq!(parse_embed_path("abc123.webm"), None);
        assert_eq!(parse_embed_path("../abc.mp4"), None);
        assert_eq!(parse_embed_path("abc/def.mp4"), None);
        assert_eq!(
            parse_embed_path("v12-23fc196adb84b43d/abc123.mp4"),
            Some(("abc123".to_string(), EmbedKind::Video))
        );
        assert_eq!(
            parse_embed_path("v12-23fc196adb84b43d/abc_123.jpg"),
            Some(("abc_123".to_string(), EmbedKind::Poster))
        );
    }

    #[test]
    fn cache_meta_roundtrip() {
        let meta = CacheMeta {
            version: CACHE_VERSION,
            etag: "etag".into(),
            size: 42,
            extension: "mp3".into(),
            title: "Midnight Drive".into(),
            author: "Someone".into(),
            thumb_key: "yt:dQw4w9WgXcQ".into(),
            seconds: 193,
            limit: 0,
            width: 640,
            height: 360,
        };
        let encoded = serde_json::to_vec(&meta).expect("serialize");
        let decoded: CacheMeta = serde_json::from_slice(&encoded).expect("deserialize");
        assert_eq!(decoded.etag, "etag");
        assert_eq!(decoded.size, 42);
        assert_eq!(decoded.title, "Midnight Drive");
        assert_eq!(decoded.author, "Someone");
        assert_eq!(decoded.thumb_key, "yt:dQw4w9WgXcQ");
        assert_eq!(decoded.limit, 0);
    }

    #[test]
    fn youtube_video_id_from_cobalt_filename() {
        assert_eq!(
            youtube_video_id_from_filename(
                "なかなおり ⧸ いよわ feat. 初音ミク（Last Rally ⧸ Iyowa feat. Hatsune Miku）【YouTube Music Weekend 12.0】-vrnKlp_epOQ.mp3"
            ),
            Some(String::from("vrnKlp_epOQ"))
        );
        assert_eq!(
            youtube_video_id_from_filename("Unlucky Dream-cJq_C3QGn44.mp3"),
            Some(String::from("cJq_C3QGn44"))
        );
        assert_eq!(
            youtube_video_id_from_filename("photo-2106825381840093184.mp3"),
            None
        );
        assert_eq!(youtube_video_id_from_filename("song.mp3"), None);
    }

    #[test]
    fn video_title_strips_extension_and_collapses_space() {
        assert_eq!(
            title_for_video("  Midnight   Drive.MP3 \n", "mp3"),
            "Midnight Drive"
        );
        assert_eq!(title_for_video("already clean", "wav"), "already clean");
    }

    #[test]
    fn video_title_truncates_to_char_limit() {
        let raw = "a".repeat(TITLE_MAX_CHARS + 12);
        let title = title_for_video(&raw, "flac");
        assert_eq!(title.chars().count(), TITLE_MAX_CHARS + 3);
        assert!(title.ends_with("..."));
    }

    #[test]
    fn empty_title_stays_empty() {
        assert_eq!(title_for_video("", "mp3"), "");
        assert_eq!(title_for_video(".mp3", "mp3"), "");
    }

    #[test]
    fn wide_title_uses_tighter_limit() {
        let raw = "な".repeat(TITLE_MAX_CHARS + 8);
        let title = title_for_video(&raw, "mp3");
        assert_eq!(title.chars().count(), TITLE_MAX_CHARS_WIDE + 3);
        assert!(title.ends_with("..."));
        assert!(title_needs_wide_font(&title));
        assert!(!title_needs_wide_font("Midnight Drive"));
        assert!(!title_needs_wide_font("A very long ascii title..."));
    }

    #[test]
    fn video_title_drops_supplementary_emoji() {
        assert_eq!(title_for_video("A 🎵 B 🇲🇽 C", "mp3"), "A B C");
        assert_eq!(
            title_for_video("goops! ：D 🇲🇽 - https", "mp3"),
            "goops! ：D - https"
        );
    }

    #[test]
    fn truncate_units_preserves_short_values() {
        assert_eq!(truncate_units("Short title", 40), "Short title");
        let cut = truncate_units("A much longer title than the limit", 12);
        assert!(text_units(&cut) <= 12);
        assert!(cut.ends_with("..."));
    }

    #[test]
    fn wrap_text_breaks_at_words_and_ellipsizes() {
        let lines = wrap_text("alpha beta gamma delta", 10, 2);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "alpha beta");
        assert!(lines[1].ends_with("..."));
        assert!(lines.iter().all(|line| text_units(line) <= 10));
    }

    #[test]
    fn clock_labels_round_to_seconds() {
        assert_eq!(format_clock(0.0), "0:00");
        assert_eq!(format_clock(82.365), "1:22");
        assert_eq!(format_clock(3661.0), "1:01:01");
    }

    #[test]
    fn author_strips_youtube_topic_suffix() {
        assert_eq!(author_for_video("OLDUCT - Topic"), "OLDUCT");
        assert_eq!(author_for_video("いよわ/ iyowa"), "いよわ/ iyowa");
    }

    #[test]
    fn youtube_metadata_extracts_album_release_track_and_credits() {
        let body = serde_json::json!({
            "artist": "OLDUCT",
            "album": "Uneven",
            "track": "凹凸",
            "release_date": "20241228",
            "description": "Provided to YouTube by OLDUCT\n\n凹凸 · OLDUCT\n\nUneven\n\n℗ 2024 OLDUCT\n\nReleased on: 2024-12-28\n\nComposer: OLDUCT\nProducer: OLDUCT"
        });
        let tags = parse_youtube_metadata(&body);
        assert!(tags.contains(&(String::from("album"), String::from("Uneven"))));
        assert!(tags.contains(&(String::from("date"), String::from("2024-12-28"))));
        assert!(tags.contains(&(String::from("track"), String::from("凹凸"))));
        assert!(tags.contains(&(String::from("composer"), String::from("OLDUCT"))));
        assert!(tags.contains(&(String::from("producer"), String::from("OLDUCT"))));
        assert!(tags.contains(&(String::from("copyright"), String::from("℗ 2024 OLDUCT"))));
    }

    #[test]
    fn metadata_rows_show_tags_without_technical_details() {
        let source = SourceInfo {
            duration: 193.2,
            audio_codec: String::from("mp3"),
            has_cover: false,
            tags: vec![
                (String::from("artist"), String::from("Iyowa")),
                (String::from("album"), String::from("Last Rally")),
                (String::from("date"), String::from("2024")),
                (String::from("encoder"), String::from("Lavf60")),
            ],
        };
        let rows = metadata_rows(&source, "Iyowa", "Song", 40);
        assert_eq!(
            rows,
            vec![
                (String::from("Album"), String::from("Last Rally")),
                (String::from("Released"), String::from("2024")),
            ]
        );
        assert!(!rows.iter().any(|(_, value)| {
            value.contains("Stereo") || value.contains("kbps") || value.contains(':')
        }));
    }

    #[test]
    fn metadata_rows_stop_at_six_entries() {
        let mut tags = Vec::new();
        for index in 0..10 {
            tags.push((format!("tag{index}"), format!("value {index}")));
        }
        let source = SourceInfo {
            duration: 10.0,
            audio_codec: String::from("flac"),
            has_cover: false,
            tags,
        };
        let rows = metadata_rows(&source, "", "", 40);
        assert_eq!(rows.len(), META_MAX_ROWS);
    }
}
