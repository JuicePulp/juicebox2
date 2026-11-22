//! End-to-end relay tests: a real axum relay bound to loopback, a mock
//! juiceback authorize endpoint, a tokio-tungstenite mock peer and a reqwest
//! "browser". No real network or file bytes leave the test process.

mod common;

use std::time::Duration;

use axum::{Router, http::StatusCode, routing::get};
use juicepeer::peer_client::connect;
use juiceproto::peer::{PEER_PROTOCOL_VERSION, PeerClientMessage, PeerFileSpec, PeerServerMessage};

use crate::common::{
    authorize_response, connect_ws, handshake, mint_device_token, read_json, send_chunk, send_json,
    spawn_authorize_backend, spawn_relay, spawn_router, test_relay_config,
};

fn file_spec(id: &str) -> PeerFileSpec {
    PeerFileSpec {
        id: id.into(),
        filename: format!("{id}.mp4"),
        mime_type: "video/mp4".into(),
        size_bytes: 4_096,
        expires_at: 9_999_999_999,
    }
}

#[tokio::test]
async fn bad_jwt_is_rejected_before_a_session_is_created() {
    let backend = spawn_authorize_backend(authorize_response(&[], &[])).await;
    let relay = spawn_relay(test_relay_config(&backend.server.base_url, "shared-secret")).await;

    let mut socket = connect_ws(&relay.ws_url).await;
    send_json(
        &mut socket,
        &PeerClientMessage::Hello {
            version: PEER_PROTOCOL_VERSION,
            token: "not-a-jwt".into(),
        },
    )
    .await;
    let reply = read_json::<PeerServerMessage>(&mut socket).await;
    match reply {
        PeerServerMessage::Error { code, .. } => assert_eq!(code, "invalid_token"),
        other => panic!("expected invalid_token error, got {other:?}"),
    }
    assert_eq!(
        relay.state.registry.stats().0,
        0,
        "no session for a bad JWT"
    );
}

#[tokio::test]
async fn unsupported_protocol_version_is_rejected() {
    let backend = spawn_authorize_backend(authorize_response(&[], &[])).await;
    let relay = spawn_relay(test_relay_config(&backend.server.base_url, "shared-secret")).await;

    let mut socket = connect_ws(&relay.ws_url).await;
    send_json(
        &mut socket,
        &PeerClientMessage::Hello {
            version: PEER_PROTOCOL_VERSION + 1,
            token: mint_device_token("shared-secret", "device-1", "user-1"),
        },
    )
    .await;
    let reply = read_json::<PeerServerMessage>(&mut socket).await;
    match reply {
        PeerServerMessage::Error { code, .. } => assert_eq!(code, "unsupported_version"),
        other => panic!("expected unsupported_version error, got {other:?}"),
    }
}

#[tokio::test]
async fn announce_uses_verified_identity_and_forwards_backend_decision() {
    let backend =
        spawn_authorize_backend(authorize_response(&["file-1"], &[("file-2", "not found")])).await;
    let relay = spawn_relay(test_relay_config(&backend.server.base_url, "shared-secret")).await;

    let mut peer = connect_ws(&relay.ws_url).await;
    let token = mint_device_token("shared-secret", "device-1", "user-1");
    let reply = handshake(
        &mut peer,
        &token,
        vec![file_spec("file-1"), file_spec("file-2")],
    )
    .await;
    match reply {
        PeerServerMessage::AnnounceOk { accepted, rejected } => {
            assert_eq!(accepted, vec!["file-1"]);
            assert_eq!(rejected.len(), 1);
            assert_eq!(rejected[0].id, "file-2");
            assert_eq!(rejected[0].reason, "not found");
        }
        other => panic!("expected announce_ok, got {other:?}"),
    }

    // The relay must derive identity from the JWT, not from the announce, and
    // must authenticate to juiceback with the configured internal API key.
    let calls = backend.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["api_key"], "test-api-key");
    assert_eq!(calls[0]["body"]["device_id"], "device-1");
    assert_eq!(calls[0]["body"]["user_id"], "user-1");
    assert_eq!(
        calls[0]["body"]["ids"],
        serde_json::json!(["file-1", "file-2"])
    );
}

