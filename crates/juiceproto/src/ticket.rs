use serde::{Deserialize, Serialize};

pub const ISSUER: &str = "juiceback-ticket";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TicketClaims {
    pub sub: String,
    pub user_id: String,
    pub file_id: String,
    pub filename: String,
    pub mime_type: String,
    pub file_size: u64,
    pub file_capability: String,
    #[serde(default)]
    pub iat: u64,
    pub exp: u64,
    pub iss: String,
}

impl TicketClaims {
    #[expect(
        clippy::too_many_arguments,
        reason = "the claims struct is the wire format; a builder would only rename the fields"
    )]
    #[must_use]
    pub fn new(
        sub: impl Into<String>,
        user_id: impl Into<String>,
        file_id: impl Into<String>,
        filename: impl Into<String>,
        mime_type: impl Into<String>,
        file_size: u64,
        capability: impl Into<String>,
        now: i64,
        ttl_secs: i64,
    ) -> Self {
        Self {
            sub: sub.into(),
            user_id: user_id.into(),
            file_id: file_id.into(),
            filename: filename.into(),
            mime_type: mime_type.into(),
            file_size,
            file_capability: capability.into(),
            iat: now.max(0) as u64,
            exp: (now + ttl_secs).max(0) as u64,
            iss: ISSUER.to_string(),
        }
    }
}

#[must_use]
pub fn validation() -> jsonwebtoken::Validation {
    use jsonwebtoken::{Algorithm, Validation};

    let mut validation = Validation::new(Algorithm::HS256);
    validation.set_required_spec_claims(&["exp", "iss"]);
    validation.set_issuer(&[ISSUER]);
    validation
}

pub fn mint(claims: &TicketClaims, secret: &str) -> Result<String, jsonwebtoken::errors::Error> {
    jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        claims,
        &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes()),
    )
}

/// Rejects empty claims even with a valid signature.
pub fn verify(token: &str, secret: &str) -> Result<TicketClaims, jsonwebtoken::errors::Error> {
    let claims = jsonwebtoken::decode::<TicketClaims>(
        token,
        &jsonwebtoken::DecodingKey::from_secret(secret.as_bytes()),
        &validation(),
    )?
    .claims;
    for (claim, present) in [
        ("sub", !claims.sub.is_empty()),
        ("user_id", !claims.user_id.is_empty()),
        ("file_id", !claims.file_id.is_empty()),
        ("filename", !claims.filename.is_empty()),
        ("file_capability", !claims.file_capability.is_empty()),
    ] {
        if !present {
            return Err(jsonwebtoken::errors::Error::from(
                jsonwebtoken::errors::ErrorKind::MissingRequiredClaim(claim.to_string()),
            ));
        }
    }
    Ok(claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now_secs() -> i64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        now.as_secs() as i64
    }

    fn claims(ttl_secs: i64) -> TicketClaims {
        TicketClaims::new(
            "browser",
            "user-1",
            "file-1",
            "a.txt",
            "text/plain",
            3,
            "cap-1",
            now_secs(),
            ttl_secs,
        )
    }

    #[test]
    fn round_trip_carries_every_claim() {
        let token = mint(&claims(300), "secret").unwrap();
        let decoded = verify(&token, "secret").unwrap();
        assert_eq!(decoded.user_id, "user-1");
        assert_eq!(decoded.file_id, "file-1");
        assert_eq!(decoded.file_size, 3);
        assert_eq!(decoded.file_capability, "cap-1");
    }

    #[test]
    fn wrong_secret_is_rejected() {
        let token = mint(&claims(300), "secret").unwrap();
        assert!(verify(&token, "other").is_err());
    }

    #[test]
    fn wrong_issuer_is_rejected() {
        let mut c = claims(300);
        c.iss = "juiceback-device".into();
        let token = mint(&c, "secret").unwrap();
        assert!(verify(&token, "secret").is_err());
    }

    #[test]
    fn expired_ticket_is_rejected() {
        let token = mint(&claims(-300), "secret").unwrap();
        assert!(verify(&token, "secret").is_err());
    }

    #[test]
    fn missing_exp_is_rejected() {
        let token = jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &serde_json::json!({"iss": ISSUER, "sub": "browser"}),
            &jsonwebtoken::EncodingKey::from_secret(b"secret"),
        )
        .unwrap();
        assert!(verify(&token, "secret").is_err());
    }

    #[test]
    fn missing_authorization_claims_is_rejected() {
        let token = jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &serde_json::json!({"iss": ISSUER, "exp": now_secs() + 300, "sub": "browser"}),
            &jsonwebtoken::EncodingKey::from_secret(b"secret"),
        )
        .unwrap();
        assert!(verify(&token, "secret").is_err());
    }

    #[test]
    fn missing_iss_is_rejected() {
        let token = jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &serde_json::json!({"exp": now_secs() + 300, "sub": "browser"}),
            &jsonwebtoken::EncodingKey::from_secret(b"secret"),
        )
        .unwrap();
        assert!(verify(&token, "secret").is_err());
    }
}
