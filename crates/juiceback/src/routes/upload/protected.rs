






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



pub const USE_RELAY_FOR_PROTECTED: &str =
    "USE_RELAY_FOR_PROTECTED: password-protected uploads must use relay upload, not direct upload";


#[must_use]
pub fn relay_required() -> AppError {
    AppError::BadRequest(USE_RELAY_FOR_PROTECTED.into())
}




#[derive(Clone)]
pub struct ProtectionSetup {
    pub password_hash: String,
    pub salt_b64: String,
    pub wrapped_b64: String,
    pub escrow_hex: String,
    pub dek: FileKey,
}

impl ProtectionSetup {
    
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
        let password_hash = crate::auth::hash_password(&owned).map_err(AppError::Internal)?;
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








pub fn unwrap_escrow(state: &Arc<AppState>, escrow_hex: &str) -> Result<FileKey, AppError> {
    let key = state
        .config
        .storage_file_key()
        .map_err(|_| AppError::Internal("storage encryption unavailable".into()))?;
    let raw =
        hex::decode(escrow_hex.trim()).map_err(|_| AppError::Internal("bad key escrow".into()))?;
    let plain = crypto_file::decrypt_chunk(&key, &raw)
        .map_err(|_| AppError::Internal("bad key escrow".into()))?;
    FileKey::from_bytes(&plain).map_err(|_| AppError::Internal("bad key escrow".into()))
}






pub enum UploadProtection {
    Fresh(ProtectionSetup),
    Reserved(ProtectionMaterial),
}

impl UploadProtection {
    
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

    
    #[must_use]
    pub fn password_hash(&self) -> Option<String> {
        match self {
            Self::Fresh(setup) => Some(setup.password_hash.clone()),
            Self::Reserved(material) => material.password_hash.clone(),
        }
    }
}





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



pub fn spawn_spool_task(
    max_size: i64,
) -> (
    mpsc::Sender<Result<Bytes, String>>,
    tokio::task::JoinHandle<Result<(tempfile::NamedTempFile, u64), AppError>>,
) {
    let (tx, mut rx) =
        mpsc::channel::<Result<Bytes, String>>(crate::constants::STREAM_CHANNEL_CAPACITY);
    let handle = tokio::spawn(async move {
        let spool = tempfile::NamedTempFile::new()
            .map_err(|e| AppError::Internal(format!("protected upload spool failed: {e}")))?;
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
        let want =
            usize::try_from(want).map_err(|_| AppError::Internal("file too large".into()))?;
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



#[must_use]
pub fn tus_spool_path(id: &str) -> PathBuf {
    std::env::temp_dir().join(format!("juicebox-tus-protected-{id}"))
}
