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

/// Off unless `YTDLP_FALLBACK=1`; running an external downloader is opt-in.
#[must_use]
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

fn ytdlp_proxy_secondary() -> String {
    std::env::var("YTDLP_PROXY_SECONDARY").unwrap_or_default()
}

fn ytdlp_tmpdir() -> String {
    std::env::var("YTDLP_TMPDIR").unwrap_or_else(|_| "/var/lib/juicebox/ytdlp-tmp".into())
}

fn ytdlp_cookies_path() -> String {
    std::env::var("YTDLP_COOKIES").unwrap_or_default()
}

fn ytdlp_js_runtime() -> String {
    std::env::var("YTDLP_JS_RUNTIME").unwrap_or_else(|_| "deno".into())
}

fn belongs_to_video(path: &std::path::Path, video_id: &str) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let stem = match name.strip_suffix(".info.json") {
        Some(stem) => stem,
        None => name.rsplit_once('.').map_or(name, |(stem, _)| stem),
    };
    stem == video_id || stem.ends_with(&format!("-{video_id}"))
}

fn parse_ytdlp_meta(raw: &[u8]) -> super::types::FetchMeta {
    let Ok(body) = serde_json::from_slice::<serde_json::Value>(raw) else {
        return super::types::FetchMeta::default();
    };
    let pick = |key: &str| body.get(key).and_then(|v| v.as_str());
    super::types::FetchMeta {
        title: pick("title").and_then(super::meta::clean_meta_value),
        author: pick("uploader")
            .or_else(|| pick("channel"))
            .and_then(super::meta::clean_meta_value),
    }
}

async fn read_ytdlp_meta(
    out_dir: &str,
    media_path: &std::path::Path,
    video_id: &str,
) -> (super::types::FetchMeta, Option<PathBuf>) {
    let expected = media_path.with_extension("info.json");
    let mut info_path = tokio::fs::metadata(&expected)
        .await
        .ok()
        .is_some()
        .then_some(expected);
    if info_path.is_none()
        && let Ok(mut rd) = tokio::fs::read_dir(out_dir).await
    {
        while let Ok(Some(entry)) = rd.next_entry().await {
            let p = entry.path();
            let is_info = p
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".info.json"));
            if is_info && belongs_to_video(&p, video_id) {
                info_path = Some(p);
                break;
            }
        }
    }
    let Some(info_path) = info_path else {
        return (super::types::FetchMeta::default(), None);
    };
    let meta = match tokio::fs::read(&info_path).await {
        Ok(raw) => parse_ytdlp_meta(&raw),
        Err(_) => super::types::FetchMeta::default(),
    };
    (meta, Some(info_path))
}

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

/// Empty option values are dropped, so an unset proxy omits `--proxy`.
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
        "--write-info-json".to_string(),
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

/// Proxies are tried in order; stale output is removed before each pass and
/// after storing.
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
    password: Option<String>,
) -> FetchResult {
    let _ = state
        .db_call("update_fetch_job_progress", {
            let job_id = job_id.to_string();
            move |db| db::update_fetch_job_progress(db, &job_id, "yt-dlp", "processing", 0, 0)
        })
        .await;

    let out_dir = ytdlp_tmpdir();
    tokio::fs::create_dir_all(&out_dir)
        .await
        .map_err(|e| format!("yt-dlp tmpdir unavailable: {e}"))?;

    let mut proxies: Vec<String> = Vec::new();
    for proxy in [ytdlp_proxy_secondary(), ytdlp_proxy()] {
        if !proxy.is_empty() && !proxies.contains(&proxy) {
            proxies.push(proxy);
        }
    }
    if proxies.is_empty() {
        return Err("yt-dlp fallback has no proxy configured".into());
    }

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
                move |db| db::update_fetch_job_progress(db, &job_id, &stage, "processing", 0, 0)
            })
            .await;

        let mut rd = match tokio::fs::read_dir(&out_dir).await {
            Ok(rd) => rd,
            Err(e) => return Err(format!("yt-dlp tmpdir unreadable: {e}")),
        };
        while let Some(entry) = rd.next_entry().await.map_err(|e| e.to_string())? {
            let p = entry.path();
            if belongs_to_video(&p, &video_id) {
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

        let pass_no = idx + 1;
        let bin = ytdlp_bin();
        tracing::info!("tier-3 pass {pass_no} via {proxy} ({stage_tag}): running {bin}");
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
            Ok(r) => r.map_err(|e| {
                let bin = ytdlp_bin();
                format!("failed to spawn {bin}: {e}")
            })?,
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
            let is_info = p
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".info.json"));
            if !is_info && belongs_to_video(&p, &video_id) {
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

    let (meta, info_path) = read_ytdlp_meta(&out_dir, &path, &video_id).await;

    let result = download_and_store(
        state,
        user_id,
        job_id,
        source_url,
        ByteSource::LocalFile(path.clone()),
        Some(&name),
        meta,
        opts,
        max_size,
        default_ttl_hours,
        encrypted_ip,
        password,
    )
    .await;

    let _ = tokio::fs::remove_file(&path).await;
    if let Some(info_path) = info_path {
        let _ = tokio::fs::remove_file(info_path).await;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn belongs_to_video_matches_titled_outputs() {
        let p = std::path::Path::new("/tmp/x/Me at the zoo-jNQXAC9IVRw.mp4");
        assert!(belongs_to_video(p, "jNQXAC9IVRw"));
        let info = std::path::Path::new("/tmp/x/Me at the zoo-jNQXAC9IVRw.info.json");
        assert!(belongs_to_video(info, "jNQXAC9IVRw"));
        let dotted = std::path::Path::new("/tmp/x/Mr. Beast-dQw4w9WgXcQ.info.json");
        assert!(belongs_to_video(dotted, "dQw4w9WgXcQ"));
        let bare = std::path::Path::new("/tmp/x/dQw4w9WgXcQ.mp4");
        assert!(belongs_to_video(bare, "dQw4w9WgXcQ"));
        let other = std::path::Path::new("/tmp/x/Other-xxxxxxxxxxx.mp4");
        assert!(!belongs_to_video(other, "dQw4w9WgXcQ"));
    }

    #[test]
    fn parse_ytdlp_meta_reads_title_and_uploader() {
        let raw = br#"{"title":"Me at the zoo","uploader":"jawed","channel":"jawed"}"#;
        let meta = parse_ytdlp_meta(raw);
        assert_eq!(meta.title.as_deref(), Some("Me at the zoo"));
        assert_eq!(meta.author.as_deref(), Some("jawed"));

        let meta = parse_ytdlp_meta(b"not json");
        assert!(meta.title.is_none());
        assert!(meta.author.is_none());
    }
}