#[tokio::test]
async fn range_request_streams_206_with_allowlisted_headers() {
    let backend = spawn_authorize_backend(authorize_response(&["file-1"], &[])).await;
    let relay = spawn_relay(test_relay_config(&backend.server.base_url, "shared-secret")).await;

    let mut peer = connect_ws(&relay.ws_url).await;
    let token = mint_device_token("shared-secret", "device-1", "user-1");
    let reply = handshake(&mut peer, &token, vec![file_spec("file-1")]).await;
    assert!(matches!(reply, PeerServerMessage::AnnounceOk { .. }));

    let url = format!("{}/v/file-1.mp4", relay.http_base);
    let get = tokio::spawn(async move {
        reqwest::Client::new()
            .get(url)
            .header("range", "bytes=0-3")
            .header("x-secret", "must-not-forward")
            .send()
            .await
            .unwrap()
    });

    let request = read_json::<PeerServerMessage>(&mut peer).await;
    let PeerServerMessage::Request {
        request_id,
        route,
        id,
        filename,
        headers,
    } = request
    else {
        panic!("expected a relay Request frame, got {request:?}");
    };
    assert_eq!(route, "v");
    assert_eq!(id, "file-1");
    assert_eq!(filename, "file-1.mp4");
    assert!(
        headers
            .iter()
            .any(|(name, value)| name.eq_ignore_ascii_case("range") && value == "bytes=0-3")
    );
    assert!(
        !headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("x-secret"))
    );

    send_json(
        &mut peer,
        &PeerClientMessage::ResponseHead {
            request_id,
            status: 206,
            headers: vec![
                ("content-type".into(), "video/mp4".into()),
                ("content-range".into(), "bytes 0-3/8".into()),
                ("etag".into(), "\"v1\"".into()),
                ("cache-control".into(), "public, max-age=999".into()),
                ("set-cookie".into(), "evil=1".into()),
            ],
        },
    )
    .await;
    send_chunk(&mut peer, request_id, b"ABCD").await;
    send_json(&mut peer, &PeerClientMessage::ResponseEnd { request_id }).await;

    let response = tokio::time::timeout(Duration::from_secs(5), get)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.headers()["content-range"], "bytes 0-3/8");
    assert_eq!(response.headers()["etag"], "\"v1\"");
    assert_eq!(response.headers()["content-type"], "video/mp4");
    // The relay always overrides caching and never forwards set-cookie.
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert!(!response.headers().contains_key("set-cookie"));
    assert_eq!(response.text().await.unwrap(), "ABCD");
}

#[tokio::test]
async fn if_none_match_request_streams_304() {
    let backend = spawn_authorize_backend(authorize_response(&["file-1"], &[])).await;
    let relay = spawn_relay(test_relay_config(&backend.server.base_url, "shared-secret")).await;

    let mut peer = connect_ws(&relay.ws_url).await;
    let token = mint_device_token("shared-secret", "device-1", "user-1");
    assert!(matches!(
        handshake(&mut peer, &token, vec![file_spec("file-1")]).await,
        PeerServerMessage::AnnounceOk { .. }
    ));

    let url = format!("{}/v/file-1.mp4", relay.http_base);
    let get = tokio::spawn(async move {
        reqwest::Client::new()
            .get(url)
            .header("if-none-match", "\"v1\"")
            .send()
            .await
            .unwrap()
    });

    let PeerServerMessage::Request {
        request_id,
        headers,
        ..
    } = read_json::<PeerServerMessage>(&mut peer).await
    else {
        panic!("expected a relay Request frame");
    };
    assert!(
        headers
            .iter()
            .any(|(name, value)| name.eq_ignore_ascii_case("if-none-match") && value == "\"v1\"")
    );

    send_json(
        &mut peer,
        &PeerClientMessage::ResponseHead {
            request_id,
            status: 304,
            headers: vec![("etag".into(), "\"v1\"".into())],
        },
    )
    .await;
    send_json(&mut peer, &PeerClientMessage::ResponseEnd { request_id }).await;

    let response = tokio::time::timeout(Duration::from_secs(5), get)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(response.headers()["etag"], "\"v1\"");
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.text().await.unwrap(), "");
}

#[tokio::test]
async fn unannounced_id_returns_404_with_no_store_headers() {
    let backend = spawn_authorize_backend(authorize_response(&[], &[])).await;
    let relay = spawn_relay(test_relay_config(&backend.server.base_url, "shared-secret")).await;

    let response = reqwest::Client::new()
        .get(format!("{}/v/no-such-peer-file.mp4", relay.http_base))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["content-security-policy"], "sandbox");
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["error"], "peer file not found");
}

#[tokio::test]
async fn busy_relay_returns_503_while_a_request_is_in_flight() {
    let backend = spawn_authorize_backend(authorize_response(&["file-1"], &[])).await;
    let mut config = test_relay_config(&backend.server.base_url, "shared-secret");
    config.max_concurrent_requests = 1;
    config.request_timeout_secs = 2;
    let relay = spawn_relay(config).await;

    let mut peer = connect_ws(&relay.ws_url).await;
    let token = mint_device_token("shared-secret", "device-1", "user-1");
    assert!(matches!(
        handshake(&mut peer, &token, vec![file_spec("file-1")]).await,
        PeerServerMessage::AnnounceOk { .. }
    ));

    // The first request deliberately never gets a ResponseHead, keeping the
    // single request slot busy.
    let first_get = {
        let url = format!("{}/v/file-1.mp4", relay.http_base);
        tokio::spawn(async move { reqwest::Client::new().get(url).send().await })
    };
    let request = read_json::<PeerServerMessage>(&mut peer).await;
    let PeerServerMessage::Request { request_id, .. } = request else {
        panic!("expected a relay Request frame");
    };
    // Keep the request alive (never send ResponseEnd for it).
    assert!(request_id > 0);

    let second = reqwest::Client::new()
        .get(format!("{}/v/file-1.mp4", relay.http_base))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(second.headers()["cache-control"], "no-store");
    let body: serde_json::Value = second.json().await.unwrap();
    assert_eq!(body["error"], "relay is busy");

    first_get.abort();
}

