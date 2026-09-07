use std::path::PathBuf;

use bytes::Bytes;
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures::{Stream, StreamExt};
use serde::Serialize;
use sqlx::PgPool;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::error::ApiError;

/// How long a session nobody touches survives. Long enough to outlast a laptop lid and a train
/// tunnel, short enough that the scratch space is not a place things accumulate.
pub const LIFETIME_HOURS: i64 = 24;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Session {
    pub id: Uuid,
    #[serde(skip)]
    pub owner_id: Uuid,
    pub path: String,
    pub size: i64,
    pub received: i64,
    #[serde(skip)]
    pub staged: String,
    pub expires_at: DateTime<Utc>,
}

/// Where the bytes of a session in flight live, which is local disk whichever backend owns the
/// blobs. An object store has no append, and its multipart parts have a five mebibyte floor that
/// would decide the client's chunk size and make an offset below a part boundary unresumable. The
/// cost is scratch space for uploads in flight, bounded by the session lifetime.
pub struct Staging {
    root: PathBuf,
}

impl Staging {
    pub async fn open(root: impl Into<PathBuf>) -> Result<Self, ApiError> {
        let root = root.into();
        fs::create_dir_all(&root).await?;
        Ok(Self { root })
    }

    #[must_use]
    pub fn path_for(&self, staged: &str) -> PathBuf {
        self.root.join(staged)
    }

    async fn create(&self, staged: &str) -> Result<(), ApiError> {
        fs::File::create(self.path_for(staged)).await?;
        Ok(())
    }

    /// Writes at `offset` and answers how many bytes the file holds afterwards.
    ///
    /// The file is cut back to the offset first rather than appended to. `received` is only
    /// recorded once a whole chunk has drained, so a request that died mid-body left bytes past
    /// the offset the session remembers, and appending after them would duplicate a region and
    /// lose the tail. A connection lost mid-chunk is the case this feature exists for, so it is
    /// the case the write has to be correct under. Truncating also makes two requests at the same
    /// offset idempotent rather than interleaved.
    async fn write_at<S, E>(
        &self,
        staged: &str,
        offset: u64,
        mut chunks: S,
    ) -> Result<u64, ApiError>
    where
        S: Stream<Item = Result<Bytes, E>> + Unpin,
        E: std::error::Error + Send + Sync + 'static,
    {
        use tokio::io::AsyncSeekExt;

        let mut file = fs::OpenOptions::new()
            .write(true)
            .open(self.path_for(staged))
            .await?;
        file.set_len(offset).await?;
        file.seek(std::io::SeekFrom::Start(offset)).await?;

        while let Some(chunk) = chunks.next().await {
            let chunk = chunk.map_err(|err| {
                ApiError::Storage(crate::storage::StorageError::Upstream(Box::new(err)))
            })?;
            file.write_all(&chunk).await?;
        }
        file.sync_all().await?;

        Ok(file.metadata().await?.len())
    }

    async fn discard(&self, staged: &str) {
        let _ = fs::remove_file(self.path_for(staged)).await;
    }

