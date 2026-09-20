use base64::Engine;

use crate::error::AppError;

pub(crate) fn validate_parallel_metadata(
    session_id: Option<&str>,
    part_index: Option<usize>,
    total_parts: Option<usize>,
) -> Result<Option<(&str, usize, usize)>, AppError> {
    match (session_id, part_index, total_parts) {
        (None, None, None) => Ok(None),
        (Some(sid), Some(index), Some(total))
            if !sid.is_empty()
                && sid.len() <= 128
                && total > 0
                && total <= crate::constants::MAX_TUS_PARALLEL_PARTS
                && index < total =>
        {
            Ok(Some((sid, index, total)))
        }
        _ => Err(AppError::BadRequest(
            "invalid parallel upload metadata".into(),
        )),
    }
}

pub(crate) fn parse_parallel_number(value: Option<&str>) -> Result<Option<usize>, AppError> {
    value
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| AppError::BadRequest("invalid parallel upload metadata".into()))
        })
        .transpose()
}

pub(crate) fn checked_upload_offset(
    offset: u64,
    chunk_len: u64,
    total: u64,
) -> Result<u64, AppError> {
    let new_offset = offset
        .checked_add(chunk_len)
        .ok_or(AppError::PayloadTooLarge)?;
    if new_offset > total {
        return Err(AppError::PayloadTooLarge);
    }
    Ok(new_offset)
}

pub(crate) fn parse_tus_metadata(
    header: Option<&axum::http::HeaderValue>,
) -> Vec<(String, String)> {
    let raw = match header.and_then(|v| v.to_str().ok()) {
        Some(v) => v,
        None => return vec![],
    };
    let mut pairs = Vec::new();
    for pair in raw.split(',') {
        let pair = pair.trim();
        if let Some(eq) = pair.find(' ') {
            let key = pair[..eq].to_string();
            let value_b64 = pair[eq + 1..].trim();
            if let Ok(decoded) =
                base64::engine::general_purpose::STANDARD.decode(value_b64.as_bytes())
            {
                if let Ok(value) = String::from_utf8(decoded) {
                    pairs.push((key, value));
                }
            }
        }
    }
    pairs
}

pub(crate) fn find_meta<'a>(metadata: &'a [(String, String)], key: &str) -> Option<&'a str> {
    metadata
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}
