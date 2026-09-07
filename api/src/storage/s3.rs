use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use aws_sdk_s3::Client;
use aws_sdk_s3::error::{ProvideErrorMetadata, SdkError};
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{CompletedMultipartUpload, CompletedPart};
use bytes::{Bytes, BytesMut};
use futures::StreamExt;

use super::{BlobStore, Reader, StorageError, Upload, Written, is_recent, shards};
use roxycloud_core::blob::BlobHash;

/// S3 refuses a part below five mebibytes unless it is the last one, and ten thousand parts is the
/// ceiling. Eight gives a comfortable margin on the floor and still allows an eighty gigabyte
/// object, which is past anything a file server is asked to hold in one piece.
const PART_SIZE: usize = 8 * 1024 * 1024;

/// A single `CopyObject` tops out at five gibibytes. Past that the copy itself has to be done in
/// parts, which is the same dance as the upload and costs nothing to get right once.
const LARGEST_SINGLE_COPY: u64 = 5 * 1024 * 1024 * 1024;

pub struct S3BlobStore {
    client: Client,
    bucket: String,
    prefix: String,
}

impl S3BlobStore {
    #[must_use]
    pub fn new(client: Client, bucket: String, prefix: &str) -> Self {
        let prefix = match prefix {
            "" => String::new(),
            _ => format!("{}/", prefix.trim_end_matches('/')),
        };
        Self {
            client,
            bucket,
            prefix,
        }
    }

    #[must_use]
    pub fn key_for(&self, hash: BlobHash) -> String {
        let [first, second, name] = shards(hash);
        format!("{}{first}/{second}/{name}", self.prefix)
    }

    fn staging_key(&self) -> String {
        format!("{}tmp/{}", self.prefix, uuid::Uuid::now_v7())
    }

    async fn exists(&self, key: &str) -> Result<bool, StorageError> {
        match self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
        {
            Ok(_) => Ok(true),
            Err(err) if is_missing(&err) => Ok(false),
            Err(err) => Err(remote(&err)),
        }
    }

    async fn discard(&self, key: &str) {
        let _ = self
            .client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await;
    }

    /// Streams the upload into one staging object, hashing as it goes. A stream that fits in a
    /// single part is sent as one request rather than three, which is what most files are.
    async fn stage(&self, chunks: Upload, key: &str) -> Result<(BlobHash, u64), StorageError> {
        let mut upload: Option<Multipart> = None;
        match self.stream_into(chunks, key, &mut upload).await {
            Ok(digest) => Ok(digest),
            Err(err) => {
                // Every failure leaves through here, so there is no path that walks away from a
                // multipart upload and keeps paying for its parts.
                self.abandon(upload, key).await;
                Err(err)
            }
        }
    }

    async fn stream_into(
        &self,
        mut chunks: Upload,
        key: &str,
        upload: &mut Option<Multipart>,
    ) -> Result<(BlobHash, u64), StorageError> {
        let mut hasher = blake3::Hasher::new();
        let mut size = 0u64;
        let mut buffer = BytesMut::with_capacity(PART_SIZE);

        while let Some(chunk) = chunks.next().await {
            let chunk = chunk?;
            hasher.update(&chunk);
            size += chunk.len() as u64;
            buffer.extend_from_slice(&chunk);

            while buffer.len() >= PART_SIZE {
                let part = buffer.split_to(PART_SIZE).freeze();
                if upload.is_none() {
                    *upload = Some(self.begin(key).await?);
                }
                let started = upload.as_mut().expect("just begun");
                self.send_part(started, key, part).await?;
            }
        }

        let rest = buffer.freeze();
        match upload.take() {
            None => {
                self.client
                    .put_object()
                    .bucket(&self.bucket)
                    .key(key)
                    .body(ByteStream::from(rest))
                    .send()
                    .await
                    .map_err(|err| remote(&err))?;
            }
            Some(mut started) => {
                // A stream that divided exactly into parts has nothing left to send, and S3
                // refuses a zero-length part.
                let mut outcome = Ok(());
                if !rest.is_empty() {
                    outcome = self.send_part(&mut started, key, rest).await;
                }
                if outcome.is_ok() {
                    outcome = self.finish(&started, key).await;
                }
                if outcome.is_err() {
                    // Handed back, so `stage` still has something to abort.
                    *upload = Some(started);
                }
                outcome?;
            }
        }

        Ok((BlobHash::from(hasher.finalize()), size))
    }

