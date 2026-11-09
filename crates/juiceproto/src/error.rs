/// Renders a downstream error body as `[CODE] message (status=N)`, falling back
/// to a scrubbed snippet when the body is not JSON.
#[must_use]
pub fn format_service_error(status: u16, body_text: &str) -> String {
    let parsed = serde_json::from_str::<serde_json::Value>(body_text).ok();
    let error_code = parsed
        .as_ref()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()))
        .unwrap_or("UNKNOWN_ERROR");
    let detail = parsed
        .as_ref()
        .and_then(|v| v.get("message").and_then(|m| m.as_str()));
    match detail {
        Some(d) if !d.is_empty() => {
            format!("[{error_code}] {d} (status={status})", d = scrub_body(d))
        }
        _ if body_text.is_empty() => format!("{error_code} (status={status}, body=<empty>)"),
        _ => format!(
            "[{error_code}] {body} (status={status})",
            body = scrub_body(body_text)
        ),
    }
}

/// Recovers the message from a string built by [`format_service_error`].
#[must_use]
pub fn service_error_message(error: &str) -> &str {
    error
        .strip_prefix('[')
        .and_then(|s| s.find("] "))
        .map(|i| &error[i + 2..])
        .and_then(|s| s.rsplit_once(" (status="))
        .map(|(msg, _)| msg)
        .unwrap_or(error)
}

/// Strips tags and truncates so error strings stay log-safe.
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
        format!(
            "{}...",
            collapsed.chars().take(MAX_CHARS).collect::<String>()
        )
    } else {
        collapsed
    }
}

#[cfg(test)]
mod tests {
    use super::{format_service_error, service_error_message};

    #[test]
    fn json_body_uses_code_and_message() {
        let body = r#"{"error": "STORAGE_FULL", "message": "disk is full"}"#;
        let result = format_service_error(507, body);
        assert!(result.contains("[STORAGE_FULL]"));
        assert!(result.contains("disk is full"));
        assert!(result.contains("status=507"));
    }

    #[test]
    fn empty_body_reports_unknown_error() {
        let result = format_service_error(500, "");
        assert!(result.contains("UNKNOWN_ERROR"));
        assert!(result.contains("body=<empty>"));
        assert!(result.contains("status=500"));
    }

    #[test]
    fn plain_body_is_preserved() {
        let result = format_service_error(403, "forbidden");
        assert!(result.contains("forbidden"));
        assert!(result.contains("status=403"));
    }

    #[test]
    fn message_round_trips() {
        let rendered = format_service_error(507, r#"{"error":"X","message":"out of space"}"#);
        assert_eq!(service_error_message(&rendered).trim(), "out of space");
        assert_eq!(service_error_message("plain failure"), "plain failure");
    }
}
