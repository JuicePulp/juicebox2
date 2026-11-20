use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::atomic::Ordering,
    time::{Duration, SystemTime},
};

use bytes::{Bytes, BytesMut};
use dashmap::DashMap;
use futures::StreamExt;
use tokio::io::{AsyncWriteExt, BufWriter};

use super::{
    ByteStream, FileData, FileMetadata, StorageBackend, StorageMetrics,
    capability::{self, CapStore},
    common::{safe_extension, valid_component},
    freeze::ensure_not_frozen,
};
use crate::{error::StorageError, live::LiveUploads};

/// An upload is abandoned when it has not been touched for this long. This
/// matches `LiveUploads::STALE_AFTER`; a published part group with no live
/// registration and no completed target is safe to remove.
const ABANDONED_AFTER: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Debug, Default, Clone, Copy)]
pub struct CleanupStats {
    pub files: u64,
    pub bytes: u64,
    pub kept_targets: u64,
    pub skipped_groups: u64,
}

impl CleanupStats {
    fn add_removed(&mut self, size: Option<u64>) {
        if let Some(size) = size {
            self.files += 1;
            self.bytes += size;
        }
    }
}

#[derive(Debug)]
struct StoredFile {
    id: String,
    path: PathBuf,
    size: u64,
    mtime: SystemTime,
    capability: Option<String>,
}

#[derive(Debug)]
struct Sidecar {
    id: String,
    path: PathBuf,
    mtime: SystemTime,
}

pub struct LocalBackend {
    files_dir: PathBuf,

    pub(crate) extensions: DashMap<String, String>,

    meta_cache: DashMap<String, FileMetadata>,

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

