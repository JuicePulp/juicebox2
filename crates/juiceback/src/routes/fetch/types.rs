use std::{path::PathBuf, sync::Arc};

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// JSON body for starting a fetch job.
#[derive(Debug, Deserialize, ToSchema)]
pub struct FetchStartRequest {
    /// Source URL (`YouTube`, tiktok, etc).
    pub url: String,
    /// Audio-only fetch (default false = video+audio).
    #[serde(default)]
    pub audio_only: bool,
    /// Video quality cap: max/2160/1440/1080/720/480/360/240/144.
    #[serde(default)]
    pub video_quality: Option<String>,
    /// Video container preference (YouTube): auto/mp4/webm/mkv.
    #[serde(default)]
    pub video_container: Option<String>,
    /// Audio format for audio-only: best/mp3/ogg/wav/opus.
    #[serde(default)]
    pub audio_format: Option<String>,
    /// Ask cobalt to hunt for the highest available audio quality (`YouTube`;
    /// requires `YOUTUBE_ALLOW_BETTER_AUDIO` on the instance).
    #[serde(default)]
    pub better_audio: bool,
    /// `YouTube` codec preference: h264/av1/vp9.
    #[serde(default)]
    pub youtube_video_codec: Option<String>,
}

/// JSON response for `POST /api/fetch`.
#[derive(Debug, Serialize, ToSchema)]
pub struct FetchStartResponse {
    pub job_id: String,
}

/// File details returned with a completed fetch job.
#[derive(Debug, Serialize, ToSchema)]
pub struct FetchFileResponse {
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub url: String,
    pub expires_at: i64,
    pub delete_token: String,
}

/// JSON response for `GET /api/fetch/:id`.
#[derive(Debug, Serialize, ToSchema)]
pub struct FetchStatusResponse {
    pub job_id: String,
    /// `pending`, `processing`, `downloading`, `done`, or `failed`.
    pub status: String,
    #[serde(default)]
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<FetchFileResponse>,
    /// Fine-grained progress hint while the job is still running.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    /// Bytes received from the media source so far.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_received: Option<u64>,
}

/// Cached service list from the cobalt instance (refreshed hourly).
pub(crate) type ServicesCache = tokio::sync::Mutex<Option<(std::time::Instant, Arc<Vec<String>>)>>;

/// Response for `GET /api/fetch/services`.
#[derive(Debug, Serialize, ToSchema)]
pub struct FetchServicesResponse {
    /// Downloadable-service domains supported by the cobalt instance.
    pub services: Vec<String>,
}

pub(crate) type FetchResult = Result<String, String>;

/// Where the media bytes come from: a cobalt tunnel URL or a local file
/// produced by the yt-dlp fallback tier.
pub(crate) enum ByteSource {
    Tunnel(String),
    LocalFile(PathBuf),
}
