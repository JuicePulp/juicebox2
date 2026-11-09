pub(crate) struct TusUploadMeta {
    pub(crate) id: String,

    pub(crate) filename: String,

    pub(crate) mime_type: String,

    pub(crate) total_length: u64,

    // Also serves as the storage capability for this upload.
    pub(crate) delete_token: String,

    // Encrypted so support can recover the uploader address.
    pub(crate) encrypted_ip: String,

    pub(crate) ttl_hours: f64,

    pub(crate) storage_host: Option<String>,

    // When set, completes this reservation instead of inserting a new file.
    pub(crate) reserve_id: Option<String>,

    pub(crate) reservation_token: Option<String>,

    pub(crate) user_id: String,

    pub(crate) password_hash: Option<String>,

    // Live key when this process set up encryption; older sessions recover it from escrow.
    pub(crate) dek: Option<crate::crypto_file::FileKey>,

    pub(crate) dek_wrapped: Option<String>,

    pub(crate) dek_salt: Option<String>,

    pub(crate) dek_escrow: Option<String>,
}
