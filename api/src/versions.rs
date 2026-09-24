use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use crate::db::{charge_quota, release_blob};
use crate::error::ApiError;
use roxycloud_core::blob::BlobHash;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Version {
    pub id: Uuid,
    #[serde(skip)]
    pub blob_hash: BlobHash,
    pub size: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(FromRow)]
struct Held {
    blob_hash: BlobHash,
    size: i64,
}

pub async fn list(
    tx: &mut Transaction<'_, Postgres>,
    node_id: Uuid,
) -> Result<Vec<Version>, ApiError> {
    sqlx::query_as::<_, Version>(
        "SELECT id, blob_hash, size, created_at FROM versions WHERE node_id = $1 ORDER BY id DESC",
    )
    .bind(node_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(Into::into)
}

pub async fn find(
    tx: &mut Transaction<'_, Postgres>,
    node_id: Uuid,
    id: Uuid,
) -> Result<Version, ApiError> {
    sqlx::query_as::<_, Version>(
        "SELECT id, blob_hash, size, created_at FROM versions WHERE node_id = $1 AND id = $2",
    )
    .bind(node_id)
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ApiError::NotFound)
}

pub async fn take(
    tx: &mut Transaction<'_, Postgres>,
    owner_id: Uuid,
    node_id: Uuid,
    id: Uuid,
) -> Result<Version, ApiError> {
    let taken = sqlx::query_as::<_, Version>(
        "DELETE FROM versions WHERE node_id = $1 AND id = $2
         RETURNING id, blob_hash, size, created_at",
    )
    .bind(node_id)
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    release_blob(tx, taken.blob_hash).await?;
    charge_quota(tx, owner_id, -taken.size).await?;
    Ok(taken)
}

pub(crate) async fn keep(
    tx: &mut Transaction<'_, Postgres>,
    node_id: Uuid,
    hash: BlobHash,
    size: i64,
) -> Result<(), ApiError> {
    sqlx::query("INSERT INTO versions (id, node_id, blob_hash, size) VALUES ($1, $2, $3, $4)")
        .bind(Uuid::now_v7())
        .bind(node_id)
        .bind(hash)
        .bind(size)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub(crate) async fn make_room(
    tx: &mut Transaction<'_, Postgres>,
    owner_id: Uuid,
    node_id: Uuid,
    size: i64,
    previous_size: i64,
) -> Result<bool, ApiError> {
    loop {
        match charge_quota(tx, owner_id, size).await {
            Ok(()) => return Ok(true),
            Err(ApiError::QuotaExceeded) => {
                if !drop_oldest(tx, owner_id, node_id).await? {
                    charge_quota(tx, owner_id, size - previous_size).await?;
                    return Ok(false);
                }
            }
            Err(other) => return Err(other),
        }
    }
}

pub(crate) async fn prune_beyond(
    tx: &mut Transaction<'_, Postgres>,
    owner_id: Uuid,
    node_id: Uuid,
    kept: i64,
) -> Result<(), ApiError> {
    let dropped = sqlx::query_as::<_, Held>(
        "DELETE FROM versions
         WHERE id IN (
             SELECT id FROM versions WHERE node_id = $1 ORDER BY id DESC OFFSET $2
         )
         RETURNING blob_hash, size",
    )
    .bind(node_id)
    .bind(kept.max(0))
    .fetch_all(&mut **tx)
    .await?;
    let freed = release(tx, dropped).await?;
    charge_quota(tx, owner_id, -freed).await
}

async fn drop_oldest(
    tx: &mut Transaction<'_, Postgres>,
    owner_id: Uuid,
    node_id: Uuid,
) -> Result<bool, ApiError> {
    let dropped = sqlx::query_as::<_, Held>(
        "DELETE FROM versions
         WHERE id = (SELECT id FROM versions WHERE node_id = $1 ORDER BY id LIMIT 1)
         RETURNING blob_hash, size",
    )
    .bind(node_id)
    .fetch_all(&mut **tx)
    .await?;
    if dropped.is_empty() {
        return Ok(false);
    }
    let freed = release(tx, dropped).await?;
    charge_quota(tx, owner_id, -freed).await?;
    Ok(true)
}

pub(crate) async fn size_under(
    tx: &mut Transaction<'_, Postgres>,
    nodes: &[Uuid],
) -> Result<i64, ApiError> {
    sqlx::query_scalar::<_, i64>(
        "SELECT coalesce(sum(size), 0)::BIGINT FROM versions WHERE node_id = ANY($1)",
    )
    .bind(nodes)
    .fetch_one(&mut **tx)
    .await
    .map_err(Into::into)
}

pub(crate) async fn blobs_under(
    tx: &mut Transaction<'_, Postgres>,
    nodes: &[Uuid],
) -> Result<Vec<BlobHash>, ApiError> {
    sqlx::query_scalar::<_, BlobHash>("SELECT blob_hash FROM versions WHERE node_id = ANY($1)")
        .bind(nodes)
        .fetch_all(&mut **tx)
        .await
        .map_err(Into::into)
}

async fn release(tx: &mut Transaction<'_, Postgres>, held: Vec<Held>) -> Result<i64, ApiError> {
    let mut freed = 0;
    for version in held {
        release_blob(tx, version.blob_hash).await?;
        freed += version.size;
    }
    Ok(freed)
}
