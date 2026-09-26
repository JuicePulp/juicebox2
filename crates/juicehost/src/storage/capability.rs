use super::common::capability_hash;
use crate::error::StorageError;

#[async_trait::async_trait]
pub trait CapStore: Send + Sync {
    async fn cap_write(&self, id: &str, hash: &str) -> Result<(), StorageError>;

    async fn cap_read(&self, id: &str) -> Result<String, StorageError>;

    async fn cap_delete(&self, id: &str);
}

#[must_use]
pub fn hash(capability: &str) -> String {
    capability_hash(capability)
}

#[must_use]
pub fn matches(stored: &str, capability: &str) -> bool {
    juiceutils::constant_time_eq(stored.trim(), &hash(capability))
}

pub async fn mint<S: CapStore>(store: &S, id: &str, capability: &str) -> Result<(), StorageError> {
    store.cap_write(id, &hash(capability)).await
}

pub async fn verify<S: CapStore>(
    store: &S,
    id: &str,
    capability: &str,
) -> Result<(), StorageError> {
    let stored = store.cap_read(id).await?;
    if matches(&stored, capability) {
        Ok(())
    } else {
        Err(StorageError::Forbidden)
    }
}

pub async fn remove<S: CapStore>(store: &S, id: &str) {
    store.cap_delete(id).await;
}

pub async fn run_minted<S, T, F, Fut>(
    store: &S,
    id: &str,
    capability: Option<&str>,
    op: F,
) -> Result<T, StorageError>
where
    S: CapStore,
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<T, StorageError>>,
{
    if let Some(cap) = capability {
        mint(store, id, cap).await?;
    }
    let result = op().await;
    if result.is_err() && capability.is_some() {
        remove(store, id).await;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_stable_hex() {
        let a = hash("tok123");
        assert_eq!(a, hash("tok123"));
        assert_eq!(a.len(), 64);
        assert_ne!(a, hash("tok124"));
    }

    #[test]
    fn matches_trims_stored_bytes() {
        assert_eq!(matches("abc123\n", "tok"), hash("tok") == "abc123\n".trim());
        assert!(matches(&hash("tok"), "tok"));
        assert!(!matches(&hash("tok"), "other"));
        assert!(!matches("", "tok"));
    }

    struct MapStore {
        map: std::sync::Mutex<std::collections::HashMap<String, String>>,
    }

    #[async_trait::async_trait]
    impl CapStore for MapStore {
        async fn cap_write(&self, id: &str, hash: &str) -> Result<(), StorageError> {
            let mut map = self.map.lock().unwrap();
            if map.contains_key(id) {
                return Err(StorageError::Conflict);
            }
            map.insert(id.to_string(), hash.to_string());
            Ok(())
        }
        async fn cap_read(&self, id: &str) -> Result<String, StorageError> {
            self.map
                .lock()
                .unwrap()
                .get(id)
                .cloned()
                .ok_or(StorageError::Forbidden)
        }
        async fn cap_delete(&self, id: &str) {
            self.map.lock().unwrap().remove(id);
        }
    }

    #[tokio::test]
    async fn mint_verify_remove_roundtrip() {
        let store = MapStore {
            map: std::sync::Mutex::new(std::collections::HashMap::new()),
        };
        mint(&store, "f1", "tok").await.unwrap();

        assert!(mint(&store, "f1", "tok").await.is_err());
        verify(&store, "f1", "tok").await.unwrap();
        assert!(verify(&store, "f1", "wrong").await.is_err());
        assert!(verify(&store, "missing", "tok").await.is_err());
        remove(&store, "f1").await;
        assert!(verify(&store, "f1", "tok").await.is_err());
    }

    #[tokio::test]
    async fn run_minted_cleans_up_on_failure() {
        let store = MapStore {
            map: std::sync::Mutex::new(std::collections::HashMap::new()),
        };
        let ok: Result<(), StorageError> =
            run_minted(&store, "a", Some("tok"), || async { Ok(()) }).await;
        assert!(ok.is_ok());
        assert!(verify(&store, "a", "tok").await.is_ok());
        let err: Result<(), StorageError> = run_minted(&store, "b", Some("tok"), || async {
            Err(StorageError::Conflict)
        })
        .await;
        assert!(err.is_err());
        assert!(verify(&store, "b", "tok").await.is_err());

        let plain: Result<(), StorageError> =
            run_minted(&store, "c", None, || async { Ok(()) }).await;
        assert!(plain.is_ok());
    }
}
