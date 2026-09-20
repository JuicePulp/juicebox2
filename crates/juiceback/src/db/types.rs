use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Result of attempting to complete an existing reservation.
#[derive(Debug, Clone)]
pub enum FinishReservationResult {
    Completed(Box<FileRecord>),
    NotFound,
    NotUploading,
    InvalidToken,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AdminUser {
    pub id: i64,
    pub username: String,
    pub password_hash: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ReportRecord {
    pub id: i64,
    pub file_url: String,
    pub reason: String,
    pub details: String,
    pub reporter_ip: Option<String>,
    pub email: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct FeedbackRecord {
    pub id: i64,
    pub message: String,
    pub email: Option<String>,
    pub reporter_ip: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct BanRecord {
    pub ip: String,
    pub reason: String,
    pub banned_by: String,
    pub banned_at: i64,
}

/// A known custom host (juicehost instance) that users have pointed the UI at.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct HosterRecord {
    pub host: String,
    pub first_seen_at: i64,
    pub last_seen_at: i64,
    pub last_status: String,
    pub last_error: String,
    pub banned: bool,
    pub banned_reason: String,
    pub banned_by: String,
    pub banned_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Announcement {
    pub id: i64,
    pub message: String,
    pub link_url: String,
    pub mode: String,
    pub is_active: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct FileRecord {
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub storage_path: String,
    pub delete_token: String,
    pub uploaded_at: i64,
    pub expires_at: i64,
    pub uploader_ip: Option<String>,
    pub storage_host: Option<String>,
    pub status: String,
}

impl FileRecord {
    #[expect(
        clippy::too_many_arguments,
        reason = "pipeline fns thread established context (state, ids, tokens); bundling params churns callers for no behavior gain"
    )]
    #[must_use]
    pub fn new(
        id: String,
        filename: String,
        mime_type: String,
        size_bytes: i64,
        delete_token: String,
        uploaded_at: i64,
        expires_at: i64,
        uploader_ip: Option<String>,
        storage_host: Option<String>,
    ) -> Self {
        Self {
            storage_path: format!("remote-{id}"),
            id,
            filename,
            mime_type,
            size_bytes,
            delete_token,
            uploaded_at,
            expires_at,
            uploader_ip,
            storage_host,
            status: "ready".to_string(),
        }
    }

    /// Build a `FileRecord` from upload parameters with an existing delete
    /// token.
    #[expect(
        clippy::too_many_arguments,
        reason = "pipeline fns thread established context (state, ids, tokens); bundling params churns callers for no behavior gain"
    )]
    #[must_use]
    pub fn from_upload_with_token(
        id: String,
        filename: String,
        mime_type: String,
        size_bytes: i64,
        ttl_hours: f64,
        delete_token: String,
        uploader_ip: Option<String>,
        storage_host: Option<String>,
    ) -> Self {
        let now = chrono::Utc::now().timestamp();
        let ttl_secs = (ttl_hours * crate::constants::SECONDS_PER_HOUR_F64).round() as i64;
        Self::new(
            id,
            filename,
            mime_type,
            size_bytes,
            delete_token,
            now,
            now + ttl_secs,
            uploader_ip,
            storage_host,
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ClientFileRecord {
    pub id: String,
    #[serde(default, skip_serializing)]
    pub token: String,
    #[serde(default)]
    pub n: String, // filename
    #[serde(default)]
    pub m: String, // mime_type
    #[serde(default)]
    pub s: i64, // size_bytes
    #[serde(default)]
    pub u: i64, // uploaded_at (sec)
    #[serde(default)]
    pub e: i64, // expires_at (sec)
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct FetchJob {
    pub id: String,
    pub user_id: String,
    pub source_url: String,
    /// `pending`, `processing`, `downloading`, `done`, or `failed`.
    pub status: String,
    pub error: String,
    /// Set once the fetched media has been stored as a normal file.
    #[serde(default)]
    pub file_id: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// Fine-grained progress hint (e.g. "cobalt", "session-fallback",
    /// "transfer"); empty for terminal states.
    #[serde(default)]
    pub stage: String,
    /// Bytes received from the media source so far (best-effort).
    #[serde(default)]
    pub bytes_received: i64,
}

pub struct ImportBan {
    pub ip: String,
    pub reason: String,
    pub banned_by: String,
}
