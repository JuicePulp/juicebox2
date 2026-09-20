use std::sync::Arc;

use super::{completion::finish_tus_upload, state::await_storage_push, types::TusUploadMeta};
use crate::{
    db::{self, FileRecord},
    error::AppError,
    state::AppState,
};

/// Check if all parallel parts are complete, and if so, collect ordered part
/// IDs.
pub(crate) fn try_collect_finalized_parts(
    state: &Arc<AppState>,
    session_id: &str,
    total: usize,
) -> Option<(Vec<String>, u64)> {
    let completed = {
        let session = state.part_sessions.get(session_id)?;
        use std::sync::atomic::Ordering;
        session.completed.fetch_add(1, Ordering::SeqCst) + 1
    };

    tracing::info!(
        "parallel part {}/{} complete for session {}",
        completed,
        total,
        session_id
    );

    // Only the exact winner finalizes: duplicate completion signals
    // (completed > total) must not proceed, and the session may have been
    // reaped between the counter bump and this lookup.
    if completed != total {
        return None;
    }

    let session = state.part_sessions.get(session_id)?;
    let (part_ids, full_size) = collect_ordered_parts(&session, total)?;
    let part_tus_ids: Vec<String> = session.part_ids.iter().map(|e| e.value().clone()).collect();
    drop(session);
    state.part_sessions.remove(session_id);

    for pid in &part_tus_ids {
        state.tus.remove(pid);
        state.tus_senders.remove(pid);
        state.push_handles.remove(pid);
    }

    Some((part_ids, full_size))
}

pub(crate) fn collect_ordered_parts(
    session: &crate::tus::PartSession,
    total: usize,
) -> Option<(Vec<String>, u64)> {
    let ordered_parts: Option<Vec<(String, u64)>> = (0..total)
        .map(|i| {
            Some((
                session.part_ids.get(&i)?.value().clone(),
                *session.part_lengths.get(&i)?.value(),
            ))
        })
        .collect();
    let ordered_parts = ordered_parts?;
    let full_size = ordered_parts
        .iter()
        .try_fold(0_u64, |sum, (_, length)| sum.checked_add(*length))?;
    let part_ids = ordered_parts.into_iter().map(|(id, _)| id).collect();
    Some((part_ids, full_size))
}

/// Concat parts on juicehost and create the merged DB record.
pub(crate) async fn finalize_concat(
    state: &Arc<AppState>,
    meta: &TusUploadMeta,
    session_id: &str,
    part_ids: &[String],
    full_size: u64,
) -> Result<serde_json::Value, AppError> {
    let target_id = if let Some(ref rid) = meta.reserve_id {
        rid.clone()
    } else {
        nanoid::nanoid!(8)
    };
    let filename = meta.filename.clone();

    crate::storage_client::concat_files(
        state,
        &target_id,
        &filename,
        part_ids,
        meta.storage_host.as_deref(),
        Some(&meta.delete_token),
    )
    .await
    .map_err(|e| {
        tracing::error!("concat failed for session {session_id}: {e}");
        AppError::Internal(format!("concat failed: {e}"))
    })?;

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
        let size = full_size as i64;
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
            "reserved parallel TUS upload completed: id={} size={}",
            completed.id,
            completed.size_bytes
        );
        completed
    } else {
        let record = FileRecord::from_upload_with_token(
            target_id,
            filename,
            meta.mime_type.clone(),
            full_size as i64,
            meta.ttl_hours,
            meta.delete_token.clone(),
            Some(meta.encrypted_ip.clone()),
            meta.storage_host.clone(),
        );
        Box::new(db::insert_new_file(state, record).await?)
    };

    let owned_record = record.clone();
    let owner = meta.user_id.clone();
    state
        .db_call("own_parallel_tus_file", move |db| {
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

/// Complete a parallel upload session: await push, register part, maybe concat.
pub(crate) async fn complete_parallel_part(
    state: &Arc<AppState>,
    meta: TusUploadMeta,
    session_id: &str,
    part_index: usize,
) -> Result<serde_json::Value, AppError> {
    state.tus_senders.remove(&meta.id);
    await_storage_push(state, &meta.id).await?;

    let session = state.part_sessions.get(session_id).ok_or_else(|| {
        tracing::error!("part session {session_id} not found");
        AppError::TusSessionNotFound
    })?;
    let total = session.total_parts;
    {
        let mut expected = session.completion_reserve_id.lock().unwrap();
        match expected.as_ref() {
            Some(reserve_id) if reserve_id != &meta.reserve_id => {
                return Err(AppError::BadRequest(
                    "parallel upload reserve_id mismatch".into(),
                ));
            }
            None => *expected = Some(meta.reserve_id.clone()),
            _ => {}
        }
    }
    drop(session);

    // Single-part uploads have nothing to concat: the streamed file already
    // IS the final file. Skip the parallel bookkeeping and finalize directly.
    if total <= 1 {
        state.part_sessions.remove(session_id);
        return finish_tus_upload(state, meta).await;
    }

    match try_collect_finalized_parts(state, session_id, total) {
        Some((part_ids, full_size)) => {
            finalize_concat(state, &meta, session_id, &part_ids, full_size).await
        }
        None => Ok(serde_json::json!({
            "status": "part_complete",
            "part": part_index + 1,
            "total": total,
        })),
    }
}
