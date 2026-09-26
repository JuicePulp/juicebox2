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
        Some(d) if !d.is_empty() => format!("[{error_code}] {d} (status={status})"),
        _ if body_text.is_empty() => format!("{error_code} (status={status}, body=<empty>)"),
        _ => format!("[{error_code}] {body_text} (status={status})"),
    }
}
