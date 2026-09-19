use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::db;
use crate::error::ApiError;
use crate::grants::{self, Mount};
use roxycloud_core::grant::{Access, SHARED_WITH_ME};
use roxycloud_core::name::NodeName;
use roxycloud_core::node::{Node, NodeKind};
use roxycloud_core::user::User;

#[derive(Debug)]
pub enum Place {
    Own(Node),
    SharedWithMe,
    Shared {
        node: Node,
        mount: Mount,
        at_mount: bool,
    },
}

impl Place {
    #[must_use]
    pub fn writable(&self) -> bool {
        match self {
            Self::Own(_) => true,
            Self::SharedWithMe => false,
            Self::Shared { mount, .. } => mount.access.may_write(),
        }
    }

    pub fn into_movable(self) -> Result<Node, ApiError> {
        match self {
            Self::Own(node)
            | Self::Shared {
                node,
                at_mount: false,
                ..
            } => Ok(node),
            Self::Shared { .. } | Self::SharedWithMe => Err(ApiError::Forbidden),
        }
    }

    pub fn into_node(self) -> Result<Node, ApiError> {
        match self {
            Self::Own(node) => Ok(node),
            Self::Shared {
                mut node,
                mount,
                at_mount,
            } => {
                if at_mount {
                    node.name = mount.mount_name;
                }
                Ok(node)
            }
            Self::SharedWithMe => Err(ApiError::WrongKind { expected: "file" }),
        }
    }
}

fn is_shelf(segment: &NodeName) -> bool {
    segment.as_str() == SHARED_WITH_ME
}

pub async fn locate(
    tx: &mut Transaction<'_, Postgres>,
    caller: &User,
    segments: &[NodeName],
    quota: i64,
) -> Result<Place, ApiError> {
    if let [first, rest @ ..] = segments
        && is_shelf(first)
    {
        let [mount_name, below @ ..] = rest else {
            return Ok(Place::SharedWithMe);
        };
        let (mount, top) = grants::mount(tx, &caller.email, mount_name).await?;
        let node = db::resolve(tx, &top, below).await?;
        return Ok(Place::Shared {
            node,
            mount,
            at_mount: below.is_empty(),
        });
    }

    let root = db::ensure_root(tx, caller.id, quota).await?;
    Ok(Place::Own(db::resolve(tx, &root, segments).await?))
}

pub async fn locate_writable(
    tx: &mut Transaction<'_, Postgres>,
    caller: &User,
    segments: &[NodeName],
    quota: i64,
) -> Result<Place, ApiError> {
    let place = locate(tx, caller, segments, quota).await?;
    if !place.writable() {
        return Err(ApiError::Forbidden);
    }
    Ok(place)
}

pub async fn directory_for_write(
    tx: &mut Transaction<'_, Postgres>,
    caller: &User,
    segments: &[NodeName],
    create: bool,
    quota: i64,
) -> Result<Node, ApiError> {
    let (top, below) = match segments {
        [first, rest @ ..] if is_shelf(first) => {
            let [mount_name, below @ ..] = rest else {
                return Err(ApiError::Forbidden);
            };
            let (mount, top) = grants::mount(tx, &caller.email, mount_name).await?;
            if !mount.access.may_write() {
                return Err(ApiError::Forbidden);
            }
            (top, below)
        }
        _ => (db::ensure_root(tx, caller.id, quota).await?, segments),
    };

    let directory = if create {
        db::create_directories(tx, top.owner_id, &top, below).await?
    } else {
        db::resolve(tx, &top, below).await?
    };
    if directory.kind != NodeKind::Directory {
        return Err(ApiError::WrongKind {
            expected: "directory",
        });
    }
    Ok(directory)
}

pub fn refuse_reserved(parent: &Node, name: &NodeName) -> Result<(), ApiError> {
    if parent.parent_id.is_none() && is_shelf(name) {
        return Err(ApiError::Conflict(SHARED_WITH_ME.to_owned()));
    }
    Ok(())
}

