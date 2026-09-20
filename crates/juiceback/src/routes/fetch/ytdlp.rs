use std::{path::PathBuf, sync::Arc};

use super::{
    store::download_and_store,
    types::{ByteSource, FetchResult},
};
use crate::{
    cobalt::{self, FetchOptions},
    db,
    state::AppState,
};

/// Tier-3 fallback gate: yt-dlp + bgutil PO provider, routed through the
/// same residential egress as cobalt. Enabled via `YTDLP_FALLBACK=1`.
pub(crate) fn ytdlp_enabled() -> bool {
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
/// home-relay tinyproxy); WARP remains as `YTDLP_PROXY` fallback.
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
#[must_use]
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
#[expect(
    clippy::too_many_arguments,
    reason = "pipeline fns thread established context (state, ids, tokens); bundling params churns callers for no behavior gain"
)]
pub(crate) async fn run_ytdlp_tier(
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
        .map_err(|e| format!("yt-dlp tmpdir unavailable: {e}"))?;

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
            Err(e) => return Err(format!("yt-dlp tmpdir unreadable: {e}")),
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

        tracing::info!(
            "tier-3 pass {} via {} ({}): running {}",
            idx + 1,
            proxy,
            stage_tag,
            ytdlp_bin()
        );
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
            let mut tail: Vec<&str> = tail.lines().rev().take(3).collect();
            tail.reverse();
            let tail = tail.join(" | ");
            last_err = Some(format!("yt-dlp fallback failed via {proxy}: {tail}"));
            continue;
        }

        let mut rd = match tokio::fs::read_dir(&out_dir).await {
            Ok(rd) => rd,
            Err(e) => {
                last_err = Some(format!("yt-dlp tmpdir unreadable: {e}"));
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
        let tail = tail
            .lines()
            .rev()
            .take(2)
            .collect::<Vec<&str>>()
            .join(" | ");
        last_err = Some(if tail.is_empty() {
            format!("yt-dlp extracted no downloadable streams via {proxy} (SABR enforcement)")
        } else {
            format!("yt-dlp extracted no streams via {proxy} | {tail}")
        });
    }

    let path = match produced {
        Some(p) => p,
        None => {
            return Err(last_err.unwrap_or_else(|| {
                "yt-dlp extracted no downloadable streams (SABR enforcement)".into()
            }));
        }
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
