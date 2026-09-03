use std::path::Path;

use futures::StreamExt;
use reqwest::Body;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;

use crate::remote::{Remote, RemoteError, check};
use crate::sync::path::RelPath;
use crate::sync::snapshot::{Entry, Snapshot};
use crate::sync::transport::Transport;
use roxycloud_core::node::{Node, NodeKind};

impl Remote {
    pub async fn upload(&self, path: &str, source: &Path) -> Result<Node, RemoteError> {
        let url = self.endpoint("files", path)?;
        let file = fs::File::open(source)
            .await
            .map_err(|source_error| RemoteError::io(source, source_error))?;

        let response = self
            .http()
            .put(&url)
            .bearer_auth(self.token())
            .body(Body::wrap_stream(ReaderStream::new(file)))
            .send()
            .await?;
        check(response.status(), path)?;
        Ok(response.json().await?)
    }

    pub async fn read(&self, path: &str) -> Result<bytes::Bytes, RemoteError> {
        let url = self.endpoint("files", path)?;
        let response = self
            .http()
            .get(&url)
            .bearer_auth(self.token())
            .send()
            .await?;
        check(response.status(), path)?;
        Ok(response.bytes().await?)
    }

    pub async fn download(&self, path: &str, destination: &Path) -> Result<(), RemoteError> {
        let url = self.endpoint("files", path)?;
        let response = self
            .http()
            .get(&url)
            .bearer_auth(self.token())
            .send()
            .await?;
        check(response.status(), path)?;

        let mut file = fs::File::create(destination)
            .await
            .map_err(|source| RemoteError::io(destination, source))?;
        let mut chunks = response.bytes_stream();
        while let Some(chunk) = chunks.next().await {
            file.write_all(&chunk?)
                .await
                .map_err(|source| RemoteError::io(destination, source))?;
        }
        file.flush()
            .await
            .map_err(|source| RemoteError::io(destination, source))
    }

    pub async fn walk(&self) -> Result<Snapshot, RemoteError> {
        let mut snapshot = Snapshot::new();
        let mut pending = vec![None];

        while let Some(directory) = pending.pop() {
            let listed = match &directory {
                Some(path) => self.list(RelPath::as_str(path)).await?,
                None => self.list("/").await?,
            };

            for node in listed {
                let Ok(path) = child_of(directory.as_ref(), &node.name) else {
                    continue;
                };
                match node.kind {
                    NodeKind::Directory => {
                        snapshot.insert(path.clone(), Entry::Directory);
                        pending.push(Some(path));
                    }
                    NodeKind::File => {
                        snapshot.insert(
                            path,
                            Entry::File {
                                etag: node.etag,
                                size: u64::try_from(node.size).unwrap_or_default(),
                            },
                        );
                    }
                }
            }
        }

        Ok(snapshot)
    }
}

fn child_of(
    directory: Option<&RelPath>,
    name: &str,
) -> Result<RelPath, crate::sync::path::InvalidRelPath> {
    match directory {
        Some(parent) => parent.child(name),
        None => RelPath::parse(name),
    }
}

impl Transport for Remote {
    type Error = RemoteError;

    async fn snapshot(&self) -> Result<Snapshot, Self::Error> {
        self.walk().await
    }

    async fn download_to(&self, path: &RelPath, destination: &Path) -> Result<(), Self::Error> {
        self.download(path.as_str(), destination).await
    }

    async fn upload_from(&self, path: &RelPath, source: &Path) -> Result<(), Self::Error> {
        self.upload(path.as_str(), source).await.map(|_| ())
    }

    async fn remove(&self, path: &RelPath) -> Result<(), Self::Error> {
        self.delete(path.as_str()).await
    }
}
