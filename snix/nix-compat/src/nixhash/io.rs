//! Helpers that calcutate the hash of the data written
//! and count the number of bytes written.

use std::io;

use tokio::io::{AsyncBufRead, AsyncRead, AsyncWrite, copy, copy_buf};
use tokio_util::io::InspectWriter;

use super::{Sha256, Sha256Digester};

/// Asynchronously copies the entire contents of a reader into a writer while hashing.
pub async fn copy_sha256<'a, R, W>(
    reader: &'a mut R,
    writer: &'a mut W,
) -> io::Result<(u64, Sha256)>
where
    R: AsyncRead + Unpin + ?Sized,
    W: AsyncWrite + Unpin + ?Sized,
{
    let mut digester = Sha256Digester::new();
    let mut writer = InspectWriter::new(writer, |data| {
        digester.update(data);
    });
    let written = copy(reader, &mut writer).await?;
    let hash = digester.finalize();
    Ok((written, hash))
}

/// Asynchronously copies the entire contents of a reader into a writer while hashing
pub async fn copy_buf_sha256<'a, R, W>(
    reader: &'a mut R,
    writer: &'a mut W,
) -> io::Result<(u64, Sha256)>
where
    R: AsyncBufRead + Unpin + ?Sized,
    W: AsyncWrite + Unpin + ?Sized,
{
    let mut digester = Sha256Digester::new();
    let mut writer = InspectWriter::new(writer, |data| {
        digester.update(data);
    });
    let written = copy_buf(reader, &mut writer).await?;
    let hash = digester.finalize();
    Ok((written, hash))
}
