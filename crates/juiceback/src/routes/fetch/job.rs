use std::sync::Arc;

use super::{
    bun::{bun_fetch_configured, run_bun_tier},
    meta,
    store::download_and_store,
    types::{ByteSource, FetchMeta, FetchResult},
    ytdlp::{run_ytdlp_tier, ytdlp_enabled},
};
use crate::{
    cobalt::{self, CobaltResponse, FetchOptions},
    db,
    state::AppState,
};

pub(crate) async fn run_fetch_job(
    state: Arc<AppState>,
    job_id: String,
    user_id: String,
    source_url: String,
    opts: FetchOptions,
    encrypted_ip: Option<String>,
    password: Option<String>,
) {
    let timeout = std::time::Duration::from_secs(crate::constants::FETCH_JOB_TIMEOUT_SECS);
    match tokio::time::timeout(
        timeout,
        run_fetch_job_inner(
            &state,
            &job_id,
            &user_id,
            &source_url,
            &opts,
            encrypted_ip,
            password,
        ),
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
            tracing::info!("fetch job {job_id} done: {file_id}");
        }
        Ok(Err(msg)) => fail_fetch_job(&state, &job_id, &msg).await,
        Err(_) => fail_fetch_job(&state, &job_id, "fetch timed out").await,
    }
}

async fn fail_fetch_job(state: &Arc<AppState>, job_id: &str, msg: &str) {
    tracing::warn!("fetch job {job_id} failed: {msg}");
    let job_id_owned = job_id.to_string();
    let msg_owned = msg.to_string();
    let _ = state
        .db_call("finish_fetch_job_failed", move |db| {
            db::finish_fetch_job(db, &job_id_owned, "failed", &msg_owned, "").map(|_: bool| ())
        })
        .await;
}

/// Bounded retries, then bun-fetch, then yt-dlp, in that order.
pub(crate) async fn run_fetch_job_inner(
    state: &Arc<AppState>,
    job_id: &str,
    user_id: &str,
    source_url: &str,
    opts: &FetchOptions,
    encrypted_ip: Option<String>,
    password: Option<String>,
) -> FetchResult {
    let (max_size, default_ttl_hours) = {
        let jh = state
            .juicehost_config_or_refresh()
            .await
            .map_err(|e| format!("{e:?}"))?;
        (jh.max_file_size_bytes, jh.default_ttl_hours)
    };

    // Reserve the tail of the timeout for transfer and finalize; rescue retries get
    // the rest.
    let budget = std::time::Duration::from_secs(
        crate::constants::FETCH_JOB_TIMEOUT_SECS.saturating_sub(180),
    );
    let started = std::time::Instant::now();

    let _ = state
        .db_call("update_fetch_job_progress", {
            let job_id = job_id.to_string();
            move |db| db::update_fetch_job_progress(db, &job_id, "cobalt", "processing", 0, 0)
        })
        .await;

    let mut last = fetch_via(
        state,
        job_id,
        user_id,
        source_url,
        opts,
        max_size,
        default_ttl_hours,
        encrypted_ip.clone(),
        password.clone(),
    )
    .await;

    let mut pass: u32 = 1;
    while matches!(&last, Err(e) if is_empty_stream_error(e)) {
        pass += 1;
        let budget_left = budget.saturating_sub(started.elapsed());

        if state.config.fetch_empty_retry_delay_secs == 0
            || pass >= crate::constants::FETCH_RESCUE_MAX_PASSES
            || budget_left
                < std::time::Duration::from_secs(state.config.fetch_empty_retry_delay_secs)
        {
            break;
        }
        let retry_delay = state.config.fetch_empty_retry_delay_secs;
        tracing::info!("youtube rescue pass {pass} failed; retrying in {retry_delay}s");
        let _ = state
            .db_call("update_fetch_job_progress", {
                let job_id = job_id.to_string();
                let stage = format!("retry-{pass}");
                move |db| db::update_fetch_job_progress(db, &job_id, &stage, "processing", 0, 0)
            })
            .await;
        tokio::time::sleep(std::time::Duration::from_secs(
            state.config.fetch_empty_retry_delay_secs,
        ))
        .await;

        last = fetch_via(
            state,
            job_id,
            user_id,
            source_url,
            opts,
            max_size,
            default_ttl_hours,
            encrypted_ip.clone(),
            password.clone(),
        )
        .await;
    }

    if !matches!(&last, Err(e) if is_rescue_err(e)) {
        return last;
    }

    if bun_fetch_configured(state) {
        tracing::info!("cobalt tiers exhausted; falling back to bun-fetch");
        match run_bun_tier(
            state,
            job_id,
            user_id,
            source_url,
            opts,
            max_size,
            default_ttl_hours,
            encrypted_ip.clone(),
            password.clone(),
        )
        .await
        {
            Ok(r) => return Ok(r),
            Err(e3) => {
                tracing::warn!("bun-fetch tier failed, trying local yt-dlp: {e3}");
                last = Err(e3);
            }
        }
    }
    if ytdlp_enabled() {
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
            password.clone(),
        )
        .await
        {
            Ok(r) => return Ok(r),
            Err(e3) => last = Err(e3),
        }
    }

    last
}

