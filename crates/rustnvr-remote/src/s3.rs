//! S3-compatible object storage (Amazon S3, MinIO, Backblaze B2, Wasabi…).

use crate::{S3Target, Sink, join_key};
use object_store::aws::{AmazonS3, AmazonS3Builder};
use object_store::path::Path as ObjectPath;
use object_store::{ObjectStoreExt as _, PutPayload};
use std::path::Path;

pub(crate) struct S3Sink {
    store: AmazonS3,
    prefix: String,
}

impl S3Sink {
    pub(crate) fn new(t: &S3Target) -> Result<Self, String> {
        let mut builder = AmazonS3Builder::new()
            .with_region(&t.region)
            .with_bucket_name(&t.bucket)
            .with_access_key_id(&t.access_key_id)
            .with_secret_access_key(t.secret_access_key.expose());
        if let Some(endpoint) = &t.endpoint {
            builder = builder
                .with_endpoint(endpoint)
                // Plain HTTP only when the endpoint says so (local MinIO…).
                .with_allow_http(endpoint.starts_with("http://"));
        }
        let store = builder.build().map_err(|e| format!("S3 : {e}"))?;
        Ok(Self {
            store,
            prefix: t.prefix.clone(),
        })
    }
}

impl Sink for S3Sink {
    async fn upload(&mut self, local: &Path, key: &str) -> Result<(), String> {
        // Segments are a few tens of MB: one request each.
        let data = tokio::fs::read(local)
            .await
            .map_err(|e| format!("{}: {e}", local.display()))?;
        let location = ObjectPath::parse(join_key(&self.prefix, key)).map_err(|e| e.to_string())?;
        self.store
            .put(&location, PutPayload::from(data))
            .await
            .map(|_| ())
            .map_err(|e| format!("S3 : {e}"))
    }
}
