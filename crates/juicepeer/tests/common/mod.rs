#![allow(dead_code)]

//! Shared harness for the juicepeer integration tests. Everything binds to a
//! loopback ephemeral port so tests are independent and never touch a real
//! relay, juiceback, or local juicehost.

use std::{
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use futures::{SinkExt, StreamExt};
use jsonwebtoken::{EncodingKey, Header, encode};
use juiceproto::{
    headers::HOST_API_KEY,
    peer::{ISS_DEVICE, PeerDeviceClaims},
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async, tungstenite::protocol::Message as WsMessage,
};

use juicepeer::relay::{RelayState, config::Config};

pub type PeerSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// A server bound to an ephemeral loopback port.
pub struct SpawnedServer {
    pub base_url: String,
    task: JoinHandle<()>,
}

impl SpawnedServer {
    pub fn shutdown(&self) {
        self.task.abort();
    }
}

/// Mints a device JWT the relay will accept for the given secret.
#[must_use]
pub fn mint_device_token(secret: &str, sub: &str, user_id: &str) -> String {
    let now = usize::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    )
    .unwrap();
    let claims = PeerDeviceClaims {
        sub: sub.to_owned(),
        user_id: user_id.to_owned(),
        iss: ISS_DEVICE.to_owned(),
        iat: now,
        exp: now + 3600,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .unwrap()
}

#[must_use]
pub async fn spawn_router(router: Router) -> SpawnedServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    SpawnedServer {
        base_url: format!("http://{addr}"),
        task,
    }
}

struct BackendCtx {
    response: Value,
    calls: Arc<Mutex<Vec<Value>>>,
}

/// A stand-in for juiceback's `/internal/peer/authorize`.
pub struct MockAuthorizeBackend {
    pub server: SpawnedServer,
    pub calls: Arc<Mutex<Vec<Value>>>,
}

async fn authorize_handler(
    State(ctx): State<Arc<BackendCtx>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    ctx.calls.lock().unwrap().push(json!({
        "api_key": headers
            .get(HOST_API_KEY)
            .and_then(|value| value.to_str().ok())
            .unwrap_or(""),
        "body": body,
    }));
    (StatusCode::OK, Json(ctx.response.clone())).into_response()
}

/// The `accepted`/`rejected` outcome the mock backend returns to the relay.
#[must_use]
pub fn authorize_response(accepted: &[&str], rejected: &[(&str, &str)]) -> Value {
    json!({
        "accepted": accepted,
        "rejected": rejected
            .iter()
            .map(|(id, reason)| json!({ "id": id, "reason": reason }))
            .collect::<Vec<_>>(),
    })
}

#[must_use]
pub async fn spawn_authorize_backend(response: Value) -> MockAuthorizeBackend {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let ctx = Arc::new(BackendCtx {
        response,
        calls: Arc::clone(&calls),
    });
    let router = Router::new()
        .route("/internal/peer/authorize", post(authorize_handler))
        .with_state(ctx);
    MockAuthorizeBackend {
        server: spawn_router(router).await,
        calls,
    }
}

/// Relay configuration with long-lived heartbeat and a short request timeout
/// so tests never depend on timers firing.
#[must_use]
pub fn test_relay_config(juiceback_url: &str, jwt_secret: &str) -> Config {
    Config {
        host: "127.0.0.1".into(),
        port: 0,
        relay_base_url: "http://127.0.0.1".into(),
        juiceback_url: juiceback_url.to_owned(),
        jwt_secret: jwt_secret.to_owned(),
        api_key: "test-api-key".into(),
        max_sessions: 16,
        max_frame_bytes: 1024 * 1024,
        max_concurrent_requests: 8,
        request_timeout_secs: 5,
        heartbeat_interval_secs: 3600,
    }
}

pub struct RunningRelay {
    pub http_base: String,
    pub ws_url: String,
    pub state: Arc<RelayState>,
    server: SpawnedServer,
}

impl RunningRelay {
    pub fn shutdown(&self) {
        self.server.shutdown();
    }
}

#[must_use]
pub async fn spawn_relay(config: Config) -> RunningRelay {
    let state = Arc::new(RelayState::new(config).unwrap());
    let server = spawn_router(juicepeer::relay::routes::build(Arc::clone(&state))).await;
    let ws_url = format!(
        "{}/peer/ws",
        server.base_url.replacen("http://", "ws://", 1)
    );
    RunningRelay {
        http_base: server.base_url.clone(),
        ws_url,
        state,
        server,
    }
}

/// Opens a raw WebSocket client connection to the relay.
#[must_use]
pub async fn connect_ws(ws_url: &str) -> PeerSocket {
    let (socket, _response) = connect_async(ws_url).await.unwrap();
    socket
}

/// Sends one JSON text frame.
pub async fn send_json<S: Serialize + Sync>(socket: &mut PeerSocket, message: &S) {
    let payload = serde_json::to_string(message).unwrap();
    socket.send(WsMessage::Text(payload.into())).await.unwrap();
}

/// Reads the next JSON text frame, skipping control/binary frames.
pub async fn read_json<M: DeserializeOwned>(socket: &mut PeerSocket) -> M {
    loop {
        match socket.next().await.unwrap().unwrap() {
            WsMessage::Text(text) => return serde_json::from_str(&text).unwrap(),
            WsMessage::Ping(_) | WsMessage::Pong(_) | WsMessage::Binary(_) => {}
            other => panic!("expected a JSON text frame, got {other:?}"),
        }
    }
}

/// Sends a binary chunk frame: big-endian request id followed by bytes.
pub async fn send_chunk(socket: &mut PeerSocket, request_id: u64, chunk: &[u8]) {
    let mut frame = request_id.to_be_bytes().to_vec();
    frame.extend_from_slice(chunk);
    socket.send(WsMessage::Binary(frame.into())).await.unwrap();
}

/// Completes the standard Hello/Announce handshake as a mock peer.
#[must_use]
pub async fn handshake(
    socket: &mut PeerSocket,
    token: &str,
    files: Vec<juiceproto::peer::PeerFileSpec>,
) -> juiceproto::peer::PeerServerMessage {
    send_json(
        socket,
        &juiceproto::peer::PeerClientMessage::Hello {
            version: juiceproto::peer::PEER_PROTOCOL_VERSION,
            token: token.to_owned(),
        },
    )
    .await;
    let hello_ok = read_json::<juiceproto::peer::PeerServerMessage>(socket).await;
    assert!(
        matches!(
            hello_ok,
            juiceproto::peer::PeerServerMessage::HelloOk { .. }
        ),
        "expected hello_ok, got {hello_ok:?}"
    );
    send_json(
        socket,
        &juiceproto::peer::PeerClientMessage::Announce { files },
    )
    .await;
    read_json::<juiceproto::peer::PeerServerMessage>(socket).await
}
