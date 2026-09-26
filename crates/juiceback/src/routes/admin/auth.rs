use std::sync::Arc;

use axum::{
    Form,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Json, Response},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::common::AdminUser;
use crate::{auth, db, error::AppError, state::AppState, utils::ClientIp};

const MAX_FAILURES: u32 = 5;
const LOCKOUT_SECS: i64 = 600;

#[derive(Deserialize, ToSchema)]
pub struct LoginRequest {
    pub username: String,

    pub password: String,
}

#[derive(Serialize, ToSchema)]
pub struct LoginResponse {
    pub username: String,
}

#[derive(Serialize, ToSchema)]
pub struct CheckResponse {
    pub ok: bool,

    pub username: String,
}

#[utoipa::path(
    get,
    post,
    path = "/api/admin/logout",
    responses(
        (status = 302, description = "Redirects to /admin/login and clears the JWT cookie"),
    ),
    security(),
    tag = "Admin",
)]
pub async fn logout_handler(State(state): State<Arc<AppState>>) -> Response {
    let cookie = format!(
        "token=; Path=/; Max-Age=0; HttpOnly; SameSite=Strict{}",
        if state.config.secure_cookies {
            "; Secure"
        } else {
            ""
        }
    );
    (
        StatusCode::FOUND,
        [
            (header::LOCATION, "/admin/login"),
            (header::SET_COOKIE, cookie.as_str()),
        ],
    )
        .into_response()
}

#[utoipa::path(
    post,
    path = "/api/admin/login",
    request_body(content_type = "application/x-www-form-urlencoded", content = inline(LoginRequest), description = "Admin username and password"),
    responses(
        (status = 200, description = "Login successful, sets JWT cookie", body = LoginResponse),
        (status = 401, description = "Invalid credentials"),
        (status = 429, description = "Rate limit exceeded"),
    ),
    tag = "Admin",
)]
pub async fn login_handler(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(client_ip): axum::extract::Extension<ClientIp>,
    Form(form): Form<LoginRequest>,
) -> Result<Response, AppError> {
    let username = form.username.trim().to_lowercase();
    if username.is_empty() || form.password.is_empty() {
        return Err(AppError::BadRequest(
            "username and password are required".into(),
        ));
    }

    let username_for_lockout = username.clone();
    let ip_hash = crate::utils::hash_ip_for_ban(&client_ip.0.to_string(), &state.config.ip_pepper);
    let ip_hash_for_lockout = ip_hash.clone();
    let failed_count = state
        .db_call("count_failed_logins", move |db| {
            db::count_failed_logins_for_ip(
                db,
                &username_for_lockout,
                &ip_hash_for_lockout,
                LOCKOUT_SECS,
            )
        })
        .await
        .unwrap_or(0);

    if failed_count >= MAX_FAILURES {
        tracing::warn!(
            "admin login: account '{username}' is locked out ({failed_count} failures in {LOCKOUT_SECS}s)"
        );
        return Err(AppError::TooManyRequests(
            "account temporarily locked due to too many failed attempts".into(),
        ));
    }
    if failed_count > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(
            200_u64.saturating_mul(1_u64 << failed_count.min(4)),
        ))
        .await;
    }

    let username_for_db = username.clone();
    let user = state
        .db_call("get_admin_by_username", move |db| {
            db::get_admin_by_username(db, &username_for_db)
        })
        .await?;
    let Some(user) = user else {
        state
            .db_call("insert_failed_login", {
                let uname = username.clone();
                let missing_user_ip_hash = ip_hash.clone();
                move |db| db::insert_failed_login_for_ip(db, &uname, &missing_user_ip_hash)
            })
            .await?;
        return Err(AppError::Unauthorized("invalid credentials".into()));
    };

    let password = form.password.clone();
    let hash = user.password_hash.clone();
    let valid = tokio::task::spawn_blocking(move || auth::verify_password(&password, &hash))
        .await
        .map_err(|_| AppError::Internal("password verification task panicked".into()))?
        .map_err(AppError::Internal)?;

    if !valid {
        let uname = username.clone();
        let failed_ip_hash = ip_hash.clone();
        state
            .db_call("insert_failed_login", move |db| {
                db::insert_failed_login_for_ip(db, &uname, &failed_ip_hash)
            })
            .await?;
        return Err(AppError::Unauthorized("invalid credentials".into()));
    }

    let uname = username.clone();
    let success_ip_hash = ip_hash.clone();
    state
        .db_call("delete_failed_logins", move |db| {
            db::delete_failed_logins_for_ip(db, &uname, &success_ip_hash)
        })
        .await?;

    let username2 = user.username.clone();
    let secret = state.config.jwt_secret.clone();
    let token = tokio::task::spawn_blocking(move || auth::create_jwt(&username2, &secret))
        .await
        .map_err(|_| AppError::Internal("jwt signing task panicked".into()))?
        .map_err(AppError::Internal)?;

    let cookie = format!(
        "token={}; Path=/; Max-Age={}; HttpOnly; SameSite=Strict{}",
        token,
        crate::constants::ADMIN_JWT_COOKIE_MAX_AGE_SECS,
        if state.config.secure_cookies {
            "; Secure"
        } else {
            ""
        }
    );

    let body = Json(LoginResponse {
        username: user.username,
    });

    Ok((StatusCode::OK, [(header::SET_COOKIE, cookie)], body).into_response())
}

#[utoipa::path(
    get,
    path = "/api/admin/check",
    responses(
        (status = 200, description = "JWT is valid", body = CheckResponse),
        (status = 401, description = "Not authenticated or invalid token"),
    ),
    security(),
    tag = "Admin",
)]
pub async fn check_handler(admin: AdminUser) -> Result<Json<CheckResponse>, AppError> {
    Ok(Json(CheckResponse {
        ok: true,
        username: admin.claims.sub,
    }))
}
