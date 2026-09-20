use axum::body::Body;
use bytes::Bytes;
use futures::StreamExt;

use super::{
    common::deadline_body,
    serve::{RangeResult, parse_range},
};
use crate::storage::valid_component as is_valid_id;

#[test]
fn is_valid_id_valid() {
    assert!(is_valid_id("abc-123"));
    assert!(is_valid_id("test_file"));
    assert!(is_valid_id("ABC123"));
}

#[test]
fn is_valid_id_empty() {
    assert!(!is_valid_id(""));
}

#[test]
fn is_valid_id_with_spaces() {
    assert!(!is_valid_id("has space"));
}

#[test]
fn is_valid_id_with_special_chars() {
    assert!(!is_valid_id("file@name"));
    assert!(!is_valid_id("file.txt"));
    assert!(!is_valid_id("file/here"));
}

#[test]
fn is_valid_id_with_underscores_and_dashes() {
    assert!(is_valid_id("my_file-123"));
}

#[test]
fn is_valid_id_rejects_non_ascii_alphanumeric() {
    assert!(!is_valid_id("cafe-é"));
}

#[test]
fn parse_range_basic() {
    assert_eq!(
        parse_range("bytes=0-99", 1000, 1000),
        RangeResult::Satisfiable(0, 99)
    );
}

#[test]
fn parse_range_suffix() {
    assert_eq!(
        parse_range("bytes=-500", 1000, 1000),
        RangeResult::Satisfiable(500, 999)
    );
}

#[test]
fn parse_range_open_ended() {
    assert_eq!(
        parse_range("bytes=500-", 1000, 1000),
        RangeResult::Satisfiable(500, 999)
    );
}

#[test]
fn parse_range_out_of_bounds() {
    assert_eq!(
        parse_range("bytes=999-1999", 1000, 1000),
        RangeResult::Satisfiable(999, 999)
    );
}

#[test]
fn parse_range_invalid_format() {
    assert_eq!(parse_range("bytes=", 1000, 1000), RangeResult::Ignore);
    assert_eq!(parse_range("nope", 1000, 1000), RangeResult::Ignore);
}

#[test]
fn parse_range_start_beyond_end() {
    assert_eq!(
        parse_range("bytes=500-100", 1000, 1000),
        RangeResult::Ignore
    );
}

#[test]
fn parse_range_clamps_end_suffix_and_response_size() {
    assert_eq!(
        parse_range("bytes=10-9999", 1000, 20),
        RangeResult::Satisfiable(10, 29)
    );
    assert_eq!(
        parse_range("bytes=-5000", 1000, 2000),
        RangeResult::Satisfiable(0, 999)
    );
    assert_eq!(
        parse_range("bytes=1000-", 1000, 1000),
        RangeResult::Unsatisfiable
    );
    assert_eq!(
        parse_range("bytes=0-1,4-5", 1000, 1000),
        RangeResult::Ignore
    );
}

#[tokio::test]
async fn deadline_body_terminates_after_timeout() {
    let body = Body::from_stream(futures::stream::pending::<Result<Bytes, std::io::Error>>());
    let mut stream = deadline_body(
        body,
        std::time::Duration::from_millis(1),
        std::time::Duration::from_secs(1),
    )
    .into_data_stream();

    assert!(stream.next().await.unwrap().is_err());
    assert!(stream.next().await.is_none());
}
