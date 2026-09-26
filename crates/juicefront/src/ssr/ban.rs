use std::net::IpAddr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BanStatus {
    pub banned: bool,
    pub reason: Option<String>,
    pub banned_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct BanResponse {
    #[serde(default)]
    banned: bool,
    reason: Option<String>,
    banned_at: Option<i64>,
}

pub async fn check_user_ban(
    http: &reqwest::Client,
    juiceback_url: &str,
    headers: &axum::http::HeaderMap,
    peer: IpAddr,
    trusted: &[juiceutils::proxy::IpCidr],
) -> BanStatus {
    let ip = juiceutils::proxy::client_ip(headers, peer, trusted).to_string();
    let url = format!("{juiceback_url}/api/ban-status?ip={}", urlencoding(&ip));
    let fetch = async {
        let response = http.get(&url).send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        response.json::<BanResponse>().await.ok()
    };
    match tokio::time::timeout(std::time::Duration::from_secs(3), fetch).await {
        Ok(Some(data)) if data.banned => BanStatus {
            banned: true,
            reason: data.reason,
            banned_at: data.banned_at,
        },
        _ => BanStatus::default(),
    }
}

fn urlencoding(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~:".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}
