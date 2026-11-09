use std::sync::Arc;

use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    auth::InternalApiKey,
    db::{self, FileRecord},
    error::AppError,
    routes::upload::sanitize_filename,
    state::AppState,
};

#[derive(Deserialize, ToSchema)]
pub struct RegisterRequest {
    pub id: String,

    pub filename: String,

    pub mime_type: String,

    pub size_bytes: i64,

    pub ttl_hours: Option<f64>,

    pub uploader_ip: Option<String>,

    pub storage_host: Option<String>,

    pub delete_token: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct RegisterResponse {
    pub delete_token: String,

    pub expires_at: i64,
}

#[utoipa::path(
    post,
    path = "/api/register",
    request_body(content = RegisterRequest, description = "File metadata for juicehost to register"),
    responses(
        (status = 200, description = "File registered, returns delete token and expiry", body = RegisterResponse),
        (status = 401, description = "Invalid or missing x-juicehost-api-key header"),
        (status = 401, description = "Missing reservation delete token when completing an existing reserved ID"),
        (status = 403, description = "Invalid reservation delete token when completing an existing reserved ID"),
        (status = 404, description = "Unknown reservation ID"),
        (status = 409, description = "File ID already exists or reservation is not uploading"),
        (status = 400, description = "Invalid file ID format"),
    ),
    tag = "General",
)]
pub async fn register_handler(
    State(state): State<Arc<AppState>>,
    _key: InternalApiKey,
    Json(payload): Json<RegisterRequest>,
) -> Result<Json<RegisterResponse>, AppError> {
    if !crate::utils::is_valid_id_component(&payload.id) {
        return Err(AppError::BadRequest("invalid file ID format".into()));
    }

    let filename = sanitize_filename(&payload.filename);

    let (default_ttl, allowed_ttl) = {
        let jh = state.juicehost_config_or_refresh().await?;
        (jh.default_ttl_hours, jh.allowed_ttl_hours.clone())
    };

    let ttl_hours_raw = payload.ttl_hours.unwrap_or(default_ttl);
    let ttl_hours = crate::constants::clamp_to_nearest(ttl_hours_raw, &allowed_ttl);

    let now = chrono::Utc::now().timestamp();
    let new_delete_token = uuid::Uuid::new_v4().to_string();
    let expires_at = now + (ttl_hours * crate::constants::SECONDS_PER_HOUR_F64).round() as i64;

    let encrypted_ip = payload
        .uploader_ip
        .and_then(|ip| crate::utils::encrypt_ip(&ip, &state.config.ip_encryption_key));

    let record = FileRecord::new(
        payload.id,
        filename,
        payload.mime_type,
        payload.size_bytes,
        new_delete_token.clone(),
        now,
        expires_at,
        encrypted_ip,
        payload.storage_host,
    );

    let rec_id = record.id.clone();
    let rec_size = record.size_bytes;
    let rec_mime = record.mime_type.clone();
    let rec_expires = record.expires_at;

    let existing_id = rec_id.clone();
    let existing = state
        .db_call("get_file_for_register", move |db| {
            db::get_file(db, &existing_id)
        })
        .await?;
    let (delete_token, response_expires_at) = if let Some(existing) = existing {
        if existing.status != FileRecord::STATUS_UPLOADING {
            return Err(AppError::Conflict("file ID already exists".into()));
        }
        let reservation_token = payload
            .delete_token
            .clone()
            .ok_or_else(|| AppError::Unauthorized("reservation delete token required".into()))?;
        let completed = crate::reservation::finish(
            &state,
            crate::reservation::FinishRequest {
                id: existing.id,
                delete_token: reservation_token,
                filename: record.filename.clone(),
                mime_type: record.mime_type.clone(),
                size_bytes: record.size_bytes,
                uploader_ip: record.uploader_ip.clone(),
            },
            crate::reservation::FinishPolicy::Strict,
        )
        .await?;
        (completed.delete_token, completed.expires_at)
    } else {
        db::insert_new_file(&state, record).await?;
        (new_delete_token, expires_at)
    };

    tracing::info!(
        "register: id={rec_id} size={rec_size} mime={rec_mime} expires_at={rec_expires}"
    );

    Ok(Json(RegisterResponse {
        delete_token,
        expires_at: response_expires_at,
    }))
}
