//! In-memory session and file-id registry plus the relay side of the peer
//! WebSocket protocol.
//!
//! A session owns a bounded queue of server messages and a map of in-flight
//! browser requests. File ids are registered only after juiceback authorizes
//! them, and identity always comes from the verified device JWT.

use std::{
    cell::Cell,
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    body::Bytes,
    extract::ws::{CloseFrame, Message, WebSocket},
    http::StatusCode,
};
use futures::{SinkExt, StreamExt, stream::SplitSink};
use juiceproto::peer::{
    PEER_PROTOCOL_VERSION, PeerClientMessage, PeerDeviceClaims, PeerRejection, PeerServerMessage,
    verify_peer_token,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot, watch};

use crate::relay::RelayState;

const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const CHUNK_SEND_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_ANNOUNCE_IDS: usize = 1_024;
const MAX_ID_LENGTH: usize = 128;
const CLOSE_POLICY: u16 = 1008;

/// Response metadata sent by the peer before its binary chunk frames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerResponseHead {
    pub status: u16,
    pub headers: Vec<(String, String)>,
}

/// One item on the bounded browser-facing chunk channel.
#[derive(Debug)]
pub enum ChunkItem {
    Data(Bytes),
    Fail(String),
}

type HeadSender = oneshot::Sender<Result<PeerResponseHead, String>>;

struct PendingRequest {
    /// Taken once the response head has been delivered; the chunk sender
    /// stays registered until `ResponseEnd` or an abort.
    head: Option<HeadSender>,
    chunks: mpsc::Sender<ChunkItem>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A verified peer connection and its in-flight requests.
pub struct PeerSession {
    pub id: String,
    pub device_id: String,
    pub user_id: String,
    outbound: mpsc::Sender<PeerServerMessage>,
    pending: Mutex<HashMap<u64, PendingRequest>>,
    last_pong: Mutex<Instant>,
    shutdown: watch::Sender<bool>,
}

impl PeerSession {
    fn new(
        id: String,
        device_id: String,
        user_id: String,
        outbound: mpsc::Sender<PeerServerMessage>,
    ) -> Self {
        Self {
            id,
            device_id,
            user_id,
            outbound,
            pending: Mutex::new(HashMap::new()),
            last_pong: Mutex::new(Instant::now()),
            shutdown: watch::Sender::new(false),
        }
    }

    /// Notifies the owning WebSocket loop that this session was replaced.
    fn shutdown(&self) {
        let _ = self.shutdown.send(true);
    }

    fn shutdown_signal(&self) -> watch::Receiver<bool> {
        self.shutdown.subscribe()
    }

    pub(crate) fn try_send(&self, message: PeerServerMessage) -> bool {
        self.outbound.try_send(message).is_ok()
    }

    pub(crate) async fn send(
        &self,
        message: PeerServerMessage,
        timeout: Duration,
    ) -> Result<(), &'static str> {
        tokio::time::timeout(timeout, self.outbound.send(message))
            .await
            .map_err(|_| "peer send timed out")?
            .map_err(|_| "peer send failed")
    }

    fn note_pong(&self) {
        *lock(&self.last_pong) = Instant::now();
    }

    fn is_stale(&self, threshold: Duration) -> bool {
        lock(&self.last_pong).elapsed() > threshold
    }

    pub(crate) fn insert_pending(
        &self,
        request_id: u64,
        head: HeadSender,
        chunks: mpsc::Sender<ChunkItem>,
    ) -> bool {
        let mut pending = lock(&self.pending);
        if pending.contains_key(&request_id) {
            return false;
        }
        pending.insert(
            request_id,
            PendingRequest {
                head: Some(head),
                chunks,
            },
        );
        true
    }

    fn remove_pending(&self, request_id: u64) -> Option<PendingRequest> {
        lock(&self.pending).remove(&request_id)
    }

    fn chunk_sender(&self, request_id: u64) -> Option<mpsc::Sender<ChunkItem>> {
        lock(&self.pending)
            .get(&request_id)
            .map(|pending| pending.chunks.clone())
    }

