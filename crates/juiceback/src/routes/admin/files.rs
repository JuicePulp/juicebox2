use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
};
use serde::Serialize;
use utoipa::ToSchema;

use super::common::{AdminUser, ListParams};
use crate::{db, error::AppError, state::AppState};

#[derive(Serialize, ToSchema)]
pub struct AdminFileEntry {
    pub id: String,

    pub filename: String,

    pub mime_type: String,

    pub size_bytes: i64,

    pub uploaded_at: i64,

    pub expires_at: i64,

    pub uploader_ip_hash: Option<String>,

    pub uploader_geo: Option<String>,

    pub url: String,

    pub storage_host: Option<String>,

    pub tier: String,

    pub hot: bool,

    pub tier_state: String,

    pub tier_pinned: bool,

    pub encrypted: bool,

    pub protected: bool,

    
    pub viewers: i64,

    
    pub views: i64,

    
    pub downloaders: i64,

    
    pub downloads: i64,

    
    pub last_viewed_at: Option<i64>,
}

#[derive(Serialize, ToSchema)]
pub struct AdminFilesResponse {
    pub items: Vec<AdminFileEntry>,

    pub total: i64,
}

fn promo_eligible(
    config: &crate::config::Config,
    official: &std::collections::HashSet<String>,
    record: &db::FileRecord,
    stats: Option<&&db::FileStatsAggregate>,
    seen: Option<&(i64, i64, i64)>,
    now: i64,
) -> bool {
    if record.tier_pinned {
        return false;
    }
    if record.status != "ready" {
        return false;
    }
    if record.expires_at <= now + config.promo_min_ttl_remaining_secs {
        return false;
    }
    let views = stats.map_or(0, |s| s.views);
    let downloads = stats.map_or(0, |s| s.downloads);
    let (seen_views, seen_downloads, checked_at) = seen.copied().unwrap_or((0, 0, 0));
    let delta = (views + downloads).saturating_sub(seen_views + seen_downloads);
    if delta < config.promo_min_views as i64 {
        return false;
    }
    let elapsed_secs = now.saturating_sub(checked_at).max(1) as f64;
    if delta as f64 / (elapsed_secs / 60.0) < config.promo_min_views_per_min {
        return false;
    }
    if record.size_bytes <= 0 || record.size_bytes > config.r2_max_promote_bytes as i64 {
        return false;
    }
    let host = record.storage_host.as_deref().unwrap_or("").trim();
    if host.is_empty() {
        if config.juicehost_url.trim().is_empty() {
            return false;
        }
    } else if !official.contains(host) {
        return false;
    }
    true
}

