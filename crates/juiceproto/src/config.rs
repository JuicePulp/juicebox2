use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Public `/api/config` payload, shared by both services.
///
/// juicehost emits the host-owned fields; juiceback emits the same fields plus
/// its backend-owned ones. Every field is optional because older nodes omit
/// newer settings, and clients must tolerate both services' subsets.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HostConfigResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,

    /// Legacy/default-retention cap. Older clients still read this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_file_size_bytes: Option<u64>,
    /// Hard cap used for request-body limits and storage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_file_size_hard_bytes: Option<u64>,
    /// Per-retention caps keyed by the TTL in hours, e.g. `"72": 5368709120`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_file_size_by_ttl_hours: Option<BTreeMap<String, u64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_ttl_hours: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_ttl_hours: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_ttl_hours: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub danger_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quick_link: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_id: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frontend_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_links: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_rest_via_juiceback: Option<bool>,

    /// Fields below are owned by juiceback, not by individual hosts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direct_upload: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upload_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cobalt: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quic: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetch_rate_limit_per_minute: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub juicehost_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit_per_minute: Option<u32>,
    /// Native clients size chunks and parts from these instead of hardcoding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tus_patch_max_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tus_parallel_parts: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direct_parallel_max_parts: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direct_parallel_max_part_bytes: Option<u64>,
}

/// Joins a base URL with the internal file-status path.
#[must_use]
pub fn file_status_url(backend_url: &str, id: &str) -> String {
    format!("{backend_url}/internal/file/{id}/status")
}

/// Joins a base URL with the internal file-resolution path.
#[must_use]
pub fn resolve_file_url(backend_url: &str, id: &str) -> String {
    format!("{backend_url}/internal/resolve/{id}")
}

/// Joins a juicehost base URL with its config endpoint.
#[must_use]
pub fn config_url(juicehost_url: &str) -> String {
    format!("{juicehost_url}/api/config")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_payloads_parse_and_unset_fields_stay_omitted() {
        let raw = r#"{"max_file_size_bytes":123,"danger_level":"medium"}"#;
        let cfg: HostConfigResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(cfg.max_file_size_bytes, Some(123));
        assert_eq!(cfg.danger_level.as_deref(), Some("medium"));
        assert_eq!(cfg.quick_link, None);
        assert_eq!(serde_json::to_value(cfg).unwrap(), serde_json::from_str::<serde_json::Value>(raw).unwrap());
    }
}