    /// Rebuilds the ID index, removes leftover temp files, and rejects
    /// symlinks, duplicates, and bad names.
    pub async fn init_cache(&self) -> Result<(), std::io::Error> {
        let mut entries = tokio::fs::read_dir(&self.files_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(".cap.") {
                // Sidecar markers must be regular files; a symlink here is corrupt state.
                let file_type = entry.file_type().await?;
                if file_type.is_symlink() || !file_type.is_file() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("capability entry is not a regular file: {name}"),
                    ));
                }
                continue;
            }

            if name.starts_with(".frz.") {
                // Any freeze marker, even a symlink, means frozen; fail closed.
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

    /// Removes abandoned upload artifacts older than [`ABANDONED_AFTER`].
    ///
    /// Parts are identified by their capability sidecar: every part and the
    /// final target of one multipart upload share a capability hash. A group is
    /// only touched when it is old and no member is registered as a live
    /// upload. If one member is exactly the sum of the others it is the
    /// completed target and only the parts are removed. If a newer, larger
    /// possible target exists but the sum does not match (for example some
    /// parts were already deleted), the group is left alone for inspection.
    pub async fn cleanup_stale(&self, live: &LiveUploads) -> Result<CleanupStats, std::io::Error> {
        let cutoff = SystemTime::now()
            .checked_sub(ABANDONED_AFTER)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        self.cleanup_stale_before(live, cutoff).await
    }

    /// Testable form of [`Self::cleanup_stale`] with an explicit cutoff.
    pub async fn cleanup_stale_before(
        &self,
        live: &LiveUploads,
        cutoff: SystemTime,
    ) -> Result<CleanupStats, std::io::Error> {
        let mut entries = tokio::fs::read_dir(&self.files_dir).await?;
        let mut files: Vec<StoredFile> = Vec::new();
        let mut live_temps: Vec<Sidecar> = Vec::new();
        let mut reserves: Vec<Sidecar> = Vec::new();
        let mut capabilities: HashMap<String, (PathBuf, SystemTime)> = HashMap::new();

        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name().to_string_lossy().to_string();
            let file_type = entry.file_type().await?;
            if file_type.is_symlink() || !file_type.is_file() {
                continue;
            }
            let meta = entry.metadata().await?;
            let mtime = meta.modified()?;
            let path = entry.path();

            if let Some(id) = name.strip_prefix(".cap.") {
                capabilities.insert(id.to_string(), (path, mtime));
                continue;
            }
            if let Some(id) = name
                .strip_prefix(".live.")
                .and_then(|rest| rest.strip_suffix(".tmp"))
            {
                live_temps.push(Sidecar {
                    id: id.to_string(),
                    path,
                    mtime,
                });
                continue;
            }
            if let Some(id) = name
                .strip_prefix('.')
                .and_then(|rest| rest.strip_suffix(".reserve"))
            {
                reserves.push(Sidecar {
                    id: id.to_string(),
                    path,
                    mtime,
                });
                continue;
            }
            if name.starts_with('.') {
                continue;
            }
            let Some(dot) = name.rfind('.') else {
                continue;
            };
            let id = name[..dot].to_string();
            if !valid_component(&id) {
                continue;
            }
            let capability = match capabilities.get(&id) {
                Some((cap_path, _)) => tokio::fs::read_to_string(cap_path)
                    .await
                    .ok()
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty()),
                None => None,
            };
            files.push(StoredFile {
                id,
                path,
                size: meta.len(),
                mtime,
                capability,
            });
        }

        let data_ids: HashSet<&str> = files.iter().map(|file| file.id.as_str()).collect();
        let mut stats = CleanupStats::default();

        for sidecar in &live_temps {
            if sidecar.mtime < cutoff
                && !data_ids.contains(sidecar.id.as_str())
                && !live.is_active(&sidecar.id)
            {
                if let Ok(()) = tokio::fs::remove_file(&sidecar.path).await {
                    stats.files += 1;
                }
            }
        }
        for sidecar in &reserves {
            if sidecar.mtime < cutoff && !live.is_active(&sidecar.id) {
                if let Ok(()) = tokio::fs::remove_file(&sidecar.path).await {
                    stats.files += 1;
                }
            }
        }
        // A capability without its data file is a half-published upload.
        for (id, (path, mtime)) in &capabilities {
            if *mtime < cutoff && !data_ids.contains(id.as_str()) && !live.is_active(id) {
                if let Ok(()) = tokio::fs::remove_file(path).await {
                    stats.files += 1;
                }
            }
        }

        let mut groups: HashMap<&str, Vec<&StoredFile>> = HashMap::new();
        for file in &files {
            let Some(capability) = file.capability.as_deref() else {
                continue;
            };
            if file.mtime < cutoff && !live.is_active(&file.id) {
                groups.entry(capability).or_default().push(file);
            }
        }

        for group in groups.values() {
            if group.len() < 2 || group.iter().any(|file| live.is_active(&file.id)) {
                continue;
            }
            let total = group.iter().map(|file| file.size).sum::<u64>();
            let exact_targets: Vec<&&StoredFile> = group
                .iter()
                .filter(|file| file.size > 0 && file.size == total - file.size)
                .collect();
            if exact_targets.len() == 1 {
                let target = exact_targets[0];
                stats.kept_targets += 1;
                for file in group.iter().filter(|file| file.id != target.id) {
                    stats.add_removed(self.remove_stale_file(file).await);
                }
                continue;
            }

            let mut by_mtime = group.clone();
            by_mtime.sort_by_key(|file| file.mtime);
            let newest = by_mtime[by_mtime.len() - 1];
            let second_newest = by_mtime[by_mtime.len() - 2];
            let largest_other = group
                .iter()
                .filter(|file| file.id != newest.id)
                .map(|file| file.size)
                .max()
                .unwrap_or(0);
            let possible_target = newest.size > largest_other
                && newest
                    .mtime
                    .duration_since(second_newest.mtime)
                    .is_ok_and(|gap| gap > Duration::from_secs(5));
            if possible_target {
                stats.skipped_groups += 1;
                continue;
            }

            for file in group {
                stats.add_removed(self.remove_stale_file(file).await);
            }
        }

        if stats.files > 0 {
            tracing::info!(
                "storage cleanup removed {} files ({:.2} MiB), kept {} completed targets, skipped {} possible targets",
                stats.files,
                stats.bytes as f64 / (1024.0 * 1024.0),
                stats.kept_targets,
                stats.skipped_groups,
            );
        }
        Ok(stats)
    }

    async fn remove_stale_file(&self, file: &StoredFile) -> Option<u64> {
        if self.frozen_marker_exists(&file.id).await.unwrap_or(true) {
            return None;
        }
        match tokio::fs::remove_file(&file.path).await {
            Ok(()) => {
                self.extensions.remove(&file.id);
                self.meta_cache.remove(&file.id);
                capability::remove(self, &file.id).await;
                Some(file.size)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                tracing::warn!(
                    "storage cleanup could not remove {}: {error}",
                    file.path.display()
                );
                None
            }
        }
    }

    fn resolve_path(&self, id: &str) -> Option<PathBuf> {
        if !valid_component(id) {
            return None;
        }
        let ext = self.extensions.get(id).map(|e| e.value().clone())?;
        self.path_for(id, &ext)
    }

    async fn stat_cached(&self, id: &str) -> Result<FileMetadata, StorageError> {
        if let Some(cached) = self.meta_cache.get(id) {
            ensure_not_frozen(self, id).await?;
            return Ok(cached.clone());
        }
        let path = self.resolve_path(id).ok_or(StorageError::NotFound)?;
        ensure_not_frozen(self, id).await?;
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

/// Removes the `.reserve` marker on drop so failed or cancelled writes release
/// the ID.
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

    fn frozen_path(&self, id: &str) -> Option<PathBuf> {
        valid_component(id).then(|| self.files_dir.join(format!(".frz.{id}")))
    }

    async fn frozen_marker_exists(&self, id: &str) -> Result<bool, StorageError> {
        let Some(path) = self.frozen_path(id) else {
            return Ok(false);
        };
        match tokio::fs::symlink_metadata(&path).await {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(StorageError::Io(format!("freeze check failed: {e}"))),
        }
    }
}

