use std::sync::{Arc, atomic::Ordering};

use bytes::Bytes;
use futures::StreamExt;
use object_store::ObjectStoreExt;

use super::{
    ByteStream, FileData, FileMetadata, StorageBackend, StorageMetrics, TEMP_COUNTER,
    common::{capability_hash, safe_extension, valid_component},
};
use crate::error::StorageError;

/// S3-compatible backend. Objects are stored under `files/{id}.{ext}`.
pub struct S3Backend {
    /// The S3 object store client.
    client: Arc<dyn object_store::ObjectStore>,
}

impl S3Backend {
    pub fn new(
        bucket: &str,
        region: &str,
        endpoint: Option<&str>,
        access_key: &str,
        secret_key: &str,
        allow_http: bool,
    ) -> Result<Self, StorageError> {
        let mut builder = object_store::aws::AmazonS3Builder::new()
            .with_bucket_name(bucket)
            .with_region(region)
            .with_access_key_id(access_key)
            .with_secret_access_key(secret_key)
            .with_copy_if_not_exists(object_store::aws::S3CopyIfNotExists::Multipart);

        if let Some(ep) = endpoint {
            if ep.starts_with("http://") && !allow_http {
                return Err(StorageError::Io(
                    "S3_ENDPOINT uses HTTP; set S3_ALLOW_HTTP=true to allow it".into(),
                ));
            }
            builder = builder.with_endpoint(ep);
            builder = builder.with_allow_http(allow_http);
        }

        let client = builder.build()?;

        Ok(Self {
            client: Arc::new(client),
        })
    }

    pub(crate) fn object_key(id: &str, ext: &str) -> object_store::path::Path {
        object_store::path::Path::from(format!("files/{id}.{ext}"))
    }

    /// Resolve `id` to its stored object with a single LIST. Shared by
    /// `stat`/`get_stream`/`get_range_stream` so no path pays for the lookup
    /// twice.
    async fn find_object(&self, id: &str) -> Result<object_store::ObjectMeta, StorageError> {
        use futures::StreamExt;

        let prefix = object_store::path::Path::from(format!("files/{id}."));
        let mut list = self.client.list(Some(&prefix));
        list.next()
            .await
            .ok_or(StorageError::NotFound)?
            .map_err(|e| StorageError::Io(format!("S3 list failed: {e}")))
    }

