//! JuiceBox x Cobalt.Tools: paste a link (YouTube, tiktok, ...), juiceback
//! asks a self-hosted cobalt instance to process it, streams the result into
//! juicehost and hands the user a normal /f/ file like any other upload.

use axum::{
    extract::{ConnectInfo, Path, State},
    http::HeaderMap,
    Json,
};
use std::path::PathBuf;

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use utoipa::ToSchema;

use crate::cobalt::{self, CobaltResponse, FetchOptions};
use crate::db::{self, FetchJob, FileRecord};
use crate::error::AppError;
use crate::routes::UserId;
use crate::state::AppState;
use crate::upload_mode::UploadMode;

/// JSON body for starting a fetch job.
#[derive(Debug, Deserialize, ToSchema)]
pub struct FetchStartRequest {
    /// Source URL (YouTube, tiktok, etc).
    pub url: String,
    /// Audio-only fetch (default false = video+audio).
    #[serde(default)]
    pub audio_only: bool,
    /// Video quality cap: max/2160/1440/1080/720/480/360/240/144.
    #[serde(default)]
    pub video_quality: Option<String>,
    /// Video container preference (YouTube): auto/mp4/webm/mkv.
    #[serde(default)]
    pub video_container: Option<String>,
    /// Audio format for audio-only: best/mp3/ogg/wav/opus.
    #[serde(default)]
    pub audio_format: Option<String>,
    /// Ask cobalt to hunt for the highest available audio quality (YouTube;
    /// requires YOUTUBE_ALLOW_BETTER_AUDIO on the instance).
    #[serde(default)]
    pub better_audio: bool,
    /// YouTube codec preference: h264/av1/vp9.
    #[serde(default)]
    pub youtube_video_codec: Option<String>,
}

/// JSON response for `POST /api/fetch`.
#[derive(Debug, Serialize, ToSchema)]
pub struct FetchStartResponse {
    pub job_id: String,
}

/// File details returned with a completed fetch job.
#[derive(Debug, Serialize, ToSchema)]
pub struct FetchFileResponse {
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub url: String,
    pub expires_at: i64,
    pub delete_token: String,
}

/// JSON response for `GET /api/fetch/:id`.
#[derive(Debug, Serialize, ToSchema)]
pub struct FetchStatusResponse {
    pub job_id: String,
    /// `pending`, `processing`, `downloading`, `done`, or `failed`.
    pub status: String,
    #[serde(default)]
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<FetchFileResponse>,
    /// Fine-grained progress hint while the job is still running.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    /// Bytes received from the media source so far.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_received: Option<u64>,
}

/// Validate a user-supplied source URL before handing it to cobalt.
/// Blocks non-http(s) schemes, credentials, local/metadata hosts, and
/// private IP literals so cobalt can't be pointed at internal networks.
fn validate_source_url(raw: &str) -> Result<String, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AppError::BadRequest("url is required".into()));
    }
    if trimmed.len() > crate::constants::FETCH_SOURCE_URL_MAX_LEN {
        return Err(AppError::BadRequest("url is too long".into()));
    }
    let parsed =
        url::Url::parse(trimmed).map_err(|_| AppError::BadRequest("invalid url".into()))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(AppError::BadRequest(
            "url scheme must be http or https".into(),
        ));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(AppError::BadRequest(
            "url must not contain credentials".into(),
        ));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| AppError::BadRequest("url must include a hostname".into()))?
        .to_ascii_lowercase();
    if crate::juicehost::is_forbidden_hostname(&host) {
        return Err(AppError::BadRequest(format!(
            "refusing to fetch from local host: {}",
            host
        )));
    }
    if let Ok(ip) = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<std::net::IpAddr>()
    {
        // IPv6 url hosts arrive bracketed; strip before parsing.
        if !crate::juicehost::is_public_ip(ip) {
            return Err(AppError::BadRequest(format!(
                "refusing to fetch from non-public address: {}",
                ip
            )));
        }
    }
    Ok(parsed.to_string())
}

fn require_cobalt_enabled(state: &AppState) -> Result<(), AppError> {
    if !state.config.cobalt_enabled {
        return Err(AppError::NotFound);
    }
    Ok(())
}

/// `POST /api/fetch` — queue a URL for processing. Returns immediately with
/// a job id; poll `GET /api/fetch/:id` for the outcome.
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

    let source_url = validate_source_url(&body.url)?;
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

    tracing::info!("fetch job {} queued", job_id);
    Ok(Json(FetchStartResponse { job_id }))
}

