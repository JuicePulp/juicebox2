//! Outbound WebSocket client for the native juicepeer peer.
//!
//! The client dials the public relay, authenticates with a device JWT, announces
//! the files it is willing to serve, and streams bytes from a **loopback-only**
//! local juicehost. Every fetch target is validated before it leaves the
//! process: the base URL must be `http://127.0.0.1:<port>` or
//! `http://localhost:<port>`, and request paths are restricted to the public
//! `v`/`f`/`d`/`e` routes with a safe `{id}.{ext}` shape.
//!
//! JSON text frames carry [`PeerClientMessage`] / [`PeerServerMessage`]. Binary
//! frames carry a big-endian `u64` request id followed by raw bytes and are
//! terminated by a JSON [`PeerClientMessage::ResponseEnd`]. A relay-sent
//! [`PeerServerMessage::EndChunks`] or [`PeerServerMessage::Abort`] stops the
//! corresponding stream immediately.

use std::{
    collections::{HashMap, HashSet},
    fmt,
    net::Ipv4Addr,
    sync::Arc,
    time::Duration,
};

use futures::{
    SinkExt, StreamExt,
    stream::{SplitSink, SplitStream},
};
use juiceproto::peer::{PEER_PROTOCOL_VERSION, PeerClientMessage, PeerFileSpec, PeerServerMessage};
use reqwest::header::{HeaderName, HeaderValue};
use tokio::{
    net::TcpStream,
    sync::{Mutex, mpsc, watch},
    task::JoinHandle,
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{
        client::IntoClientRequest, http::HeaderValue as WsHeaderValue, protocol::Message,
    },
};
use url::{Host, Url};

/// Fallback local juicehost base when the caller supplies an empty string.
pub const DEFAULT_LOCAL_BASE: &str = "http://127.0.0.1:6402";

/// Bytes of file data per binary frame. Well under the relay's minimum frame
/// limit so a frame is never rejected for being too large.
const CHUNK_BYTES: usize = 16 * 1024;
const MAX_ID_LENGTH: usize = 128;
const MAX_EXT_LENGTH: usize = 16;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const CONTROL_QUEUE: usize = 16;
const RECONNECT_BASE_DELAY: Duration = Duration::from_secs(1);
const RECONNECT_MAX_DELAY: Duration = Duration::from_secs(30);

const FORWARDED_REQUEST_HEADERS: &[&str] = &["range", "if-none-match", "accept", "user-agent"];
const RESPONSE_HEADERS: &[&str] = &[
    "content-type",
    "content-length",
    "content-range",
    "accept-ranges",
    "etag",
    "cache-control",
    "content-disposition",
    "content-security-policy",
    "x-content-type-options",
];

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;
type SocketSink = SplitSink<Socket, Message>;
type SocketStream = SplitStream<Socket>;

/// Errors produced while connecting to or running a peer client.
#[derive(Debug)]
pub enum PeerClientError {
    /// The supplied local base is not a loopback-only HTTP URL.
    InvalidLocalBase(String),
    /// The relay URL or JWT cannot be turned into a WebSocket request.
    InvalidConfiguration(String),
    /// The WebSocket handshake or connection failed.
    Connection(String),
    /// The relay did not complete the Hello/Announce handshake correctly.
    Handshake(String),
    /// Sending to or receiving from the relay failed during the run loop.
    Transport(String),
}

impl fmt::Display for PeerClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (label, detail) = match self {
            Self::InvalidLocalBase(v) => ("invalid local base", v),
            Self::InvalidConfiguration(v) => ("invalid client configuration", v),
            Self::Connection(v) => ("relay connection failed", v),
            Self::Handshake(v) => ("relay handshake failed", v),
            Self::Transport(v) => ("relay transport failed", v),
        };
        write!(f, "{label}: {detail}")
    }
}

impl std::error::Error for PeerClientError {}

