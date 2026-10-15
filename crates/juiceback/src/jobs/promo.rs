use std::{collections::HashSet, sync::Arc, time::Duration};

use crate::{db, state::AppState};

/// MIME allowlist for promote-on-first-embed: previewable types only.
/// Archives / binaries never qualify, no matter how often they're embedded.
#[must_use]
pub fn embed_mime_eligible(mime: &str) -> bool {
    let base = mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    if base.starts_with("image/") || base.starts_with("text/") {
        return true;
    }
    matches!(
        base.as_str(),
        "video/mp4"
            | "application/pdf"
            | "application/json"
            | "application/javascript"
            | "application/x-javascript"
            | "application/xml"
    ) || base.ends_with("+json")
        || base.ends_with("+xml")
}

/// Promote-on-first-embed: seed R2 for small previewable files on preview
/// render / first non-bot view. Fire-and-forget from the stats hit path;
/// all gates re-checked here. Never pins: velocity + manual still own the tier.
/// Respects `promo_dry_run` (logs only).
pub(crate) async fn maybe_embed_promote(state: &Arc<AppState>, id: &str) {
    if !state.config.r2_enabled || !state.config.embed_promote_enabled {
        return;
    }
    // No public-base-URL gate (see promo_sweep_once): /f/ proxies R2.
    let now = chrono::Utc::now().timestamp();
    let lookup = id.to_string();
    let record = match state
        .db_call("get_file_embed", move |db| db::get_file(db, &lookup))
        .await
    {
        Ok(Some(r)) => r,
        _ => return,
    };
    if record.status != "ready"
        || record.tier != "standard"
        || record.tier_pinned
        || record.size_bytes <= 0
        || record.size_bytes > state.config.embed_promote_max_bytes as i64
        || record.size_bytes > state.config.r2_max_promote_bytes as i64
        || !embed_mime_eligible(&record.mime_type)
        || record.expires_at <= now + state.config.embed_promote_min_ttl_remaining_secs
    {
        return;
    }
    let official = official_hosts(state);
    let host = record.storage_host.as_deref().unwrap_or("").trim();
    let promote_host: Option<String> = if host.is_empty() {
        None
    } else if official.contains(host) {
        Some(host.to_string())
    } else {
        return;
    };
    if promote_host.is_none() && state.config.juicehost_url.trim().is_empty() {
        return;
    }
    let cooldown = state.config.embed_promote_cooldown_secs;
    let due = match state
        .db_call("embed_due", {
            let fid = record.id.clone();
            move |db| db::embed_attempt_due(db, &fid, now, cooldown)
        })
        .await
    {
        Ok(d) => d,
        Err(_) => return,
    };
    if !due {
        return;
    }
    if state
        .db_call("embed_attempt", {
            let fid = record.id.clone();
            move |db| db::record_embed_attempt(db, &fid, now)
        })
        .await
        .is_err()
    {
        return;
    }
    let (hot_count, hot_bytes) = match state.db_call("hot_usage", db::hot_usage).await {
        Ok(u) => u,
        Err(_) => return,
    };
    if hot_count.max(0) as u64 >= state.config.r2_max_hot_files
        || (hot_bytes.max(0) as u64).saturating_add(record.size_bytes.max(0) as u64)
            > state.config.r2_max_bytes
    {
        tracing::info!("promo: embed R2 cap reached, skipping {}", record.id);
        return;
    }
    if state.config.promo_dry_run {
        tracing::info!(
            "promo: [dry-run] embed would promote id={} mime={} size={}",
            record.id,
            record.mime_type,
            record.size_bytes
        );
        return;
    }
    let state_clone = Arc::clone(state);
    let pid = record.id.clone();
    tokio::spawn(async move {
        promote_one(&state_clone, &pid, promote_host.as_deref()).await;
    });
}

pub async fn run_promo_loop(state: Arc<AppState>) {
    let interval = Duration::from_secs(state.config.promo_interval_secs.max(60));
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        let sweep = Arc::clone(&state);
        let result = tokio::spawn(async move { promo_sweep_once(&sweep).await }).await;
        if let Err(e) = result {
            tracing::error!("promo: sweep task failed: {e}");
        }
    }
}

