use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use chrono::Utc;
use jsonwebtoken::{EncodingKey, Header, encode};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{error::AppError, routes::UserId, state::AppState, utils::ClientIp};

#[derive(Deserialize, ToSchema)]
pub struct GenerateCodeRequest {
    pub device_name: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct GenerateCodeResponse {
    pub code: String,

    pub expires_in: u64,
}

#[derive(Deserialize, ToSchema)]
pub struct VerifyCodeRequest {
    pub code: String,

    pub device_name: String,
}

#[derive(Serialize, ToSchema)]
pub struct VerifyCodeResponse {
    pub device_id: String,

    pub token: String,
}

#[derive(Serialize, ToSchema)]
pub struct PairingDevice {
    pub device_id: String,

    pub device_name: String,

    pub paired_at: i64,

    pub last_seen_at: i64,
}

#[utoipa::path(
    post,
    path = "/api/pair/generate",
    request_body = GenerateCodeRequest,
    responses(
        (status = 200, description = "One-time pairing code generated", body = GenerateCodeResponse),
    ),
    tag = "Pairing",
)]

pub async fn generate_code_handler(
    State(state): State<Arc<AppState>>,
    UserId(user_id): UserId,
    axum::extract::Extension(client_ip): axum::extract::Extension<ClientIp>,
) -> Result<Json<GenerateCodeResponse>, AppError> {
    let code = generate_pairing_code();
    let code_hash = blake3::hash(code.as_bytes()).to_hex().to_string();

    let now = Utc::now().timestamp();
    let expires_at = now + 300;

    let user_id_for_insert = user_id.clone();
    let ip_hash = crate::utils::hash_ip_for_ban(&client_ip.0.to_string(), &state.config.ip_pepper);
    let inserted = state
        .db_call("insert_pairing_code", move |db| {
            let tx = Transaction::new_unchecked(db, TransactionBehavior::Immediate)?;
            let outstanding_for_user: i64 = tx.query_row(
                "SELECT COUNT(*) FROM pairing_codes WHERE user_id = ?1 AND used = 0 AND expires_at > ?2",
                rusqlite::params![user_id_for_insert, now],
                |row| row.get(0),
            )?;
            let outstanding_for_ip: i64 = tx.query_row(
                "SELECT COUNT(*) FROM pairing_codes WHERE ip_hash = ?1 AND used = 0 AND expires_at > ?2",
                rusqlite::params![ip_hash, now],
                |row| row.get(0),
            )?;
            if outstanding_for_user >= crate::constants::MAX_PAIRING_CODES_PER_USER
                || outstanding_for_ip >= crate::constants::MAX_PAIRING_CODES_PER_IP
            {
                return Ok(false);
            }
            let inserted = tx.execute(
                "INSERT INTO pairing_codes (code_hash, user_id, created_at, expires_at, used, ip_hash)
                 VALUES (?1, ?2, ?3, ?4, 0, ?5)",
                rusqlite::params![code_hash, user_id, now, expires_at, ip_hash],
            )? > 0;
            tx.commit()?;
            Ok(inserted)
        })
        .await?;
    if !inserted {
        return Err(AppError::TooManyRequests(
            "too many outstanding pairing codes".into(),
        ));
    }

    Ok(Json(GenerateCodeResponse {
        code,
        expires_in: 300,
    }))
}

#[utoipa::path(
    post,
    path = "/api/pair/verify",
    request_body = VerifyCodeRequest,
    responses(
        (status = 200, description = "Device paired successfully", body = VerifyCodeResponse),
        (status = 400, description = "Invalid code format"),
        (status = 404, description = "Code not found"),
        (status = 409, description = "Code already used"),
    ),
    tag = "Pairing",
)]

pub async fn verify_code_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<VerifyCodeRequest>,
) -> Result<Json<VerifyCodeResponse>, AppError> {
    let code = body.code.trim().to_uppercase();
    if !is_valid_pairing_code(&code) {
        return Err(AppError::BadRequest(
            "Invalid code format. Expected XXXX-XXXX.".into(),
        ));
    }

    let code_hash = blake3::hash(code.as_bytes()).to_hex().to_string();
    let now = Utc::now().timestamp();
    let device_id = uuid::Uuid::new_v4().to_string();
    let device_name = body.device_name.trim().to_string();
    if device_name.is_empty() || device_name.len() > 128 {
        return Err(AppError::BadRequest("invalid device name".into()));
    }
    let device_id_clone = device_id.clone();
    let claim = state
        .db_call("claim_pairing_code", move |db| {
            claim_pairing_code(db, &code_hash, &device_id_clone, &device_name, now)
        })
        .await?;

    let user_id = match claim {
        PairingClaim::Claimed(user_id) => user_id,
        PairingClaim::NotFound => return Err(AppError::NotFound),
        PairingClaim::Expired => return Err(AppError::Gone),
        PairingClaim::AlreadyUsed => return Err(AppError::Conflict("Code already used".into())),
    };

    let token = create_device_jwt(&device_id, &user_id, &state.config.ticket_jwt_secret)
        .map_err(|e| AppError::Internal(format!("JWT encoding failed: {e}")))?;

    Ok(Json(VerifyCodeResponse { device_id, token }))
}

