use std::sync::Arc;

use super::{state::await_storage_push, types::TusUploadMeta};
use crate::{
    crypto_file::KEY_VERSION_V1,
    db::{self, FileRecord, ProtectionMaterial},
    error::AppError,
    state::AppState,
};

pub(crate) async fn complete_tus_upload(
    state: &Arc<AppState>,
    meta: TusUploadMeta,
) -> Result<serde_json::Value, AppError> {
    state.tus_senders.remove(&meta.id);
    if meta.password_hash.is_none() {
        await_storage_push(state, &meta.id).await?;
    }
    finish_tus_upload(state, meta).await
}

pub(crate) async fn finish_tus_upload(
    state: &Arc<AppState>,
    meta: TusUploadMeta,
) -> Result<serde_json::Value, AppError> {
    // Protected uploads land here with plaintext spooled locally and nothing
    // pushed yet. Encrypt and push under the final id, skipping the rename.
    // Fresh-password sessions carry their minted DEK; reservation-backed
    // sessions reload the row material (legacy rows: global key).
    let protected: Option<(ProtectionMaterial, crate::crypto_file::FileKey)> =
        if meta.password_hash.is_some() {
            let target_id = meta.reserve_id.as_deref().unwrap_or(&meta.id);
            let spool = crate::routes::upload::protected::tus_spool_path(&meta.id);
            let spooled = tokio::fs::metadata(&spool).await.map_err(|_| {
                AppError::Internal("protected upload spool missing".into())
            })?;
            if spooled.len() != meta.total_length {
                return Err(AppError::Internal("protected upload size mismatch".into()));
            }
            let capability = meta
                .reservation_token
                .clone()
                .unwrap_or_else(|| meta.delete_token.clone());
            let (material, dek) = if let Some(dek) = meta.dek.clone() {
                (
                    ProtectionMaterial {
                        password_hash: meta.password_hash.clone(),
                        is_encrypted: true,
                        enc_header: None,
                        dek_wrapped: meta.dek_wrapped.clone(),
                        dek_salt: meta.dek_salt.clone(),
                        dek_escrow: meta.dek_escrow.clone(),
                        key_version: KEY_VERSION_V1,
                    },
                    dek,
                )
            } else if let Some(reserve_id) = meta.reserve_id.as_deref() {
                let lookup = reserve_id.to_string();
                let reservation = state
                    .db_call("get_tus_protection_row", move |db| db::get_file(db, &lookup))
                    .await?
                    .ok_or_else(|| AppError::BadRequest("invalid reserve_id".into()))?;
                let material = ProtectionMaterial {
                    password_hash: reservation.password_hash.clone(),
                    is_encrypted: true,
                    enc_header: None,
                    dek_wrapped: reservation.dek_wrapped.clone(),
                    dek_salt: reservation.dek_salt.clone(),
                    dek_escrow: reservation.dek_escrow.clone(),
                    key_version: reservation.key_version,
                };
                let dek = match material.dek_escrow.as_deref() {
                    Some(escrow) => {
                        crate::routes::upload::protected::unwrap_escrow(state, escrow)?
                    }
                    None => state
                        .config
                        .storage_file_key()
                        .map_err(|_| AppError::Internal("storage encryption unavailable".into()))?,
                };
                (material, dek)
            } else {
                return Err(AppError::Internal(
                    "protected upload missing key material".into(),
                ));
            };
            let header = crate::routes::upload::protected::push_encrypted_file(
                state,
                target_id,
                &meta.filename,
                &meta.mime_type,
                meta.storage_host.as_deref(),
                &capability,
                &spool,
                meta.total_length,
                meta.upload_mode,
                &dek,
            )
            .await?;
            let _ = tokio::fs::remove_file(&spool).await;
            let mut material = material;
            material.enc_header = Some(hex::encode(header));
            Some((material, dek))
        } else {
            None
        };

    if meta.reserve_id.is_some() && meta.password_hash.is_none() {
        crate::storage_client::rename_file_on_juicehost(
            state,
            &meta.id,
            meta.reserve_id.as_deref().unwrap_or(&meta.id),
            meta.storage_host.as_deref(),
            meta.reservation_token.as_deref(),
        )
        .await
        .map_err(AppError::from_juicehost_error)?;
    }

    let record = if let Some(reserve_id) = meta.reserve_id.as_deref() {
        let reservation_token = meta
            .reservation_token
            .clone()
            .ok_or_else(|| AppError::Forbidden("reservation delete token required".into()))?;
        let update_id = reserve_id.to_string();
        let fname = meta.filename.clone();
        let mtype = meta.mime_type.clone();
        let size = meta.total_length as i64;
        let enc_ip = meta.encrypted_ip.clone();
        let completed = match state
            .db_call("finish_file", move |db| {
                crate::db::finish_reservation(
                    db,
                    &fname,
                    &mtype,
                    size,
                    &update_id,
                    &reservation_token,
                    Some(enc_ip.as_str()),
                )
            })
            .await?
        {
            db::FinishReservationResult::Completed(record) => record,
            db::FinishReservationResult::NotFound => {
                return Err(AppError::BadRequest("invalid reserve_id".into()));
            }
            db::FinishReservationResult::NotUploading => {
                return Err(AppError::BadRequest("reservation is not uploading".into()));
            }
            db::FinishReservationResult::InvalidToken => {
                return Err(AppError::Forbidden(
                    "invalid reservation delete token".into(),
                ));
            }
        };
        let completed_id = &completed.id;
        let completed_size = completed.size_bytes;
        tracing::info!("reserved TUS upload completed: id={completed_id} size={completed_size}");
        let mut completed = completed;
        if let Some((ref material, _)) = protected {
            crate::routes::upload::protected::mark_protected(state, &completed.id, material)
                .await?;
            completed.password_hash = material.password_hash.clone();
            completed.is_encrypted = material.is_encrypted;
            completed.enc_header = material.enc_header.clone();
            completed.dek_wrapped = material.dek_wrapped.clone();
            completed.dek_salt = material.dek_salt.clone();
            completed.dek_escrow = material.dek_escrow.clone();
            completed.key_version = material.key_version;
        }
        completed
    } else {
        let mut record = FileRecord::from_upload_with_token(
            meta.id.clone(),
            meta.filename,
            meta.mime_type,
            meta.total_length as i64,
            meta.ttl_hours,
            meta.delete_token,
            Some(meta.encrypted_ip),
            meta.storage_host.clone(),
        );
        if let Some((ref material, _)) = protected {
            record.password_hash = material.password_hash.clone();
            record.is_encrypted = material.is_encrypted;
            record.enc_header = material.enc_header.clone();
            record.dek_wrapped = material.dek_wrapped.clone();
            record.dek_salt = material.dek_salt.clone();
            record.dek_escrow = material.dek_escrow.clone();
            record.key_version = material.key_version;
        }
        Box::new(db::insert_new_file(state, record).await?)
    };

    let owned_record = record.clone();
    let owner = meta.user_id.clone();
    state
        .db_call("own_tus_file", move |db| {
            db::add_client_file(db, &owner, &owned_record)
        })
        .await?;

    let public_url = crate::utils::share_url(
        &state.config.public_base_url,
        record.storage_host.as_deref(),
        &record.id,
        &record.filename,
        record.is_protected(),
    );
    let protected = record.is_protected();
    let is_encrypted = record.is_encrypted;

    Ok(serde_json::to_value(crate::routes::upload::UploadResponse {
        id: record.id,
        url: public_url,
        filename: record.filename,
        size_bytes: record.size_bytes,
        mime_type: record.mime_type,
        expires_at: record.expires_at,
        delete_token: record.delete_token,
        status: record.status,
        protected,
        is_encrypted,
    })
    .unwrap_or_default())
}