    fn deliver_head(&self, request_id: u64, status: u16, headers: Vec<(String, String)>) -> bool {
        let mut pending_map = lock(&self.pending);
        let Some(pending) = pending_map.get_mut(&request_id) else {
            return false;
        };
        let Some(head) = pending.head.take() else {
            return false;
        };
        drop(pending_map);
        head.send(Ok(PeerResponseHead { status, headers })).is_ok()
    }

    fn fail_request(&self, request_id: u64, reason: String) {
        let Some(pending) = self.remove_pending(request_id) else {
            return;
        };
        if let Some(head) = pending.head {
            let _ = head.send(Err(reason.clone()));
        }
        let _ = pending.chunks.try_send(ChunkItem::Fail(reason));
    }

    /// Fails every waiting request and asks the peer to stop sending chunks.
    fn abort_all(&self, reason: &str) {
        let pending = std::mem::take(&mut *lock(&self.pending));
        for (request_id, pending) in pending {
            let _ = self
                .outbound
                .try_send(PeerServerMessage::EndChunks { request_id });
            if let Some(head) = pending.head {
                let _ = head.send(Err(reason.to_owned()));
            }
            let _ = pending.chunks.try_send(ChunkItem::Fail(reason.to_owned()));
        }
    }
}

#[derive(Default)]
struct RegistryState {
    sessions: HashMap<String, Arc<PeerSession>>,
    by_device: HashMap<String, String>,
    by_file: HashMap<String, String>,
    session_seq: u64,
    request_seq: u64,
    active_requests: usize,
}

/// Shared, bounded mapping of file id to the session that serves it.
#[derive(Clone, Default)]
pub struct SessionRegistry {
    inner: Arc<Mutex<RegistryState>>,
}

impl SessionRegistry {
    pub(crate) fn next_session_id(&self) -> String {
        let mut state = lock(&self.inner);
        state.session_seq = state.session_seq.wrapping_add(1);
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let session_id = format!("s-{millis}-{}", state.session_seq);
        drop(state);
        session_id
    }

    /// A new connection for a device replaces its previous session.
    pub(crate) fn insert_session(
        &self,
        session: Arc<PeerSession>,
        max_sessions: usize,
    ) -> Result<(), &'static str> {
        let mut state = lock(&self.inner);
        let previous = state.by_device.get(&session.device_id).cloned();
        if previous.is_none() && state.sessions.len() >= max_sessions {
            return Err("session_limit");
        }
        if let Some(previous) = previous {
            detach(&mut state, &previous);
        }
        state
            .by_device
            .insert(session.device_id.clone(), session.id.clone());
        state.sessions.insert(session.id.clone(), session);
        drop(state);
        Ok(())
    }

    pub(crate) fn remove_session(&self, session_id: &str) -> Option<Arc<PeerSession>> {
        let mut state = lock(&self.inner);
        detach(&mut state, session_id)
    }

    /// Registers only ids that pass validation and are not served elsewhere.
    pub(crate) fn register_ids(
        &self,
        session_id: &str,
        ids: &[String],
    ) -> (Vec<String>, Vec<PeerRejection>) {
        let mut state = lock(&self.inner);
        let mut accepted = Vec::new();
        let mut rejected = Vec::new();
        for id in ids {
            let reason = if !valid_file_id(id) {
                Some("invalid file id")
            } else if state
                .by_file
                .get(id)
                .is_some_and(|owner| owner != session_id)
            {
                Some("served by another device")
            } else {
                None
            };
            if let Some(reason) = reason {
                rejected.push(PeerRejection {
                    id: id.clone(),
                    reason: reason.to_owned(),
                });
            } else {
                state.by_file.insert(id.clone(), session_id.to_owned());
                accepted.push(id.clone());
            }
        }
        drop(state);
        (accepted, rejected)
    }

    pub(crate) fn revoke_ids(&self, session_id: &str, ids: &[String]) {
        let mut state = lock(&self.inner);
        state.by_file.retain(|id, owner| {
            owner.as_str() != session_id || !ids.iter().any(|revoked| revoked == id)
        });
    }

    pub(crate) fn lookup(&self, id: &str) -> Option<Arc<PeerSession>> {
        let state = lock(&self.inner);
        let session_id = state.by_file.get(id)?;
        state.sessions.get(session_id).cloned()
    }

    /// Allocates a request id together with a guard that releases the slot.
    pub(crate) fn begin_request(
        &self,
        session: Arc<PeerSession>,
        max_concurrent: usize,
    ) -> Result<(u64, StreamGuard), &'static str> {
        let mut state = lock(&self.inner);
        if !state.sessions.contains_key(&session.id) {
            return Err("session_offline");
        }
        if state.active_requests >= max_concurrent {
            return Err("busy");
        }
        state.active_requests += 1;
        state.request_seq = state.request_seq.wrapping_add(1);
        if state.request_seq == 0 {
            state.request_seq = 1;
        }
        let request_id = state.request_seq;
        let guard = StreamGuard::new(self.clone(), session, request_id);
        drop(state);
        Ok((request_id, guard))
    }

    fn end_request(&self) {
        let mut state = lock(&self.inner);
        state.active_requests = state.active_requests.saturating_sub(1);
    }

    #[must_use]
    pub fn stats(&self) -> (usize, usize, usize) {
        let state = lock(&self.inner);
        (
            state.sessions.len(),
            state.by_file.len(),
            state.active_requests,
        )
    }
}

