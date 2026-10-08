pub(crate) fn format_error_response(status: u16, body_text: String) -> String {
    let parsed = serde_json::from_str::<serde_json::Value>(&body_text).ok();
    let error_code = parsed
        .as_ref()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()))
        .unwrap_or("UNKNOWN_ERROR");
    let detail = parsed
        .as_ref()
        .and_then(|v| v.get("message").and_then(|m| m.as_str()));
    match detail {
        Some(d) if !d.is_empty() => format!("[{error_code}] {d} (status={status})", d = scrub_body(d)),
        _ if body_text.is_empty() => format!("{error_code} (status={status}, body=<empty>)"),
        _ => format!(
            "[{error_code}] {body} (status={status})",
            body = scrub_body(&body_text)
        ),
    }
}

/// Strip HTML tags, collapse whitespace, and truncate: upstreams (proxies,
/// custom hosts, CDNs) may answer with full HTML error pages, and those must
/// never reach user-facing messages verbatim.
fn scrub_body(body: &str) -> String {
    const MAX_CHARS: usize = 200;
    let mut out = String::new();
    let mut in_tag = false;
    for c in body.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > MAX_CHARS {
        format!("{}…", collapsed.chars().take(MAX_CHARS).collect::<String>())
    } else {
        collapsed
    }
}
