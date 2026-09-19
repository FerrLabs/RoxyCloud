use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::error::DatabaseError;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::{contains, live_node, node_columns};
use crate::error::ApiError;
use roxycloud_core::grant::Access;
use roxycloud_core::name::{MAX_NAME_LEN, NodeName};
use roxycloud_core::node::{Node, NodeKind};
use roxycloud_core::user::{Email, User};

const ONCE_PER_ADDRESS: &str = "grants_once_per_address";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Mount {
    pub id: Uuid,
    pub node_id: Uuid,
    pub access: Access,
    pub mount_name: String,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Given {
    pub id: Uuid,
    pub node_id: Uuid,
    pub name: String,
    pub kind: NodeKind,
    pub email: String,
    pub access: Access,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Received {
    pub id: Uuid,
    pub name: String,
    pub kind: NodeKind,
    pub access: Access,
    pub owner_email: String,
    pub owner_name: String,
    pub created_at: DateTime<Utc>,
}

pub async fn give(
    tx: &mut Transaction<'_, Postgres>,
    owner: &User,
    node: &Node,
    grantee: &Email,
    access: Access,
) -> Result<Given, ApiError> {
    if grantee.as_str() == owner.email {
        return Err(ApiError::WrongKind {
            expected: "address other than your own",
        });
    }

    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('grants:' || $1, 0))")
        .bind(grantee.as_str())
        .execute(&mut **tx)
        .await?;

    for reached in reached_by(tx, node.owner_id, grantee).await? {
        if contains(tx, &reached, node).await? || contains(tx, node, &reached).await? {
            return Err(ApiError::AlreadyGranted);
        }
    }

    let mount_name = free_mount_name(tx, grantee, &node.name).await?;
    let (id, created_at) = sqlx::query_as::<_, (Uuid, DateTime<Utc>)>(
        "INSERT INTO grants (id, node_id, grantee_email, access, mount_name, created_by)
         VALUES ($1, $2, $3, $4, $5, $6)
         RETURNING id, created_at",
    )
    .bind(Uuid::now_v7())
    .bind(node.id)
    .bind(grantee.as_str())
    .bind(access)
    .bind(&mount_name)
    .bind(owner.id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|error| {
        match error
            .as_database_error()
            .and_then(DatabaseError::constraint)
        {
            Some(ONCE_PER_ADDRESS) => ApiError::AlreadyGranted,
            _ => ApiError::Database(error),
        }
    })?;

    Ok(Given {
        id,
        node_id: node.id,
        name: node.name.clone(),
        kind: node.kind,
        email: grantee.as_str().to_owned(),
        access,
        created_at,
    })
}

async fn reached_by(
    tx: &mut Transaction<'_, Postgres>,
    owner_id: Uuid,
    grantee: &Email,
) -> Result<Vec<Node>, ApiError> {
    sqlx::query_as::<_, Node>(concat!(
        "SELECT ",
        node_columns!(),
        " FROM nodes
         WHERE owner_id = $1
           AND id IN (SELECT node_id FROM grants WHERE grantee_email = $2)"
    ))
    .bind(owner_id)
    .bind(grantee.as_str())
    .fetch_all(&mut **tx)
    .await
    .map_err(Into::into)
}

async fn free_mount_name(
    tx: &mut Transaction<'_, Postgres>,
    grantee: &Email,
    wanted: &str,
) -> Result<String, ApiError> {
    let taken =
        sqlx::query_scalar::<_, String>("SELECT mount_name FROM grants WHERE grantee_email = $1")
            .bind(grantee.as_str())
            .fetch_all(&mut **tx)
            .await?;

    let mut candidate = wanted.to_owned();
    let mut ordinal = 2;
    while taken.contains(&candidate) {
        candidate = numbered(wanted, ordinal);
        ordinal += 1;
    }
    Ok(candidate)
}

fn numbered(name: &str, ordinal: u32) -> String {
    let suffix = format!(" ({ordinal})");
    let mut cut = MAX_NAME_LEN.saturating_sub(suffix.len()).min(name.len());
    while !name.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}{suffix}", &name[..cut])
}

