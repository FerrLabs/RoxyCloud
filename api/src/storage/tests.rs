use super::*;

use std::time::SystemTime;

use aws_sdk_s3::Client;
use tokio::io::AsyncReadExt;

pub(super) fn stream_of(parts: &[&'static [u8]]) -> Upload {
    upload(futures::stream::iter(
        parts
            .iter()
            .map(|part| Ok::<_, std::io::Error>(Bytes::from_static(part)))
            .collect::<Vec<_>>(),
    ))
}

pub(super) fn stream_of_size(bytes: usize) -> Upload {
    let chunk = Bytes::from(vec![b'x'; 64 * 1024]);
    let whole = std::iter::repeat_n(chunk.clone(), bytes / chunk.len())
        .chain(std::iter::once(chunk.slice(..bytes % chunk.len())))
        .filter(|part| !part.is_empty())
        .map(Ok::<_, std::io::Error>)
        .collect::<Vec<_>>();
    upload(futures::stream::iter(whole))
}

pub(super) async fn local_store() -> (LocalBlobStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = LocalBlobStore::open(dir.path()).await.expect("store opens");
    (store, dir)
}

pub(super) struct S3Fixture {
    pub store: S3BlobStore,
    client: Client,
    bucket: String,
    prefix: String,
}

impl S3Fixture {
    /// Every key the fixture's own prefix holds, so a test can say that nothing was left behind
    /// rather than only that the blob it asked for is present.
    pub async fn keys(&self) -> Vec<String> {
        self.client
            .list_objects_v2()
            .bucket(&self.bucket)
            .prefix(&self.prefix)
            .send()
            .await
            .expect("listing the bucket")
            .contents()
            .iter()
            .filter_map(|object| object.key().map(ToOwned::to_owned))
            .collect()
    }

    /// Multipart uploads nobody completed. These do not show up in a listing of objects, so a test
    /// that only looks at keys cannot see the parts a failed write left behind, and the storage
    /// bill can.
    ///
    /// The prefix is applied here rather than passed to the request, because `MinIO` answers a
    /// prefixed `ListMultipartUploads` with nothing at all, which makes the check quietly unable
    /// to fail.
    pub async fn unfinished_uploads(&self) -> usize {
        self.client
            .list_multipart_uploads()
            .bucket(&self.bucket)
            .send()
            .await
            .expect("listing the multipart uploads")
            .uploads()
            .iter()
            .filter(|upload| {
                upload
                    .key()
                    .is_some_and(|key| key.starts_with(&self.prefix))
            })
            .count()
    }
}

/// A `MinIO` or Garage endpoint if one is configured, and nothing otherwise, so the suite runs
/// unattended without an object store and covers both backends when there is one.
pub(super) async fn s3_store() -> Option<S3Fixture> {
    use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};

    let endpoint = std::env::var("S3_TEST_ENDPOINT")
        .ok()
        .filter(|e| !e.is_empty())?;
    let bucket = std::env::var("S3_TEST_BUCKET").unwrap_or_else(|_| "roxycloud-test".to_owned());
    let key = std::env::var("S3_TEST_ACCESS_KEY_ID").unwrap_or_else(|_| "minioadmin".to_owned());
    let secret =
        std::env::var("S3_TEST_SECRET_ACCESS_KEY").unwrap_or_else(|_| "minioadmin".to_owned());

    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new(key, secret, None, None, "roxycloud-test"))
        .endpoint_url(&endpoint)
        .force_path_style(true)
        .build();

    let client = Client::from_conf(config);
    let _ = client.create_bucket().bucket(&bucket).send().await;

    // Each test gets its own prefix, so they can run at once against one bucket and still say
    // what is and is not there.
    let prefix = format!("t{}", uuid::Uuid::now_v7().simple());
    Some(S3Fixture {
        store: S3BlobStore::new(client.clone(), bucket.clone(), &prefix),
        client,
        bucket,
        prefix: format!("{prefix}/"),
    })
}