fn detach(state: &mut RegistryState, session_id: &str) -> Option<Arc<PeerSession>> {
    state.by_device.retain(|_, owner| owner != session_id);
    state.by_file.retain(|_, owner| owner != session_id);
    let removed = state.sessions.remove(session_id);
    if let Some(session) = &removed {
        session.shutdown();
    }
    removed
}

pub(crate) fn valid_file_id(id: &str) -> bool {
    juiceutils::ids::is_valid_id_component(id) && id.len() <= MAX_ID_LENGTH
}

/// Releases a request slot and aborts the peer mid-stream when the browser
/// goes away or the stream times out.
pub struct StreamGuard {
    registry: SessionRegistry,
    session: Arc<PeerSession>,
    request_id: u64,
    send_end_chunks: Cell<bool>,
}

impl StreamGuard {
    const fn new(registry: SessionRegistry, session: Arc<PeerSession>, request_id: u64) -> Self {
        Self {
            registry,
            session,
            request_id,
            send_end_chunks: Cell::new(true),
        }
    }

    /// The peer sent `ResponseEnd`; no abort is needed.
    pub fn complete(&mut self) {
        self.send_end_chunks.set(false);
    }

    /// An error occurred; stop the peer and drop its channel sender.
    pub fn abandon(&mut self) {
        self.send_end_chunks.set(true);
        let _ = self.session.remove_pending(self.request_id);
    }
}

impl Drop for StreamGuard {
    fn drop(&mut self) {
        if self.send_end_chunks.get() {
            let _ = self.session.try_send(PeerServerMessage::EndChunks {
                request_id: self.request_id,
            });
            let _ = self.session.remove_pending(self.request_id);
        }
        self.registry.end_request();
    }
}

struct SessionGuard {
    registry: SessionRegistry,
    session: Arc<PeerSession>,
    session_id: String,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        let _ = self.registry.remove_session(&self.session_id);
        self.session.abort_all("peer disconnected");
    }
}

/// Runs one peer connection from the HTTP upgrade to socket teardown.
pub async fn serve_peer_socket(socket: WebSocket, state: Arc<RelayState>) {
    let mut socket = socket;
    let (version, token) = match read_hello(&mut socket).await {
        Ok(hello) => hello,
        Err(code) => {
            reject_socket(&mut socket, code).await;
            return;
        }
    };
    if version != PEER_PROTOCOL_VERSION {
        reject_socket(&mut socket, "unsupported_version").await;
        return;
    }
    let Ok(claims) = verify_peer_token(&token, &state.config.jwt_secret) else {
        reject_socket(&mut socket, "invalid_token").await;
        return;
    };
    if claims.sub.is_empty() || claims.user_id.is_empty() {
        reject_socket(&mut socket, "invalid_token").await;
        return;
    }
    let Some((session, receiver)) = register_peer(&state, &claims, &mut socket).await else {
        return;
    };
    let _guard = SessionGuard {
        registry: state.registry.clone(),
        session: Arc::clone(&session),
        session_id: session.id.clone(),
    };
    run_socket_loop(socket, state, session, receiver).await;
}