/// Human-readable domain for a cobalt service name (e.g. "twitter" -> "x.com").
fn service_domain(name: &str) -> String {
    match name {
        "youtube" => "youtube.com",
        "twitter" => "x.com",
        "instagram" => "instagram.com",
        "tiktok" => "tiktok.com",
        "reddit" => "reddit.com",
        "soundcloud" => "soundcloud.com",
        "twitch clips" => "twitch.tv",
        "bluesky" => "bsky.app",
        "facebook" => "facebook.com",
        "pinterest" => "pinterest.com",
        "tumblr" => "tumblr.com",
        "vimeo" => "vimeo.com",
        "dailymotion" => "dailymotion.com",
        "bilibili" => "bilibili.com",
        "loom" => "loom.com",
        "ok" => "ok.ru",
        "rutube" => "rutube.ru",
        "snapchat" => "snapchat.com",
        "streamable" => "streamable.com",
        "vk" => "vk.com",
        "newgrounds" => "newgrounds.com",
        other => other,
    }
    .to_string()
}

/// Cached service list from the cobalt instance (refreshed hourly).
type ServicesCache = tokio::sync::Mutex<Option<(std::time::Instant, Arc<Vec<String>>)>>;
static SERVICES_CACHE: std::sync::LazyLock<ServicesCache> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(None));
const SERVICES_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(3600);

/// Response for `GET /api/fetch/services`.
#[derive(Debug, Serialize, ToSchema)]
pub struct FetchServicesResponse {
    /// Downloadable-service domains supported by the cobalt instance.
    pub services: Vec<String>,
}

/// `GET /api/fetch/services` — list of services the cobalt instance supports,
/// fetched server-side and cached (clients never talk to cobalt directly).
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

    // Serve from cache while fresh.
    {
        let cache = SERVICES_CACHE.lock().await;
        if let Some((fetched_at, services)) = cache.as_ref() {
            if fetched_at.elapsed() < SERVICES_CACHE_TTL {
                return Ok(Json(FetchServicesResponse {
                    services: services.as_ref().clone(),
                }));
            }
        }
    }

    let url = format!("{}/", state.config.cobalt_api_url.trim_end_matches('/'));
    let mut request = state.http.get(&url);
    if !state.config.cobalt_api_key.is_empty() {
        request = request.header("Authorization", format!("Api-Key {}", state.config.cobalt_api_key));
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
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).map(service_domain).collect())
        .unwrap_or_default();
    services.sort();
    services.dedup();

    let services = Arc::new(services);
    *SERVICES_CACHE.lock().await = Some((std::time::Instant::now(), Arc::clone(&services)));

    Ok(Json(FetchServicesResponse {
        services: services.as_ref().clone(),
    }))
}

/// `GET /api/fetch/:id` — poll a fetch job. Only the owner sees it.
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

/// Background task driving one fetch job end-to-end:
/// cobalt API -> tunnel stream -> juicehost -> normal file record.
async fn run_fetch_job(
    state: Arc<AppState>,
    job_id: String,
    user_id: String,
    source_url: String,
    opts: FetchOptions,
    encrypted_ip: Option<String>,
) {
    let timeout = std::time::Duration::from_secs(crate::constants::FETCH_JOB_TIMEOUT_SECS);
    match tokio::time::timeout(
        timeout,
        run_fetch_job_inner(&state, &job_id, &user_id, &source_url, &opts, encrypted_ip),
    )
    .await
    {
        Ok(Ok(file_id)) => {
            let job_id_for_db = job_id.clone();
            let file_id_for_db = file_id.clone();
            let _ = state
                .db_call("finish_fetch_job_done", move |db| {
                    db::finish_fetch_job(db, &job_id_for_db, "done", "", &file_id_for_db)
                        .map(|_: bool| ())
                })
                .await;
            tracing::info!("fetch job {} done: {}", job_id, file_id);
        }
        Ok(Err(msg)) => fail_fetch_job(&state, &job_id, &msg).await,
        Err(_) => fail_fetch_job(&state, &job_id, "fetch timed out").await,
    }
}

async fn fail_fetch_job(state: &Arc<AppState>, job_id: &str, msg: &str) {
    tracing::warn!("fetch job {} failed: {}", job_id, msg);
    let job_id_owned = job_id.to_string();
    let msg_owned = msg.to_string();
    let _ = state
        .db_call("finish_fetch_job_failed", move |db| {
            db::finish_fetch_job(db, &job_id_owned, "failed", &msg_owned, "").map(|_: bool| ())
        })
        .await;
}

type FetchResult = Result<String, String>;

/// Where the media bytes come from: a cobalt tunnel URL or a local file
/// produced by the yt-dlp fallback tier.
enum ByteSource {
    Tunnel(String),
    LocalFile(PathBuf),
}

/// Empty stream payload: for YouTube links this is the platform's
/// content-bound streaming-token enforcement (not a quality/codec problem),
/// so the message should say so instead of suggesting settings tweaks.
fn empty_stream_message(source_url: &str) -> String {
    if cobalt::is_youtube_link(source_url) {
        "YouTube blocked extraction for this video (streaming-token \
         enforcement); other videos still work"
            .into()
    } else {
        "the fetch service returned no data for this link; try different \
         quality or codec settings"
            .into()
    }
}

