use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::engine::Engine;
use super::local;
use super::path::RelPath;
use super::snapshot::Snapshot;
use super::state::SyncState;
use super::transport::Transport;

struct FakeServer {
    root: PathBuf,
}

impl Transport for FakeServer {
    type Error = io::Error;

    async fn snapshot(&self) -> Result<Snapshot, Self::Error> {
        let scan = local::scan(&self.root, &SyncState::default())
            .map_err(|error| io::Error::other(error.to_string()))?;
        Ok(scan.snapshot())
    }

    async fn download_to(&self, path: &RelPath, destination: &Path) -> Result<(), Self::Error> {
        fs::copy(path.to_path(&self.root), destination).map(|_| ())
    }

    async fn upload_from(&self, path: &RelPath, source: &Path) -> Result<(), Self::Error> {
        let destination = path.to_path(&self.root);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source, destination).map(|_| ())
    }

    async fn remove(&self, path: &RelPath) -> Result<(), Self::Error> {
        fs::remove_file(path.to_path(&self.root))
    }
}

struct Pair {
    local: PathBuf,
    server: PathBuf,
}

impl Pair {
    fn new(name: &str) -> Self {
        let base = std::env::temp_dir().join(format!("roxycloud-sync-{name}"));
        let _ = fs::remove_dir_all(&base);
        let pair = Self {
            local: base.join("local"),
            server: base.join("server"),
        };
        fs::create_dir_all(&pair.local).expect("local root");
        fs::create_dir_all(&pair.server).expect("server root");
        pair
    }

    fn engine(&self) -> Engine<FakeServer> {
        Engine::open(
            self.local.clone(),
            FakeServer {
                root: self.server.clone(),
            },
        )
        .expect("the engine opens")
    }

    fn write_local(&self, relative: &str, contents: &[u8]) {
        write(&self.local, relative, contents);
    }

    fn write_server(&self, relative: &str, contents: &[u8]) {
        write(&self.server, relative, contents);
    }

    fn read_local(&self, relative: &str) -> Option<Vec<u8>> {
        fs::read(self.local.join(relative)).ok()
    }

    fn read_server(&self, relative: &str) -> Option<Vec<u8>> {
        fs::read(self.server.join(relative)).ok()
    }

    fn local_names(&self) -> Vec<String> {
        names(&self.local)
    }
}

fn write(root: &Path, relative: &str, contents: &[u8]) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent directory");
    }
    fs::write(path, contents).expect("writes the file");
}

fn names(root: &Path) -> Vec<String> {
    let scan = local::scan(root, &SyncState::default()).expect("scans");
    scan.entries
        .keys()
        .map(|path| path.as_str().to_owned())
        .collect()
}

#[tokio::test]
async fn a_new_local_file_reaches_the_server() {
    let pair = Pair::new("upload");
    pair.write_local("notes/a.txt", b"mine");

    let report = pair.engine().sync_once().await.expect("syncs");

    assert_eq!(report.uploaded, 1);
    assert_eq!(
        pair.read_server("notes/a.txt").as_deref(),
        Some(&b"mine"[..])
    );
}

#[tokio::test]
async fn a_new_server_file_reaches_the_folder() {
    let pair = Pair::new("download");
    pair.write_server("notes/b.txt", b"theirs");

    let report = pair.engine().sync_once().await.expect("syncs");

    assert_eq!(report.downloaded, 1);
    assert_eq!(
        pair.read_local("notes/b.txt").as_deref(),
        Some(&b"theirs"[..])
    );
}

#[tokio::test]
async fn a_file_deleted_locally_is_deleted_on_the_server() {
    let pair = Pair::new("delete-remote");
    pair.write_local("a.txt", b"agreed");
    pair.engine().sync_once().await.expect("first sync");

    fs::remove_file(pair.local.join("a.txt")).expect("removes the local copy");
    let report = pair.engine().sync_once().await.expect("second sync");

    assert_eq!(report.deleted_remotely, 1);
    assert!(pair.read_server("a.txt").is_none());
}

