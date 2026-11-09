use std::{collections::HashSet, sync::Arc};

use axum::{Json, extract::State, http::StatusCode};
use serde::Deserialize;
use utoipa::ToSchema;

use crate::{auth::InternalApiKey, db, error::AppError, state::AppState};

#[derive(Deserialize, ToSchema)]
pub struct HitRequest {
    pub file_id: String,

    pub kind: String,

    pub ip: String,
    #[serde(default)]
    pub surface: Option<String>,
    #[serde(default = "default_hit_count")]
    pub count: u64,
}

const fn default_hit_count() -> u64 {
    1
}

const MAX_HIT_BATCH: usize = 1_000;
const MAX_HIT_COUNT: u64 = 10_000;

fn validate_hit(body: &HitRequest) -> Result<db::HitKind, AppError> {
    let kind =
        db::HitKind::from_str(&body.kind).ok_or(AppError::BadRequest("unknown hit kind".into()))?;
    if body.file_id.is_empty() || body.file_id.len() > 64 || body.ip.is_empty() {
        return Err(AppError::BadRequest("invalid hit payload".into()));
    }
    if body.count == 0 || body.count > MAX_HIT_COUNT {
        return Err(AppError::BadRequest("invalid hit count".into()));
    }
    Ok(kind)
}

#[utoipa::path(
    post,
    path = "/internal/stats/hit",
    request_body = HitRequest,
    responses(
        (status = 204, description = "Hit recorded"),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Invalid API key"),
    ),
    security(("api_key" = [])),
    tag = "Internal",
)]
pub async fn hit_handler(
    State(state): State<Arc<AppState>>,
    _key: InternalApiKey,
    Json(body): Json<HitRequest>,
) -> Result<StatusCode, AppError> {
    let kind = validate_hit(&body)?;
    let ip_hash = crate::utils::hash_ip_for_ban(&body.ip, &state.config.ip_pepper);
    let now = chrono::Utc::now().timestamp();
    let hit_file_id = body.file_id.clone();
    let count = body.count;
    state
        .db_call("record_file_hit", move |db| {
            db::record_file_hit_count(db, &hit_file_id, &ip_hash, kind, count, now)
        })
        .await?;
    if kind == db::HitKind::View {
        let surface = body.surface.as_deref().unwrap_or("view").to_string();
        if surface == "preview" || surface == "view" {
            let embed_state = Arc::clone(&state);
            let embed_id = body.file_id.clone();
            tokio::spawn(async move {
                crate::jobs::promo::maybe_embed_promote(&embed_state, &embed_id).await;
            });
        }
    }
    {
        let geo_state = Arc::clone(&state);
        let geo_ip = body.ip.clone();
        tokio::spawn(async move {
            let Ok(addr) = geo_ip.parse::<std::net::IpAddr>() else {
                return;
            };
            if !crate::storage_client::is_public_ip(addr) {
                return;
            }
            let Some(enc) = crate::utils::encrypt_ip(&geo_ip, &geo_state.config.ip_encryption_key)
            else {
                return;
            };
            let hash = juiceutils::ban::hash_ip_for_ban(&geo_ip, &geo_state.config.ip_pepper);
            let now = chrono::Utc::now().timestamp();
            let _ = geo_state
                .db_call("geo_enqueue", move |db| {
                    db::geo::enqueue_geo_ip(db, &enc, &hash, now)
                })
                .await;
        });
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Clone)]
struct PreparedHit {
    file_id: String,
    ip: String,
    ip_hash: String,
    kind: db::HitKind,
    surface: Option<String>,
    count: u64,
}

#[derive(Deserialize, ToSchema)]
pub struct HitBatchRequest {
    pub hits: Vec<HitRequest>,
}

#[utoipa::path(
    post,
    path = "/internal/stats/hit-batch",
    request_body = HitBatchRequest,
    responses(
        (status = 204, description = "Hits recorded"),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Invalid API key"),
    ),
    security(("api_key" = [])),
    tag = "Internal",
)]
pub async fn hit_batch_handler(
    State(state): State<Arc<AppState>>,
    _key: InternalApiKey,
    Json(body): Json<HitBatchRequest>,
) -> Result<StatusCode, AppError> {
    if body.hits.is_empty() || body.hits.len() > MAX_HIT_BATCH {
        return Err(AppError::BadRequest("invalid hit batch size".into()));
    }
    let now = chrono::Utc::now().timestamp();
    let mut prepared = Vec::with_capacity(body.hits.len());
    for hit in body.hits {
        let kind = validate_hit(&hit)?;
        let ip_hash = crate::utils::hash_ip_for_ban(&hit.ip, &state.config.ip_pepper);
        prepared.push(PreparedHit {
            file_id: hit.file_id,
            ip: hit.ip,
            ip_hash,
            kind,
            surface: hit.surface,
            count: hit.count,
        });
    }

    let db_hits = prepared.clone();
    state
        .db_transaction("record_file_hits", move |tx| {
            for hit in &db_hits {
                db::record_file_hit_count(
                    tx,
                    &hit.file_id,
                    &hit.ip_hash,
                    hit.kind,
                    hit.count,
                    now,
                )?;
            }
            Ok(())
        })
        .await?;

    let mut promoted = HashSet::new();
    let mut geolocated = HashSet::new();
    for hit in prepared {
        if hit.kind == db::HitKind::View
            && matches!(hit.surface.as_deref().unwrap_or("view"), "preview" | "view")
            && promoted.insert(hit.file_id.clone())
        {
            let embed_state = Arc::clone(&state);
            let embed_id = hit.file_id.clone();
            tokio::spawn(async move {
                crate::jobs::promo::maybe_embed_promote(&embed_state, &embed_id).await;
            });
        }
        if geolocated.insert(hit.ip.clone()) {
            let geo_state = Arc::clone(&state);
            let geo_ip = hit.ip;
            tokio::spawn(async move {
                let Ok(addr) = geo_ip.parse::<std::net::IpAddr>() else {
                    return;
                };
                if !crate::storage_client::is_public_ip(addr) {
                    return;
                }
                let Some(enc) =
                    crate::utils::encrypt_ip(&geo_ip, &geo_state.config.ip_encryption_key)
                else {
                    return;
                };
                let hash = juiceutils::ban::hash_ip_for_ban(&geo_ip, &geo_state.config.ip_pepper);
                let now = chrono::Utc::now().timestamp();
                let _ = geo_state
                    .db_call("geo_enqueue", move |db| {
                        db::geo::enqueue_geo_ip(db, &enc, &hash, now)
                    })
                    .await;
            });
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, ToSchema)]
pub struct VisitRequest {
    pub ip: String,
}

#[utoipa::path(
    post,
    path = "/internal/stats/visit",
    request_body = VisitRequest,
    responses(
        (status = 204, description = "Visit recorded"),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Invalid API key"),
    ),
    security(("api_key" = [])),
    tag = "Internal",
)]
pub async fn visit_handler(
    State(state): State<Arc<AppState>>,
    _key: InternalApiKey,
    Json(body): Json<VisitRequest>,
) -> Result<StatusCode, AppError> {
    if body.ip.is_empty() {
        return Err(AppError::BadRequest("invalid visit payload".into()));
    }
    let ip_hash = crate::utils::hash_ip_for_ban(&body.ip, &state.config.ip_pepper);
    let now = chrono::Utc::now().timestamp();
    state
        .db_call("record_site_visit", move |db| {
            db::record_site_visit(db, &ip_hash, now)
        })
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