async fn run_fetch_job_inner(
    state: &Arc<AppState>,
    job_id: &str,
    user_id: &str,
    source_url: &str,
    opts: &FetchOptions,
    encrypted_ip: Option<String>,
) -> FetchResult {
    // Storage limits come from juicehost — same ceiling as regular uploads.
    let (max_size, default_ttl_hours) = {
        let guard = state.jh_config.read().unwrap();
        let jh = guard.as_ref().ok_or("storage backend unavailable")?;
        (jh.max_file_size_bytes, jh.default_ttl_hours)
    };

    // Enforcement windows flap open and closed on YouTube's side; keep
    // cycling the full ladder until the budget runs out or something
    // non-rescuable fails. Leave a margin for the final status write.
    let budget = std::time::Duration::from_secs(
        crate::constants::FETCH_JOB_TIMEOUT_SECS.saturating_sub(180),
    );
    let started = std::time::Instant::now();

    let _ = state
        .db_call("update_fetch_job_progress", {
            let job_id = job_id.to_string();
            move |db| db::update_fetch_job_progress(db, &job_id, "cobalt", "processing", 0)
        })
        .await;

    async fn fetch_via(
        state: &Arc<AppState>,
        job_id: &str,
        api_url: &str,
        api_key: &str,
        user_id: &str,
        source_url: &str,
        opts: &FetchOptions,
        max_size: u64,
        default_ttl_hours: f64,
        encrypted_ip: Option<String>,
    ) -> FetchResult {
        let response =
            cobalt::process(&state.http, api_url, api_key, source_url, opts).await?;

        // A session-enabled instance may still refuse at client level; that
        // refusal is final for this attempt and surfaces as a friendly error.
        if response.is_client_refused() {
            return Err(cobalt::friendly_error(
                "error.api.content.video.unavailable",
            ));
        }

        match response {
            CobaltResponse::Error { code } => Err(cobalt::friendly_error(&code)),
            CobaltResponse::Picker { items } => Err(format!(
                "found {} media items at that link; pickers aren't supported yet",
                items.len()
            )),
            CobaltResponse::LocalProcessing { service } => Err(format!(
                "{} requires merging separate streams; try a different container or codec setting",
                service
            )),
            CobaltResponse::Tunnel {
                tunnel_url,
                filename,
            }
            | CobaltResponse::Redirect {
                url: tunnel_url,
                filename,
            } => {
                download_and_store(
                    state,
                    user_id,
                    job_id,
                    source_url,
                    ByteSource::Tunnel(tunnel_url),
                    filename.as_deref(),
                    opts,
                    max_size,
                    default_ttl_hours,
                    encrypted_ip,
                )
                .await
            }
        }
    }

    // Primary first: it serves the vast majority of links.
    let mut primary_result = fetch_via(
        state,
        job_id,
        &state.config.cobalt_api_url,
        &state.config.cobalt_api_key,
        user_id,
        source_url,
        opts,
        max_size,
        default_ttl_hours,
        encrypted_ip.clone(),
    )
    .await;

    // Enforcement windows flap open/closed server-side. Cycle the full
    // ladder — primary -> session -> yt-dlp — every `retry_delay` seconds
    // until something sticks or the budget runs out. Each cycle surfaces a
    // user-visible stage ("retry-N") so the UI isn't a dead spinner.
    let is_rescue = |r: &FetchResult| {
        matches!(r, Err(e) if youtube_needs_rescue(e))
            && cobalt::is_youtube_link(source_url)
    };

    // Secondary server rescues FIRST, with no waiting.
    if is_rescue(&primary_result) {
        if let (Some(su0), Some(sk0)) = (
            &state.config.cobalt_session_api_url,
            &state.config.cobalt_session_api_key,
        ) {
            let _ = state
                .db_call("update_fetch_job_progress", {
                    let job_id = job_id.to_string();
                    move |db| {
                        db::update_fetch_job_progress(
                            db, &job_id, "session-fallback", "processing", 0,
                        )
                    }
                })
                .await;
            let r = fetch_via(
                state,
                job_id,
                su0,
                sk0,
                user_id,
                source_url,
                opts,
                max_size,
                default_ttl_hours,
                encrypted_ip.clone(),
            )
            .await;
            if !matches!(&r, Err(e) if youtube_needs_rescue(e)) {
                return r;
            }
            primary_result = r;
        }
    }

    let mut pass: u32 = 1;
    let mut last = primary_result;
    loop {
        if !matches!(last, Err(ref e) if is_rescue_err(e)) {
            return last;
        }
        let rescuable = matches!(&last, Err(e) if is_rescue_err(e));
        if !rescuable {
            return last;
        }
        pass += 1;
        let budget_left = budget.saturating_sub(started.elapsed());
        // Hard caps keep worst-case latency bounded; delay==0 disables the
        // rescue loop entirely (used by tests).
        if state.config.fetch_empty_retry_delay_secs == 0
            || pass >= crate::constants::FETCH_RESCUE_MAX_PASSES
            || budget_left < std::time::Duration::from_secs(
                state.config.fetch_empty_retry_delay_secs,
            )
        {
            return last;
        }
        tracing::info!(
            "youtube rescue pass {} failed; retrying in {}s",
            pass, state.config.fetch_empty_retry_delay_secs
        );
        let _ = state
            .db_call("update_fetch_job_progress", {
                let job_id = job_id.to_string();
                let stage = format!("retry-{}", pass);
                move |db| db::update_fetch_job_progress(db, &job_id, &stage, "processing", 0)
            })
            .await;
        tokio::time::sleep(std::time::Duration::from_secs(
                state.config.fetch_empty_retry_delay_secs,
            ))
            .await;

        // ---- full ladder for this pass ----
        // Tier 2: session-enabled instance when present; otherwise re-hit
        // the primary (transient googlevideo blocks clear within minutes).
        let (Some(session_url), Some(session_key)) = (
            &state.config.cobalt_session_api_url,
            &state.config.cobalt_session_api_key,
        ) else {
            let retry = fetch_via(
                state,
                job_id,
                &state.config.cobalt_api_url,
                &state.config.cobalt_api_key,
                user_id,
                source_url,
                opts,
                max_size,
                default_ttl_hours,
                encrypted_ip.clone(),
            )
            .await;
            let empty_retry = matches!(&retry, Err(e) if is_empty_stream_error(e));
            if !empty_retry || !cobalt::is_youtube_link(source_url) {
                return retry;
            }
            continue;
        };
        if true {
            let _ = state
                .db_call("update_fetch_job_progress", {
                    let job_id = job_id.to_string();
                    move |db| {
                        db::update_fetch_job_progress(
                            db, &job_id, "session-fallback", "processing", 0,
                        )
                    }
                })
                .await;
            let session_result = fetch_via(
                state,
                job_id,
                session_url,
                session_key,
                user_id,
                source_url,
                opts,
                max_size,
                default_ttl_hours,
                encrypted_ip.clone(),
            )
            .await;

            // Tier 3: yt-dlp + bgutil PO provider via WARP egress.
            if matches!(&session_result, Err(e) if is_rescue_err(e)) && ytdlp_enabled() {
                tracing::info!("cobalt tiers exhausted; falling back to yt-dlp");
                match run_ytdlp_tier(
                    state,
                    job_id,
                    user_id,
                    source_url,
                    opts,
                    max_size,
                    default_ttl_hours,
                    encrypted_ip.clone(),
                )
                .await
                {
                    Ok(r) => return Ok(r),
                    Err(e3) => last = Err(e3),
                }
            } else {
                last = session_result;
            }
        }

        // Non-youtube links never improve across passes.
        if !cobalt::is_youtube_link(source_url) {
            return last;
        }
    }
}

