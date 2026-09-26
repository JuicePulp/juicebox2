use std::sync::Arc;

use super::target::{JuicehostTarget, resolve_juicehost_target};
use crate::state::AppState;

pub struct StorageClient {
    target: JuicehostTarget,
}

impl StorageClient {
    pub async fn resolve(state: &Arc<AppState>, host: Option<&str>) -> Result<Self, String> {
        let target = resolve_juicehost_target(state, host)
            .await
            .map_err(|e| e.to_string())?;
        Ok(Self { target })
    }

    pub fn base_url(&self) -> &str {
        &self.target.base_url
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.target.client
    }

    pub fn headers_with(
        &self,
        capability: Option<&str>,
    ) -> Result<reqwest::header::HeaderMap, String> {
        let mut headers = self.target.headers.clone();
        if let Some(cap) = capability {
            headers.insert(
                "x-juicehost-file-capability",
                cap.parse::<reqwest::header::HeaderValue>()
                    .map_err(|_| "invalid file capability".to_string())?,
            );
        }
        Ok(headers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_header_inserted() {
        let target = JuicehostTarget {
            base_url: "http://127.0.0.1:6402".to_string(),
            client: reqwest::Client::new(),
            headers: reqwest::header::HeaderMap::new(),
        };
        let client = StorageClient { target };
        let headers = client.headers_with(Some("tok123")).unwrap();
        assert_eq!(
            headers
                .get("x-juicehost-file-capability")
                .and_then(|v| v.to_str().ok()),
            Some("tok123")
        );
        assert!(
            client
                .headers_with(None)
                .unwrap()
                .get("x-juicehost-file-capability")
                .is_none()
        );
    }

    #[test]
    fn bad_capability_rejected() {
        let target = JuicehostTarget {
            base_url: "http://127.0.0.1:6402".to_string(),
            client: reqwest::Client::new(),
            headers: reqwest::header::HeaderMap::new(),
        };
        let client = StorageClient { target };
        assert!(client.headers_with(Some("not\nvalid")).is_err());
    }
}