/// Normalizes a local juicehost base URL, rejecting every non-loopback target.
///
/// An empty (or whitespace-only) string resolves to [`DEFAULT_LOCAL_BASE`].
/// Only `http` (never `https`/`ws`/`wss`), an exact `127.0.0.1` or `localhost`
/// host, and an explicit non-zero port are accepted. Paths, queries,
/// fragments and user-info are rejected so the base cannot be widened later.
///
/// # Errors
///
/// Returns [`PeerClientError::InvalidLocalBase`] when the input is not a
/// loopback-only HTTP URL with an explicit port.
pub fn normalized_local_base(local_base: &str) -> Result<String, PeerClientError> {
    let trimmed = local_base.trim();
    let raw = if trimmed.is_empty() {
        DEFAULT_LOCAL_BASE
    } else {
        trimmed
    };
    let url = Url::parse(raw).map_err(|_| PeerClientError::InvalidLocalBase(raw.to_owned()))?;
    let Some(port) = url.port() else {
        return Err(PeerClientError::InvalidLocalBase(raw.to_owned()));
    };
    let valid = url.scheme() == "http"
        && port != 0
        && loopback_host(&url)
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && matches!(url.path(), "" | "/");
    if !valid {
        return Err(PeerClientError::InvalidLocalBase(raw.to_owned()));
    }
    let host = url
        .host_str()
        .ok_or_else(|| PeerClientError::InvalidLocalBase(raw.to_owned()))?;
    Ok(format!("http://{host}:{port}"))
}