#[utoipa::path(
    get,
    path = "/api/admin/files",
    params(
        ("q" = String, Query, description = "Search query"),
        ("sort" = String, Query, description = "Sort column"),
        ("dir" = String, Query, description = "Sort direction"),
        ("offset" = i64, Query, description = "Offset"),
        ("limit" = i64, Query, description = "Limit"),
    ),
    responses(
        (status = 200, description = "Paginated file list", body = AdminFilesResponse),
        (status = 401, description = "Not authenticated"),
    ),
    security(),
    tag = "Admin",
)]
pub async fn list_files_handler(
    _admin: AdminUser,
    State(state): State<Arc<AppState>>,
    Query(params): Query<ListParams>,
) -> Result<Json<AdminFilesResponse>, AppError> {
    let search = params.q.clone();
    let sort = params.sort.clone();
    let dir = params.dir.clone();
    let offset = params.offset;
    let limit = params.limit.min(200);

    let (records, total) = state
        .db_call("list_files_paginated", move |db| {
            db::list_files_paginated(db, &search, &sort, &dir, offset, limit)
        })
        .await?;

    let ids: Vec<String> = records.iter().map(|r| r.id.clone()).collect();
    let stats = state
        .db_call("file_stats_for_ids", {
            let ids = ids.clone();
            move |db| db::file_stats_for_ids(db, &ids)
        })
        .await?;
    let stats_by_id: std::collections::HashMap<&str, &db::FileStatsAggregate> =
        stats.iter().map(|s| (s.file_id.as_str(), s)).collect();
    let promo_seen = state
        .db_call("promo_state_for_ids", {
            let ids = ids.clone();
            move |db| db::read_promo_state(db, &ids)
        })
        .await?;
    let official = crate::jobs::promo::official_hosts(&state);
    let now = chrono::Utc::now().timestamp();
    let mut full_hash_by_id: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut geo_needed: Vec<String> = Vec::new();
    for r in &records {
        if let Some(enc) = r.uploader_ip.as_ref()
            && let Some(ip) = crate::utils::decrypt_ip(enc, &state.config.ip_encryption_key)
        {
            let h = crate::utils::hash_ip_for_ban(&ip, &state.config.ip_pepper);
            geo_needed.push(h.clone());
            full_hash_by_id.insert(r.id.clone(), h);
        }
    }
    let geo_by_hash = state
        .db_call("geo_lookup", move |db| {
            db::geo::ip_locations_for_hashes(db, &geo_needed)
        })
        .await?;

    let files: Vec<AdminFileEntry> = records
        .into_iter()
        .map(|r| {
            let url = crate::utils::share_link(
                &state.config,
                r.storage_host.as_deref(),
                &r.id,
                &r.filename,
            );

            let ip_hash = r
                .uploader_ip
                .as_ref()
                .and_then(|enc| crate::utils::decrypt_ip(enc, &state.config.ip_encryption_key))
                .map(|ip| {
                    let h = crate::utils::hash_ip_for_ban(&ip, &state.config.ip_pepper);
                    crate::utils::truncate_hash(&h).to_string()
                });
            let uploader_geo = full_hash_by_id
                .get(r.id.as_str())
                .and_then(|h| geo_by_hash.get(h.as_str()))
                .map(|g| {
                    let mut parts: Vec<&str> = Vec::new();
                    if !g.city.is_empty() {
                        parts.push(g.city.as_str());
                    }
                    if !g.country.is_empty() {
                        parts.push(g.country.as_str());
                    }
                    parts.join(", ")
                })
                .filter(|s| !s.is_empty());
            let tier_state = if r.tier_pinned {
                r.tier.clone()
            } else {
                match (
                    r.tier.as_str(),
                    promo_eligible(
                        &state.config,
                        &official,
                        &r,
                        stats_by_id.get(r.id.as_str()),
                        promo_seen.get(r.id.as_str()),
                        now,
                    ),
                ) {
                    ("hot", true) => "hot",
                    ("hot", false) => "cooling",
                    (_, true) => "warming",
                    (tier, false) => tier,
                }
                .to_string()
            };
            AdminFileEntry {
                id: r.id.clone(),
                filename: r.filename,
                mime_type: r.mime_type,
                size_bytes: r.size_bytes,
                uploaded_at: r.uploaded_at,
                expires_at: r.expires_at,
                uploader_ip_hash: ip_hash,
                uploader_geo,
                url,
                storage_host: r.storage_host,
                tier: r.tier.clone(),
                hot: r.tier == "hot",
                tier_state,
                tier_pinned: r.tier_pinned,
                encrypted: r.is_encrypted,
                protected: r.password_hash.is_some(),
                viewers: stats_by_id.get(r.id.as_str()).map_or(0, |s| s.viewers),
                views: stats_by_id.get(r.id.as_str()).map_or(0, |s| s.views),
                downloaders: stats_by_id.get(r.id.as_str()).map_or(0, |s| s.downloaders),
                downloads: stats_by_id.get(r.id.as_str()).map_or(0, |s| s.downloads),
                last_viewed_at: stats_by_id.get(r.id.as_str()).map(|s| s.last_seen_at),
            }
        })
        .collect();

    Ok(Json(AdminFilesResponse {
        items: files,
        total,
    }))
}

