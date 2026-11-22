use std::{net::SocketAddr, sync::Arc};

use axum::http::HeaderMap;

use super::common::dte_ticket_ttl_secs;
use crate::{
    db::{self, FileRecord},
    error::AppError,
    state::AppState,
};

/// Snaps to the nearest allowed value or the default.
#[must_use]
pub fn clamp_ttl_hours(allowed_ttl: &[f64], default_ttl_hours: f64, ttl_hours: Option<f64>) -> f64 {
    ttl_hours.map_or(default_ttl_hours, |hours| {
        crate::constants::clamp_to_nearest(hours, allowed_ttl)
    })
}

/// Snaps to the nearest allowed value or the default; returns seconds.
pub fn clamp_ttl_seconds(
    allowed_ttl: &[f64],
    default_ttl_hours: f64,
    ttl_hours: Option<f64>,
) -> i64 {
    (clamp_ttl_hours(allowed_ttl, default_ttl_hours, ttl_hours)
        * crate::constants::SECONDS_PER_HOUR_F64)
        .round() as i64
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

/// DTE tickets are sized so a slow but progressing upload still fits.
pub fn ticket_ttl_secs(state: &Arc<AppState>, file_size: u64, legacy_secs: i64) -> i64 {
    if state.config.dte_enabled {
        dte_ticket_ttl_secs(state, file_size)
    } else {
        legacy_secs
    }
}

/// Scoped to one file, user, and capability; juicehost verifies it with the
/// shared secret.
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
    let claims = juiceproto::ticket::TicketClaims::new(
        sub, user_id, file_id, filename, mime_type, file_size, capability, now, ttl_secs,
    );
    juiceproto::ticket::mint(&claims, &state.config.ticket_jwt_secret)
        .map_err(|e| AppError::Internal(format!("JWT encoding failed: {e}")))
}

/// Both must land or the id is unusable.
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

/// Shared by the direct and parallel complete handlers: the ticket must name
/// this file/session and the pending row must be owned by the caller.
pub async fn verify_ticket_and_ownership(
    state: &Arc<AppState>,
    ticket: &str,
    file_id: &str,
    user_id: &str,
    get_op: &'static str,
    own_op: &'static str,
) -> Result<FileRecord, AppError> {
    let claims = juiceproto::ticket::verify(ticket, &state.config.ticket_jwt_secret)
        .map_err(|_| AppError::Unauthorized("Invalid upload ticket".into()))?;
    if claims.file_id != file_id || claims.user_id != user_id {
        return Err(AppError::Unauthorized(
            "Upload ticket does not match session".into(),
        ));
    }
    let lookup_id = file_id.to_string();
    let file = state
        .db_call(get_op, move |db| db::get_file(db, &lookup_id))
        .await?
        .ok_or(AppError::NotFound)?;
    let owner_id = user_id.to_string();
    let owned_id = file_id.to_string();
    let owned = state
        .db_call(own_op, move |db| {
            db::client_owns_file(db, &owner_id, &owned_id)
        })
        .await?;
    if !owned {
        return Err(AppError::Unauthorized(
            "Upload is not owned by this session".into(),
        ));
    }
    Ok(file)
}

/// The middle URL bypasses the viewer page; the last is a direct download.
pub fn public_urls_for(state: &Arc<AppState>, record: &FileRecord) -> (String, String, String) {
    crate::utils::share_links(
        &state.config,
        record.storage_host.as_deref(),
        &record.id,
        &record.filename,
    )
}

/// `None` when encryption is unconfigured.
pub fn encrypted_client_ip(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    addr: SocketAddr,
) -> Option<String> {
    let raw_ip = crate::utils::client_ip(&headers, addr.ip(), state).to_string();
    crate::utils::encrypt_ip(&raw_ip, &state.config.ip_encryption_key)
}

/// An offline or lock-contended device is skipped; the ticket is still
/// returned.
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
    // Clone the sender and release both the DashMap ref and device lock before
    // awaiting, so a slow/blocked channel cannot hold either lock.
    let sender = state.connected_devices.get(user_id).and_then(|guard| {
        guard
            .value()
            .iter()
            .filter_map(|dev| dev.try_lock().ok())
            .find(|locked| locked.device_id == device_id)
            .map(|locked| locked.sender.clone())
    });
    if let Some(sender) = sender
        && let Err(err) = sender.send(upload_request).await
    {
        tracing::warn!("device {device_id} upload notification dropped: {err}");
    }
}

pub fn device_connected(state: &Arc<AppState>, user_id: &str, device_id: &str) -> bool {
    state.connected_devices.get(user_id).is_some_and(|guard| {
        guard
            .value()
            .iter()
            .any(|d| d.try_lock().is_ok_and(|l| l.device_id == device_id))
    })
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
        assert_eq!(clamp_ttl_seconds(&allowed(), 24.0, Some(2.0)), 3600);
        assert_eq!(clamp_ttl_seconds(&allowed(), 24.0, Some(5.0)), 21600);
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