/// An empty-stream instance is excluded and the race restarts against the rest.
pub(crate) async fn fetch_via(
    state: &Arc<AppState>,
    job_id: &str,
    user_id: &str,
    source_url: &str,
    opts: &FetchOptions,
    max_size: u64,
    default_ttl_hours: f64,
    encrypted_ip: Option<String>,
    password: Option<String>,
) -> FetchResult {
    let endpoints = &state.config.cobalt_endpoints;
    let mut excluded: Vec<usize> = Vec::new();
    loop {
        let (response, winner) =
            cobalt::process_race(&state.http, endpoints, source_url, opts, &excluded)
                .await
                .map_err(|err| cobalt::instance_failure_from_error(&err))?;
        tracing::debug!(
            "fetch job {job_id}: cobalt instance {} answered first",
            endpoints.get(winner).map(|e| e.url.as_str()).unwrap_or("?")
        );

        if response.is_client_refused() {
            return Err(cobalt::friendly_error(
                "error.api.content.video.unavailable",
            ));
        }

        let result = match response {
            CobaltResponse::Error { code } => {
                if cobalt::is_instance_error_code(&code) {
                    tracing::warn!(
                        "fetch job {job_id}: cobalt instance {} failed for this fetch; trying another tier",
                        endpoints.get(winner).map(|e| e.url.as_str()).unwrap_or("?")
                    );
                    return Err(cobalt::instance_failure_message(&code));
                }
                return Err(cobalt::friendly_error(&code));
            }
            CobaltResponse::Picker { items } => {
                return Err(format!(
                    "found {} media items at that link; pickers aren't supported yet",
                    items.len()
                ));
            }
            CobaltResponse::LocalProcessing { service } => {
                return Err(format!(
                    "{service} requires merging separate streams; try a different container or codec setting"
                ));
            }
            CobaltResponse::Tunnel {
                tunnel_url,
                filename,
            }
            | CobaltResponse::Redirect {
                url: tunnel_url,
                filename,
            } => {
                let meta = FetchMeta {
                    title: None,
                    author: filename
                        .as_deref()
                        .and_then(meta::author_from_cobalt_filename),
                };
                download_and_store(
                    state,
                    user_id,
                    job_id,
                    source_url,
                    ByteSource::Tunnel(tunnel_url),
                    filename.as_deref(),
                    meta,
                    opts,
                    max_size,
                    default_ttl_hours,
                    encrypted_ip.clone(),
                    password.clone(),
                )
                .await
            }
        };

        match result {
            Err(err) if is_empty_stream_error(&err) && excluded.len() + 1 < endpoints.len() => {
                tracing::info!(
                    "fetch job {job_id}: cobalt instance {} returned an empty stream; trying another instance",
                    endpoints.get(winner).map(|e| e.url.as_str()).unwrap_or("?")
                );
                excluded.push(winner);
                let _ = state
                    .db_call("update_fetch_job_progress", {
                        let job_id = job_id.to_string();
                        move |db| {
                            db::update_fetch_job_progress(
                                db,
                                &job_id,
                                "session-fallback",
                                "processing",
                                0,
                                0,
                            )
                        }
                    })
                    .await;
            }
            other => return other,
        }
    }
}

fn is_rescue_err(err: &str) -> bool {
    is_empty_stream_error(err) || cobalt::is_instance_failure_error(err)
}

/// Matches on message text; changing the wording silently disables retries.
#[must_use]
pub(crate) fn is_empty_stream_error(err: &str) -> bool {
    err.contains("returned no data") || err.contains("blocked extraction")
}
