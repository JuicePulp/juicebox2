//! Password-gated (at-rest encrypted) uploads through the relay path.
//!
//! The browser posts plaintext to juiceback, which validates it, encrypts it
//! with [`crate::crypto_file`], and pushes ciphertext to juicehost. Plaintext
//! never reaches juicehost. Direct/ticket/parallel paths bypass juiceback
//! bytes and are rejected for protected files (see [`relay_required`]).

use std::{path::PathBuf, sync::Arc};

use base64::Engine as _;
use bytes::Bytes;
use tokio::sync::mpsc;

use crate::{
    crypto_file::{self, FileKey},
    db::{self, ProtectionMaterial},
    error::AppError,
    state::AppState,
};

/// Marker returned with 400 when a protected upload arrives on a path that
/// bypasses juiceback-side encryption (direct ticket PUT, TUS parallel).
pub const USE_RELAY_FOR_PROTECTED: &str =
    "USE_RELAY_FOR_PROTECTED: password-protected uploads must use relay upload, not direct upload";

/// 400 rejection for protected uploads on non-relay paths.
#[must_use]
pub fn relay_required() -> AppError {
    AppError::BadRequest(USE_RELAY_FOR_PROTECTED.into())
}

/// A minted per-file data key plus everything needed to persist it.
/// The `dek` lives in memory only and must never be logged or stored.
/// Everything else is hashes/wrapped keys, safe for the database.
#[derive(Clone)]
pub struct ProtectionSetup {
    pub password_hash: String,
    pub salt_b64: String,
    pub wrapped_b64: String,
    pub escrow_hex: String,
    pub dek: FileKey,
}

impl ProtectionSetup {
    /// Database row material for this setup plus the container header.
    #[must_use]
    pub fn material(&self, header_hex: String) -> ProtectionMaterial {
        ProtectionMaterial {
            password_hash: Some(self.password_hash.clone()),
            is_encrypted: true,
            enc_header: Some(header_hex),
            dek_wrapped: Some(self.wrapped_b64.clone()),
            dek_salt: Some(self.salt_b64.clone()),
            dek_escrow: Some(self.escrow_hex.clone()),
            key_version: crypto_file::KEY_VERSION_V1,
        }
    }
}

/// Mint a per-file data key for `password`: Argon2id verifier, random salt,
/// password-wrapped DEK, and a global-key escrow copy (for admin/report
/// preview). Both Argon2id runs happen in one blocking task; the cleartext
/// password never leaves this call.
///
/// # Errors
///
/// Returns [`AppError::BadRequest`] on policy violation and
/// [`AppError::Internal`] when hashing, derivation, or escrow fails.
pub async fn prepare_protection(
    state: &Arc<AppState>,
    password: &str,
) -> Result<ProtectionSetup, AppError> {
    state
        .config
        .check_upload_password(password)
        .map_err(AppError::BadRequest)?;
    let storage_key = state
        .config
        .storage_file_key()
        .map_err(|_| AppError::Internal("storage encryption unavailable".into()))?;
    let owned = password.to_string();
    tokio::task::spawn_blocking(move || {
        let password_hash =
            crate::auth::hash_password(&owned).map_err(AppError::Internal)?;
        let salt = crypto_file::random_salt();
        let kek = crypto_file::derive_kek(&owned, &salt)
            .map_err(|e| AppError::Internal(format!("key wrap failed: {e}")))?;
        let dek = FileKey::generate();
        let wrapped = crypto_file::wrap_dek(&kek, &dek)
            .map_err(|e| AppError::Internal(format!("key wrap failed: {e}")))?;
        let escrow = crypto_file::encrypt_chunk(&storage_key, dek.as_bytes())
            .map_err(|e| AppError::Internal(format!("key escrow failed: {e}")))?;
        Ok::<_, AppError>(ProtectionSetup {
            password_hash,
            salt_b64: base64::engine::general_purpose::STANDARD.encode(salt),
            wrapped_b64: base64::engine::general_purpose::STANDARD.encode(wrapped),
            escrow_hex: hex::encode(escrow),
            dek,
        })
    })
    .await
    .map_err(|_| AppError::Internal("protection setup panicked".into()))?
}

