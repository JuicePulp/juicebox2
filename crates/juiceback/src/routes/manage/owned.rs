use std::sync::Arc;

use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    db, db::ClientFileRecord, error::AppError, routes::UserId, state::AppState,
    utils::constant_time_eq,
};

use super::files::FileInfoResponse;

#[derive(Deserialize, ToSchema)]
pub struct OwnedFilesRequest {
    pub pairs: Vec<FilePair>,
}

#[derive(Deserialize, ToSchema)]
pub struct FilePair {
    pub id: String,
    pub token: String,
}

#[derive(Serialize, ToSchema)]
pub struct OwnedFilesResponse {
    pub files: Vec<FileInfoResponse>,
}

#[utoipa::path(
    post,
    path = "/api/owned-files",
    request_body(content = OwnedFilesRequest, description = "List of file IDs and delete tokens you own"),
    responses(
        (status = 200, description = "List of files you own that havent expired yet", body = OwnedFilesResponse),
    ),
    tag = "Files",
)]
pub async fn owned_files_handler(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<OwnedFilesRequest>,
) -> Result<Json<OwnedFilesResponse>, AppError> {
    let now = chrono::Utc::now().timestamp();

    let ids: Vec<String> = payload.pairs.iter().map(|p| p.id.clone()).collect();
    let token_map: std::collections::HashMap<String, &str> = payload
        .pairs
        .iter()
        .map(|p| (p.id.clone(), p.token.as_str()))
        .collect();

    let records = state
        .db_call("get_files_by_ids", move |db| db::get_files_by_ids(db, &ids))
        .await?;

    let files: Vec<FileInfoResponse> = records
        .into_iter()
        .filter(|r| {
            r.expires_at >= now
                && token_map
                    .get(&r.id)
                    .is_some_and(|t| constant_time_eq(&r.delete_token, t))
        })
        .map(|r| FileInfoResponse::from_record(r, &state.config.public_base_url))
        .collect();

    Ok(Json(OwnedFilesResponse { files }))
}

#[derive(Deserialize, ToSchema)]
pub struct ClientFilesRequest {
    pub files: Vec<ClientFileRecord>,
}

#[derive(Serialize, ToSchema)]
pub struct ClientFilesResponse {
    pub files: Vec<ClientFileRecord>,
}

const CLIENT_FILES_MAX: usize = 500;

#[utoipa::path(
    post,
    path = "/api/client-files",
    request_body(content = ClientFilesRequest, description = "The client's full upload list (id + delete token + display metadata)"),
    responses(
        (status = 200, description = "Registry replaced", body = ClientFilesResponse),
        (status = 400, description = "Too many files"),
    ),
    tag = "Files",
)]
pub async fn put_client_files_handler(
    State(state): State<Arc<AppState>>,
    UserId(user_id): UserId,
    Json(payload): Json<ClientFilesRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    if payload.files.len() > CLIENT_FILES_MAX {
        return Err(AppError::BadRequest(format!(
            "Too many files: {} (max {CLIENT_FILES_MAX})",
            payload.files.len()
        )));
    }
    let key = user_id;
    let count = state
        .db_call("import_verified_client_files", move |db| {
            db::import_verified_client_files(db, &key, &payload.files)
        })
        .await?;
    Ok(Json(serde_json::json!({ "count": count })))
}

#[utoipa::path(
    get,
    path = "/api/client-files",
    responses(
        (status = 200, description = "The client's registered files", body = ClientFilesResponse),
    ),
    tag = "Files",
)]
pub async fn list_client_files_handler(
    State(state): State<Arc<AppState>>,
    UserId(user_id): UserId,
) -> Result<Json<ClientFilesResponse>, AppError> {
    let key = user_id.clone();
    let files = state
        .db_call("list_client_files", move |db| {
            db::list_client_files(db, &key)
        })
        .await?;
    Ok(Json(ClientFilesResponse { files }))
}
