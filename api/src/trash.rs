use chrono::{TimeDelta, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::{charge_quota, lock_owner, name_taken, node_columns, release_blob};
use crate::error::ApiError;
use crate::versions;
use roxycloud_core::blob::BlobHash;
use roxycloud_core::node::Node;

pub async fn send(tx: &mut Transaction<'_, Postgres>, node: &Node) -> Result<(), ApiError> {
    lock_owner(tx, node.owner_id).await?;

    let marked = sqlx::query_as::<_, (Uuid, i64)>(
        "WITH RECURSIVE subtree AS (
             SELECT id FROM nodes WHERE id = $1 AND deleted_at IS NULL
             UNION ALL
             SELECT descendant.id
             FROM nodes descendant
             JOIN subtree ON descendant.parent_id = subtree.id
             WHERE descendant.deleted_at IS NULL
         ) CYCLE id SET looped USING trail,
         marked AS (
             UPDATE nodes SET deleted_at = now(), trash_root_id = $1
             WHERE id IN (SELECT id FROM subtree)
             RETURNING id, size
         )
         SELECT id, size FROM marked",
    )
    .bind(node.id)
    .fetch_all(&mut **tx)
    .await?;

    let ids: Vec<Uuid> = marked.iter().map(|(id, _)| *id).collect();
    let freed =
        marked.iter().map(|(_, size)| size).sum::<i64>() + versions::size_under(tx, &ids).await?;
    charge_quota(tx, node.owner_id, -freed).await
}

pub async fn list(pool: &PgPool, owner_id: Uuid) -> Result<Vec<Node>, ApiError> {
    sqlx::query_as::<_, Node>(concat!(
        "SELECT ",
        node_columns!(),
        " FROM nodes
         WHERE owner_id = $1 AND trash_root_id = id
         ORDER BY deleted_at DESC, name"
    ))
    .bind(owner_id)
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

pub async fn restore(
    tx: &mut Transaction<'_, Postgres>,
    owner_id: Uuid,
    id: Uuid,
) -> Result<Node, ApiError> {
    lock_owner(tx, owner_id).await?;
    let root = root(tx, owner_id, id).await?;

    let (mut charged, opened) = restore_ancestors(tx, &root).await?;
    let restored = sqlx::query_as::<_, (Uuid, i64)>(
        "UPDATE nodes SET deleted_at = NULL, trash_root_id = NULL
         WHERE trash_root_id = $1
         RETURNING id, size",
    )
    .bind(root.id)
    .fetch_all(&mut **tx)
    .await
    .map_err(|error| name_taken(error, &root.name))?;
    let ids: Vec<Uuid> = restored.iter().map(|(id, _)| *id).collect();
    charged += restored.iter().map(|(_, size)| size).sum::<i64>();
    charged += versions::size_under(tx, &ids).await?;

    for batch in opened {
        reroot_survivors(tx, batch).await?;
    }
    charge_quota(tx, owner_id, charged).await?;

    sqlx::query_as::<_, Node>(concat!(
        "SELECT ",
        node_columns!(),
        " FROM nodes WHERE id = $1"
    ))
    .bind(root.id)
    .fetch_one(&mut **tx)
    .await
    .map_err(Into::into)
}

pub async fn purge(
    tx: &mut Transaction<'_, Postgres>,
    owner_id: Uuid,
    id: Uuid,
) -> Result<(), ApiError> {
    lock_owner(tx, owner_id).await?;
    let root = root(tx, owner_id, id).await?;

    let buried = sqlx::query_as::<_, (Uuid, Option<BlobHash>)>(
        "WITH RECURSIVE buried AS (
             SELECT id, blob_hash FROM nodes WHERE id = $1
             UNION ALL
             SELECT descendant.id, descendant.blob_hash
             FROM nodes descendant
             JOIN buried ON descendant.parent_id = buried.id
         ) CYCLE id SET looped USING trail
         SELECT id, blob_hash FROM buried",
    )
    .bind(root.id)
    .fetch_all(&mut **tx)
    .await?;
    let ids: Vec<Uuid> = buried.iter().map(|(id, _)| *id).collect();
    let mut blobs: Vec<BlobHash> = buried.into_iter().filter_map(|(_, hash)| hash).collect();
    blobs.extend(versions::blobs_under(tx, &ids).await?);

    sqlx::query("DELETE FROM nodes WHERE id = $1")
        .bind(root.id)
        .execute(&mut **tx)
        .await?;

    for hash in blobs {
        release_blob(tx, hash).await?;
    }
    Ok(())
}

