//! Wire protocol shared by the native juicepeer client and the juicepeer relay.
//!
//! JSON text frames carry [`PeerClientMessage`] / [`PeerServerMessage`]. Binary
//! frames carry a big-endian `u64` request id followed by raw file bytes. The
//! canonical end-of-chunks signal from the peer is
//! [`PeerClientMessage::ResponseEnd`]; [`PeerServerMessage::EndChunks`] is only
//! ever sent by the relay to abort a peer mid-stream.

// The `Peer*` prefixes are fixed by the wire contract shared with the relay and
// the native client, so they intentionally repeat this module's name.
#![allow(clippy::module_name_repetitions)]

use jsonwebtoken::{DecodingKey, Validation, decode};
use serde::{Deserialize, Serialize};

/// Version peers announce in `Hello.version`.
pub const PEER_PROTOCOL_VERSION: u16 = 1;

/// Issuer pinned on device JWTs. Kept in sync with `juiceback::auth::ISS_DEVICE`.
pub const ISS_DEVICE: &str = "juiceback-device";

/// One file a peer is willing to serve from its local juicehost.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerFileSpec {
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: u64,
    /// Unix seconds; mirrors the `expires_at` column of the files table.
    pub expires_at: i64,
}

/// A single file the backend refused to register for a peer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerRejection {
    pub id: String,
    pub reason: String,
}

/// Messages sent from the native peer to the relay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PeerClientMessage {
    Hello {
        version: u16,
        token: String,
    },
    Announce {
        files: Vec<PeerFileSpec>,
    },
    Revoke {
        ids: Vec<String>,
    },
    ResponseHead {
        request_id: u64,
        status: u16,
        headers: Vec<(String, String)>,
    },
    ResponseEnd {
        request_id: u64,
    },
    Abort {
        request_id: u64,
        reason: String,
    },
    Pong,
}

/// Messages sent from the relay to the native peer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PeerServerMessage {
    HelloOk {
        session_id: String,
    },
    Error {
        code: String,
        message: String,
    },
    AnnounceOk {
        accepted: Vec<String>,
        rejected: Vec<PeerRejection>,
    },
    Request {
        request_id: u64,
        route: String,
        id: String,
        filename: String,
        headers: Vec<(String, String)>,
    },
    EndChunks {
        request_id: u64,
    },
    Abort {
        request_id: u64,
        reason: String,
    },
    Ping,
}

/// Claims of a device JWT minted by juiceback with the shared ticket secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerDeviceClaims {
    pub sub: String,
    pub user_id: String,
    pub iss: String,
    pub iat: usize,
    pub exp: usize,
}

/// Verifies a device JWT presented by a peer.
///
/// The signature must be HS256 with the shared secret and `iss` must be
/// [`ISS_DEVICE`], so admin JWTs and expired tokens cannot be replayed against
/// the relay.
///
/// # Errors
///
/// Returns a description string when the signature, issuer, expiration, or
/// claim shape is invalid.
pub fn verify_peer_token(token: &str, secret: &str) -> Result<PeerDeviceClaims, String> {
    let mut validation = Validation::default();
    validation.set_issuer(&[ISS_DEVICE]);
    let token_data = decode::<PeerDeviceClaims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map_err(|e| format!("peer JWT verification failed: {e}"))?;
    Ok(token_data.claims)
}

#[cfg(test)]
mod tests {
    use jsonwebtoken::{EncodingKey, Header, encode};

    use super::*;

    fn roundtrip<T>(value: &T)
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + Eq + std::fmt::Debug,
    {
        let json = serde_json::to_string(value).unwrap();
        let decoded: T = serde_json::from_str(&json).unwrap();
        assert_eq!(&decoded, value);
    }

