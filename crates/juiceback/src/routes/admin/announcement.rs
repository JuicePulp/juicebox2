use std::sync::Arc;

use axum::{extract::State, response::Json};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::common::AdminUser;
use crate::{db, error::AppError, state::AppState};

#[derive(Serialize, ToSchema)]
pub struct AnnouncementResponse {
    pub id: i64,

    pub message: String,

    pub link_url: String,

    pub mode: String,

    pub is_active: bool,

    pub created_at: i64,

    pub updated_at: i64,
}

#[derive(Deserialize, ToSchema)]
pub struct AnnouncementRequest {
    pub message: String,

    #[serde(default)]
    pub link_url: String,

    #[serde(default = "default_warning")]
    pub mode: String,

    #[serde(default = "default_true")]
    pub is_active: bool,
}

const fn default_true() -> bool {
    true
}

fn default_warning() -> String {
    "warning".to_string()
}

impl From<crate::db::Announcement> for AnnouncementResponse {
    fn from(a: crate::db::Announcement) -> Self {
        Self {
            id: a.id,
            message: a.message,
            link_url: a.link_url,
            mode: a.mode,
            is_active: a.is_active,
            created_at: a.created_at,
            updated_at: a.updated_at,
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/admin/announcement",
    responses(
        (status = 200, description = "Current announcement", body = AnnouncementResponse),
        (status = 401, description = "Not authenticated"),
    ),
    security(),
    tag = "Admin",
)]
pub async fn get_announcement_handler(
    _admin: AdminUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Option<AnnouncementResponse>>, AppError> {
    let announcement = state
        .db_call("get_latest_announcement", db::get_latest_announcement)
        .await?;

    Ok(Json(announcement.map(AnnouncementResponse::from)))
}

#[utoipa::path(
    put,
    path = "/api/admin/announcement",
    request_body(content = AnnouncementRequest, description = "Announcement to set"),
    responses(
        (status = 200, description = "Announcement updated", body = AnnouncementResponse),
        (status = 401, description = "Not authenticated"),
    ),
    security(),
    tag = "Admin",
)]
pub async fn put_announcement_handler(
    _admin: AdminUser,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<AnnouncementRequest>,
) -> Result<Json<AnnouncementResponse>, AppError> {
    let message = payload.message.trim().to_string();
    let link_url = payload.link_url.trim().to_string();
    let mode = payload.mode.trim().to_string();

    if message.len() > 500 {
        return Err(AppError::BadRequest(
            "message must be 500 characters or fewer".into(),
        ));
    }

    if !link_url.is_empty() && !link_url.starts_with("http://") && !link_url.starts_with("https://")
    {
        return Err(AppError::BadRequest(
            "link_url must start with http:// or https://".into(),
        ));
    }
    if link_url.len() > 2048 {
        return Err(AppError::BadRequest(
            "link_url must be 2048 characters or fewer".into(),
        ));
    }

    let is_active = payload.is_active;

    let announcement = state
        .db_call("upsert_announcement", move |db| {
            db::upsert_announcement(db, &message, &link_url, &mode, is_active)
        })
        .await?;

    let announcement_id = announcement.id;
    let announcement_active = announcement.is_active;
    tracing::info!("admin updated announcement: id={announcement_id} active={announcement_active}");

    crate::cloudflare::purge_urls(
        Arc::clone(&state.config),
        vec![
            format!("{}/", state.config.juiceback_origin),
            state.config.juiceback_origin.clone(),
        ],
    )
    .await;

    Ok(Json(AnnouncementResponse::from(announcement)))
}
