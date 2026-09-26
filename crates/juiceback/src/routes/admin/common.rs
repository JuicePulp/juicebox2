use std::sync::Arc;

use axum::{
    extract::{FromRef, FromRequestParts},
    http::{HeaderMap, header, request::Parts},
};
use serde::Deserialize;

use crate::{auth, db, error::AppError, state::AppState};

#[derive(Clone)]
pub struct AdminUser {
    pub claims: auth::AdminClaims,
}

impl<S: Send + Sync> FromRequestParts<S> for AdminUser
where
    Arc<AppState>: FromRef<S>,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app_state = Arc::<AppState>::from_ref(state);

        let token = extract_jwt(&parts.headers)
            .ok_or_else(|| AppError::Unauthorized("not authenticated".into()))?;

        let secret = app_state.config.jwt_secret.clone();
        let claims = tokio::task::spawn_blocking(move || auth::verify_jwt(&token, &secret))
            .await
            .map_err(|_| AppError::Internal("JWT verification task panicked".into()))?
            .map_err(|_| AppError::Unauthorized("invalid token".into()))?;

        let username = claims.sub.clone();
        if !app_state
            .db_call("authenticate_admin", move |db| {
                Ok(db::get_admin_by_username(db, &username)?.is_some())
            })
            .await?
        {
            return Err(AppError::Unauthorized("admin no longer exists".into()));
        }

        Ok(Self { claims })
    }
}

#[derive(Deserialize)]
pub struct ListParams {
    #[serde(default)]
    pub q: String,

    #[serde(default = "default_sort")]
    pub sort: String,

    #[serde(default = "default_dir")]
    pub dir: String,

    #[serde(default = "default_offset")]
    pub offset: i64,

    #[serde(default = "default_limit")]
    pub limit: i64,
}

fn default_sort() -> String {
    "uploaded_at".to_string()
}
fn default_dir() -> String {
    "desc".to_string()
}
const fn default_offset() -> i64 {
    0
}
const fn default_limit() -> i64 {
    50
}

#[must_use]
fn extract_jwt(headers: &HeaderMap) -> Option<String> {
    let cookie = headers.get(header::COOKIE)?.to_str().ok()?;
    for pair in cookie.split(';') {
        let pair = pair.trim();
        if let Some(value) = pair.strip_prefix("token=") {
            return Some(value.to_string());
        }
    }
    None
}
