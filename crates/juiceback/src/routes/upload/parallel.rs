use std::{net::SocketAddr, sync::Arc};

use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use utoipa::ToSchema;

use super::{
    common::UploadResponse,
    ticket::{
        check_mint_limit, clamp_ttl_seconds, encrypted_client_ip, insert_owned_pending,
        mint_ticket, pinned_region_host, public_url_for, public_urls_for, reject_blocked_filename,
        ticket_ttl_secs,
    },
};
use crate::{
    db::{self, FileRecord},
    error::AppError,
    routes::{UserId, noscript},
    state::AppState,
};

pub(crate) fn direct_part_sizes(total: u64, n: usize) -> Vec<u64> {
    let step = total.div_ceil(n.max(1) as u64);
    (0..n)
        .map(|i| total.saturating_sub(i as u64 * step).min(step))
        .collect()
}

pub(crate) const DIRECT_MAX_PART_BYTES: u64 = 128 * 1024 * 1024;
pub(crate) const DIRECT_MAX_PARTS: usize = 64;

#[derive(serde::Deserialize, ToSchema)]
pub struct DirectParallelReserveRequest {
    pub filename: String,
    pub mime_type: String,
    pub file_size: u64,
    pub ttl_hours: Option<f64>,
    pub num_parts: Option<usize>,
}

#[derive(Serialize, ToSchema)]
pub struct DirectParallelPart {
    pub part_index: usize,
    pub part_id: String,
    pub part_size: u64,
    pub ticket: String,
    pub upload_url: String,
}

#[derive(Serialize, ToSchema)]
pub struct DirectParallelReserveResponse {
    pub ticket: String,
    pub file_id: String,
    pub url: String,
    pub raw_url: String,
    pub download_url: String,
    pub expires_at: i64,
    pub parts: Vec<DirectParallelPart>,
}

#[derive(serde::Deserialize, ToSchema)]
pub struct DirectParallelCompleteRequest {
    pub file_id: String,
    pub ticket: String,
    pub part_ids: Vec<String>,
}

#[utoipa::path(
    post,
    path = "/upload/direct/parallel/reserve",
    request_body = DirectParallelReserveRequest,
    responses(
        (status = 200, description = "Parallel direct upload tickets issued", body = DirectParallelReserveResponse),
        (status = 400, description = "Invalid request"),
        (status = 413, description = "File or part size exceeds limits"),
    ),
    tag = "Uploads",
)]
#[tracing::instrument(skip_all)]
pub async fn direct_parallel_reserve_handler(
    State(state): State<Arc<AppState>>,
    UserId(user_id): UserId,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<DirectParallelReserveRequest>,
) -> Result<Json<DirectParallelReserveResponse>, AppError> {
    let (default_ttl, allowed_ttl, danger, max_file_size, public_juicehost_url) = {
        let jh = state.juicehost_config_or_refresh().await?;
        (
            jh.default_ttl_hours,
            jh.allowed_ttl_hours.clone(),
            jh.danger_level,
            jh.max_file_size_bytes,
            state.config.public_juicehost_url.clone(),
        )
    };

    if body.file_size == 0 || body.file_size > max_file_size {
        return Err(AppError::PayloadTooLarge);
    }
    reject_blocked_filename(&body.filename, danger)?;

    let min_parts = (body.file_size.div_ceil(DIRECT_MAX_PART_BYTES) as usize).max(2);
    if min_parts > DIRECT_MAX_PARTS {
        return Err(AppError::PayloadTooLarge);
    }
    let mut num_parts = body
        .num_parts
        .unwrap_or_else(|| (body.file_size.div_ceil(32 * 1024 * 1024) as usize).clamp(2, 32));
    num_parts = num_parts.clamp(2, DIRECT_MAX_PARTS).max(min_parts);
    if num_parts > DIRECT_MAX_PARTS {
        return Err(AppError::PayloadTooLarge);
    }
    let part_sizes = direct_part_sizes(body.file_size, num_parts);

    check_mint_limit(&state, &headers, addr)?;

    let now = chrono::Utc::now().timestamp();
    let ttl_secs = ticket_ttl_secs(&state, body.file_size, 900);
    let ttl_seconds = clamp_ttl_seconds(&allowed_ttl, default_ttl, body.ttl_hours);
    let file_id = nanoid::nanoid!(8);
    let delete_token = uuid::Uuid::new_v4().to_string();
    let encrypted_ip = encrypted_client_ip(&state, &headers, addr);

    let region_juicehost = pinned_region_host(&state, &headers, addr);
    let record = FileRecord::new(
        file_id.clone(),
        body.filename.clone(),
        body.mime_type.clone(),
        body.file_size as i64,
        delete_token.clone(),
        now,
        now + ttl_seconds,
        encrypted_ip,
        region_juicehost.clone(),
    );
    let record = insert_owned_pending(&state, &user_id, record, "own_direct_parallel_upload").await?;

    let upload_base = region_juicehost
        .as_deref()
        .unwrap_or_else(|| public_juicehost_url.trim_end_matches('/'));
    let mut parts = Vec::with_capacity(num_parts);
    for (i, part_size) in part_sizes.iter().enumerate() {
        let part_id = nanoid::nanoid!(8);
        let ticket = mint_ticket(
            &state,
            "browser",
            &user_id,
            &part_id,
            &body.filename,
            &body.mime_type,
            *part_size,
            &delete_token,
            now,
            ttl_secs,
        )?;
        parts.push(DirectParallelPart {
            part_index: i,
            part_id: part_id.clone(),
            part_size: *part_size,
            ticket,
            upload_url: format!("{upload_base}/internal/file/upload/{part_id}"),
        });
    }

    let parent_ticket = mint_ticket(
        &state,
        "browser",
        &user_id,
        &file_id,
        &body.filename,
        &body.mime_type,
        body.file_size,
        &delete_token,
        now,
        ttl_secs,
    )?;

    let (public_url, raw_url, download_url) = public_urls_for(&state, &record);
    Ok(Json(DirectParallelReserveResponse {
        ticket: parent_ticket,
        file_id,
        url: public_url,
        raw_url,
        download_url,
        expires_at: record.expires_at,
        parts,
    }))
}