fn is_rescue_err(err: &str) -> bool {
    youtube_needs_rescue(err)
}

/// Internal marker check for the empty-stream failure class, which may be
/// retried against the session-enabled cobalt instance.
fn is_empty_stream_error(err: &str) -> bool {
    err.contains("returned no data")
        || err.contains("blocked extraction")
}

/// YouTube failures worth escalating for: client-level refusals
/// (private/age/region), login walls, and silent empty streams. Each has a
/// rescue path (session instance or yt-dlp tier) even when the primary
/// instance cannot serve the link.
fn youtube_needs_rescue(err: &str) -> bool {
    err.contains("unavailable")
        || err.contains("login")
        || is_empty_stream_error(err)
}

/// Tier-3 fallback gate: yt-dlp + bgutil PO provider, routed through the
/// same residential egress as cobalt. Enabled via YTDLP_FALLBACK=1.
fn ytdlp_enabled() -> bool {
    std::env::var("YTDLP_FALLBACK").as_deref() == Ok("1")
}

fn ytdlp_bin() -> String {
    std::env::var("YTDLP_BIN").unwrap_or_else(|_| "yt-dlp".into())
}

fn ytdlp_proxy() -> String {
    std::env::var("YTDLP_PROXY").unwrap_or_default()
}

fn ytdlp_pot_base_url() -> String {
    std::env::var("YTDLP_POT_BASE_URL").unwrap_or_default()
}

/// Secondary egress tried first by the yt-dlp tier when set (e.g. the
/// home-relay tinyproxy); WARP remains as YTDLP_PROXY fallback.
fn ytdlp_proxy_secondary() -> String {
    std::env::var("YTDLP_PROXY_SECONDARY").unwrap_or_default()
}

