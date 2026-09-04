// Each integration binary compiles this module on its own, so a helper used by one
// test file reads as dead code in the others.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;

use roxycloud_api::auth::Sessions;
use roxycloud_api::state::AppState;
use roxycloud_api::storage::LocalBlobStore;
use roxycloud_api::{db, users};
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
        let blobs = LocalBlobStore::open(&blob_root)
            .await
            .expect("opening the blob store");

        Some(Self {
            state: AppState {
                db,
                blobs: Arc::new(blobs),
                sessions: Arc::new(Sessions::new("test-secret", chrono::Duration::hours(1))),
                default_quota_bytes: 1_000_000,
            },
            blob_root,
            maintenance,
            database,
        })
    }

    pub async fn stop(self) {
        self.state.db.close().await;
        let _ = tokio::fs::remove_dir_all(&self.blob_root).await;
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
            .write(futures::stream::iter([Ok::<_, std::io::Error>(
                bytes::Bytes::copy_from_slice(contents),
            )]))
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
        Ok(node)
    }

    pub async fn stage(&self, contents: &[u8]) -> (BlobHash, i64) {
        let written = self
            .state
            .blobs
            .write(futures::stream::iter([Ok::<_, std::io::Error>(
                bytes::Bytes::copy_from_slice(contents),
            )]))
            .await
            .expect("writing the blob");
        (
            written.hash,
            i64::try_from(written.size).expect("small test payload"),
        )
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
        db::trash(&mut tx, node).await.expect("trashing");
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
