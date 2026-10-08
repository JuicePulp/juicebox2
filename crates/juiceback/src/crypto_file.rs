//! At-rest encryption for files stored on untrusted juicehost backends.
//!
//! juicehost (local disk / S3) stores ciphertext only. The key lives only in
//! juiceback, which encrypts on upload (after plaintext validation) and
//! decrypts on gated download. This protects against a malicious host
//! operator, stolen disk, or bucket leak. It does NOT protect against the
//! juiceback operator.
//!
//! Wire format (v1), all integers little-endian:
//!
//! ```text
//! header: magic "JBC1" (4B) | chunk_shift u8 (1B, always 16) | orig_size u64 (8B)
//! body:   for each 64 KiB plaintext chunk: nonce (12B) | AES-256-GCM ct | tag (16B)
//! ```
//!
//! Chunking preserves streaming and HTTP Range without full-file decrypt.
//! Each chunk gets a fresh 96-bit random nonce from `OsRng`.

use std::{
    fmt,
    io::{Read, Write},
};

use aes_gcm::{Aes256Gcm, Key, KeyInit, Nonce, aead::Aead};
use rand::Rng;
use zeroize::Zeroizing;

/// Plaintext bytes per chunk. `CHUNK_SHIFT` is its base-2 logarithm.
pub const PLAINTEXT_CHUNK_LEN: usize = 64 * 1024;
/// Value stored in the header's `chunk_shift` byte.
pub const CHUNK_SHIFT: u8 = 16;
/// GCM nonce bytes prepended to every stored chunk.
pub const NONCE_LEN: usize = 12;
/// GCM tag bytes appended to every stored chunk ciphertext.
pub const TAG_LEN: usize = 16;
/// Magic bytes identifying this container.
pub const MAGIC: &[u8; 4] = b"JBC1";
/// Header length: 4B magic + 1B shift + 8B original size.
pub const HEADER_LEN: usize = 13;

/// 256-bit storage key. Zeroized on drop; never logged or Debug-printed.
///
/// Cloned only to hand a request-scoped copy to streaming decryptors; every
/// copy is still zeroized on drop.
#[derive(Clone)]
pub struct FileKey(Zeroizing<[u8; 32]>);

impl fmt::Debug for FileKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FileKey([redacted])")
    }
}

impl FileKey {
    /// Parse a 64-char hex string (e.g. from `STORAGE_ENCRYPTION_KEY`).
    ///
    /// # Errors
    ///
    /// Returns [`CryptoError::BadKey`] when the input is not 64 hex chars.
    pub fn from_hex(hex_str: &str) -> Result<Self, CryptoError> {
        let bytes = hex::decode(hex_str.trim()).map_err(|_| CryptoError::BadKey)?;
        Self::from_bytes(&bytes)
    }

    /// Build from raw bytes. Fails unless exactly 32 bytes are given.
    ///
    /// # Errors
    ///
    /// Returns [`CryptoError::BadKey`] when `bytes.len() != 32`.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        let arr: [u8; 32] = bytes.try_into().map_err(|_| CryptoError::BadKey)?;
        Ok(Self(Zeroizing::new(arr)))
    }

    fn cipher(&self) -> Aes256Gcm {
        let key = Key::<Aes256Gcm>::try_from(&self.0[..]).expect("FileKey always holds 32 bytes");
        Aes256Gcm::new(&key)
    }
}

/// Failures for [`encrypt_reader`], [`decrypt_to_writer`], [`decrypt_range`],
/// and header/key parsing. Messages never include key material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CryptoError {
    /// Key is not 32 bytes (or not 64 hex chars).
    BadKey,
    /// Container magic or chunk shift does not match v1.
    BadFormat,
    /// Ciphertext shorter than the header, chunk framing, or declared size.
    Truncated,
    /// Plaintext reader yielded a different byte count than declared.
    LengthMismatch,
    /// Requested byte range is outside `[0, orig_size]`.
    RangeOutOfBounds,
    /// GCM authentication failed (wrong key or tampered bytes).
    Decrypt,
    /// Underlying I/O failed.
    Io(String),
}

