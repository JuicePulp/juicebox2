use std::sync::Arc;

use axum::{
    body::Body,
    extract::{FromRequestParts, State},
    http::{HeaderValue, Request, Response, header, request::Parts},
    middleware,
    response::IntoResponse,
};

use crate::{db, state::AppState};

#[derive(Clone)]
pub struct UserId(pub String);

#[derive(Debug)]
pub struct UserIdMissing;

impl IntoResponse for UserIdMissing {
    fn into_response(self) -> Response<Body> {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "UserId not set by middleware",
        )
            .into_response()
    }
}

impl<S: Send + Sync> FromRequestParts<S> for UserId {
    type Rejection = UserIdMissing;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts.extensions.get::<Self>().cloned().ok_or(UserIdMissing)
    }
}

#[must_use]
pub(crate) fn skips_session_db(path: &str) -> bool {
    matches!(
        path,
        "/api/health"
            | "/api/config"
            | "/api/ip"
            | "/api/openapi.json"
            | "/api/announcement"
            | "/api/ban-status"
    ) || path.starts_with("/internal/")
        || path.starts_with("/api/admin/")
        || (path.starts_with("/file/") && path.ends_with("/info"))
}

pub(crate) async fn user_identity_middleware(
    State(state): State<Arc<AppState>>,
    mut req: Request<Body>,
    next: middleware::Next,
) -> Response<Body> {
    if skips_session_db(req.uri().path()) {
        req.extensions_mut()
            .insert(UserId(uuid::Uuid::new_v4().to_string()));
        return next.run(req).await;
    }
    let now = chrono::Utc::now().timestamp();
    let session_token = crate::auth::cookie_value(req.headers(), crate::auth::SESSION_COOKIE_NAME);
    let user_id = if let Some(token) = session_token.as_ref() {
        let hash = crate::auth::session_token_hash(token);
        state
            .db_call("resolve_session", move |db| {
                db::resolve_session(db, &hash, now)
            })
            .await
            .ok()
            .flatten()
            .map(|(user_id, _)| user_id)
    } else {
        None
    };
    let needs_cookie = user_id.is_none();
    let user_id = user_id.unwrap_or_else(|| {
        crate::auth::verify_legacy_user_cookie(req.headers(), &state.config.jwt_secret)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
    });
    let new_token = if needs_cookie {
        let token = crate::auth::new_session_token();
        let hash = crate::auth::session_token_hash(&token);
        let uid = user_id.clone();
        if state
            .db_call("create_session", move |db| {
                db::create_session(
                    db,
                    &hash,
                    &uid,
                    now,
                    now + crate::auth::SESSION_MAX_AGE_SECS,
                )
            })
            .await
            .is_ok()
        {
            Some(token)
        } else {
            None
        }
    } else {
        None
    };
    let had_legacy =
        crate::auth::cookie_value(req.headers(), crate::auth::LEGACY_USER_COOKIE_NAME).is_some();

    req.extensions_mut().insert(UserId(user_id.clone()));

    let mut response = next.run(req).await;

    if let Some(token) = new_token {
        let cookie_val = crate::auth::create_session_cookie(&token, state.config.secure_cookies);
        if let Ok(header_val) = HeaderValue::from_str(&cookie_val) {
            response
                .headers_mut()
                .append(header::SET_COOKIE, header_val);
        }
    }
    if had_legacy {
        if let Ok(value) = HeaderValue::from_str(&crate::auth::clear_legacy_user_cookie(
            state.config.secure_cookies,
        )) {
            response.headers_mut().append(header::SET_COOKIE, value);
        }
    }

    response
}

#[cfg(test)]
mod tests {
    use super::skips_session_db;

    #[test]
    fn sessionless_paths_skip_session_db() {
        for path in [
            "/api/health",
            "/api/config",
            "/api/ip",
            "/api/openapi.json",
            "/api/announcement",
            "/api/ban-status",
            "/internal/file/abc/status",
            "/internal/alias/old",
            "/internal/ban-snapshot",
            "/api/admin/login",
            "/api/admin/files",
            "/file/abc123/info",
        ] {
            assert!(skips_session_db(path), "{path} should skip session DB");
        }
        for path in [
            "/upload/",
            "/api/tus",
            "/api/fetch/xyz",
            "/api/presence",
            "/api/device/ws",
            "/api/device/status",
            "/api/client-files",
            "/api/register",
            "/file/abc123",
            "/file/abc123/renew",
        ] {
            assert!(!skips_session_db(path), "{path} must keep session DB");
        }
    }
}
