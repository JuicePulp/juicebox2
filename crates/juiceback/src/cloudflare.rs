use std::sync::Arc;

use crate::config::Config;

pub async fn purge_urls(config: Arc<Config>, urls: Vec<String>) {
    purge_cache(config, serde_json::json!({ "files": urls }), "urls").await;
}

pub async fn purge_tags(config: Arc<Config>, tags: Vec<String>) {
    purge_cache(config, serde_json::json!({ "purge_by_tags": tags }), "tags").await;
}

async fn purge_cache(config: Arc<Config>, body: serde_json::Value, kind: &'static str) {
    let (Some(token), Some(zone_id)) = (&config.cf_api_token, &config.cf_zone_id) else {
        return;
    };
    if (body
        .get("files")
        .and_then(|v| v.as_array())
        .is_some_and(Vec::is_empty))
        || (body
            .get("purge_by_tags")
            .and_then(|v| v.as_array())
            .is_some_and(Vec::is_empty))
    {
        return;
    }

    let token = token.clone();
    let zone_id = zone_id.clone();

    tokio::spawn(async move {
        let url = format!("https://api.cloudflare.com/client/v4/zones/{zone_id}/purge_cache");

        let client = reqwest::Client::new();
        match client
            .delete(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
        {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    tracing::info!("cloudflare: cache purge {kind} OK (status={status})");
                } else {
                    let text = resp.text().await.unwrap_or_default();
                    tracing::warn!(
                        "cloudflare: cache purge {} failed (status={}): {}",
                        kind,
                        status,
                        text.chars().take(200).collect::<String>()
                    );
                }
            }
            Err(e) => {
                tracing::warn!("cloudflare: cache purge {kind} request failed: {e}");
            }
        }
    });
}

pub fn cache_variants(
    config: &Config,
    storage_host: Option<&str>,
    id: &str,
    filename: &str,
    tier: &str,
    r2_key: &str,
) -> Vec<String> {
    let mut bases = vec![crate::utils::public_url(
        &config.public_base_url,
        storage_host,
        id,
        filename,
    )];
    if crate::utils::is_official_host(config, storage_host.unwrap_or("")) {
        if let Some(router) = config
            .router_base_url
            .as_deref()
            .filter(|s| !s.trim().is_empty())
        {
            bases.push(crate::utils::public_url(router, None, id, filename));
        }
    }
    let mut urls = Vec::with_capacity(bases.len() * 3 + 1);
    for v_url in bases {
        urls.push(v_url.clone());
        if let Some(stem) = v_url.split_once("/v/") {
            urls.push(format!("{}/f/{}", stem.0, stem.1));
            urls.push(format!("{}/d/{}", stem.0, stem.1));
        }
    }
    if tier == "hot" && !r2_key.is_empty() && !config.r2_public_base_url.is_empty() {
        urls.push(format!(
            "{}/{}",
            config.r2_public_base_url.trim_end_matches('/'),
            r2_key.trim_start_matches('/')
        ));
    }
    urls
}

pub fn purge_file(config: &Arc<Config>, record: &crate::db::FileRecord) {
    let urls = cache_variants(
        config,
        record.storage_host.as_deref(),
        &record.id,
        &record.filename,
        &record.tier,
        &record.r2_key,
    );
    let config = Arc::clone(config);
    let id = record.id.clone();
    tokio::spawn(async move {
        purge_urls(Arc::clone(&config), urls).await;
        purge_tags(config, vec![format!("juicebox:{id}")]).await;
    });
}
