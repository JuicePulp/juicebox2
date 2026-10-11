use std::sync::Arc;

use axum::{Json, extract::State};
use serde::Serialize;
use utoipa::ToSchema;

use super::common::AdminUser;
use crate::{db, error::AppError, state::AppState};

#[derive(Serialize, ToSchema)]
pub struct AdminStatsOverview {
    
    pub site_visitors: i64,
    
    pub site_visits: i64,
    
    pub uploaders: i64,
    
    pub file_viewers: i64,
    
    pub file_views: i64,
    
    pub file_downloads: i64,
}

#[utoipa::path(
    get,
    path = "/api/admin/stats/overview",
    responses(
        (status = 200, description = "Visitor analytics totals", body = AdminStatsOverview),
        (status = 401, description = "Not authenticated"),
    ),
    security(),
    tag = "Admin",
)]
pub async fn stats_overview_handler(
    _admin: AdminUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<AdminStatsOverview>, AppError> {
    let overview = state
        .db_call("stats_overview", |db| db::site_overview(db))
        .await?;
    Ok(Json(AdminStatsOverview {
        site_visitors: overview.site_visitors,
        site_visits: overview.site_visits,
        uploaders: overview.uploaders,
        file_viewers: overview.file_viewers,
        file_views: overview.file_views,
        file_downloads: overview.file_downloads,
    }))
}