async fn register_peer(
    state: &Arc<RelayState>,
    claims: &PeerDeviceClaims,
    socket: &mut WebSocket,
) -> Option<(Arc<PeerSession>, mpsc::Receiver<PeerServerMessage>)> {
    let session_id = state.registry.next_session_id();
    let queue = state.config.max_concurrent_requests.clamp(16, 256);
    let (outbound, receiver) = mpsc::channel::<PeerServerMessage>(queue);
    let session = Arc::new(PeerSession::new(
        session_id.clone(),
        claims.sub.clone(),
        claims.user_id.clone(),
        outbound,
    ));
    if state
        .registry
        .insert_session(Arc::clone(&session), state.config.max_sessions)
        .is_err()
    {
        reject_socket(socket, "session_limit").await;
        return None;
    }
    let hello_ok = serde_json::to_string(&PeerServerMessage::HelloOk { session_id })
        .unwrap_or_else(|_| "{}".to_owned());
    if socket.send(Message::Text(hello_ok.into())).await.is_err() {
        return None;
    }
    Some((session, receiver))
}

async fn read_hello(socket: &mut WebSocket) -> Result<(u16, String), &'static str> {
    let message = tokio::time::timeout(HELLO_TIMEOUT, socket.next())
        .await
        .map_err(|_| "hello_timeout")?
        .ok_or("no_hello")?
        .map_err(|_| "bad_hello")?;
    let Message::Text(text) = message else {
        return Err("bad_hello");
    };
    match serde_json::from_str::<PeerClientMessage>(&text) {
        Ok(PeerClientMessage::Hello { version, token }) => Ok((version, token)),
        _ => Err("bad_hello"),
    }
}

async fn reject_socket(socket: &mut WebSocket, code: &'static str) {
    let message = match code {
        "unsupported_version" => "unsupported peer protocol version",
        "invalid_token" => "device JWT verification failed",
        "session_limit" => "relay is at capacity",
        _ => "invalid hello",
    };
    let payload = serde_json::to_string(&error(code, message)).unwrap_or_else(|_| "{}".to_owned());
    let _ = socket.send(Message::Text(payload.into())).await;
    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code: CLOSE_POLICY,
            reason: "policy violation".into(),
        })))
        .await;
}

async fn run_socket_loop(
    socket: WebSocket,
    state: Arc<RelayState>,
    session: Arc<PeerSession>,
    mut outbound: mpsc::Receiver<PeerServerMessage>,
) {
    let (mut sink, mut stream) = socket.split();
    let mut shutdown = session.shutdown_signal();
    let mut ticker = tokio::time::interval(state.config.heartbeat_interval());
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await;

    loop {
        tokio::select! {
            incoming = stream.next() => {
                if !handle_incoming(incoming, &state, &session, &mut sink).await {
                    break;
                }
            }
            outgoing = outbound.recv() => {
                let Some(message) = outgoing else { break };
                if send_json(&mut sink, message).await.is_err() {
                    break;
                }
            }
            _ = shutdown.changed() => {
                break;
            }
            _ = ticker.tick() => {
                let stale_for = state.config.heartbeat_interval().saturating_mul(2);
                if session.is_stale(stale_for) {
                    let _ = send_json(&mut sink, error("heartbeat_timeout", "peer is stale")).await;
                    break;
                }
                if send_json(&mut sink, PeerServerMessage::Ping).await.is_err() {
                    break;
                }
            }
        }
    }
}

type SocketSink = SplitSink<WebSocket, Message>;

async fn handle_incoming(
    incoming: Option<Result<Message, axum::Error>>,
    state: &Arc<RelayState>,
    session: &Arc<PeerSession>,
    sink: &mut SocketSink,
) -> bool {
    match incoming {
        Some(Ok(Message::Text(text))) => {
            let Ok(message) = serde_json::from_str::<PeerClientMessage>(&text) else {
                tracing::debug!("peer {} sent malformed JSON", session.id);
                return true;
            };
            match handle_client_message(state, session, message).await {
                Some(Control::Reply(reply)) => send_json(sink, reply).await.is_ok(),
                Some(Control::Fatal(reply)) => {
                    let _ = send_json(sink, reply).await;
                    false
                }
                None => true,
            }
        }
        Some(Ok(Message::Binary(payload))) => {
            let frame_limit = state.config.max_frame_bytes;
            match handle_binary(payload, session, frame_limit).await {
                Some(reply) => send_json(sink, reply).await.is_ok(),
                None => true,
            }
        }
        Some(Ok(Message::Pong(_))) => {
            session.note_pong();
            true
        }
        Some(Ok(Message::Ping(_))) => true,
        Some(Ok(Message::Close(_)) | Err(_)) | None => false,
    }
}