#[tokio::test]
async fn a_file_deleted_on_the_server_is_deleted_locally() {
    let pair = Pair::new("delete-local");
    pair.write_local("a.txt", b"agreed");
    pair.engine().sync_once().await.expect("first sync");

    fs::remove_file(pair.server.join("a.txt")).expect("removes the server copy");
    let report = pair.engine().sync_once().await.expect("second sync");

    assert_eq!(report.deleted_locally, 1);
    assert!(pair.read_local("a.txt").is_none());
}

#[tokio::test]
async fn a_file_changed_on_both_sides_keeps_both_copies() {
    let pair = Pair::new("conflict");
    pair.write_local("a.txt", b"agreed");
    pair.engine().sync_once().await.expect("first sync");

    pair.write_local("a.txt", b"mine");
    pair.write_server("a.txt", b"theirs");
    let report = pair.engine().sync_once().await.expect("second sync");

    assert_eq!(report.conflicts.len(), 1);
    assert_eq!(pair.read_local("a.txt").as_deref(), Some(&b"theirs"[..]));

    let conflict = report.conflicts[0].clone();
    let kept = pair
        .local_names()
        .into_iter()
        .find(|name| name.starts_with("a (conflict "))
        .expect("the losing copy is kept under a new name");
    assert_eq!(
        Path::new(&kept).extension(),
        Some("txt".as_ref()),
        "the extension survives: {kept}"
    );
    assert_eq!(pair.read_local(&kept).as_deref(), Some(&b"mine"[..]));
    assert_eq!(pair.read_server(&kept).as_deref(), Some(&b"mine"[..]));
    assert_eq!(conflict.as_str(), "a.txt");
}

#[tokio::test]
async fn a_second_sync_with_nothing_changed_does_no_work() {
    let pair = Pair::new("idempotent");
    pair.write_local("a.txt", b"mine");
    pair.write_server("b.txt", b"theirs");
    pair.engine().sync_once().await.expect("first sync");

    let report = pair.engine().sync_once().await.expect("second sync");

    assert!(report.is_quiet(), "{report:?}");
}

#[tokio::test]
async fn state_survives_a_restart_so_nothing_is_re_uploaded() {
    let pair = Pair::new("restart");
    pair.write_local("a.txt", b"mine");
    pair.engine().sync_once().await.expect("first sync");

    let mut restarted = pair.engine();
    let report = restarted.sync_once().await.expect("sync after a restart");

    assert!(report.is_quiet(), "{report:?}");
}

#[tokio::test]
async fn an_empty_server_directory_is_created_locally() {
    let pair = Pair::new("directory");
    fs::create_dir_all(pair.server.join("photos")).expect("server directory");

    let report = pair.engine().sync_once().await.expect("syncs");

    assert_eq!(report.directories_created, 1);
    assert!(pair.local.join("photos").is_dir());
}

#[tokio::test]
async fn a_directory_that_still_holds_something_is_not_removed_and_does_not_stop_the_run() {
    let pair = Pair::new("partial-failure");
    pair.write_server("dir/tracked.txt", b"came from the server");
    pair.engine().sync_once().await.expect("first sync");

    fs::remove_dir_all(pair.server.join("dir")).expect("the server drops the whole directory");
    pair.write_local("dir/untracked.txt", b"written while offline");

    let report = pair.engine().sync_once().await.expect("syncs");

    assert_eq!(report.uploaded, 1, "the new local file still went up");
    assert_eq!(report.deleted_locally, 1, "the tracked file was removed");
    assert_eq!(report.failures.len(), 1, "{:?}", report.failures);
    assert_eq!(report.failures[0].path.as_str(), "dir");
    assert!(
        pair.local.join("dir/untracked.txt").is_file(),
        "a directory with unsynced content is left alone"
    );
    assert_eq!(
        pair.read_server("dir/untracked.txt").as_deref(),
        Some(&b"written while offline"[..])
    );
}
