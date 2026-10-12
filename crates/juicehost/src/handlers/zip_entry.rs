







use std::{collections::HashMap, sync::Arc};

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderValue, header},
    response::Response,
};
use bytes::Bytes;

use super::{
    shell::{GateDecision, ShellMode, gate_decision},
    zip_list,
};
use crate::{error::JuicehostError, state::AppState, storage::{self, valid_component}};


const MAX_ENTRY_BYTES: u64 = 32 * 1024 * 1024;

fn valid_entry_path(path: &str) -> bool {
    if path.is_empty() || path.len() > 1024 || path.starts_with('/') || path.contains('\\') {
        return false;
    }
    if path.split('/').any(|seg| seg.is_empty() || seg == "." || seg == "..") {
        return false;
    }
    !path.chars().any(|c| c.is_control())
}

fn inflate_deflated(data: &[u8]) -> Option<Vec<u8>> {
    use std::io::Read;
    let decoder = flate2::read::DeflateDecoder::new(data);
    let mut out = Vec::new();
    
    decoder
        .take(MAX_ENTRY_BYTES + 1)
        .read_to_end(&mut out)
        .ok()?;
    if out.len() as u64 > MAX_ENTRY_BYTES {
        return None;
    }
    Some(out)
}

#[tracing::instrument(skip_all)]
pub async fn zip_entry_file(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response<Body>, JuicehostError> {
    let entry_path = params.get("path").map(String::as_str).unwrap_or("");
    if !valid_component(&id) || !valid_entry_path(entry_path) {
        return Err(JuicehostError::BadRequest);
    }
    
    match gate_decision(&state, &id, ShellMode::View, None).await {
        GateDecision::Respond(_) => return Err(JuicehostError::Forbidden),
        GateDecision::HotRedirect(url) => {
            return Response::builder()
                .status(axum::http::StatusCode::FOUND)
                .header(header::LOCATION, &url)
                .header(header::CACHE_CONTROL, "no-store")
                .body(Body::empty())
                .map_err(|_| JuicehostError::Internal);
        }
        GateDecision::ServeLocal => {}
    }
    let meta = state
        .storage
        .stat(&id)
        .await
        .map_err(|_| JuicehostError::NotFound)?;
    let entries = zip_list::load_zip_entries(&state.storage, &id, meta.size)
        .await
        .ok_or(JuicehostError::NotFound)?;
    let entry = entries
        .iter()
        .find(|e| !e.is_dir && e.name == entry_path)
        .ok_or(JuicehostError::NotFound)?;
    let uncomp_size = entry.size.ok_or(JuicehostError::PayloadTooLarge)?;
    if uncomp_size > MAX_ENTRY_BYTES {
        return Err(JuicehostError::PayloadTooLarge);
    }
    
    let header = zip_list::read_range_capped(
        &state.storage,
        &id,
        entry.header_offset as u64,
        entry.header_offset as u64 + 29,
        30,
    )
    .await
    .ok_or(JuicehostError::NotFound)?;
    if header.len() < 30 || &header[0..4] != b"PK\x03\x04" {
        return Err(JuicehostError::NotFound);
    }
    let name_len = u16::from_le_bytes([header[26], header[27]]) as u64;
    let extra_len = u16::from_le_bytes([header[28], header[29]]) as u64;
    let data_start = entry.header_offset as u64 + 30 + name_len + extra_len;
    if entry.comp_size > MAX_ENTRY_BYTES {
        return Err(JuicehostError::PayloadTooLarge);
    }
    if entry.comp_size == 0 {
        if uncomp_size != 0 {
            return Err(JuicehostError::NotFound);
        }
        
        return file_response(entry_path, Bytes::new());
    }
    let data = zip_list::read_range_capped(
        &state.storage,
        &id,
        data_start,
        data_start + entry.comp_size - 1,
        MAX_ENTRY_BYTES,
    )
    .await
    .ok_or(JuicehostError::NotFound)?;
    let bytes = match entry.method {
        0 => data,
        8 => inflate_deflated(&data)
            .map(Bytes::from)
            .ok_or(JuicehostError::NotFound)?,
        
        _ => return Err(JuicehostError::NotFound),
    };
    file_response(entry_path, bytes)
}

fn file_response(entry_path: &str, bytes: Bytes) -> Result<Response<Body>, JuicehostError> {
    let ext = entry_path
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let mime = storage::guess_mime(&ext);
    let content_type =
        HeaderValue::from_str(&mime).map_err(|_| JuicehostError::Internal)?;
    Response::builder()
        .status(axum::http::StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, bytes.len())
        .header(header::CACHE_CONTROL, "no-store")
        .header(
            header::CONTENT_SECURITY_POLICY,
            "sandbox allow-downloads",
        )
        .body(Body::from(bytes))
        .map_err(|_| JuicehostError::Internal)
}