    fn file_spec() -> PeerFileSpec {
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
    fn file_spec_round_trips() {
        roundtrip(&file_spec());
    }

    #[test]
    fn rejection_round_trips() {
        roundtrip(&PeerRejection {
            id: "file-2".into(),
            reason: "expired".into(),
        });
    }

    #[test]
    fn client_hello_round_trips() {
        roundtrip(&PeerClientMessage::Hello {
            version: PEER_PROTOCOL_VERSION,
            token: "jwt".into(),
        });
    }

    #[test]
    fn client_announce_round_trips() {
        roundtrip(&PeerClientMessage::Announce {
            files: vec![file_spec()],
        });
    }

    #[test]
    fn client_revoke_round_trips() {
        roundtrip(&PeerClientMessage::Revoke {
            ids: vec!["file-1".into(), "file-2".into()],
        });
    }

    #[test]
    fn client_response_head_round_trips() {
        roundtrip(&PeerClientMessage::ResponseHead {
            request_id: 42,
            status: 206,
            headers: headers(),
        });
    }

    #[test]
    fn client_response_end_round_trips() {
        roundtrip(&PeerClientMessage::ResponseEnd { request_id: 42 });
    }

    #[test]
    fn client_abort_round_trips() {
        roundtrip(&PeerClientMessage::Abort {
            request_id: 42,
            reason: "read error".into(),
        });
    }

    #[test]
    fn client_pong_round_trips() {
        roundtrip(&PeerClientMessage::Pong);
    }

    #[test]
    fn server_hello_ok_round_trips() {
        roundtrip(&PeerServerMessage::HelloOk {
            session_id: "session-1".into(),
        });
    }

    #[test]
    fn server_error_round_trips() {
        roundtrip(&PeerServerMessage::Error {
            code: "bad_token".into(),
            message: "invalid JWT".into(),
        });
    }

    #[test]
    fn server_announce_ok_round_trips() {
        roundtrip(&PeerServerMessage::AnnounceOk {
            accepted: vec!["file-1".into()],
            rejected: vec![PeerRejection {
                id: "file-2".into(),
                reason: "not owned".into(),
            }],
        });
    }

    #[test]
    fn server_request_round_trips() {
        roundtrip(&PeerServerMessage::Request {
            request_id: 7,
            route: "v".into(),
            id: "file-1".into(),
            filename: "movie.mp4".into(),
            headers: headers(),
        });
    }

    #[test]
    fn server_end_chunks_round_trips() {
        roundtrip(&PeerServerMessage::EndChunks { request_id: 7 });
    }

    #[test]
    fn server_abort_round_trips() {
        roundtrip(&PeerServerMessage::Abort {
            request_id: 7,
            reason: "browser disconnected".into(),
        });
    }

    #[test]
    fn server_ping_round_trips() {
        roundtrip(&PeerServerMessage::Ping);
    }

    #[test]
    fn client_message_tags_are_snake_case() {
        let cases = [
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
        for (message, expected) in cases {
            let value = serde_json::to_value(message).unwrap();
            assert_eq!(value["type"], expected);
        }
    }

    #[test]
    fn server_message_tags_are_snake_case() {
        let cases = [
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
            (
                PeerServerMessage::EndChunks { request_id: 1 },
                "end_chunks",
            ),
            (
                PeerServerMessage::Abort {
                    request_id: 1,
                    reason: "x".into(),
                },
                "abort",
            ),
            (PeerServerMessage::Ping, "ping"),
        ];
        for (message, expected) in cases {
            let value = serde_json::to_value(message).unwrap();
            assert_eq!(value["type"], expected);
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
    fn verify_peer_token_accepts_valid_device_token() {
        let token = sign(&valid_claims(), "shared-secret");
        let claims = verify_peer_token(&token, "shared-secret").unwrap();
        assert_eq!(claims, valid_claims());
    }

    #[test]
    fn verify_peer_token_rejects_wrong_secret() {
        let token = sign(&valid_claims(), "shared-secret");
        assert!(verify_peer_token(&token, "wrong-secret").is_err());
    }

    #[test]
    fn verify_peer_token_rejects_wrong_issuer() {
        let mut claims = valid_claims();
        claims.iss = "juiceback-admin".into();
        let token = sign(&claims, "shared-secret");
        assert!(verify_peer_token(&token, "shared-secret").is_err());
    }

    #[test]
    fn verify_peer_token_rejects_expired_token() {
        let mut claims = valid_claims();
        claims.exp = 1;
        let token = sign(&claims, "shared-secret");
        assert!(verify_peer_token(&token, "shared-secret").is_err());
    }

    #[test]
    fn verify_peer_token_rejects_malformed_token() {
        assert!(verify_peer_token("not-a-jwt", "shared-secret").is_err());
    }

    #[test]
    fn verify_peer_token_ignores_extra_backend_claims() {
        let mut payload = serde_json::to_value(valid_claims()).unwrap();
        payload["file_id"] = serde_json::json!("file-1");
        let token = encode(
            &Header::default(),
            &payload,
            &EncodingKey::from_secret(b"shared-secret"),
        )
        .unwrap();
        assert_eq!(
            verify_peer_token(&token, "shared-secret").unwrap(),
            valid_claims()
        );
    }
}
