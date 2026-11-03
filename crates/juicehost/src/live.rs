use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use dashmap::DashMap;
use tokio::sync::Notify;

const STALE_AFTER: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Clone, Default)]
pub struct LiveUploads {
    uploads: Arc<DashMap<String, Arc<LiveUpload>>>,
    parts: Arc<DashMap<String, String>>,
}

struct LiveUpload {
    target_id: String,
    filename: String,
    mime: String,
    total_size: u64,
    state: Mutex<LiveState>,
    changed: Notify,
    touched: Mutex<Instant>,
}

enum LiveState {
    Single { written: u64, ready: bool },
    Parts(BTreeMap<u64, LivePart>),
}

struct LivePart {
    id: String,
    offset: u64,
    length: u64,
    written: u64,
    ready: bool,
}

#[derive(Debug, Clone)]
pub struct LiveSnapshot {
    pub target_id: String,
    pub filename: String,
    pub mime: String,
    pub total_size: u64,
    pub uploaded: u64,
    pub available: u64,
    pub complete: bool,
}

#[derive(Debug, Clone)]
pub struct LiveSource {
    pub upload_id: String,
    pub base_offset: u64,
    pub end: u64,
    pub ready: bool,
}

#[derive(Debug, Clone)]
pub struct LiveProgress {
    pub total_size: u64,
    pub uploaded: u64,
    pub available: u64,
    pub complete: bool,
    pub filename: String,
    pub mime: String,
}

#[derive(Clone)]
pub struct LiveWriter {
    live: LiveUploads,
    upload_id: String,
}

impl LiveWriter {
    pub fn add(&self, bytes: usize) {
        if bytes > 0 {
            self.live.add_written(&self.upload_id, bytes as u64);
        }
    }
}

impl LiveUpload {
    fn touch(&self) {
        if let Ok(mut touched) = self.touched.lock() {
            *touched = Instant::now();
        }
    }

    fn is_stale(&self) -> bool {
        self.touched
            .lock()
            .map(|touched| touched.elapsed() >= STALE_AFTER)
            .unwrap_or(false)
    }

    fn notify(&self) {
        self.touch();
        self.changed.notify_waiters();
    }
}

impl LiveUploads {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a single-stream upload whose bytes are written to the target
    /// ID directly. Returns the ID used by `track`.
    pub fn ensure_single(&self, id: &str, filename: &str, mime: &str, total_size: u64) {
        if self.parts.contains_key(id) {
            return;
        }
        match self.uploads.get(id) {
            Some(entry) => {
                let upload = entry.value();
                if upload.total_size == 0 && total_size > 0 {
                    // `total_size` is immutable, so replace the entry when the
                    // real size becomes known. The state is preserved.
                    let state = upload.state.lock().unwrap_or_else(|e| e.into_inner());
                    let written = match &*state {
                        LiveState::Single { written, .. } => *written,
                        LiveState::Parts(_) => 0,
                    };
                    let new = Arc::new(LiveUpload {
                        target_id: id.to_string(),
                        filename: filename.to_string(),
                        mime: mime.to_string(),
                        total_size,
                        state: Mutex::new(LiveState::Single {
                            written,
                            ready: false,
                        }),
                        changed: Notify::new(),
                        touched: Mutex::new(Instant::now()),
                    });
                    drop(state);
                    drop(entry);
                    self.uploads.insert(id.to_string(), new);
                } else {
                    upload.touch();
                }
            }
            None => {
                self.uploads.insert(
                    id.to_string(),
                    Arc::new(LiveUpload {
                        target_id: id.to_string(),
                        filename: filename.to_string(),
                        mime: mime.to_string(),
                        total_size,
                        state: Mutex::new(LiveState::Single {
                            written: 0,
                            ready: false,
                        }),
                        changed: Notify::new(),
                        touched: Mutex::new(Instant::now()),
                    }),
                );
            }
        }
    }

