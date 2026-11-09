use std::{net::SocketAddr, sync::Arc};

use axum::{
    Json,
    extract::{Query, State},
    http::HeaderMap,
};
use serde_json::json;
use utoipa::OpenApi;

use crate::{auth::InternalApiKey, error::AppError, routes::api_doc::ApiDoc, state::AppState};

/// Liveness only: always `ok`, without touching the database or juicehost.
#[utoipa::path(
    get,
    path = "/api/health",
    responses(
        (status = 200, description = "Service health status (juicehost status is probed with a short timeout)", body = serde_json::Value),
    ),
    tag = "General",
)]
pub async fn health_handler() -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "juiceback": "ok",
    }))
}

/// Returns a degraded payload when no cached config can be obtained.
#[utoipa::path(
    get,
    path = "/api/config",
    responses(
        (status = 200, description = "Public server configuration", body = serde_json::Value),
    ),
    tag = "General",
)]
pub async fn config_handler(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let jh = match state.juicehost_config() {
        Ok(cfg) => {
            if state.juicehost_config_stale() {
                let refresh_state = Arc::clone(&state);
                tokio::spawn(async move {
                    let _ = refresh_state.refresh_juicehost_config().await;
                });
            }
            Some(cfg)
        }
        Err(_) => state.refresh_juicehost_config().await.ok(),
    };

    match jh.as_ref() {
        Some(cfg) => {
            let danger_level_str = cfg.danger_level.as_str();
            Json(json!({
                "status": "ok",
                "version": env!("CARGO_PKG_VERSION"),
                "max_file_size_bytes": cfg.max_file_size_bytes,
                "default_ttl_hours": cfg.default_ttl_hours,
                "allowed_ttl_hours": cfg.allowed_ttl_hours,
                "danger_level": danger_level_str,
                "quick_link": cfg.quick_link,
                "custom_id": cfg.custom_id_enabled,
                "direct_upload": state.config.direct_upload_enabled
                    && !state.config.public_juicehost_url.is_empty(),
                "upload_mode": if state.config.dte_enabled
                    && state.config.direct_upload_enabled
                    && !state.config.public_juicehost_url.is_empty()
                {
                    "direct-prefer"
                } else {
                    "standard"
                },
                "cobalt": state.config.cobalt_enabled,
                "encrypted": state.config.storage_file_key().is_ok(),
                "fetch_rate_limit_per_minute": crate::constants::COBALT_FETCH_RATE_LIMIT_PER_MINUTE,
                // Native clients size chunks and parts from these instead of
                // hardcoding server limits.
                "tus_patch_max_bytes": crate::constants::TUS_PATCH_MAX_BYTES,
                "max_tus_parallel_parts": crate::constants::MAX_TUS_PARALLEL_PARTS,
                "direct_parallel_max_parts": crate::routes::upload::parallel::DIRECT_MAX_PARTS,
                "direct_parallel_max_part_bytes": crate::routes::upload::parallel::DIRECT_MAX_PART_BYTES,
                "public_base_url": state.config.public_base_url,
                "rate_limit_per_minute": state.config.rate_limit_per_minute,
                "quic": cfg!(feature = "quic"),
                "juicehost_url": state.config.juicehost_url,
            }))
        }
        None => Json(json!({
            "status": "degraded",
            "error": "juicehost config unavailable",
        })),
    }
}

#[utoipa::path(
    get,
    path = "/api/ip",
    responses(
        (status = 200, description = "Client IP address and version", body = serde_json::Value),
    ),
    tag = "General",
)]
pub async fn ip_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<SocketAddr>,
) -> Json<serde_json::Value> {
    let ip = crate::utils::client_ip(&headers, peer.ip(), &state);
    Json(json!({
        "ip": ip.to_string(),
        "version": if ip.is_ipv6() { "ipv6" } else { "ipv4" },
    }))
}

