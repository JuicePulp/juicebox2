use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use argon2::{
    Argon2,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};
use axum::{
    extract::{FromRef, FromRequestParts},
    http::request::Parts,
};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use rand::Rng;
use serde::{Deserialize, Serialize};

pub const ISS_ADMIN: &str = "juiceback-admin";

pub const ISS_DEVICE: &str = "juiceback-device";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminClaims {
    pub sub: String,
    pub iss: String,

    pub exp: usize,

    pub iat: usize,
}

pub fn hash_password(password: &str) -> Result<String, String> {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|e| format!("hashing failed: {e}"))
}

pub fn verify_password(password: &str, hash: &str) -> Result<bool, String> {
    let parsed = PasswordHash::new(hash).map_err(|e| format!("invalid hash format: {e}"))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

pub fn create_jwt(username: &str, secret: &str) -> Result<String, String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("time error: {e}"))?
        .as_secs() as usize;

    let claims = AdminClaims {
        sub: username.to_string(),
        iss: ISS_ADMIN.to_string(),
        iat: now,
        exp: now + crate::constants::SECONDS_PER_DAY as usize,
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| format!("jwt encoding failed: {e}"))
}

pub fn verify_jwt(token: &str, secret: &str) -> Result<AdminClaims, String> {
    let mut validation = Validation::default();
    validation.set_issuer(&[ISS_ADMIN]);
    let token_data = decode::<AdminClaims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map_err(|e| format!("jwt verification failed: {e}"))?;
    Ok(token_data.claims)
}

