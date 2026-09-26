use std::sync::Arc;

use axum::{
    Form, Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    db,
    db::FileRecord,
    error::AppError,
    routes::{UserId, noscript},
    state::AppState,
    utils::constant_time_eq,
};

#[derive(Deserialize)]
pub struct RenameForm {
    pub token: String,
    pub custom_id: String,
}

#[derive(Serialize, ToSchema)]
pub struct FileInfoResponse {
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub uploaded_at: i64,
    pub expires_at: i64,
    pub url: String,
    pub storage_host: Option<String>,
    pub status: String,
}

impl FileInfoResponse {
    #[must_use]
    pub fn from_record(record: FileRecord, public_base_url: &str) -> Self {
        let url = crate::utils::public_url(
            public_base_url,
            record.storage_host.as_deref(),
            &record.id,
            &record.filename,
        );
        Self {
            id: record.id,
            filename: record.filename,
            mime_type: record.mime_type,
            size_bytes: record.size_bytes,
            uploaded_at: record.uploaded_at,
            expires_at: record.expires_at,
            url,
            storage_host: record.storage_host,
            status: record.status,
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct RenewResponse {
    pub id: String,
    pub url: String,
    pub filename: String,
    pub expires_at: i64,
    pub custom: bool,
}

#[derive(Deserialize, ToSchema, Default)]
pub struct RenewRequest {
    pub custom_id: Option<String>,
}

#[utoipa::path(
    get,
    path = "/file/{id}/info",
    params(
        ("id" = String, Path, description = "The file ID"),
    ),
    responses(
        (status = 200, description = "File metadata", body = FileInfoResponse),
        (status = 404, description = "File not found"),
        (status = 410, description = "File has expired"),
    ),
    tag = "Files",
)]
#[tracing::instrument(skip_all)]
pub async fn file_info_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<(HeaderMap, Json<FileInfoResponse>), AppError> {
    let id2 = id.clone();
    let record = state
        .db_call("resolve_file_info", move |db| {
            let resolved = db::resolve_alias(db, &id2)?.unwrap_or(id2);
            db::get_file(db, &resolved)
        })
        .await?
        .ok_or(AppError::NotFound)?;

    let now = chrono::Utc::now().timestamp();
    if record.expires_at < now {
        return Err(AppError::Gone);
    }

    let mut headers = HeaderMap::new();
    headers.insert(
        "cache-control",
        axum::http::HeaderValue::from_static("no-store"),
    );
    Ok((
        headers,
        Json(FileInfoResponse::from_record(
            record,
            &state.config.public_base_url,
        )),
    ))
}

#[utoipa::path(
    post,
    path = "/file/{id}/delete",
    params(
        ("id" = String, Path, description = "The file ID"),
    ),
    request_body(content_type = "application/x-www-form-urlencoded", description = "Form with a 'token' field containing the delete token"),
    responses(
        (status = 302, description = "Redirects to /files.html?deleted={id}"),
        (status = 403, description = "Invalid delete token"),
        (status = 404, description = "File not found"),
    ),
    tag = "Files",
)]
pub async fn delete_file_form_handler(
    State(state): State<Arc<AppState>>,
    UserId(user_id): UserId,
    Path(id): Path<String>,
    Form(_params): Form<noscript::DeleteRequest>,
) -> Result<Redirect, AppError> {
    delete_file_inner(&state, &user_id, &id, None).await?;
    Ok(Redirect::to(&format!("/files.html?deleted={id}")))
}

async fn delete_file_inner(
    state: &Arc<AppState>,
    user_id: &str,
    id: &str,
    compatibility_token: Option<&str>,
) -> Result<(), AppError> {
    let id2 = id.to_string();
    let record = state
        .db_call("get_file", move |db| db::get_file(db, &id2))
        .await?
        .ok_or(AppError::NotFound)?;

    let uid = user_id.to_string();
    let owned_id = id.to_string();
    let owned = state
        .db_call("client_owns_file", move |db| {
            db::client_owns_file(db, &uid, &owned_id)
        })
        .await?;
    let token_valid = compatibility_token
        .map(|token| constant_time_eq(&record.delete_token, token))
        .unwrap_or(false);
    if !owned && !token_valid {
        return Err(AppError::Forbidden(
            "file is not owned by this session".into(),
        ));
    }

    let host_opt = record.storage_host.as_deref();
    crate::storage_client::delete_file_on_juicehost(
        state,
        id,
        host_opt,
        Some(&record.delete_token),
    )
    .await
    .map_err(AppError::from_juicehost_error)?;

    let id3 = id.to_string();
    state
        .db_call("delete_file", move |db| db::delete_file(db, &id3))
        .await?;
    let owner = user_id.to_string();
    let owned_id = id.to_string();
    state
        .db_call("remove_client_file", move |db| {
            db::remove_client_file(db, &owner, &owned_id)
        })
        .await?;

    crate::cloudflare::purge_file(
        &state.config,
        id,
        &record.filename,
        record.storage_host.as_deref(),
    );

    tracing::info!("delete: id={id}");
    Ok(())
}

