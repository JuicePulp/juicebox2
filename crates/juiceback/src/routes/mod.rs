use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    extract::DefaultBodyLimit,
    http::{HeaderValue, Method, Request, Response, StatusCode, header},
    middleware,
    response::IntoResponse,
    routing::{delete, get, post},
};
use sentry::integrations::tower::NewSentryLayer;
use tower_governor::{GovernorLayer, governor::GovernorConfigBuilder};
use tower_http::{cors::CorsLayer, trace::TraceLayer};

use crate::{
    ban::middleware as ban_check_middleware,
    error::AppError,
    state::AppState,
    utils::{TrustedClientIpKeyExtractor, client_ip_middleware},
};

pub mod admin;
pub mod api_doc;
pub mod fetch;
pub mod health;
pub mod identity;
pub mod manage;
pub mod noscript;
pub mod pairing;
pub mod presence;
pub mod register;
pub mod tus;
pub mod upload;

pub use health::{
    announcement_handler, ban_snapshot_handler, ban_status_handler, config_handler, health_handler,
    ip_handler, validate_host_handler,
};
pub(crate) use identity::user_identity_middleware;
pub use identity::{UserId, UserIdMissing};

pub(crate) use health::openapi_json_handler;

#[cfg(feature = "quic")]
async fn add_security_headers(req: Request<Body>, next: middleware::Next) -> impl IntoResponse {
    juiceutils::add_security_headers(req, next).await
}

#[cfg(not(feature = "quic"))]
async fn add_security_headers(req: Request<Body>, next: middleware::Next) -> impl IntoResponse {
    let mut response = next.run(req).await;
    let headers = response.headers_mut();

    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; frame-ancestors 'none'",
        ),
    );
    headers.insert(
        "referrer-policy",
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    headers.insert(
        "permissions-policy",
        HeaderValue::from_static("geolocation=(), microphone=(), camera=()"),
    );

    response
}

async fn rate_limit_error_mapper(req: Request<Body>, next: middleware::Next) -> Response<Body> {
    let response = next.run(req).await;
    if response.status() != StatusCode::TOO_MANY_REQUESTS {
        return response;
    }
    let mut mapped = AppError::RateLimited.into_response();
    if let Some(after) = response.headers().get("x-ratelimit-after").cloned() {
        mapped
            .headers_mut()
            .insert("x-ratelimit-after", after.clone());
        mapped.headers_mut().insert(header::RETRY_AFTER, after);
    }
    mapped
}

