use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone)]
pub enum FinishReservationResult {
    Completed(Box<FileRecord>),
    NotFound,
    NotUploading,
    InvalidToken,
}

#[derive(Debug, Clone)]
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
    /// Reporter-supplied gate password for protected files (plaintext by
    /// necessity: moderators need the actual value to open the file).
    pub password: Option<String>,
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

    /// Argon2id verifier for password-gated links (`None` = public).
    pub password_hash: Option<String>,

    /// Ciphertext-only on juicehost; juiceback holds the key.
    pub is_encrypted: bool,

    /// Hex-encoded 13-byte container header (lets juiceback serve Ranges
    /// without fetching ciphertext first).
    pub enc_header: Option<String>,

    /// Password-wrapped per-file data key (base64 `nonce || ct || tag`).
    /// `None` for public files and legacy (`key_version` 0) protected files.
    pub dek_wrapped: Option<String>,

    /// Base64 salt for the password-to-KEK derivation. Public by design
    /// (like a password-hash salt); useless without the password.
    pub dek_salt: Option<String>,

    /// Global-key-wrapped copy of the data key (hex). Lets admins and
    /// report moderators preview reported files; never leaves juiceback.
    pub dek_escrow: Option<String>,

    /// 0 = encrypted under the single global storage key (legacy),
    /// 1 = encrypted under the per-file data key.
    pub key_version: i64,
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
            password_hash: None,
            is_encrypted: false,
            enc_header: None,
            dek_wrapped: None,
            dek_salt: None,
            dek_escrow: None,
            key_version: 0,
        }
    }

    /// Whether downloads of this file require a password.
    #[must_use]
    pub fn is_protected(&self) -> bool {
        self.password_hash.is_some()
    }

    /// Whether this file uses a per-file data key (vs the legacy global
    /// storage key). Only meaningful when [`Self::is_protected`].
    #[must_use]
    pub fn uses_per_file_key(&self) -> bool {
        self.key_version == crate::crypto_file::KEY_VERSION_V1
            && self.dek_wrapped.is_some()
            && self.dek_salt.is_some()
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "pipeline fns thread established context (state, ids, tokens); bundling params churns callers for no behavior gain"
    )]
    #[must_use]
    pub fn from_upload_with_token(        id: String,
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

/// Password-gate plus encryption metadata written once a protected upload
/// completes. Built where the cleartext password is still in memory, then
/// only hashes/wrapped keys touch the database.
#[derive(Debug, Clone)]
pub struct ProtectionMaterial {
    pub password_hash: Option<String>,
    pub is_encrypted: bool,
    pub enc_header: Option<String>,
    pub dek_wrapped: Option<String>,
    pub dek_salt: Option<String>,
    pub dek_escrow: Option<String>,
    pub key_version: i64,
}

impl ProtectionMaterial {
    /// Legacy shape: global-key encryption, no per-file key material.
    /// Used until each upload path mints per-file keys.
    #[must_use]
    pub fn legacy(password_hash: Option<String>, enc_header: Option<String>) -> Self {
        Self {
            password_hash,
            is_encrypted: true,
            enc_header,
            dek_wrapped: None,
            dek_salt: None,
            dek_escrow: None,
            key_version: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ClientFileRecord {
    pub id: String,
    #[serde(default, skip_serializing)]
    pub token: String,
    #[serde(default)]
    pub n: String,
    #[serde(default)]
    pub m: String,
    #[serde(default)]
    pub s: i64,
    #[serde(default)]
    pub u: i64,
    #[serde(default)]
    pub e: i64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct FetchJob {
    pub id: String,
    pub user_id: String,
    pub source_url: String,

    pub status: String,
    pub error: String,

    #[serde(default)]
    pub file_id: String,
    pub created_at: i64,
    pub updated_at: i64,

    #[serde(default)]
    pub stage: String,

    #[serde(default)]
    pub bytes_received: i64,
}

#[derive(Debug, Clone)]
pub struct ImportBan {
    pub ip: String,

    pub reason: String,

    pub banned_by: String,
}