    /// Register part metadata supplied by juiceback. Parts are addressed by
    /// the target/reservation ID and may arrive in any order.
    pub fn register_parts(
        &self,
        target_id: &str,
        filename: &str,
        mime: &str,
        total_size: u64,
        parts: &[(String, u64, u64)],
    ) {
        if total_size == 0 || parts.is_empty() {
            return;
        }
        let entry = self
            .uploads
            .entry(target_id.to_string())
            .or_insert_with(|| {
                Arc::new(LiveUpload {
                    target_id: target_id.to_string(),
                    filename: filename.to_string(),
                    mime: mime.to_string(),
                    total_size,
                    state: Mutex::new(LiveState::Parts(BTreeMap::new())),
                    changed: Notify::new(),
                    touched: Mutex::new(Instant::now()),
                })
            })
            .clone();
        {
            let mut state = entry.state.lock().unwrap_or_else(|e| e.into_inner());
            if !matches!(*state, LiveState::Parts(_)) {
                *state = LiveState::Parts(BTreeMap::new());
            }
            if let LiveState::Parts(ref mut map) = *state {
                for (id, offset, length) in parts {
                    let part = map.entry(*offset).or_insert_with(|| LivePart {
                        id: id.clone(),
                        offset: *offset,
                        length: *length,
                        written: 0,
                        ready: false,
                    });
                    if part.id == *id {
                        part.length = part.length.max(*length);
                    }
                }
            }
        }
        for (id, _, _) in parts {
            self.parts.insert(id.clone(), target_id.to_string());
        }
        entry.notify();
    }

    #[must_use]
    pub fn is_part(&self, id: &str) -> bool {
        self.parts.contains_key(id)
    }

    /// Return a writer for an upload ID that is either a single target or a
    /// registered part. Returns `None` for unknown IDs.
    #[must_use]
    pub fn track(&self, id: &str) -> Option<LiveWriter> {
        let known = self.parts.contains_key(id) || self.uploads.contains_key(id);
        known.then(|| LiveWriter {
            live: self.clone(),
            upload_id: id.to_string(),
        })
    }

    pub fn add_written(&self, id: &str, bytes: u64) {
        if bytes == 0 {
            return;
        }
        if let Some(target) = self.parts.get(id).map(|v| v.value().clone()) {
            if let Some(entry) = self.uploads.get(&target) {
                let upload = entry.value();
                let mut state = upload.state.lock().unwrap_or_else(|e| e.into_inner());
                if let LiveState::Parts(ref mut map) = *state {
                    for part in map.values_mut() {
                        if part.id == id {
                            part.written = part.written.saturating_add(bytes).min(part.length);
                            break;
                        }
                    }
                }
                upload.notify();
            }
            return;
        }
        if let Some(entry) = self.uploads.get(id) {
            let upload = entry.value();
            let mut state = upload.state.lock().unwrap_or_else(|e| e.into_inner());
            if let LiveState::Single { written, .. } = &mut *state {
                *written = written.saturating_add(bytes);
                if upload.total_size > 0 {
                    *written = (*written).min(upload.total_size);
                }
            }
            upload.notify();
        }
    }

    pub fn mark_ready(&self, id: &str) {
        if let Some(target) = self.parts.get(id).map(|v| v.value().clone()) {
            if let Some(entry) = self.uploads.get(&target) {
                let upload = entry.value();
                let mut state = upload.state.lock().unwrap_or_else(|e| e.into_inner());
                if let LiveState::Parts(ref mut map) = *state {
                    for part in map.values_mut() {
                        if part.id == id {
                            part.ready = true;
                            part.written = part.length;
                            break;
                        }
                    }
                }
                upload.notify();
            }
            return;
        }
        if let Some(entry) = self.uploads.get(id) {
            let upload = entry.value();
            let mut state = upload.state.lock().unwrap_or_else(|e| e.into_inner());
            if let LiveState::Single { written, ready } = &mut *state {
                *ready = true;
                *written = upload.total_size.max(*written);
            }
            upload.notify();
        }
    }

