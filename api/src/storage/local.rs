use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use tokio::fs;
use tokio::io::AsyncWriteExt;

use super::{BlobStore, Reader, Staged, StorageError, Upload, Written, is_recent, shards};
use roxycloud_core::blob::BlobHash;

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
        let [first, second, name] = shards(hash);
        self.root.join(first).join(second).join(name)
    }
}

#[async_trait]
impl BlobStore for LocalBlobStore {
    async fn write(&self, mut chunks: Upload) -> Result<Written, StorageError> {
        let staged = self.root.join("tmp").join(uuid::Uuid::now_v7().to_string());
        let mut file = fs::File::create(&staged).await?;
        let mut hasher = blake3::Hasher::new();
        let mut size = 0u64;

        let outcome = async {
            while let Some(chunk) = chunks.next().await {
                let chunk = chunk?;
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
                staged: Some(Staged::File(staged)),
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

    async fn settle(&self, written: &Written) -> Result<(), StorageError> {
        let Some(Staged::File(staged)) = &written.staged else {
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

    async fn read(&self, hash: BlobHash) -> Result<Reader, StorageError> {
        match fs::File::open(self.path_for(hash)).await {
            Ok(file) => Ok(Box::pin(file)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                Err(StorageError::NotFound(hash))
            }
            Err(err) => Err(err.into()),
        }
    }

    async fn remove(&self, hash: BlobHash) -> Result<(), StorageError> {
        match fs::remove_file(self.path_for(hash)).await {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    async fn written_within(&self, hash: BlobHash, grace: Duration) -> bool {
        fs::metadata(self.path_for(hash))
            .await
            .and_then(|meta| meta.modified())
            .is_ok_and(|at| is_recent(at, grace))
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
