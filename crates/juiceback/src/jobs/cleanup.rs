use std::{sync::Arc, time::Duration};

use chrono::Utc;

use crate::{db, error::AppError, state::AppState};

const MINT_LIMITER_PRUNE_INTERVAL_SECS: u64 = 120;

pub async fn run_cleanup_loop(state: Arc<AppState>) {
    let interval = Duration::from_secs(state.config.cleanup_interval_minutes * 60);
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        ticker.tick().await;

        let state_clone = Arc::clone(&state);
        let result = tokio::spawn(async move { run_cleanup_once(&state_clone).await }).await;
        if let Err(e) = result {
            if e.is_panic() {
                tracing::error!(
                    "cleanup: run_cleanup_once panicked, will retry next cycle: {}",
                    e
                );
            } else {
                tracing::error!("cleanup: run_cleanup_once task cancelled: {e}");
            }
        }
    }
}

async fn run_cleanup_once(state: &Arc<AppState>) {
    tracing::info!("cleanup: starting");

    let now = Utc::now().timestamp();
    let report_cutoff =
        now - state.config.report_retention_days as i64 * crate::constants::SECONDS_PER_DAY;
    let feedback_cutoff =
        now - state.config.feedback_retention_days as i64 * crate::constants::SECONDS_PER_DAY;
    match state
        .db_call("cleanup_ephemeral_data", move |db| {
            db::cleanup_ephemeral_data(db, now, report_cutoff, feedback_cutoff)
        })
        .await
    {
        Ok(count) if count > 0 => tracing::info!("cleanup: purged {count} ephemeral rows"),
        Err(error) => tracing::error!("cleanup: ephemeral data purge failed: {:?}", error),
        _ => {}
    }

    match state
        .db_call("cleanup_orphan_stats", |db| {
            db::delete_orphan_file_stats(db)
        })
        .await
    {
        Ok(count) if count > 0 => tracing::info!("cleanup: purged {count} orphan stat rows"),
        Err(error) => tracing::error!("cleanup: orphan stats purge failed: {:?}", error),
        _ => {}
    }

    let mut cursor = String::new();
    loop {
        let state2 = Arc::clone(state);
        let page_cursor = cursor.clone();
        let batch =
            match tokio::task::spawn_blocking(move || -> Result<Vec<db::FileRecord>, AppError> {
                let db = state2
                    .db
                    .get()
                    .map_err(|e| AppError::DbPoolError(e.to_string()))?;
                db::list_expired_batch(&db, &page_cursor, crate::constants::CLEANUP_BATCH_SIZE)
                    .map_err(AppError::DatabaseError)
            })
            .await
            {
                Ok(Ok(records)) => records,
                Ok(Err(e)) => {
                    tracing::error!("cleanup: db query failed: {:?}", e);
                    return;
                }
                Err(e) => {
                    tracing::error!("cleanup: spawn_blocking panicked: {e}");
                    return;
                }
            };

        if batch.is_empty() {
            break;
        }
        let full_page = batch.len() >= crate::constants::CLEANUP_BATCH_SIZE;
        cursor = batch.last().map(|r| r.id.clone()).unwrap_or_default();
        tracing::info!("cleanup: purging batch of {} expired files", batch.len());
        crate::routes::manage::lifecycle::delete_files_everywhere(state, batch, "cleanup").await;
        if !full_page {
            break;
        }
    }

    let stale_ids: Vec<String> = state
        .tus
        .iter()
        .filter(|e| now - e.last_activity > crate::constants::STALE_UPLOAD_TIMEOUT_SECS)
        .map(|e| e.id.clone())
        .collect();
    for id in &stale_ids {
        if let Some((_, upload)) = state.tus.remove(id) {
            tracing::info!("cleanup: removing abandoned upload id={id}");
            drop(upload);
        }
        state.tus_senders.remove(id);
        state.push_handles.remove(id);
    }
    let stale_sessions: Vec<String> = state
        .part_sessions
        .iter()
        .filter(|e| {
            let part_ids: Vec<String> = e
                .value()
                .part_ids
                .iter()
                .map(|p| p.value().clone())
                .collect();
            part_ids.iter().all(|pid| !state.tus.contains_key(pid))
        })
        .map(|e| e.key().clone())
        .collect();
    for sid in &stale_sessions {
        state.part_sessions.remove(sid);
    }

    let active_reserves: std::collections::HashSet<String> = state
        .tus
        .iter()
        .filter_map(|entry| entry.value().reserve_id.clone())
        .collect();

    let mut cursor = String::new();
    loop {
        let state4 = Arc::clone(state);
        let page_cursor = cursor.clone();
        let records =
            match tokio::task::spawn_blocking(move || -> Result<Vec<db::FileRecord>, AppError> {
                let db = state4
                    .db
                    .get()
                    .map_err(|e| AppError::DbPoolError(e.to_string()))?;
                db::list_abandoned_uploads_batch(
                    &db,
                    crate::constants::STALE_UPLOAD_TIMEOUT_SECS,
                    &page_cursor,
                    crate::constants::CLEANUP_BATCH_SIZE,
                )
                .map_err(AppError::DatabaseError)
            })
            .await
            {
                Ok(Ok(records)) => records,
                Ok(Err(e)) => {
                    tracing::error!("cleanup: abandoned uploads query failed: {:?}", e);
                    break;
                }
                Err(e) => {
                    tracing::error!("cleanup: abandoned uploads spawn_blocking panicked: {e}");
                    break;
                }
            };

        if records.is_empty() {
            break;
        }
        let full_page = records.len() >= crate::constants::CLEANUP_BATCH_SIZE;
        cursor = records.last().map(|r| r.id.clone()).unwrap_or_default();
        let abandoned: Vec<db::FileRecord> = records
            .into_iter()
            .filter(|record| !active_reserves.contains(&record.id))
            .collect();
        if abandoned.is_empty() {
            if !full_page {
                break;
            }
            continue;
        }
        tracing::info!(
            "cleanup: purging batch of {} abandoned uploads (0B, never completed)",
            abandoned.len()
        );
        let ids_to_purge: Vec<String> = abandoned.iter().map(|r| r.id.clone()).collect();
        let state5 = Arc::clone(state);
        let purge_result = tokio::task::spawn_blocking(move || -> Result<usize, AppError> {
            let db = state5
                .db
                .get()
                .map_err(|e| AppError::DbPoolError(e.to_string()))?;
            db::delete_files_by_ids(&db, &ids_to_purge).map_err(AppError::DatabaseError)
        })
        .await;

        match purge_result {
            Ok(Ok(count)) => tracing::info!("cleanup: purged {count} abandoned upload records"),
            Ok(Err(e)) => tracing::error!("cleanup: failed to purge abandoned uploads: {:?}", e),
            Err(e) => tracing::error!("cleanup: abandoned uploads purge task panicked: {e}"),
        }
        if !full_page {
            break;
        }
    }

    if !stale_ids.is_empty() || !stale_sessions.is_empty() {
        tracing::info!(
            "cleanup: removed {} abandoned uploads, {} stale sessions",
            stale_ids.len(),
            stale_sessions.len()
        );
    }
}

pub async fn run_mint_limiter_prune_loop(state: Arc<AppState>) {
    let mut ticker = tokio::time::interval(Duration::from_secs(MINT_LIMITER_PRUNE_INTERVAL_SECS));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        let before = state.mint_limiter.len_probe();
        state.mint_limiter.prune();
        if before != 0 {
            tracing::debug!("mint limiter: pruned buckets before={before}");
        }
    }
}

pub async fn run_unlock_limiter_prune_loop(state: Arc<AppState>) {
    let mut ticker = tokio::time::interval(Duration::from_secs(MINT_LIMITER_PRUNE_INTERVAL_SECS));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        let before = state.unlock_limiter.len_probe();
        state.unlock_limiter.prune();
        if before != 0 {
            tracing::debug!("unlock limiter: pruned buckets before={before}");
        }
    }
}