    fn temp_key(id: &str, ext: &str) -> object_store::path::Path {
        object_store::path::Path::from(format!(
            "files/.{}.{}.{}.{}.tmp",
            id,
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed),
            ext
        ))
    }

    fn reservation_key(id: &str) -> object_store::path::Path {
        object_store::path::Path::from(format!("files/.reservations/{id}"))
    }

    fn capability_key(id: &str) -> object_store::path::Path {
        object_store::path::Path::from(format!("files/.capabilities/{id}"))
    }

    async fn create_capability(&self, id: &str, capability: &str) -> Result<(), StorageError> {
        self.client
            .put_opts(
                &Self::capability_key(id),
                object_store::PutPayload::from(Bytes::from(capability_hash(capability))),
                object_store::PutOptions {
                    mode: object_store::PutMode::Create,
                    ..Default::default()
                },
            )
            .await
            .map(|_| ())
            .map_err(Into::into)
    }

    async fn verify_capability(&self, id: &str, capability: &str) -> Result<(), StorageError> {
        let data = self
            .client
            .get(&Self::capability_key(id))
            .await
            .map_err(|_| StorageError::Forbidden)?
            .bytes()
            .await
            .map_err(|_| StorageError::Forbidden)?;
        if juiceutils::constant_time_eq(
            std::str::from_utf8(&data).unwrap_or_default(),
            &capability_hash(capability),
        ) {
            Ok(())
        } else {
            Err(StorageError::Forbidden)
        }
    }

    async fn remove_capability(&self, id: &str) {
        let _ = self.client.delete(&Self::capability_key(id)).await;
    }

    async fn reserve_id(&self, id: &str) -> Result<object_store::path::Path, StorageError> {
        if !valid_component(id) {
            return Err(StorageError::Io("invalid logical ID".into()));
        }
        let reservation = Self::reservation_key(id);
        self.client
            .put_opts(
                &reservation,
                object_store::PutPayload::from(Bytes::new()),
                object_store::PutOptions {
                    mode: object_store::PutMode::Create,
                    ..Default::default()
                },
            )
            .await?;
        match self.stat(id).await {
            Ok(_) => {
                let _ = self.client.delete(&reservation).await;
                return Err(StorageError::Conflict);
            }
            Err(StorageError::NotFound) => {}
            Err(error) => {
                let _ = self.client.delete(&reservation).await;
                return Err(error);
            }
        }
        Ok(reservation)
    }

    async fn release_id(&self, reservation: &object_store::path::Path) {
        if let Err(e) = self.client.delete(reservation).await {
            tracing::warn!("failed to release S3 ID reservation {reservation}: {e}");
        }
    }

    async fn write_bytes(&self, id: &str, filename: &str, data: Bytes) -> Result<(), StorageError> {
        let ext = safe_extension(filename);
        let path = Self::object_key(id, &ext);
        let reservation = self.reserve_id(id).await?;

        let payload = object_store::PutPayload::from(data);

        let result = self
            .client
            .put_opts(
                &path,
                payload,
                object_store::PutOptions {
                    mode: object_store::PutMode::Create,
                    ..Default::default()
                },
            )
            .await;
        self.release_id(&reservation).await;
        result.map(|_| ()).map_err(Into::into)
    }

    async fn rename_object(&self, old_id: &str, new_id: &str) -> Result<(), StorageError> {
        let meta = self.stat(old_id).await?;
        let source = Self::object_key(old_id, &meta.extension);
        let target = Self::object_key(new_id, &meta.extension);
        let reservation = self.reserve_id(new_id).await?;
        if let Err(error) = self.client.copy_if_not_exists(&source, &target).await {
            self.release_id(&reservation).await;
            return Err(error.into());
        }
        // The reservation is held across the delete so no concurrent writer
        // can claim the target; on delete failure the copy is rolled back,
        // leaving the source intact (a crash between copy and delete can
        // still duplicate (S3 has no atomic rename).
        if let Err(e) = self.delete(old_id, None).await {
            let _ = self.client.delete(&target).await;
            self.release_id(&reservation).await;
            return Err(e);
        }
        self.release_id(&reservation).await;
        Ok(())
    }

    async fn write_stream(
        &self,
        id: &str,
        filename: &str,
        mut data: ByteStream,
    ) -> Result<u64, StorageError> {
        let ext = safe_extension(filename);
        let path = Self::object_key(id, &ext);
        let temp_path = Self::temp_key(id, &ext);
        let reservation = self.reserve_id(id).await?;
        let upload = self
            .client
            .put_multipart(&temp_path)
            .await
            .map_err(|e| StorageError::Io(format!("S3 multipart init failed: {e}")));
        let upload = match upload {
            Ok(upload) => upload,
            Err(error) => {
                self.release_id(&reservation).await;
                return Err(error);
            }
        };
        let mut writer = object_store::WriteMultipart::new(upload);
        let mut total = 0u64;
        while let Some(chunk) = data.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(e) => {
                    let _ = writer.abort().await;
                    self.release_id(&reservation).await;
                    return Err(e);
                }
            };
            total = total.saturating_add(chunk.len() as u64);
            writer.put(chunk);
            if let Err(e) = writer.wait_for_capacity(4).await {
                let _ = writer.abort().await;
                self.release_id(&reservation).await;
                return Err(StorageError::Io(format!("S3 upload failed: {e}")));
            }
        }
        if let Err(e) = writer.finish().await {
            if let Err(delete_error) = self.client.delete(&temp_path).await {
                tracing::warn!(
                    "failed to remove S3 temp object {} after upload failure: {}",
                    temp_path,
                    delete_error
                );
            }
            self.release_id(&reservation).await;
            return Err(StorageError::Io(format!("S3 upload failed: {e}")));
        }
        if let Err(error) = self.client.copy_if_not_exists(&temp_path, &path).await {
            let _ = self.client.delete(&temp_path).await;
            self.release_id(&reservation).await;
            return Err(error.into());
        }
        if let Err(e) = self.client.delete(&temp_path).await {
            tracing::warn!("failed to remove S3 temp object {temp_path}: {e}");
        }
        self.release_id(&reservation).await;
        Ok(total)
    }

    async fn concat_objects(
        &self,
        target_id: &str,
        filename: &str,
        part_ids: &[&str],
    ) -> Result<(), StorageError> {
        use futures::StreamExt;

        let ext = safe_extension(filename);
        let reservation = self.reserve_id(target_id).await?;
        let target_path = Self::object_key(target_id, &ext);
        let temp_path = Self::temp_key(target_id, &ext);
        let upload = match self.client.put_multipart(&temp_path).await {
            Ok(upload) => upload,
            Err(e) => {
                self.release_id(&reservation).await;
                return Err(StorageError::Io(format!("S3 multipart init failed: {e}")));
            }
        };
        let mut writer = object_store::WriteMultipart::new(upload);
        let upload_result = async {
            for part_id in part_ids {
                let prefix = object_store::path::Path::from(format!("files/{part_id}."));
                let mut list = self.client.list(Some(&prefix));
                let obj = list
                    .next()
                    .await
                    .ok_or(StorageError::NotFound)?
                    .map_err(|e| StorageError::Io(format!("S3 list failed for {part_id}: {e}")))?;

                let get_result =
                    self.client.get(&obj.location).await.map_err(|e| {
                        StorageError::Io(format!("S3 get failed for {part_id}: {e}"))
                    })?;

                let mut stream = get_result.into_stream();
                while let Some(chunk) = stream.next().await {
                    let chunk = chunk.map_err(|e| {
                        StorageError::Io(format!("S3 read failed for {part_id}: {e}"))
                    })?;
                    writer.put(chunk);
                    writer
                        .wait_for_capacity(4)
                        .await
                        .map_err(|e| StorageError::Io(format!("S3 concat upload failed: {e}")))?;
                }
            }
            Ok::<(), StorageError>(())
        }
        .await;
        if let Err(error) = upload_result {
            let _ = writer.abort().await;
            self.release_id(&reservation).await;
            return Err(error);
        }
        if let Err(e) = writer.finish().await {
            if let Err(delete_error) = self.client.delete(&temp_path).await {
                tracing::warn!(
                    "failed to remove S3 temp object {} after concat failure: {}",
                    temp_path,
                    delete_error
                );
            }
            self.release_id(&reservation).await;
            return Err(StorageError::Io(format!("S3 concat upload failed: {e}")));
        }
        if let Err(error) = self
            .client
            .copy_if_not_exists(&temp_path, &target_path)
            .await
        {
            let _ = self.client.delete(&temp_path).await;
            self.release_id(&reservation).await;
            return Err(error.into());
        }
        let _ = self.client.delete(&temp_path).await;
        self.release_id(&reservation).await;

        for part_id in part_ids {
            if let Err(e) = self.delete(part_id, None).await {
                tracing::warn!("concat: failed to delete part {part_id}: {e}");
            }
        }

        Ok(())
    }
}