pub(crate) fn official_hosts(state: &AppState) -> HashSet<String> {
    let mut hosts = HashSet::new();
    let mut push = |raw: &str| {
        let trimmed = raw.trim().trim_end_matches('/');
        if !trimmed.is_empty() {
            hosts.insert(trimmed.to_string());
        }
    };
    push(&state.config.juicehost_url);
    push(&state.config.public_base_url);
    push(&state.config.public_juicehost_url);
    if let Some(router) = state.config.router_base_url.as_deref() {
        push(router);
    }
    for url in state.config.region_public_juicehosts.values() {
        push(url);
    }
    hosts
}

async fn promo_sweep_once(state: &Arc<AppState>) {
    if !state.config.r2_enabled {
        return;
    }
    // No public-base-URL gate: hot bytes are served from R2 under the
    // existing /f/ + /d/ + /c/ URLs (no public /hot/ endpoint), so the S3
    // copy is useful even when no public R2 domain is configured.
    let now = chrono::Utc::now().timestamp();
    let min_expires_at = now + state.config.promo_min_ttl_remaining_secs;

    let candidates = match state
        .db_call("promo_candidates", move |db| {
            db::list_promo_candidates(db, min_expires_at, 500)
        })
        .await
    {
        Ok(candidates) => candidates,
        Err(e) => {
            tracing::error!("promo: candidate query failed: {:?}", e);
            return;
        }
    };
    if candidates.is_empty() {
        return;
    }

    let ids: Vec<String> = candidates.iter().map(|c| c.id.clone()).collect();
    let stats = match state
        .db_call("promo_stats", move |db| db::file_stats_for_ids(db, &ids))
        .await
    {
        Ok(stats) => stats,
        Err(e) => {
            tracing::error!("promo: stats query failed: {:?}", e);
            return;
        }
    };
    let stats_by_id: std::collections::HashMap<&str, &db::FileStatsAggregate> =
        stats.iter().map(|s| (s.file_id.as_str(), s)).collect();

    let ids2: Vec<String> = candidates.iter().map(|c| c.id.clone()).collect();
    let seen = match state
        .db_call("promo_state", move |db| db::read_promo_state(db, &ids2))
        .await
    {
        Ok(seen) => seen,
        Err(e) => {
            tracing::error!("promo: promo_state query failed: {:?}", e);
            return;
        }
    };

    let (hot_count, hot_bytes) = match state.db_call("hot_usage", db::hot_usage).await {
        Ok(usage) => usage,
        Err(e) => {
            tracing::error!("promo: hot usage query failed: {:?}", e);
            return;
        }
    };

    let official = official_hosts(state);
    let mut reserved_bytes: u64 = hot_bytes.max(0) as u64;
    let mut reserved_count: u64 = hot_count.max(0) as u64;
    let mut state_rows: Vec<(String, i64, i64, i64)> = Vec::new();

    for candidate in &candidates {
        let Some(aggregate) = stats_by_id.get(candidate.id.as_str()) else {
            continue;
        };
        let hits = aggregate.views + aggregate.downloads;
        let (seen_views, seen_downloads, checked_at) = seen
            .get(candidate.id.as_str())
            .copied()
            .unwrap_or((0, 0, 0));
        let seen_hits = seen_views + seen_downloads;
        state_rows.push((
            candidate.id.clone(),
            aggregate.views,
            aggregate.downloads,
            now,
        ));

        let hits_delta = hits.saturating_sub(seen_hits);
        if hits_delta < state.config.promo_min_views as i64 {
            continue;
        }
        let elapsed_secs = now.saturating_sub(checked_at).max(1) as f64;
        let per_min = hits_delta as f64 / (elapsed_secs / 60.0);
        if per_min < state.config.promo_min_views_per_min {
            continue;
        }
        if candidate.size_bytes <= 0
            || candidate.size_bytes > state.config.r2_max_promote_bytes as i64
        {
            continue;
        }
        let host = candidate.storage_host.trim();
        let promote_host: Option<String> = if host.is_empty() {
            None
        } else if official.contains(host) {
            Some(host.to_string())
        } else {
            continue;
        };
        if reserved_count >= state.config.r2_max_hot_files
            || reserved_bytes.saturating_add(candidate.size_bytes as u64)
                > state.config.r2_max_bytes
        {
            tracing::info!(
                "promo: R2 cap reached ({} files, {} bytes), skipping {}",
                reserved_count,
                reserved_bytes,
                candidate.id
            );
            continue;
        }

        if state.config.promo_dry_run {
            tracing::info!(
                "promo: [dry-run] would promote id={} hits_delta={} per_min={:.1}",
                candidate.id,
                hits_delta,
                per_min
            );
            continue;
        }

        reserved_bytes = reserved_bytes.saturating_add(candidate.size_bytes as u64);
        reserved_count += 1;
        let state_clone = Arc::clone(state);
        let id = candidate.id.clone();
        let host_opt = promote_host.clone();
        tokio::spawn(async move {
            promote_one(&state_clone, &id, host_opt.as_deref()).await;
        });
    }

    if !state_rows.is_empty() {
        if let Err(e) = state
            .db_call("promo_state_upsert", move |db| {
                db::upsert_promo_state(db, &state_rows)
            })
            .await
        {
            tracing::error!("promo: promo_state upsert failed: {:?}", e);
        }
    }
    if let Err(e) = state
        .db_call("promo_state_prune", db::prune_promo_state)
        .await
    {
        tracing::warn!("promo: promo_state prune failed: {:?}", e);
    }
}

