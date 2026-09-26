use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerFile {
    pub id: String,
    #[serde(default)]
    pub filename: String,
    #[serde(default)]
    pub mime_type: String,
    #[serde(default)]
    pub size_bytes: u64,
    #[serde(default)]
    pub uploaded_at: i64,
    #[serde(default)]
    pub expires_at: i64,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub delete_token: String,
    pub storage_host: Option<String>,
}
