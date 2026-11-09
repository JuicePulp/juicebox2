use crate::error::StorageError;

#[async_trait::async_trait]
pub trait FreezeStore: Send + Sync {
    /// Freezes an object and returns true; idempotent for an existing file.
    async fn freeze(&self, id: &str, capability: Option<&str>) -> Result<bool, StorageError>;

    /// Thaws an object and returns true; idempotent when already thawed.
    async fn unfreeze(&self, id: &str, capability: Option<&str>) -> Result<bool, StorageError>;

    async fn is_frozen(&self, id: &str) -> Result<bool, StorageError>;
}

pub async fn ensure_not_frozen<S: FreezeStore>(store: &S, id: &str) -> Result<(), StorageError> {
    if store.is_frozen(id).await? {
        return Err(StorageError::Frozen);
    }
    Ok(())
}
