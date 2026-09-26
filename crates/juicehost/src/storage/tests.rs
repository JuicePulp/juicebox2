use std::sync::Arc;

use crate::storage::*;

#[test]
fn extract_extension_with_ext() {
    assert_eq!(extract_extension("test.png"), "png");
}

#[test]
fn extract_extension_no_ext() {
    assert_eq!(extract_extension("noext"), "bin");
}

#[test]
fn extract_extension_double_ext() {
    assert_eq!(extract_extension("file.tar.gz"), "gz");
}

#[test]
fn guess_mime_txt() {
    let mime = guess_mime("txt");
    assert!(mime.starts_with("text/plain"));
    assert!(mime.contains("charset=utf-8"));
}

#[test]
fn guess_mime_html() {
    let mime = guess_mime("html");
    assert!(mime.starts_with("text/html"));
}

#[test]
fn guess_mime_png() {
    assert_eq!(guess_mime("png"), "image/png");
}

#[test]
fn guess_mime_unknown() {
    let mime = guess_mime("xyz");
    assert!(!mime.is_empty());
}

#[test]
fn guess_mime_no_ext() {
    assert_eq!(guess_mime("bin"), "application/octet-stream");
}

#[test]
fn s3_object_key() {
    let key = S3Backend::object_key("abc123", "png");
    assert_eq!(key.as_ref(), "files/abc123.png");
}

async fn setup_local_backend() -> (tempfile::TempDir, LocalBackend) {
    let dir = tempfile::tempdir().unwrap();
    let backend = LocalBackend::new(dir.path().to_path_buf(), 0).unwrap();
    (dir, backend)
}

#[tokio::test]
async fn local_put_and_get() {
    let (_dir, backend) = setup_local_backend().await;
    backend
        .put("file1", "test.txt", Bytes::from("hello"), None)
        .await
        .unwrap();
    let data = backend.get("file1").await.unwrap();
    assert_eq!(data.data.as_ref(), b"hello");
    assert_eq!(data.meta.extension, "txt");
}

#[tokio::test]
async fn local_put_does_not_overwrite_existing_file() {
    let (_dir, backend) = setup_local_backend().await;
    backend
        .put("file1", "test.txt", Bytes::from("first"), None)
        .await
        .unwrap();
    assert!(matches!(
        backend
            .put("file1", "test.txt", Bytes::from("second"), None)
            .await,
        Err(StorageError::Conflict)
    ));
    assert_eq!(backend.get("file1").await.unwrap().data.as_ref(), b"first");
}

#[tokio::test]
async fn local_logical_id_is_unique_across_extensions() {
    let (_dir, backend) = setup_local_backend().await;
    backend
        .put("same", "first.txt", Bytes::from_static(b"first"), None)
        .await
        .unwrap();
    assert!(matches!(
        backend
            .put("same", "second.png", Bytes::from_static(b"second"), None)
            .await,
        Err(StorageError::Conflict)
    ));
    assert_eq!(backend.stat("same").await.unwrap().extension, "txt");
}

#[tokio::test]
async fn local_logical_id_reservation_is_atomic_across_extensions() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(LocalBackend::new(dir.path().to_path_buf(), 0).unwrap());
    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let mut tasks = Vec::new();
    for filename in ["first.txt", "second.png"] {
        let backend = Arc::clone(&backend);
        let barrier = Arc::clone(&barrier);
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            backend
                .put("raced", filename, Bytes::from_static(b"data"), None)
                .await
        }));
    }
    barrier.wait().await;
    let results = futures::future::join_all(tasks).await;
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Ok(Ok(()))))
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Ok(Err(StorageError::Conflict))))
            .count(),
        1
    );
}

#[cfg(unix)]
#[tokio::test]
async fn local_cache_rejects_symlink_entries() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::NamedTempFile::new().unwrap();
    symlink(outside.path(), dir.path().join("linked.txt")).unwrap();
    let backend = LocalBackend::new(dir.path().to_path_buf(), 0).unwrap();
    assert!(backend.init_cache().await.is_err());
}

#[tokio::test]
async fn local_stream_failure_removes_partial_file() {
    let (dir, backend) = setup_local_backend().await;
    let stream = futures::stream::iter([
        Ok(Bytes::from_static(b"partial")),
        Err(StorageError::Io("request failed".into())),
    ]);
    assert!(
        backend
            .put_stream("broken", "file.txt", Box::pin(stream), None)
            .await
            .is_err()
    );
    assert!(backend.stat("broken").await.is_err());
    let entries: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert!(entries.is_empty());
}

