





use std::sync::Arc;

use bytes::Bytes;
use futures::StreamExt;

use super::{client::StorageClient, target::require_juicehost_url};
use crate::state::AppState;












pub async fn download_ciphertext(
    state: &Arc<AppState>,
    id: &str,
    host: Option<&str>,
    capability: Option<&str>,
    range: Option<(u64, u64)>,
) -> Result<
    (
        impl futures::Stream<Item = Result<Bytes, String>> + Send + 'static,
        bool,
    ),
    String,
> {
    let custom_host = host.map(str::trim).filter(|h| !h.is_empty());
    if custom_host.is_none() {
        require_juicehost_url(&state.config.juicehost_url).map_err(|e| e.to_string())?;
    }
    let client = StorageClient::resolve(state, custom_host).await?;
    let headers = client.headers_with(capability)?;

    let mut request = client
        .http()
        .get(format!(
            "{}/internal/file/{}/ciphertext",
            client.base_url(),
            id
        ))
        .headers(headers);
    if let Some((start, len)) = range.filter(|(_, len)| *len > 0) {
        request = request.header(
            "range",
            format!(
                "bytes={}-{}",
                start,
                start.saturating_add(len).saturating_sub(1)
            ),
        );
    }

    let resp = request
        .send()
        .await
        .map_err(|e| format!("juicehost ciphertext request error: {e}"))?;
    let ranged = resp.status() == reqwest::StatusCode::PARTIAL_CONTENT;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "juicehost ciphertext failed: status={status} body={body}"
        ));
    }
    let stream = resp
        .bytes_stream()
        .map(|chunk| chunk.map_err(|e| format!("ciphertext stream error: {e}")));
    Ok((stream, ranged))
}
