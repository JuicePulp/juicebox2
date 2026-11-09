use serde::{Deserialize, Serialize};

/// `/api/config` as served by juicehost. Every field is optional because older
/// hosts omit newer settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HostConfigResponse {
    #[serde(default)]
    pub max_file_size_bytes: Option<u64>,
    #[serde(default)]
    pub max_ttl_hours: Option<f64>,
    #[serde(default)]
    pub default_ttl_hours: Option<f64>,
    #[serde(default)]
    pub allowed_ttl_hours: Option<Vec<f64>>,
    #[serde(default)]
    pub danger_level: Option<String>,
    #[serde(default)]
    pub quick_link: Option<bool>,
    #[serde(default)]
    pub custom_id: Option<bool>,
    #[serde(default)]
    pub quic: Option<bool>,
    #[serde(default)]
    pub cobalt: Option<bool>,
    #[serde(default)]
    pub upload_mode: Option<String>,
    #[serde(default)]
    pub fetch_rate_limit_per_minute: Option<u64>,
    #[serde(default)]
    pub public_base_url: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub backend_url: Option<String>,
    #[serde(default)]
    pub frontend_url: Option<String>,
    #[serde(default)]
    pub password_links: Option<bool>,
    #[serde(default)]
    pub at_rest_via_juiceback: Option<bool>,
    #[serde(default)]
    pub encrypted: Option<bool>,
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
