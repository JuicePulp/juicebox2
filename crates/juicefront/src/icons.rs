use std::sync::atomic::{AtomicU64, Ordering};

include!(concat!(env!("OUT_DIR"), "/icon_data.rs"));

const NAME_MAP: &[(&str, &str)] = &[
    ("x", "close"),
    ("report", "flag"),
    ("back", "corner-up-left"),
    ("book", "book-open"),
    ("circle-help", "book-open"),
    ("message-square", "message"),
    ("more", "more-horizontal"),
    ("file-image", "image"),
    ("file-video", "video"),
    ("file-text", "file-text"),
    ("file-archive", "archive"),
    ("file-code", "code"),
    ("file-audio", "audio-waveform"),
    ("file-spreadsheet", "presentation"),
    ("trash-simple", "trash"),
    ("brand-bluesky", "bluesky"),
    ("brand-twitter", "twitter"),
    ("brand-reddit", "reddit"),
    ("brand-mastodon", "mastodon"),
    ("brand-linkedin", "linkedin"),
    ("brand-telegram", "telegram"),
    ("brand-whatsapp", "whatsapp"),
    ("tick", "checkmark"),
    ("tick-on", "checkmark-on"),
    ("box", "checkbox"),
    ("box-on", "checkbox-on"),
];

static ICON_COUNTER: AtomicU64 = AtomicU64::new(0);

pub const RUNTIME_ICONS: &[&str] = &[
    "copy",
    "trash",
    "upload",
    "video",
    "image",
    "audio-waveform",
    "presentation",
    "archive",
    "file-text",
    "code",
    "close",
    "flag",
];

pub fn bundle_json(names: &[&str]) -> String {
    let mut out = String::from("{");
    for (index, name) in names.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&serde_json::to_string(name).unwrap_or_default());
        out.push(':');
        out.push_str(&serde_json::to_string(get_svg(name)).unwrap_or_default());
    }
    out.push('}');
    out
}

pub fn resolve_name(name: &str) -> &str {
    NAME_MAP
        .iter()
        .find(|(alias, _)| *alias == name)
        .map_or(name, |(_, real)| *real)
}

pub fn get_svg(name: &str) -> &'static str {
    get_raw(resolve_name(name)).unwrap_or("")
}

pub fn icon_html(name: &str, size: u32, class: &str) -> String {
    let raw = get_svg(name);
    if raw.is_empty() {
        return String::new();
    }
    let uid = ICON_COUNTER.fetch_add(1, Ordering::Relaxed);
    let with_size = raw.replacen(
        "<svg",
        &format!(
            "<svg width=\"{size}\" height=\"{size}\"{class} style=\"display:block;shape-rendering:crispEdges\"",
            class = if class.is_empty() {
                String::new()
            } else {
                format!(" class=\"{class}\"")
            }
        ),
        1,
    );
    suffix_ids(&with_size, uid)
}

fn suffix_ids(svg: &str, uid: u64) -> String {
    let mut out = String::with_capacity(svg.len() + 16);
    let mut rest = svg;
    loop {
        let next = match (rest.find("id=\""), rest.find("url(#")) {
            (Some(a), Some(b)) => Some(if a < b { (a, 0) } else { (b, 1) }),
            (Some(a), None) => Some((a, 0)),
            (None, Some(b)) => Some((b, 1)),
            (None, None) => None,
        };
        let Some((pos, kind)) = next else {
            out.push_str(rest);
            break;
        };
        if kind == 0 {
            let value_start = pos + 4;
            if let Some(rel) = rest[value_start..].find('"') {
                out.push_str(&rest[..value_start + rel]);
                out.push_str(&format!("-{uid}\""));
                rest = &rest[value_start + rel + 1..];
                continue;
            }
        } else if let Some(rel) = rest[pos + 5..].find(')') {
            out.push_str(&rest[..pos + 5 + rel]);
            out.push_str(&format!("-{uid})"));
            rest = &rest[pos + 5 + rel + 1..];
            continue;
        }
        out.push_str(rest);
        break;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_icon_renders_with_size() {
        let html = icon_html("copy", 24, "");
        assert!(html.starts_with("<svg"));
        assert!(html.contains("width=\"24\""));
    }

    #[test]
    fn unknown_icon_renders_empty() {
        assert_eq!(icon_html("no-such-icon", 24, ""), "");
    }

    #[test]
    fn aliases_resolve() {
        assert_eq!(resolve_name("x"), "close");
        assert_eq!(resolve_name("copy"), "copy");
    }
}
