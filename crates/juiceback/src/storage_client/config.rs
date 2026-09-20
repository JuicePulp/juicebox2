use super::target::require_juicehost_url;

/// This is the single source of truth for upload limits, TTL, danger level,
/// etc idc.
#[derive(Debug, Clone)]
pub struct JuicehostConfig {
    pub max_file_size_bytes: u64,
    pub default_ttl_hours: f64,
    pub allowed_ttl_hours: Vec<f64>,
    pub danger_level: crate::file_validation::ProtectionLevel,
    pub quick_link: bool,
    pub custom_id_enabled: bool,
    pub ultrafast: bool,
}

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
        .unwrap_or(524_288_000);

    let default_ttl_hours = cfg
        .get("default_ttl_hours")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(24.0);

    let allowed_ttl_hours = cfg
        .get("allowed_ttl_hours")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(serde_json::Value::as_f64)
                .collect::<Vec<f64>>()
        })
        .unwrap_or_else(|| vec![0.5, 1.0, 6.0, 12.0, 24.0, 72.0, 168.0]);

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

    let ultrafast = cfg
        .get("ultrafast")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    tracing::info!(
        "juicehost config: max_file_size={}MB default_ttl={}h danger={} quick_link={} custom_id={} ultrafast={}",
        max_file_size_bytes / (1024 * 1024),
        default_ttl_hours,
        danger_level_str,
        quick_link,
        custom_id_enabled,
        ultrafast,
    );

    Ok(JuicehostConfig {
        max_file_size_bytes,
        default_ttl_hours,
        allowed_ttl_hours,
        danger_level,
        quick_link,
        custom_id_enabled,
        ultrafast,
    })
}