    pub fn snapshot(&self, id: &str) -> Option<LiveSnapshot> {
        let entry = self.uploads.get(id)?;
        let upload = entry.value();
        if upload.is_stale() {
            drop(entry);
            self.remove_target(id);
            return None;
        }
        let state = upload.state.lock().unwrap_or_else(|e| e.into_inner());
        let (uploaded, available, complete) = match &*state {
            LiveState::Single { written, ready } => {
                let uploaded = if *ready {
                    upload.total_size.max(*written)
                } else {
                    *written
                };
                (uploaded, uploaded, *ready)
            }
            LiveState::Parts(map) => {
                let uploaded = map
                    .values()
                    .map(|p| p.written.min(p.length))
                    .sum::<u64>()
                    .min(upload.total_size);
                let mut cursor = 0u64;
                for part in map.values() {
                    if part.offset > cursor || part.written == 0 {
                        break;
                    }
                    let end = part.offset.saturating_add(part.written).min(upload.total_size);
                    if end > cursor {
                        cursor = end;
                    }
                    if part.offset.saturating_add(part.length) > cursor {
                        break;
                    }
                }
                let complete = !map.is_empty()
                    && map.values().all(|p| p.ready)
                    && map
                        .values()
                        .map(|p| p.length)
                        .sum::<u64>()
                        >= upload.total_size;
                (uploaded, cursor, complete)
            }
        };
        Some(LiveSnapshot {
            target_id: upload.target_id.clone(),
            filename: upload.filename.clone(),
            mime: upload.mime.clone(),
            total_size: upload.total_size,
            uploaded,
            available,
            complete,
        })
    }

    #[must_use]
    pub fn progress(&self, id: &str) -> Option<LiveProgress> {
        self.snapshot(id).map(|snapshot| LiveProgress {
            total_size: snapshot.total_size,
            uploaded: snapshot.uploaded,
            available: snapshot.available,
            complete: snapshot.complete,
            filename: snapshot.filename,
            mime: snapshot.mime,
        })
    }

    /// Locate the part/single stream containing `offset`, and report how far
    /// that source is currently readable. The caller maps the returned source
    /// to either a final storage object or the live temp file.
    pub fn source_at(&self, id: &str, offset: u64) -> Option<LiveSource> {
        let entry = self.uploads.get(id)?;
        let upload = entry.value();
        let state = upload.state.lock().unwrap_or_else(|e| e.into_inner());
        match &*state {
            LiveState::Single { written, ready } => {
                let end = if *ready {
                    upload.total_size.max(*written)
                } else {
                    *written
                };
                (offset < end).then(|| LiveSource {
                    upload_id: upload.target_id.clone(),
                    base_offset: 0,
                    end,
                    ready: *ready,
                })
            }
            LiveState::Parts(map) => {
                for part in map.values() {
                    let end = part.offset.saturating_add(part.written);
                    if offset >= part.offset && offset < end {
                        return Some(LiveSource {
                            upload_id: part.id.clone(),
                            base_offset: part.offset,
                            end: end.min(part.offset.saturating_add(part.length)),
                            ready: part.ready,
                        });
                    }
                }
                None
            }
        }
    }

    pub async fn wait_for_change(&self, id: &str, timeout: Duration) {
        let Some(entry) = self.uploads.get(id) else {
            tokio::time::sleep(timeout).await;
            return;
        };
        let upload = entry.value().clone();
        drop(entry);
        let notified = upload.changed.notified();
        tokio::pin!(notified);
        let _ = tokio::time::timeout(timeout, notified).await;
    }

    pub fn remove_target(&self, id: &str) {
        if let Some((_, upload)) = self.uploads.remove(id) {
            if let Ok(state) = upload.state.lock()
                && let LiveState::Parts(ref map) = *state
            {
                for part in map.values() {
                    self.parts.remove(&part.id);
                }
            }
        }
    }
}

impl std::fmt::Debug for LiveUploads {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveUploads")
            .field("uploads", &self.uploads.len())
            .field("parts", &self.parts.len())
            .finish()
    }
}
