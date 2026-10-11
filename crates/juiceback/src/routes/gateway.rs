//! Public key-release gateway for password-protected files.
//!
//! juicehost serves protected unlock shells and ciphertext only. The
//! browser fetches decryption parameters here and POSTs the password
//! directly to juiceback (never via juicehost, which must not see passwords
//! or keys), receiving the file's data key over TLS. The data key is
//! per-file: the global storage key is never exposed.
//!
//! * `GET /api/gateway/params/{id}` - public derivation parameters (salt, KDF
//!   costs, chunk layout, ciphertext URL). The salt is public by design, like a
//!   password-hash salt.
//! * `POST /api/gateway/unlock` - verify the password (rate-limited) and
//!   release the data key. Wrong passwords and unknown ids are
//!   indistinguishable where possible; responses are `no-store`.

use std::{net::SocketAddr, sync::Arc};

use axum::{
    Json,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{crypto_file, db, error::AppError, state::AppState};

#[derive(Serialize, ToSchema)]
pub struct KdfParams {
    mem_kib: u32,
    time: u32,
    lanes: u32,
}

#[derive(Serialize, ToSchema)]
#[serde(tag = "key_version")]
pub enum ParamsResponse {
    #[serde(rename = "1")]
    V1 {
        id: String,
        salt: String,
        kdf: KdfParams,
        chunk_shift: u8,
        plain_len: u64,
        cipher_len: u64,
        ciphertext_url: String,
        filename: String,
        mime_type: String,
    },
    #[serde(rename = "0")]
    Legacy {
        id: String,
        filename: String,
        mime_type: String,
        plain_len: i64,
    },
}

fn ciphertext_url_for(state: &Arc<AppState>, id: &str) -> String {
    format!(
        "{}/c/{}",
        state.config.public_base_url.trim_end_matches('/'),
        id
    )
}

#[utoipa::path(
    get,
    path = "/api/gateway/params/{id}",
    params(("id" = String, Path, description = "File ID")),
    responses(
        (status = 200, description = "Decryption parameters for a protected file"),
        (status = 404, description = "File not found or not protected"),
    ),
    tag = "Gateway",
)]
#[tracing::instrument(skip_all)]
pub async fn params_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ParamsResponse>, AppError> {
    if !crate::utils::is_valid_id(&id) {
        return Err(AppError::NotFound);
    }
    let lookup = id.clone();
    let record = state
        .db_call("get_gateway_params", move |db| db::get_file(db, &lookup))
        .await?
        .ok_or(AppError::NotFound)?;
    if !record.is_protected() {
        return Err(AppError::NotFound);
    }
    if record.uses_per_file_key() {
        let plain_len = u64::try_from(record.size_bytes.max(0))
            .map_err(|_| AppError::Internal("bad file size".into()))?;
        let cipher_len = crypto_file::ciphertext_len(plain_len);
        Ok(Json(ParamsResponse::V1 {
            id: record.id.clone(),
            salt: record.dek_salt.clone().unwrap_or_default(),
            kdf: KdfParams {
                mem_kib: crypto_file::KDF_MEM_KIB,
                time: crypto_file::KDF_TIME,
                lanes: crypto_file::KDF_LANES,
            },
            chunk_shift: crypto_file::CHUNK_SHIFT,
            plain_len,
            cipher_len,
            ciphertext_url: ciphertext_url_for(&state, &record.id),
            filename: record.filename.clone(),
            mime_type: record.mime_type.clone(),
        }))
    } else {
        Ok(Json(ParamsResponse::Legacy {
            id: record.id.clone(),
            filename: record.filename.clone(),
            mime_type: record.mime_type.clone(),
            plain_len: record.size_bytes,
        }))
    }
}

#[derive(Deserialize, ToSchema)]
pub struct UnlockRequest {
    pub id: String,
    pub password: String,
}

#[derive(Serialize, ToSchema)]
struct UnlockResponse {
    dek: String,
    plain_len: u64,
    chunk_shift: u8,
}

#[utoipa::path(
    post,
    path = "/api/gateway/unlock",
    request_body = inline(UnlockRequest),
    responses(
        (status = 200, description = "Data key for a protected file"),
        (status = 403, description = "Invalid password"),
        (status = 404, description = "File not found or not protected"),
        (status = 429, description = "Too many unlock attempts"),
    ),
    tag = "Gateway",
)]
#[tracing::instrument(skip_all)]
pub async fn unlock_handler(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<UnlockRequest>,
) -> Result<Response, AppError> {
    if !crate::utils::is_valid_id(&body.id) {
        return Err(AppError::NotFound);
    }
    let lookup = body.id.clone();
    let record = state
        .db_call("get_gateway_file", move |db| db::get_file(db, &lookup))
        .await?
        .ok_or(AppError::NotFound)?;
    if !record.uses_per_file_key() {
        // Legacy and public files never release keys here: legacy rows have
        // no per-file key, and public files need none. Same 404 either way.
        return Err(AppError::NotFound);
    }

    let ip = crate::utils::client_ip(&headers, addr.ip(), &state).to_string();
    if !state.unlock_limiter.allow(&ip) {
        return Err(AppError::TooManyRequests(
            "Too many unlock attempts. Please wait and try again.".into(),
        ));
    }

    // One blocking task for both Argon2id runs (verify + derive): the
    // password never leaves this call and never appears in errors.
    let password = body.password.clone();
    let hash = record.password_hash.clone().unwrap_or_default();
    let salt_b64 = record.dek_salt.clone().unwrap_or_default();
    let wrapped_b64 = record.dek_wrapped.clone().unwrap_or_default();
    let plain_len = u64::try_from(record.size_bytes.max(0))
        .map_err(|_| AppError::Internal("bad file size".into()))?;
    let dek_b64 = tokio::task::spawn_blocking(move || {
        use base64::Engine as _;
        let ok = crate::auth::verify_password(&password, &hash).map_err(AppError::Internal)?;
        if !ok {
            return Err(AppError::Forbidden("invalid password".into()));
        }
        let salt = base64::engine::general_purpose::STANDARD
            .decode(salt_b64.trim())
            .map_err(|_| AppError::Internal("bad key material".into()))?;
        let kek = crypto_file::derive_kek(&password, &salt)
            .map_err(|_| AppError::Internal("bad key material".into()))?;
        let wrapped = base64::engine::general_purpose::STANDARD
            .decode(wrapped_b64.trim())
            .map_err(|_| AppError::Internal("bad key material".into()))?;
        let dek = crypto_file::unwrap_dek(&kek, &wrapped)
            .map_err(|_| AppError::Forbidden("invalid password".into()))?;
        Ok::<_, AppError>(base64::engine::general_purpose::STANDARD.encode(dek.as_bytes()))
    })
    .await
    .map_err(|_| AppError::Internal("unlock task panicked".into()))??;

    let mut resp = Json(UnlockResponse {
        dek: dek_b64,
        plain_len,
        chunk_shift: crypto_file::CHUNK_SHIFT,
    })
    .into_response();
    resp.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    Ok(resp)
}