#[utoipa::path(
    post,
    path = "/upload/direct/parallel/complete",
    request_body = DirectParallelCompleteRequest,
    responses(
        (status = 200, description = "Parallel direct upload completed", body = UploadResponse),
        (status = 401, description = "Invalid upload ticket"),
        (status = 409, description = "Parts missing or size mismatch"),
    ),
    tag = "Uploads",
)]
#[tracing::instrument(skip_all)]
pub async fn direct_parallel_complete_handler(
    State(state): State<Arc<AppState>>,
    UserId(user_id): UserId,
    headers: HeaderMap,
    Json(body): Json<DirectParallelCompleteRequest>,
) -> Result<Response, AppError> {
    use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode as jwt_decode};
    let mut validation = Validation::new(Algorithm::HS256);
    validation.set_required_spec_claims(&["exp"]);
    validation.set_issuer(&["juiceback-ticket"]);
    let ticket = jwt_decode::<serde_json::Value>(
        &body.ticket,
        &DecodingKey::from_secret(state.config.ticket_jwt_secret.as_bytes()),
        &validation,
    )
    .map_err(|_| AppError::Unauthorized("Invalid upload ticket".into()))?
    .claims;
    let ticket_file_id = ticket.get("file_id").and_then(|v| v.as_str()).unwrap_or("");
    let ticket_user_id = ticket.get("user_id").and_then(|v| v.as_str()).unwrap_or("");
    if ticket_file_id != body.file_id || ticket_user_id != user_id {
        return Err(AppError::Unauthorized(
            "Upload ticket does not match session".into(),
        ));
    }

    let file_id = body.file_id;
    let lookup_id = file_id.clone();
    let file = state
        .db_call("get_direct_parallel_upload", move |db| db::get_file(db, &lookup_id))
        .await?
        .ok_or(AppError::NotFound)?;
    let owner_id = user_id.clone();
    let owned_id = file_id.clone();
    let owned = state
        .db_call("verify_direct_parallel_upload_owner", move |db| {
            db::client_owns_file(db, &owner_id, &owned_id)
        })
        .await?;
    if !owned {
        return Err(AppError::Unauthorized(
            "Upload is not owned by this session".into(),
        ));
    }

    let (public_url, raw_url, download_url) = public_urls_for(&state, &file);
    let flag_protected = file.is_protected();
    let flag_encrypted = file.is_encrypted;
    let ready_response = || {
        let cookie_hdr = noscript::file_cookie_header(
            &headers,
            &noscript::FileCookieEntry {
                id: file_id.clone(),
                token: file.delete_token.clone(),
            },
        );
        let mut resp = Json(UploadResponse {
            id: file_id.clone(),
            url: public_url.clone(),
            raw_url: raw_url.clone(),
            download_url: download_url.clone(),
            filename: file.filename.clone(),
            size_bytes: file.size_bytes,
            mime_type: file.mime_type.clone(),
            expires_at: file.expires_at,
            delete_token: String::new(),
            status: "ready".into(),
            protected: flag_protected,
            is_encrypted: flag_encrypted,
        })
        .into_response();
        resp.headers_mut().insert(
            axum::http::header::SET_COOKIE,
            cookie_hdr,
        );
        resp
    };
    if file.status != "uploading" {
        return Ok(ready_response());
    }

    if body.part_ids.len() < 2
        || body.part_ids.len() > DIRECT_MAX_PARTS
        || body.part_ids.iter().any(|id| !crate::utils::is_valid_id(id))
    {
        return Err(AppError::BadRequest("invalid part list".into()));
    }
    let mut seen = std::collections::HashSet::new();
    if body.part_ids.iter().any(|id| !seen.insert(id)) {
        return Err(AppError::BadRequest("duplicate part id".into()));
    }
    let expected = direct_part_sizes(file.size_bytes as u64, body.part_ids.len());
    let mut checked: Vec<String> = Vec::with_capacity(body.part_ids.len());
    for (part_id, want) in body.part_ids.iter().zip(expected.iter()) {
        let stored = crate::storage_client::stat_file_on_juicehost(
            &state,
            part_id,
            file.storage_host.as_deref(),
            Some(&file.delete_token),
        )
        .await
        .map_err(AppError::JuicehostUnreachable)?
        .ok_or_else(|| AppError::Conflict(format!("part {part_id} has not been uploaded yet")))?;
        if stored != *want {
            return Err(AppError::Conflict(format!(
                "part {part_id} size mismatch: expected {want} bytes, got {stored}"
            )));
        }
        checked.push(part_id.clone());
    }

    crate::storage_client::concat_files(
        &state,
        &file_id,
        &file.filename,
        &checked,
        file.storage_host.as_deref(),
        Some(&file.delete_token),
    )
    .await
    .map_err(|e| AppError::Internal(format!("concat failed: {e}")))?;

    let filename = file.filename.clone();
    let mime_type = file.mime_type.clone();
    let size_bytes = file.size_bytes;
    let reservation_token = file.delete_token.clone();
    let update_id = file_id.clone();
    match state
        .db_call("complete_direct_parallel_upload", move |db| {
            db::finish_reservation(
                db,
                &filename,
                &mime_type,
                size_bytes,
                &update_id,
                &reservation_token,
                None,
            )
        })
        .await?
    {
        db::FinishReservationResult::Completed(_) => {}
        db::FinishReservationResult::NotFound => return Err(AppError::NotFound),
        db::FinishReservationResult::NotUploading => {
            return Ok(ready_response());
        }
        db::FinishReservationResult::InvalidToken => {
            return Err(AppError::Unauthorized("invalid upload ownership".into()))
        }
    }

    Ok(ready_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_sizes_cover_file_exactly() {
        for (total, n) in [(100, 3), (1, 2), (64 * 1024 * 1024, 2), (500, 64)] {
            let sizes = direct_part_sizes(total, n);
            assert_eq!(sizes.len(), n);
            assert_eq!(sizes.iter().sum::<u64>(), total);
        }
    }

    #[test]
    fn part_sizes_even_split() {
        assert_eq!(direct_part_sizes(100, 4), vec![25, 25, 25, 25]);
    }

    #[test]
    fn part_sizes_last_part_takes_remainder() {
        assert_eq!(direct_part_sizes(100, 3), vec![34, 34, 32]);
    }

    #[test]
    fn part_sizes_single_part() {
        assert_eq!(direct_part_sizes(7, 1), vec![7]);
    }
}