async fn promote_one(state: &Arc<AppState>, id: &str, host: Option<&str>) {
    match crate::storage_client::promote_file_on_juicehost(state, id, host, None).await {
        Ok(key) => {
            let id_owned = id.to_string();
            let key_owned = key.clone();
            match state
                .db_call("promote_mark_hot", move |db| {
                    db::set_file_tier(db, &id_owned, "hot", &key_owned)
                })
                .await
            {
                Ok(_) => tracing::info!("promo: id={id} promoted to R2 ({key})"),
                Err(e) => {
                    tracing::error!("promo: id={id} promoted but tier update failed: {:?}", e)
                }
            }
        }
        Err(e) => tracing::warn!("promo: id={id} promote failed: {e}"),
    }
}

pub(crate) async fn force_promote(state: &Arc<AppState>, id: &str) -> Result<String, String> {
    let lookup = id.to_string();
    let record = state
        .db_call("get_file", move |db| db::get_file(db, &lookup))
        .await
        .map_err(|e| format!("database error: {e:?}"))?
        .ok_or_else(|| "file not found".to_string())?;
    let official = official_hosts(state);
    let host = record.storage_host.as_deref().unwrap_or("").trim();
    let promote_host: Option<String> = if host.is_empty() {
        None
    } else if official.contains(host) {
        Some(host.to_string())
    } else {
        return Err("file lives on an unofficial host".to_string());
    };
    if promote_host.is_none() && state.config.juicehost_url.trim().is_empty() {
        return Err("no storage host configured".to_string());
    }
    let key =
        crate::storage_client::promote_file_on_juicehost(state, id, promote_host.as_deref(), None)
            .await?;
    let id_owned = id.to_string();
    let key_owned = key.clone();
    state
        .db_call("promote_mark_hot_pinned", move |db| {
            db::set_file_tier_manual(db, &id_owned, "hot", &key_owned)
        })
        .await
        .map_err(|e| format!("tier update failed: {e:?}"))?;
    Ok(key)
}

pub(crate) async fn force_cool(state: &Arc<AppState>, id: &str) -> Result<(), String> {
    let lookup = id.to_string();
    let record = state
        .db_call("get_file", move |db| db::get_file(db, &lookup))
        .await
        .map_err(|e| format!("database error: {e:?}"))?
        .ok_or_else(|| "file not found".to_string())?;
    let host = record
        .storage_host
        .as_deref()
        .map(str::trim)
        .filter(|h| !h.is_empty());
    crate::storage_client::demote_file_on_juicehost(state, id, host).await?;
    let id_owned = id.to_string();
    state
        .db_call("demote_mark_standard_pinned", move |db| {
            db::set_file_tier_manual(db, &id_owned, "standard", "")
        })
        .await
        .map_err(|e| format!("tier update failed: {e:?}"))?;
    Ok(())
}