async fn send_json(sink: &mut SocketSink, message: PeerServerMessage) -> Result<(), ()> {
    let payload = serde_json::to_string(&message).map_err(|_| ())?;
    sink.send(Message::Text(payload.into()))
        .await
        .map_err(|_| ())
}

enum Control {
    Reply(PeerServerMessage),
    Fatal(PeerServerMessage),
}

async fn handle_client_message(
    state: &Arc<RelayState>,
    session: &Arc<PeerSession>,
    message: PeerClientMessage,
) -> Option<Control> {
    match message {
        PeerClientMessage::Hello { .. } => {
            Some(Control::Reply(error("protocol", "unexpected hello")))
        }
        PeerClientMessage::Announce { files } => Some(announce(state, session, files).await),
        PeerClientMessage::Revoke { ids } => {
            state.registry.revoke_ids(&session.id, &ids);
            None
        }
        PeerClientMessage::ResponseHead {
            request_id,
            status,
            headers,
        } => {
            if (200..600).contains(&status) {
                session.deliver_head(request_id, status, headers);
            } else {
                session.fail_request(request_id, "invalid response status".to_owned());
            }
            None
        }
        PeerClientMessage::ResponseEnd { request_id } => {
            let _ = session.remove_pending(request_id);
            None
        }
        PeerClientMessage::Abort { request_id, reason } => {
            session.fail_request(request_id, format!("peer aborted: {reason}"));
            None
        }
        PeerClientMessage::Pong => {
            session.note_pong();
            None
        }
    }
}

/// Asks juiceback which announced ids this verified device may serve.
async fn announce(
    state: &Arc<RelayState>,
    session: &Arc<PeerSession>,
    files: Vec<juiceproto::peer::PeerFileSpec>,
) -> Control {
    let ids = files.into_iter().map(|file| file.id).collect::<Vec<_>>();
    if ids.len() > MAX_ANNOUNCE_IDS {
        return Control::Reply(error("too_many_files", "too many announced files"));
    }
    if ids.is_empty() {
        return Control::Reply(PeerServerMessage::AnnounceOk {
            accepted: Vec::new(),
            rejected: Vec::new(),
        });
    }
    match authorize_files(state, &session.device_id, &session.user_id, &ids).await {
        Ok(authorized) => {
            let (accepted, mut rejected) = state
                .registry
                .register_ids(&session.id, &authorized.accepted);
            rejected.extend(authorized.rejected);
            Control::Reply(PeerServerMessage::AnnounceOk { accepted, rejected })
        }
        Err(AuthorizeError::Unauthorized) => Control::Fatal(error(
            "unauthorized",
            "juiceback rejected the relay API key",
        )),
        Err(AuthorizeError::Backend(reason)) => {
            tracing::warn!("peer authorize failed: {reason}");
            Control::Reply(error(
                "authorize_failed",
                "could not authorize announced files",
            ))
        }
    }
}

#[derive(Serialize)]
struct AuthorizeRequest<'a> {
    device_id: &'a str,
    user_id: &'a str,
    ids: &'a [String],
}

#[derive(Deserialize)]
struct AuthorizeResponse {
    #[serde(default)]
    accepted: Vec<String>,
    #[serde(default)]
    rejected: Vec<PeerRejection>,
}

enum AuthorizeError {
    Unauthorized,
    Backend(String),
}

