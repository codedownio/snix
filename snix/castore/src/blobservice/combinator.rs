use std::sync::Arc;

use tokio::io::AsyncRead;
use tonic::async_trait;
use tracing::instrument;

use crate::B3Digest;
use crate::composition::{CompositionContext, ServiceBuilder};

use super::{BlobReader, BlobService, BlobWriter, ChunkedReader};

/// Cache for a BlobService, using a "near" and "far" blobservice.
/// Requests are tried in (and returned from) the near store first, only if
/// things are not present there, the far BlobService is queried.
/// In case the near blobservice doesn't have the blob, we ask the remote
/// blobservice for chunks, and try to read each of these chunks from the near
/// blobservice again, before falling back to the far one.
/// While doing so, the full blob is written to the near blobservice.
/// By default the far BlobService is never written to; with `write_far`,
/// writes go to far instead (near is a pure read cache over a durable far).
#[derive(Clone)]
pub struct Cache<BN, BF>
where
    BN: Clone,
    BF: Clone,
{
    instance_name: String,
    near: BN,
    far: BF,
    write_far: bool,
}

impl<BN, BF> Cache<BN, BF>
where
    BN: Clone,
    BF: Clone,
{
    pub fn new(instance_name: String, near: BN, far: BF) -> Self {
        Self {
            instance_name,
            near,
            far,
            write_far: false,
        }
    }

    pub fn new_write_far(instance_name: String, near: BN, far: BF) -> Self {
        Self {
            instance_name,
            near,
            far,
            write_far: true,
        }
    }
}

#[async_trait]
impl<BN, BF> BlobService for Cache<BN, BF>
where
    BN: BlobService + Clone + 'static,
    BF: BlobService + Clone + 'static,
{
    #[instrument(skip(self, digest), fields(blob.digest=%digest, instance_name=%self.instance_name))]
    async fn has(&self, digest: &B3Digest) -> std::io::Result<bool> {
        Ok(self.near.has(digest).await? || self.far.has(digest).await?)
    }

    #[instrument(skip(self, digest), fields(blob.digest=%digest, instance_name=%self.instance_name), err)]
    async fn open_read(&self, digest: &B3Digest) -> std::io::Result<Option<Box<dyn BlobReader>>> {
        if self.near.has(digest).await? {
            // near store has the blob, so we can assume it also has all chunks.
            self.near.open_read(digest).await
        } else {
            // near store doesn't have the blob.
            // Ask the remote one for the list of chunks,
            // and create a chunked reader that uses self.open_read() for
            // individual chunks. There's a chance we already have some chunks
            // in near, meaning we don't need to fetch them all from the far
            // BlobService.
            match self.far.chunks(digest).await? {
                // blob doesn't exist on the far side either, nothing we can do.
                None => Ok(None),
                Some(remote_chunks) => {
                    let mut far_reader = {
                        // if there's no more granular chunks, or the far
                        // blobservice doesn't support chunks, read the blob from
                        // the far blobservice directly.
                        if remote_chunks.is_empty() {
                            if let Some(reader) = self.far.open_read(digest).await? {
                                Box::new(reader) as Box<dyn AsyncRead + Unpin + Send>
                            } else {
                                return Ok(None);
                            }
                        } else {
                            // otherwise, a chunked reader, which will always try the
                            // near backend first.
                            Box::new(ChunkedReader::from_chunks(
                                remote_chunks.into_iter().map(|chunk| {
                                    (
                                        chunk.digest.try_into().expect("invalid b3 digest"),
                                        chunk.size,
                                    )
                                }),
                                self.clone(),
                            )) as Box<dyn AsyncRead + Unpin + Send>
                        }
                    };

                    // Blob is present on the remote blobservice.
                    // Copy it into the near blobservice, then return from there.
                    let mut near_blobwriter = self.near.open_write().await;
                    tokio::io::copy(&mut far_reader, &mut near_blobwriter).await?;

                    let written_digest = near_blobwriter.close().await?;
                    if written_digest != *digest {
                        return Err(std::io::Error::other(
                            "blob written to near blobservice returned unexpected digest",
                        ));
                    }

                    return self.near.open_read(digest).await;
                }
            }
        }
    }

    #[instrument(skip_all, fields(instance_name=%self.instance_name))]
    async fn open_write(&self) -> Box<dyn BlobWriter> {
        if self.write_far {
            // Tee into near as well: what gets written is usually read back right after
            // (NAR calculation, build inputs), which should be cache hits.
            Box::new(TeeBlobWriter {
                far: self.far.open_write().await,
                near: self.near.open_write().await,
                pending: Vec::new(),
            })
        } else {
            self.near.open_write().await
        }
    }
}