    async fn begin(&self, key: &str) -> Result<Multipart, StorageError> {
        let started = self
            .client
            .create_multipart_upload()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|err| remote(&err))?;

        Ok(Multipart {
            id: started.upload_id().unwrap_or_default().to_owned(),
            parts: Vec::new(),
        })
    }

    async fn send_part(
        &self,
        upload: &mut Multipart,
        key: &str,
        part: Bytes,
    ) -> Result<(), StorageError> {
        let number = i32::try_from(upload.parts.len() + 1).unwrap_or(i32::MAX);
        let sent = self
            .client
            .upload_part()
            .bucket(&self.bucket)
            .key(key)
            .upload_id(&upload.id)
            .part_number(number)
            .body(ByteStream::from(part))
            .send()
            .await
            .map_err(|err| remote(&err))?;

        upload.parts.push(
            CompletedPart::builder()
                .part_number(number)
                .set_e_tag(sent.e_tag().map(ToOwned::to_owned))
                .build(),
        );
        Ok(())
    }

    async fn finish(&self, upload: &Multipart, key: &str) -> Result<(), StorageError> {
        self.client
            .complete_multipart_upload()
            .bucket(&self.bucket)
            .key(key)
            .upload_id(&upload.id)
            .multipart_upload(
                CompletedMultipartUpload::builder()
                    .set_parts(Some(upload.parts.clone()))
                    .build(),
            )
            .send()
            .await
            .map_err(|err| remote(&err))?;
        Ok(())
    }

    /// A multipart upload nobody completes keeps its parts, and keeps being billed for them, so a
    /// failed write aborts rather than walking away.
    async fn abandon(&self, upload: Option<Multipart>, key: &str) {
        if let Some(upload) = upload {
            let _ = self
                .client
                .abort_multipart_upload()
                .bucket(&self.bucket)
                .key(key)
                .upload_id(&upload.id)
                .send()
                .await;
        }
        self.discard(key).await;
    }

    /// Whether the blob was already there. The key is the digest, so one that is already there is
    /// the same bytes and nothing is overwritten.
    async fn place(&self, staging: &str, hash: BlobHash, size: u64) -> Result<bool, StorageError> {
        let destination = self.key_for(hash);
        if self.exists(&destination).await? {
            return Ok(true);
        }
        self.copy(staging, &destination, size).await?;
        Ok(false)
    }

    async fn copy(&self, from: &str, to: &str, size: u64) -> Result<(), StorageError> {
        if size > LARGEST_SINGLE_COPY {
            return self.copy_in_parts(from, to, size).await;
        }

        self.client
            .copy_object()
            .bucket(&self.bucket)
            .key(to)
            .copy_source(format!("{}/{from}", self.bucket))
            .send()
            .await
            .map_err(|err| remote(&err))?;
        Ok(())
    }

    async fn copy_in_parts(&self, from: &str, to: &str, size: u64) -> Result<(), StorageError> {
        let mut upload = self.begin(to).await?;
        match self.copy_parts(&mut upload, from, to, size).await {
            Ok(()) => Ok(()),
            Err(err) => {
                // Parts nobody completes are kept and charged for, the same as on the write path.
                // Only the upload is aborted, never the destination key: another writer may have
                // placed the same digest there while this copy was failing.
                let _ = self
                    .client
                    .abort_multipart_upload()
                    .bucket(&self.bucket)
                    .key(to)
                    .upload_id(&upload.id)
                    .send()
                    .await;
                Err(err)
            }
        }
    }

    async fn copy_parts(
        &self,
        upload: &mut Multipart,
        from: &str,
        to: &str,
        size: u64,
    ) -> Result<(), StorageError> {
        let span = LARGEST_SINGLE_COPY;
        let mut start = 0u64;

        while start < size {
            let end = (start + span).min(size) - 1;
            let number = i32::try_from(upload.parts.len() + 1).unwrap_or(i32::MAX);
            let copied = self
                .client
                .upload_part_copy()
                .bucket(&self.bucket)
                .key(to)
                .upload_id(&upload.id)
                .part_number(number)
                .copy_source(format!("{}/{from}", self.bucket))
                .copy_source_range(format!("bytes={start}-{end}"))
                .send()
                .await
                .map_err(|err| remote(&err))?;

            upload.parts.push(
                CompletedPart::builder()
                    .part_number(number)
                    .set_e_tag(
                        copied
                            .copy_part_result()
                            .and_then(|result| result.e_tag())
                            .map(ToOwned::to_owned),
                    )
                    .build(),
            );
            start = end + 1;
        }

        self.finish(upload, to).await
    }
}

