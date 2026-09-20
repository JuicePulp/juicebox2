use std::{net::SocketAddr, sync::Arc};

use axum::{
    Json,
    extract::{ConnectInfo, Path, State},
    http::HeaderMap,
};

use super::{
    job::run_fetch_job,
    types::{
        FetchFileResponse, FetchServicesResponse, FetchStartRequest, FetchStartResponse,
        FetchStatusResponse, ServicesCache,
    },
    validation::{require_cobalt_enabled, service_domain, validate_source_url},
};
use crate::{
    cobalt::FetchOptions,
    db::{self, FetchJob, FileRecord},
    error::AppError,
    routes::UserId,
    state::AppState,
};

#[utoipa::path(
    post,
    path = "/api/fetch",
    request_body = FetchStartRequest,
    responses(
        (status = 200, description = "Fetch job queued", body = FetchStartResponse),
        (status = 400, description = "Invalid or unsafe URL"),
        (status = 404, description = "Cobalt fetching is disabled on this server"),
        (status = 429, description = "Rate limit exceeded (3 requests per minute)"),
    ),
    tag = "Cobalt",
)]
#[tracing::instrument(skip_all)]
pub async fn fetch_start_handler(
    State(state): State<Arc<AppState>>,
    UserId(user_id): UserId,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<FetchStartRequest>,
) -> Result<Json<FetchStartResponse>, AppError> {
    require_cobalt_enabled(&state)?;

    let source_url = validate_source_url(&body.url, state.config.allow_private_fetch).await?;
    let opts = FetchOptions::sanitized(
        body.audio_only,
        body.video_quality.as_deref(),
        body.video_container.as_deref(),
        body.audio_format.as_deref(),
        body.better_audio,
        body.youtube_video_codec.as_deref(),
    );

    let raw_ip = crate::utils::client_ip(&headers, addr.ip(), &state).to_string();
    let encrypted_ip = crate::utils::encrypt_ip(&raw_ip, &state.config.ip_encryption_key);

    let job_id = nanoid::nanoid!(12);
    let job_id_clone = job_id.clone();
    let source_url_for_db = source_url.clone();
    let user_id_for_db = user_id.clone();
    state
        .db_call("insert_fetch_job", move |db| {
            db::insert_fetch_job(db, &job_id_clone, &user_id_for_db, &source_url_for_db)
        })
        .await?;

    tokio::spawn(run_fetch_job(
        Arc::clone(&state),
        job_id.clone(),
        user_id,
        source_url,
        opts,
        encrypted_ip,
    ));

    tracing::info!("fetch job {job_id} queued");
    Ok(Json(FetchStartResponse { job_id }))
}

static SERVICES_CACHE: std::sync::LazyLock<ServicesCache> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(None));
const SERVICES_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(3600);

#[utoipa::path(
    get,
    path = "/api/fetch/services",
    responses(
        (status = 200, description = "Supported service domains", body = FetchServicesResponse),
        (status = 404, description = "Cobalt fetching is disabled"),
        (status = 502, description = "Cobalt instance unreachable"),
    ),
    tag = "Cobalt",
)]
pub async fn fetch_services_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<FetchServicesResponse>, AppError> {
    if !state.config.cobalt_enabled {
        return Err(AppError::NotFound);
    }

    {
        let cache = SERVICES_CACHE.lock().await;
        if let Some((fetched_at, services)) = cache.as_ref()
            && fetched_at.elapsed() < SERVICES_CACHE_TTL
        {
            return Ok(Json(FetchServicesResponse {
                services: services.as_ref().clone(),
            }));
        }
    }

    let url = format!("{}/", state.config.cobalt_api_url.trim_end_matches('/'));
    let mut request = state.http.get(&url);
    if !state.config.cobalt_api_key.is_empty() {
        request = request.header(
            "Authorization",
            format!("Api-Key {}", state.config.cobalt_api_key),
        );
    }
    let response = request
        .send()
        .await
        .map_err(|_| AppError::ServiceUnavailable("cobalt instance unreachable".into()))?;
    if !response.status().is_success() {
        return Err(AppError::ServiceUnavailable(format!(
            "cobalt returned {}",
            response.status()
        )));
    }
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|_| AppError::ServiceUnavailable("invalid cobalt response".into()))?;

    let mut services: Vec<String> = body["cobalt"]["services"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(service_domain)
                .collect()
        })
        .unwrap_or_default();
    services.sort();
    services.dedup();

    let services = Arc::new(services);
    *SERVICES_CACHE.lock().await = Some((std::time::Instant::now(), Arc::clone(&services)));

    Ok(Json(FetchServicesResponse {
        services: services.as_ref().clone(),
    }))
}

/// `GET /api/fetch/:id` polls a fetch job. Only the owner can see it.
#[utoipa::path(
    get,
    path = "/api/fetch/{job_id}",
    params(("job_id" = String, Path, description = "Fetch job id")),
    responses(
        (status = 200, description = "Current job status", body = FetchStatusResponse),
        (status = 404, description = "Job not found (or not yours)"),
    ),
    tag = "Cobalt",
)]
pub async fn fetch_status_handler(
    State(state): State<Arc<AppState>>,
    UserId(user_id): UserId,
    Path(job_id): Path<String>,
) -> Result<Json<FetchStatusResponse>, AppError> {
    let job_id_clone = job_id.clone();
    let job: Option<FetchJob> = state
        .db_call("get_fetch_job", move |db| {
            db::get_fetch_job(db, &job_id_clone)
        })
        .await?;

    let Some(job) = job else {
        return Err(AppError::NotFound);
    };
    if job.user_id != user_id {
        return Err(AppError::NotFound);
    }

    let mut response = FetchStatusResponse {
        job_id: job.id.clone(),
        status: job.status.clone(),
        error: job.error.clone(),
        file: None,
        stage: if job.stage.is_empty() {
            None
        } else {
            Some(job.stage.clone())
        },
        bytes_received: if job.bytes_received > 0 {
            Some(job.bytes_received as u64)
        } else {
            None
        },
    };

    if job.status == "done" && !job.file_id.is_empty() {
        let file_id = job.file_id.clone();
        let record: Option<FileRecord> = state
            .db_call("get_file", move |db| db::get_file(db, &file_id))
            .await?;
        if let Some(record) = record {
            let url = crate::utils::public_url(
                &state.config.public_base_url,
                &record.storage_host,
                &record.id,
                &record.filename,
            );
            response.file = Some(FetchFileResponse {
                id: record.id,
                filename: record.filename,
                mime_type: record.mime_type,
                size_bytes: record.size_bytes,
                url,
                expires_at: record.expires_at,
                delete_token: record.delete_token,
            });
        }
    }

    Ok(Json(response))
}
