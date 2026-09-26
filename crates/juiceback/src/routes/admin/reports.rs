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
pub struct AdminReportEntry {
    pub id: i64,

    pub file_url: String,

    pub reason: String,

    pub details: String,

    pub reporter_ip_hash: Option<String>,

    pub email: Option<String>,

    pub created_at: i64,
}

#[derive(Serialize, ToSchema)]
pub struct AdminReportsResponse {
    pub items: Vec<AdminReportEntry>,

    pub total: i64,
}

#[utoipa::path(
    get,
    path = "/api/admin/reports",
    params(
        ("q" = String, Query, description = "Search query"),
        ("sort" = String, Query, description = "Sort column"),
        ("dir" = String, Query, description = "Sort direction"),
        ("offset" = i64, Query, description = "Offset"),
        ("limit" = i64, Query, description = "Limit"),
    ),
    responses(
        (status = 200, description = "Paginated report list", body = AdminReportsResponse),
        (status = 401, description = "Not authenticated"),
    ),
    security(),
    tag = "Admin",
)]
pub async fn list_reports_handler(
    _admin: AdminUser,
    State(state): State<Arc<AppState>>,
    Query(params): Query<ListParams>,
) -> Result<Json<AdminReportsResponse>, AppError> {
    let search = params.q.clone();
    let sort = params.sort.clone();
    let dir = params.dir.clone();
    let offset = params.offset;
    let limit = params.limit.min(200);

    let (records, total) = state
        .db_call("list_reports_paginated", move |db| {
            db::list_reports_paginated(db, &search, &sort, &dir, offset, limit)
        })
        .await?;

    let reports: Vec<AdminReportEntry> = records
        .into_iter()
        .map(|r| {
            let ip_hash = r
                .reporter_ip
                .as_ref()
                .and_then(|enc| crate::utils::decrypt_ip(enc, &state.config.ip_encryption_key))
                .map(|ip| {
                    let h = crate::utils::hash_ip_for_ban(&ip, &state.config.ip_pepper);
                    crate::utils::truncate_hash(&h).to_string()
                });
            AdminReportEntry {
                id: r.id,
                file_url: r.file_url,
                reason: r.reason,
                details: r.details,
                reporter_ip_hash: ip_hash,
                email: r.email,
                created_at: r.created_at,
            }
        })
        .collect();

    Ok(Json(AdminReportsResponse {
        items: reports,
        total,
    }))
}

#[utoipa::path(
    delete,
    path = "/api/admin/report/{id}",
    params(
        ("id" = i64, Path, description = "The report ID to delete"),
    ),
    responses(
        (status = 204, description = "Report deleted"),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "Report not found"),
    ),
    security(),
    tag = "Admin",
)]
pub async fn delete_report_handler(
    _admin: AdminUser,
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<StatusCode, AppError> {
    let deleted = state
        .db_call("delete_report", move |db| db::delete_report(db, id))
        .await?;

    if !deleted {
        return Err(AppError::NotFound);
    }

    tracing::info!("admin delete report: id={id}");

    Ok(StatusCode::NO_CONTENT)
}
