use std::sync::Arc;

use bytes::Bytes;
use tokio::sync::mpsc;

use crate::{error::AppError, state::AppState};

pub(crate) async fn await_storage_push(
    state: &Arc<AppState>,
    upload_id: &str,
) -> Result<(), AppError> {
    let Some((_, handle)) = state.push_handles.remove(upload_id) else {
        return Err(AppError::TusSessionNotFound);
    };
    handle
        .await
        .map_err(|e| AppError::TaskPanicked(format!("TUS storage task for {upload_id}: {e}")))?
        .map_err(AppError::from_juicehost_error)
}

pub(crate) fn spawn_push_task(
    state: &Arc<AppState>,
    id: &str,
    rx: mpsc::Receiver<Result<Bytes, String>>,
) {
    let Some(upload) = state.tus.get(id) else {
        return;
    };
    let push_filename = upload.filename.clone();
    let push_mime = upload.mime_type.clone();
    let push_host = upload.storage_host.clone();
    let push_mode = upload.upload_mode;
    let push_capability = upload.capability.clone();
    let sid = upload.session_id.clone();
    let pi = upload.part_index;
    let total_len = upload.total_length;
    drop(upload);

    let push_state = Arc::clone(state);
    let push_id = id.to_string();
    let handle_id = id.to_string();
    let handle = tokio::spawn(async move {
        let result = crate::storage_client::push_file_streaming(
            &push_state,
            &push_id,
            &push_filename,
            &push_mime,
            rx,
            push_host,
            &push_mode,
            Some(&push_capability),
        )
        .await;
        if let Err(ref e) = result {
            tracing::warn!("tus push failed for {push_id}: {e}");

            release_parallel_slot(&push_state, &push_id, sid.as_deref(), pi, total_len);
        }
        result
    });
    state.push_handles.insert(handle_id, handle);
}

pub(crate) fn release_parallel_slot(
    state: &Arc<AppState>,
    id: &str,
    session_id: Option<&str>,
    part_index: Option<usize>,
    total_length: u64,
) {
    state.tus.remove(id);
    state.tus_senders.remove(id);
    state.push_handles.remove(id);
    let (Some(sid), Some(pi)) = (session_id, part_index) else {
        return;
    };
    use std::sync::atomic::Ordering;
    let Some(session) = state.part_sessions.get(sid) else {
        return;
    };
    let ours = session
        .part_ids
        .get(&pi)
        .map(|v| v.value() == id)
        .unwrap_or(false);
    if ours && session.part_ids.remove(&pi).is_some() {
        let _ = session
            .declared_size
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |size| {
                Some(size.saturating_sub(total_length))
            });
    }
}
