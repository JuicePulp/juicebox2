use std::collections::BTreeMap;

use super::target::require_juicehost_url;

#[derive(Debug, Clone)]
pub struct JuicehostConfig {
    /// Legacy/default-retention cap.
    pub max_file_size_bytes: u64,
    /// Hard cap used for body limits and storage.
    pub hard_max_file_size_bytes: u64,
    /// Per-retention caps keyed by TTL in hours; empty when disabled.
    pub max_file_size_by_ttl_hours: BTreeMap<String, u64>,
    pub default_ttl_hours: f64,
    pub allowed_ttl_hours: Vec<f64>,
    pub danger_level: crate::file_validation::ProtectionLevel,
    pub quick_link: bool,
    pub custom_id_enabled: bool,
}

impl JuicehostConfig {
    /// Cap for a TTL that has already been snapped to an allowed value.
    #[must_use]
    pub fn max_file_size_for_ttl(&self, ttl_hours: f64) -> u64 {
        self.max_file_size_by_ttl_hours
            .get(&juiceutils::size_scale::ttl_key(ttl_hours))
            .copied()
            .unwrap_or(self.max_file_size_bytes)
            .min(self.hard_max_file_size_bytes)
    }
}

/// Missing fields fall back to compiled defaults.
pub async fn fetch_juicehost_config(
    client: &reqwest::Client,
    juicehost_url: &str,
    headers: &reqwest::header::HeaderMap,
) -> Result<JuicehostConfig, String> {
    require_juicehost_url(juicehost_url).map_err(|e| e.to_string())?;

    let url = format!("{juicehost_url}/api/config");
    let resp = client
        .get(&url)
        .headers(headers.clone())
        .send()
        .await
        .map_err(|e| format!("juicehost config fetch error: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "juicehost config fetch failed: status={status} body={body}"
        ));
    }

    let cfg: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("juicehost config parse error: {e}"))?;

    let max_file_size_bytes = cfg
        .get("max_file_size_bytes")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(juiceutils::config::defaults::MAX_FILE_SIZE_BYTES);
    let hard_max_file_size_bytes = cfg
        .get("max_file_size_hard_bytes")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(max_file_size_bytes)
        .max(max_file_size_bytes);
    let max_file_size_by_ttl_hours = cfg
        .get("max_file_size_by_ttl_hours")
        .and_then(serde_json::Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(ttl, size)| {
                    size.as_u64().map(|size| (ttl.clone(), size))
                })
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();

    let default_ttl_hours = cfg
        .get("default_ttl_hours")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(juiceutils::config::defaults::DEFAULT_TTL_HOURS);

    let allowed_ttl_hours = cfg
        .get("allowed_ttl_hours")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(serde_json::Value::as_f64)
                .collect::<Vec<f64>>()
        })
        .unwrap_or_else(|| juiceutils::config::defaults::ALLOWED_TTL_HOURS.to_vec());

    let danger_level_str = cfg
        .get("danger_level")
        .and_then(|v| v.as_str())
        .unwrap_or("high");
    let danger_level = crate::file_validation::ProtectionLevel::parse(danger_level_str);

    let quick_link = cfg
        .get("quick_link")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);

    let custom_id_enabled = cfg
        .get("custom_id")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);

    tracing::debug!(
        "juicehost config: max_file_size={}MB hard={}MB scaled_ttls={} default_ttl={}h danger={} quick_link={} custom_id={}",
        max_file_size_bytes / (1024 * 1024),
        hard_max_file_size_bytes / (1024 * 1024),
        max_file_size_by_ttl_hours.len(),
        default_ttl_hours,
        danger_level_str,
        quick_link,
        custom_id_enabled,
    );

    Ok(JuicehostConfig {
        max_file_size_bytes,
        hard_max_file_size_bytes,
        max_file_size_by_ttl_hours,
        default_ttl_hours,
        allowed_ttl_hours,
        danger_level,
        quick_link,
        custom_id_enabled,
    })
}