pub fn verify_device_jwt(token: &str, secret: &str) -> Result<DeviceClaims, String> {
    let mut validation = Validation::default();
    validation.set_issuer(&[ISS_DEVICE]);
    let token_data = decode::<DeviceClaims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map_err(|e| format!("device JWT verification failed: {e}"))?;
    Ok(token_data.claims)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceClaims {
    pub sub: String,

    pub user_id: String,
    pub iss: String,
    pub iat: usize,
    pub exp: usize,

    #[serde(default)]
    pub file_id: Option<String>,
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub file_size: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct DeviceAuth(pub DeviceClaims);

impl<S: Send + Sync> FromRequestParts<S> for DeviceAuth
where
    Arc<crate::state::AppState>: FromRef<S>,
{
    type Rejection = crate::error::AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app_state = Arc::<crate::state::AppState>::from_ref(state);
        let token = juiceutils::extract_bearer_token(&parts.headers)
            .ok_or_else(|| crate::error::AppError::Unauthorized("Bearer token required".into()))?;
        let claims = verify_device_jwt(token, &app_state.config.ticket_jwt_secret)
            .map_err(|_| crate::error::AppError::Unauthorized("Invalid device token".into()))?;
        let device_id = claims.sub.clone();
        let user_id = claims.user_id.clone();
        let valid = app_state
            .db_call("authenticate_device", move |db| {
                crate::db::device_belongs_to_user(db, &device_id, &user_id)
            })
            .await?;
        if !valid {
            return Err(crate::error::AppError::Unauthorized(
                "Device no longer paired".into(),
            ));
        }
        Ok(Self(claims))
    }
}

pub const SESSION_COOKIE_NAME: &str = "jb_session";

pub const LEGACY_USER_COOKIE_NAME: &str = "jb_uid";

pub const SESSION_MAX_AGE_SECS: i64 = 90 * 24 * 60 * 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserClaims {
    pub sub: String,
    pub iat: usize,
    pub exp: usize,
}

#[must_use]
pub fn create_session_cookie(token: &str, secure: bool) -> String {
    let secure_attribute = if secure { "; Secure" } else { "" };
    format!(
        "{SESSION_COOKIE_NAME}={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age={SESSION_MAX_AGE_SECS}{secure_attribute}"
    )
}

#[must_use]
pub fn clear_legacy_user_cookie(secure: bool) -> String {
    let secure_attribute = if secure { "; Secure" } else { "" };
    format!(
        "{LEGACY_USER_COOKIE_NAME}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0{secure_attribute}"
    )
}

#[must_use]
pub fn cookie_value(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    let cookie_header = headers.get("cookie")?.to_str().ok()?;
    cookie_header
        .split(';')
        .find(|c| c.trim().starts_with(&format!("{name}=")))?
        .trim()
        .strip_prefix(&format!("{name}="))
        .map(str::to_owned)
}

#[must_use]
pub fn new_session_token() -> String {
    let mut token = [0_u8; 32];
    rand::rng().fill_bytes(&mut token);
    hex::encode(token)
}

#[must_use]
pub fn session_token_hash(token: &str) -> String {
    blake3::hash(token.as_bytes()).to_hex().to_string()
}

#[must_use]
pub fn verify_legacy_user_cookie(headers: &axum::http::HeaderMap, secret: &str) -> Option<String> {
    let jwt = cookie_value(headers, LEGACY_USER_COOKIE_NAME)?;

    let token_data = decode::<serde_json::Value>(
        &jwt,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .ok()?;
    if token_data.claims.get("iss").is_some() {
        return None;
    }
    let claims: UserClaims = serde_json::from_value(token_data.claims).ok()?;

    Some(claims.sub)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify_password() {
        let hash = hash_password("my_secret").unwrap();
        assert!(verify_password("my_secret", &hash).unwrap());
    }

    #[test]
    fn opaque_session_cookie_has_defensive_attributes() {
        let token = new_session_token();
        assert_eq!(token.len(), 64);
        let cookie = create_session_cookie(&token, true);
        assert!(cookie.starts_with("jb_session="));
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Strict"));
        assert!(cookie.contains("Max-Age=7776000"));
        assert!(cookie.contains("; Secure"));
        assert!(!cookie.contains("jb_uid"));
    }

    #[test]
    fn verify_wrong_password() {
        let hash = hash_password("correct").unwrap();
        assert!(!verify_password("wrong", &hash).unwrap());
    }

    #[test]
    fn hash_produces_different_salts() {
        let h1 = hash_password("pw").unwrap();
        let h2 = hash_password("pw").unwrap();
        assert_ne!(h1, h2);
    }

    #[test]
    fn create_and_verify_jwt() {
        let token = create_jwt("admin", "secret123").unwrap();
        let claims = verify_jwt(&token, "secret123").unwrap();
        assert_eq!(claims.sub, "admin");
        assert_eq!(claims.iss, ISS_ADMIN);
        assert!(claims.exp > claims.iat);
    }

    fn legacy_cookie_headers(token: &str) -> axum::http::HeaderMap {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            "cookie",
            format!("{LEGACY_USER_COOKIE_NAME}={token}")
                .parse()
                .unwrap(),
        );
        headers
    }

    #[test]
    fn legacy_cookie_without_issuer_verifies() {
        let claims = UserClaims {
            sub: "user-1".into(),
            iat: 1,
            exp: 9_999_999_999,
        };
        let token = jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(b"secret123"),
        )
        .unwrap();
        let headers = legacy_cookie_headers(&token);
        assert_eq!(
            verify_legacy_user_cookie(&headers, "secret123"),
            Some("user-1".into())
        );
    }

    #[test]
    fn admin_jwt_rejected_as_legacy_cookie() {
        let token = create_jwt("admin", "secret123").unwrap();
        let headers = legacy_cookie_headers(&token);
        assert_eq!(verify_legacy_user_cookie(&headers, "secret123"), None);
    }

    #[test]
    fn verify_jwt_wrong_secret() {
        let token = create_jwt("admin", "secret").unwrap();
        let result = verify_jwt(&token, "wrong_secret");
        assert!(result.is_err());
    }

    #[test]
    fn verify_jwt_invalid_token() {
        let result = verify_jwt("not_a_real_token", "secret");
        assert!(result.is_err());
    }

    #[test]
    fn device_jwt_rejected_as_admin_jwt() {
        let token = create_device_jwt_for_test("dev1", "user1", "secret123");
        let result = verify_jwt(&token, "secret123");
        assert!(
            result.is_err(),
            "device JWT should not pass admin verification"
        );
    }

    fn create_device_jwt_for_test(device_id: &str, user_id: &str, secret: &str) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as usize;
        let claims = DeviceClaims {
            sub: device_id.to_string(),
            user_id: user_id.to_string(),
            iss: ISS_DEVICE.to_string(),
            iat: now,
            exp: now + 3600,
            file_id: None,
            filename: None,
            mime_type: None,
            file_size: None,
        };
        encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(secret.as_bytes()),
        )
        .unwrap()
    }
}
