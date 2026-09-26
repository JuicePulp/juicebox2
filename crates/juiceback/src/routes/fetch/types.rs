use std::{path::PathBuf, sync::Arc};

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Deserialize, ToSchema)]
pub struct FetchStartRequest {
    pub url: String,

    #[serde(default)]
    pub audio_only: bool,

    #[serde(default)]
    pub video_quality: Option<String>,

    #[serde(default)]
    pub video_container: Option<String>,

    #[serde(default)]
    pub audio_format: Option<String>,

    #[serde(default)]
    pub better_audio: bool,

    #[serde(default)]
    pub youtube_video_codec: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct FetchStartResponse {
    pub job_id: String,
}

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

#[derive(Debug, Serialize, ToSchema)]
pub struct FetchStatusResponse {
    pub job_id: String,

    pub status: String,

    #[serde(default)]
    pub error: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<FetchFileResponse>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_received: Option<u64>,
}

pub(crate) type ServicesCache = tokio::sync::Mutex<Option<(std::time::Instant, Arc<Vec<String>>)>>;

#[derive(Debug, Serialize, ToSchema)]
pub struct FetchServicesResponse {
    pub services: Vec<String>,
}

pub(crate) type FetchResult = Result<String, String>;

pub(crate) enum ByteSource {
    Tunnel(String),

    LocalFile(PathBuf),
}
