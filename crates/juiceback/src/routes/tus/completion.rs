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
    let protected: Option<(ProtectionMaterial, crate::crypto_file::FileKey)> =
        if meta.password_hash.is_some() {
            let target_id = meta.reserve_id.as_deref().unwrap_or(&meta.id);
            let spool = crate::routes::upload::protected::tus_spool_path(&meta.id);
            let spooled = tokio::fs::metadata(&spool)
                .await
                .map_err(|_| AppError::Internal("protected upload spool missing".into()))?;
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
                    .db_call("get_tus_protection_row", move |db| {
                        db::get_file(db, &lookup)
                    })
                    .await?
                    .ok_or_else(|| AppError::BadRequest("invalid reserve_id".into()))?;
                let material = ProtectionMaterial::from_record(&reservation);
                let dek = match material.dek_escrow.as_deref() {
                    Some(escrow) => crate::routes::upload::protected::unwrap_escrow(state, escrow)?,
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
        let mut completed = crate::reservation::finish(
            state,
            crate::reservation::FinishRequest {
                id: reserve_id.to_string(),
                delete_token: reservation_token,
                filename: meta.filename.clone(),
                mime_type: meta.mime_type.clone(),
                size_bytes: meta.total_length as i64,
                uploader_ip: Some(meta.encrypted_ip.clone()),
            },
            crate::reservation::FinishPolicy::Strict,
        )
        .await?;
        let completed_id = &completed.id;
        let completed_size = completed.size_bytes;
        tracing::info!("reserved TUS upload completed: id={completed_id} size={completed_size}");
        if let Some((ref material, _)) = protected {
            crate::routes::upload::protected::mark_protected(state, &completed.id, material)
                .await?;
            material.apply_to(&mut completed);
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
            material.apply_to(&mut record);
        }
        db::insert_new_file(state, record).await?
    };

    let owned_record = record.clone();
    let owner = meta.user_id.clone();
    state
        .db_call("own_tus_file", move |db| {
            db::add_client_file(db, &owner, &owned_record)
        })
        .await?;

    Ok(
        serde_json::to_value(crate::routes::upload::UploadResponse::from_record(
            state, &record, true,
        ))
        .unwrap_or_default(),
    )
}
