//! The shared state every request handler can access

use std::sync::Arc;

use dashmap::DashMap;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;

use crate::{
    config::Config,
    error::AppError,
    storage_client::JuicehostConfig,
    tus::{PartSessionMap, TusMap, TusSenderMap},
};

pub type DbPool = Pool<SqliteConnectionManager>;

/// A connected device's WebSocket handle.
pub struct ConnectedDevice {
    pub device_id: String,
    pub device_name: String,
    pub sender: tokio::sync::mpsc::Sender<String>,
    pub connected_at: std::time::Instant,
    pub last_heartbeat: std::time::Instant,
}

/// Presence events broadcast to SSE listeners.
#[derive(Clone, Debug)]
pub enum PresenceEvent {
    DeviceConnected {
        device_id: String,
        device_name: String,
    },
    DeviceDisconnected {
        device_id: String,
    },
    PingResponse {
        device_id: String,
    },
}

/// Shared state that every axum handler can access.
#[derive(Clone)]
pub struct AppState {
    pub db: DbPool,
    pub config: Arc<Config>,
    pub http: reqwest::Client,
    pub tus: TusMap,
    /// Per-upload mpsc senders for TUS streaming (chunks flow directly to
    /// juicehost).
    pub tus_senders: TusSenderMap,
    /// Tracks multi-part parallel upload sessions.
    pub part_sessions: PartSessionMap,
    /// TUS storage tasks, awaited before publishing uploads or concatenating
    /// parts.
    pub push_handles: Arc<DashMap<String, tokio::task::JoinHandle<Result<(), String>>>>,
    /// In-memory set of banned IPs for fast O(1) lookups in middleware.
    pub banned_ips: DashMap<String, ()>,
    /// Pre-built juicehost authentication headers (built once at startup).
    pub juicehost_headers: reqwest::header::HeaderMap,
    /// Semaphore limiting concurrent direct (non-TUS) uploads.
    pub upload_semaphore: Arc<tokio::sync::Semaphore>,
    /// Juicehost-provided config (file size limits, TTL, danger level, etc.).
    /// `None` when juicehost is unreachable (degraded mode).
    pub jh_config: Arc<std::sync::RwLock<Option<Arc<JuicehostConfig>>>>,
    /// Persistent QUIC client endpoint for connection reuse.
    #[cfg(feature = "quic")]
    pub quic_endpoint: std::sync::Arc<tokio::sync::OnceCell<h3_quinn::quinn::Endpoint>>,
    /// Path to the QUIC TLS certificate for cert pinning.
    #[cfg(feature = "quic")]
    pub quic_cert_path: Option<std::path::PathBuf>,
    /// Connected device WebSocket connections, keyed by `user_id`.
    pub connected_devices: DashMap<String, Vec<Arc<tokio::sync::Mutex<ConnectedDevice>>>>,
    /// Broadcast senders for presence events, keyed by `user_id`.
    pub presence_listeners: DashMap<String, tokio::sync::broadcast::Sender<PresenceEvent>>,
    pub presence_semaphore: Arc<tokio::sync::Semaphore>,
    pub presence_by_ip: DashMap<std::net::IpAddr, usize>,
    pub notification_semaphore: Arc<tokio::sync::Semaphore>,
    pub mint_limiter: Arc<crate::mint_limiter::MintLimiter>,
}