/// Recover a file's data key through the global-key escrow copy (admin and
/// reservation-completion paths). Fast symmetric crypto, safe inline.
///
/// # Errors
///
/// Returns [`AppError::Internal`] when the escrow blob is malformed or the
/// storage key is unavailable.
pub fn unwrap_escrow(state: &Arc<AppState>, escrow_hex: &str) -> Result<FileKey, AppError> {
    let key = state
        .config
        .storage_file_key()
        .map_err(|_| AppError::Internal("storage encryption unavailable".into()))?;
    let raw = hex::decode(escrow_hex.trim())
        .map_err(|_| AppError::Internal("bad key escrow".into()))?;
    let plain = crypto_file::decrypt_chunk(&key, &raw)
        .map_err(|_| AppError::Internal("bad key escrow".into()))?;
    FileKey::from_bytes(&plain).map_err(|_| AppError::Internal("bad key escrow".into()))
}

/// The encryption key plus row material for one protected upload.
/// `Fresh` mints from a form password; `Reserved` reuses the reservation
/// row (whose verifier and wraps already correspond to one password - a
/// form password is never mixed in). Legacy reservation rows without key
/// material fall back to the global storage key.
pub enum UploadProtection {
    Fresh(ProtectionSetup),
    Reserved(ProtectionMaterial),
}

impl UploadProtection {
    /// Data key for chunk encryption (memory only).
    pub fn dek(&self, state: &Arc<AppState>) -> Result<FileKey, AppError> {
        let global = || {
            state
                .config
                .storage_file_key()
                .map_err(|_| AppError::Internal("storage encryption unavailable".into()))
        };
        match self {
            Self::Fresh(setup) => Ok(setup.dek.clone()),
            Self::Reserved(material) => match material.dek_escrow.as_deref() {
                Some(escrow) => unwrap_escrow(state, escrow),
                None => global(),
            },
        }
    }

    /// Row material with the container header filled in.
    #[must_use]
    pub fn material(&self, header_hex: String) -> ProtectionMaterial {
        match self {
            Self::Fresh(setup) => setup.material(header_hex),
            Self::Reserved(material) => {
                let mut material = material.clone();
                material.enc_header = Some(header_hex);
                material
            }
        }
    }

    /// Verifier for response flags.
    #[must_use]
    pub fn password_hash(&self) -> Option<String> {
        match self {
            Self::Fresh(setup) => Some(setup.password_hash.clone()),
            Self::Reserved(material) => material.password_hash.clone(),
        }
    }
}

/// Resolve protection for a multipart completion: a protected reservation
/// reuses its row material (v1) or the global key (legacy rows); otherwise
/// a form password mints fresh v1 material. Returns `None` for public
/// uploads.
pub async fn resolve_multipart_protection(
    state: &Arc<AppState>,
    password: Option<String>,
    reservation: Option<&db::FileRecord>,
) -> Result<Option<UploadProtection>, AppError> {
    if let Some(record) = reservation.filter(|r| r.is_protected()) {
        return Ok(Some(UploadProtection::Reserved(ProtectionMaterial {
            password_hash: record.password_hash.clone(),
            is_encrypted: true,
            enc_header: None,
            dek_wrapped: record.dek_wrapped.clone(),
            dek_salt: record.dek_salt.clone(),
            dek_escrow: record.dek_escrow.clone(),
            key_version: record.key_version,
        })));
    }
    let Some(password) = password.filter(|p| !p.is_empty()) else {
        return Ok(None);
    };
    Ok(Some(UploadProtection::Fresh(
        prepare_protection(state, &password).await?,
    )))
}

/// Spool one side of a plaintext byte stream into a temp file, enforcing the
/// plaintext size cap. Returns the file and the spooled byte count.
pub fn spawn_spool_task(
    max_size: i64,
) -> (
    mpsc::Sender<Result<Bytes, String>>,
    tokio::task::JoinHandle<Result<(tempfile::NamedTempFile, u64), AppError>>,
) {
    let (tx, mut rx) = mpsc::channel::<Result<Bytes, String>>(crate::constants::STREAM_CHANNEL_CAPACITY);
    let handle = tokio::spawn(async move {
        let spool = tempfile::NamedTempFile::new().map_err(|e| {
            AppError::Internal(format!("protected upload spool failed: {e}"))
        })?;
        let mut out = tokio::fs::OpenOptions::new()
            .append(true)
            .open(spool.path())
            .await
            .map_err(|e| AppError::Internal(format!("protected upload spool failed: {e}")))?;
        let mut total: u64 = 0;
        while let Some(item) = rx.recv().await {
            let chunk = item.map_err(AppError::from_juicehost_error)?;
            total += chunk.len() as u64;
            if total > max_size as u64 {
                return Err(AppError::PayloadTooLarge);
            }
            tokio::io::AsyncWriteExt::write_all(&mut out, &chunk)
                .await
                .map_err(|e| AppError::Internal(format!("protected upload spool failed: {e}")))?;
        }
        tokio::io::AsyncWriteExt::flush(&mut out)
            .await
            .map_err(|e| AppError::Internal(format!("protected upload spool failed: {e}")))?;
        Ok((spool, total))
    });
    (tx, handle)
}

