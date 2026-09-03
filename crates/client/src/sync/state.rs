use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::path::RelPath;
use super::snapshot::{Entry, Snapshot};

pub const STATE_FILE_NAME: &str = ".roxycloud-sync.json";

#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("reading {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("writing {path}")]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{path} is not readable sync state; delete it to start from a full comparison")]
    Corrupt {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tracked {
    pub entry: Entry,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtime_ms: Option<i64>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncState {
    #[serde(default)]
    tracked: BTreeMap<RelPath, Tracked>,
}

impl SyncState {
    #[must_use]
    pub fn base(&self) -> Snapshot {
        self.tracked
            .iter()
            .map(|(path, tracked)| (path.clone(), tracked.entry.clone()))
            .collect()
    }

    #[must_use]
    pub fn get(&self, path: &RelPath) -> Option<&Tracked> {
        self.tracked.get(path)
    }

    pub fn record(&mut self, path: RelPath, entry: Entry, mtime_ms: Option<i64>) {
        self.tracked.insert(path, Tracked { entry, mtime_ms });
    }

    pub fn forget(&mut self, path: &RelPath) {
        self.tracked.remove(path);
    }

    pub fn load(path: &Path) -> Result<Self, StateError> {
        match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|source| StateError::Corrupt {
                path: path.to_path_buf(),
                source,
            }),
            Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(StateError::Read {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), StateError> {
        let write = |source| StateError::Write {
            path: path.to_path_buf(),
            source,
        };

        let json = serde_json::to_vec_pretty(self)
            .map_err(io::Error::from)
            .map_err(write)?;
        let staged = path.with_extension("tmp");
        fs::write(&staged, &json).map_err(write)?;
        fs::rename(&staged, path).map_err(write)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use roxycloud_core::blob::BlobHash;

    fn at(input: &str) -> RelPath {
        RelPath::parse(input).expect("valid path")
    }

    fn file(contents: &[u8]) -> Entry {
        let size = u64::try_from(contents.len()).expect("small test payload");
        Entry::file(BlobHash::from(blake3::hash(contents)), size)
    }

    fn scratch(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("roxycloud-state-{name}"));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("scratch directory");
        directory.join(STATE_FILE_NAME)
    }

    #[test]
    fn missing_state_reads_as_empty_rather_than_failing() {
        let path = scratch("missing").with_file_name("nothing-here.json");
        assert_eq!(
            SyncState::load(&path).expect("empty state"),
            SyncState::default()
        );
    }

    #[test]
    fn state_round_trips_through_a_file() {
        let path = scratch("round-trip");
        let mut state = SyncState::default();
        state.record(at("a/b.txt"), file(b"agreed"), Some(1_700_000_000_000));
        state.save(&path).expect("saves");

        assert_eq!(SyncState::load(&path).expect("loads"), state);
    }

    #[test]
    fn corrupt_state_is_reported_instead_of_silently_reset() {
        let path = scratch("corrupt");
        fs::write(&path, b"{ not json").expect("writes");
        assert!(matches!(
            SyncState::load(&path),
            Err(StateError::Corrupt { .. })
        ));
    }

    #[test]
    fn the_base_snapshot_drops_the_mtime_cache() {
        let mut state = SyncState::default();
        state.record(at("a.txt"), file(b"agreed"), Some(42));
        state.record(at("dir"), Entry::Directory, None);

        let base = state.base();
        assert_eq!(base.get(&at("a.txt")), Some(&file(b"agreed")));
        assert_eq!(base.get(&at("dir")), Some(&Entry::Directory));
    }

    #[test]
    fn forgetting_a_path_removes_it_from_the_base() {
        let mut state = SyncState::default();
        state.record(at("a.txt"), file(b"agreed"), None);
        state.forget(&at("a.txt"));
        assert!(state.base().is_empty());
    }
}
