use std::sync::Arc;

use super::{state::await_storage_push, types::TusUploadMeta};
use crate::{
    db::{self, FileRecord},
    error::AppError,
    state::AppState,
};

/// Finish a non-parallel TUS upload by closing the sender and writing the DB
/// record
pub(crate) async fn complete_tus_upload(
    state: &Arc<AppState>,
    meta: TusUploadMeta,
) -> Result<serde_json::Value, AppError> {
    // Close the sender side so the push task sees EOF and finishes.
    state.tus_senders.remove(&meta.id);
    await_storage_push(state, &meta.id).await?;
    finish_tus_upload(state, meta).await
}

/// Finalize a TUS upload whose storage push has already been awaited: write the
/// DB record and return the public URL response.
pub(crate) async fn finish_tus_upload(
    state: &Arc<AppState>,
    meta: TusUploadMeta,
) -> Result<serde_json::Value, AppError> {
    if let Some(ref reserve_id) = meta.reserve_id {
        crate::storage_client::rename_file_on_juicehost(
            state,
            &meta.id,
            reserve_id,
            meta.storage_host.as_deref(),
            meta.reservation_token.as_deref(),
        )
        .await
        .map_err(AppError::from_juicehost_error)?;
    }

    let record = if let Some(ref reserve_id) = meta.reserve_id {
        // This was a pre-reserved upload (Quick Link). Update the existing record.
        let rid = reserve_id.clone();
        let reservation_token = meta
            .reservation_token
            .clone()
            .ok_or_else(|| AppError::Forbidden("reservation delete token required".into()))?;
        let update_id = rid;
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
        tracing::info!(
            "reserved TUS upload completed: id={} size={}",
            completed.id,
            completed.size_bytes
        );
        completed
    } else {
        let record = FileRecord::from_upload_with_token(
            meta.id.clone(),
            meta.filename,
            meta.mime_type,
            meta.total_length as i64,
            meta.ttl_hours,
            meta.delete_token,
            Some(meta.encrypted_ip),
            meta.storage_host.clone(),
        );
        Box::new(db::insert_new_file(state, record).await?)
    };

    let owned_record = record.clone();
    let owner = meta.user_id.clone();
    state
        .db_call("own_tus_file", move |db| {
            db::add_client_file(db, &owner, &owned_record)
        })
        .await?;

    let public_url = crate::utils::public_url(
        &state.config.public_base_url,
        &record.storage_host,
        &record.id,
        &record.filename,
    );

    Ok(serde_json::to_value(crate::routes::upload::UploadResponse {
        id: record.id,
        url: public_url,
        filename: record.filename,
        size_bytes: record.size_bytes,
        mime_type: record.mime_type,
        expires_at: record.expires_at,
        delete_token: record.delete_token,
        status: record.status,
    })
    .unwrap_or_default())
}