#[utoipa::path(
    get,
    path = "/api/device",
    responses(
        (status = 200, description = "List of paired devices", body = Vec<PairingDevice>),
    ),
    tag = "Pairing",
)]

pub async fn list_devices_handler(
    State(state): State<Arc<AppState>>,
    UserId(user_id): UserId,
) -> Result<Json<Vec<PairingDevice>>, AppError> {
    let devices = state
        .db_call("list_devices_handler", move |db| {
            let mut stmt = db.prepare(
                "SELECT id, device_name, paired_at, last_seen_at FROM devices WHERE user_id = ?1",
            )?;
            let rows = stmt.query_map(rusqlite::params![user_id], |row| {
                Ok(PairingDevice {
                    device_id: row.get(0)?,
                    device_name: row.get(1)?,
                    paired_at: row.get(2)?,
                    last_seen_at: row.get(3)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .await?;

    Ok(Json(devices))
}

#[utoipa::path(
    delete,
    path = "/api/device/{id}",
    params(
        ("id" = String, Path, description = "The device ID to unpair"),
    ),
    responses(
        (status = 200, description = "Device unpaired"),
        (status = 404, description = "Device not found"),
    ),
    tag = "Pairing",
)]

pub async fn unpair_device_handler(
    State(state): State<Arc<AppState>>,
    UserId(user_id): UserId,
    Path(device_id): Path<String>,
) -> Result<StatusCode, AppError> {
    let did_del = device_id.clone();
    let uid_del = user_id.clone();
    let deleted = state
        .db_call("unpair_device_handler", move |db| {
            let mut stmt = db.prepare("DELETE FROM devices WHERE id = ?1 AND user_id = ?2")?;
            let rows = stmt.execute(rusqlite::params![did_del, uid_del])?;
            Ok(rows > 0)
        })
        .await?;

    if deleted {
        if let Some(mut devices) = state.connected_devices.get_mut(&user_id) {
            devices.retain(|d| {
                let Ok(inner) = d.try_lock() else {
                    return true;
                };
                if inner.device_id == device_id {
                    false
                } else {
                    true
                }
            });
            if devices.is_empty() {
                drop(devices);
                state.connected_devices.remove(&user_id);
            }
        }

        if let Some(tx) = state.presence_listeners.get(&user_id) {
            let _ = tx.send(crate::state::PresenceEvent::DeviceDisconnected {
                device_id: device_id.clone(),
            });
        }

        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AppError::NotFound)
    }
}

#[derive(Debug, PartialEq, Eq)]
enum PairingClaim {
    Claimed(String),
    NotFound,
    Expired,
    AlreadyUsed,
}

fn claim_pairing_code(
    db: &rusqlite::Connection,
    code_hash: &str,
    device_id: &str,
    device_name: &str,
    now: i64,
) -> rusqlite::Result<PairingClaim> {
    let tx = Transaction::new_unchecked(db, TransactionBehavior::Immediate)?;
    let record: Option<(String, i64, i64)> = tx
        .query_row(
            "SELECT user_id, expires_at, used FROM pairing_codes WHERE code_hash = ?1",
            rusqlite::params![code_hash],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;

    let Some((user_id, expires_at, used)) = record else {
        return Ok(PairingClaim::NotFound);
    };
    if now > expires_at {
        return Ok(PairingClaim::Expired);
    }
    if used != 0 {
        return Ok(PairingClaim::AlreadyUsed);
    }

    tx.execute(
        "UPDATE pairing_codes SET used = 1 WHERE code_hash = ?1",
        rusqlite::params![code_hash],
    )?;
    tx.execute(
        "INSERT INTO devices (id, user_id, device_name, paired_at, last_seen_at)
         VALUES (?1, ?2, ?3, ?4, ?4)",
        rusqlite::params![device_id, user_id, device_name, now],
    )?;
    tx.commit()?;

    Ok(PairingClaim::Claimed(user_id))
}

fn generate_pairing_code() -> String {
    let chars: Vec<char> = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789".chars().collect();
    (0..9)
        .map(|i| {
            if i == 4 {
                '-'
            } else {
                chars[rand::random_range(0..chars.len())]
            }
        })
        .collect()
}

#[must_use]
pub fn is_valid_pairing_code(code: &str) -> bool {
    let bytes = code.as_bytes();
    bytes.len() == 9
        && bytes[..4].iter().all(|b| b.is_ascii_alphanumeric())
        && bytes[4] == b'-'
        && bytes[5..].iter().all(|b| b.is_ascii_alphanumeric())
}

fn create_device_jwt(device_id: &str, user_id: &str, secret: &str) -> Result<String, String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("time error: {e}"))?
        .as_secs() as usize;

    let claims = DeviceClaims {
        sub: device_id.to_string(),
        user_id: user_id.to_string(),
        iss: "juiceback-device".to_string(),
        iat: now,
        exp: now + 30 * 24 * 3600,
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| format!("JWT encoding failed: {e}"))
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DeviceClaims {
    pub sub: String,

    pub user_id: String,

    pub iss: String,

    pub iat: usize,

    pub exp: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_format_valid() {
        let code = generate_pairing_code();
        assert_eq!(code.len(), 9);
        assert_eq!(code.as_bytes()[4], b'-');
        assert!(is_valid_pairing_code(&code));
    }

    #[test]
    fn code_validation_rejects_bad_lengths() {
        assert!(!is_valid_pairing_code("ABC"));
        assert!(!is_valid_pairing_code("ABCDEFGHJKLMNP23456789"));
        assert!(!is_valid_pairing_code("ABCDEFGH"));
    }

    #[test]
    fn code_validation_rejects_dash_in_wrong_place() {
        assert!(!is_valid_pairing_code("ABC-EFGHJ"));
    }

    #[test]
    fn code_is_uppercase_alphanumeric() {
        let code = generate_pairing_code();
        let chars: Vec<char> = code.chars().collect();

        for (i, c) in chars.iter().enumerate() {
            if i == 4 {
                assert_eq!(*c, '-');
            } else {
                assert!(c.is_ascii_uppercase() || c.is_ascii_digit());
                assert!(!matches!(*c, 'I' | '1' | 'O' | '0'));
            }
        }
    }

    fn pairing_db() -> rusqlite::Connection {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::init_db(&db).unwrap();
        db
    }

    fn insert_pairing_code(db: &rusqlite::Connection, hash: &str, expires_at: i64, used: i64) {
        db.execute(
            "INSERT INTO pairing_codes (code_hash, user_id, created_at, expires_at, used)
             VALUES (?1, 'user-1', 1, ?2, ?3)",
            rusqlite::params![hash, expires_at, used],
        )
        .unwrap();
    }

    #[test]
    fn claim_pairing_code_inserts_device_and_prevents_reuse() {
        let db = pairing_db();
        insert_pairing_code(&db, "hash", 200, 0);

        assert_eq!(
            claim_pairing_code(&db, "hash", "device-1", "Laptop", 100).unwrap(),
            PairingClaim::Claimed("user-1".into())
        );
        assert_eq!(
            claim_pairing_code(&db, "hash", "device-2", "Phone", 100).unwrap(),
            PairingClaim::AlreadyUsed
        );
        let count: i64 = db
            .query_row("SELECT COUNT(*) FROM devices", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn claim_pairing_code_distinguishes_missing_and_expired() {
        let db = pairing_db();
        insert_pairing_code(&db, "expired", 99, 0);

        assert_eq!(
            claim_pairing_code(&db, "missing", "device-1", "Laptop", 100).unwrap(),
            PairingClaim::NotFound
        );
        assert_eq!(
            claim_pairing_code(&db, "expired", "device-1", "Laptop", 100).unwrap(),
            PairingClaim::Expired
        );
    }

    #[test]
    fn failed_device_insert_does_not_consume_code() {
        let db = pairing_db();
        insert_pairing_code(&db, "hash", 200, 0);
        db.execute(
            "INSERT INTO devices (id, user_id, device_name, paired_at, last_seen_at)
             VALUES ('duplicate', 'user-1', 'Existing', 1, 1)",
            [],
        )
        .unwrap();

        assert!(claim_pairing_code(&db, "hash", "duplicate", "Laptop", 100).is_err());
        let used: i64 = db
            .query_row(
                "SELECT used FROM pairing_codes WHERE code_hash = 'hash'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(used, 0);
    }
}
