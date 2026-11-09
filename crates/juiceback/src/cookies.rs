use axum::http::{HeaderMap, header};

/// Builds a `Set-Cookie` value with the deployment's shared attributes.
#[must_use]
pub fn header(name: &str, value: &str, max_age_secs: i64, http_only: bool, secure: bool) -> String {
    let mut cookie = format!("{name}={value}; Path=/; SameSite=Strict; Max-Age={max_age_secs}");
    if http_only {
        cookie.push_str("; HttpOnly");
    }
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

/// Reads a cookie without percent-decoding; callers only store cookie-safe
/// values.
#[must_use]
pub fn value(headers: &HeaderMap, name: &str) -> Option<String> {
    let cookie_header = headers.get(header::COOKIE)?.to_str().ok()?;
    cookie_header
        .split(';')
        .find_map(|pair| pair.trim().strip_prefix(&format!("{name}=")))
        .map(str::to_owned)
}
