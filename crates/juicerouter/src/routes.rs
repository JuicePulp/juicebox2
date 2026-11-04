use std::sync::Arc;

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, Request, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};
use tower_http::cors::{Any, CorsLayer};

use crate::{
    config::LocalMode,
    state::{AppState, HealthState, NodeHealth, RouteDecision, RouteKind, Selection},
};

pub fn build(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/hosts", get(hosts))
        .route("/route/{id}", get(route_info))
        .route("/upload-target", get(upload_target))
        .route("/v/{*path}", get(redirect))
        .route("/f/{*path}", get(redirect))
        .route("/d/{*path}", get(redirect))
        .route("/e/{*path}", get(redirect))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([axum::http::Method::GET])
                .allow_headers(Any),
        )
        .with_state(state)
}

#[derive(Serialize)]
struct NodeView {
    region: String,
    url: String,
    health: NodeHealth,
}

#[derive(Serialize)]
struct HealthView {
    status: &'static str,
    base_url: String,
    healthy: usize,
    nodes: Vec<NodeView>,
}

fn node_views(state: &AppState) -> Vec<NodeView> {
    state
        .registry
        .nodes
        .iter()
        .map(|node| NodeView {
            region: node.region.clone(),
            url: node.url.clone(),
            health: node.health(),
        })
        .collect()
}

async fn health(State(state): State<Arc<AppState>>) -> Json<HealthView> {
    let nodes = node_views(&state);
    let healthy = nodes
        .iter()
        .filter(|node| node.health.state == HealthState::Healthy)
        .count();
    Json(HealthView {
        status: "ok",
        base_url: state.config.router.public_base_url.clone(),
        healthy,
        nodes,
    })
}

async fn hosts(State(state): State<Arc<AppState>>) -> Json<Vec<NodeView>> {
    Json(node_views(&state))
}

#[derive(Deserialize)]
struct RouteQuery {
    #[serde(default)]
    route: Option<String>,
    #[serde(default)]
    path: Option<String>,
}

async fn route_info(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(query): Query<RouteQuery>,
) -> Result<Json<RouteDecision>, ApiError> {
    let id = parse_id(&id)?;
    let route = clean_route(query.route.as_deref().unwrap_or("v"));
    let path = query.path.unwrap_or_else(|| id.clone());
    let record = state
        .resolve(&id)
        .await
        .map_err(|_| ApiError::Unavailable)?;
    let Some(record) = record else {
        return Err(ApiError::NotFound);
    };
    Ok(Json(state.decision(&id, &record, route, &path)))
}

#[derive(Deserialize)]
struct UploadTargetQuery {
    #[serde(default)]
    region: Option<String>,
}

async fn upload_target(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<UploadTargetQuery>,
) -> Result<Json<Selection>, ApiError> {
    let country = headers
        .get("cf-ipcountry")
        .and_then(|value| value.to_str().ok());
    let colo = headers
        .get("cf-ray")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split('-').nth(1));
    let selection = state
        .registry
        .select(
            &state.config.geo,
            query.region.as_deref(),
            country,
            colo,
        )
        .ok_or(ApiError::Unavailable)?;
    tracing::info!(
        "upload-target region={} reason={} country={:?} colo={:?}",
        selection.region,
        selection.reason,
        country,
        colo
    );
    Ok(Json(selection))
}

async fn redirect(
    State(state): State<Arc<AppState>>,
    Path(path): Path<String>,
    request: Request<Body>,
) -> Result<Response, ApiError> {
    let uri = request.uri().clone();
    let route = uri
        .path()
        .trim_start_matches('/')
        .split('/')
        .next()
        .map_or("f", clean_route);
    let id = parse_id(path.split('.').next().unwrap_or(&path))?;

    let record = state
        .resolve(&id)
        .await
        .map_err(|_| ApiError::Unavailable)?;
    let Some(record) = record else {
        return Err(ApiError::NotFound);
    };
    let decision = state.decision(&id, &record, route, &path);

    match decision.kind {
        RouteKind::Uploading => Err(ApiError::Uploading),
        RouteKind::Local if state.config.router.local_mode == LocalMode::Proxy => {
            proxy_local(&state, request).await
        }
        _ => {
            let Some(target) = decision.target else {
                return Err(ApiError::NotFound);
            };
            tracing::debug!("route id={id} kind={} -> {target}", decision.kind.as_str());
            Response::builder()
                .status(StatusCode::FOUND)
                .header(header::LOCATION, target)
                .header(header::CACHE_CONTROL, "no-store")
                .body(Body::empty())
                .map_err(|_| ApiError::Internal)
        }
    }
}

fn clean_route(route: &str) -> &'static str {
    match route {
        "f" => "f",
        "d" => "d",
        "e" => "e",
        _ => "v",
    }
}

fn parse_id(raw: &str) -> Result<String, ApiError> {
    let id = raw.trim();
    if juiceutils::ids::is_valid_custom_id(id, 4, 64) {
        Ok(id.to_string())
    } else {
        Err(ApiError::BadRequest("invalid file id"))
    }
}

fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name,
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "trailers"
            | "transfer-encoding"
            | "upgrade"
    )
}

async fn proxy_local(state: &AppState, request: Request<Body>) -> Result<Response, ApiError> {
    let upstream_base = state
        .config
        .router
        .local_upstream
        .trim_end_matches('/')
        .to_string();
    let path_query = request
        .uri()
        .path_and_query()
        .map_or("/".to_string(), |value| value.as_str().to_string());
    let url = format!("{upstream_base}{path_query}");
    let method = request.method().clone();

    let mut builder = state.stream_client.request(method, url);
    for (name, value) in request.headers() {
        if name == header::HOST || is_hop_by_hop(name.as_str()) {
            continue;
        }
        builder = builder.header(name, value);
    }
    let body = reqwest::Body::wrap_stream(request.into_body().into_data_stream());
    let upstream = builder.body(body).send().await.map_err(|error| {
        tracing::warn!("local proxy failed: {error}");
        ApiError::Unavailable
    })?;

    let status = upstream.status();
    let mut response = Response::builder().status(status);
    for (name, value) in upstream.headers() {
        if is_hop_by_hop(name.as_str()) {
            continue;
        }
        response = response.header(name, value);
    }
    response
        .body(Body::from_stream(upstream.bytes_stream()))
        .map_err(|_| ApiError::Internal)
}

enum ApiError {
    NotFound,
    BadRequest(&'static str),
    Uploading,
    Unavailable,
    Internal,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::NotFound => (StatusCode::NOT_FOUND, "not found"),
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            Self::Uploading => (StatusCode::TOO_EARLY, "upload still in progress"),
            Self::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "route unavailable"),
            Self::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "internal error"),
        };
        let mut response = (status, Json(serde_json::json!({ "error": message }))).into_response();
        if status == StatusCode::TOO_EARLY {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, header::HeaderValue::from_static("5"));
        }
        response
    }
}
