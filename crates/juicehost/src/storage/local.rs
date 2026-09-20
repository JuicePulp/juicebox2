use std::{path::PathBuf, sync::atomic::Ordering};

use bytes::Bytes;
use dashmap::DashMap;
use futures::StreamExt;
use tokio::io::AsyncWriteExt;

use super::{
    ByteStream, FileData, FileMetadata, StorageBackend, StorageMetrics, TEMP_COUNTER,
    common::{capability_hash, safe_extension, valid_component},
};
use crate::error::StorageError;

/// Local disk backend.
pub struct LocalBackend {
    /// Root directory where files are stored.
    files_dir: PathBuf,
    /// Maps file ID -> extension for MIME type detection.
    pub(crate) extensions: DashMap<String, String>,
    /// Metadata cache (`ETag`, size) to avoid repeated `stat()` syscalls on hot
    /// files.
    meta_cache: DashMap<String, FileMetadata>,
    /// Minimum free disk space required before rejecting writes.
    min_free_space_bytes: u64,
}

impl LocalBackend {
    pub fn new(files_dir: PathBuf, min_free_space_bytes: u64) -> Result<Self, std::io::Error> {
        let files_dir = std::fs::canonicalize(&files_dir)?;
        if !std::fs::metadata(&files_dir).is_ok_and(|m| m.is_dir()) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "files directory is not a directory",
            ));
        }
        Ok(Self {
            files_dir,
            extensions: DashMap::new(),
            meta_cache: DashMap::new(),
            min_free_space_bytes,
        })
    }

    /// Scan the files directory and populate the extension cache.
    pub async fn init_cache(&self) -> Result<(), std::io::Error> {
        let mut entries = tokio::fs::read_dir(&self.files_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(".cap.") {
                let file_type = entry.file_type().await?;
                if file_type.is_symlink() || !file_type.is_file() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("capability entry is not a regular file: {name}"),
                    ));
                }
                continue;
            }
            if name.starts_with('.') && (name.ends_with(".reserve") || name.ends_with(".tmp")) {
                let file_type = entry.file_type().await?;
                if file_type.is_file() {
                    tokio::fs::remove_file(entry.path()).await?;
                }
                continue;
            }
            let file_type = entry.file_type().await?;
            if file_type.is_symlink() || !file_type.is_file() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("storage entry is not a regular file: {name}"),
                ));
            }
            if let Some(dot) = name.rfind('.') {
                let id = name[..dot].to_string();
                let ext = name[dot + 1..].to_string();
                if !valid_component(&id) || !valid_component(&ext) {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("invalid storage entry name: {name}"),
                    ));
                }
                if self.extensions.insert(id.clone(), ext).is_some() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::AlreadyExists,
                        format!("duplicate logical file ID: {id}"),
                    ));
                }
            }
        }
        tracing::info!(
            "local storage cache initialized with {} entries",
            self.extensions.len()
        );
        Ok(())
    }

    /// Resolve the filesystem path for a file ID using the extension cache.
    fn resolve_path(&self, id: &str) -> Option<PathBuf> {
        if !valid_component(id) {
            return None;
        }
        let ext = self.extensions.get(id).map(|e| e.value().clone())?;
        self.path_for(id, &ext)
    }

    /// Stat a file using the cache when available without reading its contents.
    async fn stat_cached(&self, id: &str) -> Result<FileMetadata, StorageError> {
        if let Some(cached) = self.meta_cache.get(id) {
            return Ok(cached.clone());
        }
        let path = self.resolve_path(id).ok_or(StorageError::NotFound)?;
        let meta = tokio::fs::symlink_metadata(&path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                StorageError::NotFound
            } else {
                StorageError::Io(format!("metadata failed: {e}"))
            }
        })?;
        if !meta.is_file() {
            return Err(StorageError::Io(
                "storage path is not a regular file".into(),
            ));
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("bin")
            .to_string();
        let file_meta = FileMetadata {
            size: meta.len(),
            etag: etag_from_metadata(&meta),
            extension: ext,
        };
        self.meta_cache.insert(id.to_string(), file_meta.clone());
        Ok(file_meta)
    }
}

struct LocalReservation {
    path: PathBuf,
}

