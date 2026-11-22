use super::common::capability_hash;
use crate::error::StorageError;

#[async_trait::async_trait]
pub trait CapStore: Send + Sync {
    async fn cap_write(&self, id: &str, hash: &str) -> Result<(), StorageError>;

    async fn cap_read(&self, id: &str) -> Result<String, StorageError>;

    async fn cap_delete(&self, id: &str);

    /// Best-effort synchronous removal for cancellation cleanup: an async
    /// delete cannot run from `Drop`. Backends with filesystem sidecars
    /// override this; remote object stores keep the no-op default.
    fn cap_delete_blocking(&self, _id: &str) {}
}

#[must_use]
pub fn hash(capability: &str) -> String {
    capability_hash(capability)
}

/// Constant-time comparison against a stored digest, tolerating trailing
/// whitespace.
#[must_use]
pub fn matches(stored: &str, capability: &str) -> bool {
    juiceutils::constant_time_eq(stored.trim(), &hash(capability))
}

/// Binds a capability to an ID; fails with `Conflict` if one already exists.
pub async fn mint<S: CapStore>(store: &S, id: &str, capability: &str) -> Result<(), StorageError> {
    store.cap_write(id, &hash(capability)).await
}

/// Returns `Forbidden` on mismatch or when no capability is stored.
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

/// Removes a freshly minted capability on drop. When `op` is cancelled
/// mid-await (aborted upload / body deadline) neither the success path nor the
/// explicit error cleanup runs, so the sidecar must be reclaimed from `Drop`
/// or a retry with the same ID fails at `mint` with `Conflict`.
struct MintGuard<'a, S: CapStore> {
    store: &'a S,
    id: &'a str,
    armed: bool,
}

impl<S: CapStore> MintGuard<'_, S> {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl<S: CapStore> Drop for MintGuard<'_, S> {
    fn drop(&mut self) {
        if self.armed {
            self.store.cap_delete_blocking(self.id);
        }
    }
}

/// Mints before running `op`, then removes the capability if `op` fails.
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
    let mut guard = MintGuard {
        store,
        id,
        armed: capability.is_some(),
    };
    if let Some(cap) = capability {
        mint(store, id, cap).await?;
    }
    let result = op().await;
    if result.is_ok() {
        guard.disarm();
    } else if capability.is_some() {
        // The async delete still handles remote backends and keeps the normal
        // failure path observable to stores whose blocking hook is a no-op.
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
        let stored_with_newline = format!("{}\n", hash("tok"));
        assert!(matches(&stored_with_newline, "tok"));
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

        fn cap_delete_blocking(&self, id: &str) {
            self.map.lock().unwrap().remove(id);
        }
    }

    #[test]
    fn mint_guard_removes_sidecar_on_drop() {
        let store = MapStore {
            map: std::sync::Mutex::new(std::collections::HashMap::from([(
                "x".to_string(),
                hash("tok"),
            )])),
        };
        {
            let _guard = MintGuard {
                store: &store,
                id: "x",
                armed: true,
            };
        }
        assert!(!store.map.lock().unwrap().contains_key("x"));
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
