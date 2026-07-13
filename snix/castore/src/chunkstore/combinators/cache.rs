use std::sync::Arc;

use tonic::async_trait;
use tracing::{instrument, trace, warn};

use crate::{
    B3Digest,
    chunkstore::{self, Chunk, ChunkStore},
    composition::{CompositionContext, ServiceBuilder},
};

/// Asks near first, if not found, asks far.
/// If found in there, returns it, and *inserts* it into
/// near.
/// There is no negative cache.
///
/// Near failures are treated as misses: far is the durable side. By default
/// writes go to near only; with `write_far`, writes go to far (and near is
/// warmed best-effort), so the cache can front a durable remote store.
pub struct Cache<CN, CF> {
    instance_name: String,
    near: CN,
    far: CF,
    write_far: bool,
}

impl<CN, CF> Cache<CN, CF> {
    /// Constructs a new [Cache] from the passed `instance_name`, `near` and `far` [ChunkStore]s.
    pub fn new(instance_name: String, near: CN, far: CF) -> Self {
        Self {
            instance_name,
            near,
            far,
            write_far: false,
        }
    }

    /// Like [Cache::new], but writes go to `far`, warming `near` best-effort.
    pub fn new_write_far(instance_name: String, near: CN, far: CF) -> Self {
        Self {
            instance_name,
            near,
            far,
            write_far: true,
        }
    }
}

#[async_trait]
impl<CN, CF> ChunkStore for Cache<CN, CF>
where
    CN: ChunkStore,
    CF: ChunkStore,
{
    #[instrument(skip(self, digest), fields(chunk.digest=%digest, instance_name=%self.instance_name))]
    async fn get(&self, digest: &B3Digest) -> Result<Option<Chunk>, chunkstore::Error> {
        // check near; a near failure shouldn't fail the read when far has the data.
        match self.near.get(digest).await {
            Ok(Some(chunk)) => {
                trace!("serving from cache");
                return Ok(Some(chunk));
            }
            Ok(None) => {}
            Err(e) => warn!(error = %e, "near get failed, asking far"),
        }

        trace!("not found in near, asking remote…");

        match self.far.get(digest).await.map_err(Error::FarGet)? {
            // chunk doesn't exist on the far side either, nothing we can do.
            None => Ok(None),
            Some(chunk) => {
                // chunk is present on the far side, insert it into near before returning
                // (best-effort: a broken near cache must not fail a read far answered).
                match self.near.put(chunk.clone()).await {
                    Ok(digest_near) if digest_near != *digest => {
                        Err(Error::InsertingMismatch {
                            expected: *digest,
                            actual: digest_near,
                        })?;
                    }
                    Ok(_) => {}
                    Err(e) => warn!(error = %e, "failed to populate near cache"),
                }

                Ok(Some(chunk))
            }
        }
    }

    #[instrument(skip_all, fields(chunk.digest=%digest, instance_name=%self.instance_name))]
    async fn has(&self, digest: &B3Digest) -> Result<bool, chunkstore::Error> {
        // Near failures are misses: far is the durable side.
        Ok(self.near.has(digest).await.unwrap_or(false)
            || self.far.has(digest).await.map_err(Error::FarGet)?)
    }

    #[instrument(skip_all, fields(instance_name=%self.instance_name))]
    async fn put(&self, chunk: Chunk) -> Result<B3Digest, chunkstore::Error> {
        if self.write_far {
            // far is the durable side; warm near so what was just written (and is usually
            // read back right after) is a cache hit.
            let digest = self.far.put(chunk.clone()).await.map_err(Error::FarPut)?;
            match self.near.put(chunk).await {
                Ok(digest_near) if digest_near != digest => {
                    warn!(%digest_near, %digest, "near/far chunk digests diverged");
                }
                Ok(_) => {}
                Err(e) => warn!(error = %e, "failed to warm near cache on put"),
            }
            Ok(digest)
        } else {
            Ok(self.near.put(chunk).await.map_err(Error::NearPut)?)
        }
    }
}