impl AppState {
    pub fn new(
        pool: DbPool,
        config: Config,
        http: reqwest::Client,
        juicehost_headers: reqwest::header::HeaderMap,
        jh_config: Option<JuicehostConfig>,
    ) -> Arc<Self> {
        #[cfg(feature = "quic")]
        let quic_cert_path = config.quic_cert_path.clone();

        let mint_limiter = Arc::new(crate::mint_limiter::MintLimiter::new(
            config.dte_mint_limit,
            config.dte_mint_window_secs,
            config.dte_mint_burst,
        ));

        let max_concurrent_uploads = config.max_concurrent_uploads.max(1) as usize;

        Arc::new(Self {
            db: pool,
            config: Arc::new(config),
            http,
            tus: crate::tus::new_tus_state(),
            tus_senders: crate::tus::new_tus_sender_map(),
            part_sessions: crate::tus::new_part_session_map(),
            push_handles: Arc::new(DashMap::new()),
            banned_ips: DashMap::new(),
            juicehost_headers,
            upload_semaphore: Arc::new(tokio::sync::Semaphore::new(max_concurrent_uploads)),
            jh_config: Arc::new(std::sync::RwLock::new(jh_config.map(Arc::new))),
            #[cfg(feature = "quic")]
            quic_endpoint: std::sync::Arc::new(tokio::sync::OnceCell::new()),
            #[cfg(feature = "quic")]
            quic_cert_path,
            connected_devices: DashMap::new(),
            presence_listeners: DashMap::new(),
            presence_semaphore: Arc::new(tokio::sync::Semaphore::new(
                crate::constants::MAX_SSE_CONNECTIONS,
            )),
            presence_by_ip: DashMap::new(),
            notification_semaphore: Arc::new(tokio::sync::Semaphore::new(8)),
            mint_limiter,
        })
    }

    #[must_use]
    pub fn is_banned(&self, ip: &str) -> bool {
        self.banned_ips.contains_key(ip)
    }

    /// Clone the juicehost config out of the lock without holding the guard.
    /// Maps lock poisoning to `Internal` and a missing config to the
    /// degraded-mode `ServiceUnavailable` callers already return.
    ///
    /// # Errors
    ///
    /// Returns `AppError::Internal` when the lock is poisoned and
    /// `AppError::ServiceUnavailable` when juicehost is unreachable
    /// (degraded mode).
    pub fn juicehost_config(&self) -> Result<Arc<JuicehostConfig>, AppError> {
        let cfg = self
            .jh_config
            .read()
            .map_err(|_| AppError::Internal("config lock poisoned".into()))?
            .clone();
        cfg.ok_or_else(|| {
            AppError::ServiceUnavailable("juicehost config unavailable (degraded mode)".into())
        })
    }

    /// Reload the banned IPs set from the database.
    pub fn reload_banned_ips(&self) {
        if let Ok(conn) = self.db.get()
            && let Ok(ips) = crate::db::load_banned_ips_set(&conn)
        {
            self.banned_ips.clear();
            for ip in ips {
                self.banned_ips.insert(ip, ());
            }
            tracing::info!("reloaded {} banned IPs", self.banned_ips.len());
        }
    }

    /// Ban an IP: write to DB and update in-memory cache atomically.
    pub async fn ban_ip(
        self: &Arc<Self>,
        ip: &str,
        reason: &str,
        banned_by: &str,
    ) -> Result<(), AppError> {
        let ip_c = ip.to_string();
        let reason_c = reason.to_string();
        let banned_by_c = banned_by.to_string();
        self.db_call("ban_ip", move |db| {
            crate::db::ban_ip(db, &ip_c, &reason_c, &banned_by_c)
        })
        .await?;
        self.banned_ips.insert(ip.to_string(), ());
        Ok(())
    }

    /// Unban an IP: remove from DB and update in-memory cache atomically.
    pub async fn unban_ip(self: &Arc<Self>, ip: &str) -> Result<bool, AppError> {
        let ip_c = ip.to_string();
        let deleted = self
            .db_call("unban_ip", move |db| crate::db::unban_ip(db, &ip_c))
            .await?;
        if deleted {
            self.banned_ips.remove(ip);
        }
        Ok(deleted)
    }

    /// Import many bans: write to DB in one transaction and update the
    /// in-memory cache.
    pub async fn import_bans(
        self: &Arc<Self>,
        bans: Vec<crate::db::ImportBan>,
    ) -> Result<usize, AppError> {
        let hashes: Vec<String> = bans.iter().map(|b| b.ip.clone()).collect();
        let count = self
            .db_call("insert_banned_ips", move |db| {
                crate::db::insert_banned_ips(db, &bans)
            })
            .await?;
        for h in hashes {
            self.banned_ips.insert(h, ());
        }
        Ok(count)
    }

