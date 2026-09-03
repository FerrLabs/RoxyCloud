use std::collections::BTreeMap;
use std::fs::{self, File, Metadata};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use roxycloud_core::blob::BlobHash;

use super::path::RelPath;
use super::snapshot::Entry;
use super::state::{STATE_FILE_NAME, SyncState};

const READ_CHUNK: usize = 64 * 1024;

#[derive(Debug, thiserror::Error)]
#[error("reading {path}")]
pub struct ScanError {
    pub path: PathBuf,
    #[source]
    pub source: io::Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scanned {
    pub entry: Entry,
    pub mtime_ms: Option<i64>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LocalScan {
    pub entries: BTreeMap<RelPath, Scanned>,
    pub skipped: Vec<PathBuf>,
}

impl LocalScan {
    #[must_use]
    pub fn snapshot(&self) -> super::snapshot::Snapshot {
        self.entries
            .iter()
            .map(|(path, scanned)| (path.clone(), scanned.entry.clone()))
            .collect()
    }
}

pub fn scan(root: &Path, cache: &SyncState) -> Result<LocalScan, ScanError> {
    let mut scan = LocalScan::default();
    walk(root, None, cache, &mut scan)?;
    Ok(scan)
}

fn walk(
    root: &Path,
    directory: Option<&RelPath>,
    cache: &SyncState,
    scan: &mut LocalScan,
) -> Result<(), ScanError> {
    let absolute = directory.map_or_else(|| root.to_path_buf(), |path| path.to_path(root));
    let reader = fs::read_dir(&absolute).map_err(|source| ScanError {
        path: absolute.clone(),
        source,
    })?;

    for entry in reader {
        let entry = entry.map_err(|source| ScanError {
            path: absolute.clone(),
            source,
        })?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            scan.skipped.push(entry.path());
            continue;
        };
        if name == STATE_FILE_NAME {
            continue;
        }

        let Ok(path) = (match directory {
            Some(parent) => parent.child(name),
            None => RelPath::parse(name),
        }) else {
            scan.skipped.push(entry.path());
            continue;
        };

        let metadata = entry.metadata().map_err(|source| ScanError {
            path: entry.path(),
            source,
        })?;

        if metadata.is_symlink() {
            scan.skipped.push(entry.path());
        } else if metadata.is_dir() {
            scan.entries.insert(
                path.clone(),
                Scanned {
                    entry: Entry::Directory,
                    mtime_ms: None,
                },
            );
            walk(root, Some(&path), cache, scan)?;
        } else if metadata.is_file() {
            let scanned = fingerprint(root, &path, &metadata, cache)?;
            scan.entries.insert(path, scanned);
        } else {
            scan.skipped.push(entry.path());
        }
    }

    Ok(())
}

fn fingerprint(
    root: &Path,
    path: &RelPath,
    metadata: &Metadata,
    cache: &SyncState,
) -> Result<Scanned, ScanError> {
    let size = metadata.len();
    let mtime_ms = modified_ms(metadata);

    if let (Some(tracked), Some(mtime_ms)) = (cache.get(path), mtime_ms)
        && tracked.mtime_ms == Some(mtime_ms)
        && matches!(&tracked.entry, Entry::File { size: known, .. } if *known == size)
    {
        return Ok(Scanned {
            entry: tracked.entry.clone(),
            mtime_ms: Some(mtime_ms),
        });
    }

    let hash = hash_file(&path.to_path(root))?;
    Ok(Scanned {
        entry: Entry::file(hash, size),
        mtime_ms,
    })
}

fn modified_ms(metadata: &Metadata) -> Option<i64> {
    metadata
        .modified()
        .ok()
        .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
        .and_then(|since| i64::try_from(since.as_millis()).ok())
}

pub fn hash_file(path: &Path) -> Result<BlobHash, ScanError> {
    let read = |source| ScanError {
        path: path.to_path_buf(),
        source,
    };

    let mut file = File::open(path).map_err(read)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0u8; READ_CHUNK];
    loop {
        let read_bytes = file.read(&mut buffer).map_err(read)?;
        if read_bytes == 0 {
            break;
        }
        hasher.update(&buffer[..read_bytes]);
    }
    Ok(BlobHash::from(hasher.finalize()))
}

#[must_use]
pub fn mtime_of(path: &Path) -> Option<i64> {
    fs::metadata(path).ok().as_ref().and_then(modified_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("roxycloud-scan-{name}"));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("scratch directory");
        directory
    }

    fn write(root: &Path, relative: &str, contents: &[u8]) {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent directory");
        }
        fs::write(path, contents).expect("writes the file");
    }

    fn at(input: &str) -> RelPath {
        RelPath::parse(input).expect("valid path")
    }

    #[test]
    fn walks_files_and_directories() {
        let root = scratch("walk");
        write(&root, "a.txt", b"one");
        write(&root, "photos/b.txt", b"two");

        let scan = scan(&root, &SyncState::default()).expect("scans");

        assert_eq!(
            scan.entries.keys().collect::<Vec<_>>(),
            [&at("a.txt"), &at("photos"), &at("photos/b.txt")]
        );
        assert_eq!(scan.entries[&at("photos")].entry, Entry::Directory);
    }

    #[test]
    fn hashes_match_what_the_server_would_store() {
        let root = scratch("hash");
        write(&root, "a.txt", b"contents");

        let scan = scan(&root, &SyncState::default()).expect("scans");
        let expected = Entry::file(BlobHash::from(blake3::hash(b"contents")), 8);
        assert_eq!(scan.entries[&at("a.txt")].entry, expected);
    }

    #[test]
    fn its_own_state_file_is_not_part_of_the_folder() {
        let root = scratch("state-file");
        write(&root, STATE_FILE_NAME, b"{}");
        write(&root, "a.txt", b"one");

        let scan = scan(&root, &SyncState::default()).expect("scans");
        assert_eq!(scan.entries.keys().collect::<Vec<_>>(), [&at("a.txt")]);
    }

    #[test]
    fn an_unchanged_file_is_not_rehashed() {
        let root = scratch("cache");
        write(&root, "a.txt", b"original");

        let first = scan(&root, &SyncState::default()).expect("scans");
        let scanned = first.entries[&at("a.txt")].clone();

        let mut cache = SyncState::default();
        cache.record(
            at("a.txt"),
            Entry::File {
                etag: "\"a stale etag the scan should trust\"".to_owned(),
                size: 8,
            },
            scanned.mtime_ms,
        );

        let second = scan(&root, &cache).expect("scans");
        assert_eq!(
            second.entries[&at("a.txt")].entry,
            Entry::File {
                etag: "\"a stale etag the scan should trust\"".to_owned(),
                size: 8
            },
            "same size and mtime means the cached fingerprint is reused"
        );
    }

    #[test]
    fn a_file_whose_size_changed_is_rehashed_even_at_the_same_mtime() {
        let root = scratch("resize");
        write(&root, "a.txt", b"grown a bit");

        let scanned =
            scan(&root, &SyncState::default()).expect("scans").entries[&at("a.txt")].clone();

        let mut cache = SyncState::default();
        cache.record(
            at("a.txt"),
            Entry::File {
                etag: "\"stale\"".to_owned(),
                size: 8,
            },
            scanned.mtime_ms,
        );

        let second = scan(&root, &cache).expect("scans");
        assert_eq!(second.entries[&at("a.txt")].entry, scanned.entry);
    }
}
