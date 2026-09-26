use std::sync::Arc;

use axum::extract::Multipart;

use super::{
    common::sanitize_filename,
    stream::{stream_gzip_upload_to_juicehost, stream_upload_to_juicehost},
    ticket::clamp_ttl_seconds,
};
use crate::{
    db::{self, FileRecord},
    error::AppError,
    state::AppState,
    upload_mode::UploadMode,
};

pub struct UploadParams {
    pub(crate) file_id: String,
    pub(crate) filename: String,
    pub(crate) mime_type: String,
    pub(crate) total_bytes: i64,
    pub(crate) selected_host: Option<String>,
    pub(crate) selected_upload_mode: UploadMode,
    pub(crate) ttl_seconds: i64,
    pub(crate) reservation: Option<FileRecord>,
    pub(crate) file_capability: String,
}

pub async fn parse_multipart(
    multipart: &mut Multipart,
    state: &Arc<AppState>,
    is_gzip: bool,
    reservation_token: Option<&str>,
    default_capability: String,
) -> Result<UploadParams, AppError> {
    let (default_ttl, allowed_ttl, max_size, danger) = {
        let jh = state.juicehost_config()?;
        (
            jh.default_ttl_hours,
            jh.allowed_ttl_hours.clone(),
            jh.max_file_size_bytes,
            jh.danger_level,
        )
    };
    let mut ttl_seconds = clamp_ttl_seconds(&allowed_ttl, default_ttl, None);
    let mut file_id = String::new();
    let mut sanitized_filename = String::new();
    let mut mime_type = String::new();
    let mut total_bytes: i64 = 0;
    let mut selected_host: Option<String> = None;
    let mut selected_upload_mode = UploadMode::Standard;
    let mut reservation = None;
    let mut file_capability = default_capability;

    while let Some(field) = multipart.next_field().await.map_err(|e| {
        tracing::warn!("multipart field parse error: {:?}", e);
        AppError::InvalidMultipart
    })? {
        let name = field.name().unwrap_or("").to_string();
        let field_start = std::time::Instant::now();

        if name == "ttl_hours" {
            let text = field.text().await.map_err(|_| AppError::InvalidMultipart)?;
            if let Ok(val) = text.parse::<f64>() {
                ttl_seconds = clamp_ttl_seconds(&allowed_ttl, default_ttl, Some(val));
            }
            tracing::debug!("  field ttl_hours took {:?}", field_start.elapsed());
        } else if name == "upload_mode" {
            let text = field.text().await.map_err(|_| AppError::InvalidMultipart)?;
            selected_upload_mode = UploadMode::from(text.trim());
            tracing::debug!("  field upload_mode took {:?}", field_start.elapsed());
        } else if name == "host" {
            let text = field.text().await.map_err(|_| AppError::InvalidMultipart)?;
            let text = text.trim().trim_end_matches('/').to_string();
            if !text.is_empty() {
                let checked = crate::storage_client::check_storage_host(
                    &text,
                    state.config.allow_private_fetch,
                )
                .await
                .map_err(|_| AppError::InvalidMultipart)?;
                selected_host = Some(checked);
            }
            tracing::debug!("  field host took {:?}", field_start.elapsed());
        } else if name == "reserve_id" {
            let text = field.text().await.map_err(|_| AppError::InvalidMultipart)?;
            let text = text.trim().to_string();
            if !text.is_empty() {
                if !crate::utils::is_valid_id(&text) {
                    return Err(AppError::BadRequest("invalid reserve_id".into()));
                }
                let token = reservation_token.ok_or_else(|| {
                    AppError::Forbidden("reservation delete token required".into())
                })?;
                let lookup_id = text.clone();
                let existing = state
                    .db_call("get_reserved_file", move |db| db::get_file(db, &lookup_id))
                    .await?
                    .ok_or_else(|| AppError::BadRequest("invalid reserve_id".into()))?;
                if existing.status != "uploading" {
                    return Err(AppError::BadRequest("reservation is not uploading".into()));
                }
                if !crate::utils::constant_time_eq(&existing.delete_token, token) {
                    return Err(AppError::Forbidden(
                        "invalid reservation delete token".into(),
                    ));
                }
                file_id = text;
                file_capability.clone_from(&existing.delete_token);
                reservation = Some(existing);
            }
            tracing::debug!("  field reserve_id took {:?}", field_start.elapsed());
        } else if name == "file" {
            let filename = field.file_name().unwrap_or("upload").to_string();
            sanitized_filename = sanitize_filename(&filename);

            if let Some(ref existing) = reservation {
                selected_host = existing.storage_host.clone();
            }

            mime_type = field
                .content_type()
                .map(ToString::to_string)
                .unwrap_or_else(|| {
                    mime_guess::from_path(&sanitized_filename)
                        .first_or_octet_stream()
                        .to_string()
                });

            if file_id.is_empty() {
                file_id = nanoid::nanoid!(8);
            }

            if is_gzip {
                let (bytes, read_time, push_wait) = stream_gzip_upload_to_juicehost(
                    field,
                    state,
                    &file_id,
                    &sanitized_filename,
                    &mime_type,
                    selected_host.as_deref(),
                    selected_upload_mode,
                    max_size as i64,
                    &file_capability,
                    danger,
                )
                .await?;
                total_bytes = bytes;
                let gzip_bps = if read_time.as_secs_f64() > 0.0 {
                    total_bytes as f64 / read_time.as_secs_f64()
                } else {
                    0.0
                };
                tracing::debug!(
                    "  field file (gzip): read={:?} bytes={} {:.2} MB/s push_wait={:?}",
                    read_time,
                    total_bytes,
                    gzip_bps / crate::constants::BYTES_PER_MIB,
                    push_wait,
                );
            } else {
                let (bytes, _chunks, _backpressure, _push_wait) = stream_upload_to_juicehost(
                    field,
                    state,
                    &file_id,
                    &sanitized_filename,
                    &mime_type,
                    selected_host.as_deref(),
                    selected_upload_mode,
                    max_size as i64,
                    &file_capability,
                )
                .await?;
                total_bytes = bytes;
            }
            break;
        }
    }

    Ok(UploadParams {
        file_id,
        filename: sanitized_filename,
        mime_type,
        total_bytes,
        selected_host,
        selected_upload_mode,
        ttl_seconds,
        reservation,
        file_capability,
    })
}
