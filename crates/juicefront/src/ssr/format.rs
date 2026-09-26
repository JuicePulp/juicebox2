pub fn format_size(bytes: u64) -> String {
    if bytes == 0 {
        return "0 B".to_owned();
    }
    let units = ["B", "KB", "MB", "GB", "TB"];
    let index = (bytes as f64).log(1024_f64) as usize;
    let index = index.min(units.len() - 1);
    let value = bytes as f64 / 1024_f64.powi(index as i32);
    if value >= 10.0 || index == 0 {
        format!("{} {}", value.round() as u64, units[index])
    } else {
        format!("{value:.1} {}", units[index])
    }
}

pub fn format_max_size(bytes: u64) -> String {
    let mb = bytes as f64 / (1024.0 * 1024.0);
    if mb >= 1024.0 {
        let gb = mb / 1024.0;
        if gb.fract() == 0.0 {
            format!("{} GB", gb as u64)
        } else {
            format!("{gb:.1} GB")
        }
    } else if mb.fract() == 0.0 {
        format!("{} MB", mb as u64)
    } else {
        format!("{mb:.1} MB")
    }
}

pub fn icon_for_mime(mime: &str) -> &'static str {
    if mime.starts_with("image/") {
        "image"
    } else if mime.starts_with("video/") {
        "video"
    } else if mime.starts_with("audio/") {
        "audio-waveform"
    } else if mime.contains("spreadsheet") || mime.contains("csv") || mime.contains("excel") {
        "presentation"
    } else if mime.contains("archive")
        || mime.contains("zip")
        || mime.contains("gzip")
        || mime.contains("rar")
        || mime.contains("7z")
        || mime.contains("tar")
        || mime.contains("bzip")
        || mime.contains("xz")
        || mime.contains("compress")
        || mime.contains("zstd")
    {
        "archive"
    } else if mime.contains("wordprocessingml") || mime.contains("presentationml") {
        "file-text"
    } else if mime.contains("json")
        || mime.contains("xml")
        || mime.contains("javascript")
        || mime.contains("ecmascript")
        || mime.contains("typescript")
        || mime.contains("html")
        || mime.contains("css")
    {
        "code"
    } else {
        "file-text"
    }
}

pub fn remaining_label(locale: &str, expires_at: i64, now_ms: i64) -> String {
    if expires_at == 0 {
        return "???".to_owned();
    }
    let left = expires_at * 1000 - now_ms;
    if left <= 0 {
        return crate::i18n::t(locale, "files.expired");
    }
    let secs = left / 1000;
    let mins = secs / 60;
    let hours = mins / 60;
    let days = hours / 24;
    let time = if mins < 1 {
        format!("{secs}s")
    } else if mins < 60 {
        format!("{mins}m")
    } else if hours < 24 {
        format!("{hours}h")
    } else {
        format!("{days}d")
    };
    crate::i18n::tv(locale, "files.expires", "time", &time)
}

pub fn pct_remaining(expires_at: i64, uploaded_at: i64, now_ms: i64) -> u64 {
    let total = (expires_at - uploaded_at) * 1000;
    if total <= 0 {
        return 0;
    }
    let left = expires_at * 1000 - now_ms;
    ((left.max(0) as f64 / total as f64) * 100.0).round() as u64
}

const DEFAULT_HOSTS: &[&str] = &[
    "localhost:6402",
    "127.0.0.1:6402",
    "localhost:6400",
    "127.0.0.1:6400",
];

fn strip_host(host: &str) -> String {
    host.trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_end_matches('/')
        .to_owned()
}

pub fn is_default_host(host: &str, default_host: Option<&str>) -> bool {
    if host.is_empty() {
        return true;
    }
    let stripped = strip_host(host);
    if DEFAULT_HOSTS.contains(&stripped.as_str()) {
        return true;
    }
    match default_host {
        Some(default_host) => stripped == strip_host(default_host),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_format_like_typescript() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(500 * 1024 * 1024), "500 MB");
        assert_eq!(format_max_size(500 * 1024 * 1024), "500 MB");
        assert_eq!(format_max_size(5 * 1024 * 1024 * 1024), "5 GB");
    }

    #[test]
    fn mime_maps_to_icons() {
        assert_eq!(icon_for_mime("image/png"), "image");
        assert_eq!(icon_for_mime("video/mp4"), "video");
        assert_eq!(icon_for_mime("application/zip"), "archive");
        assert_eq!(icon_for_mime("application/octet-stream"), "file-text");
    }
}