impl Drop for LocalReservation {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

impl LocalBackend {
    fn path_for(&self, id: &str, ext: &str) -> Option<PathBuf> {
        if !valid_component(id) || !valid_component(ext) {
            return None;
        }
        let path = self.files_dir.join(format!("{id}.{ext}"));
        path.parent().filter(|parent| *parent == self.files_dir)?;
        Some(path)
    }

    fn capability_path(&self, id: &str) -> Option<PathBuf> {
        valid_component(id).then(|| self.files_dir.join(format!(".cap.{id}")))
    }

    async fn create_capability(&self, id: &str, capability: &str) -> Result<(), StorageError> {
        let path = self
            .capability_path(id)
            .ok_or_else(|| StorageError::Io("invalid logical ID".into()))?;
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    StorageError::Conflict
                } else {
                    StorageError::Io(format!("create capability failed: {e}"))
                }
            })?;
        file.write_all(capability_hash(capability).as_bytes())
            .await
            .map_err(|e| StorageError::Io(format!("write capability failed: {e}")))?;
        file.flush()
            .await
            .map_err(|e| StorageError::Io(format!("flush capability failed: {e}")))
    }

    async fn verify_capability(&self, id: &str, capability: &str) -> Result<(), StorageError> {
        let path = self.capability_path(id).ok_or(StorageError::Forbidden)?;
        let expected = tokio::fs::read_to_string(path)
            .await
            .map_err(|_| StorageError::Forbidden)?;
        if juiceutils::constant_time_eq(expected.trim(), &capability_hash(capability)) {
            Ok(())
        } else {
            Err(StorageError::Forbidden)
        }
    }

    async fn remove_capability(&self, id: &str) {
        if let Some(path) = self.capability_path(id) {
            let _ = tokio::fs::remove_file(path).await;
        }
    }

    async fn write_bytes(&self, id: &str, filename: &str, data: Bytes) -> Result<(), StorageError> {
        let ext = safe_extension(filename);
        let path = self
            .path_for(id, &ext)
            .ok_or_else(|| StorageError::Io("invalid storage path".into()))?;
        let _reservation = self.reserve_id(id).await?;

        let free = fs2::available_space(&self.files_dir)
            .map_err(|e| StorageError::Io(format!("disk check failed: {e}")))?;
        if free < self.min_free_space_bytes {
            return Err(StorageError::InsufficientStorage);
        }

        self.write_local(
            id,
            &path,
            &ext,
            Box::pin(futures::stream::once(async move { Ok(data) })),
        )
        .await?;
        Ok(())
    }

    async fn write_stream(
        &self,
        id: &str,
        filename: &str,
        mut data: ByteStream,
    ) -> Result<u64, StorageError> {
        let ext = safe_extension(filename);
        let path = self
            .path_for(id, &ext)
            .ok_or_else(|| StorageError::Io("invalid storage path".into()))?;
        let _reservation = self.reserve_id(id).await?;
        let free = fs2::available_space(&self.files_dir)
            .map_err(|e| StorageError::Io(format!("disk check failed: {e}")))?;
        if free < self.min_free_space_bytes {
            return Err(StorageError::InsufficientStorage);
        }

        let temp_path = self.temp_path(id);
        let result = async {
            let mut file = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp_path)
                .await
                .map_err(|e| {
                    if e.kind() == std::io::ErrorKind::AlreadyExists {
                        StorageError::Conflict
                    } else {
                        StorageError::Io(format!("create temp failed: {e}"))
                    }
                })?;
            let mut total = 0u64;
            while let Some(chunk) = data.next().await {
                let chunk = chunk?;
                total = total.saturating_add(chunk.len() as u64);
                tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
                    .await
                    .map_err(|e| StorageError::Io(format!("write failed: {e}")))?;
            }
            tokio::io::AsyncWriteExt::flush(&mut file)
                .await
                .map_err(|e| StorageError::Io(format!("flush failed: {e}")))?;
            drop(file);
            publish_new_file(&temp_path, &path).await?;
            Ok(total)
        }
        .await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(&temp_path).await;
        } else {
            self.extensions.insert(id.to_string(), ext);
            self.meta_cache.remove(id);
        }
        result
    }

    async fn concat_objects(
        &self,
        target_id: &str,
        filename: &str,
        part_ids: &[&str],
    ) -> Result<(), StorageError> {
        let ext = safe_extension(filename);
        let target_path = self
            .path_for(target_id, &ext)
            .ok_or_else(|| StorageError::Io("invalid storage path".into()))?;
        let _reservation = self.reserve_id(target_id).await?;
        let free = fs2::available_space(&self.files_dir)
            .map_err(|e| StorageError::Io(format!("disk check failed: {e}")))?;
        if free < self.min_free_space_bytes {
            return Err(StorageError::InsufficientStorage);
        }

        let temp_path = self.temp_path(target_id);
        let result = async {
            let mut target_file = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp_path)
                .await
                .map_err(|e| StorageError::Io(format!("create target failed: {e}")))?;

            for part_id in part_ids {
                let part_path = self.resolve_path(part_id).ok_or(StorageError::NotFound)?;
                let mut part = self.open_regular(&part_path).await?;
                let meta = part
                    .metadata()
                    .await
                    .map_err(|e| StorageError::Io(format!("part metadata failed: {e}")))?;
                if !meta.is_file() {
                    return Err(StorageError::Io(format!(
                        "part {part_id} is not a regular file"
                    )));
                }
                tokio::io::copy(&mut part, &mut target_file)
                    .await
                    .map_err(|e| StorageError::Io(format!("copy part {part_id} failed: {e}")))?;
            }

            target_file
                .flush()
                .await
                .map_err(|e| StorageError::Io(format!("flush failed: {e}")))?;
            drop(target_file);
            publish_new_file(&temp_path, &target_path).await
        }
        .await;
        if let Err(e) = result {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(e);
        }
        self.extensions.insert(target_id.to_string(), ext);

        for part_id in part_ids {
            if let Err(e) = self.delete(part_id, None).await {
                tracing::warn!("concat: failed to delete part {part_id}: {e}");
            }
        }

        Ok(())
    }

    async fn reserve_id(&self, id: &str) -> Result<LocalReservation, StorageError> {
        if !valid_component(id) {
            return Err(StorageError::Io("invalid logical ID".into()));
        }
        let path = self.files_dir.join(format!(".{id}.reserve"));
        tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    StorageError::Conflict
                } else {
                    StorageError::Io(format!("reserve ID failed: {e}"))
                }
            })?;
        if self.extensions.contains_key(id) {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(StorageError::Conflict);
        }
        Ok(LocalReservation { path })
    }

    async fn open_regular(&self, path: &PathBuf) -> Result<tokio::fs::File, StorageError> {
        if path.parent() != Some(self.files_dir.as_path()) {
            return Err(StorageError::Io(
                "storage path escaped files directory".into(),
            ));
        }
        let mut options = tokio::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let file = options.open(path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                StorageError::NotFound
            } else {
                StorageError::Io(format!("open regular file failed: {e}"))
            }
        })?;
        let metadata = file
            .metadata()
            .await
            .map_err(|e| StorageError::Io(format!("file metadata failed: {e}")))?;
        if !metadata.is_file() {
            return Err(StorageError::Io(
                "storage path is not a regular file".into(),
            ));
        }
        Ok(file)
    }
}