#[async_trait::async_trait]
impl StorageBackend for S3Backend {
    async fn put(
        &self,
        id: &str,
        filename: &str,
        data: Bytes,
        capability: Option<&str>,
    ) -> Result<(), StorageError> {
        if let Some(capability) = capability {
            self.create_capability(id, capability).await?;
        }
        let result = self.write_bytes(id, filename, data).await;
        if result.is_err() && capability.is_some() {
            self.remove_capability(id).await;
        }
        result
    }

    async fn put_stream(
        &self,
        id: &str,
        filename: &str,
        data: ByteStream,
        capability: Option<&str>,
    ) -> Result<u64, StorageError> {
        if let Some(capability) = capability {
            self.create_capability(id, capability).await?;
        }
        let result = self.write_stream(id, filename, data).await;
        if result.is_err() && capability.is_some() {
            self.remove_capability(id).await;
        }
        result
    }

    async fn get(&self, id: &str) -> Result<FileData, StorageError> {
        use futures::StreamExt;

        let prefix = object_store::path::Path::from(format!("files/{id}."));
        let mut list = self.client.list(Some(&prefix));

        let obj = list
            .next()
            .await
            .ok_or(StorageError::NotFound)?
            .map_err(|e| StorageError::Io(format!("S3 list failed: {e}")))?;

        let get_result = self
            .client
            .get(&obj.location)
            .await
            .map_err(|e| StorageError::Io(format!("S3 get failed: {e}")))?;

        let obj_meta = get_result.meta.clone();
        let etag = obj_meta.e_tag.clone().unwrap_or_else(|| {
            use std::time::{SystemTime, UNIX_EPOCH};
            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            format!("\"{ts:x}\"")
        });

        let data = get_result
            .bytes()
            .await
            .map_err(|e| StorageError::Io(format!("S3 read failed: {e}")))?;

        let key_str = obj.location.as_ref();
        let ext = key_str.rsplit('.').next().unwrap_or("bin").to_string();

        Ok(FileData {
            data,
            meta: FileMetadata {
                size: obj_meta.size,
                etag,
                extension: ext,
            },
        })
    }