/// Writes to both sides of a write-far [Cache]; far is the durable side (its digest is
/// returned, and a near/far digest mismatch is an error). far is polled first; bytes it
/// accepts are owed to near (`pending`) and drained before accepting more.
struct TeeBlobWriter {
    far: Box<dyn BlobWriter>,
    near: Box<dyn BlobWriter>,
    pending: Vec<u8>,
}

impl TeeBlobWriter {
    /// Drain bytes owed to near. Ready(Ok) when nothing is owed.
    fn poll_drain_near(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let Self { near, pending, .. } = self;
        while !pending.is_empty() {
            match std::pin::Pin::new(&mut **near).poll_write(cx, pending) {
                std::task::Poll::Ready(Ok(n)) => {
                    pending.drain(..n);
                }
                std::task::Poll::Ready(Err(e)) => return std::task::Poll::Ready(Err(e)),
                std::task::Poll::Pending => return std::task::Poll::Pending,
            }
        }
        std::task::Poll::Ready(Ok(()))
    }
}

impl tokio::io::AsyncWrite for TeeBlobWriter {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match self.poll_drain_near(cx) {
            std::task::Poll::Ready(Ok(())) => {}
            other => return other.map_ok(|()| 0),
        }
        match std::pin::Pin::new(&mut self.far).poll_write(cx, buf) {
            std::task::Poll::Ready(Ok(n)) => {
                self.pending.extend_from_slice(&buf[..n]);
                std::task::Poll::Ready(Ok(n))
            }
            other => other,
        }
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.poll_drain_near(cx) {
            std::task::Poll::Ready(Ok(())) => {}
            other => return other,
        }
        match std::pin::Pin::new(&mut self.far).poll_flush(cx) {
            std::task::Poll::Ready(Ok(())) => std::pin::Pin::new(&mut self.near).poll_flush(cx),
            other => other,
        }
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.poll_drain_near(cx) {
            std::task::Poll::Ready(Ok(())) => {}
            other => return other,
        }
        match std::pin::Pin::new(&mut self.far).poll_shutdown(cx) {
            std::task::Poll::Ready(Ok(())) => std::pin::Pin::new(&mut self.near).poll_shutdown(cx),
            other => other,
        }
    }
}

#[async_trait]
impl BlobWriter for TeeBlobWriter {
    async fn close(&mut self) -> std::io::Result<B3Digest> {
        if !self.pending.is_empty() {
            let pending = std::mem::take(&mut self.pending);
            tokio::io::AsyncWriteExt::write_all(&mut self.near, &pending).await?;
        }
        let far_digest = self.far.close().await?;
        let near_digest = self.near.close().await?;
        if far_digest != near_digest {
            return Err(std::io::Error::other(
                "near/far blob digests diverged in tee write",
            ));
        }
        Ok(far_digest)
    }
}

#[derive(serde::Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct CacheBlobServiceConfig {
    near: String,
    far: String,
    #[serde(default)]
    write_far: bool,
}

impl TryFrom<url::Url> for CacheBlobServiceConfig {
    type Error = Box<dyn std::error::Error + Send + Sync>;
    fn try_from(_url: url::Url) -> Result<Self, Self::Error> {
        Err("Instantiating a CacheBlobService from a url is not supported".into())
    }
}

#[async_trait]
impl ServiceBuilder for CacheBlobServiceConfig {
    type Output = dyn BlobService;
    async fn build<'a>(
        &'a self,
        instance_name: &str,
        context: &CompositionContext,
    ) -> Result<Arc<Self::Output>, Box<dyn std::error::Error + Send + Sync>> {
        let (near, far) = futures::join!(
            context.resolve::<dyn BlobService>(&self.near),
            context.resolve::<dyn BlobService>(&self.far)
        );
        Ok(Arc::new(Cache {
            instance_name: instance_name.to_string(),
            near: near?,
            far: far?,
            write_far: self.write_far,
        }))
    }
}
