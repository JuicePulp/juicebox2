use std::{net::SocketAddr, sync::Arc};

use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, header},
    response::{IntoResponse, Json, Redirect, Response},
};
use serde_json::json;

use crate::{
    error::{JuicehostError, StorageError, not_found_html},
    state::AppState,
    storage,
    storage::valid_component as is_valid_id,
};

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
pub async fn health() -> Response {
    // Static liveness only: no backend probe, no per-request metric.
    let body = serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "juicehost": "ok",
    });

    let mut resp = Response::new(Body::from(serde_json::to_string(&body).unwrap_or_default()));
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );
    resp
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
        "version": env!("CARGO_PKG_VERSION"),
        "default_ttl_hours": state.default_ttl_hours,
        "allowed_ttl_hours": state.allowed_ttl_hours,
        "danger_level": danger_level_str,
        "quick_link": state.quick_link,
        "custom_id": state.custom_id,
        "ultrafast": !state.ticket_jwt_secret.is_empty(),
        "backend_url": state.backend_url,
        "frontend_url": state.frontend_url,
        "password_links": true,
        "at_rest_via_juiceback": true,
    }))
}

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
        // Frozen files still exist: report the hold (without bytes) so
        // admins and health checks can see it. Not an error.
        Err(StorageError::Frozen) => Ok(Json(serde_json::json!({
            "exists": true,
            "frozen": true,
            "id": id,
        }))),
        Err(StorageError::NotFound) => Ok(Json(serde_json::json!({
            "exists": false,
            "id": id,
        }))),
        Err(_) => Err(JuicehostError::Internal),
    }
}

#[utoipa::path(
    get,
    path = "/internal/file/{id}/ciphertext",
    params(("id" = String, Path, description = "File ID")),
    responses(
        (status = 200, description = "Stored ciphertext bytes"),
        (status = 206, description = "Ciphertext byte range"),
        (status = 400, description = "Invalid file ID"),
        (status = 401, description = "Missing or invalid credentials"),
        (status = 404, description = "File not found"),
        (status = 416, description = "Range not satisfiable"),
    ),
    tag = "Internal",
)]
#[tracing::instrument(skip_all)]
pub async fn ciphertext_file(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response<Body>, JuicehostError> {
    serve_stored_bytes(&state, &headers, &id).await
}

/// Public ciphertext bytes for browser-side decryption (`/c/{id}`).
/// No gate: ciphertext is useless without the data key, which juicehost
/// never holds. Same bytes and Range semantics as the internal endpoint.
#[tracing::instrument(skip_all)]
pub async fn ciphertext_public(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(path): Path<String>,
) -> Result<Response<Body>, JuicehostError> {
    let id = path.split('.').next().unwrap_or(&path).to_string();
    serve_stored_bytes(&state, &headers, &id).await
}

pub(crate) async fn serve_stored_bytes(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    id: &str,
) -> Result<Response<Body>, JuicehostError> {
    use futures::StreamExt as _;

    if !is_valid_id(id) {
        return Err(JuicehostError::BadRequest);
    }
    let meta = state.storage.stat(id).await.map_err(|e| match e {
        StorageError::NotFound => JuicehostError::NotFound,
        StorageError::Frozen => JuicehostError::Frozen,
        _ => JuicehostError::Internal,
    })?;
    let total_size = meta.size;

    if let Some(range_header) = headers.get(axum::http::header::RANGE)
        && let Ok(range_val) = range_header.to_str()
    {
        match super::serve::parse_range(range_val, total_size, u64::MAX) {
            super::serve::RangeResult::Satisfiable(start, end) => {
                let stream = state
                    .storage
                    .get_range_stream(id, start, end)
                    .await
                    .map_err(|e| match e {
                        StorageError::Frozen => JuicehostError::Frozen,
                        _ => JuicehostError::Internal,
                    })?;
                return Response::builder()
                    .status(axum::http::StatusCode::PARTIAL_CONTENT)
                    .header(axum::http::header::CONTENT_TYPE, "application/octet-stream")
                    .header(axum::http::header::CONTENT_LENGTH, end - start + 1)
                    .header(
                        axum::http::header::CONTENT_RANGE,
                        format!("bytes {start}-{end}/{total_size}"),
                    )
                    .header(axum::http::header::ACCEPT_RANGES, "bytes")
                    .header(axum::http::header::CACHE_CONTROL, "no-store")
                    .body(Body::from_stream(stream.map(|item| {
                        item.map_err(|_| {
                            axum::Error::new(std::io::Error::other("ciphertext stream failed"))
                        })
                    })))
                    .map_err(|_| JuicehostError::Internal);
            }
            super::serve::RangeResult::Unsatisfiable => {
                return Response::builder()
                    .status(axum::http::StatusCode::RANGE_NOT_SATISFIABLE)
                    .header(
                        axum::http::header::CONTENT_RANGE,
                        format!("bytes */{total_size}"),
                    )
                    .header(axum::http::header::ACCEPT_RANGES, "bytes")
                    .body(Body::empty())
                    .map_err(|_| JuicehostError::Internal);
            }
            super::serve::RangeResult::Ignore => {}
        }
    }

    let stream = state.storage.get_stream(id).await.map_err(|e| match e {
        StorageError::Frozen => JuicehostError::Frozen,
        _ => JuicehostError::Internal,
    })?;
    Response::builder()
        .status(axum::http::StatusCode::OK)
        .header(axum::http::header::CONTENT_TYPE, "application/octet-stream")
        .header(axum::http::header::CONTENT_LENGTH, total_size)
        .header(axum::http::header::ACCEPT_RANGES, "bytes")
        .header(axum::http::header::CACHE_CONTROL, "no-store")
        .body(Body::from_stream(stream.map(|item| {
            item.map_err(|_| axum::Error::new(std::io::Error::other("ciphertext stream failed")))
        })))
        .map_err(|_| JuicehostError::Internal)
}