fn loopback_host(url: &Url) -> bool {
    match url.host() {
        Some(Host::Ipv4(ip)) => ip == Ipv4Addr::LOCALHOST,
        Some(Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        _ => false,
    }
}

/// Encodes one chunk as `big-endian u64 request_id || chunk bytes`.
#[must_use]
fn encode_frame(request_id: u64, chunk: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(8 + chunk.len());
    frame.extend_from_slice(&request_id.to_be_bytes());
    frame.extend_from_slice(chunk);
    frame
}

/// Splits a binary frame into its request id and chunk payload.
///
/// # Errors
///
/// Returns [`PeerClientError::Transport`] when fewer than eight bytes are
/// present, i.e. the frame has no complete request id.
fn decode_frame(payload: &[u8]) -> Result<(u64, &[u8]), PeerClientError> {
    if payload.len() < 8 {
        return Err(PeerClientError::Transport(format!(
            "binary frame is too short: {} bytes",
            payload.len()
        )));
    }
    let id_bytes: [u8; 8] = payload[..8]
        .try_into()
        .expect("the first eight bytes always form a u64");
    Ok((u64::from_be_bytes(id_bytes), &payload[8..]))
}

fn valid_route(route: &str) -> bool {
    matches!(route, "v" | "f" | "d" | "e")
}

fn valid_filename(id: &str, filename: &str) -> bool {
    if !juiceutils::ids::is_valid_id_component(id) || id.len() > MAX_ID_LENGTH {
        return false;
    }
    let Some(extension) = filename
        .strip_prefix(id)
        .and_then(|rest| rest.strip_prefix('.'))
    else {
        return false;
    };
    !extension.is_empty()
        && extension.len() <= MAX_EXT_LENGTH
        && extension
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Builds and re-validates the loopback URL for a relay file request.
fn fetch_url(local_base: &str, route: &str, id: &str, filename: &str) -> Result<Url, String> {
    if !valid_route(route) {
        return Err(format!("unsupported public route: {route}"));
    }
    if !valid_filename(id, filename) {
        return Err(format!("unsafe file path: {route}/{filename}"));
    }
    let raw = format!("{local_base}/{route}/{filename}");
    let url = Url::parse(&raw).map_err(|_| "invalid local fetch URL".to_owned())?;
    if url.scheme() != "http" || !loopback_host(&url) || url.port().is_none() {
        return Err("local fetch target escaped the loopback boundary".to_owned());
    }
    Ok(url)
}

fn is_forwarded_request_header(name: &str) -> bool {
    FORWARDED_REQUEST_HEADERS
        .iter()
        .any(|allowed| name.eq_ignore_ascii_case(allowed))
}

fn forwarded_request_headers(headers: &[(String, String)]) -> Vec<(HeaderName, HeaderValue)> {
    headers
        .iter()
        .filter(|(name, _)| is_forwarded_request_header(name))
        .filter_map(|(name, value)| {
            let name = HeaderName::from_bytes(name.as_bytes()).ok()?;
            let value = HeaderValue::from_str(value).ok()?;
            Some((name, value))
        })
        .collect()
}

fn forwarded_response_headers(response: &reqwest::Response) -> Vec<(String, String)> {
    response
        .headers()
        .iter()
        .filter(|(name, _)| RESPONSE_HEADERS.contains(&name.as_str()))
        .filter_map(|(name, value)| {
            Some((name.as_str().to_owned(), value.to_str().ok()?.to_owned()))
        })
        .collect()
}

/// Internal commands accepted by the running client.
#[derive(Debug)]
enum Control {
    Revoke(Vec<String>),
    Shutdown,
}

/// Cloneable handle for controlling a client after [`PeerClient::run`] starts.
#[derive(Clone, Debug)]
pub struct PeerControl {
    tx: mpsc::Sender<Control>,
}

impl PeerControl {
    /// Tells the relay that the given file ids are no longer served.
    ///
    /// # Errors
    ///
    /// Returns [`PeerClientError::Transport`] when the client is no longer
    /// running.
    pub async fn revoke(&self, ids: Vec<String>) -> Result<(), PeerClientError> {
        self.tx
            .send(Control::Revoke(ids))
            .await
            .map_err(|_| PeerClientError::Transport("peer client is not running".to_owned()))
    }

    /// Requests a clean shutdown of the run loop.
    pub async fn shutdown(&self) {
        let _ = self.tx.send(Control::Shutdown).await;
    }
}

/// A connected, handshaked juicepeer client. Call [`PeerClient::run`] to own
/// the WebSocket connection and serve requests until it closes.
pub struct PeerClient {
    sink: SocketSink,
    stream: SocketStream,
    http: reqwest::Client,
    local_base: String,
    accepted: HashSet<String>,
    control_tx: mpsc::Sender<Control>,
    control_rx: mpsc::Receiver<Control>,
}

impl PeerClient {
    /// Returns the file ids the relay accepted in `AnnounceOk`.
    #[must_use]
    pub const fn accepted_files(&self) -> &HashSet<String> {
        &self.accepted
    }

    /// Returns a cloneable handle for revoking files or shutting down.
    #[must_use]
    pub fn control(&self) -> PeerControl {
        PeerControl {
            tx: self.control_tx.clone(),
        }
    }

    /// Drives the connection until the relay closes it, shutdown is requested,
    /// or the transport fails. In-flight streams are aborted on exit.
    ///
    /// # Errors
    ///
    /// Returns [`PeerClientError::Transport`] when the WebSocket fails while
    /// the loop is running.
    pub async fn run(self) -> Result<(), PeerClientError> {
        let Self {
            sink,
            mut stream,
            http,
            local_base,
            accepted: _,
            control_tx,
            mut control_rx,
        } = self;
        // Keep one sender alive for the whole loop. If every sender were
        // dropped, `recv()` would return `None` immediately and the loop would
        // exit before serving any relay request.
        let _control_guard = control_tx;
        let sink = Arc::new(Mutex::new(sink));
        let outcome = run_loop(&mut stream, &sink, &mut control_rx, &http, &local_base).await;
        shutdown_socket(&sink).await;
        outcome
    }
}

/// Connects to the relay, authenticates and announces the local files.
///
/// The device JWT is sent both as an `Authorization: Bearer` header and inside
/// the `Hello` frame, as required by the wire contract. The local juicehost
/// base is validated before any network activity occurs.
///
/// # Errors
///
/// Returns an error when the local base is not loopback-only, the relay URL or
/// token is invalid, the connection fails, or the Hello/Announce handshake
/// does not complete.
pub async fn connect(
    relay_ws_url: impl Into<String>,
    token: String,
    local_base: String,
    files: Vec<PeerFileSpec>,
) -> Result<PeerClient, PeerClientError> {
    let local_base = normalized_local_base(&local_base)?;
    let mut request = relay_ws_url
        .into()
        .into_client_request()
        .map_err(|e| PeerClientError::InvalidConfiguration(format!("bad relay URL: {e}")))?;
    let auth = WsHeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
        PeerClientError::InvalidConfiguration("device token is not a valid header value".to_owned())
    })?;
    request.headers_mut().insert(
        tokio_tungstenite::tungstenite::http::header::AUTHORIZATION,
        auth,
    );

    let (socket, _response) = connect_async(request)
        .await
        .map_err(|e| PeerClientError::Connection(e.to_string()))?;
    let (mut sink, mut stream) = socket.split();
    let accepted = handshake(&mut sink, &mut stream, &token, files).await?;
    // Redirects and proxies are disabled so every fetch stays on the validated
    // loopback target, even if the local host replies with a Location header.
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .map_err(|e| PeerClientError::InvalidConfiguration(format!("HTTP client: {e}")))?;
    let (control_tx, control_rx) = mpsc::channel(CONTROL_QUEUE);
    Ok(PeerClient {
        sink,
        stream,
        http,
        local_base,
        accepted,
        control_tx,
        control_rx,
    })
}

