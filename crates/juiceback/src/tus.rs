use std::sync::Arc;

use bytes::Bytes;
use dashmap::DashMap;
use tokio::sync::{Mutex, mpsc};

use crate::upload_mode::UploadMode;

/// `patch_lock` serializes offset updates for concurrent PATCHes.
#[derive(Debug)]
pub struct TusUpload {
    pub id: String,
    pub offset: u64,
    pub total_length: u64,
    pub filename: String,
    pub mime_type: String,
    pub ttl_hours: f64,
    pub created_at: i64,
    pub last_activity: i64,
    pub delete_token: String,

    // Hashed for ban checks; encrypted so support can recover the address.
    pub hashed_ip: String,

    pub encrypted_ip: String,
    pub storage_host: Option<String>,

    pub session_id: Option<String>,

    pub part_index: Option<usize>,

    pub reserve_id: Option<String>,

    pub reservation_token: Option<String>,

    pub capability: String,

    pub patch_lock: Arc<Mutex<()>>,
    pub user_id: String,

    pub upload_mode: UploadMode,

    pub push_rx: Option<mpsc::Receiver<Result<Bytes, String>>>,

    pub password_hash: Option<String>,

    // Per-file data key: live copy, password-wrapped copy, and server escrow copy.
    pub dek: Option<crate::crypto_file::FileKey>,

    pub dek_wrapped: Option<String>,

    pub dek_salt: Option<String>,

    pub dek_escrow: Option<String>,
}

pub type TusMap = Arc<DashMap<String, TusUpload>>;

#[must_use]
pub fn new_tus_state() -> TusMap {
    Arc::new(DashMap::new())
}

pub type TusSenderMap = Arc<DashMap<String, mpsc::Sender<Result<Bytes, String>>>>;

#[must_use]
pub fn new_tus_sender_map() -> TusSenderMap {
    Arc::new(DashMap::new())
}

/// `finalizing` makes finalize idempotent.
pub struct PartSession {
    pub session_id: String,
    pub total_parts: usize,
    pub filename: String,
    pub mime_type: String,
    pub storage_host: Option<String>,
    pub ttl_hours: f64,
    pub reserve_id: Option<String>,
    pub reservation_token: Option<String>,
    pub capability: String,
    pub user_id: String,
    pub upload_mode: UploadMode,
    pub declared_size: std::sync::atomic::AtomicU64,
    pub finalized_parts: DashMap<usize, ()>,
    pub finalizing: std::sync::atomic::AtomicBool,

    pub part_ids: DashMap<usize, String>,
    pub part_lengths: DashMap<usize, u64>,
    // Caches reservation creation so concurrent finalizers agree, including `None`.
    pub completion_reserve_id: std::sync::Mutex<Option<Option<String>>>,
}

impl std::fmt::Debug for PartSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PartSession")
            .field("session_id", &self.session_id)
            .field("total_parts", &self.total_parts)
            .field("finalized", &self.finalized_parts.len())
            .finish()
    }
}

pub type PartSessionMap = Arc<DashMap<String, Arc<PartSession>>>;

#[must_use]
pub fn new_part_session_map() -> PartSessionMap {
    Arc::new(DashMap::new())
}
