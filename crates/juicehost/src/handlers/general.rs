use std::{net::SocketAddr, sync::Arc};

use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, header},
    response::{IntoResponse, Json, Redirect, Response},
};
use serde_json::json;

use super::common::backend_request;
use crate::{
    error::{JuicehostError, StorageError, not_found_html},
    state::AppState,
    storage,
    storage::valid_component as is_valid_id,
};

/// Header sent on peer health probes. The receiving side skips probing back so
/// juiceback and juicehost don't recurse into each other's /api/health forever.
const HEALTH_PROBE_HEADER: &str = "x-health-probe";

/// Serve the index page. Redirects to the configured juicefront URL, or 404s
/// when no frontend is configured.
#[utoipa::path(
    get,
    path = "/",
    responses(
        (status = 308, description = "Permanent redirect to FRONTEND_URL"),
        (status = 404, description = "No frontend configured"),
    ),
    tag = "General",
)]
pub async fn index_handler(State(state): State<Arc<AppState>>) -> Response {
    match &state.frontend_url {
        Some(url) => Redirect::permanent(url).into_response(),
        None => not_found_html().into_response(),
    }
}

#[utoipa::path(
    get,
    path = "/api/health",
    responses(
        (status = 200, description = "Health status. Storage metrics live at /api/storage", body = serde_json::Value),
    ),
    tag = "General",
)]
pub async fn health(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let mut body = serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "juicehost": "ok",
    });

    // Skip the peer probe when this request was itself a health probe.
    if !headers.contains_key(HEALTH_PROBE_HEADER) {
        if let Some(ref backend_url) = state.backend_url {
            body["juiceback"] =
                serde_json::json!(if check_backend_health(&state, backend_url).await {
                    "ok"
                } else {
                    "unreachable"
                });
        }
    }

    let mut resp = Response::new(Body::from(serde_json::to_string(&body).unwrap_or_default()));
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );
    sentry::metrics::counter("juicehost.health", 1).capture();
    resp
}

/// Probes the juiceback health endpoint if it is configured
async fn check_backend_health(state: &AppState, backend_url: &str) -> bool {
    let url = format!("{}/api/health", backend_url.trim_end_matches('/'));
    let Ok(resp) = backend_request(state, url)
        .header(HEALTH_PROBE_HEADER, "1")
        .send()
        .await
    else {
        return false;
    };
    if !resp.status().is_success() {
        return false;
    }
    resp.json::<serde_json::Value>().await.is_ok_and(|v| {
        v.get("status")
            .and_then(|s| s.as_str())
            .is_some_and(|s| s == "ok")
    })
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
    let ip = juiceutils::proxy::client_ip(&headers, peer.ip(), &state.trusted_proxy_cidrs);
    Json(json!({
        "ip": ip.to_string(),
        "version": if ip.is_ipv6() { "ipv6" } else { "ipv4" },
    }))
}

#[utoipa::path(
    get,
    path = "/api/storage",
    responses(
        (status = 200, description = "Storage metrics", body = storage::StorageMetrics),
    ),
    tag = "General",
)]
pub async fn storage_handler(State(state): State<Arc<AppState>>) -> Json<storage::StorageMetrics> {
    Json(state.storage.storage_metrics(state.min_free_space_bytes))
}

/// Return the instance's upload configuration as JSON.
#[utoipa::path(
    get,
    path = "/api/config",
    responses(
        (status = 200, description = "Instance configuration", body = serde_json::Value),
    ),
    tag = "General",
)]
pub async fn config_handler(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let danger_level_str = state.danger_level.as_str();
    Json(serde_json::json!({
        "max_file_size_bytes": state.max_file_size_bytes,
        "default_ttl_hours": state.default_ttl_hours,
        "allowed_ttl_hours": state.allowed_ttl_hours,
        "danger_level": danger_level_str,
        "quick_link": state.quick_link,
        "custom_id": state.custom_id,
        "ultrafast": !state.ticket_jwt_secret.is_empty(),
    }))
}

/// Report whether a file exists in the configured storage backend and its size.
#[utoipa::path(
    get,
    path = "/internal/file/{id}/stat",
    params(("id" = String, Path, description = "The file ID")),
    responses(
        (status = 200, description = "File status", body = serde_json::Value),
        (status = 400, description = "Invalid file ID"),
        (status = 401, description = "Missing or invalid credentials"),
    ),
    tag = "Internal",
)]
#[tracing::instrument(skip_all)]
pub async fn stat_file(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, JuicehostError> {
    if !is_valid_id(&id) {
        return Err(JuicehostError::BadRequest);
    }

    match state.storage.stat(&id).await {
        Ok(meta) => Ok(Json(serde_json::json!({
            "exists": true,
            "id": id,
            "size_bytes": meta.size,
            "extension": meta.extension,
        }))),
        Err(StorageError::NotFound) => Ok(Json(serde_json::json!({
            "exists": false,
            "id": id,
        }))),
        Err(_) => Err(JuicehostError::Internal),
    }
}
