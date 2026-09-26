use std::time::Duration;

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq)]
pub struct FetchOptions {
    pub audio_only: bool,

    pub video_quality: String,

    pub youtube_video_container: String,

    pub audio_format: String,

    pub youtube_better_audio: bool,

    pub youtube_video_codec: String,
}

impl Default for FetchOptions {
    fn default() -> Self {
        Self {
            audio_only: false,
            video_quality: "1080".to_string(),
            youtube_video_container: "auto".to_string(),
            audio_format: "mp3".to_string(),
            youtube_better_audio: false,
            youtube_video_codec: "h264".to_string(),
        }
    }
}

impl FetchOptions {
    #[must_use]
    pub fn sanitized(
        audio_only: bool,
        video_quality: Option<&str>,
        video_container: Option<&str>,
        audio_format: Option<&str>,
        youtube_better_audio: bool,
        youtube_video_codec: Option<&str>,
    ) -> Self {
        let video_quality = match video_quality {
            Some(q)
                if matches!(
                    q,
                    "max"
                        | "4320"
                        | "2160"
                        | "1440"
                        | "1080"
                        | "720"
                        | "480"
                        | "360"
                        | "240"
                        | "144"
                ) =>
            {
                q.to_string()
            }
            _ => "1080".to_string(),
        };
        let youtube_video_container = match video_container {
            Some(c) if matches!(c, "auto" | "mp4" | "webm" | "mkv") => c.to_string(),
            _ => "auto".to_string(),
        };
        let audio_format = match audio_format {
            Some(f) if matches!(f, "best" | "mp3" | "ogg" | "wav" | "opus") => f.to_string(),
            _ => "mp3".to_string(),
        };
        let youtube_video_codec = match youtube_video_codec {
            Some(c) if matches!(c, "h264" | "av1" | "vp9") => c.to_string(),
            _ => "h264".to_string(),
        };
        Self {
            audio_only,
            video_quality,
            youtube_video_container,
            audio_format,
            youtube_better_audio,
            youtube_video_codec,
        }
    }
}

