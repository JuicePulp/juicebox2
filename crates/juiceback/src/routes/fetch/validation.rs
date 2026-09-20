use crate::{cobalt, error::AppError, state::AppState};

/// Validate a user-supplied source URL before handing it to cobalt.
/// Blocks non-http(s) schemes, credentials, local/metadata hosts, and
/// private IP literals so cobalt can't be pointed at internal networks.
/// DNS hostnames are resolved and rejected unless every resolved address
/// is publicly routable, unless `allow_private` is set (tests/dev only).
pub(crate) async fn validate_source_url(
    raw: &str,
    allow_private: bool,
) -> Result<String, AppError> {
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
    if !allow_private {
        if crate::storage_client::is_forbidden_hostname(&host) {
            return Err(AppError::BadRequest(format!(
                "refusing to fetch from local host: {host}"
            )));
        }
        if let Ok(ip) = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::IpAddr>()
        {
            // IPv6 url hosts arrive bracketed; strip before parsing.
            if !crate::storage_client::is_public_ip(ip) {
                return Err(AppError::BadRequest(format!(
                    "refusing to fetch from non-public address: {ip}"
                )));
            }
            return Ok(parsed.to_string());
        }
        let port = parsed
            .port_or_known_default()
            .ok_or_else(|| AppError::BadRequest("url must include a valid port".into()))?;
        crate::storage_client::resolve_host_public(&host, port, false)
            .await
            .map_err(|e| AppError::BadRequest(e.to_string()))?;
    }
    Ok(parsed.to_string())
}

#[must_use]
pub(crate) fn require_cobalt_enabled(state: &AppState) -> Result<(), AppError> {
    if !state.config.cobalt_enabled {
        return Err(AppError::NotFound);
    }
    Ok(())
}

/// Human-readable domain for a cobalt service name (e.g. "twitter" -> "x.com").
#[must_use]
pub(crate) fn service_domain(name: &str) -> String {
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

/// Sanitize cobalt's suggested filename and make sure it has an extension.
/// Takes a basename (`../evil.mp4` becomes `evil.mp4`) and appends a
/// fallback extension when missing.
/// NOTE: intentionally different from `upload::common::sanitize_filename`,
/// which strips separators inline to preserve user naming. Keep the two
/// contracts separate.
#[must_use]
pub(crate) fn sanitize_filename(name: Option<&str>, audio_only: bool) -> String {
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

/// Empty stream payload: for `YouTube` links this is the platform blocking
/// the streaming token. The message names that cause.
#[must_use]
pub(crate) fn empty_stream_message(source_url: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn validate_accepts_public_https() {
        assert!(
            validate_source_url("https://youtu.be/dQw4w9WgXcQ", false)
                .await
                .is_ok()
        );
        assert!(
            validate_source_url("http://example.com/video?v=1", false)
                .await
                .is_ok()
        );
        assert!(
            validate_source_url("http://8.8.8.8/video", false)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn validate_allows_private_when_permitted() {
        assert!(
            validate_source_url("http://127.0.0.1:7272/session", true)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn validate_rejects_bad_urls() {
        assert!(validate_source_url("", false).await.is_err());
        assert!(validate_source_url("not a url", false).await.is_err());
        assert!(
            validate_source_url("ftp://example.com/x", false)
                .await
                .is_err()
        );
        assert!(
            validate_source_url("javascript:alert(1)", false)
                .await
                .is_err()
        );
        assert!(
            validate_source_url("https://u:p@example.com/", false)
                .await
                .is_err()
        );
        assert!(
            validate_source_url("https://localhost/video", false)
                .await
                .is_err()
        );
        assert!(
            validate_source_url("https://metadata.google.internal/", false)
                .await
                .is_err()
        );
        assert!(
            validate_source_url("http://127.0.0.1:7272/session", false)
                .await
                .is_err()
        );
        assert!(
            validate_source_url("http://192.168.1.10/x", false)
                .await
                .is_err()
        );
        assert!(
            validate_source_url("http://169.254.169.254/latest/meta-data", false)
                .await
                .is_err()
        );
        assert!(validate_source_url("http://[::1]/x", false).await.is_err());
        assert!(
            validate_source_url("http://2130706433/x", false)
                .await
                .is_err()
        );
        let long = format!("https://example.com/{}", "a".repeat(3000));
        assert!(validate_source_url(&long, false).await.is_err());
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
