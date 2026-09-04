use std::path::{Path, PathBuf};

use bytes::Bytes;
use futures::{Stream, StreamExt};
use tokio::fs;
use tokio::io::AsyncWriteExt;

use roxycloud_core::blob::BlobHash;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("blob {0} is not in the store")]
    NotFound(BlobHash),
    #[error("reading the upload stream failed")]
    Upstream(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("blob store i/o failed")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct Written {
    pub hash: BlobHash,
    pub size: u64,
    pub deduplicated: bool,
    staged: Option<PathBuf>,
}

pub struct LocalBlobStore {
    root: PathBuf,
}

impl LocalBlobStore {
    pub async fn open(root: impl Into<PathBuf>) -> Result<Self, StorageError> {
        let root = root.into();
        fs::create_dir_all(root.join("tmp")).await?;
        Ok(Self { root })
    }

    #[must_use]
    pub fn path_for(&self, hash: BlobHash) -> PathBuf {
        let hex = hash.to_hex();
        self.root.join(&hex[0..2]).join(&hex[2..4]).join(&hex)
    }

    pub async fn write<S, E>(&self, mut chunks: S) -> Result<Written, StorageError>
    where
        S: Stream<Item = Result<Bytes, E>> + Unpin,
        E: std::error::Error + Send + Sync + 'static,
    {
        let staged = self.root.join("tmp").join(uuid::Uuid::now_v7().to_string());
        let mut file = fs::File::create(&staged).await?;
        let mut hasher = blake3::Hasher::new();
        let mut size = 0u64;

        let outcome = async {
            while let Some(chunk) = chunks.next().await {
                let chunk = chunk.map_err(|err| StorageError::Upstream(Box::new(err)))?;
                hasher.update(&chunk);
                size += chunk.len() as u64;
                file.write_all(&chunk).await?;
            }
            file.sync_all().await?;
            Ok::<_, StorageError>(())
        }
        .await;

        if let Err(err) = outcome {
            let _ = fs::remove_file(&staged).await;
            return Err(err);
        }

        let hash = BlobHash::from(hasher.finalize());
        let destination = self.path_for(hash);

        if fs::try_exists(&destination).await? {
            return Ok(Written {
                hash,
                size,
                deduplicated: true,
                staged: Some(staged),
            });
        }

        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).await?;
        }
        rename_or_discard(&staged, &destination).await?;

        Ok(Written {
            hash,
            size,
            deduplicated: false,
            staged: None,
        })
    }

    pub async fn settle(&self, written: &Written) -> Result<(), StorageError> {
        let Some(staged) = &written.staged else {
            return Ok(());
        };

        let destination = self.path_for(written.hash);
        if fs::try_exists(&destination).await? {
            let _ = fs::remove_file(staged).await;
            return Ok(());
        }

        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).await?;
        }
        rename_or_discard(staged, &destination).await
    }

    pub async fn read(&self, hash: BlobHash) -> Result<fs::File, StorageError> {
        match fs::File::open(self.path_for(hash)).await {
            Ok(file) => Ok(file),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                Err(StorageError::NotFound(hash))
            }
            Err(err) => Err(err.into()),
        }
    }

    pub async fn remove(&self, hash: BlobHash) -> Result<(), StorageError> {
        match fs::remove_file(self.path_for(hash)).await {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err.into()),
        }
    }
}

async fn rename_or_discard(staged: &Path, destination: &Path) -> Result<(), StorageError> {
    match fs::rename(staged, destination).await {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = fs::remove_file(staged).await;
            if fs::try_exists(destination).await.unwrap_or(false) {
                return Ok(());
            }
            Err(err.into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    fn stream_of(parts: &[&'static [u8]]) -> impl Stream<Item = Result<Bytes, std::io::Error>> {
        futures::stream::iter(
            parts
                .iter()
                .map(|part| Ok(Bytes::from_static(part)))
                .collect::<Vec<_>>(),
        )
    }

    async fn store() -> (LocalBlobStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = LocalBlobStore::open(dir.path()).await.expect("store opens");
        (store, dir)
    }

    #[tokio::test]
    async fn write_hashes_the_whole_stream_not_just_the_first_chunk() {
        let (store, _dir) = store().await;
        let written = store
            .write(stream_of(&[b"hello ", b"world"]))
            .await
            .expect("write");

        assert_eq!(written.hash, BlobHash::from(blake3::hash(b"hello world")));
        assert_eq!(written.size, 11);
        assert!(!written.deduplicated);
    }

    #[tokio::test]
    async fn identical_content_is_stored_once() {
        let (store, _dir) = store().await;
        let first = store.write(stream_of(&[b"same"])).await.expect("first");
        let second = store.write(stream_of(&[b"same"])).await.expect("second");

        assert_eq!(first.hash, second.hash);
        assert!(!first.deduplicated);
        assert!(second.deduplicated, "second write should dedupe");
        assert!(fs::try_exists(store.path_for(first.hash)).await.unwrap());
    }

    #[tokio::test]
    async fn read_returns_the_bytes_that_were_written() {
        let (store, _dir) = store().await;
        let written = store.write(stream_of(&[b"payload"])).await.expect("write");

        let mut buf = Vec::new();
        store
            .read(written.hash)
            .await
            .expect("read")
            .read_to_end(&mut buf)
            .await
            .expect("read to end");

        assert_eq!(buf, b"payload");
    }

    #[tokio::test]
    async fn reading_an_absent_blob_is_not_found_rather_than_io_error() {
        let (store, _dir) = store().await;
        let missing = BlobHash::from(blake3::hash(b"never written"));

        assert!(matches!(
            store.read(missing).await,
            Err(StorageError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn a_failing_stream_leaves_no_partial_blob_and_no_staged_file() {
        let (store, dir) = store().await;
        let failing = futures::stream::iter(vec![
            Ok(Bytes::from_static(b"good")),
            Err(std::io::Error::other("connection reset")),
        ]);

        assert!(matches!(
            store.write(failing).await,
            Err(StorageError::Upstream(_))
        ));

        let mut staged = fs::read_dir(dir.path().join("tmp")).await.expect("tmp dir");
        assert!(
            staged.next_entry().await.expect("entry").is_none(),
            "staged upload should be cleaned up"
        );
    }

    #[tokio::test]
    async fn removing_an_absent_blob_is_idempotent() {
        let (store, _dir) = store().await;
        let hash = BlobHash::from(blake3::hash(b"gone"));
        assert!(store.remove(hash).await.is_ok());
    }

    #[tokio::test]
    async fn blobs_are_sharded_by_digest_prefix() {
        let (store, dir) = store().await;
        let written = store.write(stream_of(&[b"shard me"])).await.expect("write");
        let hex = written.hash.to_hex();

        assert_eq!(
            store.path_for(written.hash),
            dir.path().join(&hex[0..2]).join(&hex[2..4]).join(&hex)
        );
    }
}