/// Keeps a peer online across relay restarts and transient network failures.
///
/// Each attempt re-runs [`connect`], so the Hello/Announce handshake (and with
/// it the full `files` list) is repeated on every reconnect. Transient failures
/// are retried with exponential backoff capped at 30 seconds; the delay resets
/// after a successful connection. Configuration errors are fatal.
///
/// `shutdown` is the caller's cancellation signal: pass a
/// [`tokio::sync::watch::Receiver<bool>`] that yields `true` (or whose senders
/// are all dropped) to stop the loop. Callers with no other shutdown token can
/// create one with `tokio::sync::watch::channel(false)`. Note that a
/// [`PeerControl`] belongs to one connection only and is not carried over.
///
/// # Errors
///
/// Returns immediately when the local base, relay URL or token is invalid,
/// since retrying cannot fix those. Otherwise returns `Ok(())` on shutdown.
pub async fn run_with_reconnect(
    relay_ws_url: impl Into<String>,
    token: String,
    local_base: String,
    files: Vec<PeerFileSpec>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), PeerClientError> {
    let relay_ws_url = relay_ws_url.into();
    let mut delay = RECONNECT_BASE_DELAY;
    loop {
        if *shutdown.borrow() {
            return Ok(());
        }
        match connect(
            relay_ws_url.clone(),
            token.clone(),
            local_base.clone(),
            files.clone(),
        )
        .await
        {
            Ok(client) => {
                delay = RECONNECT_BASE_DELAY;
                if let Err(error) = client.run().await {
                    tracing::warn!(%error, "peer connection lost; reconnecting");
                }
            }
            Err(
                error @ (PeerClientError::InvalidLocalBase(_)
                | PeerClientError::InvalidConfiguration(_)),
            ) => return Err(error),
            Err(error) => tracing::warn!(%error, "peer connect failed; retrying"),
        }
        tokio::select! {
            () = tokio::time::sleep(delay) => {}
            result = shutdown.changed() => {
                // A closed channel (all senders dropped) also means shutdown.
                if result.is_err() || *shutdown.borrow() {
                    return Ok(());
                }
            }
        }
        delay = delay.saturating_mul(2).min(RECONNECT_MAX_DELAY);
    }
}

async fn send_json(
    sink: &mut SocketSink,
    message: PeerClientMessage,
) -> Result<(), PeerClientError> {
    let payload =
        serde_json::to_string(&message).map_err(|e| PeerClientError::Transport(e.to_string()))?;
    sink.send(Message::Text(payload.into()))
        .await
        .map_err(|e| PeerClientError::Transport(e.to_string()))
}