#[utoipa::path(
    delete,
    path = "/file/{id}",
    params(
        ("id" = String, Path, description = "The file ID"),
    ),
    request_body(content_type = "application/json", content = inline(serde_json::Value), description = "Set the x-delete-token header instead of a body"),
    responses(
        (status = 204, description = "File deleted"),
        (status = 403, description = "Invalid delete token"),
        (status = 404, description = "File not found"),
    ),
    tag = "Files",
)]
pub async fn delete_file_handler(
    State(state): State<Arc<AppState>>,
    UserId(user_id): UserId,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    let token = headers.get("x-delete-token").and_then(|v| v.to_str().ok());

    delete_file_inner(&state, &user_id, &id, token).await?;

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/file/{id}/renew",
    params(
        ("id" = String, Path, description = "The current file ID"),
    ),
    request_body(content = RenewRequest, description = "Optional custom_id and x-delete-token header"),
    responses(
        (status = 200, description = "File ID renewed", body = RenewResponse),
        (status = 400, description = "Invalid custom ID"),
        (status = 403, description = "Invalid delete token"),
        (status = 404, description = "File not found"),
        (status = 409, description = "Custom ID already taken"),
        (status = 410, description = "File has expired"),
    ),
    tag = "Files",
)]
pub async fn renew_file_id_handler(
    State(state): State<Arc<AppState>>,
    UserId(user_id): UserId,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Option<Json<RenewRequest>>,
) -> Result<Json<RenewResponse>, AppError> {
    let token = headers.get("x-delete-token").and_then(|v| v.to_str().ok());
    let id2 = id.clone();
    let record = state
        .db_call("get_file", move |db| db::get_file(db, &id2))
        .await?
        .ok_or(AppError::NotFound)?;

    let now = chrono::Utc::now().timestamp();
    if record.expires_at < now {
        return Err(AppError::Gone);
    }

    let owner = user_id.clone();
    let owner_file = id.clone();
    let owned = state
        .db_call("client_owns_file", move |db| {
            db::client_owns_file(db, &owner, &owner_file)
        })
        .await?;
    let token_valid = token
        .map(|token| constant_time_eq(&record.delete_token, token))
        .unwrap_or(false);
    if !owned && !token_valid {
        return Err(AppError::Forbidden(
            "file is not owned by this session".into(),
        ));
    }

    let custom_id_raw = body.and_then(|Json(r)| r.custom_id).unwrap_or_default();

    let (new_id, is_custom) = super::lifecycle::pick_new_id(&state, &custom_id_raw).await?;

    let updated = super::lifecycle::swap_ids_transactional(&state, &user_id, &id, &new_id).await?;

    if !updated {
        return Err(AppError::NotFound);
    }

    let host_opt = record.storage_host.as_deref();
    super::lifecycle::rename_on_host_with_rollback(
        &state,
        &id,
        &new_id,
        host_opt,
        &record.delete_token,
    )
    .await?;

    let url = crate::utils::public_url(
        &state.config.public_base_url,
        record.storage_host.as_deref(),
        &new_id,
        &record.filename,
    );

    crate::cloudflare::purge_file(
        &state.config,
        &id,
        &record.filename,
        record.storage_host.as_deref(),
    );

    tracing::info!(
        "renew: old_id={} new_id={} custom={}",
        id,
        new_id,
        is_custom
    );

    Ok(Json(RenewResponse {
        id: new_id,
        url,
        filename: record.filename,
        expires_at: record.expires_at,
        custom: is_custom,
    }))
}

pub async fn rename_file_form_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    UserId(user_id): UserId,
    Path(id): Path<String>,
    Form(form): Form<RenameForm>,
) -> Result<Response, AppError> {
    let id2 = id.clone();
    let record = state
        .db_call("get_file", move |db| db::get_file(db, &id2))
        .await?
        .ok_or(AppError::NotFound)?;

    let now = chrono::Utc::now().timestamp();
    if record.expires_at < now {
        return Err(AppError::Gone);
    }

    let owner = user_id.clone();
    let owner_file = id.clone();
    let owned = state
        .db_call("client_owns_file", move |db| {
            db::client_owns_file(db, &owner, &owner_file)
        })
        .await?;
    if !owned {
        return Err(AppError::Forbidden(
            "file is not owned by this session".into(),
        ));
    }

    let (new_id, _) = super::lifecycle::pick_new_id(&state, &form.custom_id).await?;

    let updated = super::lifecycle::swap_ids_transactional(&state, &user_id, &id, &new_id).await?;

    if !updated {
        return Err(AppError::NotFound);
    }

    let host_opt = record.storage_host.as_deref();
    super::lifecycle::rename_on_host_with_rollback(
        &state,
        &id,
        &new_id,
        host_opt,
        &record.delete_token,
    )
    .await?;

    let cookie_hdr =
        noscript::rename_file_cookie_header(&headers, &id, &new_id, &record.delete_token);

    tracing::info!("rename form: old_id={id} new_id={new_id}");
    let mut response = Redirect::to("/files.html?renamed=1").into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, cookie_hdr);
    Ok(response)
}

pub async fn resolve_alias_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(old_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    if state.config.juicehost_api_key.is_empty() {
        return Err(AppError::Unauthorized("API key not configured".into()));
    }
    let provided = headers
        .get("x-juicehost-api-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !constant_time_eq(provided, &state.config.juicehost_api_key) {
        return Err(AppError::Unauthorized("invalid API key".into()));
    }
    let old_id2 = old_id.clone();
    let resolved = state
        .db_call("resolve_alias", move |db| db::resolve_alias(db, &old_id2))
        .await?;

    match resolved {
        Some(new_id) if new_id != old_id => {
            let id = new_id.clone();
            let record = state
                .db_call("get_file", move |db| db::get_file(db, &id))
                .await?;
            if let Some(rec) = record {
                let url = crate::utils::public_url(
                    &state.config.public_base_url,
                    rec.storage_host.as_deref(),
                    &new_id,
                    &rec.filename,
                );
                Ok(Json(serde_json::json!({ "new_id": new_id, "url": url })))
            } else {
                Err(AppError::NotFound)
            }
        }
        _ => Err(AppError::NotFound),
    }
}