#[async_trait::async_trait]
impl super::freeze::FreezeStore for LocalBackend {
    async fn freeze(&self, id: &str, capability: Option<&str>) -> Result<bool, StorageError> {
        if !valid_component(id) {
            return Err(StorageError::Io("invalid logical ID".into()));
        }
        if !self.extensions.contains_key(id) {
            return Err(StorageError::NotFound);
        }
        if let Some(capability) = capability {
            capability::verify(self, id, capability).await?;
        }
        let path = self
            .frozen_path(id)
            .ok_or_else(|| StorageError::Io("invalid logical ID".into()))?;

        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
        {
            Ok(file) => {
                file.sync_all()
                    .await
                    .map_err(|e| StorageError::Io(format!("sync freeze marker failed: {e}")))?;
                Ok(true)
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(true),
            Err(e) => Err(StorageError::Io(format!(
                "create freeze marker failed: {e}"
            ))),
        }
    }

    async fn unfreeze(&self, id: &str, capability: Option<&str>) -> Result<bool, StorageError> {
        if !valid_component(id) {
            return Err(StorageError::Io("invalid logical ID".into()));
        }
        if !self.extensions.contains_key(id) {
            return Err(StorageError::NotFound);
        }
        if let Some(capability) = capability {
            capability::verify(self, id, capability).await?;
        }
        if let Some(path) = self.frozen_path(id) {
            let _ = tokio::fs::remove_file(&path).await;
        }
        Ok(true)
    }

    async fn is_frozen(&self, id: &str) -> Result<bool, StorageError> {
        self.frozen_marker_exists(id).await
    }
}

#[async_trait::async_trait]
impl CapStore for LocalBackend {
    async fn cap_write(&self, id: &str, hash: &str) -> Result<(), StorageError> {
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
        file.write_all(hash.as_bytes())
            .await
            .map_err(|e| StorageError::Io(format!("write capability failed: {e}")))?;
        file.flush()
            .await
            .map_err(|e| StorageError::Io(format!("flush capability failed: {e}")))
    }

    async fn cap_read(&self, id: &str) -> Result<String, StorageError> {
        let path = self.capability_path(id).ok_or(StorageError::Forbidden)?;
        // Any read failure is `Forbidden`, never revealing whether a marker exists.
        tokio::fs::read_to_string(path)
            .await
            .map_err(|_| StorageError::Forbidden)
    }