async fn next_server_text(stream: &mut SocketStream) -> Result<PeerServerMessage, PeerClientError> {
    loop {
        let message = tokio::time::timeout(HANDSHAKE_TIMEOUT, stream.next())
            .await
            .map_err(|_| PeerClientError::Handshake("timed out waiting for relay".to_owned()))?
            .ok_or_else(|| PeerClientError::Handshake("relay closed during handshake".to_owned()))?
            .map_err(|e| PeerClientError::Connection(e.to_string()))?;
        match message {
            Message::Text(text) => {
                return serde_json::from_str(&text)
                    .map_err(|e| PeerClientError::Handshake(format!("invalid relay JSON: {e}")));
            }
            // tungstenite queues the pong and flushes it on the next write.
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) | Message::Binary(_) => {}
            Message::Close(_) => {
                return Err(PeerClientError::Handshake(
                    "relay closed during handshake".to_owned(),
                ));
            }
        }
    }
}

async fn handshake(
    sink: &mut SocketSink,
    stream: &mut SocketStream,
    token: &str,
    files: Vec<PeerFileSpec>,
) -> Result<HashSet<String>, PeerClientError> {
    send_json(
        sink,
        PeerClientMessage::Hello {
            version: PEER_PROTOCOL_VERSION,
            token: token.to_owned(),
        },
    )
    .await?;
    let session_id = match next_server_text(stream).await? {
        PeerServerMessage::HelloOk { session_id } => session_id,
        PeerServerMessage::Error { code, message } => {
            return Err(PeerClientError::Handshake(format!(
                "relay rejected hello ({code}): {message}"
            )));
        }
        other => {
            return Err(PeerClientError::Handshake(format!(
                "expected hello_ok, got {other:?}"
            )));
        }
    };
    tracing::info!(session_id = %session_id, "peer session established");

    send_json(sink, PeerClientMessage::Announce { files }).await?;
    match next_server_text(stream).await? {
        PeerServerMessage::AnnounceOk { accepted, rejected } => {
            for rejection in &rejected {
                tracing::warn!(
                    id = %rejection.id,
                    reason = %rejection.reason,
                    "relay rejected announced file"
                );
            }
            Ok(accepted.into_iter().collect())
        }
        PeerServerMessage::Error { code, message } => Err(PeerClientError::Handshake(format!(
            "relay rejected announce ({code}): {message}"
        ))),
        other => Err(PeerClientError::Handshake(format!(
            "expected announce_ok, got {other:?}"
        ))),
    }
}

/// Aborts all active streams on drop, including when `?` returns early.
#[derive(Default)]
struct ActiveRequests(HashMap<u64, JoinHandle<()>>);

impl Drop for ActiveRequests {
    fn drop(&mut self) {
        for handle in self.0.values() {
            handle.abort();
        }
    }
}

async fn run_loop(
    stream: &mut SocketStream,
    sink: &Arc<Mutex<SocketSink>>,
    control_rx: &mut mpsc::Receiver<Control>,
    http: &reqwest::Client,
    local_base: &str,
) -> Result<(), PeerClientError> {
    let mut active = ActiveRequests::default();
    loop {
        tokio::select! {
            incoming = stream.next() => {
                let Some(incoming) = incoming else { return Ok(()); };
                let message = incoming.map_err(|e| PeerClientError::Transport(e.to_string()))?;
                if !handle_socket_message(message, sink, http, local_base, &mut active).await? {
                    return Ok(());
                }
            }
            control = control_rx.recv() => {
                match control {
                    Some(Control::Shutdown) | None => return Ok(()),
                    Some(Control::Revoke(ids)) => {
                        send_json(&mut *sink.lock().await, PeerClientMessage::Revoke { ids }).await?;
                    }
                }
            }
        }
        active.0.retain(|_, handle| !handle.is_finished());
    }
}

async fn shutdown_socket(sink: &Arc<Mutex<SocketSink>>) {
    let mut guard = sink.lock().await;
    let _ = guard.send(Message::Close(None)).await;
    let _ = guard.close().await;
    drop(guard);
}