fn ytdlp_tmpdir() -> String {
    std::env::var("YTDLP_TMPDIR").unwrap_or_else(|_| "/var/lib/juicebox/ytdlp-tmp".into())
}

/// yt-dlp's extractor needs a JS runtime (EJS); deno preferred, node ok.
fn ytdlp_cookies_path() -> String {
    std::env::var("YTDLP_COOKIES").unwrap_or_default()
}

fn ytdlp_js_runtime() -> String {
    std::env::var("YTDLP_JS_RUNTIME").unwrap_or_else(|_| "deno".into())
}

/// Pure arg builder for the yt-dlp fallback invocation.
pub fn build_ytdlp_args_for_test(
    proxy: &str,
    pot_base_url: &str,
    js_runtime: &str,
    cookies_path: &str,
    url: &str,
    out_dir: &str,
) -> Vec<String> {
    build_ytdlp_args(proxy, pot_base_url, js_runtime, cookies_path, url, out_dir)
}

fn build_ytdlp_args(
    proxy: &str,
    pot_base_url: &str,
    js_runtime: &str,
    cookies_path: &str,
    url: &str,
    out_dir: &str,
) -> Vec<String> {
    let mut args = vec![
        "--proxy".to_string(),
        proxy.to_string(),
        "--extractor-args".to_string(),
        format!("youtubepot-bgutilhttp:base_url={pot_base_url}"),
        "-f".to_string(),
        "bv*+ba/b".to_string(),
        "--merge-output-format".to_string(),
        "mp4".to_string(),
        "--no-playlist".to_string(),
        "--no-progress".to_string(),
        "-q".to_string(),
        "--js-runtimes".to_string(),
        js_runtime.to_string(),
        "--remote-components".to_string(),
        "ejs:github".to_string(),
        "--cookies".to_string(),
        cookies_path.to_string(),
        "-o".to_string(),
        format!("{out_dir}/%(title).180B-%(id)s.%(ext)s"),
        url.to_string(),
    ];
    args.retain(|a| !a.is_empty());
    args
}

/// Tier-3: run yt-dlp through the residential egress + bgutil PO provider,
/// then store the produced file exactly like a tunnel download.
#[allow(clippy::too_many_arguments)]
async fn run_ytdlp_tier(
    state: &Arc<AppState>,
    job_id: &str,
    user_id: &str,
    source_url: &str,
    opts: &FetchOptions,
    max_size: u64,
    default_ttl_hours: f64,
    encrypted_ip: Option<String>,
) -> FetchResult {
    let _ = state
        .db_call("update_fetch_job_progress", {
            let job_id = job_id.to_string();
            move |db| db::update_fetch_job_progress(db, &job_id, "yt-dlp", "processing", 0)
        })
        .await;

    let out_dir = ytdlp_tmpdir();
    tokio::fs::create_dir_all(&out_dir)
        .await
        .map_err(|e| format!("yt-dlp tmpdir unavailable: {}", e))?;

    // Egress candidates, in order: secondary server (home relay) first,
    // then the Cloudflare WARP namespace as fallback.
    let mut proxies: Vec<String> = Vec::new();
    for p in [ytdlp_proxy_secondary(), ytdlp_proxy()] {
        if !p.is_empty() && !proxies.contains(&p) {
            proxies.push(p);
        }
    }
    if proxies.is_empty() {
        return Err("yt-dlp fallback has no proxy configured".into());
    }

    // Output template is id-based; locate the produced artifact per attempt.
    let video_id = cobalt::parse_youtube_video_id(source_url)
        .ok_or("could not determine youtube video id for yt-dlp fallback")?;

    let mut last_err: Option<String> = None;
    let mut produced: Option<PathBuf> = None;

    'proxies: for (idx, proxy) in proxies.iter().enumerate() {
        let stage_tag = if idx == 0 { "yt-dlp" } else { "yt-dlp-warp" };
        let _ = state
            .db_call("update_fetch_job_progress", {
                let job_id = job_id.to_string();
                let stage = stage_tag.to_string();
                move |db| db::update_fetch_job_progress(db, &job_id, &stage, "processing", 0)
            })
            .await;

        // Clear stale artifacts for this video before each attempt so a
        // previous proxy's failure can't masquerade as success.
        let mut rd = match tokio::fs::read_dir(&out_dir).await {
            Ok(rd) => rd,
            Err(e) => return Err(format!("yt-dlp tmpdir unreadable: {}", e)),
        };
        while let Some(entry) = rd.next_entry().await.map_err(|e| e.to_string())? {
            let p = entry.path();
            if p.file_stem().and_then(|s| s.to_str()) == Some(video_id.as_str()) {
                let _ = tokio::fs::remove_file(&p).await;
            }
        }

        let args = build_ytdlp_args(
            proxy,
            &ytdlp_pot_base_url(),
            &ytdlp_js_runtime(),
            &ytdlp_cookies_path(),
            source_url,
            &out_dir,
        );

        tracing::info!("tier-3 pass {} via {} ({}): running {}", idx + 1, proxy, stage_tag, ytdlp_bin());
        let output = match tokio::time::timeout(
            std::time::Duration::from_secs(crate::constants::FETCH_JOB_TIMEOUT_SECS),
            tokio::process::Command::new(ytdlp_bin())
                .args(&args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .output(),
        )
        .await
        {
            Ok(r) => r.map_err(|e| format!("failed to spawn {}: {}", ytdlp_bin(), e))?,
            Err(_) => {
                last_err = Some("yt-dlp fallback timed out".into());
                continue;
            }
        };

        if !output.status.success() {
            let tail = String::from_utf8_lossy(&output.stderr);
            let tail = tail.lines().rev().take(3).collect::<Vec<&str>>().iter().rev().copied().collect::<Vec<_>>().join(" | ");
            last_err = Some(format!("yt-dlp fallback failed via {}: {}", proxy, tail));
            continue;
        }

        let mut rd = match tokio::fs::read_dir(&out_dir).await {
            Ok(rd) => rd,
            Err(e) => {
                last_err = Some(format!("yt-dlp tmpdir unreadable: {}", e));
                continue;
            }
        };
        while let Some(entry) = rd.next_entry().await.map_err(|e| e.to_string())? {
            let p = entry.path();
            if p.file_stem().and_then(|s| s.to_str()) == Some(video_id.as_str()) {
                produced = Some(p);
                break;
            }
        }
        if produced.is_some() {
            break 'proxies;
        }

        let tail = String::from_utf8_lossy(&output.stderr);
        let tail = tail.lines().rev().take(2).collect::<Vec<&str>>().join(" | ");
        last_err = Some(if tail.is_empty() {
            format!(
                "yt-dlp extracted no downloadable streams via {} (SABR enforcement)",
                proxy
            )
        } else {
            format!("yt-dlp extracted no streams via {} | {}", proxy, tail)
        });
    }

    let path = match produced {
        Some(p) => p,
        None => return Err(last_err.unwrap_or_else(|| {
            "yt-dlp extracted no downloadable streams (SABR enforcement)".into()
        })),
    };

    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "video.mp4".into());


    let result = download_and_store(
        state,
        user_id,
        job_id,
        source_url,
        ByteSource::LocalFile(path.clone()),
        Some(&name),
        opts,
        max_size,
        default_ttl_hours,
        encrypted_ip,
    )
    .await;

    // Temp artifact is either stored or failed; either way it must not linger.
    let _ = tokio::fs::remove_file(&path).await;
    result
}