    async fn cap_delete(&self, id: &str) {
        if let Some(path) = self.capability_path(id) {
            let _ = tokio::fs::remove_file(path).await;
        }
    }
}

impl LocalBackend {
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
            let file = tokio::fs::OpenOptions::new()
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
            let mut file = BufWriter::with_capacity(512 * 1024, file);
            let mut total = 0u64;
            let mut visible = 0u64;
            while let Some(chunk) = data.next().await {
                let chunk = chunk?;
                total = total.saturating_add(chunk.len() as u64);
                file.write_all(&chunk)
                    .await
                    .map_err(|e| StorageError::Io(format!("write failed: {e}")))?;
                // Flush in coarse steps so live readers see progress without flushing on every
                // chunk.
                if total.saturating_sub(visible) >= 256 * 1024 {
                    file.flush()
                        .await
                        .map_err(|e| StorageError::Io(format!("flush failed: {e}")))?;
                    visible = total;
                }
            }
            file.flush()
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
        let started = std::time::Instant::now();
        let result = async {
            let mut target_file = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp_path)
                .await
                .map_err(|e| StorageError::Io(format!("create target failed: {e}")))?;

            let mut dst_off = 0u64;
            for part_id in part_ids {
                let part_path = self.resolve_path(part_id).ok_or(StorageError::NotFound)?;
                ensure_not_frozen(self, part_id).await?;
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
                dst_off = copy_part_fast(&mut part, &mut target_file, meta.len(), dst_off, part_id)
                    .await?;
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

        tracing::debug!(
            "concat: {} parts -> {target_id} in {:?}",
            part_ids.len(),
            started.elapsed()
        );

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
        // The marker is keyed by ID alone, so it also locks out other extensions.
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
            // Refuse symlinks so a swapped-in link cannot redirect reads outside the files
            // directory.
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

/// Linux reflink ioctl number; clones extents without copying bytes.
#[cfg(target_os = "linux")]
const FICLONERANGE: libc::c_ulong = 0x4020_940d;

#[cfg(target_os = "linux")]
#[repr(C)]
struct FileCloneRange {
    src_fd: i64,
    src_offset: u64,
    src_length: u64,
    dest_offset: u64,
}

#[cfg(target_os = "linux")]
fn try_reflink_range(
    part: &tokio::fs::File,
    target_file: &tokio::fs::File,
    part_len: u64,
    dst_off: u64,
) -> Result<bool, StorageError> {
    use std::os::fd::AsRawFd;
    let range = FileCloneRange {
        src_fd: i64::from(part.as_raw_fd()),
        src_offset: 0,
        src_length: part_len,
        dest_offset: dst_off,
    };
    let rc = unsafe { libc::ioctl(target_file.as_raw_fd(), FICLONERANGE, &raw const range) };
    if rc == 0 {
        return Ok(true);
    }
    tracing::debug!(
        "reflink declined, copying: {}",
        std::io::Error::last_os_error()
    );
    Ok(false)
}

/// Copies one part at `dst_off`, preferring reflink, then `copy_file_range`,
/// then buffered copy.
async fn copy_part_fast(
    part: &mut tokio::fs::File,
    target_file: &mut tokio::fs::File,
    part_len: u64,
    dst_off: u64,
    part_id: &str,
) -> Result<u64, StorageError> {
    #[cfg(target_os = "linux")]
    {
        if try_reflink_range(part, target_file, part_len, dst_off)? {
            return Ok(dst_off + part_len);
        }
    }
    let part_std = part
        .try_clone()
        .await
        .map_err(|e| StorageError::Io(format!("clone part {part_id} handle: {e}")))?
        .into_std()
        .await;
    let target_std = target_file
        .try_clone()
        .await
        .map_err(|e| StorageError::Io(format!("clone target handle: {e}")))?
        .into_std()
        .await;
    let id = part_id.to_owned();
    tokio::task::spawn_blocking(move || {
        copy_part_blocking(&part_std, &target_std, part_len, dst_off, &id)
    })
    .await
    .map_err(|e| StorageError::Io(format!("copy part {part_id} task failed: {e}")))?
}

#[cfg(unix)]
fn read_part_at(file: &std::fs::File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    use std::os::unix::fs::FileExt;
    file.read_at(buf, offset)
}

#[cfg(not(unix))]
fn read_part_at(file: &std::fs::File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = file;
    file.seek(SeekFrom::Start(offset))?;
    file.read(buf)
}

#[cfg(unix)]
fn write_part_at(file: &std::fs::File, buf: &[u8], offset: u64) -> std::io::Result<usize> {
    use std::os::unix::fs::FileExt;
    file.write_at(buf, offset)
}

#[cfg(not(unix))]
fn write_part_at(file: &std::fs::File, buf: &[u8], offset: u64) -> std::io::Result<usize> {
    use std::io::{Seek, SeekFrom, Write};
    let mut file = file;
    file.seek(SeekFrom::Start(offset))?;
    file.write(buf)
}

fn copy_part_blocking(
    part: &std::fs::File,
    target: &std::fs::File,
    part_len: u64,
    mut dst_off: u64,
    part_id: &str,
) -> Result<u64, StorageError> {
    let mut src_off = 0u64;
    let mut remaining = part_len;

    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        const CHUNK: u64 = 32 * 1024 * 1024;
        while remaining > 0 {
            let want = remaining.min(CHUNK) as libc::size_t;
            let mut in_off = src_off as libc::off_t;
            let mut out_off = dst_off as libc::off_t;
            let rc = unsafe {
                libc::copy_file_range(
                    part.as_raw_fd(),
                    &mut in_off,
                    target.as_raw_fd(),
                    &mut out_off,
                    want,
                    0,
                )
            };
            if rc < 0 {
                let err = std::io::Error::last_os_error();
                match err.raw_os_error() {
                    // Unsupported on this filesystem pair; fall back to buffered copying.
                    Some(code)
                        if code == libc::ENOSYS
                            || code == libc::EXDEV
                            || code == libc::EINVAL
                            || code == libc::EPERM
                            || code == libc::EOPNOTSUPP =>
                    {
                        break;
                    }
                    Some(code) if code == libc::EINTR => continue,
                    _ => {
                        return Err(StorageError::Io(format!(
                            "copy part {part_id} failed: {err}"
                        )));
                    }
                }
            }
            if rc == 0 {
                return Err(StorageError::Io(format!(
                    "copy part {part_id} hit unexpected EOF"
                )));
            }
            let n = rc as u64;
            src_off = in_off as u64;
            dst_off = out_off as u64;
            remaining -= n;
        }
    }
    let mut buf = vec![0u8; 1 << 20];
    while remaining > 0 {
        let want = remaining.min(buf.len() as u64) as usize;
        let n = read_part_at(part, &mut buf[..want], src_off)
            .map_err(|e| StorageError::Io(format!("read part {part_id} failed: {e}")))?;
        if n == 0 {
            return Err(StorageError::Io(format!(
                "copy part {part_id} hit unexpected EOF"
            )));
        }
        let mut written = 0;
        while written < n {
            let w = write_part_at(target, &buf[written..n], dst_off + written as u64)
                .map_err(|e| StorageError::Io(format!("write target failed: {e}")))?;
            if w == 0 {
                return Err(StorageError::Io("write target made no progress".into()));
            }
            written += w;
        }
        src_off += n as u64;
        dst_off += n as u64;
        remaining -= n as u64;
    }
    Ok(dst_off)
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
        capability::run_minted(self, id, capability, || async {
            self.write_bytes(id, filename, data).await
        })
        .await
    }

