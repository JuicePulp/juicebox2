use std::sync::Arc;

use bytes::Bytes;
use futures::StreamExt;
use object_store::ObjectStoreExt;

use crate::error::StorageError;

pub struct R2Store {
    client: Arc<dyn object_store::ObjectStore>,
}

impl R2Store {
    pub fn new(
        bucket: &str,
        endpoint: &str,
        region: &str,
        access_key: &str,
        secret_key: &str,
    ) -> Result<Self, StorageError> {
        if !endpoint.starts_with("https://") {
            return Err(StorageError::Io(
                "R2 endpoint must be an https:// URL".into(),
            ));
        }
        let client = object_store::aws::AmazonS3Builder::new()
            .with_bucket_name(bucket)
            .with_region(region)
            .with_endpoint(endpoint)
            .with_access_key_id(access_key)
            .with_secret_access_key(secret_key)
            .build()
            .map_err(|e| StorageError::Io(format!("R2 client build failed: {e}")))?;
        Ok(Self {
            client: Arc::new(client),
        })
    }

    pub fn hot_key(id: &str, ext: &str) -> String {
        format!("hot/{id}.{ext}")
    }

    pub fn hot_prefix(id: &str) -> String {
        format!("hot/{id}.")
    }

    pub async fn put_bytes(&self, key: &str, bytes: Bytes, content_type: &str) -> Result<(), StorageError> {
        let mut attributes = object_store::Attributes::new();
        if !content_type.is_empty() {
            attributes.insert(
                object_store::Attribute::ContentType,
                content_type.to_string().into(),
            );
        }
        attributes.insert(
            object_store::Attribute::CacheControl,
            "public, max-age=86400".to_string().into(),
        );
        let opts = object_store::PutOptions {
            attributes,
            ..Default::default()
        };
        self.client
            .put_opts(
                &object_store::path::Path::from(key),
                object_store::PutPayload::from(bytes),
                opts,
            )
            .await
            .map(|_| ())
            .map_err(|e| StorageError::Io(format!("R2 put failed: {e}")))
    }

    pub async fn delete_prefix(&self, prefix: &str) -> Result<usize, StorageError> {
        let mut list = self
            .client
            .list(Some(&object_store::path::Path::from(prefix)));
        let mut removed = 0usize;
        while let Some(obj) = list.next().await {
            let obj = obj.map_err(|e| StorageError::Io(format!("R2 list failed: {e}")))?;
            self.client
                .delete(&obj.location)
                .await
                .map_err(|e| StorageError::Io(format!("R2 delete failed: {e}")))?;
            removed += 1;
        }
        Ok(removed)
    }
}