/// Sanitize cobalt's suggested filename and make sure it has an extension.
fn sanitize_filename(name: Option<&str>, audio_only: bool) -> String {
    let fallback = if audio_only { "audio.mp3" } else { "video.mp4" };
    let mut name = name.unwrap_or(fallback);
    name = name.rsplit(['/', '\\']).next().unwrap_or(fallback);
    let mut name: String = name
        .chars()
        .filter(|c| !c.is_control())
        .take(crate::constants::MAX_FILENAME_LEN)
        .collect();
    let trimmed = name.trim().to_string();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        return fallback.to_string();
    }
    name = trimmed;
    if !name.contains('.') {
        name.push('.');
        name.push_str(if audio_only { "mp3" } else { "mp4" });
    }
    name
}

#[allow(clippy::too_many_arguments)]
async fn download_and_store(
    state: &Arc<AppState>,
    user_id: &str,
    job_id: &str,
    source_url: &str,
    byte_source: ByteSource,
    filename: Option<&str>,
    opts: &FetchOptions,
    max_size: u64,
    ttl_hours: f64,
    encrypted_ip: Option<String>,
) -> FetchResult {
    let filename = sanitize_filename(filename, opts.audio_only);
    let fallback_mime: &str = if opts.audio_only {
        "audio/mpeg"
    } else {
        "video/mp4"
    };
    let mime_type = mime_guess::from_path(&filename)
        .first()
        .map(|m| m.to_string())
        .unwrap_or_else(|| fallback_mime.to_string());

    let file_id = nanoid::nanoid!(8);
    let delete_token = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp();
    let expires_at = now + (ttl_hours * crate::constants::SECONDS_PER_HOUR_F64).round() as i64;

    let record = FileRecord::new(
        file_id.clone(),
        filename.clone(),
        mime_type.clone(),
        0,
        delete_token.clone(),
        now,
        expires_at,
        encrypted_ip,
        None,
    );

    // Reserve the row as 'uploading' so nothing serves it mid-transfer.
    if let Err(e) = db::insert_pending_file(state, record.clone()).await {
        return Err(format!("failed to reserve file slot: {:?}", e));
    }

    let mut cleanup = CleanupOnDrop {
        state: Arc::clone(state),
        file_id: file_id.clone(),
        delete_token: delete_token.clone(),
        active: true,
    };

    let result = transfer_to_juicehost(
        state,
        job_id,
        &file_id,
        &filename,
        &mime_type,
        &delete_token,
        source_url,
        &byte_source,
        max_size,
    )
    .await;

    match result {
        Ok(size_bytes) => {
            cleanup.active = false;
            finalize_stored_file(
                state,
                user_id,
                file_id,
                filename,
                mime_type,
                size_bytes,
                delete_token,
            )
            .await
        }
        Err(msg) => Err(msg),
    }
}