pub fn build_router(state: Arc<AppState>) -> Router {
    let mut upload_builder = GovernorConfigBuilder::default();
    upload_builder.period(std::time::Duration::from_secs_f64(
        60.0 / f64::from(state.config.rate_limit_per_minute.max(1)),
    ));
    upload_builder.burst_size(state.config.rate_limit_per_minute.max(1));
    let upload_governor_conf = Arc::new(
        upload_builder
            .key_extractor(TrustedClientIpKeyExtractor::new(
                state.config.trusted_proxy_cidrs.clone(),
            ))
            .finish()
            .unwrap(),
    );

    let mut action_builder = GovernorConfigBuilder::default();
    action_builder.period(std::time::Duration::from_secs(6));
    action_builder.burst_size(10);
    let action_governor_conf = Arc::new(
        action_builder
            .key_extractor(TrustedClientIpKeyExtractor::new(
                state.config.trusted_proxy_cidrs.clone(),
            ))
            .finish()
            .unwrap(),
    );
    let mut sse_builder = GovernorConfigBuilder::default();
    sse_builder.period(std::time::Duration::from_secs_f64(
        60.0 / f64::from(crate::constants::MAX_SSE_CONNECTIONS_PER_IP),
    ));
    sse_builder.burst_size(crate::constants::MAX_SSE_CONNECTIONS_PER_IP);
    let sse_governor_conf = Arc::new(
        sse_builder
            .key_extractor(TrustedClientIpKeyExtractor::new(
                state.config.trusted_proxy_cidrs.clone(),
            ))
            .finish()
            .unwrap(),
    );

    let mut fetch_builder = GovernorConfigBuilder::default();
    fetch_builder.period(std::time::Duration::from_secs_f64(
        60.0 / f64::from(crate::constants::COBALT_FETCH_RATE_LIMIT_PER_MINUTE),
    ));
    fetch_builder.burst_size(crate::constants::COBALT_FETCH_RATE_LIMIT_PER_MINUTE);
    let fetch_governor_conf = Arc::new(
        fetch_builder
            .key_extractor(TrustedClientIpKeyExtractor::new(
                state.config.trusted_proxy_cidrs.clone(),
            ))
            .finish()
            .unwrap(),
    );

    {
        let governors = [
            Arc::clone(&upload_governor_conf),
            Arc::clone(&action_governor_conf),
            Arc::clone(&sse_governor_conf),
            Arc::clone(&fetch_governor_conf),
        ];
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(120));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                for conf in &governors {
                    conf.limiter().retain_recent();
                }
            }
        });
    }

    let cors = {
        let origins: Vec<HeaderValue> = state
            .config
            .cors_origins
            .iter()
            .filter_map(|o| match o.parse::<HeaderValue>() {
                Ok(v) => Some(v),
                Err(_) => {
                    tracing::warn!("invalid CORS origin ignored: {o}");
                    None
                }
            })
            .collect();
        CorsLayer::new()
            .allow_origin(origins)
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PATCH,
                Method::DELETE,
                Method::OPTIONS,
            ])
            .allow_credentials(true)
            .allow_headers([
                header::CONTENT_TYPE,
                header::AUTHORIZATION,
                header::HeaderName::from_static("x-file-encoding"),
                header::HeaderName::from_static("upload-length"),
                header::HeaderName::from_static("upload-offset"),
                header::HeaderName::from_static("upload-metadata"),
                header::HeaderName::from_static("tus-resumable"),
                header::HeaderName::from_static("x-delete-token"),
            ])
    };

    let max_size_bytes = state
        .jh_config
        .read()
        .unwrap()
        .as_ref()
        .map(|c| c.max_file_size_bytes as usize)
        .unwrap_or(500 * 1024 * 1024);

    let upload_router = Router::new()
        .route("/", post(upload::upload_handler))
        .route("/reserve", post(upload::reserve_upload_handler))
        .route(
            "/direct/reserve",
            post(upload::direct_upload_reserve_handler),
        )
        .route(
            "/direct/complete",
            post(upload::direct_upload_complete_handler),
        )
        .layer(GovernorLayer::new(upload_governor_conf.clone()))
        .layer(middleware::from_fn(rate_limit_error_mapper))
        .layer(DefaultBodyLimit::max(
            max_size_bytes + crate::constants::UPLOAD_SIZE_OVERHEAD_BYTES,
        ));

    let tus_router = tus::tus_routes()
        .layer(GovernorLayer::new(action_governor_conf.clone()))
        .layer(DefaultBodyLimit::max(
            max_size_bytes + crate::constants::UPLOAD_SIZE_OVERHEAD_BYTES,
        ));

    let abuse_routes = Router::new()
        .route("/api/report", post(noscript::report_submit_handler))
        .route("/api/feedback", post(noscript::feedback_submit_handler))
        .route("/api/pair/generate", post(pairing::generate_code_handler))
        .route("/api/pair/verify", post(pairing::verify_code_handler))
        .layer(GovernorLayer::new(action_governor_conf.clone()));

    let small_payload_routes = Router::new()
        .route("/api/owned-files", post(manage::owned_files_handler))
        .route(
            "/api/client-files",
            get(manage::list_client_files_handler).post(manage::put_client_files_handler),
        )
        .route(
            "/api/client-files/migrate",
            post(manage::put_client_files_handler),
        )
        .route("/api/register", post(register::register_handler))
        .merge(abuse_routes)
        .layer(DefaultBodyLimit::max(64 * 1024));

    let presence_route = Router::new()
        .route("/api/presence", get(presence::presence_sse_handler))
        .layer(GovernorLayer::new(sse_governor_conf));

    let fetch_start_route = Router::new()
        .route("/api/fetch", post(fetch::fetch_start_handler))
        .layer(GovernorLayer::new(fetch_governor_conf))
        .layer(DefaultBodyLimit::max(64 * 1024));

    let fetch_services_route = Router::new()
        .route("/api/fetch/services", get(fetch::fetch_services_handler))
        .layer(DefaultBodyLimit::max(64 * 1024));

    let validate_routes = Router::new()
        .route("/api/validate-host", post(validate_host_handler))
        .route("/api/ip", get(ip_handler))
        .layer(GovernorLayer::new(action_governor_conf.clone()))
        .layer(DefaultBodyLimit::max(64 * 1024));

    Router::new()
        .nest("/upload", upload_router)
        .merge(tus_router)
        .route("/file/{id}/info", get(manage::file_info_handler))
        .route("/file/{id}/renew", post(manage::renew_file_id_handler))
        .route("/file/{id}", delete(manage::delete_file_handler))
        .route(
            "/internal/file/{id}/status",
            get(upload::file_status_handler),
        )
        .route(
            "/internal/alias/{old_id}",
            get(manage::resolve_alias_handler),
        )
        .route("/internal/ban-snapshot", get(ban_snapshot_handler))
        .route("/file/{id}/delete", post(manage::delete_file_form_handler))
        .route("/file/{id}/rename", post(manage::rename_file_form_handler))
        .merge(small_payload_routes)
        .route("/api/health", get(health_handler))
        .route("/api/config", get(config_handler))
        .route("/api/fetch/{job_id}", get(fetch::fetch_status_handler))
        .route("/api/announcement", get(announcement_handler))
        .route("/api/ban-status", get(ban_status_handler))
        .route("/api/openapi.json", get(openapi_json_handler))
        .route("/api/device", get(pairing::list_devices_handler))
        .route("/api/device/{id}", delete(pairing::unpair_device_handler))
        .route("/api/device/ws", get(presence::device_ws_handler))
        .route("/api/device/status", get(presence::device_status_handler))
        .route(
            "/api/device/connected",
            get(presence::list_connected_devices_handler),
        )
        .route("/api/device/ping", post(presence::ping_device_handler))
        .merge(validate_routes)
        .merge(fetch_start_route)
        .merge(fetch_services_route)
        .merge(presence_route)
        .merge(admin::admin_routes(&state.config.trusted_proxy_cidrs))
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            ban_check_middleware,
        ))
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            client_ip_middleware,
        ))
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            user_identity_middleware,
        ))
        .layer(middleware::from_fn(add_security_headers))
        .layer(
            TraceLayer::new_for_http().make_span_with(|request: &Request<Body>| {
                tracing::info_span!(
                    "http.request",
                    method = %request.method(),
                    path = request.uri().path(),
                    version = ?request.version(),
                    client_ip = tracing::field::Empty,
                )
            }),
        )
        .layer(NewSentryLayer::<Request<Body>>::new_from_top())
        .layer(cors)
        .with_state(state)
}
