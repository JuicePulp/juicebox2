use std::{net::SocketAddr, sync::Arc};

use axum::http::HeaderMap;
use jsonwebtoken::{EncodingKey, Header, encode};

use super::common::{dte_ticket_ttl_secs, enforce_mint_limit, region_host};
use crate::{
    db::{self, FileRecord},
    error::AppError,
    state::AppState,
};

pub fn clamp_ttl_seconds(
    allowed_ttl: &[f64],
    default_ttl_hours: f64,
    ttl_hours: Option<f64>,
) -> i64 {
    ttl_hours
        .map(|hours| crate::constants::clamp_to_nearest(hours, allowed_ttl))
        .map(|hours| (hours * crate::constants::SECONDS_PER_HOUR_F64).round() as i64)
        .unwrap_or_else(|| {
            (default_ttl_hours * crate::constants::SECONDS_PER_HOUR_F64).round() as i64
        })
}

pub fn reject_blocked_filename(
    filename: &str,
    danger: crate::file_validation::ProtectionLevel,
) -> Result<(), AppError> {
    if danger == crate::file_validation::ProtectionLevel::None {
        return Ok(());
    }
    if let crate::file_validation::FileValidation::BlockedExtension { tier, .. } =
        crate::file_validation::validate_filename(filename, danger)
    {
        return Err(AppError::BlockedFileType(
            crate::file_validation::friendly_block_reason(tier),
        ));
    }
    Ok(())
}

pub fn ticket_ttl_secs(state: &Arc<AppState>, file_size: u64, legacy_secs: i64) -> i64 {
    if state.config.dte_enabled {
        dte_ticket_ttl_secs(state, file_size)
    } else {
        legacy_secs
    }
}

#[allow(clippy::too_many_arguments)]
pub fn mint_ticket(
    state: &Arc<AppState>,
    sub: &str,
    user_id: &str,
    file_id: &str,
    filename: &str,
    mime_type: &str,
    file_size: u64,
    capability: &str,
    now: i64,
    ttl_secs: i64,
) -> Result<String, AppError> {
    let claims = serde_json::json!({
        "sub": sub,
        "user_id": user_id,
        "iss": "juiceback-ticket",
        "file_id": file_id,
        "filename": filename,
        "mime_type": mime_type,
        "file_size": file_size,
        "file_capability": capability,
        "iat": now as usize,
        "exp": (now + ttl_secs) as usize,
    });
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(state.config.ticket_jwt_secret.as_bytes()),
    )
    .map_err(|e| AppError::Internal(format!("JWT encoding failed: {e}")))
}

pub async fn insert_owned_pending(
    state: &Arc<AppState>,
    user_id: &str,
    record: FileRecord,
    op: &'static str,
) -> Result<FileRecord, AppError> {
    let record = db::insert_pending_file(state, record).await?;
    let owned_record = record.clone();
    let owner = user_id.to_string();
    state
        .db_call(op, move |db| db::add_client_file(db, &owner, &owned_record))
        .await?;
    Ok(record)
}

pub fn public_url_for(state: &Arc<AppState>, record: &FileRecord) -> String {
    crate::utils::public_url(
        &state.config.public_base_url,
        record.storage_host.as_deref(),
        &record.id,
        &record.filename,
    )
}

pub fn encrypted_client_ip(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    addr: SocketAddr,
) -> Option<String> {
    let raw_ip = crate::utils::client_ip(&headers, addr.ip(), state).to_string();
    crate::utils::encrypt_ip(&raw_ip, &state.config.ip_encryption_key)
}

pub fn check_mint_limit(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    addr: SocketAddr,
) -> Result<(), AppError> {
    enforce_mint_limit(state, headers, addr)
}

pub fn pinned_region_host(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    addr: SocketAddr,
) -> Option<String> {
    region_host(state, headers, addr)
}

#[allow(clippy::too_many_arguments)]
pub async fn notify_device(
    state: &Arc<AppState>,
    user_id: &str,
    device_id: &str,
    file_id: &str,
    filename: &str,
    mime_type: &str,
    file_size: u64,
    ticket: &str,
    download_url: &str,
) {
    let upload_request = serde_json::json!({
        "type": "upload_request",
        "file_id": file_id,
        "filename": filename,
        "mime_type": mime_type,
        "file_size": file_size,
        "ticket": ticket,
        "juicehost_url": state.config.public_juicehost_url,
        "juiceback_url": state.config.public_base_url,
        "download_url": download_url,
    })
    .to_string();
    if let Some(guard) = state.connected_devices.get(user_id) {
        for dev in guard.value().iter() {
            let matches = dev
                .try_lock()
                .map(|l| l.device_id == device_id)
                .unwrap_or(false);
            if matches {
                if let Ok(locked) = dev.try_lock() {
                    let _ = locked.sender.send(upload_request).await;
                }
                break;
            }
        }
    }
}

pub fn device_connected(state: &Arc<AppState>, user_id: &str, device_id: &str) -> bool {
    state
        .connected_devices
        .get(user_id)
        .map(|guard| {
            guard.value().iter().any(|d| {
                d.try_lock()
                    .map(|l| l.device_id == device_id)
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed() -> Vec<f64> {
        vec![0.5, 1.0, 6.0, 12.0, 24.0, 72.0, 168.0]
    }

    #[test]
    fn ttl_defaults_to_configured_hours() {
        assert_eq!(clamp_ttl_seconds(&allowed(), 24.0, None), 86400);
    }

    #[test]
    fn ttl_clamps_request_to_nearest_allowed() {
        let ttl = clamp_ttl_seconds(&allowed(), 24.0, Some(2.0));
        assert!(ttl == 3600 || ttl == 21600, "ttl={ttl}");
    }

    #[test]
    fn ttl_honours_allowed_request() {
        assert_eq!(clamp_ttl_seconds(&allowed(), 24.0, Some(1.0)), 3600);
    }

    #[test]
    fn blocked_filename_rejected_at_high() {
        let err =
            reject_blocked_filename("evil.exe", crate::file_validation::ProtectionLevel::High)
                .unwrap_err();
        assert!(matches!(err, AppError::BlockedFileType(_)));
    }

    #[test]
    fn safe_filename_passes_and_none_skips() {
        assert!(
            reject_blocked_filename("photo.jpg", crate::file_validation::ProtectionLevel::High,)
                .is_ok()
        );
        assert!(
            reject_blocked_filename("evil.exe", crate::file_validation::ProtectionLevel::None,)
                .is_ok()
        );
    }
}
