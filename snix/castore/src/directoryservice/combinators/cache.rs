use std::sync::Arc;

use futures::StreamExt;
use futures::TryStreamExt;
use futures::stream::BoxStream;
use tonic::async_trait;
use tracing::{instrument, trace};

use crate::composition::{CompositionContext, ServiceBuilder};
use crate::directoryservice::directory_graph::DirectoryGraphBuilder;
use crate::directoryservice::{self, DirectoryPutter, DirectoryService};
use crate::{B3Digest, Directory};

/// Asks near first, if not found, asks far.
/// If found in there, returns it, and *inserts* it into
/// near.
/// Specifically, it always obtains the entire directory closure from far and inserts it into near,
/// which is useful when far does not support accessing intermediate directories (but near does).
/// There is no negative cache.
/// Inserts and listings are not implemented for now.
pub struct Cache<DN, DF> {
    instance_name: String,
    near: DN,
    far: DF,
}

impl<DN, DF> Cache<DN, DF> {
    pub fn new(instance_name: String, near: DN, far: DF) -> Self {
        Self {
            instance_name,
            near,
            far,
        }
    }
}

#[async_trait]
impl<DN, DF> DirectoryService for Cache<DN, DF>
where
    DN: DirectoryService + Clone + 'static,
    DF: DirectoryService + Clone + 'static,
{
    #[instrument(skip(self, digest), fields(directory.digest = %digest, instance_name = %self.instance_name))]
    async fn get(&self, digest: &B3Digest) -> Result<Option<Directory>, directoryservice::Error> {
        // check near; a near failure shouldn't fail the read when far has the data.
        match self.near.get(digest).await {
            Ok(Some(directory)) => {
                trace!("serving from cache");
                return Ok(Some(directory));
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(error = %e, "near get failed, asking far");
            }
        }

        trace!("not found in near, asking remote…");
        // We always ask recursive, and populate the children to support far not allowing non-root access
        // We currently wait for all children to be received before returning
        // the requested directory, so subsequent children requests don't fail when these
        // stores are used.
        // FUTUREWORK: make this configurable, allow firing off a background task populating the children.
        let mut directories = self.far.get_recursive(digest);
        let mut graph_builder = DirectoryGraphBuilder::new_root_to_leaves(*digest);

        let mut resp_directory = None;
        while let Some(directory) = directories.try_next().await.map_err(Error::FarGet)? {
            graph_builder
                .try_insert(directory.clone())
                .map_err(Error::DirectoryOrdering)?;
            if resp_directory.is_none() {
                resp_directory = Some(directory);
            }
        }

        // If far had the directory, put into near (best-effort: a broken near cache must
        // not fail a read the far side answered).
        if let Some(resp_directory) = resp_directory {
            let directory_graph = graph_builder.build().map_err(Error::DirectoryOrdering)?;
            let populate = async {
                let mut near_putter = self.near.put_multiple_start();
                for directory in directory_graph.drain_leaves_to_root() {
                    near_putter.put(directory).await?;
                }
                near_putter.close().await
            };
            match populate.await {
                Ok(actual_digest) => debug_assert_eq!(digest, &actual_digest),
                Err(e) => tracing::warn!(error = %e, "failed to populate near cache"),
            }
            Ok(Some(resp_directory))
        } else {
            Ok(None)
        }
    }

    #[instrument(skip_all, fields(instance_name = %self.instance_name))]
    async fn put(&self, directory: Directory) -> Result<B3Digest, directoryservice::Error> {
        // Write through to far (the durable side); also warm near so subsequent
        // reads of what we just wrote are cache hits.
        let digest = self.far.put(directory.clone()).await.map_err(Error::FarPut)?;
        if let Err(e) = self.near.put(directory).await {
            trace!(error = %e, "failed to warm near cache on put");
        }
        Ok(digest)
    }

    #[instrument(skip_all, fields(directory.digest = %root_directory_digest, instance_name = %self.instance_name))]
    fn get_recursive(
        &self,
        root_directory_digest: &B3Digest,
    ) -> BoxStream<'_, Result<Directory, directoryservice::Error>> {
        let near = &self.near;
        let far = &self.far;
        let digest = *root_directory_digest;

        async_stream::try_stream! {
            // Try near first, but treat near errors as misses (far is the durable side).
            let mut near_first = None;
            let mut directories = near.get_recursive(&digest);
            match directories.try_next().await {
                Ok(first) => near_first = first,
                Err(e) => tracing::warn!(error = %e, "near get_recursive failed, asking far"),
            }
            if let Some(first) = near_first {
                trace!("serving from cache");
                yield first;

                while let Some(dir) = directories.try_next().await.map_err(Error::NearGet)? {
                    yield dir;
                }
                return;
            }
            drop(directories);

            trace!("not found in near, asking remote…");

            let mut directories = far.get_recursive(&digest);
            let mut builder = DirectoryGraphBuilder::new_root_to_leaves(digest);

            // Return to the client, while inserting to the graph builder.
            while let Some(directory) = directories.try_next().await.map_err(Error::FarGet)? {
                builder.try_insert(directory.clone()).map_err(Error::DirectoryOrdering)?;
                yield directory;
            }

            match builder.build() {
                Ok(directory_graph) => {
                    // Drain into near, best-effort.
                    let populate = async {
                        let mut near_putter = near.put_multiple_start();
                        for directory in directory_graph.drain_leaves_to_root() {
                            near_putter.put(directory).await?;
                        }
                        near_putter.close().await
                    };
                    match populate.await {
                        Ok(actual_digest) => debug_assert_eq!(digest, actual_digest),
                        Err(e) => tracing::warn!(error = %e, "failed to populate near cache"),
                    }
                }
                Err(crate::directoryservice::OrderingError::EmptySet) => return,
                Err(err) => Err(Error::DirectoryOrdering(err))?
            }
        }
        .boxed()
    }

    #[instrument(skip_all)]
    fn put_multiple_start(&self) -> Box<dyn DirectoryPutter + '_> {
        // Stream to far in one putter (far may validate closure connectivity per stream, e.g.
        // grpc), warming near along the way.
        Box::new(TeePutter {
            far: self.far.put_multiple_start(),
            near: self.near.put_multiple_start(),
            near_dead: false,
        })
    }
}

