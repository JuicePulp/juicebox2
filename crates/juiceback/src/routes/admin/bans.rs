use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::common::{AdminUser, ListParams};
use crate::{db, error::AppError, state::AppState};

#[derive(Serialize, ToSchema)]
pub struct AdminBansResponse {
    pub items: Vec<db::BanRecord>,
    pub total: i64,
}

#[utoipa::path(
    get,
    path = "/api/admin/bans",
    params(
        ("q" = String, Query, description = "Search query"),
        ("sort" = String, Query, description = "Sort column"),
        ("dir" = String, Query, description = "Sort direction"),
        ("offset" = i64, Query, description = "Offset"),
        ("limit" = i64, Query, description = "Limit"),
    ),
    responses(
        (status = 200, description = "Paginated ban list", body = AdminBansResponse),
        (status = 401, description = "Not authenticated"),
    ),
    security(),
    tag = "Admin",
)]
pub async fn list_bans_handler(
    _admin: AdminUser,
    State(state): State<Arc<AppState>>,
    Query(params): Query<ListParams>,
) -> Result<Json<AdminBansResponse>, AppError> {
    let search = params.q.clone();
    let sort = params.sort.clone();
    let dir = params.dir.clone();
    let offset = params.offset;
    let limit = params.limit.min(200);

    let (records, total) = state
        .db_call("list_banned_ips_paginated", move |db| {
            db::list_banned_ips_paginated(db, &search, &sort, &dir, offset, limit)
        })
        .await?;

    Ok(Json(AdminBansResponse {
        items: records,
        total,
    }))
}

#[derive(Deserialize, ToSchema)]
pub struct BanIpRequest {
    /// Pre-computed HMAC hash from click-to-ban if you have one
    #[serde(default)]
    pub hash: Option<String>,
    /// Raw IP address that gets hashed server-side before storage if hash isn't
    /// provided
    #[serde(default)]
    pub ip: Option<String>,
    #[serde(default)]
    pub reason: String,
}

#[derive(Serialize, ToSchema)]
pub struct BanExportEntry {
    /// Pre-computed HMAC hash (this is what the DB stores, never a raw IP)
    pub hash: String,
    pub reason: String,
    pub banned_by: String,
    pub banned_at: i64,
}

#[derive(Serialize, ToSchema)]
pub struct BanExportResponse {
    pub entries: Vec<BanExportEntry>,
    pub count: usize,
}

#[derive(Deserialize, ToSchema)]
pub struct BanImportEntry {
    /// Pre-computed HMAC hash; stored as-is
    #[serde(default)]
    pub hash: Option<String>,
    /// Raw IP address; hashed server-side before storage
    #[serde(default)]
    pub ip: Option<String>,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub banned_by: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct BanImportRequest {
    pub entries: Vec<BanImportEntry>,
}

#[derive(Serialize, ToSchema)]
pub struct BanImportResponse {
    pub imported: usize,
    #[serde(default)]
    pub errors: Vec<String>,
}

#[utoipa::path(
    post,
    path = "/api/admin/bans",
    request_body(content = BanIpRequest, description = "IP address and optional reason to ban"),
    responses(
        (status = 201, description = "IP banned"),
        (status = 400, description = "IP is required"),
        (status = 401, description = "Not authenticated"),
    ),
    security(),
    tag = "Admin",
)]
pub async fn ban_ip_handler(
    admin: AdminUser,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<BanIpRequest>,
) -> Result<StatusCode, AppError> {
    let ban_hash = if let Some(ref hash) = payload.hash {
        hash.trim().to_string()
    } else if let Some(ref ip) = payload.ip {
        crate::utils::hash_ip_for_ban(ip.trim(), &state.config.ip_pepper)
    } else {
        return Err(AppError::BadRequest("either hash or ip is required".into()));
    };

    if ban_hash.is_empty() {
        return Err(AppError::BadRequest("hash or ip is required".into()));
    }

    let reason = if payload.reason.trim().is_empty() {
        "no reason given".to_string()
    } else {
        payload.reason.trim().to_string()
    };

    state.ban_ip(&ban_hash, &reason, &admin.claims.sub).await?;

    tracing::info!(
        "admin banned hash={} reason={} by={}",
        crate::utils::truncate_hash(&ban_hash),
        reason,
        admin.claims.sub
    );

    Ok(StatusCode::CREATED)
}

