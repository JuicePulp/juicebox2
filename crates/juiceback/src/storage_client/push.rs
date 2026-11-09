use std::sync::Arc;

use crate::state::AppState;

fn normalized_origin(value: &str) -> Option<String> {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    url::Url::parse(&with_scheme)
        .ok()
        .map(|url| url.origin().ascii_serialization())
}

/// `None` when `host` is the default, so the push uses the local connection.
fn local_push_host<'a>(state: &Arc<AppState>, host: Option<&'a str>) -> Option<&'a str> {
    let host = host.map(str::trim).filter(|host| !host.is_empty())?;
    let Some(default_host) = state.config.default_storage_host.as_deref() else {
        return Some(host);
    };
    match (normalized_origin(default_host), normalized_origin(host)) {
        (Some(default_origin), Some(host_origin)) if default_origin == host_origin => None,
        _ => Some(host),
    }
}

#[tracing::instrument(skip_all)]
pub async fn push_file_streaming(
    state: &Arc<AppState>,
    id: &str,
    filename: &str,
    mime_type: &str,
    chunk_rx: tokio::sync::mpsc::Receiver<Result<bytes::Bytes, String>>,
    host: Option<String>,
    capability: Option<&str>,
    live_total: Option<u64>,
) -> Result<(), String> {
    let custom_host = host.as_deref().map(str::trim).filter(|h| !h.is_empty());
    let client =
        super::client::StorageClient::resolve(state, local_push_host(state, custom_host)).await?;
    let target_base = client.base_url().to_string();

    let encoded_fn =
        percent_encoding::utf8_percent_encode(filename, percent_encoding::NON_ALPHANUMERIC)
            .to_string();
    let url = format!("{}/internal/file/stream/{}/{}", target_base, id, encoded_fn);
    let stream = tokio_stream::wrappers::ReceiverStream::new(chunk_rx);
    let body = reqwest::Body::wrap_stream(stream);

    let req_start = std::time::Instant::now();
    let mut request = client
        .http()
        .post(&url)
        .header(juiceproto::headers::MIME_TYPE, mime_type)
        .headers(client.headers_with(None)?);
    if let Some(capability) = capability {
        request = request.header(juiceproto::headers::FILE_CAPABILITY, capability);
    }
    if let Some(total) = live_total {
        request = request.header(juiceproto::headers::LIVE_TOTAL, total.to_string());
    }
    let resp = request
        .body(body)
        .send()
        .await
        .map_err(|e| format!("juicehost streaming push error: {e}"))?;
    let send_time = req_start.elapsed();

    if resp.status().is_success() {
        tracing::debug!("push_stream: id={} url={} send={:?}", id, url, send_time,);
        tracing::info!("juicehost streaming push: id={id} ok");
        Ok(())
    } else {
        let status = resp.status();
        let resp_start = std::time::Instant::now();
        let body_text = resp.text().await.unwrap_or_default();
        let resp_time = resp_start.elapsed();
        tracing::warn!(
            "push_stream error: id={} url={} send={:?} resp_read={:?} status={}",
            id,
            url,
            send_time,
            resp_time,
            status.as_u16(),
        );
        Err(format!(
            "{} (url={})",
            juiceproto::error::format_service_error(status.as_u16(), &body_text),
            url
        ))
    }
}

#[tracing::instrument(skip_all)]
pub async fn push_file_to_juicehost(
    state: &Arc<AppState>,
    id: &str,
    filename: &str,
    mime_type: &str,
    data: Vec<u8>,
    host: Option<&str>,
    capability: Option<&str>,
) -> Result<(), String> {
    let client = super::client::StorageClient::resolve(state, local_push_host(state, host)).await?;

    let build_start = std::time::Instant::now();
    let file_part = reqwest::multipart::Part::bytes(data)
        .file_name(filename.to_string())
        .mime_str(mime_type)
        .map_err(|e| format!("mime error: {e}"))?;

    let form = reqwest::multipart::Form::new()
        .text("id", id.to_string())
        .text("filename", filename.to_string())
        .part("file", file_part);
    let build_time = build_start.elapsed();

    let send_start = std::time::Instant::now();
    let url = format!("{}/internal/file", client.base_url());
    let mut request = client.http().post(&url).headers(client.headers_with(None)?);
    if let Some(capability) = capability {
        request = request.header(juiceproto::headers::FILE_CAPABILITY, capability);
    }
    let resp = request
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("juicehost push error: {e}"))?;
    let send_time = send_start.elapsed();

    tracing::debug!(
        "juicehost push: id={} build={:?} send={:?} status={}",
        id,
        build_time,
        send_time,
        resp.status()
    );

    if resp.status().is_success() {
        tracing::info!("juicehost push: id={id} ok");
        Ok(())
    } else {
        let body_start = std::time::Instant::now();
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        let body_time = body_start.elapsed();
        tracing::warn!(
            "juicehost push error: id={} url={} body_read={:?} status={}",
            id,
            url,
            body_time,
            status.as_u16(),
        );
        Err(format!(
            "{} (url={})",
            juiceproto::error::format_service_error(status.as_u16(), &body),
            url
        ))
    }
}
