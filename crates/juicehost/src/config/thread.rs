use crate::config::{ConfigError, ThreadFile, bounded_env};

/// Tokio worker thread settings.
#[derive(Debug)]
pub struct ThreadSettings {
    worker_threads: usize,
}

impl ThreadSettings {
    pub const fn worker_threads(&self) -> usize {
        self.worker_threads
    }

    pub fn load(file: &ThreadFile) -> Result<Self, ConfigError> {
        Ok(Self {
            worker_threads: bounded_env("WORKER_THREADS", file.worker_threads, 1, 256)?,
        })
    }
}
