pub(crate) mod capability;
mod common;
pub(crate) mod freeze;
mod local;
mod r2;
mod s3;

#[cfg(test)]
mod tests;

use std::{path::PathBuf, pin::Pin, sync::atomic::AtomicU64};

use bytes::Bytes;
pub(crate) use common::valid_component;
pub use common::{extract_extension, guess_mime};
pub use freeze::FreezeStore;
use futures::Stream;
pub use local::LocalBackend;
pub use r2::R2Store;
pub use s3::S3Backend;
use serde::Serialize;
use utoipa::ToSchema;

use crate::error::StorageError;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, StorageError>> + Send>>;

#[derive(Debug, Clone)]
pub struct FileMetadata {
    pub size: u64,

    pub etag: String,

    pub extension: String,
}

pub struct FileData {
    pub data: Bytes,

    pub meta: FileMetadata,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct StorageMetrics {
    pub total_bytes: u64,

    pub used_bytes: u64,

    pub free_bytes: u64,

    pub min_free_bytes: u64,

    pub out_of_space: bool,
}

/// Storage contract; implementations must reject invalid IDs and enforce
/// capabilities/freezes before mutating.
#[async_trait::async_trait]
pub trait StorageBackend: FreezeStore + Send + Sync + 'static {
    /// Stores a new object, binding `capability` when given; never overwrites
    /// an ID.
    async fn put(
        &self,
        id: &str,
        filename: &str,
        data: Bytes,
        capability: Option<&str>,
    ) -> Result<(), StorageError>;

    async fn put_stream(
        &self,
        id: &str,
        filename: &str,
        data: ByteStream,
        capability: Option<&str>,
    ) -> Result<u64, StorageError>;

    async fn get(&self, id: &str) -> Result<FileData, StorageError>;

    async fn stat(&self, id: &str) -> Result<FileMetadata, StorageError>;

    fn local_path(&self, _id: &str) -> Option<PathBuf> {
        None
    }

    fn live_path(&self, _id: &str) -> Option<PathBuf> {
        None
    }

    /// Streams an inclusive byte range; out-of-bounds ranges are an error.
    async fn get_range_stream(
        &self,
        id: &str,
        start: u64,
        end: u64,
    ) -> Result<ByteStream, StorageError>;

    async fn get_stream(&self, id: &str) -> Result<ByteStream, StorageError>;

    /// Removes the object and its capability; false when the ID is absent.
    async fn delete(&self, id: &str, capability: Option<&str>) -> Result<bool, StorageError>;

    /// Moves the object and its capability to `new_id`, failing if the target
    /// exists.
    async fn rename(
        &self,
        old_id: &str,
        new_id: &str,
        capability: Option<&str>,
    ) -> Result<(), StorageError>;

    /// Appends the parts in order into `target_id`, then deletes the source
    /// parts.
    async fn concat(
        &self,
        target_id: &str,
        filename: &str,
        part_ids: &[&str],
        capability: Option<&str>,
    ) -> Result<(), StorageError>;

    fn storage_metrics(&self, min_free_bytes: u64) -> StorageMetrics;
}
