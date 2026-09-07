// Each integration binary compiles this module on its own, so a helper used by one
// test file reads as dead code in the others.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;

use roxycloud_api::auth::Sessions;
use roxycloud_api::state::AppState;
use roxycloud_api::storage::LocalBlobStore;
use roxycloud_api::{db, trash, users};
use roxycloud_core::blob::BlobHash;
use roxycloud_core::node::Node;
use roxycloud_core::role::Role;
use roxycloud_core::user::{Email, User};
use sqlx::{AssertSqlSafe, Executor, PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

pub const PASSWORD: &str = "twelve-characters-at-least";

pub struct Harness {
    pub state: AppState,
    pub blob_root: PathBuf,
    pub upload_root: PathBuf,
    /// The same store the state holds, kept concrete so a test can look at the files on disk.
    local: Arc<LocalBlobStore>,
    maintenance: PgPool,
    database: String,
}

impl Harness {
    pub async fn start() -> Option<Self> {
        let url = std::env::var("DATABASE_URL").ok()?;

        let maintenance = PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .expect("connecting to DATABASE_URL");

        let database = format!("roxy_test_{}", Uuid::now_v7().simple());
        maintenance
            .execute(AssertSqlSafe(format!("CREATE DATABASE \"{database}\"")))
            .await
            .expect("creating the test database");

        let db = PgPoolOptions::new()
            .max_connections(5)
            .connect(&with_database(&url, &database))
            .await
            .expect("connecting to the test database");

        sqlx::migrate!("./migrations")
            .run(&db)
            .await
            .expect("running the migrations");

        let blob_root = std::env::temp_dir().join(format!("roxy-blobs-{database}"));
        let upload_root = std::env::temp_dir().join(format!("roxy-uploads-{database}"));
        let blobs = Arc::new(
            LocalBlobStore::open(&blob_root)
                .await
                .expect("opening the blob store"),
        );

        Some(Self {
            state: AppState {
                db,
                blobs: blobs.clone(),
                staging: Arc::new(
                    roxycloud_api::uploads::Staging::open(&upload_root)
                        .await
                        .expect("opening the upload staging"),
                ),
                sessions: Arc::new(Sessions::new("test-secret", chrono::Duration::hours(1))),
                decoders: Arc::new(tokio::sync::Semaphore::new(2)),
                default_quota_bytes: 1_000_000,
            },
            blob_root,
            upload_root,
            local: blobs,
            maintenance,
            database,
        })
    }

    pub async fn stop(self) {
        self.state.db.close().await;
        let _ = tokio::fs::remove_dir_all(&self.blob_root).await;
        let _ = tokio::fs::remove_dir_all(&self.upload_root).await;
        let _ = self
            .maintenance
            .execute(AssertSqlSafe(format!(
                "DROP DATABASE IF EXISTS \"{}\" WITH (FORCE)",
                self.database
            )))
            .await;
    }

    pub async fn account(&self, email: &str, role: Role) -> User {
        let mut tx = self.state.db.begin().await.expect("begin");
        let user = users::create(
            &mut tx,
            &email.parse::<Email>().expect("valid email"),
            "Tester",
            PASSWORD,
            role,
        )
        .await
        .expect("creating the account");
        tx.commit().await.expect("commit");
        user
    }

    pub async fn root(&self, owner: Uuid) -> Node {
        let mut tx = self.state.db.begin().await.expect("begin");
        let root = db::ensure_root(&mut tx, owner, self.state.default_quota_bytes)
            .await
            .expect("ensuring the root");
        tx.commit().await.expect("commit");
        root
    }

    pub async fn write(&self, owner: Uuid, path: &str, contents: &[u8]) -> Node {
        self.try_write(owner, path, contents)
            .await
            .expect("writing the file")
    }

    pub async fn try_write(
        &self,
        owner: Uuid,
        path: &str,
        contents: &[u8],
    ) -> Result<Node, roxycloud_api::error::ApiError> {
        let mut segments = roxycloud_core::name::parse_path(path).expect("valid path");
        let name = segments.pop().expect("a file name");
        let written = self
            .state
            .blobs
            .write(roxycloud_api::storage::upload(futures::stream::iter([
                Ok::<_, std::io::Error>(bytes::Bytes::copy_from_slice(contents)),
            ])))
            .await
            .expect("writing the blob");

        let mut tx = self.state.db.begin().await.expect("begin");
        let root = db::ensure_root(&mut tx, owner, self.state.default_quota_bytes).await?;
        let parent = db::create_directories(&mut tx, owner, &root, &segments).await?;
        let node = db::put_file(
            &mut tx,
            owner,
            &parent,
            &name,
            written.hash,
            i64::try_from(written.size).expect("small test payload"),
        )
        .await?;
        tx.commit().await.expect("commit");
        self.state.blobs.settle(&written).await.expect("settling");
        Ok(node)
    }

    pub async fn stage(&self, contents: &[u8]) -> (BlobHash, i64) {
        let written = self
            .state
            .blobs
            .write(roxycloud_api::storage::upload(futures::stream::iter([
                Ok::<_, std::io::Error>(bytes::Bytes::copy_from_slice(contents)),
            ])))
            .await
            .expect("writing the blob");
        (
            written.hash,
            i64::try_from(written.size).expect("small test payload"),
        )
    }

    pub async fn stage_kept(&self, contents: &[u8]) -> roxycloud_api::storage::Written {
        self.state
            .blobs
            .write(roxycloud_api::storage::upload(futures::stream::iter([
                Ok::<_, std::io::Error>(bytes::Bytes::copy_from_slice(contents)),
            ])))
            .await
            .expect("writing the blob")
    }

    pub async fn finish_write(
        &self,
        owner: Uuid,
        path: &str,
        written: &roxycloud_api::storage::Written,
    ) -> Node {
        let mut segments = roxycloud_core::name::parse_path(path).expect("valid path");
        let name = segments.pop().expect("a file name");

        let mut tx = self.state.db.begin().await.expect("begin");
        let root = db::ensure_root(&mut tx, owner, self.state.default_quota_bytes)
            .await
            .expect("root");
        let parent = db::create_directories(&mut tx, owner, &root, &segments)
            .await
            .expect("directories");
        let node = db::put_file(
            &mut tx,
            owner,
            &parent,
            &name,
            written.hash,
            i64::try_from(written.size).expect("small test payload"),
        )
        .await
        .expect("writing the file");
        tx.commit().await.expect("commit");
        self.state.blobs.settle(written).await.expect("settling");
        node
    }

    pub async fn wait_until_blocked_on(&self, statement: &str) {
        for _ in 0..500 {
            let waiting = sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM pg_stat_activity
                 WHERE datname = current_database()
                   AND wait_event_type = 'Lock'
                   AND query ILIKE '%' || $1 || '%'",
            )
            .bind(statement)
            .fetch_one(&self.state.db)
            .await
            .expect("reading pg_stat_activity");
            if waiting > 0 {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("nothing ever blocked on {statement}, so the race under test never happened");
    }

    pub async fn resolve(&self, owner: Uuid, path: &str) -> Node {
        let segments = roxycloud_core::name::parse_path(path).expect("valid path");
        let mut tx = self.state.db.begin().await.expect("begin");
        let root = db::ensure_root(&mut tx, owner, self.state.default_quota_bytes)
            .await
            .expect("ensuring the root");
        let node = db::resolve(&mut tx, &root, &segments)
            .await
            .expect("resolving the path");
        tx.commit().await.expect("commit");
        node
    }

    pub async fn rename(&self, owner: Uuid, from: &str, to: &str) -> Node {
        self.try_rename(owner, from, to)
            .await
            .expect("renaming the node")
    }

    pub async fn try_rename(
        &self,
        owner: Uuid,
        from: &str,
        to: &str,
    ) -> Result<Node, roxycloud_api::error::ApiError> {
        let source = roxycloud_core::name::parse_path(from).expect("valid path");
        let mut destination = roxycloud_core::name::parse_path(to).expect("valid path");
        let name = destination.pop().expect("a destination name");

        let mut tx = self.state.db.begin().await.expect("begin");
        let root = db::ensure_root(&mut tx, owner, self.state.default_quota_bytes).await?;
        let node = db::resolve(&mut tx, &root, &source).await?;
        let parent = db::resolve(&mut tx, &root, &destination).await?;
        let renamed = db::rename(&mut tx, &node, &parent, &name).await?;
        tx.commit().await.expect("commit");
        Ok(renamed)
    }

    pub async fn trash(&self, node: &Node) {
        let mut tx = self.state.db.begin().await.expect("begin");
        trash::send(&mut tx, node).await.expect("trashing");
        tx.commit().await.expect("commit");
    }

    pub async fn children(&self, parent: &Node) -> Vec<String> {
        db::list_children(&self.state.db, parent.id)
            .await
            .expect("listing")
            .into_iter()
            .map(|node| node.name)
            .collect()
    }

    pub async fn restore(&self, owner: Uuid, id: Uuid) -> Node {
        self.try_restore(owner, id).await.expect("restoring")
    }

    pub async fn try_restore(
        &self,
        owner: Uuid,
        id: Uuid,
    ) -> Result<Node, roxycloud_api::error::ApiError> {
        let mut tx = self.state.db.begin().await.expect("begin");
        let restored = trash::restore(&mut tx, owner, id).await?;
        tx.commit().await.expect("commit");
        Ok(restored)
    }

    pub async fn purge(&self, owner: Uuid, id: Uuid) {
        let mut tx = self.state.db.begin().await.expect("begin");
        trash::purge(&mut tx, owner, id).await.expect("purging");
        tx.commit().await.expect("commit");
    }

    pub async fn resolve_trashed(&self, owner: Uuid, name: &str) -> Uuid {
        trash::list(&self.state.db, owner)
            .await
            .expect("listing the trash")
            .into_iter()
            .find(|node| node.name == name)
            .expect("a trash entry with that name")
            .id
    }

    pub async fn trashed(&self, owner: Uuid) -> Vec<String> {
        trash::list(&self.state.db, owner)
            .await
            .expect("listing the trash")
            .into_iter()
            .map(|node| node.name)
            .collect()
    }

    pub async fn live_nodes(&self, owner: Uuid) -> i64 {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM nodes
             WHERE owner_id = $1 AND parent_id IS NOT NULL AND deleted_at IS NULL",
        )
        .bind(owner)
        .fetch_one(&self.state.db)
        .await
        .expect("counting the live nodes")
    }

    pub async fn used_bytes(&self, owner: Uuid) -> i64 {
        sqlx::query_scalar::<_, i64>("SELECT bytes_used FROM quotas WHERE owner_id = $1")
            .bind(owner)
            .fetch_one(&self.state.db)
            .await
            .expect("reading the quota")
    }

    pub async fn set_quota(&self, owner: Uuid, bytes_max: i64) {
        sqlx::query("UPDATE quotas SET bytes_max = $2 WHERE owner_id = $1")
            .bind(owner)
            .bind(bytes_max)
            .execute(&self.state.db)
            .await
            .expect("setting the quota");
    }

    pub async fn blob_file_exists(&self, hash: BlobHash) -> bool {
        tokio::fs::try_exists(self.local.path_for(hash))
            .await
            .unwrap_or(false)
    }

    pub async fn age_blob(&self, hash: BlobHash, by: std::time::Duration) {
        sqlx::query("UPDATE blobs SET unreferenced_since = now() - $2::INTERVAL WHERE hash = $1")
            .bind(hash)
            .bind(sqlx::postgres::types::PgInterval {
                months: 0,
                days: 0,
                microseconds: i64::try_from(by.as_micros()).expect("a small test interval"),
            })
            .execute(&self.state.db)
            .await
            .expect("ageing the blob");
    }

    /// Moves a node under a parent the API would never allow, to check that a query defends itself
    /// rather than trusting the tree to be intact.
    pub async fn graft(&self, node: Uuid, parent: Uuid) {
        sqlx::query("UPDATE nodes SET parent_id = $2 WHERE id = $1")
            .bind(node)
            .bind(parent)
            .execute(&self.state.db)
            .await
            .expect("grafting the node");
    }

    pub async fn blob_references(&self, hash: BlobHash) -> i64 {
        sqlx::query_scalar::<_, i64>("SELECT ref_count FROM blobs WHERE hash = $1")
            .bind(hash)
            .fetch_one(&self.state.db)
            .await
            .expect("reading the blob")
    }

    pub async fn thumbnail_rows(&self) -> i64 {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM thumbnails")
            .fetch_one(&self.state.db)
            .await
            .expect("counting the thumbnails")
    }

    pub async fn thumbnail_blob(&self) -> BlobHash {
        sqlx::query_scalar::<_, BlobHash>("SELECT blob_hash FROM thumbnails LIMIT 1")
            .fetch_one(&self.state.db)
            .await
            .expect("a thumbnail")
    }

    pub async fn staged_uploads(&self) -> usize {
        let mut entries = tokio::fs::read_dir(&self.upload_root)
            .await
            .expect("the staging directory");
        let mut count = 0;
        while entries.next_entry().await.expect("entry").is_some() {
            count += 1;
        }
        count
    }

    /// Puts a session's recorded offset back, the way a request that died mid-body leaves it: the
    /// bytes landed, the row never caught up.
    pub async fn rewind_upload(&self, id: &str, to: i64) {
        sqlx::query("UPDATE uploads SET received = $2 WHERE id = $1::uuid")
            .bind(id)
            .bind(to)
            .execute(&self.state.db)
            .await
            .expect("rewinding the upload");
    }

    pub async fn hold_upload(&self, id: &str) {
        sqlx::query(
            "UPDATE uploads SET writing_until = now() + INTERVAL '5 minutes' WHERE id = $1::uuid",
        )
        .bind(id)
        .execute(&self.state.db)
        .await
        .expect("holding the upload");
    }

    /// Hands the claim to somebody else while a write is in flight, which is what a body slower
    /// than the claim lets happen.
    pub async fn steal_claim(&self, id: Uuid) {
        sqlx::query(
            "UPDATE uploads
             SET writing_until = now() + INTERVAL '5 minutes', writer = gen_random_uuid()
             WHERE id = $1",
        )
        .bind(id)
        .execute(&self.state.db)
        .await
        .expect("stealing the claim");
    }

    pub async fn release_upload(&self, id: &str) {
        sqlx::query("UPDATE uploads SET writing_until = NULL WHERE id = $1::uuid")
            .bind(id)
            .execute(&self.state.db)
            .await
            .expect("releasing the upload");
    }

    pub async fn expire_upload_claims(&self) {
        sqlx::query("UPDATE uploads SET writing_until = now() - INTERVAL '1 second'")
            .execute(&self.state.db)
            .await
            .expect("expiring the claims");
    }

    pub async fn expire_uploads(&self) {
        sqlx::query("UPDATE uploads SET expires_at = now() - INTERVAL '1 second'")
            .execute(&self.state.db)
            .await
            .expect("expiring the uploads");
    }

    pub async fn attempt_rows(&self) -> i64 {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM attempts")
            .fetch_one(&self.state.db)
            .await
            .expect("counting the attempts")
    }

    pub async fn attempt_subjects(&self) -> Vec<String> {
        sqlx::query_scalar::<_, String>("SELECT subject FROM attempts")
            .fetch_all(&self.state.db)
            .await
            .expect("reading the attempts")
    }

    pub async fn age_attempts(&self, by: chrono::Duration) {
        sqlx::query(
            "UPDATE attempts
             SET last_at = last_at - make_interval(secs => $1),
                 blocked_until = blocked_until - make_interval(secs => $1)",
        )
        .bind(f64::from(
            i32::try_from(by.num_seconds()).expect("a small test interval"),
        ))
        .execute(&self.state.db)
        .await
        .expect("ageing the attempts");
    }

    pub async fn expire_shares(&self) {
        sqlx::query("UPDATE shares SET expires_at = now() - INTERVAL '1 second'")
            .execute(&self.state.db)
            .await
            .expect("expiring the shares");
    }

    pub async fn share_rows(&self) -> i64 {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM shares")
            .fetch_one(&self.state.db)
            .await
            .expect("counting the shares")
    }

    pub async fn blob(&self, hash: BlobHash) -> Option<(i64, bool)> {
        sqlx::query_as::<_, (i64, Option<chrono::DateTime<chrono::Utc>>)>(
            "SELECT ref_count, unreferenced_since FROM blobs WHERE hash = $1",
        )
        .bind(hash)
        .fetch_optional(&self.state.db)
        .await
        .expect("reading the blob")
        .map(|(count, since)| (count, since.is_some()))
    }
}

fn with_database(url: &str, database: &str) -> String {
    let (base, query) = match url.split_once('?') {
        Some((base, query)) => (base, Some(query)),
        None => (url, None),
    };
    let trimmed = base.trim_end_matches('/');
    let cut = trimmed.rfind('/').expect("a database in DATABASE_URL");
    let replaced = format!("{}/{database}", &trimmed[..cut]);
    match query {
        Some(query) => format!("{replaced}?{query}"),
        None => replaced,
    }
}

#[macro_export]
macro_rules! database_test {
    ($name:ident, $harness:ident, $body:block) => {
        #[tokio::test]
        async fn $name() {
            let Some($harness) = common::Harness::start().await else {
                eprintln!(
                    "skipping {}: DATABASE_URL is not set",
                    stringify!($name)
                );
                return;
            };
            $body
            $harness.stop().await;
        }
    };
}
