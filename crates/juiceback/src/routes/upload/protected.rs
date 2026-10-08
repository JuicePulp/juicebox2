//! Password-gated (at-rest encrypted) uploads through the relay path.
//!
//! The browser posts plaintext to juiceback, which validates it, encrypts it
//! with [`crate::crypto_file`], and pushes ciphertext to juicehost. Plaintext
//! never reaches juicehost. Direct/ticket/parallel paths bypass juiceback
//! bytes and are rejected for protected files (see [`relay_required`]).

use std::{path::PathBuf, sync::Arc};

use bytes::Bytes;
use tokio::sync::mpsc;

use crate::{db, error::AppError, state::AppState};

/// Marker returned with 400 when a protected upload arrives on a path that
/// bypasses juiceback-side encryption (direct ticket PUT, TUS parallel).
pub const USE_RELAY_FOR_PROTECTED: &str =
    "USE_RELAY_FOR_PROTECTED: password-protected uploads must use relay upload, not direct upload";

/// 400 rejection for protected uploads on non-relay paths.
#[must_use]
pub fn relay_required() -> AppError {
    AppError::BadRequest(USE_RELAY_FOR_PROTECTED.into())
}

/// Validate an optional upload password and return its Argon2id verifier.
///
/// Returns `None` for unprotected uploads. Hashing runs on the blocking pool:
/// Argon2id is CPU-bound (and deep-stacked in debug builds), so it must not
/// run on async workers. The cleartext password is never logged; callers
/// must keep it out of tracing fields.
///
/// # Errors
///
/// Returns [`AppError::BadRequest`] on policy violation and
/// [`AppError::Internal`] when hashing fails.
pub async fn hash_upload_password(
    state: &Arc<AppState>,
    password: Option<String>,
) -> Result<Option<String>, AppError> {
    let Some(password) = password.filter(|p| !p.is_empty()) else {
        return Ok(None);
    };
    state
        .config
        .check_upload_password(&password)
        .map_err(AppError::BadRequest)?;
    tokio::task::spawn_blocking(move || crate::auth::hash_password(&password))
        .await
        .map_err(|_| AppError::Internal("password hashing panicked".into()))?
        .map(Some)
        .map_err(AppError::Internal)
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
/// Returns an error when the storage key is unavailable, the spool cannot be
/// read, encryption fails, or the juicehost push is rejected.
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
) -> Result<[u8; crate::crypto_file::HEADER_LEN], AppError> {
    let key = state
        .config
        .storage_file_key()
        .map_err(|_| AppError::Internal("storage encryption unavailable".into()))?;
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
        let enc = crate::crypto_file::encrypt_chunk(&key, &buf[..want])
            .map_err(|e| AppError::Internal(format!("encryption failed: {e}")))?;
        if tx.send(Ok(Bytes::from(enc))).await.is_err() {
            return Err(AppError::TaskPanicked("storage push task gone".into()));
        }
        remaining -= want as u64;
    }
    if plain_len == 0 {
        let enc = crate::crypto_file::encrypt_chunk(&key, &[])
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

/// Persist protection metadata after a relayed completion. Returns the
/// hex-encoded header for responses.
///
/// # Errors
///
/// Returns [`AppError::NotFound`] when the row vanished mid-completion.
pub async fn mark_protected(
    state: &Arc<AppState>,
    file_id: &str,
    password_hash: Option<&str>,
    header: &[u8; crate::crypto_file::HEADER_LEN],
) -> Result<String, AppError> {
    let header_hex = hex::encode(header);
    let id = file_id.to_string();
    let hash = password_hash.map(str::to_string);
    let hex_owned = header_hex.clone();
    let updated = state
        .db_call("set_protection", move |db| {
            db::set_protection(db, &id, hash.as_deref(), true, Some(&hex_owned))
        })
        .await?;
    if !updated {
        return Err(AppError::NotFound);
    }
    Ok(header_hex)
}

/// Absolute path of a TUS protected-upload spool file. Lives beside the
/// process temp dir and is removed when the upload completes or aborts.
#[must_use]
pub fn tus_spool_path(id: &str) -> PathBuf {
    std::env::temp_dir().join(format!("juicebox-tus-protected-{id}"))
}