#[must_use]
pub fn build_request_body(url: &str, opts: &FetchOptions) -> serde_json::Value {
    serde_json::json!({
        "url": url,
        "downloadMode": if opts.audio_only { "audio" } else { "auto" },
        "videoQuality": opts.video_quality,
        "audioFormat": opts.audio_format,
        "youtubeVideoCodec": opts.youtube_video_codec,
        "youtubeVideoContainer": opts.youtube_video_container,
        "youtubeBetterAudio": opts.youtube_better_audio,
        "filenameStyle": "basic",
        "localProcessing": "disabled",
    })
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct CobaltPickerItem {
    #[serde(default)]
    pub r#type: String,
    pub url: String,
    #[serde(default)]
    pub thumb: Option<String>,
}

#[derive(Debug, Clone)]
pub enum CobaltResponse {
    Tunnel {
        tunnel_url: String,
        filename: Option<String>,
    },

    Redirect {
        url: String,
        filename: Option<String>,
    },

    Picker {
        items: Vec<CobaltPickerItem>,
    },

    LocalProcessing {
        service: String,
    },

    Error {
        code: String,
    },
}

impl CobaltResponse {
    #[must_use]
    pub fn is_client_refused(&self) -> bool {
        matches!(
            self,
            Self::Error { code }
                if code == "error.api.content.video.unavailable"
        )
    }

    #[must_use]
    pub fn from_json(value: serde_json::Value) -> Self {
        let status = value
            .get("status")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string();
        let get_str = |key: &str| {
            value
                .get(key)
                .and_then(|v| v.as_str())
                .map(ToString::to_string)
        };
        match status.as_str() {
            "tunnel" => match get_str("url") {
                Some(tunnel_url) => Self::Tunnel {
                    tunnel_url,
                    filename: get_str("filename"),
                },
                None => Self::Error {
                    code: "error.api.malformed_response".into(),
                },
            },
            "redirect" => match get_str("url") {
                Some(url) => Self::Redirect {
                    url,
                    filename: get_str("filename"),
                },
                None => Self::Error {
                    code: "error.api.malformed_response".into(),
                },
            },
            "picker" => Self::Picker {
                items: value
                    .get("picker")
                    .and_then(|p| p.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|item| {
                                item.get("url")
                                    .and_then(|u| u.as_str())
                                    .map(|u| CobaltPickerItem {
                                        r#type: item
                                            .get("type")
                                            .and_then(|t| t.as_str())
                                            .unwrap_or("")
                                            .to_string(),
                                        url: u.to_string(),
                                        thumb: item
                                            .get("thumb")
                                            .and_then(|t| t.as_str())
                                            .map(ToString::to_string),
                                    })
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            },
            "local-processing" => Self::LocalProcessing {
                service: get_str("service").unwrap_or_else(|| "unknown".into()),
            },
            "error" => Self::Error {
                code: value
                    .get("error")
                    .and_then(|e| e.get("code"))
                    .and_then(|c| c.as_str())
                    .unwrap_or("error.api.unknown")
                    .to_string(),
            },
            "" => Self::Error {
                code: "error.api.empty_response".into(),
            },
            other => {
                tracing::warn!("cobalt returned unknown status {:?}", other);
                Self::Error {
                    code: format!("error.api.unknown_status:{other}"),
                }
            }
        }
    }
}

pub async fn process(
    client: &reqwest::Client,
    api_url: &str,
    api_key: &str,
    source_url: &str,
    opts: &FetchOptions,
) -> Result<CobaltResponse, String> {
    let mut request = client
        .post(format!("{}/", api_url.trim_end_matches('/')))
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .timeout(Duration::from_secs(30))
        .json(&build_request_body(source_url, opts));
    if !api_key.is_empty() {
        request = request.header("Authorization", format!("Api-Key {api_key}"));
    }

    let resp = request
        .send()
        .await
        .map_err(|e| format!("fetch service unreachable: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();

        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
            return Ok(CobaltResponse::from_json(v));
        }
        return Err(format!("fetch service returned status {status}"));
    }

    let value: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("fetch service returned invalid json: {e}"))?;
    Ok(CobaltResponse::from_json(value))
}

#[must_use]
pub fn parse_youtube_video_id(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    if let Some((_, q)) = rest.split_once("v=") {
        let id: String = q.chars().take_while(|c| *c != '&' && *c != '#').collect();
        if !id.is_empty() {
            return Some(id);
        }
    }
    let parts: Vec<&str> = rest.split('/').collect();
    if parts.len() >= 2 && parts[0].eq_ignore_ascii_case("youtu.be") {
        let seg = parts[1].split('?').next().unwrap_or(parts[1]);
        if !seg.is_empty() {
            return Some(seg.to_string());
        }
    }
    None
}

#[must_use]
pub fn is_youtube_link(url: &str) -> bool {
    let Some((_, rest)) = url.split_once("://") else {
        return false;
    };
    let mut host = rest.split('/').next().unwrap_or("");
    if let Some((_user, h)) = host.rsplit_once('@') {
        host = h;
    }
    let host = host.split(':').next().unwrap_or(host);
    let host = host.to_ascii_lowercase();
    host == "youtube.com" || host == "youtu.be" || host.ends_with(".youtube.com")
}

#[must_use]
pub fn friendly_error(code: &str) -> String {
    match code {
        "error.api.auth.jwt.missing"
        | "error.api.auth.jwt.invalid"
        | "error.api.auth.key.missing"
        | "error.api.auth.key.invalid"
        | "error.api.auth.turnstile.missing"
        | "error.api.auth.turnstile.invalid" => {
            "fetch service rejected our credentials (check COBALT_API_KEY)".into()
        }
        "error.api.rate_limit" => {
            "the fetch service is rate limiting requests, try again later".into()
        }
        "error.api.link.invalid" | "error.api.link.unsupported" => {
            "that link isn't supported by cobalt".into()
        }
        "error.api.service.unsupported" => {
            "this service is not supported by the fetch service".into()
        }
        "error.api.service.disabled" => {
            "this service is disabled on the fetch service instance".into()
        }
        "error.api.fetch.empty" | "error.api.fetch.fail" | "error.api.fetch.critical" => {
            "couldn't find any downloadable media at that link".into()
        }
        "error.api.content.too_long" => "that video is too long for the fetch service".into(),
        "error.api.content.video.unavailable"
        | "error.api.content.video.region"
        | "error.api.content.video.age"
        | "error.api.content.video.private"
        | "error.api.content.video.unplayable" => {
            "the video is unavailable (private, age-restricted or region-locked)".into()
        }
        "error.api.content.post.private" | "error.api.content.post.account" => {
            "that post is private or requires an account".into()
        }
        "error.api.youtube.login"
        | "error.api.youtube.token_expired"
        | "error.api.youtube.token_mismatch" => "youtube wants a login for this video".into(),
        "error.api.malformed_response" | "error.api.empty_response" => {
            "the fetch service sent back a malformed response".into()
        }
        other => {
            let clean = other.split(':').next().unwrap_or(other);
            match clean.strip_prefix("error.api.") {
                Some(rest) => format!("fetch failed ({})", rest.replace('.', " ")),
                None => format!("fetch failed ({clean})"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_body_auto_mode() {
        let body = build_request_body(
            "https://youtu.be/x",
            &FetchOptions::sanitized(false, Some("720"), None, None, false, None),
        );
        assert_eq!(body["downloadMode"], "auto");
        assert_eq!(body["videoQuality"], "720");
        assert_eq!(body["audioFormat"], "mp3");
        assert_eq!(body["youtubeVideoContainer"], "auto");
        assert_eq!(body["youtubeBetterAudio"], false);
        assert_eq!(body["filenameStyle"], "basic");
        assert_eq!(body["localProcessing"], "disabled");
    }

    #[test]
    fn request_body_audio_mode() {
        let body = build_request_body(
            "https://youtu.be/x",
            &FetchOptions::sanitized(
                true,
                Some("bogus"),
                Some("webm"),
                Some("opus"),
                true,
                Some("av1"),
            ),
        );
        assert_eq!(body["downloadMode"], "audio");
        assert_eq!(body["videoQuality"], "1080");
        assert_eq!(body["youtubeVideoContainer"], "webm");
        assert_eq!(body["audioFormat"], "opus");
        assert_eq!(body["youtubeBetterAudio"], true);
        assert_eq!(body["youtubeVideoCodec"], "av1");
    }

    #[test]
    fn sanitize_rejects_bad_values() {
        let o = FetchOptions::sanitized(
            false,
            Some("<script>"),
            Some("avi"),
            Some("flac"),
            true,
            Some("mpeg2"),
        );
        assert_eq!(o.video_quality, "1080");
        assert_eq!(o.youtube_video_container, "auto");
        assert_eq!(o.audio_format, "mp3");
        assert_eq!(o.youtube_video_codec, "h264");
        assert!(o.youtube_better_audio);
    }

    #[test]
    fn parse_tunnel_response() {
        let v = serde_json::json!({
            "status": "tunnel",
            "url": "http://localhost:7272/tunnel?id=x",
            "filename": "video.mp4"
        });
        match CobaltResponse::from_json(v) {
            CobaltResponse::Tunnel {
                tunnel_url,
                filename,
            } => {
                assert_eq!(tunnel_url, "http://localhost:7272/tunnel?id=x");
                assert_eq!(filename.as_deref(), Some("video.mp4"));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_error_response() {
        let v = serde_json::json!({
            "status": "error",
            "error": { "code": "error.api.link.invalid" }
        });
        match CobaltResponse::from_json(v) {
            CobaltResponse::Error { code } => assert_eq!(code, "error.api.link.invalid"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_picker_response() {
        let v = serde_json::json!({
            "status": "picker",
            "picker": [
                { "type": "photo", "url": "https://example.com/1.jpg", "thumb": "https://example.com/1_thumb.jpg" },
                { "type": "video", "url": "https://example.com/2.mp4" }
            ]
        });
        match CobaltResponse::from_json(v) {
            CobaltResponse::Picker { items } => {
                assert_eq!(items.len(), 2);
                assert_eq!(items[0].r#type, "photo");
                assert_eq!(items[1].thumb, None);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_local_processing_response() {
        let v = serde_json::json!({
            "status": "local-processing",
            "type": "merge",
            "service": "youtube",
            "tunnel": [],
            "output": {}
        });
        match CobaltResponse::from_json(v) {
            CobaltResponse::LocalProcessing { service } => assert_eq!(service, "youtube"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn friendly_errors_are_helpful() {
        assert!(friendly_error("error.api.auth.key.invalid").contains("credentials"));
        assert!(friendly_error("error.api.link.unsupported").contains("isn't supported"));
        assert!(friendly_error("error.api.content.too_long").contains("too long"));
        let msg = friendly_error("error.api.something.new");
        assert!(msg.starts_with("fetch failed"));
        assert!(!msg.contains("error.api"));
    }

    #[test]
    fn client_refused_matches_only_unavailable() {
        let refused = CobaltResponse::from_json(serde_json::json!({
            "status": "error",
            "error": {"code": "error.api.content.video.unavailable"}
        }));
        assert!(refused.is_client_refused());

        for code in [
            "error.api.content.too_long",
            "error.api.auth.key.invalid",
            "error.api.fetch.critical",
        ] {
            let r = CobaltResponse::from_json(serde_json::json!({
                "status": "error",
                "error": {"code": code}
            }));
            assert!(
                !r.is_client_refused(),
                "{code} must not count as client-refused"
            );
        }

        assert!(
            !CobaltResponse::LocalProcessing {
                service: "youtube".into()
            }
            .is_client_refused()
        );
    }

    #[test]
    fn youtube_video_id_extraction() {
        assert_eq!(
            parse_youtube_video_id("https://www.youtube.com/watch?v=sPyAQQklc1s&t=1s").as_deref(),
            Some("sPyAQQklc1s")
        );
        assert_eq!(
            parse_youtube_video_id("https://youtu.be/Wfv4Uj-xBT4").as_deref(),
            Some("Wfv4Uj-xBT4")
        );
        assert_eq!(parse_youtube_video_id("https://vimeo.com/123"), None);
    }

    #[test]
    fn youtube_link_detection() {
        assert!(is_youtube_link(
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
        ));
        assert!(is_youtube_link("https://YOUTUBE.COM/shorts/abc"));
        assert!(is_youtube_link("https://youtu.be/jNQXAC9IVRw"));
        assert!(!is_youtube_link("https://vimeo.com/76979871"));
        assert!(!is_youtube_link("https://example.com/not-youtube.com/x"));
    }
}
