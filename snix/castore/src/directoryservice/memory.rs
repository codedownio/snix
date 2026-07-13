//! An in-memory [DirectoryService] holding *decoded* [Directory] values.
//!
//! Unlike `redb+memory:` (which stores serialized protos and pays an encode on every put and a
//! decode + validation on every get), gets here clone an already-decoded [Directory]. That makes
//! it the right `near` side of a `cache:?near=&far=` composition for read-heavy consumers like
//! evaluation, where the same directories are resolved over and over.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use futures::stream::BoxStream;
use futures::{StreamExt, TryStreamExt};
use tonic::async_trait;
use tracing::instrument;

use super::{
    Directory, DirectoryPutter, DirectoryService, Error as CastoreError,
    simple_putter::SimplePutter, traversal,
};
use crate::B3Digest;
use crate::composition::{CompositionContext, ServiceBuilder};

#[derive(Clone, Default)]
pub struct MemoryDirectoryService {
    instance_name: String,
    db: Arc<RwLock<HashMap<B3Digest, Directory>>>,
}

impl MemoryDirectoryService {
    pub fn new(instance_name: String) -> Self {
        Self {
            instance_name,
            db: Default::default(),
        }
    }
}

#[async_trait]
impl DirectoryService for MemoryDirectoryService {
    #[instrument(skip(self, digest), fields(directory.digest = %digest, instance_name = %self.instance_name))]
    async fn get(&self, digest: &B3Digest) -> Result<Option<Directory>, CastoreError> {
        Ok(self.db.read().expect("poisoned lock").get(digest).cloned())
    }

    #[instrument(skip(self, directory), fields(directory.digest = %directory.digest(), instance_name = %self.instance_name))]
    async fn put(&self, directory: Directory) -> Result<B3Digest, CastoreError> {
        // [Directory] is validated at construction, so a present value is always well-formed.
        let digest = directory.digest();
        self.db
            .write()
            .expect("poisoned lock")
            .insert(digest, directory);
        Ok(digest)
    }

    #[instrument(skip_all, fields(directory.digest = %root_directory_digest, instance_name = %self.instance_name))]
    fn get_recursive(
        &self,
        root_directory_digest: &B3Digest,
    ) -> BoxStream<'_, Result<Directory, CastoreError>> {
        let this = self.clone();
        traversal::root_to_leaves(*root_directory_digest, move |digest| {
            let this = this.clone();
            async move { this.get(&digest).await }
        })
        .map_err(|e| CastoreError(Box::new(e)))
        .boxed()
    }

    #[instrument(skip_all)]
    fn put_multiple_start<'a>(&'a self) -> Box<dyn DirectoryPutter + 'a> {
        Box::new(SimplePutter::new(self))
    }
}

#[derive(serde::Deserialize, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct MemoryDirectoryServiceConfig {}

impl TryFrom<url::Url> for MemoryDirectoryServiceConfig {
    type Error = Box<dyn std::error::Error + Send + Sync>;
    fn try_from(url: url::Url) -> Result<Self, Self::Error> {
        // memory doesn't support host or path in the URL.
        if url.has_host() || !url.path().is_empty() {
            return Err("no host or path allowed".into());
        }
        Ok(Self {})
    }
}

#[async_trait]
impl ServiceBuilder for MemoryDirectoryServiceConfig {
    type Output = dyn DirectoryService;
    async fn build<'a>(
        &'a self,
        instance_name: &str,
        _context: &CompositionContext,
    ) -> Result<Arc<Self::Output>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Arc::new(MemoryDirectoryService::new(
            instance_name.to_string(),
        )))
    }
}
