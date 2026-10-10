use futures::StreamExt;
use sqlx::PgPool;
use stashden_core::blob::BlobHash;
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::error::ApiError;
use crate::storage::{BlobStore, StorageError};

macro_rules! expected_references {
    () => {
        "SELECT hash, count(*)::BIGINT AS references
         FROM (
             SELECT blob_hash AS hash FROM nodes WHERE blob_hash IS NOT NULL
             UNION ALL
             SELECT blob_hash FROM versions
             UNION ALL
             SELECT blob_hash FROM thumbnails
         ) held
         GROUP BY hash"
    };
}

macro_rules! expected_usage {
    () => {
        "SELECT owner_id, COALESCE(sum(bytes), 0)::BIGINT AS bytes
         FROM (
             SELECT owner_id, size AS bytes
             FROM nodes WHERE kind = 'file' AND deleted_at IS NULL
             UNION ALL
             SELECT nodes.owner_id, versions.size
             FROM versions JOIN nodes ON nodes.id = versions.node_id
             WHERE nodes.deleted_at IS NULL
         ) charged
         GROUP BY owner_id"
    };
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    pub verify_content: bool,
    pub repair: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrongCount {
    pub hash: BlobHash,
    pub stored: i64,
    pub expected: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrongUsage {
    pub owner_id: Uuid,
    pub stored: i64,
    pub expected: i64,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Findings {
    pub missing: Vec<BlobHash>,
    pub corrupt: Vec<BlobHash>,
    pub wrong_counts: Vec<WrongCount>,
    pub wrong_usage: Vec<WrongUsage>,
    pub checked_blobs: usize,
    pub repaired: bool,
}

impl Findings {
    #[must_use]
    pub fn needs_a_person(&self) -> bool {
        !self.missing.is_empty() || !self.corrupt.is_empty()
    }

    #[must_use]
    pub fn is_clean(&self) -> bool {
        !self.needs_a_person() && self.wrong_counts.is_empty() && self.wrong_usage.is_empty()
    }
}

pub async fn run(
    pool: &PgPool,
    blobs: &dyn BlobStore,
    options: Options,
) -> Result<Findings, ApiError> {
    let mut findings = Findings {
        wrong_counts: wrong_counts(pool).await?,
        wrong_usage: wrong_usage(pool).await?,
        ..Findings::default()
    };

    let wanted = sqlx::query_scalar::<_, BlobHash>(concat!(
        "SELECT hash FROM (",
        expected_references!(),
        ") referenced ORDER BY hash"
    ))
    .fetch_all(pool)
    .await?;
    findings.checked_blobs = wanted.len();
    for hash in wanted {
        match inspect(blobs, hash, options.verify_content).await? {
            Condition::Sound => {}
            Condition::Missing => findings.missing.push(hash),
            Condition::Corrupt => findings.corrupt.push(hash),
        }
    }

    if options.repair {
        repair(pool, &findings).await?;
        findings.wrong_counts.clear();
        findings.wrong_usage.clear();
        findings.repaired = true;
    }
    Ok(findings)
}

enum Condition {
    Sound,
    Missing,
    Corrupt,
}

async fn inspect(
    blobs: &dyn BlobStore,
    hash: BlobHash,
    verify_content: bool,
) -> Result<Condition, ApiError> {
    let reader = match blobs.read(hash).await {
        Ok(reader) => reader,
        Err(StorageError::NotFound(_)) => return Ok(Condition::Missing),
        Err(other) => return Err(other.into()),
    };
    if !verify_content {
        return Ok(Condition::Sound);
    }

    let mut hasher = blake3::Hasher::new();
    let mut chunks = ReaderStream::new(reader);
    while let Some(chunk) = chunks.next().await {
        hasher.update(&chunk.map_err(StorageError::Io)?);
    }
    Ok(if BlobHash::from(hasher.finalize()) == hash {
        Condition::Sound
    } else {
        Condition::Corrupt
    })
}

async fn wrong_counts(pool: &PgPool) -> Result<Vec<WrongCount>, ApiError> {
    let rows = sqlx::query_as::<_, (BlobHash, i64, i64)>(concat!(
        "WITH expected AS (",
        expected_references!(),
        ")
         SELECT blobs.hash, blobs.ref_count, COALESCE(expected.references, 0)
         FROM blobs LEFT JOIN expected ON expected.hash = blobs.hash
         WHERE blobs.ref_count <> COALESCE(expected.references, 0)
         ORDER BY blobs.hash"
    ))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(hash, stored, expected)| WrongCount {
            hash,
            stored,
            expected,
        })
        .collect())
}

async fn wrong_usage(pool: &PgPool) -> Result<Vec<WrongUsage>, ApiError> {
    let rows = sqlx::query_as::<_, (Uuid, i64, i64)>(concat!(
        "WITH expected AS (",
        expected_usage!(),
        ")
         SELECT quotas.owner_id, quotas.bytes_used, COALESCE(expected.bytes, 0)
         FROM quotas LEFT JOIN expected ON expected.owner_id = quotas.owner_id
         WHERE quotas.bytes_used <> COALESCE(expected.bytes, 0)
         ORDER BY quotas.owner_id"
    ))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(owner_id, stored, expected)| WrongUsage {
            owner_id,
            stored,
            expected,
        })
        .collect())
}

async fn repair(pool: &PgPool, findings: &Findings) -> Result<(), ApiError> {
    let mut tx = pool.begin().await?;
    for wrong in &findings.wrong_counts {
        sqlx::query(
            "UPDATE blobs
             SET ref_count = $2,
                 unreferenced_since = CASE
                     WHEN $2 = 0 THEN COALESCE(unreferenced_since, now())
                     ELSE NULL
                 END
             WHERE hash = $1",
        )
        .bind(wrong.hash)
        .bind(wrong.expected)
        .execute(&mut *tx)
        .await?;
    }
    for wrong in &findings.wrong_usage {
        sqlx::query("UPDATE quotas SET bytes_used = $2, updated_at = now() WHERE owner_id = $1")
            .bind(wrong.owner_id)
            .bind(wrong.expected)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}
