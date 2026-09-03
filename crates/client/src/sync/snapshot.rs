use std::collections::BTreeMap;

use roxycloud_core::blob::BlobHash;
use roxycloud_core::node::etag_for_file;
use serde::{Deserialize, Serialize};

use super::path::RelPath;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Entry {
    Directory,
    File { etag: String, size: u64 },
}

impl Entry {
    #[must_use]
    pub fn file(hash: BlobHash, size: u64) -> Self {
        Self::File {
            etag: etag_for_file(hash),
            size,
        }
    }

    #[must_use]
    pub const fn is_directory(&self) -> bool {
        matches!(self, Self::Directory)
    }
}

pub type Snapshot = BTreeMap<RelPath, Entry>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_entry_carries_the_etag_the_server_would_compute() {
        let hash = BlobHash::from(blake3::hash(b"contents"));
        let entry = Entry::file(hash, 8);
        assert_eq!(
            entry,
            Entry::File {
                etag: etag_for_file(hash),
                size: 8
            }
        );
    }

    #[test]
    fn identical_bytes_produce_equal_entries() {
        let left = Entry::file(BlobHash::from(blake3::hash(b"same")), 4);
        let right = Entry::file(BlobHash::from(blake3::hash(b"same")), 4);
        assert_eq!(left, right);
    }

    #[test]
    fn different_bytes_produce_different_entries() {
        let left = Entry::file(BlobHash::from(blake3::hash(b"before")), 6);
        let right = Entry::file(BlobHash::from(blake3::hash(b"after")), 5);
        assert_ne!(left, right);
    }
}
