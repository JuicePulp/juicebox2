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
pub struct AdminFeedbackEntry {
    pub id: i64,

    pub message: String,

    pub email: Option<String>,

    pub reporter_ip_hash: Option<String>,

    pub created_at: i64,
}

#[derive(Serialize, ToSchema)]
pub struct AdminFeedbackResponse {
    pub items: Vec<AdminFeedbackEntry>,

    pub total: i64,
}

#[utoipa::path(
    get,
    path = "/api/admin/feedbacks",
    params(
        ("q" = String, Query, description = "Search query"),
        ("sort" = String, Query, description = "Sort column"),
        ("dir" = String, Query, description = "Sort direction"),
        ("offset" = i64, Query, description = "Offset"),
        ("limit" = i64, Query, description = "Limit"),
    ),
    responses(
        (status = 200, description = "Paginated feedback list", body = AdminFeedbackResponse),
        (status = 401, description = "Not authenticated"),
    ),
    security(),
    tag = "Admin",
)]
pub async fn list_feedback_handler(
    _admin: AdminUser,
    State(state): State<Arc<AppState>>,
    Query(params): Query<ListParams>,
) -> Result<Json<AdminFeedbackResponse>, AppError> {
    let search = params.q.clone();
    let sort = params.sort.clone();
    let dir = params.dir.clone();
    let offset = params.offset;
    let limit = params.limit.min(200);

    let (records, total) = state
        .db_call("list_feedback_paginated", move |db| {
            db::list_feedback_paginated(db, &search, &sort, &dir, offset, limit)
        })
        .await?;

    let entries: Vec<AdminFeedbackEntry> = records
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
            AdminFeedbackEntry {
                id: r.id,
                message: r.message,
                email: r.email,
                reporter_ip_hash: ip_hash,
                created_at: r.created_at,
            }
        })
        .collect();

    Ok(Json(AdminFeedbackResponse {
        items: entries,
        total,
    }))
}

#[utoipa::path(
    delete,
    path = "/api/admin/feedbacks/{id}",
    params(
        ("id" = i64, Path, description = "The feedback ID to delete"),
    ),
    responses(
        (status = 204, description = "Feedback deleted"),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "Feedback not found"),
    ),
    security(),
    tag = "Admin",
)]
pub async fn delete_feedback_handler(
    _admin: AdminUser,
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<StatusCode, AppError> {
    let deleted = state
        .db_call("delete_feedback", move |db| db::delete_feedback(db, id))
        .await?;

    if !deleted {
        return Err(AppError::NotFound);
    }

    tracing::info!("admin delete feedback: id={id}");

    Ok(StatusCode::NO_CONTENT)
}
