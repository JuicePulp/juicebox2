use std::{sync::Arc, time::Duration};

use serde_json::Value;

use crate::{cobalt::CobaltPickerItem, state::AppState};

/// Public X/Twitter metadata APIs that expose the original media URLs without
/// requiring an account. Cobalt cannot extract some image-only posts, so this
/// is used as a fallback when it reports an empty fetch.
const METADATA_APIS: &[&str] = &[
    "https://api.vxtwitter.com/Twitter/status/{id}",
    "https://api.fxtwitter.com/status/{id}",
];

const USER_AGENT: &str = "Juicebox/2.2 (+https://juicebox2.juicey.dev)";

/// Returns the numeric status id for an X/Twitter post URL.
#[must_use]
pub(crate) fn status_id(raw: &str) -> Option<String> {
    let parsed = url::Url::parse(raw.trim()).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    if host != "x.com"
        && host != "twitter.com"
        && host != "www.x.com"
        && host != "www.twitter.com"
        && host != "mobile.x.com"
        && host != "mobile.twitter.com"
    {
        return None;
    }
    let mut segments = parsed.path_segments()?.filter(|s| !s.is_empty());
    let _first = segments.next()?; // username, or `i` for /i/status/ID
    let marker = segments.next()?;
    if !matches!(marker, "status" | "statuses") {
        return None;
    }
    let id = segments.next()?;
    if id.len() < 5 || id.len() > 25 || !id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(id.to_owned())
}

/// Only original Twitter media hosts are accepted; the generic tunnel fetcher
/// still revalidates each URL before following redirects.
fn media_url(raw: &str) -> Option<String> {
    let parsed = url::Url::parse(raw.trim()).ok()?;
    if parsed.scheme() != "https" || !parsed.username().is_empty() || parsed.password().is_some() {
        return None;
    }
    let host = parsed.host_str()?.to_ascii_lowercase();
    if host != "pbs.twimg.com" && host != "video.twimg.com" {
        return None;
    }
    Some(parsed.to_string())
}

fn media_kind(declared: Option<&str>, url: &str) -> &'static str {
    let declared = declared.unwrap_or_default().to_ascii_lowercase();
    if declared.contains("gif") {
        return "gif";
    }
    if declared.contains("video") || declared.contains("animated") {
        return "video";
    }
    if declared.contains("photo") || declared.contains("image") {
        return "image";
    }
    let path = url.split('?').next().unwrap_or(url).to_ascii_lowercase();
    if path.ends_with(".gif") {
        "gif"
    } else if path.ends_with(".mp4") || path.ends_with(".m3u8") {
        "video"
    } else {
        "image"
    }
}

fn push_media(out: &mut Vec<CobaltPickerItem>, url: &str, kind: Option<&str>) {
    let Some(url) = media_url(url) else { return };
    if out.iter().any(|item| item.url == url) {
        return;
    }
    out.push(CobaltPickerItem {
        r#type: media_kind(kind, &url).to_owned(),
        url,
        thumb: None,
    });
}

fn collect_vxtwitter(value: &Value, out: &mut Vec<CobaltPickerItem>) {
    if let Some(extended) = value.get("media_extended").and_then(Value::as_array) {
        for entry in extended {
            let url = entry.get("url").and_then(Value::as_str).unwrap_or_default();
            let kind = entry.get("type").and_then(Value::as_str);
            push_media(out, url, kind);
        }
    }
    if out.is_empty()
        && let Some(urls) = value.get("mediaURLs").and_then(Value::as_array)
    {
        for url in urls.iter().filter_map(Value::as_str) {
            push_media(out, url, None);
        }
    }
}

fn collect_fxtwitter(value: &Value, out: &mut Vec<CobaltPickerItem>) {
    let Some(media) = value.get("tweet").and_then(|t| t.get("media")) else {
        return;
    };
    for key in ["all", "photos", "videos"] {
        let Some(entries) = media.get(key).and_then(Value::as_array) else {
            continue;
        };
        for entry in entries {
            let url = entry.get("url").and_then(Value::as_str).unwrap_or_default();
            let kind = entry.get("type").and_then(Value::as_str);
            push_media(out, url, kind);
        }
        if !out.is_empty() {
            return;
        }
    }
}

/// Fetches the original media items for an X/Twitter post.
pub(crate) async fn fetch_media(
    state: &Arc<AppState>,
    source_url: &str,
) -> Result<Vec<CobaltPickerItem>, String> {
    let id = status_id(source_url)
        .ok_or_else(|| "not an X/Twitter status URL".to_owned())?;
    let mut last_error = "no X/Twitter metadata API answered".to_owned();

    for template in METADATA_APIS {
        let api_url = template.replace("{id}", &id);
        let response = match state
            .http
            .get(&api_url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::ACCEPT, "application/json")
            .timeout(Duration::from_secs(20))
            .send()
            .await
        {
            Ok(response) => response,
            Err(err) => {
                last_error = format!("{api_url}: {err}");
                continue;
            }
        };
        if !response.status().is_success() {
            last_error = format!("{api_url}: status {}", response.status());
            continue;
        }
        let value: Value = match response.json().await {
            Ok(value) => value,
            Err(err) => {
                last_error = format!("{api_url}: invalid JSON: {err}");
                continue;
            }
        };

        let mut items = Vec::new();
        if api_url.contains("vxtwitter") {
            collect_vxtwitter(&value, &mut items);
        } else {
            collect_fxtwitter(&value, &mut items);
        }
        if !items.is_empty() {
            tracing::info!(
                "x fallback: found {} media item(s) for status {id} via {api_url}",
                items.len()
            );
            return Ok(items);
        }
        last_error = format!("{api_url}: no media URLs");
    }

    Err(last_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_ids() {
        assert_eq!(
            status_id("https://x.com/user/status/2122524486289461421").as_deref(),
            Some("2122524486289461421")
        );
        assert_eq!(
            status_id("https://twitter.com/i/status/1234567890123456789?s=20").as_deref(),
            Some("1234567890123456789")
        );
        assert!(status_id("https://x.com/user").is_none());
        assert!(status_id("https://example.com/user/status/123456789").is_none());
        assert!(status_id("https://x.com/user/status/not-a-number").is_none());
    }

    #[test]
    fn only_accepts_twitter_media_hosts() {
        assert!(media_url("https://pbs.twimg.com/media/abc.jpg?name=orig").is_some());
        assert!(media_url("http://pbs.twimg.com/media/abc.jpg").is_none());
        assert!(media_url("https://evil.example/media/abc.jpg").is_none());
    }

    #[test]
    fn parses_vxtwitter_media() {
        let value: Value = serde_json::from_str(
            r#"{"media_extended":[{"type":"image","url":"https://pbs.twimg.com/media/a.jpg"},{"type":"image","url":"https://pbs.twimg.com/media/b.jpg"}]}"#,
        )
        .unwrap();
        let mut items = Vec::new();
        collect_vxtwitter(&value, &mut items);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].r#type, "image");
    }

    #[test]
    fn parses_fxtwitter_media() {
        let value: Value = serde_json::from_str(
            r#"{"tweet":{"media":{"all":[{"type":"photo","url":"https://pbs.twimg.com/media/a.jpg?name=orig"}]}}}"#,
        )
        .unwrap();
        let mut items = Vec::new();
        collect_fxtwitter(&value, &mut items);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].r#type, "image");
    }
}