pub async fn empty(tx: &mut Transaction<'_, Postgres>, owner_id: Uuid) -> Result<u64, ApiError> {
    lock_owner(tx, owner_id).await?;
    let roots = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM nodes WHERE owner_id = $1 AND trash_root_id = id ORDER BY deleted_at DESC",
    )
    .bind(owner_id)
    .fetch_all(&mut **tx)
    .await?;

    let mut purged = 0;
    for id in roots {
        match purge(tx, owner_id, id).await {
            Ok(()) => purged += 1,
            Err(ApiError::NotFound) => {}
            Err(other) => return Err(other),
        }
    }
    Ok(purged)
}

pub async fn expire(pool: &PgPool, retention: TimeDelta) -> Result<u64, ApiError> {
    let due = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT owner_id, id FROM nodes
         WHERE trash_root_id = id AND deleted_at < $1
         ORDER BY deleted_at DESC",
    )
    .bind(Utc::now() - retention)
    .fetch_all(pool)
    .await?;

    let mut purged = 0;
    for (owner_id, id) in due {
        let mut tx = pool.begin().await?;
        match purge(&mut tx, owner_id, id).await {
            Ok(()) => {
                tx.commit().await?;
                purged += 1;
            }
            Err(ApiError::NotFound) => tx.rollback().await?,
            Err(other) => return Err(other),
        }
    }
    Ok(purged)
}

async fn root(
    tx: &mut Transaction<'_, Postgres>,
    owner_id: Uuid,
    id: Uuid,
) -> Result<Node, ApiError> {
    sqlx::query_as::<_, Node>(concat!(
        "SELECT ",
        node_columns!(),
        " FROM nodes
         WHERE id = $1 AND owner_id = $2 AND trash_root_id = id"
    ))
    .bind(id)
    .bind(owner_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ApiError::NotFound)
}

async fn restore_ancestors(
    tx: &mut Transaction<'_, Postgres>,
    node: &Node,
) -> Result<(i64, Vec<Uuid>), ApiError> {
    let mut charged = 0;
    let mut opened = Vec::new();
    let mut next = node.parent_id;

    while let Some(id) = next {
        let trashed = sqlx::query_as::<_, (Uuid, Option<Uuid>, Uuid, String, i64)>(
            "SELECT id, parent_id, trash_root_id, name, size
             FROM nodes WHERE id = $1 AND deleted_at IS NOT NULL",
        )
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?;

        let Some((ancestor, parent_id, batch, name, size)) = trashed else {
            break;
        };

        sqlx::query("UPDATE nodes SET deleted_at = NULL, trash_root_id = NULL WHERE id = $1")
            .bind(ancestor)
            .execute(&mut **tx)
            .await
            .map_err(|error| name_taken(error, &name))?;

        if !opened.contains(&batch) {
            opened.push(batch);
        }
        charged += size;
        next = parent_id;
    }

    Ok((charged, opened))
}

async fn reroot_survivors(tx: &mut Transaction<'_, Postgres>, batch: Uuid) -> Result<(), ApiError> {
    sqlx::query(
        "WITH RECURSIVE stranded AS (
             SELECT survivor.id, survivor.id AS batch
             FROM nodes survivor
             JOIN nodes parent ON parent.id = survivor.parent_id
             WHERE survivor.trash_root_id = $1
               AND survivor.deleted_at IS NOT NULL
               AND parent.deleted_at IS NULL
             UNION ALL
             SELECT descendant.id, stranded.batch
             FROM nodes descendant
             JOIN stranded ON descendant.parent_id = stranded.id
             WHERE descendant.trash_root_id = $1 AND descendant.deleted_at IS NOT NULL
         ) CYCLE id SET looped USING trail
         UPDATE nodes SET trash_root_id = stranded.batch
         FROM stranded WHERE nodes.id = stranded.id",
    )
    .bind(batch)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
