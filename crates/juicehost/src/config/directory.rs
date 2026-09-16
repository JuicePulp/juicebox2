use std::path::PathBuf;

use crate::config::DirsFile;

/// Local directory and peer URL settings.
#[derive(Debug)]
pub struct DirectorySettings {
    files_dir: PathBuf,
    backend_url: Option<String>,
    frontend_url: Option<String>,
}

impl DirectorySettings {
    pub const fn files_dir(&self) -> &PathBuf {
        &self.files_dir
    }

    pub const fn backend_url(&self) -> Option<&String> {
        self.backend_url.as_ref()
    }

    pub const fn frontend_url(&self) -> Option<&String> {
        self.frontend_url.as_ref()
    }

    pub fn load(file: &DirsFile) -> Self {
        let files_dir = std::env::var("FILES_DIR")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map_or_else(|| file.files_dir.clone(), PathBuf::from);
        let backend_url = std::env::var("BACKEND_URL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| file.backend_url.clone())
            .filter(|s| !s.trim().is_empty() && s.trim() != "none")
            .map(|s| s.trim_end_matches('/').to_string());
        let frontend_url = std::env::var("FRONTEND_URL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| file.frontend_url.clone())
            .filter(|s| !s.trim().is_empty() && s.trim() != "none")
            .map(|s| s.trim_end_matches('/').to_string());
        Self {
            files_dir,
            backend_url,
            frontend_url,
        }
    }
}
