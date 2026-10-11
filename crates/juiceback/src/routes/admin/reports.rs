use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Json, Response},
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

    /// Whether the reporter supplied a gate password (preview available).
    /// The password itself is never exposed; preview decrypts server-side.
    pub has_password: bool,
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
                has_password: r.password.is_some(),
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

#[utoipa::path(
    get,
    path = "/api/admin/report/{id}/preview",
    params(
        ("id" = i64, Path, description = "The report ID to preview"),
    ),
    responses(
        (status = 200, description = "Decrypted file bytes"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "No usable password for this report"),
        (status = 404, description = "Report or file not found"),
    ),
    security(),
    tag = "Admin",
)]
#[tracing::instrument(skip_all)]
pub async fn preview_report_handler(
    admin: AdminUser,
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let report = state
        .db_call("get_report", move |db| db::get_report(db, id))
        .await?
        .ok_or(AppError::NotFound)?;
    let file_id = db::file_id_from_url(&report.file_url).ok_or(AppError::NotFound)?;
    let lookup = file_id.clone();
    let record = state
        .db_call("get_preview_file", move |db| db::get_file(db, &lookup))
        .await?
        .ok_or(AppError::NotFound)?;
    if !record.is_protected() {
        return Err(AppError::NotFound);
    }
    let password = report
        .password
        .filter(|p| !p.is_empty())
        .ok_or_else(|| AppError::Forbidden("report has no password for this file".into()))?;
    let hash = record.password_hash.clone().unwrap_or_default();
    let ok = tokio::task::spawn_blocking(move || crate::auth::verify_password(&password, &hash))
        .await
        .map_err(|_| AppError::Internal("password check panicked".into()))?
        .map_err(AppError::Internal)?;
    if !ok {
        return Err(AppError::Forbidden(
            "stored password does not open this file".into(),
        ));
    }

    tracing::warn!(
        "admin decrypt preview: admin={} report={} file={}",
        admin.claims.sub,
        report.id,
        record.id
    );

    let plain_len: u64 = u64::try_from(record.size_bytes.max(0))
        .map_err(|_| AppError::Internal("bad file size".into()))?;
    crate::routes::unlock::serve_protected_bytes(
        &state,
        &record,
        0,
        plain_len,
        &axum::http::Method::GET,
    )
    .await
}