    async fn put_stream(
        &self,
        id: &str,
        filename: &str,
        data: ByteStream,
        capability: Option<&str>,
    ) -> Result<u64, StorageError> {
        capability::run_minted(self, id, capability, || async {
            self.write_stream(id, filename, data).await
        })
        .await
    }

    async fn get(&self, id: &str) -> Result<FileData, StorageError> {
        let path = self.resolve_path(id).ok_or(StorageError::NotFound)?;
        ensure_not_frozen(self, id).await?;

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

    fn local_path(&self, id: &str) -> Option<PathBuf> {
        self.resolve_path(id)
    }

    fn live_path(&self, id: &str) -> Option<PathBuf> {
        valid_component(id).then(|| self.files_dir.join(format!(".live.{id}.tmp")))
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
        ensure_not_frozen(self, id).await?;

        if start > meta.size || end >= meta.size {
            // `end` is inclusive, so it must stay strictly below the file size.
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
                let mut buf = BytesMut::with_capacity(remaining.min(64 * 1024) as usize);
                match tokio::io::AsyncReadExt::read_buf(&mut file, &mut buf).await {
                    Ok(0) => Some((
                        Err(StorageError::Io("unexpected EOF reading range".into())),
                        (file, 0),
                    )),
                    Ok(n) => Some((Ok(buf.split().freeze()), (file, remaining - n as u64))),
                    Err(e) => Some((
                        Err(StorageError::Io(format!("read failed: {e}"))),
                        (file, 0),
                    )),
                }
            });
        Ok(Box::pin(stream))
    }

