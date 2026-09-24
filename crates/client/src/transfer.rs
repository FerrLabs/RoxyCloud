use std::path::{Path, PathBuf};

use futures::StreamExt;
use reqwest::Body;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;

use crate::remote::{Remote, RemoteError, check};
use crate::sync::held::Held;
use crate::sync::path::RelPath;
use crate::sync::snapshot::{Entry, Snapshot};
use crate::sync::transport::Transport;
use roxycloud_core::grant::{Access, Received};
use roxycloud_core::node::{Node, NodeKind};
use roxycloud_core::user::User;

#[must_use]
pub fn free_path(directory: &Path, name: &str) -> PathBuf {
    let wanted = directory.join(name);
    if !wanted.exists() {
        return wanted;
    }
    let numbered = |n: u32| match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => format!("{stem} ({n}).{extension}"),
        _ => format!("{name} ({n})"),
    };
    (1..u32::MAX)
        .map(|n| directory.join(numbered(n)))
        .find(|candidate| !candidate.exists())
        .unwrap_or(wanted)
}

pub(crate) async fn save(
    response: reqwest::Response,
    destination: &Path,
) -> Result<(), RemoteError> {
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

    pub async fn me(&self) -> Result<User, RemoteError> {
        let url = format!("{}/v1/auth/me", self.base());
        let response = self
            .http()
            .get(&url)
            .bearer_auth(self.token())
            .send()
            .await?;
        check(response.status(), "the authenticated account")?;
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
        save(response, destination).await
    }

    pub async fn received(&self) -> Result<Vec<Received>, RemoteError> {
        self.grants_received().await
    }

    async fn grants_received<T: DeserializeOwned>(&self) -> Result<Vec<T>, RemoteError> {
        let url = format!("{}/v1/grants/received", self.base());
        let response = self
            .http()
            .get(&url)
            .bearer_auth(self.token())
            .send()
            .await?;
        check(response.status(), "the shares received")?;
        Ok(response.json().await?)
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

    async fn held(&self) -> Result<Held, Self::Error> {
        held_from(self.grants_received().await)
    }
}

#[derive(Debug, Deserialize)]
struct Mount {
    name: String,
    access: Access,
}

fn held_from(received: Result<Vec<Mount>, RemoteError>) -> Result<Held, RemoteError> {
    match received {
        Ok(mounts) => Ok(Held::from_mounts(
            mounts.into_iter().map(|mount| (mount.name, mount.access)),
        )),
        Err(RemoteError::NotFound(_)) => Ok(Held::default()),
        Err(other) => Err(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("roxycloud-free-{name}"));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("scratch directory");
        directory
    }

    fn touch(directory: &Path, name: &str) {
        std::fs::write(directory.join(name), b"already here").expect("writing a file");
    }

    #[test]
    fn a_name_nothing_holds_is_used_as_it_is() {
        let directory = scratch("unused");
        assert_eq!(free_path(&directory, "a.txt"), directory.join("a.txt"));
    }

    #[test]
    fn a_taken_name_gets_the_first_free_number_before_its_extension() {
        let directory = scratch("taken");
        touch(&directory, "report.pdf");
        touch(&directory, "report (1).pdf");
        assert_eq!(
            free_path(&directory, "report.pdf"),
            directory.join("report (2).pdf"),
            "a download never overwrites what is already in the folder"
        );
    }

    #[test]
    fn a_name_with_no_extension_or_only_a_leading_dot_is_numbered_at_the_end() {
        let directory = scratch("bare");
        touch(&directory, "Makefile");
        touch(&directory, ".env");
        assert_eq!(
            free_path(&directory, "Makefile"),
            directory.join("Makefile (1)")
        );
        assert_eq!(free_path(&directory, ".env"), directory.join(".env (1)"));
    }

    #[test]
    fn a_server_older_than_shares_holds_nothing_back() {
        let held = held_from(Err(RemoteError::NotFound("the shares received".to_owned())))
            .expect("an older server is not an error");
        assert_eq!(held, Held::default());
    }

    #[test]
    fn any_other_failure_still_stops_the_sync() {
        assert!(held_from(Err(RemoteError::Unauthenticated)).is_err());
    }

    #[test]
    fn sync_reads_a_received_share_from_its_name_and_access_alone() {
        let mounts: Vec<Mount> = serde_json::from_str(r#"[{"name":"archive","access":"read"}]"#)
            .expect("a server that sends less than the full row still syncs");
        assert_eq!(mounts[0].name, "archive");
        assert_eq!(mounts[0].access, Access::Read);
    }

    #[test]
    fn received_shares_become_what_is_held() {
        let held = held_from(Ok(vec![Mount {
            name: "archive".to_owned(),
            access: Access::Read,
        }]))
        .expect("held");
        assert_eq!(
            held,
            Held::from_mounts([("archive".to_owned(), Access::Read)])
        );
    }
}
