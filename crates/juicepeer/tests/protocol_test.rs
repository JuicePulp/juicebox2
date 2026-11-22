//! Wire-protocol contract tests exercised through `juiceproto`'s public API.

use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{EncodingKey, Header, encode};
use juiceproto::peer::{
    ISS_DEVICE, PEER_PROTOCOL_VERSION, PeerClientMessage, PeerDeviceClaims, PeerFileSpec,
    PeerRejection, PeerServerMessage, verify_peer_token,
};
use serde::{Serialize, de::DeserializeOwned};

fn roundtrip<T>(value: &T)
where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let json = serde_json::to_string(value).unwrap();
    let decoded: T = serde_json::from_str(&json).unwrap();
    assert_eq!(&decoded, value, "round-trip failed for {json}");
}

fn spec() -> PeerFileSpec {
    PeerFileSpec {
        id: "file-1".into(),
        filename: "movie.mp4".into(),
        mime_type: "video/mp4".into(),
        size_bytes: 4_096,
        expires_at: 9_999_999_999,
    }
}

fn headers() -> Vec<(String, String)> {
    vec![
        ("range".into(), "bytes=0-1023".into()),
        ("if-none-match".into(), "\"etag-1\"".into()),
    ]
}

#[test]
fn protocol_version_and_issuer_are_pinned() {
    assert_eq!(PEER_PROTOCOL_VERSION, 1);
    assert_eq!(ISS_DEVICE, "juiceback-device");
}

#[test]
fn structs_round_trip() {
    roundtrip(&spec());
    roundtrip(&PeerRejection {
        id: "file-2".into(),
        reason: "expired".into(),
    });
    roundtrip(&PeerDeviceClaims {
        sub: "device-1".into(),
        user_id: "user-1".into(),
        iss: ISS_DEVICE.into(),
        iat: 1_700_000_000,
        exp: 9_999_999_999,
    });
}

#[test]
fn every_client_message_round_trips() {
    let messages = [
        PeerClientMessage::Hello {
            version: PEER_PROTOCOL_VERSION,
            token: "jwt".into(),
        },
        PeerClientMessage::Announce {
            files: vec![spec()],
        },
        PeerClientMessage::Revoke {
            ids: vec!["file-1".into(), "file-2".into()],
        },
        PeerClientMessage::ResponseHead {
            request_id: 42,
            status: 206,
            headers: headers(),
        },
        PeerClientMessage::ResponseEnd { request_id: 42 },
        PeerClientMessage::Abort {
            request_id: 42,
            reason: "read error".into(),
        },
        PeerClientMessage::Pong,
    ];
    for message in messages {
        roundtrip(&message);
    }
}

#[test]
fn every_server_message_round_trips() {
    let messages = [
        PeerServerMessage::HelloOk {
            session_id: "session-1".into(),
        },
        PeerServerMessage::Error {
            code: "bad_token".into(),
            message: "invalid JWT".into(),
        },
        PeerServerMessage::AnnounceOk {
            accepted: vec!["file-1".into()],
            rejected: vec![PeerRejection {
                id: "file-2".into(),
                reason: "not owned".into(),
            }],
        },
        PeerServerMessage::Request {
            request_id: 7,
            route: "v".into(),
            id: "file-1".into(),
            filename: "movie.mp4".into(),
            headers: headers(),
        },
        PeerServerMessage::EndChunks { request_id: 7 },
        PeerServerMessage::Abort {
            request_id: 7,
            reason: "browser disconnected".into(),
        },
        PeerServerMessage::Ping,
    ];
    for message in messages {
        roundtrip(&message);
    }
}

#[test]
fn message_tags_are_snake_case_and_wire_shape_is_stable() {
    let client_cases = [
        (
            PeerClientMessage::Hello {
                version: 1,
                token: "jwt".into(),
            },
            "hello",
        ),
        (PeerClientMessage::Announce { files: vec![] }, "announce"),
        (PeerClientMessage::Revoke { ids: vec![] }, "revoke"),
        (
            PeerClientMessage::ResponseHead {
                request_id: 1,
                status: 200,
                headers: vec![],
            },
            "response_head",
        ),
        (
            PeerClientMessage::ResponseEnd { request_id: 1 },
            "response_end",
        ),
        (
            PeerClientMessage::Abort {
                request_id: 1,
                reason: "x".into(),
            },
            "abort",
        ),
        (PeerClientMessage::Pong, "pong"),
    ];
    for (message, tag) in client_cases {
        assert_eq!(serde_json::to_value(message).unwrap()["type"], tag);
    }

    let server_cases = [
        (
            PeerServerMessage::HelloOk {
                session_id: "s".into(),
            },
            "hello_ok",
        ),
        (
            PeerServerMessage::Error {
                code: "c".into(),
                message: "m".into(),
            },
            "error",
        ),
        (
            PeerServerMessage::AnnounceOk {
                accepted: vec![],
                rejected: vec![],
            },
            "announce_ok",
        ),
        (
            PeerServerMessage::Request {
                request_id: 1,
                route: "v".into(),
                id: "i".into(),
                filename: "f".into(),
                headers: vec![],
            },
            "request",
        ),
        (PeerServerMessage::EndChunks { request_id: 1 }, "end_chunks"),
        (
            PeerServerMessage::Abort {
                request_id: 1,
                reason: "x".into(),
            },
            "abort",
        ),
        (PeerServerMessage::Ping, "ping"),
    ];
    for (message, tag) in server_cases {
        assert_eq!(serde_json::to_value(message).unwrap()["type"], tag);
    }
}

fn sign(claims: &PeerDeviceClaims, secret: &str) -> String {
    encode(
        &Header::default(),
        claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .unwrap()
}

fn valid_claims() -> PeerDeviceClaims {
    PeerDeviceClaims {
        sub: "device-1".into(),
        user_id: "user-1".into(),
        iss: ISS_DEVICE.into(),
        iat: 1_700_000_000,
        exp: 9_999_999_999,
    }
}

#[test]
fn device_token_verification_accepts_valid_and_rejects_invalid() {
    let token = sign(&valid_claims(), "shared-secret");
    assert_eq!(
        verify_peer_token(&token, "shared-secret").unwrap(),
        valid_claims()
    );
    assert!(verify_peer_token(&token, "wrong-secret").is_err());

    let mut wrong_issuer = valid_claims();
    wrong_issuer.iss = "juiceback-admin".into();
    assert!(verify_peer_token(&sign(&wrong_issuer, "shared-secret"), "shared-secret").is_err());

    let mut expired = valid_claims();
    expired.exp = 1;
    assert!(verify_peer_token(&sign(&expired, "shared-secret"), "shared-secret").is_err());

    assert!(verify_peer_token("not-a-jwt", "shared-secret").is_err());
}

#[test]
fn device_token_verification_ignores_extra_backend_claims() {
    // juiceback's DeviceClaims carries extra optional fields; the relay only
    // needs the identity subset and must still accept the token.
    let now = usize::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    )
    .unwrap();
    let payload = serde_json::json!({
        "sub": "device-1",
        "user_id": "user-1",
        "iss": ISS_DEVICE,
        "iat": now,
        "exp": now + 3600,
        "file_id": "file-1",
        "filename": "movie.mp4",
        "mime_type": "video/mp4",
        "file_size": 4096u64,
    });
    let token = encode(
        &Header::default(),
        &payload,
        &EncodingKey::from_secret(b"shared-secret"),
    )
    .unwrap();
    let claims = verify_peer_token(&token, "shared-secret").unwrap();
    assert_eq!(claims.sub, "device-1");
    assert_eq!(claims.user_id, "user-1");
}
