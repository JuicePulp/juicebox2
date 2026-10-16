
use std::{collections::HashMap, sync::Arc, time::Duration};

use crate::{db, state::AppState};

const BATCH_MAX: usize = 100;
const GEO_ENDPOINT: &str = "http://ip-api.com/batch";

pub async fn run_geo_loop(state: Arc<AppState>) {
    geo_sweep_once(&state).await;
    let mut ticker = tokio::time::interval(Duration::from_secs(60));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        geo_sweep_once(&state).await;
    }
}

struct Target {
    queue_id: Option<i64>,
    ip: String,
    ip_hash: String,
}

fn is_routable(ip: &str) -> bool {
    match ip.parse::<std::net::IpAddr>() {
        Ok(addr) => crate::storage_client::is_public_ip(addr),
        Err(_) => false,
    }
}

async fn geo_sweep_once(state: &Arc<AppState>) {
    let mut targets: Vec<Target> = Vec::new();

    let queued = state
        .db_call("geo_dequeue", move |db| {
            db::geo::dequeue_geo_batch(db, BATCH_MAX as i64)
        })
        .await
        .unwrap_or_default();
    for row in queued {
        let plain = juiceutils::ip_crypt::decrypt_ip(&row.enc_ip, &state.config.ip_encryption_key);
        match plain {
            Some(ip) if is_routable(&ip) => targets.push(Target {
                queue_id: Some(row.id),
                ip,
                ip_hash: row.ip_hash,
            }),
            _ => {
                let id = row.id;
                let _ = state
                    .db_call("geo_drop", move |db| db::geo::delete_geo_queue(db, &[id]))
                    .await;
            }
        }
        if targets.len() >= BATCH_MAX {
            break;
        }
    }

    if targets.len() < BATCH_MAX {
        let enc_ips = state
            .db_call("geo_uploader_sample", move |db| {
                db::geo::uploader_ips_sample(db, 200)
            })
            .await
            .unwrap_or_default();
        let mut hashes: Vec<String> = Vec::new();
        let mut decoded: Vec<(String, String)> = Vec::new();
        for enc in enc_ips {
            if let Some(ip) = juiceutils::ip_crypt::decrypt_ip(&enc, &state.config.ip_encryption_key)
            {
                if !is_routable(&ip) {
                    continue;
                }
                let hash = juiceutils::ban::hash_ip_for_ban(&ip, &state.config.ip_pepper);
                hashes.push(hash.clone());
                decoded.push((ip, hash));
            }
        }
        let known = state
            .db_call("geo_known", move |db| db::geo::known_geo_hashes(db, &hashes))
            .await
            .unwrap_or_default();
        for (ip, hash) in decoded {
            if targets.len() >= BATCH_MAX {
                break;
            }
            if known.contains(&hash) || targets.iter().any(|t| t.ip_hash == hash) {
                continue;
            }
            targets.push(Target {
                queue_id: None,
                ip,
                ip_hash: hash,
            });
        }
    }

    if targets.is_empty() {
        return;
    }
    tracing::info!("geo: resolving {} addresses", targets.len());

    let body: Vec<serde_json::Value> = targets
        .iter()
        .map(|t| {
            serde_json::json!({
                "query": t.ip,
                "fields": "status,country,regionName,city,lat,lon,query",
            })
        })
        .collect();
    let resp = state
        .http
        .post(GEO_ENDPOINT)
        .json(&body)
        .timeout(Duration::from_secs(30))
        .send()
        .await;
    let now = chrono::Utc::now().timestamp();
    let mut done_ids: Vec<i64> = Vec::new();
    let mut retry_ids: Vec<i64> = Vec::new();
    match resp {
        Ok(r) if r.status().is_success() => {
            let items: Vec<serde_json::Value> = r.json().await.unwrap_or_default();
            let mut by_ip: HashMap<&str, &serde_json::Value> = HashMap::new();
            for item in &items {
                if let Some(q) = item.get("query").and_then(|v| v.as_str()) {
                    by_ip.insert(q, item);
                }
            }
            for t in &targets {
                let ok = by_ip.get(t.ip.as_str()).is_some_and(|item| {
                    item.get("status").and_then(|v| v.as_str()) == Some("success")
                });
                if !ok {
                    if let Some(id) = t.queue_id {
                        retry_ids.push(id);
                    }
                    continue;
                }
                let item = by_ip[t.ip.as_str()];
                let get = |k: &str| item.get(k).and_then(|v| v.as_str()).unwrap_or_default();
                let lat = item.get("lat").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let lon = item.get("lon").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let hash = t.ip_hash.clone();
                let country = get("country").to_string();
                let region = get("regionName").to_string();
                let city = get("city").to_string();
                let stored = state
                    .db_call("geo_upsert", move |db| {
                        db::geo::upsert_ip_location(
                            db, &hash, &country, &region, &city, lat, lon, now,
                        )
                    })
                    .await;
                if stored.is_ok() {
                    if let Some(id) = t.queue_id {
                        done_ids.push(id);
                    }
                } else if let Some(id) = t.queue_id {
                    retry_ids.push(id);
                }
            }
        }
        _ => {
            tracing::warn!("geo: batch request failed, retrying next tick");
            for t in &targets {
                if let Some(id) = t.queue_id {
                    retry_ids.push(id);
                }
            }
        }
    }
    tracing::info!(
        "geo: sweep done: {} resolved, {} retrying",
        done_ids.len(),
        retry_ids.len()
    );
    if !done_ids.is_empty() {
        let _ = state
            .db_call("geo_delete_done", move |db| {
                db::geo::delete_geo_queue(db, &done_ids)
            })
            .await;
    }
    if !retry_ids.is_empty() {
        let _ = state
            .db_call("geo_bump", move |db| db::geo::bump_geo_attempts(db, &retry_ids))
            .await;
    }
}