impl fmt::Display for CryptoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadKey => write!(f, "storage key must be 32 bytes"),
            Self::BadFormat => write!(f, "not a juicebox v1 encrypted file"),
            Self::Truncated => write!(f, "ciphertext truncated"),
            Self::LengthMismatch => write!(f, "plaintext length mismatch"),
            Self::RangeOutOfBounds => write!(f, "range outside file bounds"),
            Self::Decrypt => write!(f, "authentication failed"),
            Self::Io(detail) => write!(f, "storage crypto i/o: {detail}"),
        }
    }
}

impl std::error::Error for CryptoError {}

impl From<std::io::Error> for CryptoError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

/// Number of 64 KiB chunks covering `plain_len` bytes (at least one, so even
/// an empty file has a single empty chunk to authenticate).
#[must_use]
pub const fn chunk_count(plain_len: u64) -> u64 {
    let chunks = plain_len.div_ceil(PLAINTEXT_CHUNK_LEN as u64);
    if chunks == 0 { 1 } else { chunks }
}

/// Exact stored size for `plain_len` plaintext bytes:
/// header + plaintext + per-chunk nonce/tag overhead.
#[must_use]
pub const fn ciphertext_len(plain_len: u64) -> u64 {
    HEADER_LEN as u64 + plain_len + chunk_count(plain_len) * (NONCE_LEN as u64 + TAG_LEN as u64)
}

/// Stored length of one chunk holding `plain_len` plaintext bytes.
const fn stored_chunk_len(plain_len: usize) -> usize {
    NONCE_LEN + plain_len + TAG_LEN
}

/// Stored byte range covering plaintext `[start, end)` (end exclusive).
///
/// Lets callers issue a single Range GET for exactly the chunks overlapping
/// the requested plaintext, then decrypt and slice locally.
///
/// # Errors
///
/// Returns [`CryptoError::RangeOutOfBounds`] when `start > end` or
/// `end > plain_len`.
pub fn cipher_range_for_plain(
    start: u64,
    end: u64,
    plain_len: u64,
) -> Result<(u64, u64), CryptoError> {
    if start > end || end > plain_len {
        return Err(CryptoError::RangeOutOfBounds);
    }
    if start == end {
        return Ok((HEADER_LEN as u64, 0));
    }
    let chunk = PLAINTEXT_CHUNK_LEN as u64;
    let full_stored = NONCE_LEN as u64 + chunk + TAG_LEN as u64;
    let first = start / chunk;
    let last = (end - 1) / chunk;
    // Only the final chunk overall can be short, so every chunk before
    // `last` is full and offsets are closed-form.
    let cipher_start = HEADER_LEN as u64 + first * full_stored;
    let last_plain = (plain_len - last * chunk).min(chunk);
    let cipher_len =
        (last - first) * full_stored + (NONCE_LEN as u64 + last_plain + TAG_LEN as u64);
    Ok((cipher_start, cipher_len))
}

fn write_header(out: &mut Vec<u8>, plain_len: u64) {
    out.extend_from_slice(&encode_header(plain_len));
}

/// The 13-byte container header for `plain_len` plaintext bytes.
#[must_use]
pub const fn encode_header(plain_len: u64) -> [u8; 13] {
    let size = plain_len.to_le_bytes();
    [
        MAGIC[0], MAGIC[1], MAGIC[2], MAGIC[3], CHUNK_SHIFT, size[0], size[1], size[2], size[3],
        size[4], size[5], size[6], size[7],
    ]
}

/// Parse and validate the 13-byte header, returning the original plaintext size.
///
/// # Errors
///
/// Returns [`CryptoError::Truncated`] when fewer than 13 bytes are given and
/// [`CryptoError::BadFormat`] on magic/shift mismatch.
pub fn parse_header(data: &[u8]) -> Result<u64, CryptoError> {
    let header = data.get(..HEADER_LEN).ok_or(CryptoError::Truncated)?;
    if &header[..4] != MAGIC || header[4] != CHUNK_SHIFT {
        return Err(CryptoError::BadFormat);
    }
    let mut size = [0_u8; 8];
    size.copy_from_slice(&header[5..13]);
    Ok(u64::from_le_bytes(size))
}

