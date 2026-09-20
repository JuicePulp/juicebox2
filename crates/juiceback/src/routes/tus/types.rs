/// Snapshot of a TUS upload's metadata, taken when the upload completes.
pub(crate) struct TusUploadMeta {
    pub(crate) id: String,
    pub(crate) filename: String,
    pub(crate) mime_type: String,
    pub(crate) total_length: u64,
    pub(crate) delete_token: String,
    pub(crate) encrypted_ip: String,
    pub(crate) ttl_hours: f64,
    pub(crate) storage_host: Option<String>,
    pub(crate) reserve_id: Option<String>,
    pub(crate) reservation_token: Option<String>,
    pub(crate) user_id: String,
}