#[utoipa::path(
    delete,
    path = "/api/admin/ban/{ip}",
    params(
        ("ip" = String, Path, description = "The IP address to unban"),
    ),
    responses(
        (status = 204, description = "IP unbanned"),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "IP not found in ban list"),
    ),
    security(),
    tag = "Admin",
)]
pub async fn unban_ip_handler(
    _admin: AdminUser,
    State(state): State<Arc<AppState>>,
    Path(ip): Path<String>,
) -> Result<StatusCode, AppError> {
    let deleted = state.unban_ip(&ip).await?;

    if !deleted {
        return Err(AppError::NotFound);
    }

    tracing::info!("admin unbanned hash={}", crate::utils::truncate_hash(&ip));

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/api/admin/bans/export",
    responses(
        (status = 200, description = "Full ban list as a JSON document that can be re-imported"),
        (status = 401, description = "Not authenticated"),
    ),
    security(),
    tag = "Admin",
)]
pub async fn export_bans_handler(
    _admin: AdminUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<BanExportResponse>, AppError> {
    let records = state
        .db_call("list_banned_ips", db::list_banned_ips)
        .await?;

    let entries = records
        .into_iter()
        .map(|r| BanExportEntry {
            hash: r.ip,
            reason: r.reason,
            banned_by: r.banned_by,
            banned_at: r.banned_at,
        })
        .collect::<Vec<_>>();
    let count = entries.len();

    tracing::info!("admin exported {count} bans");

    Ok(Json(BanExportResponse { entries, count }))
}

#[utoipa::path(
    post,
    path = "/api/admin/bans/import",
    request_body(content = BanImportRequest, description = "A list of bans to import. Each entry needs `hash` (pre-computed) or `ip` (raw IP, hashed server-side)."),
    responses(
        (status = 201, description = "Bans imported"),
        (status = 400, description = "No valid entries to import"),
        (status = 401, description = "Not authenticated"),
    ),
    security(),
    tag = "Admin",
)]
pub async fn import_bans_handler(
    admin: AdminUser,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<BanImportRequest>,
) -> Result<(StatusCode, Json<BanImportResponse>), AppError> {
    if payload.entries.is_empty() {
        return Err(AppError::BadRequest(
            "entries must contain at least one ban".into(),
        ));
    }

    let mut bans = Vec::with_capacity(payload.entries.len());
    let mut errors = Vec::new();

    for (idx, entry) in payload.entries.into_iter().enumerate() {
        let ban_hash = if let Some(ref hash) = entry.hash {
            hash.trim().to_string()
        } else if let Some(ref ip) = entry.ip {
            crate::utils::hash_ip_for_ban(ip.trim(), &state.config.ip_pepper)
        } else {
            errors.push(format!("entry {}: missing hash or ip", idx + 1));
            continue;
        };

        if ban_hash.is_empty() {
            errors.push(format!("entry {}: empty hash or ip", idx + 1));
            continue;
        }

        let reason = if entry.reason.trim().is_empty() {
            "no reason given".to_string()
        } else {
            entry.reason.trim().to_string()
        };

        let banned_by = entry.banned_by.unwrap_or_default();
        let banned_by = if banned_by.trim().is_empty() {
            admin.claims.sub.clone()
        } else {
            banned_by.trim().to_string()
        };

        bans.push(db::ImportBan {
            ip: ban_hash,
            reason,
            banned_by,
        });
    }

    if bans.is_empty() {
        let suffix = if errors.is_empty() {
            String::new()
        } else {
            format!(": {}", errors.join("; "))
        };
        return Err(AppError::BadRequest(format!(
            "no valid entries to import{suffix}"
        )));
    }

    let imported = state.import_bans(bans).await?;

    tracing::info!(
        "admin imported {} bans ({} errors) by {}",
        imported,
        errors.len(),
        admin.claims.sub
    );

    Ok((
        StatusCode::CREATED,
        Json(BanImportResponse { imported, errors }),
    ))
}