struct Multipart {
    id: String,
    parts: Vec<CompletedPart>,
}

#[async_trait]
impl BlobStore for S3BlobStore {
    async fn write(&self, chunks: Upload) -> Result<Written, StorageError> {
        let staging = self.staging_key();
        let (hash, size) = self.stage(chunks, &staging).await?;

        let placed = self.place(&staging, hash, size).await;
        // Both arms clear the staging object. Holding it until `settle` would mean a caller that
        // fails in between pays for it for good, since nothing walks the staging prefix looking
        // for orphans, and a copy that failed would leave one nobody can even name.
        self.discard(&staging).await;

        Ok(Written {
            hash,
            size,
            deduplicated: placed?,
            staged: None,
        })
    }

    /// Nothing is outstanding once a write returns, so there is nothing here to settle. The local
    /// store is the one that leaves a temp file behind.
    async fn settle(&self, _written: &Written) -> Result<(), StorageError> {
        Ok(())
    }

    async fn read(&self, hash: BlobHash) -> Result<Reader, StorageError> {
        let object = match self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(self.key_for(hash))
            .send()
            .await
        {
            Ok(object) => object,
            Err(err) if is_missing(&err) => return Err(StorageError::NotFound(hash)),
            Err(err) => return Err(remote(&err)),
        };

        Ok(Box::pin(object.body.into_async_read()))
    }

    /// Unlike `discard`, a refusal here is reported. The sweep deletes the row that names the blob
    /// only once this succeeds, so swallowing the error would leave an object nothing names and no
    /// later sweep can find.
    async fn remove(&self, hash: BlobHash) -> Result<(), StorageError> {
        match self
            .client
            .delete_object()
            .bucket(&self.bucket)
            .key(self.key_for(hash))
            .send()
            .await
        {
            Ok(_) => Ok(()),
            Err(err) if is_missing(&err) => Ok(()),
            Err(err) => Err(remote(&err)),
        }
    }

    async fn written_within(&self, hash: BlobHash, grace: Duration) -> bool {
        let head = match self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(self.key_for(hash))
            .send()
            .await
        {
            Ok(head) => head,
            // Absent is nothing to protect. Anything else means the store did not answer, and a
            // collector that reads "I could not tell" as "safe to delete" deletes live bytes.
            Err(err) => return !is_missing(&err),
        };

        head.last_modified()
            .and_then(|at| SystemTime::try_from(*at).ok())
            .is_some_and(|at| is_recent(at, grace))
    }
}

/// A missing object and a missing bucket both arrive as a 404, and neither is a failure worth
/// propagating where the caller asked whether something is there.
fn is_missing<E: ProvideErrorMetadata, R>(err: &SdkError<E, R>) -> bool {
    err.code()
        .is_some_and(|code| matches!(code, "NoSuchKey" | "NotFound" | "NoSuchBucket"))
}

fn remote<E: ProvideErrorMetadata, R>(err: &SdkError<E, R>) -> StorageError {
    let detail = err
        .message()
        .or_else(|| err.code())
        .unwrap_or("the object store refused the request");
    StorageError::Remote(detail.to_owned())
}