/// One body, run against local disk and against an object store, because a backend that passes its
/// own tests and disagrees with the other one is the failure mode a trait invites.
macro_rules! both_backends {
    ($name:ident, $store:ident, $body:block) => {
        mod $name {
            use super::*;

            #[tokio::test]
            async fn on_local_disk() {
                let (owned, _dir) = local_store().await;
                let $store: &dyn BlobStore = &owned;
                $body
            }

            #[tokio::test]
            async fn on_an_object_store() {
                let Some(fixture) = s3_store().await else {
                    eprintln!(concat!(
                        "skipping ",
                        stringify!($name),
                        ": S3_TEST_ENDPOINT is not set"
                    ));
                    return;
                };
                let $store: &dyn BlobStore = &fixture.store;
                $body
            }
        }
    };
}

both_backends!(a_write_hashes_the_whole_stream, store, {
    let written = store
        .write(stream_of(&[b"hello ", b"world"]))
        .await
        .expect("write");

    assert_eq!(written.hash, BlobHash::from(blake3::hash(b"hello world")));
    assert_eq!(written.size, 11);
    assert!(!written.deduplicated);
});

both_backends!(identical_content_is_stored_once, store, {
    let first = store.write(stream_of(&[b"same"])).await.expect("first");
    store.settle(&first).await.expect("settle");
    let second = store.write(stream_of(&[b"same"])).await.expect("second");
    store.settle(&second).await.expect("settle");

    assert_eq!(first.hash, second.hash);
    assert!(!first.deduplicated);
    assert!(second.deduplicated, "the second write should dedupe");
});

both_backends!(a_read_returns_what_was_written, store, {
    let written = store.write(stream_of(&[b"payload"])).await.expect("write");
    store.settle(&written).await.expect("settle");

    let mut buf = Vec::new();
    store
        .read(written.hash)
        .await
        .expect("read")
        .read_to_end(&mut buf)
        .await
        .expect("read to end");

    assert_eq!(buf, b"payload");
});

both_backends!(reading_an_absent_blob_is_not_found, store, {
    let missing = BlobHash::from(blake3::hash(b"never written"));

    assert!(matches!(
        store.read(missing).await,
        Err(StorageError::NotFound(_))
    ));
});

both_backends!(removing_an_absent_blob_is_idempotent, store, {
    let hash = BlobHash::from(blake3::hash(b"gone"));
    assert!(store.remove(hash).await.is_ok());
});

both_backends!(a_removed_blob_is_gone, store, {
    let written = store
        .write(stream_of(&[b"transient"]))
        .await
        .expect("write");
    store.settle(&written).await.expect("settle");

    store.remove(written.hash).await.expect("remove");

    assert!(matches!(
        store.read(written.hash).await,
        Err(StorageError::NotFound(_))
    ));
});

both_backends!(a_fresh_blob_counts_as_recently_written, store, {
    let written = store.write(stream_of(&[b"just now"])).await.expect("write");
    store.settle(&written).await.expect("settle");

    assert!(
        store
            .written_within(written.hash, Duration::from_secs(3600))
            .await,
        "the sweep leans on this to not collect a blob somebody just re-uploaded"
    );
});

both_backends!(a_blob_nobody_wrote_was_not_written_recently, store, {
    let missing = BlobHash::from(blake3::hash(b"never written"));
    assert!(
        !store
            .written_within(missing, Duration::from_secs(3600))
            .await
    );
});

both_backends!(a_failing_stream_stores_nothing, store, {
    let failing = upload(futures::stream::iter(vec![
        Ok(Bytes::from_static(b"good")),
        Err(std::io::Error::other("connection reset")),
    ]));

    assert!(matches!(
        store.write(failing).await,
        Err(StorageError::Upstream(_))
    ));

    let would_have_been = BlobHash::from(blake3::hash(b"good"));
    assert!(matches!(
        store.read(would_have_been).await,
        Err(StorageError::NotFound(_))
    ));
});

both_backends!(
    a_payload_larger_than_one_part_survives_the_round_trip,
    store,
    {
        // Past the eight mebibyte part size, so the object store takes the multipart path and the
        // local one is unaffected by the same input.
        let size = 9 * 1024 * 1024;
        let written = store.write(stream_of_size(size)).await.expect("write");
        store.settle(&written).await.expect("settle");

        assert_eq!(written.size, size as u64);

        let mut buf = Vec::new();
        store
            .read(written.hash)
            .await
            .expect("read")
            .read_to_end(&mut buf)
            .await
            .expect("read to end");

        assert_eq!(buf.len(), size);
        assert_eq!(BlobHash::from(blake3::hash(&buf)), written.hash);
    }
);