async fn handle_socket_message(
    message: Message,
    sink: &Arc<Mutex<SocketSink>>,
    http: &reqwest::Client,
    local_base: &str,
    active: &mut ActiveRequests,
) -> Result<bool, PeerClientError> {
    match message {
        Message::Text(text) => {
            let Ok(message) = serde_json::from_str::<PeerServerMessage>(&text) else {
                tracing::debug!("relay sent malformed JSON");
                return Ok(true);
            };
            handle_server_message(message, sink, http, local_base, active).await?;
            Ok(true)
        }
        Message::Binary(payload) => {
            match decode_frame(&payload) {
                Ok((request_id, chunk)) => tracing::debug!(
                    request_id,
                    bytes = chunk.len(),
                    "ignoring unexpected binary frame from relay"
                ),
                Err(error) => tracing::debug!("{error}"),
            }
            Ok(true)
        }
        Message::Ping(payload) => {
            sink.lock()
                .await
                .send(Message::Pong(payload))
                .await
                .map_err(|e| PeerClientError::Transport(e.to_string()))?;
            Ok(true)
        }
        Message::Pong(_) | Message::Frame(_) => Ok(true),
        Message::Close(_) => Ok(false),
    }
}

async fn handle_server_message(
    message: PeerServerMessage,
    sink: &Arc<Mutex<SocketSink>>,
    http: &reqwest::Client,
    local_base: &str,
    active: &mut ActiveRequests,
) -> Result<(), PeerClientError> {
    match message {
        PeerServerMessage::Request {
            request_id,
            route,
            id,
            filename,
            headers,
        } => {
            let job = FetchJob {
                request_id,
                route,
                id,
                filename,
                headers,
            };
            let task = tokio::spawn(stream_request(
                job,
                Arc::clone(sink),
                http.clone(),
                local_base.to_owned(),
            ));
            if let Some(previous) = active.0.insert(request_id, task) {
                previous.abort();
            }
        }
        PeerServerMessage::EndChunks { request_id }
        | PeerServerMessage::Abort { request_id, .. } => {
            if let Some(handle) = active.0.remove(&request_id) {
                handle.abort();
            }
        }
        PeerServerMessage::Ping => {
            send_json(&mut *sink.lock().await, PeerClientMessage::Pong).await?;
        }
        PeerServerMessage::Error { code, message } => {
            tracing::warn!(%code, %message, "relay reported an error");
        }
        PeerServerMessage::HelloOk { .. } | PeerServerMessage::AnnounceOk { .. } => {
            tracing::debug!("ignoring unexpected handshake reply");
        }
    }
    Ok(())
}

/// One relay request, bundled to keep the spawned task signature small.
struct FetchJob {
    request_id: u64,
    route: String,
    id: String,
    filename: String,
    headers: Vec<(String, String)>,
}

async fn stream_request(
    job: FetchJob,
    sink: Arc<Mutex<SocketSink>>,
    http: reqwest::Client,
    local_base: String,
) {
    let outcome = fetch_and_stream(&job, &sink, &http, &local_base).await;
    if let Err(reason) = outcome {
        let abort = PeerClientMessage::Abort {
            request_id: job.request_id,
            reason,
        };
        let mut guard = sink.lock().await;
        if let Err(error) = send_json(&mut guard, abort).await {
            tracing::debug!(request_id = job.request_id, %error, "could not report aborted request");
        }
    }
}

