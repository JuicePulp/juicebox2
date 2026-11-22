//! Client tests through its public seams: loopback validation plus a full
//! Hello/Announce/Request/Response loop against a mock relay and a mock local
//! juicehost bound to loopback.

mod common;

use std::time::Duration;

use axum::{
    Router,
    extract::{
        State,
        ws::{Message as AxumMessage, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use futures::{
    SinkExt, StreamExt,
    stream::{SplitSink, SplitStream},
};
use juicepeer::peer_client::{DEFAULT_LOCAL_BASE, PeerClientError, connect, normalized_local_base};
use juiceproto::peer::{PeerClientMessage, PeerFileSpec, PeerServerMessage};
use serde::{Serialize, de::DeserializeOwned};
use tokio::sync::{mpsc, watch};

use crate::common::{mint_device_token, spawn_router};

type AxumSink = SplitSink<WebSocket, AxumMessage>;
type AxumStream = SplitStream<WebSocket>;

async fn axum_send<M: Serialize + Sync>(sink: &mut AxumSink, message: &M) {
    let payload = serde_json::to_string(message).unwrap();
    sink.send(AxumMessage::Text(payload.into())).await.unwrap();
}

async fn axum_read<M: DeserializeOwned>(stream: &mut AxumStream) -> M {
    loop {
        match stream.next().await.unwrap().unwrap() {
            AxumMessage::Text(text) => return serde_json::from_str(&text).unwrap(),
            AxumMessage::Ping(_) | AxumMessage::Pong(_) => {}
            other => panic!("expected a JSON text frame, got {other:?}"),
        }
    }
}

async fn complete_handshake(sink: &mut AxumSink, stream: &mut AxumStream, accepted: &[&str]) {
    let hello = axum_read::<PeerClientMessage>(stream).await;
    assert!(
        matches!(hello, PeerClientMessage::Hello { version: 1, .. }),
        "expected Hello v1, got {hello:?}"
    );
    axum_send(
        sink,
        &PeerServerMessage::HelloOk {
            session_id: "s1".into(),
        },
    )
    .await;
    let announce = axum_read::<PeerClientMessage>(stream).await;
    assert!(
        matches!(announce, PeerClientMessage::Announce { .. }),
        "expected Announce, got {announce:?}"
    );
    axum_send(
        sink,
        &PeerServerMessage::AnnounceOk {
            accepted: accepted.iter().map(|id| (*id).to_owned()).collect(),
            rejected: Vec::new(),
        },
    )
    .await;
}

#[test]
fn normalized_local_base_only_accepts_loopback_http_with_port() {
    for valid in [
        "http://127.0.0.1:6402",
        "http://localhost:6402",
        "http://127.0.0.1:6402/",
        "http://LOCALHOST:8080",
    ] {
        assert!(
            normalized_local_base(valid).is_ok(),
            "rejected valid loopback base {valid}"
        );
    }
    assert_eq!(normalized_local_base("").unwrap(), DEFAULT_LOCAL_BASE);
    assert_eq!(normalized_local_base("   ").unwrap(), DEFAULT_LOCAL_BASE);

    for invalid in [
        "https://127.0.0.1:6402",
        "ws://127.0.0.1:6402",
        "http://example.com:6402",
        "http://127.0.0.2:6402",
        "http://0.0.0.0:6402",
        "http://[::1]:6402",
        "http://127.0.0.1",
        "http://127.0.0.1:0",
        "http://user@127.0.0.1:6402",
        "http://127.0.0.1:6402/path",
        "http://127.0.0.1:6402?q=1",
        "not a url",
    ] {
        assert!(
            matches!(
                normalized_local_base(invalid),
                Err(PeerClientError::InvalidLocalBase(_))
            ),
            "accepted unsafe local base {invalid}"
        );
    }
}

#[tokio::test]
async fn connect_rejects_non_loopback_base_before_any_network_call() {
    let result = connect(
        "ws://127.0.0.1:1/peer/ws",
        "token".into(),
        "http://example.com:6402".into(),
        Vec::new(),
    )
    .await;
    assert!(matches!(result, Err(PeerClientError::InvalidLocalBase(_))));
}

/// Mock local juicehost: only the public route and safe filename are served.
async fn local_file(headers: HeaderMap) -> Response {
    assert_eq!(headers["range"], "bytes=0-4");
    assert!(!headers.contains_key("x-evil"));
    (
        StatusCode::PARTIAL_CONTENT,
        [
            ("content-type", "text/plain"),
            ("etag", "\"abc\""),
            ("content-range", "bytes 0-4/11"),
        ],
        "hello world",
    )
        .into_response()
}

/// Mock relay: handshakes, issues one request, then verifies the framing.
async fn relay_ws_upgrade(
    ws: WebSocketUpgrade,
    State((result_tx, mut close_rx)): State<(mpsc::Sender<Vec<u8>>, watch::Receiver<bool>)>,
) -> Response {
    ws.on_upgrade(move |socket| async move {
        let (mut sink, mut stream) = socket.split();
        complete_handshake(&mut sink, &mut stream, &["file-1"]).await;
        axum_send(
            &mut sink,
            &PeerServerMessage::Request {
                request_id: 77,
                route: "v".into(),
                id: "file-1".into(),
                filename: "file-1.txt".into(),
                headers: vec![
                    ("range".into(), "bytes=0-4".into()),
                    ("x-evil".into(), "drop".into()),
                ],
            },
        )
        .await;

        let mut body = Vec::new();
        loop {
            match stream.next().await.unwrap().unwrap() {
                AxumMessage::Text(text) => {
                    let message: PeerClientMessage = serde_json::from_str(&text).unwrap();
                    match message {
                        PeerClientMessage::ResponseHead {
                            request_id,
                            status,
                            headers,
                        } => {
                            assert_eq!(request_id, 77);
                            assert_eq!(status, 206);
                            assert!(headers.iter().any(|(k, v)| k == "etag" && v == "\"abc\""));
                        }
                        PeerClientMessage::ResponseEnd { request_id } => {
                            assert_eq!(request_id, 77);
                            break;
                        }
                        other => panic!("unexpected client message: {other:?}"),
                    }
                }
                AxumMessage::Binary(payload) => {
                    assert!(payload.len() >= 8, "binary frame is missing its request id");
                    let id = u64::from_be_bytes(payload[..8].try_into().unwrap());
                    assert_eq!(id, 77, "wrong big-endian request id");
                    body.extend_from_slice(&payload[8..]);
                }
                AxumMessage::Ping(_) | AxumMessage::Pong(_) => {}
                AxumMessage::Close(_) => panic!("client closed before finishing the stream"),
            }
        }
        result_tx.send(body).await.unwrap();
        // Hold the socket open until the test has shut the client down.
        let _ = close_rx.changed().await;
    })
}

#[tokio::test]
async fn client_streams_loopback_file_with_big_endian_framed_chunks() {
    let juicehost = spawn_router(Router::new().route("/v/{file}", get(local_file))).await;
    let (result_tx, mut result_rx) = mpsc::channel(1);
    let (close_tx, close_rx) = watch::channel(false);
    let relay = spawn_router(
        Router::new()
            .route("/peer/ws", get(relay_ws_upgrade))
            .with_state((result_tx, close_rx)),
    )
    .await;

    let files = vec![PeerFileSpec {
        id: "file-1".into(),
        filename: "file-1.txt".into(),
        mime_type: "text/plain".into(),
        size_bytes: 11,
        expires_at: 9_999_999_999,
    }];
    let ws_url = format!("{}/peer/ws", relay.base_url.replacen("http://", "ws://", 1));
    let client = connect(
        &ws_url,
        mint_device_token("shared-secret", "device-1", "user-1"),
        juicehost.base_url.clone(),
        files,
    )
    .await
    .unwrap();
    assert!(client.accepted_files().contains("file-1"));
    let control = client.control();
    let runner = tokio::spawn(client.run());

    let body = tokio::time::timeout(Duration::from_secs(5), result_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(body, b"hello world");

    control.shutdown().await;
    tokio::time::timeout(Duration::from_secs(5), runner)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    close_tx.send(true).unwrap();
    relay.shutdown();
    juicehost.shutdown();
}

#[tokio::test]
async fn connect_reports_handshake_error_frame() {
    async fn bad_relay(ws: WebSocketUpgrade) -> Response {
        ws.on_upgrade(|mut socket| async move {
            let _ = socket.next().await; // the client's Hello
            let payload = serde_json::to_string(&PeerServerMessage::Error {
                code: "bad_token".into(),
                message: "invalid JWT".into(),
            })
            .unwrap();
            socket
                .send(AxumMessage::Text(payload.into()))
                .await
                .unwrap();
        })
    }
    let relay = spawn_router(Router::new().route("/peer/ws", get(bad_relay))).await;
    let ws_url = format!("{}/peer/ws", relay.base_url.replacen("http://", "ws://", 1));

    let result = connect(
        &ws_url,
        "token".into(),
        DEFAULT_LOCAL_BASE.into(),
        Vec::new(),
    )
    .await;
    match result {
        Err(PeerClientError::Handshake(message)) => {
            assert!(message.contains("bad_token"), "unexpected error: {message}");
        }
        Err(other) => panic!("expected a Handshake error, got {other:?}"),
        Ok(_) => panic!("expected connect to fail on a bad handshake"),
    }
    relay.shutdown();
}

#[tokio::test]
async fn client_replies_pong_to_server_ping() {
    async fn ping_relay(
        ws: WebSocketUpgrade,
        State((result_tx, mut close_rx)): State<(mpsc::Sender<()>, watch::Receiver<bool>)>,
    ) -> Response {
        ws.on_upgrade(move |socket| async move {
            let (mut sink, mut stream) = socket.split();
            complete_handshake(&mut sink, &mut stream, &[]).await;
            axum_send(&mut sink, &PeerServerMessage::Ping).await;
            let reply = axum_read::<PeerClientMessage>(&mut stream).await;
            assert!(matches!(reply, PeerClientMessage::Pong), "got {reply:?}");
            result_tx.send(()).await.unwrap();
            let _ = close_rx.changed().await;
        })
    }
    let (result_tx, mut result_rx) = mpsc::channel(1);
    let (close_tx, close_rx) = watch::channel(false);
    let relay = spawn_router(
        Router::new()
            .route("/peer/ws", get(ping_relay))
            .with_state((result_tx, close_rx)),
    )
    .await;
    let ws_url = format!("{}/peer/ws", relay.base_url.replacen("http://", "ws://", 1));

    let client = connect(
        &ws_url,
        "token".into(),
        DEFAULT_LOCAL_BASE.into(),
        Vec::new(),
    )
    .await
    .unwrap();
    let control = client.control();
    let runner = tokio::spawn(client.run());

    tokio::time::timeout(Duration::from_secs(5), result_rx.recv())
        .await
        .unwrap()
        .unwrap();
    control.shutdown().await;
    runner.await.unwrap().unwrap();
    close_tx.send(true).unwrap();
    relay.shutdown();
}
