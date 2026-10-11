use crate::error::StorageError;

/// Persistent per-file freeze markers (legal hold).
///
/// A frozen file keeps its bytes on disk but must not be served, previewed,
/// copied into new files, renamed, or deleted until an admin unfreezes it.
/// Markers live next to the existing sidecars (`LocalBackend` stores an
/// empty `.frz.{id}` file, `S3Backend` an empty `files/.frozen/{id}` object)
/// so freeze state survives restarts without any extra database.
///
/// Reads check the marker on every access (authoritative, never cached) so
/// a freeze takes effect immediately on all instances sharing the storage.
#[async_trait::async_trait]
pub trait FreezeStore: Send + Sync {
    /// Freeze an existing file. Idempotent: freezing an already-frozen file
    /// succeeds. Fails with [`StorageError::NotFound`] when no file with
    /// this ID exists, so markers can never be created for ghost IDs.
    async fn freeze(&self, id: &str) -> Result<bool, StorageError>;

    /// Unfreeze a file, clearing its marker. Idempotent: unfreezing a file
    /// without a marker succeeds. Fails with [`StorageError::NotFound`]
    /// when no file with this ID exists.
    async fn unfreeze(&self, id: &str) -> Result<bool, StorageError>;

    /// Authoritative frozen check. Returns `false` only when the marker is
    /// definitively absent; any I/O error while checking propagates as
    /// [`StorageError::Io`] so callers fail closed instead of serving bytes
    /// they could not verify.
    async fn is_frozen(&self, id: &str) -> Result<bool, StorageError>;
}

/// Reject frozen files with [`StorageError::Frozen`].
///
/// Call after establishing existence so missing files keep reporting
/// [`StorageError::NotFound`] (frozen markers can only exist for files that
/// existed when frozen, so a missing file is never "frozen").
pub async fn ensure_not_frozen<S: FreezeStore>(store: &S, id: &str) -> Result<(), StorageError> {
    if store.is_frozen(id).await? {
        return Err(StorageError::Frozen);
    }
    Ok(())
}