#[async_trait::async_trait]
impl StorageBackend for LocalBackend {
    async fn put(
        &self,
        id: &str,
        filename: &str,
        data: Bytes,
        capability: Option<&str>,
    ) -> Result<(), StorageError> {
        if let Some(capability) = capability {
            self.create_capability(id, capability).await?;
        }
        let result = self.write_bytes(id, filename, data).await;
        if result.is_err() && capability.is_some() {
            self.remove_capability(id).await;
        }
        result
    }

    async fn put_stream(
        &self,
        id: &str,
        filename: &str,
        data: ByteStream,
        capability: Option<&str>,
    ) -> Result<u64, StorageError> {
        if let Some(capability) = capability {
            self.create_capability(id, capability).await?;
        }
        let result = self.write_stream(id, filename, data).await;
        if result.is_err() && capability.is_some() {
            self.remove_capability(id).await;
        }
        result
    }

    async fn get(&self, id: &str) -> Result<FileData, StorageError> {
        let path = self.resolve_path(id).ok_or(StorageError::NotFound)?;

        let mut file = self.open_regular(&path).await?;
        let meta = file
            .metadata()
            .await
            .map_err(|e| StorageError::Io(format!("metadata failed: {e}")))?;

        let etag = etag_from_metadata(&meta);
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("bin")
            .to_string();

        let mut data = Vec::with_capacity(meta.len().min(usize::MAX as u64) as usize);
        tokio::io::AsyncReadExt::read_to_end(&mut file, &mut data)
            .await
            .map_err(|e| StorageError::Io(format!("read failed: {e}")))?;

        let file_meta = FileMetadata {
            size: meta.len(),
            etag,
            extension: ext,
        };
        self.meta_cache.insert(id.to_string(), file_meta.clone());

        Ok(FileData {
            data: Bytes::from(data),
            meta: file_meta,
        })
    }

