//! AES-256-GCM encryption of client IP addresses for privacy-preserving logs.
//!
//! Ciphertexts are `"nonce_hex:ciphertext_hex"` where the ciphertext includes
//! the 16-byte auth tag.

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit},
};

/// Encrypt an IP address with AES-256-GCM using a random 12-byte nonce.
/// Returns `"nonce_hex:ciphertext_hex"` where ciphertext includes the 16-byte
/// auth tag.
#[must_use]
pub fn encrypt_ip(ip: &str, key_hex: &str) -> Option<String> {
    let key_bytes = hex::decode(key_hex).ok()?;
    let cipher = Aes256Gcm::new_from_slice(&key_bytes).ok()?;
    let nonce_bytes: [u8; 12] = rand::random();
    let nonce = Nonce::try_from(nonce_bytes.as_slice()).ok()?;
    let ciphertext = cipher.encrypt(&nonce, ip.as_bytes()).ok()?;
    Some(format!(
        "{}:{}",
        hex::encode(nonce_bytes),
        hex::encode(ciphertext)
    ))
}

/// Decrypt an IP address that was encrypted with [`encrypt_ip`].
#[must_use]
pub fn decrypt_ip(encrypted: &str, key_hex: &str) -> Option<String> {
    let (nonce_hex, ct_hex) = encrypted.split_once(':')?;
    let key_bytes = hex::decode(key_hex).ok()?;
    let nonce_bytes = hex::decode(nonce_hex).ok()?;
    let ct_bytes = hex::decode(ct_hex).ok()?;
    let cipher = Aes256Gcm::new_from_slice(&key_bytes).ok()?;
    let nonce = Nonce::try_from(nonce_bytes.as_slice()).ok()?;
    let plaintext = cipher.decrypt(&nonce, ct_bytes.as_ref()).ok()?;
    String::from_utf8(plaintext).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "0000000000000000000000000000000000000000000000000000000000000001";

    #[test]
    fn roundtrip_ipv4() {
        let encrypted = encrypt_ip("192.168.1.100", KEY).unwrap();
        assert_eq!(
            decrypt_ip(&encrypted, KEY).as_deref(),
            Some("192.168.1.100")
        );
    }

    #[test]
    fn roundtrip_ipv6() {
        let encrypted = encrypt_ip("2001:db8::1", KEY).unwrap();
        assert_eq!(decrypt_ip(&encrypted, KEY).as_deref(), Some("2001:db8::1"));
    }

    #[test]
    fn nonce_is_randomized() {
        let a = encrypt_ip("10.0.0.1", KEY).unwrap();
        let b = encrypt_ip("10.0.0.1", KEY).unwrap();
        assert_ne!(a, b);
        assert_eq!(decrypt_ip(&a, KEY).as_deref(), Some("10.0.0.1"));
        assert_eq!(decrypt_ip(&b, KEY).as_deref(), Some("10.0.0.1"));
    }

    #[test]
    fn wrong_key_fails() {
        let encrypted = encrypt_ip(
            "10.0.0.1",
            "0000000000000000000000000000000000000000000000000000000000000002",
        )
        .unwrap();
        assert_eq!(decrypt_ip(&encrypted, KEY), None);
    }

    #[test]
    fn malformed_inputs_fail() {
        assert_eq!(decrypt_ip("not-an-encrypted-value", KEY), None);
        assert_eq!(decrypt_ip("", KEY), None);
        assert_eq!(encrypt_ip("10.0.0.1", "zz"), None);
        assert_eq!(encrypt_ip("10.0.0.1", "abcd"), None);
    }
}
