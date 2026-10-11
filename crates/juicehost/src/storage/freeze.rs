use crate::error::StorageError;











#[async_trait::async_trait]
pub trait FreezeStore: Send + Sync {
    
    
    
    async fn freeze(&self, id: &str) -> Result<bool, StorageError>;

    
    
    
    async fn unfreeze(&self, id: &str) -> Result<bool, StorageError>;

    
    
    
    
    async fn is_frozen(&self, id: &str) -> Result<bool, StorageError>;
}






pub async fn ensure_not_frozen<S: FreezeStore>(store: &S, id: &str) -> Result<(), StorageError> {
    if store.is_frozen(id).await? {
        return Err(StorageError::Frozen);
    }
    Ok(())
}
