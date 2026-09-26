use std::sync::Arc;

use super::{
    store::download_and_store,
    types::{ByteSource, FetchResult},
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

pub(crate) async fn run_fetch_job_inner(
    state: &Arc<AppState>,
    job_id: &str,
    user_id: &str,
    source_url: &str,
    opts: &FetchOptions,
    encrypted_ip: Option<String>,
) -> FetchResult {
    let (max_size, default_ttl_hours) = {
        let jh = state.juicehost_config().map_err(|e| format!("{e:?}"))?;
        (jh.max_file_size_bytes, jh.default_ttl_hours)
    };

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

    let is_rescue = |r: &FetchResult| {
        matches!(r, Err(e) if youtube_needs_rescue(e)) && cobalt::is_youtube_link(source_url)
    };

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
                            db,
                            &job_id,
                            "session-fallback",
                            "processing",
                            0,
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
        pass += 1;
        let budget_left = budget.saturating_sub(started.elapsed());

        if state.config.fetch_empty_retry_delay_secs == 0
            || pass >= crate::constants::FETCH_RESCUE_MAX_PASSES
            || budget_left
                < std::time::Duration::from_secs(state.config.fetch_empty_retry_delay_secs)
        {
            return last;
        }
        let retry_delay = state.config.fetch_empty_retry_delay_secs;
        tracing::info!("youtube rescue pass {pass} failed; retrying in {retry_delay}s");
        let _ = state
            .db_call("update_fetch_job_progress", {
                let job_id = job_id.to_string();
                let stage = format!("retry-{pass}");
                move |db| db::update_fetch_job_progress(db, &job_id, &stage, "processing", 0)
            })
            .await;
        tokio::time::sleep(std::time::Duration::from_secs(
            state.config.fetch_empty_retry_delay_secs,
        ))
        .await;

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
        let _ = state
            .db_call("update_fetch_job_progress", {
                let job_id = job_id.to_string();
                move |db| {
                    db::update_fetch_job_progress(db, &job_id, "session-fallback", "processing", 0)
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

        if !cobalt::is_youtube_link(source_url) {
            return last;
        }
    }
}

pub(crate) async fn fetch_via(
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
    let response = cobalt::process(&state.http, api_url, api_key, source_url, opts).await?;

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
            "{service} requires merging separate streams; try a different container or codec setting"
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

fn is_rescue_err(err: &str) -> bool {
    youtube_needs_rescue(err)
}

#[must_use]
pub(crate) fn is_empty_stream_error(err: &str) -> bool {
    err.contains("returned no data") || err.contains("blocked extraction")
}

#[must_use]
pub(crate) fn youtube_needs_rescue(err: &str) -> bool {
    err.contains("unavailable") || err.contains("login") || is_empty_stream_error(err)
}