    /// Run a database query on a background thread via r2d2 pool.
    pub async fn db_call<T, F>(self: &Arc<Self>, name: &'static str, f: F) -> Result<T, AppError>
    where
        F: FnOnce(&rusqlite::Connection) -> Result<T, rusqlite::Error> + Send + 'static,
        T: Send + 'static,
    {
        let pool = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool
                .get()
                .map_err(|e| AppError::DbPoolError(e.to_string()))?;
            f(&conn).map_err(AppError::DatabaseError)
        })
        .await
        .map_err(|_| AppError::TaskPanicked(format!("db task '{name}' panicked")))?
    }

    /// Run several statements atomically on one pooled connection. The
    /// closure receives a transaction: returning `Err` rolls everything back,
    /// returning `Ok` commits. Prefer this over sequential [`db_call`]s when
    /// the steps must succeed or fail together (each `db_call` otherwise
    /// checks out its own connection, which cannot share a transaction).
    pub async fn db_transaction<T, F>(
        self: &Arc<Self>,
        name: &'static str,
        f: F,
    ) -> Result<T, AppError>
    where
        F: FnOnce(&rusqlite::Transaction) -> Result<T, rusqlite::Error> + Send + 'static,
        T: Send + 'static,
    {
        let pool = self.db.clone();
        tokio::task::spawn_blocking(move || run_db_transaction(&pool, f))
            .await
            .map_err(|_| AppError::TaskPanicked(format!("db task '{name}' panicked")))?
    }
}

/// Synchronous core of [`AppState::db_transaction`]: one pooled connection,
/// one transaction, commit on `Ok`, rollback (via drop) on `Err`.
fn run_db_transaction<T, F>(pool: &DbPool, f: F) -> Result<T, AppError>
where
    F: FnOnce(&rusqlite::Transaction) -> Result<T, rusqlite::Error>,
{
    let mut conn = pool
        .get()
        .map_err(|e| AppError::DbPoolError(e.to_string()))?;
    let tx = conn.transaction().map_err(|e| AppError::DatabaseError(e))?;
    let out = f(&tx).map_err(AppError::DatabaseError)?;
    tx.commit().map_err(AppError::DatabaseError)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory_pool() -> DbPool {
        Pool::builder()
            .max_size(1)
            .build(r2d2_sqlite::SqliteConnectionManager::memory())
            .unwrap()
    }

    fn table_count(pool: &DbPool) -> i64 {
        pool.get()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM t", [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn transaction_commits_on_ok() {
        let pool = memory_pool();
        run_db_transaction(&pool, |tx| {
            tx.execute("CREATE TABLE t (id INTEGER PRIMARY KEY)", [])?;
            tx.execute("INSERT INTO t (id) VALUES (1)", [])?;
            Ok(())
        })
        .unwrap();
        assert_eq!(table_count(&pool), 1);
    }

    #[test]
    fn transaction_rolls_back_on_err() {
        let pool = memory_pool();
        run_db_transaction(&pool, |tx| {
            tx.execute("CREATE TABLE t (id INTEGER PRIMARY KEY)", [])?;
            tx.execute("INSERT INTO t (id) VALUES (1)", [])?;
            Ok(())
        })
        .unwrap();
        let failed: Result<(), AppError> = run_db_transaction(&pool, |tx| {
            tx.execute("INSERT INTO t (id) VALUES (2)", [])?;
            // Primary-key clash: the whole transaction must roll back.
            tx.execute("INSERT INTO t (id) VALUES (2)", [])?;
            Ok(())
        });
        assert!(failed.is_err());
        assert_eq!(table_count(&pool), 1);
    }
}
