//! A [BlobEngine] wrapper that times blob finalization (close/PUT) into [perf_stats::WRITE] and
//! [perf_stats::WRITE_WALL]. Reads and the streaming write pass straight through; only the
//! per-blob commit — the small-file PUT that dominates substitution-write cost — is measured, so
//! full-snix can report how much of its wall goes to ingesting substituted NARs into the store.
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Instant;

use tokio::io::AsyncWrite;

use snix_castore::B3Digest;
use snix_castore::blob_engine::{self, BlobEngine};
use snix_castore::blobstore::{Reader, Writer};
use tonic::async_trait;

use crate::perf_stats;

#[derive(Clone)]
pub struct TimingBlobEngine<BE>(pub BE);

#[async_trait]
impl<BE> BlobEngine for TimingBlobEngine<BE>
where
    BE: BlobEngine,
{
    async fn has(&self, digest: &B3Digest) -> Result<bool, blob_engine::Error> {
        self.0.has(digest).await
    }

    async fn open_read(
        &self,
        digest: &B3Digest,
        size_hint: Option<u64>,
    ) -> Result<Option<Box<dyn Reader + 'static>>, blob_engine::Error> {
        self.0.open_read(digest, size_hint).await
    }

    async fn open_write<'a>(&'a self) -> Box<dyn Writer + 'a> {
        Box::new(TimingBlobWriter {
            inner: self.0.open_write().await,
        })
    }
}

struct TimingBlobWriter<'a> {
    inner: Box<dyn Writer + 'a>,
}

impl AsyncWrite for TimingBlobWriter<'_> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut *self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut *self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut *self.inner).poll_shutdown(cx)
    }
}

#[async_trait]
impl Writer for TimingBlobWriter<'_> {
    async fn close(&mut self) -> std::io::Result<B3Digest> {
        let t = Instant::now();
        let _wall = perf_stats::WRITE_WALL.enter();
        let r = self.inner.close().await;
        perf_stats::WRITE.record(t);
        r
    }
}
