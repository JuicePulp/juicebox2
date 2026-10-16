use std::sync::Arc;

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use serde::Deserialize;
use utoipa::ToSchema;

use crate::{db, error::AppError, state::AppState};

fn check_internal_key(state: &AppState, headers: &HeaderMap) -> Result<(), AppError> {
    if state.config.juicehost_api_key.is_empty() {
        return Err(AppError::Unauthorized("API key not configured".into()));
    }
    let provided = headers
        .get("x-juicehost-api-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !crate::utils::constant_time_eq(provided, &state.config.juicehost_api_key) {
        return Err(AppError::Unauthorized("invalid API key".into()));
    }
    Ok(())
}

#[derive(Deserialize, ToSchema)]
pub struct HitRequest {
    
    pub file_id: String,
    
    
    pub kind: String,
    
    
    pub ip: String,
    #[serde(default)]
    pub surface: Option<String>,
}

#[utoipa::path(
    post,
    path = "/internal/stats/hit",
    request_body = HitRequest,
    responses(
        (status = 204, description = "Hit recorded"),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Invalid API key"),
    ),
    security(("api_key" = [])),
    tag = "Internal",
)]
pub async fn hit_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<HitRequest>,
) -> Result<StatusCode, AppError> {
    check_internal_key(&state, &headers)?;
    let kind =
        db::HitKind::from_str(&body.kind).ok_or(AppError::BadRequest("unknown hit kind".into()))?;
    if body.file_id.is_empty() || body.file_id.len() > 64 || body.ip.is_empty() {
        return Err(AppError::BadRequest("invalid hit payload".into()));
    }
    let ip_hash = crate::utils::hash_ip_for_ban(&body.ip, &state.config.ip_pepper);
    let now = chrono::Utc::now().timestamp();
    let hit_file_id = body.file_id.clone();
    state
        .db_call("record_file_hit", move |db| {
            db::record_file_hit(db, &hit_file_id, &ip_hash, kind, now)
        })
        .await?;
    if kind == db::HitKind::View {
        let surface = body.surface.as_deref().unwrap_or("view").to_string();
        if surface == "preview" || surface == "view" {
            let embed_state = Arc::clone(&state);
            let embed_id = body.file_id.clone();
            tokio::spawn(async move {
                crate::jobs::promo::maybe_embed_promote(&embed_state, &embed_id).await;
            });
        }
    }
    {
        let geo_state = Arc::clone(&state);
        let geo_ip = body.ip.clone();
        tokio::spawn(async move {
            let Ok(addr) = geo_ip.parse::<std::net::IpAddr>() else {
                return;
            };
            if !crate::storage_client::is_public_ip(addr) {
                return;
            }
            let Some(enc) =
                crate::utils::encrypt_ip(&geo_ip, &geo_state.config.ip_encryption_key)
            else {
                return;
            };
            let hash = juiceutils::ban::hash_ip_for_ban(&geo_ip, &geo_state.config.ip_pepper);
            let now = chrono::Utc::now().timestamp();
            let _ = geo_state
                .db_call("geo_enqueue", move |db| {
                    db::geo::enqueue_geo_ip(db, &enc, &hash, now)
                })
                .await;
        });
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, ToSchema)]
pub struct VisitRequest {
    
    pub ip: String,
}

#[utoipa::path(
    post,
    path = "/internal/stats/visit",
    request_body = VisitRequest,
    responses(
        (status = 204, description = "Visit recorded"),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Invalid API key"),
    ),
    security(("api_key" = [])),
    tag = "Internal",
)]
pub async fn visit_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<VisitRequest>,
) -> Result<StatusCode, AppError> {
    check_internal_key(&state, &headers)?;
    if body.ip.is_empty() {
        return Err(AppError::BadRequest("invalid visit payload".into()));
    }
    let ip_hash = crate::utils::hash_ip_for_ban(&body.ip, &state.config.ip_pepper);
    let now = chrono::Utc::now().timestamp();
    state
        .db_call("record_site_visit", move |db| {
            db::record_site_visit(db, &ip_hash, now)
        })
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
