use std::path::PathBuf;

use crate::config::BanFile;

#[derive(Debug)]
pub struct BanSettings {
    list_file: Option<PathBuf>,
}

impl BanSettings {
    #[must_use]
    pub const fn list_file(&self) -> Option<&PathBuf> {
        self.list_file.as_ref()
    }

    #[must_use]
    pub fn load(file: &BanFile) -> Self {
        Self {
            list_file: file.list_file.clone(),
        }
    }
}