#[utoipa::path(
    delete,
    path = "/api/admin/file/{id}",
    params(
        ("id" = String, Path, description = "The file ID to delete"),
    ),
    responses(
        (status = 204, description = "File deleted"),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "File not found"),
    ),
    security(),
    tag = "Admin",
)]
pub async fn delete_file_handler(
    _admin: AdminUser,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    let id_for_get = id.clone();
    let record = state
        .db_call("get_file", move |db| db::get_file(db, &id_for_get))
        .await?
        .ok_or(AppError::NotFound)?;

    let host_opt = record.storage_host.as_deref();
    crate::storage_client::delete_file_on_juicehost(
        &state,
        &id,
        host_opt,
        Some(&record.delete_token),
    )
    .await
    .map_err(AppError::from_juicehost_error)?;

    let id_for_delete = id.clone();
    state
        .db_call("delete_file", move |db| db::delete_file(db, &id_for_delete))
        .await?;

    crate::cloudflare::purge_file(
        &state.config,
        &record,
    );

    tracing::info!("admin delete: id={id}");

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/admin/file/{id}/warm",
    params(
        ("id" = String, Path, description = "The file ID to promote to R2 now"),
    ),
    responses(
        (status = 200, description = "File promoted to hot tier"),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "File not found"),
    ),
    security(),
    tag = "Admin",
)]
pub async fn warm_file_handler(
    _admin: AdminUser,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    crate::jobs::promo::force_promote(&state, &id)
        .await
        .map_err(AppError::Internal)?;
    tracing::info!("admin warm: id={id}");
    Ok(Json(serde_json::json!({ "tier": "hot", "tier_pinned": true })))
}

#[utoipa::path(
    post,
    path = "/api/admin/file/{id}/cool",
    params(
        ("id" = String, Path, description = "The file ID to pull back from R2"),
    ),
    responses(
        (status = 200, description = "File demoted to standard tier"),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "File not found"),
    ),
    security(),
    tag = "Admin",
)]
pub async fn cool_file_handler(
    _admin: AdminUser,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    crate::jobs::promo::force_cool(&state, &id)
        .await
        .map_err(AppError::Internal)?;
    tracing::info!("admin cool: id={id}");
    Ok(Json(serde_json::json!({ "tier": "standard", "tier_pinned": true })))
}

#[utoipa::path(
    post,
    path = "/api/admin/file/{id}/freeze",
    params(
        ("id" = String, Path, description = "The file ID to freeze"),
    ),
    responses(
        (status = 200, description = "File frozen"),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "File not found"),
    ),
    security(),
    tag = "Admin",
)]
pub async fn freeze_file_handler(
    _admin: AdminUser,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let lookup = id.clone();
    let record = state
        .db_call("get_file", move |db| db::get_file(db, &lookup))
        .await?
        .ok_or(AppError::NotFound)?;
    let host = record
        .storage_host
        .as_deref()
        .map(str::trim)
        .filter(|h| !h.is_empty());
    crate::storage_client::freeze_file_on_juicehost(&state, &id, host)
        .await
        .map_err(AppError::Internal)?;
    tracing::info!("admin freeze: id={id}");
    Ok(Json(serde_json::json!({ "frozen": true })))
}

#[utoipa::path(
    post,
    path = "/api/admin/file/{id}/unfreeze",
    params(
        ("id" = String, Path, description = "The file ID to unfreeze"),
    ),
    responses(
        (status = 200, description = "File unfrozen"),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "File not found"),
    ),
    security(),
    tag = "Admin",
)]
pub async fn unfreeze_file_handler(
    _admin: AdminUser,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let lookup = id.clone();
    let record = state
        .db_call("get_file", move |db| db::get_file(db, &lookup))
        .await?
        .ok_or(AppError::NotFound)?;
    let host = record
        .storage_host
        .as_deref()
        .map(str::trim)
        .filter(|h| !h.is_empty());
    crate::storage_client::unfreeze_file_on_juicehost(&state, &id, host)
        .await
        .map_err(AppError::Internal)?;
    tracing::info!("admin unfreeze: id={id}");
    Ok(Json(serde_json::json!({ "frozen": false })))
}