    async fn delete(&self, id: &str, capability: Option<&str>) -> Result<bool, StorageError> {
        use futures::StreamExt;

        let prefix = object_store::path::Path::from(format!("files/{id}."));
        let mut list = self.client.list(Some(&prefix));

        // If the blob is already gone there is nothing to protect, so the
        // not-found report below runs before any capability check. This lets
        // cleanup purge orphaned rows without weakening auth for files that
        // still exist (those still require a valid capability). A list *error*
        // still goes through verification first, matching the old twin.
        let head = list.next().await;
        if head.is_none() {
            return Ok(false);
        }
        if let Some(capability) = capability {
            self.verify_capability(id, capability).await?;
        }
        let obj: object_store::ObjectMeta = match head {
            Some(Ok(obj)) => obj,
            _ => return Ok(false),
        };

        self.client
            .delete(&obj.location)
            .await
            .map_err(|e| StorageError::Io(format!("S3 delete failed: {e}")))?;

        self.remove_capability(id).await;
        Ok(true)
    }

    async fn rename(
        &self,
        old_id: &str,
        new_id: &str,
        capability: Option<&str>,
    ) -> Result<(), StorageError> {
        if let Some(capability) = capability {
            self.verify_capability(old_id, capability).await?;
            self.create_capability(new_id, capability).await?;
        }
        let result = self.rename_object(old_id, new_id).await;
        if result.is_err() && capability.is_some() {
            self.remove_capability(new_id).await;
        }
        result
    }

    async fn stat(&self, id: &str) -> Result<FileMetadata, StorageError> {
        let obj = self.find_object(id).await?;

        let key_str = obj.location.as_ref();
        let ext = key_str.rsplit('.').next().unwrap_or("bin").to_string();
        let etag = obj.e_tag.clone().unwrap_or_else(|| {
            use std::time::{SystemTime, UNIX_EPOCH};
            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            format!("\"{ts:x}\"")
        });

        Ok(FileMetadata {
            size: obj.size as u64,
            etag,
            extension: ext,
        })
    }

    async fn get_range_stream(
        &self,
        id: &str,
        start: u64,
        end: u64,
    ) -> Result<ByteStream, StorageError> {
        use futures::StreamExt;

        // No separate stat() here: find_object is the single LIST, and the
        // caller already statted for headers before choosing range vs full.
        let obj = self.find_object(id).await?;

        let result = self
            .client
            .get_opts(
                &obj.location,
                object_store::GetOptions {
                    range: Some(object_store::GetRange::Bounded(start..end + 1)),
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| StorageError::Io(format!("S3 range get failed: {e}")))?;
        let stream = result.into_stream().map(|result| {
            result.map_err(|e| StorageError::Io(format!("S3 range read failed: {e}")))
        });
        Ok(Box::pin(stream))
    }

    async fn get_stream(&self, id: &str) -> Result<ByteStream, StorageError> {
        let obj = self.find_object(id).await?;
        let stream = self
            .client
            .get(&obj.location)
            .await
            .map_err(|e| StorageError::Io(format!("S3 get failed: {e}")))?
            .into_stream()
            .map(|result| result.map_err(|e| StorageError::Io(format!("S3 read failed: {e}"))));
        Ok(Box::pin(stream))
    }

    fn storage_metrics(&self, _min_free_bytes: u64) -> StorageMetrics {
        StorageMetrics {
            total_bytes: 0,
            used_bytes: 0,
            free_bytes: u64::MAX,
            min_free_bytes: _min_free_bytes,
            out_of_space: false,
        }
    }

    async fn concat(
        &self,
        target_id: &str,
        filename: &str,
        part_ids: &[&str],
        capability: Option<&str>,
    ) -> Result<(), StorageError> {
        if let Some(capability) = capability {
            for part_id in part_ids {
                self.verify_capability(part_id, capability).await?;
            }
            self.create_capability(target_id, capability).await?;
        }
        let result = self.concat_objects(target_id, filename, part_ids).await;
        if result.is_err() && capability.is_some() {
            self.remove_capability(target_id).await;
        }
        result
    }
}

impl From<object_store::Error> for StorageError {
    fn from(error: object_store::Error) -> Self {
        match error {
            object_store::Error::AlreadyExists { .. }
            | object_store::Error::Precondition { .. } => Self::Conflict,
            error => Self::Io(error.to_string()),
        }
    }
}