pub async fn given(pool: &PgPool, owner_id: Uuid) -> Result<Vec<Given>, ApiError> {
    sqlx::query_as::<_, Given>(
        "SELECT grants.id, grants.node_id, nodes.name, nodes.kind,
                grants.grantee_email AS email, grants.access, grants.created_at
         FROM grants
         JOIN nodes ON nodes.id = grants.node_id
         WHERE nodes.owner_id = $1 AND nodes.deleted_at IS NULL
         ORDER BY grants.created_at DESC",
    )
    .bind(owner_id)
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

pub async fn received(pool: &PgPool, grantee: &str) -> Result<Vec<Received>, ApiError> {
    sqlx::query_as::<_, Received>(
        "SELECT grants.id, grants.mount_name AS name, nodes.kind, grants.access,
                owners.email AS owner_email, owners.display_name AS owner_name,
                grants.created_at
         FROM grants
         JOIN nodes ON nodes.id = grants.node_id AND nodes.deleted_at IS NULL
         JOIN users owners ON owners.id = nodes.owner_id
         WHERE grants.grantee_email = $1
         ORDER BY lower(grants.mount_name)",
    )
    .bind(grantee)
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

pub async fn mounts(
    tx: &mut Transaction<'_, Postgres>,
    grantee: &str,
) -> Result<Vec<(Mount, Node)>, ApiError> {
    let mounts = sqlx::query_as::<_, Mount>(
        "SELECT grants.id, grants.node_id, grants.access, grants.mount_name
         FROM grants
         JOIN nodes ON nodes.id = grants.node_id AND nodes.deleted_at IS NULL
         WHERE grants.grantee_email = $1
         ORDER BY lower(grants.mount_name)",
    )
    .bind(grantee)
    .fetch_all(&mut **tx)
    .await?;

    let mut found = Vec::with_capacity(mounts.len());
    for mount in mounts {
        if let Some(node) = live_node(tx, mount.node_id).await? {
            found.push((mount, node));
        }
    }
    Ok(found)
}

pub async fn mount(
    tx: &mut Transaction<'_, Postgres>,
    grantee: &str,
    name: &NodeName,
) -> Result<(Mount, Node), ApiError> {
    let mount = sqlx::query_as::<_, Mount>(
        "SELECT id, node_id, access, mount_name
         FROM grants
         WHERE grantee_email = $1 AND mount_name = $2",
    )
    .bind(grantee)
    .bind(name.as_str())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ApiError::NotFound)?;

    let node = live_node(tx, mount.node_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok((mount, node))
}

pub async fn withdraw(pool: &PgPool, caller: &User, id: Uuid) -> Result<(), ApiError> {
    let removed = sqlx::query(
        "DELETE FROM grants
         WHERE id = $1
           AND (grantee_email = $2
                OR node_id IN (SELECT id FROM nodes WHERE owner_id = $3))",
    )
    .bind(id)
    .bind(&caller.email)
    .bind(caller.id)
    .execute(pool)
    .await?;

    if removed.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_mount_of_the_same_name_is_numbered() {
        assert_eq!(numbered("Photos", 2), "Photos (2)");
    }

    #[test]
    fn a_numbered_name_still_fits_in_a_name() {
        let long = "x".repeat(MAX_NAME_LEN);
        let named = numbered(&long, 12);
        assert!(named.len() <= MAX_NAME_LEN);
        assert!(named.ends_with(" (12)"));
    }

    #[test]
    fn numbering_does_not_cut_a_character_in_half() {
        let long = "é".repeat(MAX_NAME_LEN / 2);
        let named = numbered(&long, 3);
        assert!(named.len() <= MAX_NAME_LEN);
        assert!(named.ends_with(" (3)"));
    }
}
