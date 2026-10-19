use std::time::Duration;

use super::types::FetchMeta;
use crate::cobalt;

const META_VALUE_MAX_CHARS: usize = 300;
const OEMBED_TIMEOUT_SECS: u64 = 8;

#[must_use]
pub(crate) fn clean_meta_value(value: &str) -> Option<String> {
    let cleaned: String = value.chars().filter(|c| !c.is_control()).collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(META_VALUE_MAX_CHARS).collect())
}

#[must_use]
pub(crate) fn author_from_cobalt_filename(filename: &str) -> Option<String> {
    let stem = filename.rsplit_once('.').map_or(filename, |(stem, _)| stem);
    let rest = stem.strip_prefix("tiktok_")?;
    let (author, id) = rest.rsplit_once('_')?;
    if author.is_empty() || id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    clean_meta_value(author)
}

#[must_use]
pub(crate) fn title_from_bun_filename(filename: &str, video_id: Option<&str>) -> Option<String> {
    let stem = filename.rsplit_once('.').map_or(filename, |(stem, _)| stem);
    let title = match video_id {
        Some(id) if !id.is_empty() => stem.strip_suffix(&format!("-{id}")).unwrap_or(stem),
        _ => stem,
    };
    clean_meta_value(title)
}

#[must_use]
pub(crate) fn decoded_header_value(raw: &str) -> String {
    percent_encoding::percent_decode_str(raw)
        .decode_utf8_lossy()
        .to_string()
}

pub(crate) async fn youtube_oembed_metadata(
    client: &reqwest::Client,
    source_url: &str,
) -> FetchMeta {
    if !cobalt::is_youtube_link(source_url) {
        return FetchMeta::default();
    }
    let encoded =
        percent_encoding::utf8_percent_encode(source_url, percent_encoding::NON_ALPHANUMERIC)
            .to_string();
    let url = format!("https://www.youtube.com/oembed?url={encoded}&format=json");
    let body: serde_json::Value = match client
        .get(url)
        .timeout(Duration::from_secs(OEMBED_TIMEOUT_SECS))
        .header(reqwest::header::USER_AGENT, "juicebox/2.1 (metadata lookup)")
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => match resp.json().await {
            Ok(body) => body,
            Err(_) => return FetchMeta::default(),
        },
        _ => return FetchMeta::default(),
    };
    FetchMeta {
        title: body
            .get("title")
            .and_then(|v| v.as_str())
            .and_then(clean_meta_value),
        author: body
            .get("author_name")
            .and_then(|v| v.as_str())
            .and_then(clean_meta_value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cobalt_tiktok_filename_yields_author() {
        assert_eq!(
            author_from_cobalt_filename("tiktok_olmac20_7689169056590581014.mp4").as_deref(),
            Some("olmac20")
        );
        assert_eq!(
            author_from_cobalt_filename("tiktok_swift.orfevacle_7697665408748309782.mp4").as_deref(),
            Some("swift.orfevacle")
        );
    }

    #[test]
    fn cobalt_non_tiktok_filename_yields_nothing() {
        assert_eq!(author_from_cobalt_filename("video.mp4"), None);
        assert_eq!(author_from_cobalt_filename("title - channel.mp4"), None);
        assert_eq!(author_from_cobalt_filename("tiktok_notanid_abc.mp4"), None);
    }

    #[test]
    fn bun_filename_strips_video_id_suffix() {
        assert_eq!(
            title_from_bun_filename(
                "Me at the zoo-jNQXAC9IVRw.mp4",
                Some("jNQXAC9IVRw")
            )
            .as_deref(),
            Some("Me at the zoo")
        );
        assert_eq!(
            title_from_bun_filename("Some Clip.mp4", Some("dQw4w9WgXcQ")).as_deref(),
            Some("Some Clip")
        );
        assert_eq!(
            title_from_bun_filename("video.mp4", None).as_deref(),
            Some("video")
        );
    }

    #[test]
    fn clean_meta_value_trims_and_drops_empties() {
        assert_eq!(clean_meta_value("  jawed  ").as_deref(), Some("jawed"));
        assert_eq!(clean_meta_value("   "), None);
        assert_eq!(clean_meta_value("bad\u{0}value").as_deref(), Some("badvalue"));
    }
}