#[tokio::test]
async fn client_relay_and_loopback_juicehost_round_trip() {
    async fn local_file() -> &'static str {
        "peer file bytes"
    }

    // Real relay + real client, with only juiceback replaced by a stub.
    let backend = spawn_authorize_backend(authorize_response(&["file-1"], &[])).await;
    let relay = spawn_relay(test_relay_config(&backend.server.base_url, "shared-secret")).await;
    let juicehost = spawn_router(Router::new().route("/v/{file}", get(local_file))).await;

    let client = connect(
        &relay.ws_url,
        mint_device_token("shared-secret", "device-1", "user-1"),
        juicehost.base_url.clone(),
        vec![file_spec("file-1")],
    )
    .await
    .unwrap();
    assert!(client.accepted_files().contains("file-1"));
    let control = client.control();
    let runner = tokio::spawn(client.run());

    // A browser fetch traverses relay -> peer -> loopback juicehost -> relay.
    let response = reqwest::Client::new()
        .get(format!("{}/v/file-1.mp4", relay.http_base))
        .header("range", "bytes=0-3")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.text().await.unwrap(), "peer file bytes");

    control.shutdown().await;
    tokio::time::timeout(Duration::from_secs(5), runner)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    relay.shutdown();
    backend.server.shutdown();
    juicehost.shutdown();
}

#[tokio::test]
async fn peer_disconnect_mid_request_returns_503_offline() {
    let backend = spawn_authorize_backend(authorize_response(&["file-1"], &[])).await;
    let relay = spawn_relay(test_relay_config(&backend.server.base_url, "shared-secret")).await;

    let mut peer = connect_ws(&relay.ws_url).await;
    let token = mint_device_token("shared-secret", "device-1", "user-1");
    assert!(matches!(
        handshake(&mut peer, &token, vec![file_spec("file-1")]).await,
        PeerServerMessage::AnnounceOk { .. }
    ));

    let url = format!("{}/v/file-1.mp4", relay.http_base);
    let get = tokio::spawn(async move { reqwest::Client::new().get(url).send().await.unwrap() });
    let PeerServerMessage::Request { request_id, .. } =
        read_json::<PeerServerMessage>(&mut peer).await
    else {
        panic!("expected a relay Request frame");
    };
    assert!(request_id > 0);

    // The peer drops mid-stream; the relay must fail the waiting browser with
    // "peer is offline" rather than hanging until the request timeout.
    peer.close(None).await.unwrap();

    let response = tokio::time::timeout(Duration::from_secs(5), get)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["error"], "peer is offline");
}

#[tokio::test]
async fn file_id_cannot_be_hijacked_by_a_second_peer() {
    // The backend is configured to accept the id for both devices; the relay's
    // own registry must still refuse the second claimant.
    let backend = spawn_authorize_backend(authorize_response(&["file-1"], &[])).await;
    let relay = spawn_relay(test_relay_config(&backend.server.base_url, "shared-secret")).await;

    let mut first = connect_ws(&relay.ws_url).await;
    let first_reply = handshake(
        &mut first,
        &mint_device_token("shared-secret", "device-1", "user-1"),
        vec![file_spec("file-1")],
    )
    .await;
    match first_reply {
        PeerServerMessage::AnnounceOk { accepted, rejected } => {
            assert_eq!(accepted, vec!["file-1"]);
            assert_eq!(rejected, vec![]);
        }
        other => panic!("expected announce_ok for the first peer, got {other:?}"),
    }

    let mut second = connect_ws(&relay.ws_url).await;
    let second_reply = handshake(
        &mut second,
        &mint_device_token("shared-secret", "device-2", "user-2"),
        vec![file_spec("file-1")],
    )
    .await;
    match second_reply {
        PeerServerMessage::AnnounceOk { accepted, rejected } => {
            assert!(accepted.is_empty(), "hijacked id must not be accepted");
            assert_eq!(rejected.len(), 1);
            assert_eq!(rejected[0].id, "file-1");
            assert_eq!(rejected[0].reason, "served by another device");
        }
        other => panic!("expected announce_ok for the second peer, got {other:?}"),
    }
    assert_eq!(backend.calls.lock().unwrap().len(), 2);
}