pub async fn file_target(
    tx: &mut Transaction<'_, Postgres>,
    caller: &User,
    segments: &[NodeName],
    create: bool,
    quota: i64,
) -> Result<(Node, NodeName), ApiError> {
    if let [first, mount_name] = segments
        && is_shelf(first)
    {
        let (mount, node) = grants::mount(tx, &caller.email, mount_name).await?;
        if !mount.access.may_write() {
            return Err(ApiError::Forbidden);
        }
        if node.kind == NodeKind::Directory {
            return Err(ApiError::Conflict(mount.mount_name));
        }
        let parent_id = node.parent_id.ok_or(ApiError::NotFound)?;
        let parent = db::live_node(tx, parent_id)
            .await?
            .ok_or(ApiError::NotFound)?;
        return Ok((parent, node.name.parse()?));
    }

    let (name, parents) = segments.split_last().ok_or(ApiError::WrongKind {
        expected: "file path",
    })?;
    let parent = directory_for_write(tx, caller, parents, create, quota).await?;
    refuse_reserved(&parent, name)?;
    Ok((parent, name.clone()))
}

pub async fn quota_holder(
    tx: &mut Transaction<'_, Postgres>,
    caller: &User,
    segments: &[NodeName],
    quota: i64,
) -> Result<Uuid, ApiError> {
    match segments {
        [first, mount_name, ..] if is_shelf(first) => {
            let (mount, top) = grants::mount(tx, &caller.email, mount_name).await?;
            if !mount.access.may_write() {
                return Err(ApiError::Forbidden);
            }
            Ok(top.owner_id)
        }
        [first] if is_shelf(first) => Err(ApiError::Conflict(SHARED_WITH_ME.to_owned())),
        _ => {
            db::ensure_root(tx, caller.id, quota).await?;
            Ok(caller.id)
        }
    }
}

pub async fn shelf(
    tx: &mut Transaction<'_, Postgres>,
    caller: &User,
    root: &Node,
) -> Result<Option<(Node, Vec<(Node, Access)>)>, ApiError> {
    let mounts = grants::mounts(tx, &caller.email).await?;
    if mounts.is_empty() {
        return Ok(None);
    }

    let id = shelf_id(caller.id);
    let mut fingerprint = blake3::Hasher::new();
    let mut updated_at = root.created_at;
    let mut entries = Vec::with_capacity(mounts.len());
    for (mount, mut node) in mounts {
        fingerprint.update(mount.id.as_bytes());
        fingerprint.update(mount.mount_name.as_bytes());
        fingerprint.update(node.etag.as_bytes());
        updated_at = updated_at.max(node.updated_at);
        node.name = mount.mount_name;
        node.parent_id = Some(id);
        entries.push((node, mount.access));
    }

    let directory = Node {
        id,
        owner_id: caller.id,
        parent_id: Some(root.id),
        name: SHARED_WITH_ME.to_owned(),
        kind: NodeKind::Directory,
        blob_hash: None,
        size: 0,
        etag: format!("\"{}\"", &fingerprint.finalize().to_hex()[..32]),
        created_at: root.created_at,
        updated_at,
        deleted_at: None,
    };
    Ok(Some((directory, entries)))
}

fn shelf_id(owner: Uuid) -> Uuid {
    let digest = blake3::hash(&[b"shared-with-me:".as_slice(), owner.as_bytes()].concat());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest.as_bytes()[..16]);
    uuid::Builder::from_custom_bytes(bytes).into_uuid()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shelf_keeps_its_id_from_one_request_to_the_next() {
        let owner = Uuid::now_v7();
        assert_eq!(shelf_id(owner), shelf_id(owner));
    }

    #[test]
    fn two_accounts_do_not_share_a_shelf_id() {
        assert_ne!(shelf_id(Uuid::now_v7()), shelf_id(Uuid::now_v7()));
    }
}
