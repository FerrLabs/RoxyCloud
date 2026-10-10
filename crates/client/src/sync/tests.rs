use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use stashden_core::grant::Access;
use tokio::sync::broadcast;

use super::debounce::Debounce;
use super::engine::{Engine, Report};
use super::fit::{Rules, folds_case};
use super::held::Held;
use super::local;
use super::path::RelPath;
use super::snapshot::{Entry, Snapshot};
use super::state::{LEGACY_STATE_FILE_NAME, STATE_FILE_NAME, SyncState};
use super::transport::{Expect, Transport};
use super::watch::{Command, DEFAULT_POLL, Status, watch};

struct FakeServer {
    root: PathBuf,
    held: Held,
    before_upload: Option<Box<dyn Fn() + Send + Sync>>,
}

fn changed(path: &RelPath) -> io::Error {
    io::Error::other(format!("{path} changed on the server since it was listed"))
}

#[expect(
    clippy::unused_async_trait_impl,
    reason = "a fake that stands in for the network is clearer sharing the trait's own shape"
)]
impl Transport for FakeServer {
    type Error = io::Error;

    async fn snapshot(&self) -> Result<Snapshot, Self::Error> {
        let mut snapshot = Snapshot::new();
        list_everything(&self.root, None, &mut snapshot)?;
        Ok(snapshot)
    }

    async fn download_to(&self, path: &RelPath, destination: &Path) -> Result<(), Self::Error> {
        fs::copy(path.to_path(&self.root), destination).map(|_| ())
    }

    async fn upload_from(
        &self,
        path: &RelPath,
        source: &Path,
        expect: &Expect,
    ) -> Result<(), Self::Error> {
        if let Some(interfere) = &self.before_upload {
            interfere();
        }
        let destination = path.to_path(&self.root);
        let current = fs::read(&destination).ok().map(|bytes| {
            Entry::file(
                blake3::hash(&bytes).into(),
                u64::try_from(bytes.len()).expect("small test file"),
            )
        });
        match (expect, current) {
            (Expect::Absent, None) => {}
            (Expect::Etag(wanted), Some(Entry::File { etag, .. })) if *wanted == etag => {}
            _ => return Err(changed(path)),
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source, destination).map(|_| ())
    }

    async fn remove(&self, path: &RelPath) -> Result<(), Self::Error> {
        let target = path.to_path(&self.root);
        if target.is_dir() {
            return fs::remove_dir_all(target);
        }
        fs::remove_file(target)
    }

    async fn create_directory(&self, path: &RelPath) -> Result<(), Self::Error> {
        fs::create_dir_all(path.to_path(&self.root))
    }

    async fn held(&self) -> Result<Held, Self::Error> {
        Ok(self.held.clone())
    }
}

fn list_everything(
    root: &Path,
    directory: Option<&RelPath>,
    snapshot: &mut Snapshot,
) -> io::Result<()> {
    let absolute = directory.map_or_else(|| root.to_path_buf(), |path| path.to_path(root));
    for entry in fs::read_dir(absolute)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = match directory {
            Some(parent) => parent.child(&name),
            None => RelPath::parse(&name),
        }
        .map_err(|error| io::Error::other(error.to_string()))?;
        if entry.file_type()?.is_dir() {
            snapshot.insert(path.clone(), Entry::Directory);
            list_everything(root, Some(&path), snapshot)?;
        } else {
            let bytes = fs::read(entry.path())?;
            let size = u64::try_from(bytes.len()).expect("small test file");
            snapshot.insert(path, Entry::file(blake3::hash(&bytes).into(), size));
        }
    }
    Ok(())
}

struct Pair {
    local: PathBuf,
    server: PathBuf,
}

impl Pair {
    fn new(name: &str) -> Self {
        let base = std::env::temp_dir().join(format!("stashden-sync-{name}"));
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
        self.engine_holding(Held::default())
    }

    fn engine_holding(&self, held: Held) -> Engine<FakeServer> {
        Engine::open(
            self.local.clone(),
            FakeServer {
                root: self.server.clone(),
                held,
                before_upload: None,
            },
        )
        .expect("the engine opens")
    }

