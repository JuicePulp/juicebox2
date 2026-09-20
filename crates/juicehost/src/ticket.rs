//! Ticket JWT verification shared by the server middleware and upload handlers.

pub fn verify_ticket_jwt(
    token: &str,
    secret: &str,
) -> Result<serde_json::Value, jsonwebtoken::errors::Error> {
    use jsonwebtoken::{DecodingKey, decode};

    Ok(decode::<serde_json::Value>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &ticket_validation(),
    )?
    .claims)
}

#[must_use]
pub fn ticket_validation() -> jsonwebtoken::Validation {
    use jsonwebtoken::{Algorithm, Validation};

    let mut validation = Validation::new(Algorithm::HS256);
    validation.set_required_spec_claims(&["exp"]);
    validation.set_issuer(&["juiceback-ticket"]);
    validation
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mint(claims: serde_json::Value, secret: &[u8]) -> String {
        jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(secret),
        )
        .unwrap()
    }

    fn base_claims(iss: &str, exp_offset_secs: i64) -> serde_json::Value {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        serde_json::json!({
            "sub": "dev-1",
            "iss": iss,
            "iat": now,
            "exp": now + exp_offset_secs,
        })
    }

    #[test]
    fn valid_ticket_verifies() {
        let token = mint(base_claims("juiceback-ticket", 300), b"secret");
        let claims = verify_ticket_jwt(&token, "secret").unwrap();
        assert_eq!(claims.get("sub").and_then(|v| v.as_str()), Some("dev-1"));
    }

    #[test]
    fn wrong_secret_rejected() {
        let token = mint(base_claims("juiceback-ticket", 300), b"secret");
        assert!(verify_ticket_jwt(&token, "wrong").is_err());
    }

    #[test]
    fn wrong_issuer_rejected() {
        let token = mint(base_claims("juiceback-device", 300), b"secret");
        assert!(verify_ticket_jwt(&token, "secret").is_err());
    }

    #[test]
    fn expired_ticket_rejected() {
        // Default jsonwebtoken leeway is 60s, so expire well past it.
        let token = mint(base_claims("juiceback-ticket", -300), b"secret");
        assert!(verify_ticket_jwt(&token, "secret").is_err());
    }

    #[test]
    fn missing_exp_rejected() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let token = mint(
            serde_json::json!({"sub": "dev-1", "iss": "juiceback-ticket", "iat": now}),
            b"secret",
        );
        assert!(verify_ticket_jwt(&token, "secret").is_err());
    }
}