    async fn stat(&self, id: &str) -> Result<FileMetadata, StorageError> {
        self.stat_cached(id).await
    }

    async fn get_range_stream(
        &self,
        id: &str,
        start: u64,
        end: u64,
    ) -> Result<ByteStream, StorageError> {
        use tokio::io::AsyncSeekExt;

        let meta = self.stat_cached(id).await?;
        let path = self.resolve_path(id).ok_or(StorageError::NotFound)?;

        if start > meta.size || end >= meta.size {
            return Err(StorageError::Io(format!(
                "range {}-{} out of bounds for {} byte file",
                start, end, meta.size
            )));
        }

        let mut file = self.open_regular(&path).await?;
        file.seek(std::io::SeekFrom::Start(start))
            .await
            .map_err(|e| StorageError::Io(format!("seek failed: {e}")))?;
        let remaining = end - start + 1;
        let stream =
            futures::stream::unfold((file, remaining), |(mut file, remaining)| async move {
                if remaining == 0 {
                    return None;
                }
                let mut buf = vec![0u8; remaining.min(64 * 1024) as usize];
                match tokio::io::AsyncReadExt::read(&mut file, &mut buf).await {
                    Ok(0) => Some((
                        Err(StorageError::Io("unexpected EOF reading range".into())),
                        (file, 0),
                    )),
                    Ok(n) => {
                        buf.truncate(n);
                        Some((Ok(Bytes::from(buf)), (file, remaining - n as u64)))
                    }
                    Err(e) => Some((
                        Err(StorageError::Io(format!("read failed: {e}"))),
                        (file, 0),
                    )),
                }
            });
        Ok(Box::pin(stream))
    }

    async fn delete(&self, id: &str, capability: Option<&str>) -> Result<bool, StorageError> {
        // If the blob is already gone there is nothing to protect, so report it
        // as not-found rather than failing on a missing/stale capability file.
        // This lets cleanup purge orphaned rows without weakening auth for
        // files that still exist on disk (those still require a valid capability).
        let ext = {
            let Some(e) = self.extensions.get(id) else {
                return Ok(false);
            };
            e.value().clone()
        };
        if let Some(capability) = capability {
            self.verify_capability(id, capability).await?;
        }
        let path = self.files_dir.join(format!("{id}.{ext}"));
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {
                self.extensions.remove(id);
                self.meta_cache.remove(id);
                self.remove_capability(id).await;
                Ok(true)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(StorageError::Io(format!("delete failed: {e}"))),
        }
    }