    fn engine_racing(&self, interfere: impl Fn() + Send + Sync + 'static) -> Engine<FakeServer> {
        Engine::open(
            self.local.clone(),
            FakeServer {
                root: self.server.clone(),
                held: Held::default(),
                before_upload: Some(Box::new(interfere)),
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
async fn a_folder_synced_by_an_older_version_keeps_its_history() {
    let pair = Pair::new("legacy-state");
    pair.write_local("a.txt", b"agreed");
    pair.engine().sync_once().await.expect("first sync");
    fs::rename(
        pair.local.join(STATE_FILE_NAME),
        pair.local.join(LEGACY_STATE_FILE_NAME),
    )
    .expect("stands in for a folder an older version synced");

    fs::remove_file(pair.local.join("a.txt")).expect("removes the local copy");
    let report = pair.engine().sync_once().await.expect("second sync");

    assert_eq!(
        report.deleted_remotely, 1,
        "the deletion is recognised, so the old state was carried over"
    );
    assert!(pair.local.join(STATE_FILE_NAME).exists());
    assert!(!pair.local.join(LEGACY_STATE_FILE_NAME).exists());
}

#[tokio::test]
async fn the_current_state_file_wins_over_one_an_older_version_left() {
    let pair = Pair::new("both-states");
    pair.write_local("a.txt", b"agreed");
    pair.engine().sync_once().await.expect("first sync");
    fs::write(pair.local.join(LEGACY_STATE_FILE_NAME), b"{}").expect("writes a stale legacy state");

    fs::remove_file(pair.local.join("a.txt")).expect("removes the local copy");
    let report = pair.engine().sync_once().await.expect("second sync");

    assert_eq!(report.deleted_remotely, 1);
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

fn eager() -> Debounce {
    Debounce::new(Duration::from_millis(50), Duration::from_secs(5))
}

async fn next_sync(status: &mut broadcast::Receiver<Status>) -> Report {
    let waiting = async {
        loop {
            match status.recv().await {
                Ok(Status::Synced(report)) if !report.is_quiet() => return report,
                Ok(_) => (),
                Err(error) => panic!("the status channel closed: {error}"),
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), waiting)
        .await
        .expect("a sync within ten seconds")
}

async fn any_sync(status: &mut broadcast::Receiver<Status>) {
    let waiting = async {
        loop {
            match status.recv().await {
                Ok(Status::Synced(_)) => return,
                Ok(_) => (),
                Err(error) => panic!("the status channel closed: {error}"),
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), waiting)
        .await
        .expect("a pass within ten seconds");
}

#[tokio::test]
async fn the_watcher_syncs_a_file_written_after_it_started() {
    let pair = Pair::new("watch");
    let session = watch(pair.engine(), eager(), DEFAULT_POLL).expect("watches the folder");
    let mut status = session.subscribe();

    pair.write_local("a.txt", b"written while watching");

    let report = next_sync(&mut status).await;
    assert_eq!(report.uploaded, 1);
    assert_eq!(
        pair.read_server("a.txt").as_deref(),
        Some(&b"written while watching"[..])
    );

    session.stop().await;
}

#[tokio::test]
async fn a_paused_session_holds_the_change_until_it_resumes() {
    let pair = Pair::new("watch-pause");
    let session = watch(pair.engine(), eager(), DEFAULT_POLL).expect("watches the folder");
    let mut status = session.subscribe();
    any_sync(&mut status).await;
    session.send(Command::Pause).await;

    pair.write_local("a.txt", b"written while paused");
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        pair.read_server("a.txt").is_none(),
        "a paused session transfers nothing"
    );

    session.send(Command::Resume).await;
    let report = next_sync(&mut status).await;

    assert_eq!(report.uploaded, 1);
    assert_eq!(
        pair.read_server("a.txt").as_deref(),
        Some(&b"written while paused"[..])
    );

    session.stop().await;
}

#[tokio::test]
async fn a_folder_removed_locally_is_removed_on_the_server() {
    let pair = Pair::new("remove-remote-directory");
    pair.write_local("photos/summer/x.jpg", b"agreed");
    pair.engine().sync_once().await.expect("first sync");

    fs::remove_dir_all(pair.local.join("photos")).expect("removes the local folder");
    let report = pair.engine().sync_once().await.expect("second sync");

    assert_eq!(report.directories_removed_remotely, 2);
    assert_eq!(report.deleted_remotely, 1);
    assert!(!pair.server.join("photos").exists());

    let settled = pair.engine().sync_once().await.expect("third sync");
    assert!(
        settled.is_quiet(),
        "the removal is recorded, so the next sync has nothing to redo: {settled:?}"
    );
}

#[tokio::test]
async fn a_folder_holding_something_new_on_the_server_survives_a_local_removal() {
    let pair = Pair::new("keep-remote-directory");
    pair.write_local("photos/x.jpg", b"agreed");
    pair.engine().sync_once().await.expect("first sync");

    pair.write_server("photos/added.jpg", b"from another machine");
    fs::remove_dir_all(pair.local.join("photos")).expect("removes the local folder");
    let report = pair.engine().sync_once().await.expect("second sync");

    assert_eq!(report.directories_removed_remotely, 0);
    assert_eq!(
        pair.read_server("photos/added.jpg").as_deref(),
        Some(&b"from another machine"[..]),
        "a file this side never saw is not something a local delete may take"
    );
    assert_eq!(report.downloaded, 1);
}

#[tokio::test]
async fn a_folder_that_came_back_on_the_server_is_not_removed_again() {
    let pair = Pair::new("stale-directory-record");
    pair.write_local("photos/x.jpg", b"agreed");
    pair.engine().sync_once().await.expect("first sync");

    fs::remove_dir_all(pair.local.join("photos")).expect("removes the local folder");
    fs::remove_dir_all(pair.server.join("photos")).expect("removes the server folder");
    pair.engine().sync_once().await.expect("second sync");

    fs::create_dir_all(pair.server.join("photos")).expect("another machine makes it again");
    let report = pair.engine().sync_once().await.expect("third sync");

    assert_eq!(
        report.directories_removed_remotely, 0,
        "the record from the old folder is not permission to delete a new one"
    );
    assert!(pair.server.join("photos").is_dir());
    assert!(
        pair.local.join("photos").is_dir(),
        "it arrives here instead, like any other folder made elsewhere"
    );
}

#[tokio::test]
async fn an_edit_in_a_read_only_share_is_held_back_rather_than_retried() {
    let pair = Pair::new("read-only-share");
    pair.write_server("Shared with me/archive/old.txt", b"theirs");
    let held = Held::from_mounts([("archive".to_owned(), stashden_core::grant::Access::Read)]);
    pair.engine_holding(held.clone())
        .sync_once()
        .await
        .expect("the first sync");

    pair.write_local("Shared with me/archive/old.txt", b"mine, locally");
    let report = pair
        .engine_holding(held)
        .sync_once()
        .await
        .expect("the second sync");

    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert!(report.blocked.is_empty(), "{:?}", report.blocked);
    assert_eq!(report.uploaded, 0);
    assert_eq!(
        report.held.iter().map(RelPath::as_str).collect::<Vec<_>>(),
        ["Shared with me/archive/old.txt"]
    );
    assert_eq!(
        pair.read_server("Shared with me/archive/old.txt")
            .as_deref(),
        Some(&b"theirs"[..])
    );
    assert_eq!(
        pair.read_local("Shared with me/archive/old.txt").as_deref(),
        Some(&b"mine, locally"[..])
    );
}

#[tokio::test]
async fn a_conflict_in_a_read_only_share_still_brings_the_owners_version_down() {
    let pair = Pair::new("read-only-conflict");
    pair.write_server("Shared with me/archive/old.txt", b"theirs");
    let held = Held::from_mounts([("archive".to_owned(), stashden_core::grant::Access::Read)]);
    pair.engine_holding(held.clone())
        .sync_once()
        .await
        .expect("the first sync");

    pair.write_local("Shared with me/archive/old.txt", b"mine, locally");
    pair.write_server("Shared with me/archive/old.txt", b"theirs, edited");
    let report = pair
        .engine_holding(held.clone())
        .sync_once()
        .await
        .expect("the conflicting sync");

    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(report.uploaded, 0);
    assert_eq!(
        pair.read_local("Shared with me/archive/old.txt").as_deref(),
        Some(&b"theirs, edited"[..])
    );
    let copies: Vec<String> = pair
        .local_names()
        .into_iter()
        .filter(|name| name.contains("conflict"))
        .collect();
    assert_eq!(copies.len(), 1, "{copies:?}");
    assert_eq!(
        pair.read_local(&copies[0]).as_deref(),
        Some(&b"mine, locally"[..])
    );
    assert!(pair.read_server(&copies[0]).is_none());
    assert_eq!(
        report.held.iter().map(RelPath::as_str).collect::<Vec<_>>(),
        [copies[0].as_str()]
    );

    pair.write_server("Shared with me/archive/old.txt", b"theirs, again");
    let later = pair
        .engine_holding(held)
        .sync_once()
        .await
        .expect("a later sync");

    assert!(later.failures.is_empty(), "{:?}", later.failures);
    assert_eq!(later.uploaded, 0);
    assert_eq!(
        pair.read_local("Shared with me/archive/old.txt").as_deref(),
        Some(&b"theirs, again"[..])
    );
    assert!(pair.read_server(&copies[0]).is_none());
}

#[tokio::test]
async fn the_watcher_brings_down_what_was_already_on_the_server_when_it_starts() {
    let pair = Pair::new("watch-first-pass");
    pair.write_server("a.txt", b"waiting on the server");

    let session = watch(pair.engine(), eager(), DEFAULT_POLL).expect("watches the folder");
    let mut status = session.subscribe();

    let report = next_sync(&mut status).await;
    assert_eq!(report.downloaded, 1);
    assert_eq!(
        pair.read_local("a.txt").as_deref(),
        Some(&b"waiting on the server"[..])
    );

    session.stop().await;
}

#[tokio::test]
async fn the_watcher_brings_down_a_server_change_without_a_local_one() {
    let pair = Pair::new("watch-poll");
    let session =
        watch(pair.engine(), eager(), Duration::from_millis(200)).expect("watches the folder");
    let mut status = session.subscribe();
    any_sync(&mut status).await;

    pair.write_server("b.txt", b"added from another machine");

    let report = next_sync(&mut status).await;
    assert_eq!(report.downloaded, 1);
    assert_eq!(
        pair.read_local("b.txt").as_deref(),
        Some(&b"added from another machine"[..])
    );

    session.stop().await;
}

#[tokio::test]
async fn a_server_edit_during_the_pass_is_not_overwritten() {
    let pair = Pair::new("lost-update");
    pair.write_local("a.txt", b"agreed");
    pair.engine().sync_once().await.expect("first sync");

    pair.write_local("a.txt", b"edited here");
    let server = pair.server.clone();
    let report = pair
        .engine_racing(move || write(&server, "a.txt", b"edited there"))
        .sync_once()
        .await
        .expect("second sync");

    assert_eq!(report.uploaded, 0);
    assert_eq!(report.failures.len(), 1, "{report:?}");
    assert_eq!(
        pair.read_server("a.txt").as_deref(),
        Some(&b"edited there"[..]),
        "an edit the pass never saw is not replaced"
    );

    let report = pair.engine().sync_once().await.expect("third sync");

    assert_eq!(report.conflicts.len(), 1, "{report:?}");
    assert_eq!(
        pair.read_local("a.txt").as_deref(),
        Some(&b"edited there"[..])
    );
    let kept = pair
        .local_names()
        .into_iter()
        .find(|name| name.contains("conflict"))
        .expect("the local edit is kept under another name");
    assert_eq!(pair.read_local(&kept).as_deref(), Some(&b"edited here"[..]));
    assert_eq!(
        pair.read_server(&kept).as_deref(),
        Some(&b"edited here"[..])
    );
}

#[tokio::test]
async fn a_server_file_created_during_the_pass_is_not_overwritten() {
    let pair = Pair::new("lost-create");
    pair.write_local("a.txt", b"created here");
    let server = pair.server.clone();

    let report = pair
        .engine_racing(move || write(&server, "a.txt", b"created there"))
        .sync_once()
        .await
        .expect("syncs");

    assert_eq!(report.uploaded, 0);
    assert_eq!(
        pair.read_server("a.txt").as_deref(),
        Some(&b"created there"[..])
    );
}

#[tokio::test]
async fn junk_the_system_leaves_in_the_folder_stays_local() {
    let pair = Pair::new("ignore-local");
    pair.write_local("photos/.DS_Store", b"finder");
    pair.write_local("~$report.docx", b"word lock");
    pair.write_local("photos/beach.jpg", b"sand");

    let report = pair.engine().sync_once().await.expect("syncs");

    assert_eq!(report.uploaded, 1);
    assert!(pair.read_server("photos/.DS_Store").is_none());
    assert!(pair.read_server("~$report.docx").is_none());
    assert!(
        report.skipped.is_empty(),
        "ignoring is not worth a report line"
    );
}

#[tokio::test]
async fn junk_already_on_the_server_does_not_come_down() {
    let pair = Pair::new("ignore-remote");
    pair.write_server("Thumbs.db", b"explorer");

    let report = pair.engine().sync_once().await.expect("syncs");

    assert_eq!(report.downloaded, 0);
    assert!(pair.read_local("Thumbs.db").is_none());
}

#[tokio::test]
async fn junk_synced_by_an_older_client_is_left_where_it_is() {
    let pair = Pair::new("ignore-legacy");
    pair.write_server("desktop.ini", b"explorer");
    let path = RelPath::parse("desktop.ini").expect("a path");
    let mut state = SyncState::default();
    state.record(path, Entry::file(blake3::hash(b"explorer").into(), 8), None);
    state
        .save(&pair.local.join(STATE_FILE_NAME))
        .expect("saving the old state");

    let report = pair.engine().sync_once().await.expect("syncs");

    assert_eq!(report.deleted_remotely, 0);
    assert_eq!(
        pair.read_server("desktop.ini").as_deref(),
        Some(&b"explorer"[..]),
        "missing locally because it is ignored, not because it was deleted"
    );
}

#[tokio::test]
async fn a_folder_removed_on_the_server_goes_even_with_junk_in_it() {
    let pair = Pair::new("ignore-rmdir");
    pair.write_local("photos/beach.jpg", b"sand");
    pair.engine().sync_once().await.expect("first sync");
    pair.write_local("photos/.DS_Store", b"finder opened it");

    fs::remove_dir_all(pair.server.join("photos")).expect("removed on the server");
    let report = pair.engine().sync_once().await.expect("second sync");

    assert_eq!(report.failures, Vec::new());
    assert_eq!(report.directories_removed_locally, 1);
    assert!(!pair.local.join("photos").exists());
    assert!(
        pair.engine()
            .sync_once()
            .await
            .expect("third sync")
            .is_quiet(),
        "the removal is done, not planned again on every pass"
    );
}

#[tokio::test]
async fn a_folder_with_real_files_left_in_it_is_still_not_removed() {
    let pair = Pair::new("ignore-rmdir-kept");
    pair.write_local("photos/beach.jpg", b"sand");
    pair.engine().sync_once().await.expect("first sync");
    pair.write_local("photos/.DS_Store", b"finder");

    fs::remove_dir_all(pair.server.join("photos")).expect("removed on the server");
    pair.write_local("photos/new.jpg", b"added meanwhile");
    pair.engine().sync_once().await.expect("second sync");

    assert_eq!(
        pair.read_local("photos/new.jpg").as_deref(),
        Some(&b"added meanwhile"[..])
    );
}

const LIKE_WINDOWS: Rules = Rules {
    windows_names: true,
    folds_case: true,
};

fn server_holds_every_name(pair: &Pair) -> bool {
    !cfg!(windows) && !folds_case(&pair.server)
}

#[tokio::test]
async fn a_server_name_windows_refuses_is_reported_and_left_alone() {
    let pair = Pair::new("windows-name");
    if !server_holds_every_name(&pair) {
        return;
    }
    pair.write_server("notes/why?.txt", b"question");
    pair.write_server("notes/fine.txt", b"answer");

    let report = pair
        .engine()
        .holding_names_by(LIKE_WINDOWS)
        .sync_once()
        .await
        .expect("syncs");

    assert_eq!(report.downloaded, 1);
    assert_eq!(report.failures, Vec::new(), "not a failure on every pass");
    assert_eq!(report.unsupported.len(), 1, "{report:?}");
    assert_eq!(report.unsupported[0].path.as_str(), "notes/why?.txt");
    assert_eq!(
        pair.read_server("notes/why?.txt").as_deref(),
        Some(&b"question"[..]),
        "missing here because it cannot exist here, not because it was deleted"
    );
}

#[tokio::test]
async fn two_server_names_one_folder_cannot_tell_apart_are_both_left_alone() {
    let pair = Pair::new("case-clash");
    if !server_holds_every_name(&pair) {
        return;
    }
    pair.write_server("A.txt", b"upper");
    pair.write_server("a.txt", b"lower");

    let report = pair
        .engine()
        .holding_names_by(LIKE_WINDOWS)
        .sync_once()
        .await
        .expect("syncs");

    assert_eq!(report.downloaded, 0);
    assert_eq!(report.unsupported.len(), 2, "{report:?}");
    assert_eq!(pair.read_server("A.txt").as_deref(), Some(&b"upper"[..]));
    assert_eq!(pair.read_server("a.txt").as_deref(), Some(&b"lower"[..]));
}

#[tokio::test]
async fn a_clash_appearing_after_a_sync_deletes_nothing() {
    let pair = Pair::new("case-clash-later");
    if !server_holds_every_name(&pair) {
        return;
    }
    pair.write_local("Notes.txt", b"synced");
    pair.engine()
        .holding_names_by(LIKE_WINDOWS)
        .sync_once()
        .await
        .expect("first sync");

    pair.write_server("notes.txt", b"added from a case-sensitive machine");
    let report = pair
        .engine()
        .holding_names_by(LIKE_WINDOWS)
        .sync_once()
        .await
        .expect("second sync");

    assert_eq!(report.deleted_locally, 0);
    assert_eq!(report.deleted_remotely, 0);
    assert_eq!(
        pair.read_local("Notes.txt").as_deref(),
        Some(&b"synced"[..])
    );
    assert_eq!(
        pair.read_server("Notes.txt").as_deref(),
        Some(&b"synced"[..])
    );
    assert_eq!(
        pair.read_server("notes.txt").as_deref(),
        Some(&b"added from a case-sensitive machine"[..])
    );
}

#[tokio::test]
async fn an_edit_made_during_a_clash_goes_up_once_it_is_resolved() {
    let pair = Pair::new("case-clash-resolved");
    if !server_holds_every_name(&pair) {
        return;
    }
    pair.write_local("Notes.txt", b"synced");
    pair.engine()
        .holding_names_by(LIKE_WINDOWS)
        .sync_once()
        .await
        .expect("first sync");
    pair.write_server("notes.txt", b"added from a case-sensitive machine");
    pair.engine()
        .holding_names_by(LIKE_WINDOWS)
        .sync_once()
        .await
        .expect("sync during the clash");

    pair.write_local("Notes.txt", b"edited during the clash");
    fs::remove_file(pair.server.join("notes.txt")).expect("resolved on the server");
    let report = pair
        .engine()
        .holding_names_by(LIKE_WINDOWS)
        .sync_once()
        .await
        .expect("sync after the clash");

    assert_eq!(
        report.conflicts,
        Vec::new(),
        "the server copy never changed"
    );
    assert_eq!(report.uploaded, 1);
    assert_eq!(
        pair.read_server("Notes.txt").as_deref(),
        Some(&b"edited during the clash"[..])
    );
}

#[tokio::test]
async fn an_empty_local_folder_is_created_on_the_server() {
    let pair = Pair::new("mkdir-remote");
    fs::create_dir_all(pair.local.join("photos/2026")).expect("an empty folder");

    let report = pair.engine().sync_once().await.expect("syncs");

    assert_eq!(report.directories_created_remotely, 2);
    assert!(pair.server.join("photos/2026").is_dir());
    assert!(!report.is_quiet());
}

#[tokio::test]
async fn a_created_folder_is_not_created_again_and_the_next_pass_is_quiet() {
    let pair = Pair::new("mkdir-remote-once");
    fs::create_dir_all(pair.local.join("photos")).expect("an empty folder");
    pair.engine().sync_once().await.expect("first sync");

    let report = pair.engine().sync_once().await.expect("second sync");

    assert!(report.is_quiet(), "{report:?}");
    assert_eq!(report.directories_created_remotely, 0);
}

#[tokio::test]
async fn a_folder_removed_on_the_server_after_it_was_created_goes_here_too() {
    let pair = Pair::new("mkdir-remote-removed");
    fs::create_dir_all(pair.local.join("photos")).expect("an empty folder");
    pair.engine().sync_once().await.expect("first sync");

    fs::remove_dir(pair.server.join("photos")).expect("removed on the server");
    let report = pair.engine().sync_once().await.expect("second sync");

    assert_eq!(report.directories_removed_locally, 1);
    assert!(!pair.local.join("photos").exists());
    assert!(!pair.server.join("photos").exists(), "never recreated");
}

#[tokio::test]
async fn a_folder_inside_a_read_only_share_is_held_not_created() {
    let pair = Pair::new("mkdir-read-only");
    pair.write_server("Shared with me/archive/old.txt", b"theirs");
    pair.engine_holding(Held::from_mounts([("archive".to_owned(), Access::Read)]))
        .sync_once()
        .await
        .expect("first sync");
    fs::create_dir_all(pair.local.join("Shared with me/archive/2026")).expect("a new folder");

    let report = pair
        .engine_holding(Held::from_mounts([("archive".to_owned(), Access::Read)]))
        .sync_once()
        .await
        .expect("second sync");

    assert_eq!(report.directories_created_remotely, 0);
    assert_eq!(report.held.len(), 1, "{report:?}");
    assert!(!pair.server.join("Shared with me/archive/2026").exists());
}