struct CleanupOnDrop {
    state: Arc<AppState>,
    file_id: String,
    delete_token: String,
    active: bool,
}

impl Drop for CleanupOnDrop {
    fn drop(&mut self) {
        if self.active {
            let state = Arc::clone(&self.state);
            let file_id = self.file_id.clone();
            let token = self.delete_token.clone();
            tokio::spawn(async move {
                let _ = db::delete_pending_file(&state, file_id.clone()).await;
                let _ = crate::juicehost::delete_file_on_juicehost(
                    &state,
                    &file_id,
                    None,
                    Some(&token),
                )
                .await;
            });
        }
    }
}

/// Stream the tunnel bytes into juicehost while enforcing the size cap.
/// Reports live progress (`downloading` + byte count) back to the job row.
async fn transfer_to_juicehost(
    state: &Arc<AppState>,
    job_id: &str,
    file_id: &str,
    filename: &str,
    mime_type: &str,
    capability: &str,
    source_url: &str,
    byte_source: &ByteSource,
    max_size: u64,
) -> Result<u64, String> {
    // Unified byte stream: cobalt tunnel HTTP response or a local yt-dlp
    // output file. Both flow through the same size-capped push pipeline.
    use tokio::io::AsyncReadExt as _;
    let (mut stream, _precheck_len): (
        std::pin::Pin<Box<dyn futures::Stream<Item = Result<bytes::Bytes, String>> + Send>>,
        Option<u64>,
    ) = match &byte_source {
        ByteSource::Tunnel(url) => {
            let client = cobalt::tunnel_client()
                .map_err(|e| format!("failed to build download client: {}", e))?;
            let resp = client
                .get(url)
                .timeout(std::time::Duration::from_secs(
                    crate::constants::FETCH_JOB_TIMEOUT_SECS,
                ))
                .send()
                .await
                .map_err(|e| format!("media download failed: {}", e))?;
            if !resp.status().is_success() {
                return Err(format!(
                    "media download failed: server returned status {}",
                    resp.status()
                ));
            }
            let len = resp.content_length().map(|l| l as u64);
            if len == Some(0) {
                return Err(empty_stream_message(source_url));
            }
            if let Some(l) = len {
                if l > max_size {
                    return Err("file exceeds this server's maximum file size".into());
                }
            }
            (
                Box::pin(resp.bytes_stream().map(|c| {
                    c.map_err(|e| format!("media download failed mid-stream: {}", e))
                })),
                len,
            )
        }
        ByteSource::LocalFile(path) => {
            let meta = tokio::fs::metadata(path)
                .await
                .map_err(|e| format!("yt-dlp output missing: {}", e))?;
            let size = meta.len();
            if size == 0 {
                return Err(empty_stream_message(source_url));
            }
            if size > max_size {
                return Err("file exceeds this server's maximum file size".into());
            }
            let file = tokio::fs::File::open(path)
                .await
                .map_err(|e| format!("failed to open yt-dlp output: {}", e))?;
            (
                Box::pin(futures::stream::unfold(file, |mut file| async move {
                    let mut buf = bytes::BytesMut::with_capacity(256 * 1024);
                    match file.read_buf(&mut buf).await {
                        Ok(0) => None,
                        Ok(_) => Some((Ok(buf.freeze()), file)),
                        Err(e) => Some((
                            Err(format!("failed reading yt-dlp output: {}", e)),
                            file,
                        )),
                    }
                })),
                None,
            )
        }
    };

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, String>>(
        crate::constants::STREAM_CHANNEL_CAPACITY,
    );
    let push_state = Arc::clone(state);
    let push_file_id = file_id.to_string();
    let push_filename = filename.to_string();
    let push_mime = mime_type.to_string();
    let push_capability = capability.to_string();
    let push_task = tokio::spawn(async move {
        crate::juicehost::push_file_streaming(
            &push_state,
            &push_file_id,
            &push_filename,
            &push_mime,
            rx,
            None,
            &UploadMode::Standard,
            Some(&push_capability),
        )
        .await
    });

    let mut total: u64 = 0;
    let mut last_progress = std::time::Instant::now() - std::time::Duration::from_secs(1);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("media download failed mid-stream: {}", e))?;
        total += chunk.len() as u64;
        if total > max_size {
            return Err("file exceeds this server's maximum file size".into());
        }
        // Throttled live progress; losing a tick must never fail the transfer.
        if last_progress.elapsed() >= std::time::Duration::from_millis(750) {
            last_progress = std::time::Instant::now();
            let job_id_owned = job_id.to_string();
            let _ = state
                .db_call(
                    "update_fetch_job_progress",
                    move |db| {
                        db::update_fetch_job_progress(db, &job_id_owned, "transfer", "downloading", total as i64)
                    },
                )
                .await;
        }
        if tx.send(Ok(chunk)).await.is_err() {
            return Err("storage backend rejected the transfer".into());
        }
    }

    // Final accurate byte count before completion flips the status.
    {
        let job_id_owned = job_id.to_string();
        let _ = state
            .db_call(
                "update_fetch_job_progress_final",
                move |db| {
                    db::update_fetch_job_progress(db, &job_id_owned, "transfer", "downloading", total as i64)
                },
            )
            .await;
    }

    // The source can answer 200 with zero bytes (some youtube videos do this
    // with certain quality/codec picks). Don't hand an empty file to juicehost.
    if total == 0 {
        push_task.abort();
        return Err(empty_stream_message(source_url));
    }
    drop(tx);

    push_task
        .await
        .map_err(|_| "storage task panicked".to_string())??;

    Ok(total)
}

