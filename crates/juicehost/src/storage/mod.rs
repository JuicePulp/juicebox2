pub(crate) mod capability;
mod common;
mod local;
mod s3;

#[cfg(test)]
mod tests;

use std::{pin::Pin, sync::atomic::AtomicU64};

use bytes::Bytes;
pub(crate) use common::valid_component;
pub use common::{extract_extension, guess_mime};
use futures::Stream;
pub use local::LocalBackend;
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

#[async_trait::async_trait]
pub trait StorageBackend: Send + Sync + 'static {
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

    async fn get_range_stream(
        &self,
        id: &str,
        start: u64,
        end: u64,
    ) -> Result<ByteStream, StorageError>;

    async fn get_stream(&self, id: &str) -> Result<ByteStream, StorageError>;

    async fn delete(&self, id: &str, capability: Option<&str>) -> Result<bool, StorageError>;

    async fn rename(
        &self,
        old_id: &str,
        new_id: &str,
        capability: Option<&str>,
    ) -> Result<(), StorageError>;

    async fn concat(
        &self,
        target_id: &str,
        filename: &str,
        part_ids: &[&str],
        capability: Option<&str>,
    ) -> Result<(), StorageError>;

    fn storage_metrics(&self, min_free_bytes: u64) -> StorageMetrics;
}
