use std::io::SeekFrom;
use std::path::Path;
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::header::{HeaderName, IF_MATCH, IF_NONE_MATCH};
use serde::Deserialize;
use stashden_core::node::Node;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use uuid::Uuid;

use crate::remote::{Authorize, Remote, RemoteError, answered, check};
use crate::sync::transport::Expect;

pub(crate) const CHUNK: u64 = 8 * 1024 * 1024;

const ATTEMPTS: u32 = 5;
const FIRST_WAIT: Duration = Duration::from_millis(500);
const OFFSET: HeaderName = HeaderName::from_static("upload-offset");

#[derive(Debug, Deserialize)]
struct Opened {
    id: Uuid,
    received: i64,
}

enum Chunk {
    Landed(u64),
    Retry(RemoteError),
    Fatal(RemoteError),
}

impl Remote {
    pub async fn upload_chunked(
        &self,
        path: &str,
        source: &Path,
        expect: Option<&Expect>,
        chunk: u64,
    ) -> Result<Node, RemoteError> {
        let chunk = if chunk == 0 { CHUNK } else { chunk };
        let size = fs::metadata(source)
            .await
            .map_err(|error| RemoteError::io(source, error))?
            .len();
        if size <= chunk {
            return self.put_file(path, source, expect).await;
        }

        let session = self.open_upload(path, size, expect).await?;
        let finished = match self.fill_upload(&session, source, size, chunk).await {
            Ok(()) => self.finish_upload(session.id, expect).await,
            Err(error) => Err(error),
        };
        if finished.is_err() {
            self.abandon_upload(session.id).await;
        }
        finished
    }

    async fn open_upload(
        &self,
        path: &str,
        size: u64,
        expect: Option<&Expect>,
    ) -> Result<Opened, RemoteError> {
        let request = self
            .http()
            .post(format!("{}/v1/uploads", self.base()))
            .authorized(self)
            .json(&serde_json::json!({ "path": path, "size": size }));
        let response = conditional(request, expect).send().await?;
        Ok(answered(response, path).await?.json().await?)
    }

    async fn fill_upload(
        &self,
        session: &Opened,
        source: &Path,
        size: u64,
        chunk: u64,
    ) -> Result<(), RemoteError> {
        let mut file = fs::File::open(source)
            .await
            .map_err(|error| RemoteError::io(source, error))?;
        let mut received = u64::try_from(session.received).unwrap_or(0);
        let mut failures = 0u32;
        let mut wait = FIRST_WAIT;

        while received < size {
            let take = chunk.min(size - received);
            match self
                .send_chunk(session.id, &mut file, source, received, take)
                .await
            {
                Chunk::Landed(next) => {
                    received = next;
                    failures = 0;
                    wait = FIRST_WAIT;
                }
                Chunk::Fatal(error) => return Err(error),
                Chunk::Retry(error) => {
                    failures += 1;
                    if failures > ATTEMPTS {
                        return Err(error);
                    }
                    tokio::time::sleep(wait).await;
                    wait *= 2;
                    if let Some(at) = self.upload_offset(session.id).await {
                        received = at;
                    }
                }
            }
        }
        Ok(())
    }

    async fn send_chunk(
        &self,
        id: Uuid,
        file: &mut fs::File,
        source: &Path,
        at: u64,
        take: u64,
    ) -> Chunk {
        let mut bytes = vec![0u8; usize::try_from(take).unwrap_or(0)];
        if let Err(error) = read_at(file, at, &mut bytes).await {
            return Chunk::Fatal(RemoteError::io(source, error));
        }

        let sent = self
            .http()
            .patch(format!("{}/v1/uploads/{id}", self.base()))
            .authorized(self)
            .header(OFFSET, at)
            .body(bytes)
            .send()
            .await;
        let response = match sent {
            Ok(response) => response,
            Err(error) => return Chunk::Retry(error.into()),
        };

        let status = response.status();
        if status == StatusCode::CONFLICT {
            let expected = response
                .headers()
                .get(OFFSET)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok());
            return match expected {
                Some(expected) if expected != at => Chunk::Landed(expected),
                _ => Chunk::Retry(RemoteError::Conflict("the upload".to_owned())),
            };
        }
        if status.is_server_error() {
            return Chunk::Retry(RemoteError::Status(status));
        }
        if let Err(error) = check(status, "the upload") {
            return Chunk::Fatal(error);
        }
        match response.json::<Opened>().await {
            Ok(opened) => Chunk::Landed(u64::try_from(opened.received).unwrap_or(0)),
            Err(error) => Chunk::Retry(error.into()),
        }
    }

    async fn upload_offset(&self, id: Uuid) -> Option<u64> {
        let response = self
            .http()
            .get(format!("{}/v1/uploads/{id}", self.base()))
            .authorized(self)
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let opened = response.json::<Opened>().await.ok()?;
        u64::try_from(opened.received).ok()
    }

    async fn finish_upload(&self, id: Uuid, expect: Option<&Expect>) -> Result<Node, RemoteError> {
        let request = self
            .http()
            .post(format!("{}/v1/uploads/{id}/finish", self.base()))
            .authorized(self);
        let response = conditional(request, expect).send().await?;
        Ok(answered(response, "the upload").await?.json().await?)
    }

    async fn abandon_upload(&self, id: Uuid) {
        let _ = self
            .http()
            .delete(format!("{}/v1/uploads/{id}", self.base()))
            .authorized(self)
            .send()
            .await;
    }
}

fn conditional(
    request: reqwest::RequestBuilder,
    expect: Option<&Expect>,
) -> reqwest::RequestBuilder {
    match expect {
        None => request,
        Some(Expect::Absent) => request.header(IF_NONE_MATCH, "*"),
        Some(Expect::Etag(etag)) => request.header(IF_MATCH, etag),
    }
}

async fn read_at(file: &mut fs::File, at: u64, into: &mut [u8]) -> std::io::Result<()> {
    file.seek(SeekFrom::Start(at)).await?;
    file.read_exact(into).await?;
    Ok(())
}
