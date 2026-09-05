use sqlx::error::DatabaseError;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::error::ApiError;
use roxycloud_core::blob::BlobHash;
use roxycloud_core::name::NodeName;
use roxycloud_core::node::{Node, NodeKind, etag_for_directory, etag_for_file};

const NAME_PER_PARENT: &str = "nodes_unique_name_per_parent";

pub(crate) async fn lock_owner(
    tx: &mut Transaction<'_, Postgres>,
    owner_id: Uuid,
) -> Result<(), ApiError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text, 0))")
        .bind(owner_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub(crate) fn name_taken(error: sqlx::Error, name: &str) -> ApiError {
    match error
        .as_database_error()
        .and_then(DatabaseError::constraint)
    {
        Some(NAME_PER_PARENT) => ApiError::Conflict(name.to_owned()),
        _ => ApiError::Database(error),
    }
}

macro_rules! node_columns {
    () => {
        "id, owner_id, parent_id, name, kind, blob_hash, size, etag, created_at, updated_at, deleted_at"
    };
}

pub(crate) use node_columns;

pub async fn ensure_root(
    tx: &mut Transaction<'_, Postgres>,
    owner_id: Uuid,
    quota_bytes: i64,
) -> Result<Node, ApiError> {
    sqlx::query("INSERT INTO quotas (owner_id, bytes_max) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(owner_id)
        .bind(quota_bytes)
        .execute(&mut **tx)
        .await?;

    sqlx::query(
        "INSERT INTO nodes (id, owner_id, parent_id, name, kind, etag)
         VALUES ($1, $2, NULL, '', 'directory', $3)
         ON CONFLICT DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(owner_id)
    .bind(etag_for_directory())
    .execute(&mut **tx)
    .await?;

    sqlx::query_as::<_, Node>(concat!(
        "SELECT ",
        node_columns!(),
        " FROM nodes
         WHERE owner_id = $1 AND parent_id IS NULL AND deleted_at IS NULL"
    ))
    .bind(owner_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(Into::into)
}

pub async fn child(
    tx: &mut Transaction<'_, Postgres>,
    parent_id: Uuid,
    name: &NodeName,
) -> Result<Option<Node>, ApiError> {
    sqlx::query_as::<_, Node>(concat!(
        "SELECT ",
        node_columns!(),
        " FROM nodes
         WHERE parent_id = $1 AND name = $2 AND deleted_at IS NULL"
    ))
    .bind(parent_id)
    .bind(name.as_str())
    .fetch_optional(&mut **tx)
    .await
    .map_err(Into::into)
}

pub async fn resolve(
    tx: &mut Transaction<'_, Postgres>,
    root: &Node,
    segments: &[NodeName],
) -> Result<Node, ApiError> {
    let mut current = root.clone();
    for segment in segments {
        current = child(tx, current.id, segment)
            .await?
            .ok_or(ApiError::NotFound)?;
    }
    Ok(current)
}

pub async fn create_directories(
    tx: &mut Transaction<'_, Postgres>,
    owner_id: Uuid,
    root: &Node,
    segments: &[NodeName],
) -> Result<Node, ApiError> {
    let mut current = root.clone();
    for segment in segments {
        current = match child(tx, current.id, segment).await? {
            Some(existing) if existing.kind == NodeKind::Directory => existing,
            Some(_) => {
                return Err(ApiError::Conflict(segment.to_string()));
            }
            None => insert_directory(tx, owner_id, current.id, segment).await?,
        };
    }
    Ok(current)
}

async fn insert_directory(
    tx: &mut Transaction<'_, Postgres>,
    owner_id: Uuid,
    parent_id: Uuid,
    name: &NodeName,
) -> Result<Node, ApiError> {
    let inserted = sqlx::query_as::<_, Node>(concat!(
        "INSERT INTO nodes (id, owner_id, parent_id, name, kind, etag)
         VALUES ($1, $2, $3, $4, 'directory', $5)
         ON CONFLICT (parent_id, name) WHERE deleted_at IS NULL DO NOTHING
         RETURNING ",
        node_columns!()
    ))
    .bind(Uuid::now_v7())
    .bind(owner_id)
    .bind(parent_id)
    .bind(name.as_str())
    .bind(etag_for_directory())
    .fetch_optional(&mut **tx)
    .await?;

    match inserted {
        Some(node) => Ok(node),
        None => match child(tx, parent_id, name).await? {
            Some(existing) if existing.kind == NodeKind::Directory => Ok(existing),
            _ => Err(ApiError::Conflict(name.to_string())),
        },
    }
}

pub async fn list_children(pool: &PgPool, parent_id: Uuid) -> Result<Vec<Node>, ApiError> {
    sqlx::query_as::<_, Node>(concat!(
        "SELECT ",
        node_columns!(),
        " FROM nodes
         WHERE parent_id = $1 AND deleted_at IS NULL
         ORDER BY kind DESC, name"
    ))
    .bind(parent_id)
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

/// Bytes reach the store before anything decides whether a node may hold them, so the row goes in
/// as soon as they land, unreferenced. A write that then fails leaves something the sweeper collects
/// after the grace period instead of a file on disk that nothing knows about.
pub async fn register_blob(pool: &PgPool, hash: BlobHash, size: i64) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO blobs (hash, size, ref_count, unreferenced_since)
         VALUES ($1, $2, 0, now())
         ON CONFLICT DO NOTHING",
    )
    .bind(hash)
    .bind(size)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn put_file(
    tx: &mut Transaction<'_, Postgres>,
    owner_id: Uuid,
    parent: &Node,
    name: &NodeName,
    hash: BlobHash,
    size: i64,
) -> Result<Node, ApiError> {
    sqlx::query("INSERT INTO blobs (hash, size) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(hash)
        .bind(size)
        .execute(&mut **tx)
        .await?;

    let existing = child(tx, parent.id, name).await?;
    if let Some(node) = &existing
        && node.kind == NodeKind::Directory
    {
        return Err(ApiError::Conflict(name.to_string()));
    }

    let previous_size = existing.as_ref().map_or(0, |node| node.size);
    charge_quota(tx, owner_id, size - previous_size).await?;

    acquire_blob(tx, hash).await?;
    if let Some(previous) = existing.as_ref().and_then(|node| node.blob_hash) {
        release_blob(tx, previous).await?;
    }

    let etag = etag_for_file(hash);
    let node = match existing {
        Some(node) => {
            sqlx::query_as::<_, Node>(concat!(
                "UPDATE nodes
                 SET blob_hash = $2, size = $3, etag = $4, updated_at = now()
                 WHERE id = $1
                 RETURNING ",
                node_columns!()
            ))
            .bind(node.id)
            .bind(hash)
            .bind(size)
            .bind(etag)
            .fetch_one(&mut **tx)
            .await?
        }
        None => {
            sqlx::query_as::<_, Node>(concat!(
                "INSERT INTO nodes (id, owner_id, parent_id, name, kind, blob_hash, size, etag)
                 VALUES ($1, $2, $3, $4, 'file', $5, $6, $7)
                 RETURNING ",
                node_columns!()
            ))
            .bind(Uuid::now_v7())
            .bind(owner_id)
            .bind(parent.id)
            .bind(name.as_str())
            .bind(hash)
            .bind(size)
            .bind(etag)
            .fetch_one(&mut **tx)
            .await?
        }
    };

    Ok(node)
}

pub async fn copy_tree(
    tx: &mut Transaction<'_, Postgres>,
    owner_id: Uuid,
    source: &Node,
    parent: &Node,
    name: &NodeName,
) -> Result<Node, ApiError> {
    lock_owner(tx, owner_id).await?;

    if parent.kind != NodeKind::Directory {
        return Err(ApiError::WrongKind {
            expected: "directory",
        });
    }
    if source.kind == NodeKind::Directory && contains(tx, source, parent).await? {
        return Err(ApiError::MoveIntoSelf);
    }
    if child(tx, parent.id, name).await?.is_some() {
        return Err(ApiError::Conflict(name.to_string()));
    }

    let root = copy_one(tx, owner_id, parent.id, name.as_str(), source).await?;
    let mut charged = source.size;
    let mut pending = vec![(source.id, root.id)];

    while let Some((from, into)) = pending.pop() {
        for original in children_in(tx, from).await? {
            let copy = copy_one(tx, owner_id, into, &original.name, &original).await?;
            charged += original.size;
            if original.kind == NodeKind::Directory {
                pending.push((original.id, copy.id));
            }
        }
    }

    charge_quota(tx, owner_id, charged).await?;
    Ok(root)
}

async fn copy_one(
    tx: &mut Transaction<'_, Postgres>,
    owner_id: Uuid,
    parent_id: Uuid,
    name: &str,
    original: &Node,
) -> Result<Node, ApiError> {
    let etag = match original.kind {
        NodeKind::Directory => etag_for_directory(),
        NodeKind::File => original.etag.clone(),
    };

    let copy = sqlx::query_as::<_, Node>(concat!(
        "INSERT INTO nodes (id, owner_id, parent_id, name, kind, blob_hash, size, etag)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         RETURNING ",
        node_columns!()
    ))
    .bind(Uuid::now_v7())
    .bind(owner_id)
    .bind(parent_id)
    .bind(name)
    .bind(original.kind)
    .bind(original.blob_hash)
    .bind(original.size)
    .bind(etag)
    .fetch_one(&mut **tx)
    .await
    .map_err(|error| name_taken(error, name))?;

    if let Some(hash) = original.blob_hash {
        acquire_blob(tx, hash).await?;
    }
    Ok(copy)
}

async fn children_in(
    tx: &mut Transaction<'_, Postgres>,
    parent_id: Uuid,
) -> Result<Vec<Node>, ApiError> {
    sqlx::query_as::<_, Node>(concat!(
        "SELECT ",
        node_columns!(),
        " FROM nodes WHERE parent_id = $1 AND deleted_at IS NULL"
    ))
    .bind(parent_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(Into::into)
}

pub async fn rename(
    tx: &mut Transaction<'_, Postgres>,
    node: &Node,
    parent: &Node,
    name: &NodeName,
) -> Result<Node, ApiError> {
    lock_owner(tx, node.owner_id).await?;

    if parent.kind != NodeKind::Directory {
        return Err(ApiError::WrongKind {
            expected: "directory",
        });
    }
    if node.kind == NodeKind::Directory && contains(tx, node, parent).await? {
        return Err(ApiError::MoveIntoSelf);
    }
    if child(tx, parent.id, name).await?.is_some() {
        return Err(ApiError::Conflict(name.to_string()));
    }

    let etag = match node.kind {
        NodeKind::Directory => etag_for_directory(),
        NodeKind::File => node.etag.clone(),
    };

    sqlx::query_as::<_, Node>(concat!(
        "UPDATE nodes
         SET parent_id = $2, name = $3, etag = $4, updated_at = now()
         WHERE id = $1
         RETURNING ",
        node_columns!()
    ))
    .bind(node.id)
    .bind(parent.id)
    .bind(name.as_str())
    .bind(etag)
    .fetch_one(&mut **tx)
    .await
    .map_err(|error| name_taken(error, name.as_str()))
}

/// Whether `ancestor` is `node` itself or sits above it. Moving a node under something it already
/// contains would orphan the whole subtree, and replacing a node with something inside it would
/// take the replacement along with it.
pub(crate) async fn contains(
    tx: &mut Transaction<'_, Postgres>,
    ancestor: &Node,
    node: &Node,
) -> Result<bool, ApiError> {
    sqlx::query_scalar::<_, bool>(
        "WITH RECURSIVE ancestry AS (
             SELECT id, parent_id FROM nodes WHERE id = $1
             UNION ALL
             SELECT ancestor.id, ancestor.parent_id
             FROM nodes ancestor
             JOIN ancestry ON ancestor.id = ancestry.parent_id
         ) CYCLE id SET looped USING trail
         SELECT EXISTS (SELECT 1 FROM ancestry WHERE id = $2)",
    )
    .bind(node.id)
    .bind(ancestor.id)
    .fetch_one(&mut **tx)
    .await
    .map_err(Into::into)
}

async fn acquire_blob(tx: &mut Transaction<'_, Postgres>, hash: BlobHash) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE blobs SET ref_count = ref_count + 1, unreferenced_since = NULL WHERE hash = $1",
    )
    .bind(hash)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(crate) async fn release_blob(
    tx: &mut Transaction<'_, Postgres>,
    hash: BlobHash,
) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE blobs
         SET ref_count = ref_count - 1,
             unreferenced_since = CASE WHEN ref_count - 1 = 0 THEN now() ELSE NULL END
         WHERE hash = $1 AND ref_count > 0",
    )
    .bind(hash)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(crate) async fn charge_quota(
    tx: &mut Transaction<'_, Postgres>,
    owner_id: Uuid,
    delta: i64,
) -> Result<(), ApiError> {
    if delta == 0 {
        return Ok(());
    }
    let updated = sqlx::query(
        "UPDATE quotas
         SET bytes_used = bytes_used + $2, updated_at = now()
         WHERE owner_id = $1 AND bytes_used + $2 <= bytes_max",
    )
    .bind(owner_id)
    .bind(delta)
    .execute(&mut **tx)
    .await?;

    if updated.rows_affected() == 0 {
        return Err(ApiError::QuotaExceeded);
    }
    Ok(())
}
