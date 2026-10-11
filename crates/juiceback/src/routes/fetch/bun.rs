use std::sync::Arc;

use super::{
    store::download_and_store,
    types::{ByteSource, FetchResult},
};
use crate::{
    cobalt::FetchOptions,
    db,
    state::AppState,
};

/// Tier gate: the Bun SABR-fetch sidecar (BUN_FETCH_URL + BUN_FETCH_KEY).
/// Best-effort middle rung — every failure falls through to yt-dlp.
pub(crate) fn bun_fetch_configured(state: &AppState) -> bool {
    state.config.bun_fetch_url.is_some() && state.config.bun_fetch_key.is_some()
}

/// Map the requested cobalt quality cap to a sidecar height cap.
/// "max" (and anything unparseable) means uncapped (0).
fn bun_max_height(video_quality: &str) -> u32 {
    if video_quality == "max" {
        return 0;
    }
    video_quality.parse::<u32>().ok().filter(|h| *h > 0).unwrap_or(0)
}

/// Tier: POST the URL to the Bun SABR-fetch sidecar and store the
/// returned file bytes exactly like a tunnel download. The sidecar runs
/// yt-dlp with Bun JS challenges + per-video PO tokens — ideally on the
/// residential secondary box, where no proxy hop is needed at all.
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
    let (Some(fetch_url), Some(api_key)) = (
        &state.config.bun_fetch_url,
        &state.config.bun_fetch_key,
    ) else {
        return Err("bun-fetch tier not configured".into());
    };

    let _ = state
        .db_call("update_fetch_job_progress", {
            let job_id = job_id.to_string();
            move |db| db::update_fetch_job_progress(db, &job_id, "bun-fetch", "processing", 0)
        })
        .await;

    tracing::info!("tier via bun-fetch ({}): fetching {}", fetch_url, source_url);
    download_and_store(
        state,
        user_id,
        job_id,
        source_url,
        ByteSource::BunService {
            fetch_url: fetch_url.clone(),
            api_key: api_key.clone(),
            payload_url: source_url.to_string(),
            audio_only: opts.audio_only,
            max_height: bun_max_height(&opts.video_quality),
        },
        None,
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