    /// Clears staged files no session names any more, which is what a crash between deleting the
    /// row and deleting the file would otherwise leave for good.
    ///
    /// A file younger than the grace is left alone whatever the list says: `begin` creates the file
    /// before it inserts the row, so a sweep landing between the two would otherwise take a file
    /// the session about to exist is going to need. The blob store answers the same shape the same
    /// way rather than inventing a second rule.
    pub async fn sweep(&self, live: &[String], grace: Duration) -> Result<u64, ApiError> {
        let mut entries = fs::read_dir(&self.root).await?;
        let mut removed = 0;

        while let Some(entry) = entries.next_entry().await? {
            let named = entry
                .file_name()
                .to_str()
                .is_some_and(|name| live.iter().any(|staged| staged == name));
            let young = entry
                .metadata()
                .await
                .and_then(|meta| meta.modified())
                .is_ok_and(|at| crate::storage::is_recent(at, grace));

            if !named && !young && fs::remove_file(entry.path()).await.is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }
}

pub async fn begin(
    pool: &PgPool,
    staging: &Staging,
    owner_id: Uuid,
    path: &str,
    size: i64,
) -> Result<Session, ApiError> {
    if size < 0 {
        return Err(ApiError::WrongKind {
            expected: "size in bytes",
        });
    }

    let staged = Uuid::now_v7().to_string();
    staging.create(&staged).await?;

    sqlx::query_as::<_, Session>(
        "INSERT INTO uploads (id, owner_id, path, size, staged, expires_at)
         VALUES ($1, $2, $3, $4, $5, now() + make_interval(hours => $6))
         RETURNING id, owner_id, path, size, received, staged, expires_at",
    )
    .bind(Uuid::now_v7())
    .bind(owner_id)
    .bind(path)
    .bind(size)
    .bind(&staged)
    .bind(i32::try_from(LIFETIME_HOURS).unwrap_or(24))
    .fetch_one(pool)
    .await
    .map_err(Into::into)
}

pub async fn of(pool: &PgPool, owner_id: Uuid, id: Uuid) -> Result<Session, ApiError> {
    sqlx::query_as::<_, Session>(
        "SELECT id, owner_id, path, size, received, staged, expires_at
         FROM uploads
         WHERE id = $1 AND owner_id = $2 AND expires_at > now()",
    )
    .bind(id)
    .bind(owner_id)
    .fetch_optional(pool)
    .await?
    .ok_or(ApiError::NotFound)
}

/// Appends at `offset`, which has to be the offset the session is actually at. A client that lost
/// the connection mid-chunk knows what it sent, not what arrived, so the mismatch is reported with
/// the real offset rather than refused blindly.
pub async fn append<S, E>(
    pool: &PgPool,
    staging: &Staging,
    session: &Session,
    offset: i64,
    chunks: S,
) -> Result<Session, ApiError>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::error::Error + Send + Sync + 'static,
{
    if offset != session.received {
        return Err(ApiError::OffsetMismatch {
            expected: session.received,
        });
    }

    let at = u64::try_from(offset).map_err(|_| ApiError::WrongKind {
        expected: "offset that is not negative",
    })?;
    let received = staging.write_at(&session.staged, at, chunks).await?;
    let received = i64::try_from(received).map_err(|_| ApiError::QuotaExceeded)?;

    if received > session.size {
        staging.discard(&session.staged).await;
        drop_session(pool, session.id).await?;
        return Err(ApiError::WrongKind {
            expected: "no more than the size the session was opened with",
        });
    }

    sqlx::query_as::<_, Session>(
        "UPDATE uploads SET received = $2 WHERE id = $1
         RETURNING id, owner_id, path, size, received, staged, expires_at",
    )
    .bind(session.id)
    .bind(received)
    .fetch_one(pool)
    .await
    .map_err(Into::into)
}

pub async fn abandon(pool: &PgPool, staging: &Staging, session: &Session) -> Result<(), ApiError> {
    drop_session(pool, session.id).await?;
    staging.discard(&session.staged).await;
    Ok(())
}

pub async fn drop_session(pool: &PgPool, id: Uuid) -> Result<(), ApiError> {
    sqlx::query("DELETE FROM uploads WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn purge_expired(pool: &PgPool) -> Result<Vec<String>, ApiError> {
    sqlx::query_scalar::<_, String>(
        "DELETE FROM uploads WHERE expires_at <= now() RETURNING staged",
    )
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

pub async fn live_staged(pool: &PgPool) -> Result<Vec<String>, ApiError> {
    sqlx::query_scalar::<_, String>("SELECT staged FROM uploads")
        .fetch_all(pool)
        .await
        .map_err(Into::into)
}

#[must_use]
pub fn staged_path(staging: &Staging, session: &Session) -> PathBuf {
    staging.path_for(&session.staged)
}
