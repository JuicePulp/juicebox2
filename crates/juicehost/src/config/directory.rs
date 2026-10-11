use std::path::PathBuf;

use crate::config::DirectoryFile;

#[derive(Debug)]
pub struct DirectorySettings {
    files_dir: PathBuf,
    backend_url: Option<String>,
    frontend_url: Option<String>,
}

impl DirectorySettings {
    #[must_use]
    pub const fn files_dir(&self) -> &PathBuf {
        &self.files_dir
    }

    #[must_use]
    pub fn backend_url(&self) -> Option<&str> {
        self.backend_url.as_deref()
    }

    #[must_use]
    pub fn frontend_url(&self) -> Option<&str> {
        self.frontend_url.as_deref()
    }

    #[must_use]
    pub fn load(file: &DirectoryFile) -> Self {
        let files_dir = file.files_dir.clone();
        // Explicit environment wins over the file (the TOML documents
        // `Env: BACKEND_URL / FRONTEND_URL`). Unset falls back to the
        // file; set-but-blank or "none" disables.
        let url_setting = |env_name: &str, file_value: Option<String>| match std::env::var(env_name)
        {
            Ok(raw) => {
                let trimmed = raw.trim();
                if trimmed.is_empty() || trimmed == "none" {
                    None
                } else {
                    Some(trimmed.trim_end_matches('/').to_string())
                }
            }
            Err(_) => file_value
                .filter(|s| !s.trim().is_empty() && s.trim() != "none")
                .map(|s| s.trim_end_matches('/').to_string()),
        };
        let backend_url = url_setting("BACKEND_URL", file.backend_url.clone());
        let frontend_url = url_setting("FRONTEND_URL", file.frontend_url.clone());
        Self {
            files_dir,
            backend_url,
            frontend_url,
        }
    }
}
