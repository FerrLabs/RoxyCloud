pub mod local;
pub mod s3;

#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::pin::Pin;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use tokio::io::AsyncRead;

pub use local::LocalBlobStore;
pub use s3::S3BlobStore;

use roxycloud_core::blob::BlobHash;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("blob {0} is not in the store")]
    NotFound(BlobHash),
    #[error("reading the upload stream failed")]
    Upstream(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("blob store i/o failed")]
    Io(#[from] std::io::Error),
    #[error("object store request failed: {0}")]
    Remote(String),
}

/// Bytes on their way in. Boxed because the trait is used through `dyn`, and adapted from whatever
/// the caller has by [`upload`].
pub type Upload = Pin<Box<dyn Stream<Item = Result<Bytes, StorageError>> + Send>>;

/// Bytes on their way out. A local file and an object body are both this.
pub type Reader = Pin<Box<dyn AsyncRead + Send>>;

pub fn upload<S, E>(chunks: S) -> Upload
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    Box::pin(chunks.map(|chunk| chunk.map_err(|err| StorageError::Upstream(Box::new(err)))))
}

#[derive(Debug, Clone)]
pub struct Written {
    pub hash: BlobHash,
    pub size: u64,
    pub deduplicated: bool,
    /// A temp file the write left for `settle` to deal with. Only the local store uses one: the
    /// object store has nothing outstanding by the time a write returns.
    pub(crate) staged: Option<PathBuf>,
}

/// Content-addressed bytes. The name a blob is stored under is derived from its contents, so a
/// write is idempotent and two callers uploading the same file cannot disagree about where it went.
#[async_trait]
pub trait BlobStore: Send + Sync {
    /// Consumes the stream, hashes it, and puts the bytes where `read` will find them. The blob is
    /// durable when this returns, so a caller that fails afterwards leaves an orphan for the sweep
    /// rather than a node pointing at nothing.
    async fn write(&self, chunks: Upload) -> Result<Written, StorageError>;

    /// Clears up whatever the write staged. Separate from `write` because the caller commits its
    /// own rows in between, and a blob that is already in place must survive that failing.
    async fn settle(&self, written: &Written) -> Result<(), StorageError>;

    async fn read(&self, hash: BlobHash) -> Result<Reader, StorageError>;

    /// Absent is not an error: the sweep removes what nothing points at, and losing a race with
    /// another sweeper is not a failure.
    async fn remove(&self, hash: BlobHash) -> Result<(), StorageError>;

    /// Whether the stored bytes were written recently enough that the sweep should leave them
    /// alone. A delete followed by a re-upload of the same content adopts the existing blob, and
    /// this is what stops the collector removing it in between.
    async fn written_within(&self, hash: BlobHash, grace: Duration) -> bool;

    /// Clears what an upload left staged and nobody came back for. A caller does its own work
    /// between `write` and `settle` and can fail there, and a process can be killed mid-upload, so
    /// the staging area needs an owner that is not the request that filled it.
    ///
    /// Answers how much it removed.
    async fn sweep_staged(&self, grace: Duration) -> Result<u64, StorageError>;
}

/// Whether bytes stamped `at` are young enough that the sweep should leave them alone.
///
/// A stamp the clock has not reached yet counts as recent. The object store and the API are two
/// machines that rarely agree to the second, and a collector that reads a small disagreement as
/// "old enough to delete" deletes something somebody has just uploaded.
pub(crate) fn is_recent(at: SystemTime, grace: Duration) -> bool {
    match at.elapsed() {
        Ok(since) => since < grace,
        Err(_) => true,
    }
}

/// Where a blob's bytes live under a store's own naming, sharded by the first two byte-pairs of the
/// digest so no directory or key prefix holds every blob in the deployment.
#[must_use]
pub fn shards(hash: BlobHash) -> [String; 3] {
    let hex = hash.to_hex();
    [
        hex[0..2].to_owned(),
        hex[2..4].to_owned(),
        hex[..].to_owned(),
    ]
}
