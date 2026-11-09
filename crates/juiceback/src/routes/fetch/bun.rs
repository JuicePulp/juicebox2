use std::{sync::Arc, time::Duration};

use super::{
    meta::{decoded_header_value, title_from_bun_filename, youtube_oembed_metadata},
    store::download_and_store,
    types::{ByteSource, FetchResult},
};
use crate::{
    cobalt::{self, FetchOptions},
    db,
    state::AppState,
};

pub(crate) fn bun_fetch_configured(state: &AppState) -> bool {
    state.config.bun_fetch_url.is_some() && state.config.bun_fetch_key.is_some()
}

fn bun_max_height(video_quality: &str) -> u32 {
    if video_quality == "max" {
        return 0;
    }
    video_quality
        .parse::<u32>()
        .ok()
        .filter(|h| *h > 0)
        .unwrap_or(0)
}

#[expect(
    clippy::too_many_arguments,
    reason = "pipeline fns thread established context (state, ids, tokens); bundling params churns callers for no behavior gain"
)]
pub(crate) async fn run_bun_tier(
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
    let (Some(fetch_url), Some(api_key)) =
        (&state.config.bun_fetch_url, &state.config.bun_fetch_key)
    else {
        return Err("bun-fetch tier not configured".into());
    };

    let _ = state
        .db_call("update_fetch_job_progress", {
            let job_id = job_id.to_string();
            move |db| db::update_fetch_job_progress(db, &job_id, "bun-fetch", "processing", 0, 0)
        })
        .await;

    tracing::info!(
        "tier via bun-fetch ({}): fetching {}",
        fetch_url,
        source_url
    );

    let resp = state
        .http
        .post(fetch_url.as_str())
        .bearer_auth(api_key.as_str())
        .timeout(Duration::from_secs(
            crate::constants::FETCH_JOB_TIMEOUT_SECS,
        ))
        .json(&serde_json::json!({
            "url": source_url,
            "audio_only": opts.audio_only,
            "max_height": bun_max_height(&opts.video_quality),
        }))
        .send()
        .await
        .map_err(|e| format!("bun-fetch request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("bun-fetch returned status {}", resp.status()));
    }

    let filename = resp
        .headers()
        .get("x-bun-fetch-filename")
        .and_then(|v| v.to_str().ok())
        .map(decoded_header_value);

    let mut meta = youtube_oembed_metadata(&state.http, source_url).await;
    if meta.title.is_none() {
        let video_id = cobalt::parse_youtube_video_id(source_url);
        meta.title = filename
            .as_deref()
            .and_then(|name| title_from_bun_filename(name, video_id.as_deref()));
    }

    download_and_store(
        state,
        user_id,
        job_id,
        source_url,
        ByteSource::BunResponse(resp),
        filename.as_deref(),
        meta,
        opts,
        max_size,
        default_ttl_hours,
        encrypted_ip,
        password,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_height_max_means_uncapped() {
        assert_eq!(bun_max_height("max"), 0);
    }

    #[test]
    fn max_height_parses_caps() {
        assert_eq!(bun_max_height("1080"), 1080);
        assert_eq!(bun_max_height("0"), 0);
        assert_eq!(bun_max_height("bogus"), 0);
    }
}