async fn authorize_files(
    state: &RelayState,
    device_id: &str,
    user_id: &str,
    ids: &[String],
) -> Result<AuthorizeResponse, AuthorizeError> {
    let url = format!(
        "{}/internal/peer/authorize",
        state.config.juiceback_url.trim_end_matches('/')
    );
    let body = AuthorizeRequest {
        device_id,
        user_id,
        ids,
    };
    let response = state
        .client
        .post(url)
        .header("x-juicehost-api-key", &state.config.api_key)
        .json(&body)
        .send()
        .await
        .map_err(|error| AuthorizeError::Backend(error.to_string()))?;
    if response.status() == StatusCode::UNAUTHORIZED {
        return Err(AuthorizeError::Unauthorized);
    }
    if !response.status().is_success() {
        return Err(AuthorizeError::Backend(format!(
            "backend returned {}",
            response.status()
        )));
    }
    response
        .json::<AuthorizeResponse>()
        .await
        .map_err(|error| AuthorizeError::Backend(error.to_string()))
}

async fn handle_binary(
    payload: Bytes,
    session: &PeerSession,
    max_frame_bytes: usize,
) -> Option<PeerServerMessage> {
    if payload.len() < 8 {
        return Some(error("bad_frame", "binary frame is missing a request id"));
    }
    if payload.len() - 8 > max_frame_bytes {
        return Some(error(
            "frame_too_large",
            "binary frame exceeds the size limit",
        ));
    }
    let request_id = u64::from_be_bytes(
        payload[..8]
            .try_into()
            .expect("slice of eight bytes converts to an array"),
    );
    let Some(sender) = session.chunk_sender(request_id) else {
        return Some(PeerServerMessage::EndChunks { request_id });
    };
    let chunk = payload.slice(8..);
    match tokio::time::timeout(CHUNK_SEND_TIMEOUT, sender.send(ChunkItem::Data(chunk))).await {
        Ok(Ok(())) => None,
        _ => Some(PeerServerMessage::EndChunks { request_id }),
    }
}

fn error(code: &str, message: &str) -> PeerServerMessage {
    PeerServerMessage::Error {
        code: code.to_owned(),
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, device: &str) -> Arc<PeerSession> {
        let (tx, _rx) = mpsc::channel(4);
        Arc::new(PeerSession::new(
            id.to_owned(),
            device.to_owned(),
            "user-1".to_owned(),
            tx,
        ))
    }

    #[test]
    fn registry_registers_looks_up_and_revokes_ids() {
        let registry = SessionRegistry::default();
        let peer = session("s1", "device-1");
        registry.insert_session(Arc::clone(&peer), 4).unwrap();
        let ids = vec!["file-1".to_owned(), "bad id".to_owned()];
        let (accepted, rejected) = registry.register_ids("s1", &ids);
        assert_eq!(accepted, vec!["file-1"]);
        assert_eq!(rejected.len(), 1);
        assert!(registry.lookup("file-1").is_some());
        registry.revoke_ids("s1", &["file-1".to_owned()]);
        assert!(registry.lookup("file-1").is_none());
    }

    #[test]
    fn ids_cannot_be_hijacked_by_another_peer() {
        let registry = SessionRegistry::default();
        registry
            .insert_session(session("s1", "device-1"), 4)
            .unwrap();
        registry
            .insert_session(session("s2", "device-2"), 4)
            .unwrap();
        let (accepted, _) = registry.register_ids("s1", &["file-1".to_owned()]);
        assert_eq!(accepted.len(), 1);
        let (accepted, rejected) = registry.register_ids("s2", &["file-1".to_owned()]);
        assert!(accepted.is_empty());
        assert_eq!(rejected[0].reason, "served by another device");
    }

    #[test]
    fn session_limit_and_reconnect_are_enforced() {
        let registry = SessionRegistry::default();
        registry
            .insert_session(session("s1", "device-1"), 1)
            .unwrap();
        assert_eq!(
            registry.insert_session(session("s2", "device-2"), 1),
            Err("session_limit")
        );
        registry
            .insert_session(session("s3", "device-1"), 1)
            .unwrap();
        assert_eq!(registry.stats().0, 1);
    }

    #[test]
    fn request_slots_are_bounded_and_released() {
        let registry = SessionRegistry::default();
        let peer = session("s1", "device-1");
        registry.insert_session(Arc::clone(&peer), 4).unwrap();
        let (_id, guard) = registry.begin_request(Arc::clone(&peer), 1).unwrap();
        assert_eq!(registry.stats().2, 1);
        assert_eq!(
            registry.begin_request(Arc::clone(&peer), 1).err(),
            Some("busy")
        );
        drop(guard);
        assert_eq!(registry.stats().2, 0);
    }
}
