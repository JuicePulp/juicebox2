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
pub struct AdminHostersResponse {
    pub items: Vec<db::HosterRecord>,
    pub total: i64,
}

#[derive(Deserialize, ToSchema)]
pub struct HosterBanRequest {
    #[serde(default)]
    pub reason: String,
}

#[utoipa::path(
    get,
    path = "/api/admin/hosters",
    params(
        ("q" = String, Query, description = "Search query"),
        ("sort" = String, Query, description = "Sort column"),
        ("dir" = String, Query, description = "Sort direction"),
        ("offset" = i64, Query, description = "Offset"),
        ("limit" = i64, Query, description = "Limit"),
    ),
    responses(
        (status = 200, description = "Paginated list of known custom hosts", body = AdminHostersResponse),
        (status = 401, description = "Not authenticated"),
    ),
    security(),
    tag = "Admin",
)]
pub async fn list_hosters_handler(
    _admin: AdminUser,
    State(state): State<Arc<AppState>>,
    Query(params): Query<ListParams>,
) -> Result<Json<AdminHostersResponse>, AppError> {
    let search = params.q.clone();
    let sort = params.sort.clone();
    let dir = params.dir.clone();
    let offset = params.offset;
    let limit = params.limit.min(200);

    let (records, total) = state
        .db_call("list_hosters_paginated", move |db| {
            db::list_hosters_paginated(db, &search, &sort, &dir, offset, limit)
        })
        .await?;

    Ok(Json(AdminHostersResponse {
        items: records,
        total,
    }))
}

#[utoipa::path(
    post,
    path = "/api/admin/hosters/{host}/ban",
    params(
        ("host" = String, Path, description = "Hostname of the custom juicehost to ban"),
    ),
    request_body(content = HosterBanRequest, description = "Optional ban reason"),
    responses(
        (status = 201, description = "Host banned"),
        (status = 400, description = "Host is required"),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "Host not found in known hosters"),
    ),
    security(),
    tag = "Admin",
)]
pub async fn ban_hoster_handler(
    admin: AdminUser,
    State(state): State<Arc<AppState>>,
    Path(host): Path<String>,
    Json(payload): Json<HosterBanRequest>,
) -> Result<StatusCode, AppError> {
    let host = host.trim().trim_end_matches('/').to_string();
    if host.is_empty() {
        return Err(AppError::BadRequest("host is required".into()));
    }

    let reason = if payload.reason.trim().is_empty() {
        "no reason given".to_string()
    } else {
        payload.reason.trim().to_string()
    };
    let banned_by = admin.claims.sub.clone();
    let host_value = host.clone();
    let reason_value = reason.clone();
    let banned_by_value = banned_by.clone();

    let updated = state
        .db_call("update_hoster_banned", move |db| {
            db::update_hoster_banned(db, &host_value, true, &reason_value, &banned_by_value)
        })
        .await?;
    if !updated {
        return Err(AppError::NotFound);
    }

    tracing::info!(
        "admin banned host={} reason={} by={}",
        host,
        reason,
        banned_by
    );

    Ok(StatusCode::CREATED)
}

#[utoipa::path(
    delete,
    path = "/api/admin/hosters/{host}/ban",
    params(
        ("host" = String, Path, description = "Hostname of the custom juicehost to unban"),
    ),
    responses(
        (status = 204, description = "Host unbanned"),
        (status = 400, description = "Host is required"),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "Host not found in known hosters"),
    ),
    security(),
    tag = "Admin",
)]
pub async fn unban_hoster_handler(
    _admin: AdminUser,
    State(state): State<Arc<AppState>>,
    Path(host): Path<String>,
) -> Result<StatusCode, AppError> {
    let host = host.trim().trim_end_matches('/').to_string();
    if host.is_empty() {
        return Err(AppError::BadRequest("host is required".into()));
    }

    let host_value = host.clone();

    let updated = state
        .db_call("update_hoster_banned", move |db| {
            db::update_hoster_banned(db, &host_value, false, "", "")
        })
        .await?;
    if !updated {
        return Err(AppError::NotFound);
    }

    tracing::info!("admin unbanned host={host}");

    Ok(StatusCode::NO_CONTENT)
}