/// Encrypt one chunk (`plaintext.len() <= 64 KiB`) to `nonce || ct || tag`.
///
/// # Errors
///
/// Returns [`CryptoError::LengthMismatch`] when the chunk is oversized. GCM
/// encryption itself cannot fail for valid inputs.
pub fn encrypt_chunk(key: &FileKey, plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if plaintext.len() > PLAINTEXT_CHUNK_LEN {
        return Err(CryptoError::LengthMismatch);
    }
    let mut nonce = [0_u8; NONCE_LEN];
    rand::rng().fill_bytes(&mut nonce);
    let ct = key
        .cipher()
        .encrypt(&Nonce::from(nonce), plaintext)
        .map_err(|_| CryptoError::Decrypt)?;
    let mut out = Vec::with_capacity(stored_chunk_len(plaintext.len()));
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Decrypt one stored `nonce || ct || tag` chunk.
///
/// # Errors
///
/// Returns [`CryptoError::Truncated`] on framing shortfall and
/// [`CryptoError::Decrypt`] on authentication failure.
pub fn decrypt_chunk(key: &FileKey, chunk: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if chunk.len() < NONCE_LEN + TAG_LEN || chunk.len() > stored_chunk_len(PLAINTEXT_CHUNK_LEN) {
        return Err(CryptoError::Truncated);
    }
    let (nonce, body) = chunk.split_at(NONCE_LEN);
    let nonce = Nonce::try_from(nonce).map_err(|_| CryptoError::Truncated)?;
    key.cipher()
        .decrypt(&nonce, body)
        .map_err(|_| CryptoError::Decrypt)
}

/// Encrypt exactly `plain_len` bytes from `reader` into a complete container.
///
/// # Errors
///
/// Returns [`CryptoError::LengthMismatch`] when the reader yields more or
/// fewer bytes than declared, plus I/O errors.
pub fn encrypt_reader(
    key: &FileKey,
    plain_len: u64,
    reader: impl Read,
) -> Result<Vec<u8>, CryptoError> {
    let capacity =
        usize::try_from(ciphertext_len(plain_len)).map_err(|_| CryptoError::LengthMismatch)?;
    let mut out = Vec::with_capacity(capacity);
    write_header(&mut out, plain_len);
    let mut reader = reader;
    let mut remaining = plain_len;
    let mut buf = vec![0_u8; PLAINTEXT_CHUNK_LEN];
    while remaining > 0 {
        let want = usize::try_from(remaining.min(PLAINTEXT_CHUNK_LEN as u64))
            .map_err(|_| CryptoError::LengthMismatch)?;
        let mut got = 0;
        while got < want {
            let n = reader.read(&mut buf[got..want])?;
            if n == 0 {
                return Err(CryptoError::LengthMismatch);
            }
            got += n;
        }
        out.extend_from_slice(&encrypt_chunk(key, &buf[..want])?);
        remaining -= want as u64;
    }
    if plain_len == 0 {
        out.extend_from_slice(&encrypt_chunk(key, &[])?);
    }
    // A longer-than-declared reader is a caller bug; refuse silently wrong output.
    let mut extra = [0_u8; 1];
    if reader.read(&mut extra)? != 0 {
        return Err(CryptoError::LengthMismatch);
    }
    Ok(out)
}

/// Decrypt a full container into `writer`, returning the plaintext size.
///
/// # Errors
///
/// Returns framing, authentication, and I/O errors; see [`CryptoError`].
pub fn decrypt_to_writer(
    key: &FileKey,
    data: &[u8],
    writer: impl Write,
) -> Result<u64, CryptoError> {
    let plain_len = parse_header(data)?;
    let mut body = &data[HEADER_LEN..];
    let mut writer = writer;
    let mut remaining = plain_len;
    while remaining > 0 || (plain_len == 0 && !body.is_empty()) {
        let want = usize::try_from(remaining.min(PLAINTEXT_CHUNK_LEN as u64))
            .map_err(|_| CryptoError::Truncated)?;
        let take = stored_chunk_len(if plain_len == 0 { 0 } else { want });
        let chunk = body.get(..take).ok_or(CryptoError::Truncated)?;
        body = &body[take..];
        let plain = decrypt_chunk(key, chunk)?;
        if plain.len() != want {
            return Err(CryptoError::Truncated);
        }
        writer.write_all(&plain)?;
        if plain_len == 0 {
            break;
        }
        remaining -= want as u64;
    }
    if !body.is_empty() {
        return Err(CryptoError::Truncated);
    }
    Ok(plain_len)
}

/// Decrypt `data[start..end]` (plaintext offsets, end exclusive), touching
/// only the overlapping chunks.
///
/// # Errors
///
/// Returns [`CryptoError::RangeOutOfBounds`] when `start > end` or
/// `end > orig_size`, plus framing/authentication errors.
pub fn decrypt_range(
    key: &FileKey,
    data: &[u8],
    start: u64,
    end: u64,
) -> Result<Vec<u8>, CryptoError> {
    let plain_len = parse_header(data)?;
    if start > end || end > plain_len {
        return Err(CryptoError::RangeOutOfBounds);
    }
    if start == end {
        return Ok(Vec::new());
    }
    let body = &data[HEADER_LEN..];
    // Walk stored chunks with a cursor instead of precomputing offsets so a
    // short/truncated tail surfaces as Truncated rather than panicking.
    let mut out = Vec::with_capacity(
        usize::try_from(end - start).map_err(|_| CryptoError::RangeOutOfBounds)?,
    );
    let mut chunk_index: u64 = 0;
    let mut cursor = body;
    let mut plain_off: u64 = 0;
    while plain_off < plain_len {
        let want = usize::try_from((plain_len - plain_off).min(PLAINTEXT_CHUNK_LEN as u64))
            .map_err(|_| CryptoError::Truncated)?;
        let take = stored_chunk_len(want);
        let chunk = cursor.get(..take).ok_or(CryptoError::Truncated)?;
        cursor = &cursor[take..];
        let chunk_start = chunk_index * PLAINTEXT_CHUNK_LEN as u64;
        let chunk_end = chunk_start + want as u64;
        if chunk_end > start && chunk_start < end {
            let plain = decrypt_chunk(key, chunk)?;
            let from = usize::try_from(start.saturating_sub(chunk_start))
                .map_err(|_| CryptoError::RangeOutOfBounds)?;
            let to = usize::try_from((end - chunk_start).min(want as u64))
                .map_err(|_| CryptoError::RangeOutOfBounds)?;
            out.extend_from_slice(&plain[from..to]);
        }
        plain_off += want as u64;
        chunk_index += 1;
    }
    if !cursor.is_empty() {
        return Err(CryptoError::Truncated);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> FileKey {
        FileKey::from_hex("00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff")
            .unwrap()
    }

    fn other_key() -> FileKey {
        FileKey::from_hex("ffeeddccbbaa99887766554433221100ffeeddccbbaa99887766554433221100")
            .unwrap()
    }

    fn pattern(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i.wrapping_mul(31).wrapping_add(7)) as u8).collect()
    }

    #[test]
    fn header_roundtrip_and_len() {
        for &len in &[0_u64, 1, 65_535, 65_536, 65_537, 200_000] {
            let data = encrypt_reader(&test_key(), len, &pattern(len as usize)[..]).unwrap();
            assert_eq!(data.len() as u64, ciphertext_len(len), "len {len}");
            assert_eq!(&data[..4], b"JBC1");
            assert_eq!(data[4], CHUNK_SHIFT);
            assert_eq!(parse_header(&data).unwrap(), len);
        }
    }

    #[test]
    fn decrypt_full_roundtrip() {
        for &len in &[0_usize, 1, 65_535, 65_536, 65_537, 200_000] {
            let plain = pattern(len);
            let data = encrypt_reader(&test_key(), len as u64, &plain[..]).unwrap();
            assert_ne!(&data[HEADER_LEN..], &plain[..]);
            let mut out = Vec::new();
            let n = decrypt_to_writer(&test_key(), &data, &mut out).unwrap();
            assert_eq!(n, len as u64);
            assert_eq!(out, plain);
        }
    }

    #[test]
    fn nonces_are_random() {
        let a = encrypt_reader(&test_key(), 10, &[9_u8; 10][..]).unwrap();
        let b = encrypt_reader(&test_key(), 10, &[9_u8; 10][..]).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn wrong_key_fails() {
        let data = encrypt_reader(&test_key(), 100, &pattern(100)[..]).unwrap();
        assert_eq!(
            decrypt_to_writer(&other_key(), &data, &mut Vec::new()).unwrap_err(),
            CryptoError::Decrypt
        );
    }

    #[test]
    fn tamper_fails() {
        let mut data = encrypt_reader(&test_key(), 70_000, &pattern(70_000)[..]).unwrap();
        data[HEADER_LEN + 20] ^= 1;
        assert_eq!(
            decrypt_to_writer(&test_key(), &data, &mut Vec::new()).unwrap_err(),
            CryptoError::Decrypt
        );
    }

    #[test]
    fn truncated_fails() {
        let data = encrypt_reader(&test_key(), 70_000, &pattern(70_000)[..]).unwrap();
        for cut in [5, HEADER_LEN, HEADER_LEN + 10, data.len() - 1] {
            assert_eq!(
                decrypt_to_writer(&test_key(), &data[..cut], &mut Vec::new()).unwrap_err(),
                CryptoError::Truncated
            );
        }
    }

    #[test]
    fn bad_header_rejected() {
        assert_eq!(parse_header(&[]).unwrap_err(), CryptoError::Truncated);
        assert_eq!(parse_header(&[0_u8; HEADER_LEN]).unwrap_err(), CryptoError::BadFormat);
        let mut good = vec![0_u8; HEADER_LEN];
        good[..4].copy_from_slice(b"JBC1");
        good[4] = 15;
        assert_eq!(parse_header(&good).unwrap_err(), CryptoError::BadFormat);
    }

    #[test]
    fn key_parsing() {
        assert!(FileKey::from_hex(&"ab".repeat(32)).is_ok());
        assert_eq!(FileKey::from_hex("abc").unwrap_err(), CryptoError::BadKey);
        assert_eq!(FileKey::from_hex(&"zz".repeat(32)).unwrap_err(), CryptoError::BadKey);
        assert_eq!(FileKey::from_bytes(&[1_u8; 31][..]).unwrap_err(), CryptoError::BadKey);
    }

    #[test]
    fn range_decrypt() {
        let key = test_key();
        let plain = pattern(200_000);
        let data = encrypt_reader(&key, 200_000, &plain[..]).unwrap();
        // Full, head, tail, cross-chunk middle, single byte, empty.
        for (start, end) in [
            (0, 200_000),
            (0, 10),
            (199_990, 200_000),
            (65_530, 65_545),
            (1_000, 150_000),
            (7, 8),
            (5, 5),
        ] {
            assert_eq!(
                decrypt_range(&key, &data, start, end).unwrap(),
                plain[start as usize..end as usize],
                "range {start}..{end}"
            );
        }
        // Out of bounds.
        for (start, end) in [(0, 200_001), (200_001, 200_001), (10, 5)] {
            assert_eq!(
                decrypt_range(&key, &data, start, end).unwrap_err(),
                CryptoError::RangeOutOfBounds,
                "range {start}..{end}"
            );
        }
        // Wrong key on a range still authenticates.
        assert_eq!(
            decrypt_range(&other_key(), &data, 0, 10).unwrap_err(),
            CryptoError::Decrypt
        );
    }

    #[test]
    fn cipher_range_covers_exact_chunks() {
        // Single-chunk file.
        assert_eq!(cipher_range_for_plain(0, 100, 100).unwrap(), (13, 12 + 100 + 16));
        // Empty range.
        assert_eq!(cipher_range_for_plain(5, 5, 100).unwrap(), (13, 0));
        // Two full chunks + short tail: range inside second chunk only.
        let len = 65_536 * 2 + 7;
        let (off, n) = cipher_range_for_plain(65_536, 65_540, len).unwrap();
        assert_eq!(off, 13 + (12 + 65_536 + 16));
        assert_eq!(n, 12 + 65_536 + 16);
        // Spanning chunks 0..2 (tail short): covers all three stored chunks.
        let (off, n) = cipher_range_for_plain(0, len, len).unwrap();
        assert_eq!(off, 13);
        assert_eq!(n, 2 * (12 + 65_536 + 16) + (12 + 7 + 16));
        assert_eq!(off + n, ciphertext_len(len));
        // Out of bounds.
        assert_eq!(
            cipher_range_for_plain(0, len + 1, len).unwrap_err(),
            CryptoError::RangeOutOfBounds
        );
        assert_eq!(
            cipher_range_for_plain(9, 3, len).unwrap_err(),
            CryptoError::RangeOutOfBounds
        );
    }

    #[test]
    fn length_mismatch_rejected() {
        assert_eq!(
            encrypt_reader(&test_key(), 10, &[1_u8; 9][..]).unwrap_err(),
            CryptoError::LengthMismatch
        );
        assert_eq!(
            encrypt_reader(&test_key(), 9, &[1_u8; 10][..]).unwrap_err(),
            CryptoError::LengthMismatch
        );
    }
}
