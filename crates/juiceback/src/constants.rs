pub const SECONDS_PER_HOUR: i64 = 3600;
pub const SECONDS_PER_HOUR_F64: f64 = 3600.0;
pub const SECONDS_PER_DAY: i64 = 86_400;
pub const UPLOAD_SIZE_OVERHEAD_BYTES: usize = 1024 * 1024;
pub const STREAM_CHANNEL_CAPACITY: usize = 256;
pub const MAX_FILENAME_LEN: usize = 255;
pub const MAX_CUSTOM_ID_LEN: usize = 32;
pub const MIN_CUSTOM_ID_LEN: usize = 3;
pub const GZIP_READ_BUFFER_SIZE: usize = 128 * 1024;
pub const FILE_SNIFF_PREFIX_LEN: usize = 1024;
pub const MAX_TUS_SESSIONS: usize = 1000;
pub const MAX_TUS_PARALLEL_PARTS: usize = 64;
pub const TUS_PATCH_MAX_BYTES: usize = 96 * 1024 * 1024;
pub const QUIC_SPILL_THRESHOLD_BYTES: usize = 8 * 1024 * 1024;
pub const BYTES_PER_MIB: f64 = 1024.0 * 1024.0;
pub const STALE_UPLOAD_TIMEOUT_SECS: i64 = 3600;
pub const CLEANUP_BATCH_SIZE: usize = 500;
pub const SESSION_TOUCH_INTERVAL_SECS: i64 = 300;
pub const DEVICE_SEEN_TOUCH_INTERVAL_SECS: u64 = 60;
pub const ADMIN_RATE_LIMIT_WINDOW_SECS: u64 = 60;
pub const ADMIN_RATE_LIMIT_BURST: u32 = 5;
pub const ADMIN_JWT_COOKIE_MAX_AGE_SECS: i64 = 86400;
pub const HTTP_CONNECT_TIMEOUT_SECS: u64 = 10;
pub const HTTP_IDLE_TIMEOUT_SECS: u64 = 30;
pub const MAX_TUS_PER_IP: usize = 128;
pub const MAX_REPORT_FILE_URL_LEN: usize = 2048;
pub const MAX_REPORT_REASON_LEN: usize = 256;
pub const MAX_REPORT_DETAILS_LEN: usize = 4096;
pub const MAX_FEEDBACK_MESSAGE_LEN: usize = 4096;
pub const MAX_FEEDBACK_EMAIL_LEN: usize = 320;
pub const MAX_PAIRING_CODES_PER_USER: i64 = 5;
pub const MAX_PAIRING_CODES_PER_IP: i64 = 10;
pub const MAX_SSE_CONNECTIONS: usize = 256;
pub const MAX_SSE_CONNECTIONS_PER_IP: u32 = 8;
pub const DEGRADED_RETRY_INTERVAL_SECS: u64 = 30;
pub const STARTUP_CONFIG_RETRIES: u32 = 3;
pub const STARTUP_BACKOFF_BASE_SECS: u64 = 2;
pub const COBALT_FETCH_RATE_LIMIT_PER_MINUTE: u32 = 3;
pub const FETCH_JOB_TIMEOUT_SECS: u64 = 30 * 60;
pub const FETCH_EMPTY_RETRY_DELAY_SECS: u64 = 10;
pub const FETCH_RESCUE_MAX_PASSES: u32 = 30;
pub const FETCH_JOB_RETENTION_SECS: i64 = 86_400;
pub const STALE_FETCH_JOB_TIMEOUT_SECS: i64 = 3600;
pub const FETCH_SOURCE_URL_MAX_LEN: usize = 2048;
pub const DTE_ASSUMED_BPS: f64 = 2.0 * 1024.0 * 1024.0;
pub const DTE_SAFETY_MULT: f64 = 2.0;
pub const DTE_BASE_OVERHEAD_SECS: i64 = 120;
pub const DTE_MIN_TTL_SECS: i64 = 600;
pub const DTE_MAX_TTL_SECS: i64 = 21600;
pub const DTE_MINT_LIMIT: u32 = 60;
pub const DTE_MINT_WINDOW_SECS: u64 = 600;
pub const DTE_MINT_BURST: u32 = 15;

#[must_use]
pub fn clamp_to_nearest(value: f64, allowed: &[f64]) -> f64 {
    assert!(!allowed.is_empty(), "allowed_ttl_hours must not be empty");
    *allowed
        .iter()
        .min_by_key(|&&a| ((a - value).abs() * 1000.0) as u64)
        .expect("allowed list is non-empty, so a minimum exists")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_picks_nearest_allowed_ttl() {
        let allowed = vec![1.0, 24.0, 168.0];
        assert_eq!(clamp_to_nearest(24.0, &allowed), 24.0);
        assert_eq!(clamp_to_nearest(20.0, &allowed), 24.0);
        assert_eq!(clamp_to_nearest(100.0, &allowed), 168.0);
        assert_eq!(clamp_to_nearest(0.5, &allowed), 1.0);
    }

    #[test]
    #[should_panic(expected = "allowed_ttl_hours must not be empty")]
    fn clamp_rejects_empty_allowed() {
        let _ = clamp_to_nearest(24.0, &[]);
    }
}
