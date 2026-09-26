use crate::config::{ConfigError, ThreadFile};

#[derive(Debug)]
pub struct ThreadSettings {
    worker_threads: usize,
}

impl ThreadSettings {
    #[must_use]
    pub const fn worker_threads(&self) -> usize {
        self.worker_threads
    }

    pub fn load(file: &ThreadFile) -> Result<Self, ConfigError> {
        Ok(Self {
            worker_threads: file.worker_threads,
        })
    }
}
