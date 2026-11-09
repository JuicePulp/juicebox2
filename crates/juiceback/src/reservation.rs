use std::sync::Arc;

use crate::{
    db::{self, FileRecord, FinishReservationResult},
    error::AppError,
    state::AppState,
};

/// Record must still be uploading and the token must match, in constant time.
pub fn verify(record: Option<FileRecord>, token: &str) -> Result<FileRecord, AppError> {
    let record = record.ok_or(AppError::NotFound)?;
    if record.status != FileRecord::STATUS_UPLOADING {
        return Err(AppError::Conflict("reservation is not uploading".into()));
    }
    if !crate::utils::constant_time_eq(&record.delete_token, token) {
        return Err(AppError::Forbidden(
            "invalid reservation delete token".into(),
        ));
    }
    Ok(record)
}

pub async fn verify_by_id(
    state: &Arc<AppState>,
    id: &str,
    token: &str,
) -> Result<FileRecord, AppError> {
    if !crate::utils::is_valid_id(id) {
        return Err(AppError::BadRequest("invalid reserve_id".into()));
    }
    let lookup_id = id.to_string();
    let record = state
        .db_call("get_reservation", move |db| db::get_file(db, &lookup_id))
        .await?;
    verify(record, token)
}

/// The final stored size, not the originally declared one.
pub struct FinishRequest {
    pub id: String,
    pub delete_token: String,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub uploader_ip: Option<String>,
}

/// Retried finish returns the ready record; `Strict` reports it as a conflict.
#[derive(Clone, Copy)]
pub enum FinishPolicy {
    Strict,
    Idempotent,
}

pub async fn finish(
    state: &Arc<AppState>,
    request: FinishRequest,
    policy: FinishPolicy,
) -> Result<FileRecord, AppError> {
    if !crate::utils::is_valid_id(&request.id) {
        return Err(AppError::BadRequest("invalid reserve_id".into()));
    }
    let lookup_id = request.id.clone();
    let FinishRequest {
        id,
        delete_token,
        filename,
        mime_type,
        size_bytes,
        uploader_ip,
    } = request;
    let outcome = state
        .db_call("finish_reservation", move |db| {
            db::finish_reservation(
                db,
                &filename,
                &mime_type,
                size_bytes,
                &id,
                &delete_token,
                uploader_ip.as_deref(),
            )
        })
        .await?;
    match outcome {
        FinishReservationResult::Completed(record) => Ok(record),
        FinishReservationResult::NotFound => Err(AppError::NotFound),
        FinishReservationResult::NotUploading => match policy {
            FinishPolicy::Strict => Err(AppError::Conflict("reservation is not uploading".into())),
            FinishPolicy::Idempotent => {
                let record = state
                    .db_call("get_finished_reservation", move |db| {
                        db::get_file(db, &lookup_id)
                    })
                    .await?
                    .ok_or(AppError::NotFound)?;
                if record.status != FileRecord::STATUS_READY {
                    return Err(AppError::Conflict("reservation is not uploading".into()));
                }
                Ok(record)
            }
        },
        FinishReservationResult::InvalidToken => Err(AppError::Forbidden(
            "invalid reservation delete token".into(),
        )),
    }
}
