use std::sync::{Arc, atomic::Ordering};

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

/// Returns ordered parts once, together with the part TUS ids needed for
/// teardown; the winner marks `finalizing` so a late part cannot resume. The
/// caller tears down only after the concat succeeds, so a failure stays
/// retryable.
pub(crate) fn try_collect_finalized_parts(
    state: &Arc<AppState>,
    session_id: &str,
    part_index: usize,
    total: usize,
) -> Option<(Vec<String>, u64, Vec<String>)> {
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
    Some((part_ids, full_size, part_tus_ids))
}

/// Drops all part state after a successful concat. `finalizing` stays set so a
/// late part cannot start a second assembly.
pub(crate) fn cleanup_finalized_parts(
    state: &Arc<AppState>,
    session_id: &str,
    part_tus_ids: &[String],
) {
    state.part_sessions.remove(session_id);
    for pid in part_tus_ids {
        state.tus.remove(pid);
        state.tus_senders.remove(pid);
        state.push_handles.remove(pid);
    }
}

/// A failed, or cancelled, concat releases the guard so the client can retry
/// the final part. All part state is left untouched for re-collection.
fn allow_finalize_retry(sessions: &crate::tus::PartSessionMap, session_id: &str) {
    if let Some(session) = sessions.get(session_id) {
        session.finalizing.store(false, Ordering::SeqCst);
    }
}

/// RAII release for `PartSession::finalizing`.
///
/// The final PATCH can be aborted (browser watchdog or user cancel) while
/// `finalize_concat` is in flight, dropping its future before a success or
/// error branch runs. Holding this guard across the await guarantees the flag
/// is cleared on cancellation; `disarm` is only called after the parts have
/// been torn down on success.
struct FinalizeGuard {
    sessions: crate::tus::PartSessionMap,
    session_id: String,
    armed: bool,
}

impl FinalizeGuard {
    fn new(sessions: &crate::tus::PartSessionMap, session_id: &str) -> Self {
        Self {
            sessions: Arc::clone(sessions),
            session_id: session_id.to_string(),
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for FinalizeGuard {
    fn drop(&mut self) {
        if self.armed {
            allow_finalize_retry(&self.sessions, &self.session_id);
        }
    }
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
            AppError::from_concat_error(e)
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
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        Some((part_ids, full_size, part_tus_ids)) => {
            // The guard releases `finalizing` if this future is dropped while
            // the concat is in flight, so a cancelled PATCH stays retryable.
            let mut guard = FinalizeGuard::new(&state.part_sessions, session_id);
            let result = finalize_concat(state, &meta, session_id, &part_ids, full_size).await;
            match result {
                Ok(value) => {
                    cleanup_finalized_parts(state, session_id, &part_tus_ids);
                    guard.disarm();
                    Ok(value)
                }
                Err(err) => Err(err),
            }
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

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    };

    use super::*;
    use crate::upload_mode::UploadMode;

    fn test_session() -> Arc<crate::tus::PartSession> {
        Arc::new(crate::tus::PartSession {
            session_id: "session".into(),
            total_parts: 2,
            filename: "file.bin".into(),
            mime_type: "application/octet-stream".into(),
            storage_host: None,
            ttl_hours: 24.0,
            reserve_id: None,
            reservation_token: None,
            capability: "cap".into(),
            user_id: "user".into(),
            upload_mode: UploadMode::Standard,
            declared_size: AtomicU64::new(0),
            finalized_parts: dashmap::DashMap::new(),
            finalizing: AtomicBool::new(true),
            part_ids: dashmap::DashMap::new(),
            part_lengths: dashmap::DashMap::new(),
            completion_reserve_id: std::sync::Mutex::new(None),
        })
    }

    #[test]
    fn guard_releases_finalizing_on_drop_but_not_when_disarmed() {
        let sessions = crate::tus::new_part_session_map();
        let session = test_session();
        sessions.insert("session".into(), Arc::clone(&session));

        {
            let mut guard = FinalizeGuard::new(&sessions, "session");
            guard.disarm();
        }
        assert!(session.finalizing.load(Ordering::SeqCst));

        {
            let _guard = FinalizeGuard::new(&sessions, "session");
            assert!(session.finalizing.load(Ordering::SeqCst));
        }
        assert!(!session.finalizing.load(Ordering::SeqCst));
    }
}