#[tokio::test]
async fn local_stream_does_not_overwrite_existing_file() {
    let (_dir, backend) = setup_local_backend().await;
    backend
        .put("file1", "test.txt", Bytes::from("first"), None)
        .await
        .unwrap();
    let stream = futures::stream::once(async { Ok(Bytes::from_static(b"second")) });
    assert!(matches!(
        backend
            .put_stream("file1", "test.txt", Box::pin(stream), None)
            .await,
        Err(StorageError::Conflict)
    ));
    assert_eq!(backend.get("file1").await.unwrap().data.as_ref(), b"first");
}

#[tokio::test]
async fn local_delete() {
    let (_dir, backend) = setup_local_backend().await;
    backend
        .put("d1", "f.bin", Bytes::from("data"), None)
        .await
        .unwrap();
    let deleted = backend.delete("d1", None).await.unwrap();
    assert!(deleted);
    assert!(backend.get("d1").await.is_err());
}

#[tokio::test]
async fn local_delete_not_found() {
    let (_dir, backend) = setup_local_backend().await;
    let deleted = backend.delete("nonexistent", None).await.unwrap();
    assert!(!deleted);
}

#[tokio::test]
async fn local_rename() {
    let (_dir, backend) = setup_local_backend().await;
    backend
        .put("old", "f.txt", Bytes::from("content"), None)
        .await
        .unwrap();
    backend.rename("old", "new", None).await.unwrap();
    assert!(backend.get("old").await.is_err());
    let data = backend.get("new").await.unwrap();
    assert_eq!(data.data.as_ref(), b"content");
}

#[tokio::test]
async fn local_rename_not_found() {
    let (_dir, backend) = setup_local_backend().await;
    let result = backend.rename("missing", "new", None).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn local_get_not_found() {
    let (_dir, backend) = setup_local_backend().await;
    let result = backend.get("nope").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn local_concat() {
    let (_dir, backend) = setup_local_backend().await;
    backend
        .put("p1", "a.txt", Bytes::from("hello"), None)
        .await
        .unwrap();
    backend
        .put("p2", "b.txt", Bytes::from(" world"), None)
        .await
        .unwrap();
    backend
        .concat("merged", "merged.txt", &["p1", "p2"], None)
        .await
        .unwrap();
    let data = backend.get("merged").await.unwrap();
    assert_eq!(data.data.as_ref(), b"hello world");
    assert!(backend.get("p1").await.is_err());
    assert!(backend.get("p2").await.is_err());
}

#[tokio::test]
async fn local_put_stores_file() {
    let dir = tempfile::tempdir().unwrap();
    let backend = LocalBackend::new(dir.path().to_path_buf(), 0).unwrap();
    backend
        .put("deep-id", "file.txt", Bytes::from("data"), None)
        .await
        .unwrap();
    let data = backend.get("deep-id").await.unwrap();
    assert_eq!(data.data.as_ref(), b"data");
}

#[tokio::test]
async fn local_capability_gates_mutations() {
    let (_dir, backend) = setup_local_backend().await;
    backend
        .put("f1", "a.txt", Bytes::from("data"), Some("cap"))
        .await
        .unwrap();
    assert!(matches!(
        backend.delete("f1", Some("wrong")).await,
        Err(StorageError::Forbidden)
    ));
    assert!(backend.get("f1").await.is_ok());
    backend.rename("f1", "f2", Some("cap")).await.unwrap();
    assert!(backend.delete("f2", Some("cap")).await.unwrap());
}

#[tokio::test]
async fn local_capability_missing_file_reports_not_found() {
    let (_dir, backend) = setup_local_backend().await;
    assert!(!backend.delete("ghost", Some("stale")).await.unwrap());
}

#[tokio::test]
async fn local_capability_failed_put_rolls_back() {
    let (_dir, backend) = setup_local_backend().await;
    backend
        .put("f1", "a.txt", Bytes::from("data"), None)
        .await
        .unwrap();
    assert!(matches!(
        backend
            .put("f1", "a.txt", Bytes::from("other"), Some("cap"))
            .await,
        Err(StorageError::Conflict)
    ));

    assert!(matches!(
        backend.delete("f1", Some("cap")).await,
        Err(StorageError::Forbidden)
    ));
    assert!(backend.delete("f1", None).await.unwrap());
}

#[tokio::test]
async fn local_storage_metrics() {
    let (_dir, backend) = setup_local_backend().await;
    let metrics = backend.storage_metrics(0);
    assert!(metrics.total_bytes > 0);
    assert!(!metrics.out_of_space);
}

#[tokio::test]
async fn local_cache_init() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("abc.txt"), "data").unwrap();
    let backend = LocalBackend::new(dir.path().to_path_buf(), 0).unwrap();
    backend.init_cache().await.unwrap();
    assert_eq!(
        backend.extensions.get("abc").map(|e| e.value().clone()),
        Some("txt".into())
    );
}
