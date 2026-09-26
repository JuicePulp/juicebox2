use super::{metadata::*, parallel::collect_ordered_parts};
use crate::{error::AppError, upload_mode::UploadMode};

#[test]
fn parse_tus_metadata_empty() {
    let result = parse_tus_metadata(None);
    assert!(result.is_empty());
}

#[test]
fn parse_tus_metadata_single_pair() {
    let header = axum::http::HeaderValue::from_static("filename dGVzdC50eHQ=");
    let result = parse_tus_metadata(Some(&header));
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, "filename");
    assert_eq!(result[0].1, "test.txt");
}

#[test]
fn parse_tus_metadata_multiple_pairs() {
    let header =
        axum::http::HeaderValue::from_static("filename dGVzdC50eHQ=, mimetype dGV4dC9wbGFpbg==");
    let result = parse_tus_metadata(Some(&header));
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].0, "filename");
    assert_eq!(result[0].1, "test.txt");
    assert_eq!(result[1].0, "mimetype");
    assert_eq!(result[1].1, "text/plain");
}

#[test]
fn parse_tus_metadata_invalid_base64() {
    let header = axum::http::HeaderValue::from_static("key !!!invalid!!!");
    let result = parse_tus_metadata(Some(&header));
    assert!(result.is_empty());
}

#[test]
fn find_meta_found() {
    let meta = vec![
        ("filename".into(), "test.txt".into()),
        ("mimetype".into(), "text/plain".into()),
    ];
    assert_eq!(find_meta(&meta, "filename"), Some("test.txt"));
}

#[test]
fn find_meta_not_found() {
    let meta = vec![("filename".into(), "test.txt".into())];
    assert_eq!(find_meta(&meta, "mimetype"), None);
}

#[test]
fn find_meta_empty() {
    assert_eq!(find_meta(&[], "filename"), None);
}

#[test]
fn parallel_metadata_requires_complete_bounded_tuple() {
    assert!(
        validate_parallel_metadata(None, None, None)
            .unwrap()
            .is_none()
    );
    assert!(validate_parallel_metadata(Some("session"), Some(0), Some(4)).is_ok());
    assert!(validate_parallel_metadata(Some("session"), None, Some(4)).is_err());
    assert!(validate_parallel_metadata(Some("session"), Some(4), Some(4)).is_err());
    assert!(
        validate_parallel_metadata(
            Some("session"),
            Some(0),
            Some(crate::constants::MAX_TUS_PARALLEL_PARTS + 1),
        )
        .is_err()
    );
    assert!(parse_parallel_number(Some("not-a-number")).is_err());
}

#[test]
fn ordered_parts_use_actual_declared_lengths() {
    let session = crate::tus::PartSession {
        session_id: "session".into(),
        total_parts: 3,
        filename: "file.bin".into(),
        mime_type: "application/octet-stream".into(),
        hashed_ip: "hash".into(),
        storage_host: None,
        ttl_hours: 24.0,
        reserve_id: None,
        reservation_token: None,
        capability: "capability".into(),
        user_id: "user-1".into(),
        upload_mode: UploadMode::Standard,
        declared_size: std::sync::atomic::AtomicU64::new(17),
        completed: std::sync::atomic::AtomicUsize::new(3),
        part_ids: dashmap::DashMap::new(),
        part_lengths: dashmap::DashMap::new(),
        completion_reserve_id: std::sync::Mutex::new(None),
    };
    session.part_ids.insert(2, "part-c".into());
    session.part_ids.insert(0, "part-a".into());
    session.part_ids.insert(1, "part-b".into());
    session.part_lengths.insert(0, 8);
    session.part_lengths.insert(1, 8);
    session.part_lengths.insert(2, 1);

    let (ids, size) = collect_ordered_parts(&session, 3).unwrap();
    assert_eq!(ids, vec!["part-a", "part-b", "part-c"]);
    assert_eq!(size, 17);
}

#[test]
fn upload_offset_is_checked_before_streaming() {
    assert_eq!(checked_upload_offset(4, 6, 10).unwrap(), 10);
    assert!(matches!(
        checked_upload_offset(4, 7, 10),
        Err(AppError::PayloadTooLarge)
    ));
    assert!(matches!(
        checked_upload_offset(u64::MAX, 1, u64::MAX),
        Err(AppError::PayloadTooLarge)
    ));
}

#[tokio::test]
async fn bounded_body_accepts_multi_frame_under_cap() {
    use futures::stream::{self, StreamExt as _};

    use super::patch::collect_bounded_body;

    let frames = vec![
        axum::body::Bytes::from_static(b"hello "),
        axum::body::Bytes::from_static(b"chunked "),
        axum::body::Bytes::from_static(b"world"),
    ];
    let body = axum::body::Body::from_stream(stream::iter(frames).map(Ok::<_, axum::Error>));
    let out = collect_bounded_body(body, 1024).await.unwrap();
    assert_eq!(out, b"hello chunked world");
}

#[tokio::test]
async fn bounded_body_rejects_over_cap_before_buffering_all() {
    use futures::stream::{self, StreamExt as _};

    use super::patch::collect_bounded_body;

    let frames = vec![
        axum::body::Bytes::from(vec![0_u8; 512]),
        axum::body::Bytes::from(vec![0_u8; 512]),
        axum::body::Bytes::from(vec![0_u8; 512]),
        axum::body::Bytes::from(vec![0_u8; 512]),
    ];
    let body = axum::body::Body::from_stream(stream::iter(frames).map(Ok::<_, axum::Error>));
    assert!(matches!(
        collect_bounded_body(body, 1024).await,
        Err(AppError::PayloadTooLarge)
    ));
}

#[test]
fn gzip_decode_roundtrip() {
    use std::io::Write as _;

    use super::patch::decode_gzip_bounded;

    let original = b"The quick brown fox jumps over the lazy dog".repeat(100);
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&original).unwrap();
    let compressed = encoder.finish().unwrap();

    let decoded = decode_gzip_bounded(compressed, 1024 * 1024).unwrap();
    assert_eq!(decoded.as_ref(), original.as_slice());
}

#[test]
fn gzip_decode_rejects_zip_bomb() {
    use std::io::Write as _;

    use super::patch::decode_gzip_bounded;

    let original = vec![0_u8; 1024 * 1024];
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&original).unwrap();
    let compressed = encoder.finish().unwrap();
    assert!(compressed.len() < 4096);

    assert!(matches!(
        decode_gzip_bounded(compressed, 1024),
        Err(AppError::PayloadTooLarge)
    ));
}

#[test]
fn gzip_decode_rejects_corrupt_input() {
    use super::patch::decode_gzip_bounded;

    assert!(matches!(
        decode_gzip_bounded(b"not gzip at all".to_vec(), 1024 * 1024),
        Err(AppError::GzipDecodeFailed)
    ));
}
