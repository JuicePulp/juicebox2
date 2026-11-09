use std::sync::Arc;

use super::{completion::finish_tus_upload, state::await_storage_push, types::TusUploadMeta};
use crate::{
    db::{self, FileRecord},
    error::AppError,
    state::AppState,
};

/// Repeats of the same index do not inflate the count, so a retried part cannot
/// trigger assembly early.
#[must_use]
pub(crate) fn note_part_finalized(
    session: &crate::tus::PartSession,
    part_index: usize,
    total: usize,
) -> bool {
    if part_index >= total {
        return false;
    }
    session.finalized_parts.insert(part_index, ());
    session.finalized_parts.len() >= total
}

/// Returns ordered parts once; the winner marks `finalizing` so a late part
/// cannot resume.
pub(crate) fn try_collect_finalized_parts(
    state: &Arc<AppState>,
    session_id: &str,
    part_index: usize,
    total: usize,
) -> Option<(Vec<String>, u64)> {
    use std::sync::atomic::Ordering;

    if part_index >= total {
        return None;
    }
    let session = state.part_sessions.get(session_id)?;
    let all_finalized = note_part_finalized(&session, part_index, total);
    let finalized = session.finalized_parts.len();

    tracing::info!(
        "parallel part {}/{} complete for session {session_id}",
        finalized.min(total),
        total
    );

    if !all_finalized {
        return None;
    }

    let (part_ids, full_size) = match collect_ordered_parts(&session, total) {
        Some(parts) => parts,
        None => {
            tracing::warn!(
                "parallel session {session_id}: {finalized}/{total} parts finalized but the part set is incomplete; waiting for the missing part"
            );
            return None;
        }
    };

    if session.finalizing.swap(true, Ordering::SeqCst) {
        return None;
    }

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

/// `None` if an index is missing or the sum overflows.
#[must_use]
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

pub(crate) async fn finalize_concat(
    state: &Arc<AppState>,
    meta: &TusUploadMeta,
    session_id: &str,
    part_ids: &[String],
    full_size: u64,
) -> Result<serde_json::Value, AppError> {
    if let Some(reserve_id) = meta.reserve_id.as_deref() {
        let lookup = reserve_id.to_string();
        let reservation = state
            .db_call("get_concat_reservation", move |db| {
                crate::db::get_file(db, &lookup)
            })
            .await?
            .ok_or_else(|| AppError::BadRequest("invalid reserve_id".into()))?;
        if reservation.is_protected() {
            return Err(crate::routes::upload::protected::relay_required());
        }
    }
    let target_id = if let Some(ref rid) = meta.reserve_id {
        rid.clone()
    } else {
        nanoid::nanoid!(8)
    };
    let filename = meta.filename.clone();

    let existing = crate::storage_client::stat_file_on_juicehost(
        state,
        &target_id,
        meta.storage_host.as_deref(),
        Some(&meta.delete_token),
    )
    .await
    .ok()
    .flatten();
    let already_merged = existing == Some(full_size);

    if !already_merged {
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
    }

    let record = if let Some(reserve_id) = meta.reserve_id.as_deref() {
        let reservation_token = meta
            .reservation_token
            .clone()
            .ok_or_else(|| AppError::Forbidden("reservation delete token required".into()))?;
        let completed = crate::reservation::finish(
            state,
            crate::reservation::FinishRequest {
                id: reserve_id.to_string(),
                delete_token: reservation_token,
                filename: meta.filename.clone(),
                mime_type: meta.mime_type.clone(),
                size_bytes: full_size as i64,
                uploader_ip: Some(meta.encrypted_ip.clone()),
            },
            crate::reservation::FinishPolicy::Strict,
        )
        .await?;
        let completed_id = &completed.id;
        let completed_size = completed.size_bytes;
        tracing::info!(
            "reserved parallel TUS upload completed: id={completed_id} size={completed_size}"
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
        db::insert_new_file(state, record).await?
    };

    let owned_record = record.clone();
    let owner = meta.user_id.clone();
    state
        .db_call("own_parallel_tus_file", move |db| {
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

/// Only the part that observes the full set performs the concat; the rest get a
/// `part_complete` marker.
pub(crate) async fn complete_parallel_part(
    state: &Arc<AppState>,
    meta: TusUploadMeta,
    session_id: &str,
    part_index: usize,
) -> Result<serde_json::Value, AppError> {
    state.tus_senders.remove(&meta.id);
    await_storage_push(state, &meta.id).await?;

    let session = match state.part_sessions.get(session_id) {
        Some(session) => session,
        None => return finalized_parallel_part(state, &meta).await,
    };
    let total = session.total_parts;
    {
        // Every part must agree on the reservation; `None` is cached too.
        let mut expected = session
            .completion_reserve_id
            .lock()
            .expect("part session completion lock poisoned");
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

    if total <= 1 {
        state.part_sessions.remove(session_id);
        return finish_tus_upload(state, meta).await;
    }

    match try_collect_finalized_parts(state, session_id, part_index, total) {
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

/// Polls briefly for the record, since the finishing part may still be
/// mid-write.
async fn finalized_parallel_part(
    state: &Arc<AppState>,
    meta: &TusUploadMeta,
) -> Result<serde_json::Value, AppError> {
    let Some(reserve_id) = meta.reserve_id.clone() else {
        return Err(AppError::TusSessionNotFound);
    };
    let mut record = None;
    for attempt in 0..60 {
        let lookup = reserve_id.clone();
        let found = state
            .db_call("get_finalized_parallel_part", move |db| {
                db::get_file(db, &lookup)
            })
            .await?;
        match found {
            Some(found) if found.status != "uploading" => {
                record = Some(found);
                break;
            }
            Some(_) | None if attempt < 59 => {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
            _ => break,
        }
    }
    let record = record.ok_or(AppError::TusSessionNotFound)?;

    let (public_url, raw_url, download_url) = crate::utils::share_links(
        &state.config,
        record.storage_host.as_deref(),
        &record.id,
        &record.filename,
    );
    let protected = record.is_protected();
    let is_encrypted = record.is_encrypted;
    Ok(serde_json::to_value(crate::routes::upload::UploadResponse {
        id: record.id,
        url: public_url,
        raw_url,
        download_url,
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