    async fn rename(
        &self,
        old_id: &str,
        new_id: &str,
        capability: Option<&str>,
    ) -> Result<(), StorageError> {
        if let Some(capability) = capability {
            self.verify_capability(old_id, capability).await?;
        }
        let _reservation = self.reserve_id(new_id).await?;
        let ext = self
            .extensions
            .get(old_id)
            .map(|e| e.value().clone())
            .ok_or(StorageError::NotFound)?;

        let old_path = self.files_dir.join(format!("{old_id}.{ext}"));
        let old_meta = tokio::fs::symlink_metadata(&old_path)
            .await
            .map_err(|_| StorageError::NotFound)?;
        if !old_meta.is_file() {
            return Err(StorageError::Io(
                "rename source is not a regular file".into(),
            ));
        }
        let new_path = self
            .path_for(new_id, &ext)
            .ok_or_else(|| StorageError::Io("invalid storage path".into()))?;

        tokio::fs::hard_link(&old_path, &new_path)
            .await
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    StorageError::Conflict
                } else {
                    StorageError::Io(format!("rename link failed: {e}"))
                }
            })?;
        if let Err(e) = tokio::fs::remove_file(&old_path).await {
            let _ = tokio::fs::remove_file(&new_path).await;
            return Err(StorageError::Io(format!("rename cleanup failed: {e}")));
        }

        self.extensions.remove(old_id);
        self.extensions.insert(new_id.to_string(), ext);
        self.meta_cache.remove(old_id);
        self.meta_cache.remove(new_id);
        if let (Some(old), Some(new)) = (self.capability_path(old_id), self.capability_path(new_id))
        {
            if tokio::fs::try_exists(&old).await.unwrap_or(false) {
                tokio::fs::rename(old, new)
                    .await
                    .map_err(|e| StorageError::Io(format!("rename capability failed: {e}")))?;
            }
        }
        Ok(())
    }

    fn storage_metrics(&self, min_free_bytes: u64) -> StorageMetrics {
        let total = fs2::total_space(&self.files_dir).unwrap_or(0);
        let free = fs2::available_space(&self.files_dir).unwrap_or(0);
        let used = total.saturating_sub(free);
        StorageMetrics {
            total_bytes: total,
            used_bytes: used,
            free_bytes: free,
            min_free_bytes,
            out_of_space: free < min_free_bytes,
        }
    }

    async fn concat(
        &self,
        target_id: &str,
        filename: &str,
        part_ids: &[&str],
        capability: Option<&str>,
    ) -> Result<(), StorageError> {
        if let Some(capability) = capability {
            for part_id in part_ids {
                self.verify_capability(part_id, capability).await?;
            }
            self.create_capability(target_id, capability).await?;
        }
        let result = self.concat_objects(target_id, filename, part_ids).await;
        if result.is_err() && capability.is_some() {
            self.remove_capability(target_id).await;
        }
        result
    }

    async fn get_stream(&self, id: &str) -> Result<ByteStream, StorageError> {
        let path = self.resolve_path(id).ok_or(StorageError::NotFound)?;
        let file = self.open_regular(&path).await?;
        let stream = futures::stream::unfold(file, |mut file| async move {
            let mut buf = vec![0u8; 64 * 1024];
            match tokio::io::AsyncReadExt::read(&mut file, &mut buf).await {
                Ok(0) => None,
                Ok(n) => {
                    buf.truncate(n);
                    Some((Ok(Bytes::from(buf)), file))
                }
                Err(e) => Some((Err(StorageError::Io(format!("read failed: {e}"))), file)),
            }
        });
        Ok(Box::pin(stream))
    }
}

impl LocalBackend {
    fn temp_path(&self, id: &str) -> PathBuf {
        self.files_dir.join(format!(
            ".{}.{}.{}.tmp",
            id,
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    async fn write_local(
        &self,
        id: &str,
        path: &PathBuf,
        ext: &str,
        mut data: ByteStream,
    ) -> Result<(), StorageError> {
        let temp_path = self.temp_path(id);
        let result = async {
            let mut file = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp_path)
                .await
                .map_err(|e| {
                    if e.kind() == std::io::ErrorKind::AlreadyExists {
                        StorageError::Conflict
                    } else {
                        StorageError::Io(format!("create temp failed: {e}"))
                    }
                })?;
            while let Some(chunk) = data.next().await {
                let chunk = chunk?;
                tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
                    .await
                    .map_err(|e| StorageError::Io(format!("write failed: {e}")))?;
            }
            tokio::io::AsyncWriteExt::flush(&mut file)
                .await
                .map_err(|e| StorageError::Io(format!("flush failed: {e}")))?;
            drop(file);
            publish_new_file(&temp_path, path).await
        }
        .await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(&temp_path).await;
        }
        if result.is_ok() {
            self.extensions.insert(id.to_string(), ext.to_string());
            self.meta_cache.remove(id);
        }
        result
    }
}

async fn publish_new_file(temp: &PathBuf, target: &PathBuf) -> Result<(), StorageError> {
    tokio::fs::hard_link(temp, target).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            StorageError::Conflict
        } else {
            StorageError::Io(format!("publish failed: {e}"))
        }
    })?;
    if let Err(e) = tokio::fs::remove_file(temp).await {
        tracing::warn!(
            "failed to remove published temp file {}: {}",
            temp.display(),
            e
        );
    }
    Ok(())
}

fn etag_from_metadata(meta: &std::fs::Metadata) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        format!(
            "\"{:x}-{:x}-{:x}\"",
            meta.ino(),
            meta.mtime() as u64,
            meta.len()
        )
    }
    #[cfg(not(unix))]
    {
        format!(
            "\"{:x}-{:x}\"",
            meta.modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0),
            meta.len()
        )
    }
}