/// Always 200 with `banned`, `valid`, or `error`; the ban check runs first.
#[utoipa::path(
    post,
    path = "/api/validate-host",
    request_body(content = serde_json::Value, description = "JSON with a \"host\" field containing the juicehost URL"),
    responses(
        (status = 200, description = "Host validation result", body = serde_json::Value),
    ),
    tag = "General",
)]
pub async fn validate_host_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, AppError> {
    let host = body
        .get("host")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::BadRequest("host field required".into()))?;

    let target = crate::storage_client::custom_juicehost_target(host)
        .await
        .map_err(|e| AppError::BadRequest(e.to_string()))?;
    let host = target.base_url.clone();

    let host_key = host.clone();
    let banned = state
        .db_call("get_hoster", move |db| crate::db::get_hoster(db, &host_key))
        .await?
        .is_some_and(|h| h.banned);

    if banned {
        let host_key = host.clone();
        let _ = state
            .db_call("upsert_hoster", move |db| {
                crate::db::upsert_hoster(db, &host_key, "banned", "")
            })
            .await;
        return Ok(Json(json!({
            "status": "banned",
            "host": host,
        })));
    }

    let url = format!("{}/api/config", target.base_url);
    let resp = match target.client.get(&url).headers(target.headers).send().await {
        Ok(resp) => resp,
        Err(e) => {
            let msg = format!("failed to reach host: {e}");
            let host_key = host.clone();
            let value = msg.clone();
            let _ = state
                .db_call("upsert_hoster", move |db| {
                    crate::db::upsert_hoster(db, &host_key, "error", &value)
                })
                .await;
            return Ok(Json(json!({
                "status": "error",
                "host": host,
                "error": msg,
            })));
        }
    };

    if !resp.status().is_success() {
        let msg = format!("host returned status {}", resp.status());
        let host_key = host.clone();
        let value = msg.clone();
        let _ = state
            .db_call("upsert_hoster", move |db| {
                crate::db::upsert_hoster(db, &host_key, "error", &value)
            })
            .await;
        return Ok(Json(json!({
            "status": "error",
            "host": host,
            "error": msg,
        })));
    }

    let cfg: serde_json::Value = match resp.json().await {
        Ok(cfg) => cfg,
        Err(e) => {
            let msg = format!("invalid config response: {e}");
            let host_key = host.clone();
            let value = msg.clone();
            let _ = state
                .db_call("upsert_hoster", move |db| {
                    crate::db::upsert_hoster(db, &host_key, "error", &value)
                })
                .await;
            return Ok(Json(json!({
                "status": "error",
                "host": host,
                "error": msg,
            })));
        }
    };

    let host_key = host.clone();
    let _ = state
        .db_call("upsert_hoster", move |db| {
            crate::db::upsert_hoster(db, &host_key, "ok", "")
        })
        .await;

    Ok(Json(json!({
        "status": "valid",
        "host": host,
        "max_file_size_bytes": cfg.get("max_file_size_bytes").and_then(serde_json::Value::as_u64).unwrap_or(0),
        "danger_level": cfg.get("danger_level").and_then(|v| v.as_str()).unwrap_or("high"),
    })))
}

/// Database errors are treated as no announcement.
#[utoipa::path(
    get,
    path = "/api/announcement",
    responses(
        (status = 200, description = "Active site announcement, or null", body = serde_json::Value),
    ),
    tag = "General",
)]
pub async fn announcement_handler(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let announcement = state
        .db_call("get_active_announcement", |db| {
            crate::db::get_active_announcement(db)
        })
        .await
        .ok()
        .flatten();

    match announcement {
        Some(a) => Json(json!({
            "message": a.message,
            "link_url": a.link_url,
            "mode": a.mode,
        })),
        None => Json(json!(null)),
    }
}

/// Hashes the supplied IP and reports only its ban state.
#[utoipa::path(
    get,
    path = "/api/ban-status",
    params(
        ("ip" = String, Query, description = "IP address to check"),
    ),
    responses(
        (status = 200, description = "Ban status for the given IP", body = serde_json::Value),
    ),
    tag = "General",
)]
pub async fn ban_status_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let ip_str = params
        .get("ip")
        .filter(|s| !s.is_empty())
        .cloned()
        .ok_or_else(|| AppError::BadRequest("ip parameter required".into()))?;

    let hashed = crate::utils::hash_ip_for_ban(&ip_str, &state.config.ip_pepper);

    let ban = state
        .db_call("get_ban_record", move |db| {
            crate::db::get_ban_record(db, &hashed)
        })
        .await
        .ok()
        .flatten();

    match ban {
        Some(b) => Ok(Json(json!({
            "banned": true,
            "reason": b.reason,
            "banned_at": b.banned_at,
        }))),
        None => Ok(Json(json!({ "banned": false }))),
    }
}

/// Internal API key required; the pepper and full hash list are sensitive.
#[utoipa::path(
    get,
    path = "/internal/ban-snapshot",
    responses(
        (status = 200, description = "Ban hash list and pepper", body = serde_json::Value),
    ),
    tag = "Internal",
    security(),
)]
pub async fn ban_snapshot_handler(
    State(state): State<Arc<AppState>>,
    _key: InternalApiKey,
) -> Result<Json<serde_json::Value>, AppError> {
    let (pepper, hashes) = crate::ban::snapshot(&state);

    tracing::debug!("internal ban snapshot: {} hashes", hashes.len());

    Ok(Json(json!({
        "pepper": pepper,
        "hashes": hashes,
        "count": hashes.len(),
    })))
}

pub(crate) async fn openapi_json_handler() -> (axum::http::header::HeaderMap, String) {
    let json = serde_json::to_string_pretty(&ApiDoc::openapi()).unwrap_or_default();
    let mut headers = axum::http::header::HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/json"),
    );
    (headers, json)
}
