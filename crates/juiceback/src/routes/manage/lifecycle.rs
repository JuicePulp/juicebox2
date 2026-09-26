use std::sync::Arc;

use crate::{db, error::AppError, state::AppState};

pub async fn pick_new_id(
    state: &Arc<AppState>,
    custom_raw: &str,
) -> Result<(String, bool), AppError> {
    if custom_raw.is_empty() {
        return Ok((nanoid::nanoid!(8), false));
    }
    let normalized = crate::utils::normalize_custom_id(custom_raw);
    if !crate::utils::is_valid_id(&normalized) {
        return Err(AppError::BadRequest(format!(
            "invalid custom ID: must be {}-{} chars, alphanumeric, hyphens, or underscores",
            crate::constants::MIN_CUSTOM_ID_LEN,
            crate::constants::MAX_CUSTOM_ID_LEN,
        )));
    }
    let probe = normalized.clone();
    let exists = state
        .db_call("get_file", move |db| db::get_file(db, &probe))
        .await?;
    if exists.is_some() {
        return Err(AppError::Conflict("custom ID is already taken".into()));
    }
    Ok((normalized, true))
}

pub async fn swap_ids_transactional(
    state: &Arc<AppState>,
    user_id: &str,
    old_id: &str,
    new_id: &str,
) -> Result<bool, AppError> {
    let alias_old = old_id.to_string();
    let alias_new = new_id.to_string();
    let old_id = old_id.to_string();
    let new_id2 = new_id.to_string();
    let storage_path = format!("remote-{new_id2}");
    let owner = user_id.to_string();
    let old_owned_id = old_id.clone();
    let new_owned_id = new_id2.clone();
    state
        .db_transaction("renew_file_ids", move |tx| {
            let _ = db::insert_alias(tx, &alias_old, &alias_new);
            let updated = db::renew_file_id(tx, &old_id, &new_id2, &storage_path)?;
            db::update_client_file_id_in_tx(tx, &owner, &old_owned_id, &new_owned_id)?;
            Ok(updated)
        })
        .await
}

pub async fn rename_on_host_with_rollback(
    state: &Arc<AppState>,
    old_id: &str,
    new_id: &str,
    host: Option<&str>,
    capability: &str,
) -> Result<(), AppError> {
    if let Err(e) = crate::storage_client::rename_file_on_juicehost(
        state,
        old_id,
        new_id,
        host,
        Some(capability),
    )
    .await
    {
        tracing::error!("lifecycle: storage-host rename failed for id={old_id}, rolling back: {e}");
        rollback_swap(state, new_id, old_id).await;
        return Err(AppError::JuicehostRejected(e));
    }
    Ok(())
}

async fn rollback_swap(state: &Arc<AppState>, old_id: &str, new_id: &str) {
    let old_id = old_id.to_string();
    let new_id = new_id.to_string();
    let alias_id = new_id.clone();
    let storage_path = format!("remote-{new_id}");
    let _ = state
        .db_call("renew_file_id_rollback", move |db| {
            db::renew_file_id(db, &old_id, &new_id, &storage_path)
        })
        .await;

    let _ = state
        .db_call("delete_alias", move |db| db::delete_alias(db, &alias_id))
        .await;
}
