use std::path::PathBuf;

use crate::config::DirectoryFile;

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

    pub fn load(file: &DirectoryFile) -> Self {
        let files_dir = file.files_dir.clone();
        let backend_url = file
            .backend_url
            .clone()
            .filter(|s| !s.trim().is_empty() && s.trim() != "none")
            .map(|s| s.trim_end_matches('/').to_string());
        let frontend_url = file
            .frontend_url
            .clone()
            .filter(|s| !s.trim().is_empty() && s.trim() != "none")
            .map(|s| s.trim_end_matches('/').to_string());
        Self {
            files_dir,
            backend_url,
            frontend_url,
        }
    }
}