#[test]
fn a_stamp_the_clock_has_not_reached_counts_as_recent() {
    let grace = Duration::from_secs(60);
    let now = SystemTime::now();

    assert!(is_recent(now, grace));
    assert!(!is_recent(now - Duration::from_secs(120), grace));
    assert!(
        is_recent(now + Duration::from_secs(5), grace),
        "an object store two seconds ahead of us must not make a fresh blob look collectable"
    );
}

#[tokio::test]
async fn a_blob_older_than_the_grace_is_no_longer_recent() {
    let (store, _dir) = local_store().await;
    let written = store.write(stream_of(&[b"ageing"])).await.expect("write");

    std::fs::File::options()
        .write(true)
        .open(store.path_for(written.hash))
        .expect("opening the blob")
        .set_modified(SystemTime::now() - Duration::from_secs(600))
        .expect("ageing the blob");

    assert!(
        !store
            .written_within(written.hash, Duration::from_secs(60))
            .await
    );
    assert!(
        store
            .written_within(written.hash, Duration::from_secs(3600))
            .await
    );
}

#[tokio::test]
async fn blobs_are_sharded_by_digest_prefix() {
    let (store, dir) = local_store().await;
    let written = store.write(stream_of(&[b"shard me"])).await.expect("write");
    let hex = written.hash.to_hex();

    assert_eq!(
        store.path_for(written.hash),
        dir.path().join(&hex[0..2]).join(&hex[2..4]).join(&hex)
    );
}

#[tokio::test]
async fn a_local_failure_leaves_nothing_staged() {
    let (store, dir) = local_store().await;
    let failing = upload(futures::stream::iter(vec![
        Ok(Bytes::from_static(b"good")),
        Err(std::io::Error::other("connection reset")),
    ]));

    assert!(store.write(failing).await.is_err());

    let mut staged = tokio::fs::read_dir(dir.path().join("tmp"))
        .await
        .expect("tmp dir");
    assert!(
        staged.next_entry().await.expect("entry").is_none(),
        "the staged upload should be cleaned up"
    );
}

#[tokio::test]
async fn an_object_store_keeps_only_the_blob_once_a_write_returns() {
    let Some(fixture) = s3_store().await else {
        eprintln!("skipping: S3_TEST_ENDPOINT is not set");
        return;
    };

    let written = fixture
        .store
        .write(stream_of(&[b"staged then placed"]))
        .await
        .expect("write");

    // Deliberately before `settle`. A caller does its own database work in between and can fail
    // there, so a staging object that only a later call clears up is one nobody ever clears up.
    assert_eq!(
        fixture.keys().await,
        vec![fixture.store.key_for(written.hash)],
        "the staging object has to go, or every upload that is not followed through is billed for good"
    );

    fixture.store.settle(&written).await.expect("settle");
    assert_eq!(fixture.keys().await.len(), 1);
}

#[tokio::test]
async fn an_object_store_keeps_nothing_when_a_write_dedupes() {
    let Some(fixture) = s3_store().await else {
        eprintln!("skipping: S3_TEST_ENDPOINT is not set");
        return;
    };

    let first = fixture
        .store
        .write(stream_of(&[b"twice"]))
        .await
        .expect("first");
    let second = fixture
        .store
        .write(stream_of(&[b"twice"]))
        .await
        .expect("second");

    assert!(second.deduplicated);
    assert_eq!(
        fixture.keys().await,
        vec![fixture.store.key_for(first.hash)],
        "the second upload staged its own copy and has to clear it up too"
    );
}

#[tokio::test]
async fn an_object_store_leaves_nothing_behind_when_a_stream_fails() {
    let Some(fixture) = s3_store().await else {
        eprintln!("skipping: S3_TEST_ENDPOINT is not set");
        return;
    };

    let failing = upload(futures::stream::iter(vec![
        Ok(Bytes::from(vec![b'x'; 9 * 1024 * 1024])),
        Err(std::io::Error::other("connection reset")),
    ]));

    assert!(fixture.store.write(failing).await.is_err());

    assert!(fixture.keys().await.is_empty(), "no object should remain");
    assert_eq!(
        fixture.unfinished_uploads().await,
        0,
        "a multipart upload nobody aborts keeps its parts, and keeps being charged for them"
    );
}
