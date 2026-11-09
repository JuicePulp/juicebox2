use std::sync::Arc;

use futures::{StreamExt, stream::FuturesUnordered};
use tokio::sync::Semaphore;

use crate::{db, error::AppError, state::AppState};

const DELETE_PARALLELISM: usize = 20;

pub async fn delete_file_everywhere(
    state: &Arc<AppState>,
    record: &db::FileRecord,
    reason: &str,
) -> Result<bool, AppError> {
    let id = record.id.clone();
    crate::storage_client::delete_file_on_juicehost(
        state,
        &id,
        record.storage_host.as_deref(),
        Some(&record.delete_token),
    )
    .await
    .map_err(AppError::from_juicehost_error)?;

    let id_for_db = id.clone();
    let deleted = state
        .db_call("delete_file", move |db| db::delete_file(db, &id_for_db))
        .await?;

    if let Err(e) = remove_client_files(state, std::slice::from_ref(&id)).await {
        tracing::error!("{reason}: failed to remove client file mappings for id={id}: {e:?}");
    }

    crate::cloudflare::purge_file(&state.config, record);

    tracing::info!("{reason}: delete id={id} db_deleted={deleted}");
    Ok(deleted)
}

pub async fn delete_files_everywhere(
    state: &Arc<AppState>,
    records: Vec<db::FileRecord>,
    reason: &str,
) -> usize {
    let sem = Arc::new(Semaphore::new(DELETE_PARALLELISM));
    let mut pending = FuturesUnordered::new();
    for record in records {
        let permit = match Arc::clone(&sem).acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => {
                tracing::error!("{reason}: semaphore closed, aborting deletion loop");
                break;
            }
        };
        let state = Arc::clone(state);
        let id = record.id.clone();
        let host = record.storage_host.clone();
        let capability = record.delete_token.clone();
        let reason = reason.to_string();
        pending.push(tokio::spawn(async move {
            let _permit = permit;
            match crate::storage_client::delete_file_on_juicehost(
                &state,
                &id,
                host.as_deref(),
                Some(&capability),
            )
            .await
            {
                Ok(()) => Some(record),
                Err(e) => {
                    tracing::warn!("{reason}: failed to delete id={id} from juicehost: {e}");
                    None
                }
            }
        }));
    }

    let mut deleted = Vec::new();
    while let Some(result) = pending.next().await {
        match result {
            Ok(Some(record)) => deleted.push(record),
            Ok(None) => {}
            Err(e) => tracing::warn!("{reason}: juicehost delete task failed: {e}"),
        }
    }
    if deleted.is_empty() {
        return 0;
    }

    let ids: Vec<String> = deleted.iter().map(|record| record.id.clone()).collect();
    if let Err(e) = remove_client_files(state, &ids).await {
        tracing::error!("{reason}: failed to remove client file mappings: {e:?}");
    }
    crate::cloudflare::purge_files(&state.config, &deleted);

    let ids_for_db = ids;
    match state
        .db_call("delete_files_by_ids", move |db| {
            db::delete_files_by_ids(db, &ids_for_db)
        })
        .await
    {
        Ok(count) => {
            tracing::info!("{reason}: purged {count} rows from db");
            count
        }
        Err(e) => {
            tracing::error!("{reason}: failed to purge db rows: {e:?}");
            0
        }
    }
}

async fn remove_client_files(state: &Arc<AppState>, ids: &[String]) -> Result<usize, AppError> {
    let ids = ids.to_vec();
    state
        .db_call("remove_client_files", move |db| {
            let mut removed = 0;
            for id in &ids {
                removed += db.execute(
                    "DELETE FROM client_files WHERE file_id = ?1",
                    rusqlite::params![id],
                )?;
            }
            Ok(removed)
        })
        .await
}

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