    async fn delete(&self, id: &str, capability: Option<&str>) -> Result<bool, StorageError> {
        let ext = {
            let Some(e) = self.extensions.get(id) else {
                return Ok(false);
            };
            e.value().clone()
        };
        if let Some(capability) = capability {
            capability::verify(self, id, capability).await?;
        }

        ensure_not_frozen(self, id).await?;
        let path = self.files_dir.join(format!("{id}.{ext}"));
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {
                self.extensions.remove(id);
                self.meta_cache.remove(id);
                capability::remove(self, id).await;
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
            capability::verify(self, old_id, capability).await?;
        }
        let _reservation = self.reserve_id(new_id).await?;
        let ext = self
            .extensions
            .get(old_id)
            .map(|e| e.value().clone())
            .ok_or(StorageError::NotFound)?;
        ensure_not_frozen(self, old_id).await?;

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

        link_or_rename(&old_path, &new_path).await?;

        self.extensions.remove(old_id);
        self.extensions.insert(new_id.to_string(), ext);
        self.meta_cache.remove(old_id);
        self.meta_cache.remove(new_id);
        if let (Some(old), Some(new)) = (self.capability_path(old_id), self.capability_path(new_id))
            && tokio::fs::try_exists(&old).await.unwrap_or(false)
        {
            tokio::fs::rename(old, new)
                .await
                .map_err(|e| StorageError::Io(format!("rename capability failed: {e}")))?;
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
                capability::verify(self, part_id, capability).await?;
            }
        }
        capability::run_minted(self, target_id, capability, || async {
            self.concat_objects(target_id, filename, part_ids).await
        })
        .await
    }

    async fn get_stream(&self, id: &str) -> Result<ByteStream, StorageError> {
        let path = self.resolve_path(id).ok_or(StorageError::NotFound)?;
        ensure_not_frozen(self, id).await?;
        let file = self.open_regular(&path).await?;
        let stream = futures::stream::unfold(file, |mut file| async move {
            let mut buf = BytesMut::with_capacity(256 * 1024);
            match tokio::io::AsyncReadExt::read_buf(&mut file, &mut buf).await {
                Ok(0) => None,
                Ok(_) => Some((Ok(buf.split().freeze()), file)),
                Err(e) => Some((Err(StorageError::Io(format!("read failed: {e}"))), file)),
            }
        });
        Ok(Box::pin(stream))
    }
}

impl LocalBackend {
    fn temp_path(&self, id: &str) -> PathBuf {
        self.files_dir.join(format!(".live.{id}.tmp"))
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

/// Move `src` onto `dst` without ever overwriting an existing `dst`.
///
/// A hard link is tried first: it is atomic and reports `AlreadyExists`,
/// so concurrent writers can never clobber each other. Not every
/// filesystem allows hard links though -- Android app-private storage
/// rejects them with `EPERM` (so every upload failed on Termux), and
/// overlay/`EXDEV` layouts fail too. When linking is unavailable we fall
/// back to `rename`, which is still atomic because `src` and `dst` live
/// in the same directory. That fallback has to check existence itself,
/// since `rename` silently replaces the destination.
pub(super) async fn link_or_rename(src: &Path, dst: &Path) -> Result<(), StorageError> {
    match tokio::fs::hard_link(src, dst).await {
        Ok(()) => {
            if let Err(e) = tokio::fs::remove_file(src).await {
                let _ = tokio::fs::remove_file(dst).await;
                return Err(StorageError::Io(format!("link cleanup failed: {e}")));
            }
            Ok(())
        }
        Err(link_err) if link_err.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(StorageError::Conflict)
        }
        Err(link_err) => {
            if tokio::fs::try_exists(dst).await.unwrap_or(false) {
                return Err(StorageError::Conflict);
            }
            tokio::fs::rename(src, dst).await.map_err(|e| {
                StorageError::Io(format!(
                    "publish failed: hard link unavailable ({link_err}) and rename failed ({e})"
                ))
            })
        }
    }
}

async fn publish_new_file(temp: &PathBuf, target: &PathBuf) -> Result<(), StorageError> {
    link_or_rename(temp, target).await
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
