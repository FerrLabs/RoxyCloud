use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::blob::BlobHash;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "postgres", derive(sqlx::Type))]
#[cfg_attr(
    feature = "postgres",
    sqlx(type_name = "node_kind", rename_all = "lowercase")
)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind {
    Directory,
    File,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
pub struct Node {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub name: String,
    pub kind: NodeKind,
    #[serde(skip)]
    pub blob_hash: Option<BlobHash>,
    pub size: i64,
    pub etag: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<DateTime<Utc>>,
}

impl Node {
    #[must_use]
    pub fn is_trashed(&self) -> bool {
        self.deleted_at.is_some()
    }
}

#[must_use]
pub fn etag_for_file(hash: BlobHash) -> String {
    format!("\"{}\"", &hash.to_hex()[..32])
}

#[must_use]
pub fn etag_for_directory() -> String {
    format!("\"{}\"", Uuid::now_v7().simple())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_etag_is_derived_from_content() {
        let hash = BlobHash::from(blake3::hash(b"same bytes"));
        assert_eq!(etag_for_file(hash), etag_for_file(hash));
    }

    #[test]
    fn file_etag_changes_with_content() {
        let a = etag_for_file(BlobHash::from(blake3::hash(b"before")));
        let b = etag_for_file(BlobHash::from(blake3::hash(b"after")));
        assert_ne!(a, b);
    }

    #[test]
    fn etags_are_quoted_per_rfc9110() {
        let etag = etag_for_file(BlobHash::from(blake3::hash(b"x")));
        assert!(etag.starts_with('"') && etag.ends_with('"'));
        assert!(!etag[1..etag.len() - 1].contains('"'));
    }

    #[test]
    fn directory_etags_are_unique_per_change() {
        assert_ne!(etag_for_directory(), etag_for_directory());
    }
}