async fn fetch_and_stream(
    job: &FetchJob,
    sink: &Arc<Mutex<SocketSink>>,
    http: &reqwest::Client,
    local_base: &str,
) -> Result<(), String> {
    let url = fetch_url(local_base, &job.route, &job.id, &job.filename)?;
    let mut request = http.get(url);
    for (name, value) in forwarded_request_headers(&job.headers) {
        request = request.header(name, value);
    }
    let response = request
        .send()
        .await
        .map_err(|e| format!("local juicehost request failed: {e}"))?;
    let status = response.status().as_u16();
    let response_headers = forwarded_response_headers(&response);
    send_json(
        &mut *sink.lock().await,
        PeerClientMessage::ResponseHead {
            request_id: job.request_id,
            status,
            headers: response_headers,
        },
    )
    .await
    .map_err(|e| format!("could not send response head: {e}"))?;

    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|e| format!("local juicehost body read failed: {e}"))?;
        for piece in chunk.chunks(CHUNK_BYTES) {
            let frame = encode_frame(job.request_id, piece);
            sink.lock()
                .await
                .send(Message::Binary(frame.into()))
                .await
                .map_err(|e| format!("could not send chunk: {e}"))?;
        }
    }

    send_json(
        &mut *sink.lock().await,
        PeerClientMessage::ResponseEnd {
            request_id: job.request_id,
        },
    )
    .await
    .map_err(|e| format!("could not send response end: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_loopback_http_bases() {
        for raw in [
            "http://127.0.0.1:6402",
            "http://localhost:6402",
            "http://127.0.0.1:6402/",
            "http://LOCALHOST:8080",
        ] {
            let base = normalized_local_base(raw).unwrap();
            assert!(base.starts_with("http://"), "{raw}");
            assert!(!base.ends_with('/'), "{raw}");
        }
    }

    #[test]
    fn empty_base_uses_default() {
        assert_eq!(normalized_local_base("").unwrap(), DEFAULT_LOCAL_BASE);
        assert_eq!(normalized_local_base("   ").unwrap(), DEFAULT_LOCAL_BASE);
    }

    #[test]
    fn rejects_non_loopback_and_non_http_bases() {
        let invalid = [
            "https://127.0.0.1:6402",
            "ws://127.0.0.1:6402",
            "wss://127.0.0.1:6402",
            "http://example.com:6402",
            "http://127.0.0.2:6402",
            "http://0.0.0.0:6402",
            "http://[::1]:6402",
            "http://127.0.0.1",
            "http://127.0.0.1:0",
            "http://user@127.0.0.1:6402",
            "http://127.0.0.1:6402/path",
            "http://127.0.0.1:6402?q=1",
            "http://127.0.0.1:6402#f",
            "not a url",
        ];
        for raw in invalid {
            assert!(normalized_local_base(raw).is_err(), "accepted {raw}");
        }
    }

    #[test]
    fn frame_round_trips_request_id_and_payload() {
        let id = 0x0102_0304_0506_0708u64;
        let frame = encode_frame(id, b"hello");
        assert_eq!(&frame[..8], &id.to_be_bytes());
        let (decoded_id, payload) = decode_frame(&frame).unwrap();
        assert_eq!(decoded_id, id);
        assert_eq!(payload, b"hello");
    }

    #[test]
    fn frame_handles_empty_payload_and_max_id() {
        let frame = encode_frame(u64::MAX, b"");
        assert_eq!(frame.len(), 8);
        let (id, payload) = decode_frame(&frame).unwrap();
        assert_eq!(id, u64::MAX);
        assert_eq!(payload, b"");
    }

    #[test]
    fn decode_rejects_short_frames() {
        for len in 0..8 {
            let bytes = vec![0u8; len];
            assert!(decode_frame(&bytes).is_err(), "accepted {len} bytes");
        }
    }

    #[test]
    fn fetch_url_uses_route_and_filename() {
        let url = fetch_url("http://127.0.0.1:6402", "v", "abc-123", "abc-123.mp4").unwrap();
        assert_eq!(url.as_str(), "http://127.0.0.1:6402/v/abc-123.mp4");
    }

    #[test]
    fn fetch_url_rejects_untrusted_components() {
        let base = "http://127.0.0.1:6402";
        assert!(fetch_url(base, "etc", "abc", "abc.mp4").is_err());
        assert!(fetch_url(base, "v", "../secret", "../secret.mp4").is_err());
        assert!(fetch_url(base, "v", "abc", "other.mp4").is_err());
        assert!(fetch_url(base, "v", "abc", "abc.bad/ext").is_err());
        assert!(fetch_url("http://example.com:6402", "v", "abc", "abc.mp4").is_err());
    }

    #[test]
    fn only_request_allowlist_headers_are_forwarded() {
        let headers = vec![
            ("Range".into(), "bytes=0-7".into()),
            ("if-none-match".into(), "\"etag\"".into()),
            ("accept".into(), "video/mp4".into()),
            ("user-agent".into(), "browser".into()),
            ("host".into(), "evil.example".into()),
            ("bad name\n".into(), "x".into()),
        ];
        let forwarded = forwarded_request_headers(&headers);
        assert_eq!(forwarded.len(), 4);
        assert_eq!(forwarded[0].0.as_str(), "range");
    }
}