/// Error returned from [Cache].
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("wrong arguments: {0}")]
    WrongConfig(&'static str),

    #[error("getting from near: {0}")]
    NearGet(#[source] chunkstore::Error),
    #[error("putting into near: {0}")]
    NearPut(#[source] chunkstore::Error),
    #[error("getting from far: {0}")]
    FarGet(#[source] chunkstore::Error),
    #[error("putting into far: {0}")]
    FarPut(#[source] chunkstore::Error),
    #[error(
        "inserting chunk with digest {expected} into near returned different digest ({actual})"
    )]
    InsertingMismatch {
        expected: B3Digest,
        actual: B3Digest,
    },
}

impl From<Error> for chunkstore::Error {
    fn from(value: Error) -> Self {
        Self(Box::new(value))
    }
}

/// Configuration for a [Cache] [ChunkStore].
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct CacheConfig {
    near: String,
    far: String,
    #[serde(default)]
    write_far: bool,
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
    type Output = dyn ChunkStore;
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
            write_far: self.write_far,
        }))
    }
}

#[cfg(test)]
mod test {
    use crate::chunkstore::{Chunk, ChunkStore, MockChunkStore, memory::MemoryChunkStore};

    use super::Cache;

    const CHUNK: Chunk = Chunk::from_static(b"foo");

    /// A chunk present in near is served from there, far is not queried.
    #[tokio::test]
    async fn get_near_only() {
        let near = MemoryChunkStore::default();
        near.put(CHUNK.clone()).await.expect("to succeed");
        let mut far = MockChunkStore::new();
        far.expect_get().never();

        let uut = Cache::new("uut".to_string(), near, far);

        assert_eq!(
            Some(CHUNK.clone()),
            uut.get(&CHUNK.digest()).await.expect("to succeed")
        );
    }

    /// A chunk only present in far is returned, and inserted into near.
    #[tokio::test]
    async fn get_populates_near() {
        let near = MemoryChunkStore::default();
        let far_with_chunk = MemoryChunkStore::default();
        far_with_chunk.put(CHUNK.clone()).await.expect("to succeed");

        let uut = Cache::new("uut".to_string(), &near, far_with_chunk);

        // we know near is empty.
        assert!(
            !near.has(&CHUNK.digest()).await.expect("to succeed"),
            "near should be empty"
        );

        // query uut, which will populate near
        uut.get(&CHUNK.digest()).await.expect("to succeed");

        // now near should have the chunk.
        assert!(
            near.has(&CHUNK.digest()).await.expect("to succeed"),
            "near should be populated now"
        );
    }

    /// If neither has it, None is returned.
    #[tokio::test]
    async fn get_not_found() {
        let near = MemoryChunkStore::default();
        let far = MemoryChunkStore::default();

        let uut = Cache::new("uut".to_string(), near, far);

        assert_eq!(None, uut.get(&CHUNK.digest()).await.expect("to succeed"));
    }

    /// has() returns true if either near or far has the chunk.
    #[tokio::test]
    async fn has_near_or_far() {
        let cs_without = MemoryChunkStore::default();
        let cs_with = MemoryChunkStore::default();
        cs_with.put(CHUNK.clone()).await.expect("to succeed");

        let uut = Cache::new("uut".to_string(), &cs_without, &cs_with);
        assert!(uut.has(&CHUNK.digest()).await.expect("to succeed"));
        let uut = Cache::new("uut".to_string(), &cs_with, &cs_without);
        assert!(uut.has(&CHUNK.digest()).await.expect("to succeed"));
    }

    /// put() writes to near only.
    #[tokio::test]
    async fn put_writes_to_near() {
        let near = MemoryChunkStore::default();
        let far = MemoryChunkStore::default();

        let uut = Cache::new("uut".to_string(), near.clone(), far.clone());

        assert_eq!(
            CHUNK.digest(),
            uut.put(CHUNK.clone()).await.expect("to succeed")
        );

        assert!(near.has(&CHUNK.digest()).await.expect("to succeed"));
        assert!(!far.has(&CHUNK.digest()).await.expect("to succeed"));
    }

    /// With write_far, put() writes to far and warms near.
    #[tokio::test]
    async fn put_write_far_writes_both() {
        let near = MemoryChunkStore::default();
        let far = MemoryChunkStore::default();

        let uut = Cache::new_write_far("uut".to_string(), near.clone(), far.clone());

        assert_eq!(
            CHUNK.digest(),
            uut.put(CHUNK.clone()).await.expect("to succeed")
        );

        assert!(near.has(&CHUNK.digest()).await.expect("to succeed"));
        assert!(far.has(&CHUNK.digest()).await.expect("to succeed"));
    }
}