/// Feeds a directory stream to both sides of a [Cache]: far is the durable side (its close
/// digest is returned), near is warmed best-effort — on a near error, mirroring stops (a
/// half-written near subtree is only ever a cache miss, never served as a partial tree).
struct TeePutter<'a> {
    far: Box<dyn DirectoryPutter + 'a>,
    near: Box<dyn DirectoryPutter + 'a>,
    near_dead: bool,
}

#[async_trait]
impl DirectoryPutter for TeePutter<'_> {
    async fn put(&mut self, directory: Directory) -> Result<(), directoryservice::Error> {
        self.far
            .put(directory.clone())
            .await
            .map_err(Error::FarPut)?;
        if !self.near_dead
            && let Err(e) = self.near.put(directory).await
        {
            tracing::warn!(error = %e, "near put failed, no longer warming near");
            self.near_dead = true;
        }
        Ok(())
    }

    async fn close(&mut self) -> Result<B3Digest, directoryservice::Error> {
        let digest = self.far.close().await.map_err(Error::FarPut)?;
        if !self.near_dead
            && let Err(e) = self.near.close().await
        {
            tracing::warn!(error = %e, "near close failed");
        }
        Ok(digest)
    }
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("wrong arguments: {0}")]
    WrongConfig(&'static str),
    #[error("Directory Graph ordering error: {0}")]
    DirectoryOrdering(#[from] crate::directoryservice::OrderingError),
    #[error("serde-qs error: {0}")]
    SerdeQS(#[from] serde_qs::Error),

    #[error("getting from near: {0}")]
    NearGet(#[source] directoryservice::Error),
    #[error("putting into near: {0}")]
    NearPut(#[source] directoryservice::Error),
    #[error("getting from far: {0}")]
    FarGet(#[source] directoryservice::Error),
    #[error("putting into far: {0}")]
    FarPut(#[source] directoryservice::Error),

    #[error("puts are unimplemented")]
    Unimplemented,
}

impl From<Error> for directoryservice::Error {
    fn from(value: Error) -> Self {
        Self(Box::new(value))
    }
}

#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct CacheConfig {
    near: String,
    far: String,
}

impl TryFrom<url::Url> for CacheConfig {
    type Error = Box<dyn std::error::Error + Send + Sync>;
    fn try_from(url: url::Url) -> Result<Self, Self::Error> {
        // cache doesn't support host or path in the URL.
        if url.has_authority() || !url.path().is_empty() {
            return Err(Error::WrongConfig("no authority or path allowed").into());
        }
        Ok(serde_qs::from_str(url.query().unwrap_or_default())?)
    }
}

#[async_trait]
impl ServiceBuilder for CacheConfig {
    type Output = dyn DirectoryService;
    async fn build<'a>(
        &'a self,
        instance_name: &str,
        context: &CompositionContext,
    ) -> Result<Arc<Self::Output>, Box<dyn std::error::Error + Send + Sync>> {
        let (near, far) = futures::join!(
            context.resolve::<Self::Output>(&self.near),
            context.resolve::<Self::Output>(&self.far)
        );
        Ok(Arc::new(Cache {
            instance_name: instance_name.to_string(),
            near: near?,
            far: far?,
        }))
    }
}