/// Mark the reserved file ready, mirror ownership, hand back the final file id.
#[allow(clippy::too_many_arguments)]
async fn finalize_stored_file(
    state: &Arc<AppState>,
    user_id: &str,
    file_id: String,
    filename: String,
    mime_type: String,
    size_bytes: u64,
    delete_token: String,
) -> FetchResult {
    let completed = state
        .db_call("complete_fetch_reservation", move |db| {
            db::complete_reservation(
                db,
                &filename,
                &mime_type,
                size_bytes as i64,
                &file_id,
                &delete_token,
                None,
            )
        })
        .await
        .map_err(|e| format!("failed to finalize fetched file: {:?}", e))?;

    match completed {
        db::CompleteReservationResult::Completed(record) => {
            let owned = record.clone();
            let owner = user_id.to_string();
            if let Err(e) = state
                .db_call("own_fetched_file", move |db| {
                    db::add_client_file(db, &owner, &owned)
                })
                .await
            {
                tracing::warn!("fetch: could not register ownership: {:?}", e);
            }
            Ok(record.id)
        }
        _ => {
            // Shouldn't happen: nobody else knows this reservation's token.
            Err("reservation vanished during finalization".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_accepts_public_https() {
        assert!(validate_source_url("https://youtu.be/dQw4w9WgXcQ").is_ok());
        assert!(validate_source_url("http://example.com/video?v=1").is_ok());
    }

    #[test]
    fn validate_rejects_bad_urls() {
        assert!(validate_source_url("").is_err());
        assert!(validate_source_url("not a url").is_err());
        assert!(validate_source_url("ftp://example.com/x").is_err());
        assert!(validate_source_url("javascript:alert(1)").is_err());
        assert!(validate_source_url("https://u:p@example.com/").is_err());
        assert!(validate_source_url("https://localhost/video").is_err());
        assert!(validate_source_url("https://metadata.google.internal/").is_err());
        assert!(validate_source_url("http://127.0.0.1:7272/session").is_err());
        assert!(validate_source_url("http://192.168.1.10/x").is_err());
        assert!(validate_source_url("http://169.254.169.254/latest/meta-data").is_err());
        assert!(validate_source_url("http://[::1]/x").is_err());
        let long = format!("https://example.com/{}", "a".repeat(3000));
        assert!(validate_source_url(&long).is_err());
    }

    #[test]
    fn sanitize_filenames() {
        assert_eq!(sanitize_filename(None, false), "video.mp4");
        assert_eq!(sanitize_filename(None, true), "audio.mp3");
        assert_eq!(sanitize_filename(Some("../evil.mp4"), false), "evil.mp4");
        assert_eq!(sanitize_filename(Some("noext"), false), "noext.mp4");
        assert_eq!(sanitize_filename(Some("clip.webm"), false), "clip.webm");
        assert_eq!(sanitize_filename(Some(""), true), "audio.mp3");
        assert_eq!(sanitize_filename(Some(".."), false), "video.mp4");
    }

    #[test]
    fn empty_stream_message_is_youtube_aware() {
        let yt = empty_stream_message("https://youtu.be/Wfv4Uj-xBT4");
        assert!(yt.contains("YouTube blocked extraction"));
        assert!(!yt.contains("codec"));

        let other = empty_stream_message("https://vimeo.com/12345");
        assert!(other.contains("quality or codec"));
    }
}