/// Encrypt a spooled plaintext file and push the container (header + chunks)
/// to juicehost through the standard relay channel. Returns the header bytes
/// for the `enc_header` DB column.
///
/// # Errors
///
/// Returns an error when the spool cannot be read, encryption fails, or the
/// juicehost push is rejected.
#[expect(
    clippy::too_many_arguments,
    reason = "pipeline fns thread established context (state, ids, tokens); bundling params churns callers for no behavior gain"
)]
pub async fn push_encrypted_file(
    state: &Arc<AppState>,
    file_id: &str,
    filename: &str,
    mime_type: &str,
    host: Option<&str>,
    capability: &str,
    path: &std::path::Path,
    plain_len: u64,
    upload_mode: crate::upload_mode::UploadMode,
    dek: &FileKey,
) -> Result<[u8; crate::crypto_file::HEADER_LEN], AppError> {
    let header = crate::crypto_file::encode_header(plain_len);
    let (tx, push_handle) = crate::routes::upload::stream::spawn_upload_push(
        state,
        file_id,
        filename,
        mime_type,
        host,
        upload_mode,
        capability,
    );
    tx.send(Ok(Bytes::copy_from_slice(&header)))
        .await
        .map_err(|_| AppError::TaskPanicked("storage push task gone".into()))?;

    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| AppError::Internal(format!("protected upload read failed: {e}")))?;
    let mut buf = vec![0_u8; crate::crypto_file::PLAINTEXT_CHUNK_LEN];
    let mut remaining = plain_len;
    while remaining > 0 {
        let want = remaining.min(crate::crypto_file::PLAINTEXT_CHUNK_LEN as u64);
        let want = usize::try_from(want).map_err(|_| AppError::Internal("file too large".into()))?;
        let mut got = 0;
        while got < want {
            let n = tokio::io::AsyncReadExt::read(&mut file, &mut buf[got..want])
                .await
                .map_err(|e| AppError::Internal(format!("protected upload read failed: {e}")))?;
            if n == 0 {
                return Err(AppError::Internal("spooled upload truncated".into()));
            }
            got += n;
        }
        let enc = crate::crypto_file::encrypt_chunk(dek, &buf[..want])
            .map_err(|e| AppError::Internal(format!("encryption failed: {e}")))?;
        if tx.send(Ok(Bytes::from(enc))).await.is_err() {
            return Err(AppError::TaskPanicked("storage push task gone".into()));
        }
        remaining -= want as u64;
    }
    if plain_len == 0 {
        let enc = crate::crypto_file::encrypt_chunk(dek, &[])
            .map_err(|e| AppError::Internal(format!("encryption failed: {e}")))?;
        if tx.send(Ok(Bytes::from(enc))).await.is_err() {
            return Err(AppError::TaskPanicked("storage push task gone".into()));
        }
    }
    drop(tx);
    push_handle
        .await
        .map_err(|_| AppError::TaskPanicked("storage push task panicked".into()))?
        .map_err(AppError::from_juicehost_error)?;
    Ok(header)
}

/// Persist protection metadata after a relayed completion.
///
/// # Errors
///
/// Returns [`AppError::NotFound`] when the row vanished mid-completion.
pub async fn mark_protected(
    state: &Arc<AppState>,
    file_id: &str,
    material: &ProtectionMaterial,
) -> Result<(), AppError> {
    let id = file_id.to_string();
    let owned = material.clone();
    let updated = state
        .db_call("set_protection", move |db| {
            db::set_protection(db, &id, &owned)
        })
        .await?;
    if !updated {
        return Err(AppError::NotFound);
    }
    Ok(())
}

/// Absolute path of a TUS protected-upload spool file. Lives beside the
/// process temp dir and is removed when the upload completes or aborts.
#[must_use]
pub fn tus_spool_path(id: &str) -> PathBuf {
    std::env::temp_dir().join(format!("juicebox-tus-protected-{id}"))
}
