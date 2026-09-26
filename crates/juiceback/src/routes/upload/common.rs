use std::{net::SocketAddr, sync::Arc};

use axum::http::HeaderMap;
use serde::Serialize;
use utoipa::ToSchema;

use crate::{error::AppError, state::AppState};

#[derive(Serialize, ToSchema)]
pub struct UploadResponse {
    pub id: String,
    pub url: String,
    pub filename: String,
    pub size_bytes: i64,
    pub mime_type: String,
    pub expires_at: i64,
    pub delete_token: String,
    pub status: String,
}

pub(crate) fn compute_ticket_ttl_secs(
    file_size_bytes: u64,
    assumed_bps: f64,
    safety_mult: f64,
    base_overhead_secs: i64,
    min_ttl: i64,
    max_ttl: i64,
) -> i64 {
    debug_assert!(assumed_bps > 0.0, "assumed_bps must be positive");
    if file_size_bytes == 0 {
        return min_ttl;
    }
    let estimated = (file_size_bytes as f64 / assumed_bps).ceil() as i64;
    let ttl = estimated * safety_mult as i64 + base_overhead_secs;
    ttl.clamp(min_ttl, max_ttl)
}

pub(crate) fn dte_ticket_ttl_secs(state: &Arc<AppState>, file_size_bytes: u64) -> i64 {
    compute_ticket_ttl_secs(
        file_size_bytes,
        state.config.dte_assumed_bps,
        state.config.dte_safety_mult,
        state.config.dte_base_overhead_secs,
        state.config.dte_min_ttl_secs,
        state.config.dte_max_ttl_secs,
    )
}

pub(crate) fn enforce_mint_limit(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    addr: SocketAddr,
) -> Result<(), AppError> {
    if !state.config.dte_enabled {
        return Ok(());
    }
    let ip = crate::utils::client_ip(headers, addr.ip(), state).to_string();
    if state.mint_limiter.allow(&ip) {
        Ok(())
    } else {
        Err(AppError::TooManyRequests(
            "Too many upload tickets minted. Please wait and try again.".into(),
        ))
    }
}

pub(crate) fn region_host(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    addr: SocketAddr,
) -> Option<String> {
    if !juiceutils::proxy::is_trusted(addr.ip(), &state.config.trusted_proxy_cidrs) {
        return None;
    }
    let region = headers
        .get("x-juicebox-region")?
        .to_str()
        .ok()?
        .trim()
        .to_ascii_lowercase();
    if region.is_empty() {
        return None;
    }
    state.config.region_public_juicehosts.get(&region).cloned()
}

pub(crate) fn sanitize_filename(name: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        return "upload".to_string();
    }
    let mut result = String::with_capacity(name.len().min(crate::constants::MAX_FILENAME_LEN));
    for c in name.chars() {
        if c.is_control() || c == '/' || c == '\\' || c == '\0' {
            continue;
        }
        result.push(c);
        if result.len() > crate::constants::MAX_FILENAME_LEN {
            result.pop();
            break;
        }
    }
    let result = result.replace("..", "");
    if result.is_empty() {
        return "upload".to_string();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_empty_string() {
        assert_eq!(sanitize_filename(""), "upload");
    }

    #[test]
    fn sanitize_whitespace_only() {
        assert_eq!(sanitize_filename("   "), "upload");
    }

    #[test]
    fn sanitize_control_chars() {
        let result = sanitize_filename("hello\x00world\x01test");
        assert_eq!(result, "helloworldtest");
    }

    #[test]
    fn sanitize_slashes() {
        let result = sanitize_filename("path/to/file.txt");
        assert_eq!(result, "pathtofile.txt");
    }

    #[test]
    fn sanitize_backslashes() {
        let result = sanitize_filename("path\\to\\file.txt");
        assert_eq!(result, "pathtofile.txt");
    }

    #[test]
    fn sanitize_dotdot_removed() {
        let result = sanitize_filename("file..name.txt");
        assert_eq!(result, "filename.txt");
    }

    #[test]
    fn sanitize_normal_filename() {
        assert_eq!(sanitize_filename("photo.jpg"), "photo.jpg");
    }

    #[test]
    fn sanitize_long_filename_truncated() {
        let long_name = "a".repeat(300);
        let result = sanitize_filename(&long_name);
        assert!(result.len() <= crate::constants::MAX_FILENAME_LEN);
    }

    #[test]
    fn sanitize_path_separators_only() {
        assert_eq!(sanitize_filename("/\\\\\0"), "upload");
    }

    fn compute(size: u64) -> i64 {
        compute_ticket_ttl_secs(
            size,
            crate::constants::DTE_ASSUMED_BPS,
            crate::constants::DTE_SAFETY_MULT,
            crate::constants::DTE_BASE_OVERHEAD_SECS,
            crate::constants::DTE_MIN_TTL_SECS,
            crate::constants::DTE_MAX_TTL_SECS,
        )
    }

    #[test]
    fn ttl_min_clamp_for_tiny_file() {
        let ttl = compute(1024 * 1024);
        assert!(ttl >= crate::constants::DTE_MIN_TTL_SECS, "ttl={ttl}");
    }

    #[test]
    fn ttl_small_file_at_least_ten_minutes() {
        let ttl = compute(1_000_000);
        assert!(ttl >= 600, "ttl={ttl}");
    }

    #[test]
    fn ttl_mid_size_in_range() {
        let ttl = compute(4 * 1024 * 1024 * 1024);
        assert!(ttl > 600 && ttl < 21_600, "ttl={ttl}");
    }

    #[test]
    fn ttl_large_file_clamped_to_max() {
        let ttl = compute(u64::MAX);
        assert_eq!(ttl, crate::constants::DTE_MAX_TTL_SECS);
    }

    #[test]
    fn ttl_max_clamp_for_very_large_file() {
        let ttl = compute(100 * 1024 * 1024 * 1024);
        assert_eq!(ttl, crate::constants::DTE_MAX_TTL_SECS);
    }

    #[test]
    fn ttl_is_monotonic_in_size() {
        let mut prev = 0_i64;
        for size in [
            1,
            1024,
            1_000_000,
            10_000_000,
            100_000_000,
            1_000_000_000,
            u64::MAX,
        ] {
            let ttl = compute(size);
            assert!(ttl >= prev, "ttl regressed: {ttl} < {prev} at size {size}");
            prev = ttl;
        }
    }

    #[test]
    fn ttl_zero_size_returns_min() {
        assert_eq!(compute(0), crate::constants::DTE_MIN_TTL_SECS);
    }
}
