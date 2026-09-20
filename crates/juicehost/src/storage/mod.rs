//! Disk handling and S3-compatible storage.

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

/// Metadata about a stored file, used for `ETag` generation and responses.
#[derive(Debug, Clone)]
pub struct FileMetadata {
    /// Content length in bytes.
    pub size: u64,
    /// `ETag` value (backend-specific format).
    pub etag: String,
    /// File extension (e.g. "png", "bin").
    pub extension: String,
}

/// Response from a storage get operation.
pub struct FileData {
    /// The file contents.
    pub data: Bytes,
    /// File metadata including `ETag`.
    pub meta: FileMetadata,
}

/// Storage metrics for health/info endpoints.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct StorageMetrics {
    /// Total disk capacity in bytes (0 for S3).
    pub total_bytes: u64,
    /// Bytes used on disk (0 for S3).
    pub used_bytes: u64,
    /// Available free bytes (`u64::MAX` for S3).
    pub free_bytes: u64,
    /// Minimum free bytes required before rejecting writes.
    pub min_free_bytes: u64,
    /// True when free space is below the minimum threshold.
    pub out_of_space: bool,
}

/// Trait for pluggable file storage backends.
#[async_trait::async_trait]
pub trait StorageBackend: Send + Sync + 'static {
    /// Store a file. Returns an error if the file already exists.
    /// When `capability` is `Some`, a capability is minted for the new file
    /// (removed again if the store fails).
    async fn put(
        &self,
        id: &str,
        filename: &str,
        data: Bytes,
        capability: Option<&str>,
    ) -> Result<(), StorageError>;

    /// Stream a new file into storage without exposing backend-specific paths.
    /// `capability` behaves as in [`StorageBackend::put`].
    async fn put_stream(
        &self,
        id: &str,
        filename: &str,
        data: ByteStream,
        capability: Option<&str>,
    ) -> Result<u64, StorageError>;

    /// Retrieve a file's contents and metadata.
    async fn get(&self, id: &str) -> Result<FileData, StorageError>;

    /// Get only metadata (size, `ETag`, extension) without reading file
    /// contents. Used for ETag-first lookups to avoid buffering the full
    /// file for 304 responses.
    async fn stat(&self, id: &str) -> Result<FileMetadata, StorageError>;

    /// Stream a byte range. `start` and `end` are inclusive.
    async fn get_range_stream(
        &self,
        id: &str,
        start: u64,
        end: u64,
    ) -> Result<ByteStream, StorageError>;

    /// Stream a whole file.
    async fn get_stream(&self, id: &str) -> Result<ByteStream, StorageError>;

    /// Delete a file. Returns Ok(true) if deleted, Ok(false) if not found.
    /// When `capability` is `Some`, it is verified before deleting (missing
    /// files still report not-found without requiring a capability, so cleanup
    /// can purge orphaned rows).
    async fn delete(&self, id: &str, capability: Option<&str>) -> Result<bool, StorageError>;

    /// Rename a file (change its ID). Returns error if new ID already exists.
    /// When `capability` is `Some`, it is verified against the source ID first.
    async fn rename(
        &self,
        old_id: &str,
        new_id: &str,
        capability: Option<&str>,
    ) -> Result<(), StorageError>;

    /// Concatenate multiple files into a new file, then delete the originals.
    /// When `capability` is `Some`, every part is verified and the target
    /// inherits the capability (removed again if the concat fails).
    async fn concat(
        &self,
        target_id: &str,
        filename: &str,
        part_ids: &[&str],
        capability: Option<&str>,
    ) -> Result<(), StorageError>;

    fn storage_metrics(&self, min_free_bytes: u64) -> StorageMetrics;
}
