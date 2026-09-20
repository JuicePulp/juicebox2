use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
};
use serde::Serialize;
use utoipa::ToSchema;

use super::common::{AdminUser, ListParams};
use crate::{db, error::AppError, state::AppState};

#[derive(Serialize, ToSchema)]
pub struct AdminFileEntry {
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub uploaded_at: i64,
    pub expires_at: i64,
    pub uploader_ip_hash: Option<String>,
    pub url: String,
    pub storage_host: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct AdminFilesResponse {
    pub items: Vec<AdminFileEntry>,
    pub total: i64,
}

#[utoipa::path(
    get,
    path = "/api/admin/files",
    params(
        ("q" = String, Query, description = "Search query"),
        ("sort" = String, Query, description = "Sort column"),
        ("dir" = String, Query, description = "Sort direction"),
        ("offset" = i64, Query, description = "Offset"),
        ("limit" = i64, Query, description = "Limit"),
    ),
    responses(
        (status = 200, description = "Paginated file list", body = AdminFilesResponse),
        (status = 401, description = "Not authenticated"),
    ),
    security(),
    tag = "Admin",
)]
pub async fn list_files_handler(
    _admin: AdminUser,
    State(state): State<Arc<AppState>>,
    Query(params): Query<ListParams>,
) -> Result<Json<AdminFilesResponse>, AppError> {
    let search = params.q.clone();
    let sort = params.sort.clone();
    let dir = params.dir.clone();
    let offset = params.offset;
    let limit = params.limit.min(200);

    let (records, total) = state
        .db_call("list_files_paginated", move |db| {
            db::list_files_paginated(db, &search, &sort, &dir, offset, limit)
        })
        .await?;

    let files: Vec<AdminFileEntry> = records
        .into_iter()
        .map(|r| {
            let url = crate::utils::public_url(
                &state.config.public_base_url,
                &r.storage_host,
                &r.id,
                &r.filename,
            );
            // Raw IPs are never exposed: decrypt transiently only to derive
            // the truncated abuse hash, which is all the dashboard needs.
            let ip_hash = r
                .uploader_ip
                .as_ref()
                .and_then(|enc| crate::utils::decrypt_ip(enc, &state.config.ip_encryption_key))
                .map(|ip| {
                    let h = crate::utils::hash_ip_for_ban(&ip, &state.config.ip_pepper);
                    crate::utils::truncate_hash(&h).to_string()
                });
            AdminFileEntry {
                id: r.id,
                filename: r.filename,
                mime_type: r.mime_type,
                size_bytes: r.size_bytes,
                uploaded_at: r.uploaded_at,
                expires_at: r.expires_at,
                uploader_ip_hash: ip_hash,
                url,
                storage_host: r.storage_host,
            }
        })
        .collect();

    Ok(Json(AdminFilesResponse {
        items: files,
        total,
    }))
}

#[utoipa::path(
    delete,
    path = "/api/admin/file/{id}",
    params(
        ("id" = String, Path, description = "The file ID to delete"),
    ),
    responses(
        (status = 204, description = "File deleted"),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "File not found"),
    ),
    security(),
    tag = "Admin",
)]
pub async fn delete_file_handler(
    _admin: AdminUser,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    let id2 = id.clone();
    let record = state
        .db_call("get_file", move |db| db::get_file(db, &id2))
        .await?
        .ok_or(AppError::NotFound)?;

    let host_opt = record.storage_host.as_deref();
    crate::storage_client::delete_file_on_juicehost(
        &state,
        &id,
        host_opt,
        Some(&record.delete_token),
    )
    .await
    .map_err(AppError::from_juicehost_error)?;

    let id3 = id.clone();
    state
        .db_call("delete_file", move |db| db::delete_file(db, &id3))
        .await?;

    crate::cloudflare::purge_file(&state.config, &id, &record.filename, &record.storage_host);

    tracing::info!("admin delete: id={id}");

    Ok(StatusCode::NO_CONTENT)
}
