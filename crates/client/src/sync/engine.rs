use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::Serialize;

use super::local::{self, LocalScan, ScanError};
use super::path::RelPath;
use super::plan::{Action, reconcile};
use super::snapshot::Entry;
use super::state::{STATE_FILE_NAME, StateError, SyncState};
use super::transport::Transport;

const PARTIAL_SUFFIX: &str = ".roxypart";

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error(transparent)]
    Scan(#[from] ScanError),
    #[error(transparent)]
    State(#[from] StateError),
    #[error("the scan did not finish")]
    Interrupted(#[from] tokio::task::JoinError),
    #[error("listing the other side failed")]
    Transport(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("{action} {path}")]
    Local {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Failure {
    pub path: RelPath,
    pub reason: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    pub uploaded: usize,
    pub downloaded: usize,
    pub deleted_locally: usize,
    pub deleted_remotely: usize,
    pub directories_created: usize,
    pub directories_removed_locally: usize,
    pub directories_removed_remotely: usize,
    pub conflicts: Vec<RelPath>,
    pub blocked: Vec<RelPath>,
    pub skipped: Vec<String>,
    pub failures: Vec<Failure>,
}

impl Report {
    #[must_use]
    pub fn transferred(&self) -> usize {
        self.uploaded + self.downloaded
    }

    #[must_use]
    pub fn is_quiet(&self) -> bool {
        self.transferred() == 0
            && self.deleted_locally == 0
            && self.deleted_remotely == 0
            && self.directories_created == 0
            && self.directories_removed_locally == 0
            && self.directories_removed_remotely == 0
            && self.conflicts.is_empty()
            && self.failures.is_empty()
    }
}

pub struct Engine<T> {
    root: PathBuf,
    transport: T,
    state: SyncState,
}

impl<T: Transport> Engine<T> {
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn open(root: impl Into<PathBuf>, transport: T) -> Result<Self, SyncError> {
        let root = root.into();
        let state = SyncState::load(&state_path(&root))?;
        Ok(Self {
            root,
            transport,
            state,
        })
    }

    pub async fn sync_once(&mut self) -> Result<Report, SyncError> {
        let scan = self.scan().await?;
        let remote = self
            .transport
            .snapshot()
            .await
            .map_err(|source| SyncError::Transport(Box::new(source)))?;

        let plan = reconcile(&scan.snapshot(), &remote, &self.state.base(), Utc::now());

        let mut report = Report {
            blocked: plan.blocked,
            skipped: scan
                .skipped
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
            ..Report::default()
        };

        for action in plan.actions {
            if let Err(reason) = self.apply(&action, &scan, &remote, &mut report).await {
                report.failures.push(Failure {
                    path: subject(&action).clone(),
                    reason,
                });
            }
        }

        self.record_agreed_directories(&scan, &remote);
        self.state.save(&state_path(&self.root))?;
        Ok(report)
    }

    fn record_agreed_directories(&mut self, scan: &LocalScan, remote: &super::snapshot::Snapshot) {
        let agreed: Vec<RelPath> = scan
            .entries
            .iter()
            .filter(|(path, scanned)| {
                scanned.entry.is_directory() && remote.get(*path) == Some(&Entry::Directory)
            })
            .map(|(path, _)| path.clone())
            .collect();

        for path in agreed {
            self.state.record(path, Entry::Directory, None);
        }
    }

    async fn scan(&self) -> Result<LocalScan, SyncError> {
        let root = self.root.clone();
        let cache = self.state.clone();
        Ok(tokio::task::spawn_blocking(move || local::scan(&root, &cache)).await??)
    }

    async fn apply(
        &mut self,
        action: &Action,
        scan: &LocalScan,
        remote: &super::snapshot::Snapshot,
        report: &mut Report,
    ) -> Result<(), String> {
        match action {
            Action::CreateLocalDirectory(path) => {
                create_directory(&path.to_path(&self.root))?;
                self.state.record(path.clone(), Entry::Directory, None);
                report.directories_created += 1;
            }
            Action::RemoveLocalDirectory(path) => {
                remove_directory(&path.to_path(&self.root))?;
                self.state.forget(path);
                report.directories_removed_locally += 1;
            }
            Action::Download(path) => {
                self.download(path, remote).await?;
                report.downloaded += 1;
            }
            Action::Upload(path) => {
                self.upload(path, scan).await?;
                report.uploaded += 1;
            }
            Action::DeleteLocal(path) => {
                remove_file(&path.to_path(&self.root))?;
                self.state.forget(path);
                report.deleted_locally += 1;
            }
            Action::DeleteRemote(path) => {
                self.transport
                    .remove(path)
                    .await
                    .map_err(|source| source.to_string())?;
                self.state.forget(path);
                report.deleted_remotely += 1;
            }
            Action::RemoveRemoteDirectory(path) => {
                self.transport
                    .remove(path)
                    .await
                    .map_err(|source| source.to_string())?;
                self.state.forget(path);
                report.directories_removed_remotely += 1;
            }
            Action::KeepBoth { path, local_copy } => {
                let from = path.to_path(&self.root);
                let to = local_copy.to_path(&self.root);
                fs::rename(&from, &to)
                    .map_err(|source| format!("renaming {}: {source}", from.display()))?;

                let Some(scanned) = scan.entries.get(path) else {
                    return Err("the local copy vanished mid-sync".to_owned());
                };
                self.transport
                    .upload_from(local_copy, &to)
                    .await
                    .map_err(|source| source.to_string())?;
                self.state.record(
                    local_copy.clone(),
                    scanned.entry.clone(),
                    local::mtime_of(&to),
                );
                self.state.forget(path);

                self.download(path, remote).await?;
                report.conflicts.push(path.clone());
                report.downloaded += 1;
                report.uploaded += 1;
            }
        }
        Ok(())
    }

    async fn download(
        &mut self,
        path: &RelPath,
        remote: &super::snapshot::Snapshot,
    ) -> Result<(), String> {
        let Some(entry) = remote.get(path) else {
            return Err("the remote copy vanished mid-sync".to_owned());
        };

        let destination = path.to_path(&self.root);
        if let Some(parent) = destination.parent() {
            create_directory(parent)?;
        }

        let partial = with_suffix(&destination, PARTIAL_SUFFIX);
        self.transport
            .download_to(path, &partial)
            .await
            .map_err(|source| source.to_string())?;
        fs::rename(&partial, &destination)
            .map_err(|source| format!("moving {} into place: {source}", partial.display()))?;

        self.state
            .record(path.clone(), entry.clone(), local::mtime_of(&destination));
        Ok(())
    }

    async fn upload(&mut self, path: &RelPath, scan: &LocalScan) -> Result<(), String> {
        let Some(scanned) = scan.entries.get(path) else {
            return Err("the local copy vanished mid-sync".to_owned());
        };

        let source = path.to_path(&self.root);
        self.transport
            .upload_from(path, &source)
            .await
            .map_err(|error| error.to_string())?;

        self.state
            .record(path.clone(), scanned.entry.clone(), scanned.mtime_ms);
        self.record_directories_above(path);
        Ok(())
    }

    fn record_directories_above(&mut self, path: &RelPath) {
        let mut parent = path.parent();
        while let Some(directory) = parent {
            parent = directory.parent();
            self.state.record(directory, Entry::Directory, None);
        }
    }
}

fn state_path(root: &Path) -> PathBuf {
    root.join(STATE_FILE_NAME)
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn subject(action: &Action) -> &RelPath {
    match action {
        Action::CreateLocalDirectory(path)
        | Action::RemoveLocalDirectory(path)
        | Action::RemoveRemoteDirectory(path)
        | Action::Download(path)
        | Action::Upload(path)
        | Action::DeleteLocal(path)
        | Action::DeleteRemote(path)
        | Action::KeepBoth { path, .. } => path,
    }
}

fn create_directory(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|source| format!("creating {}: {source}", path.display()))
}

fn remove_directory(path: &Path) -> Result<(), String> {
    fs::remove_dir(path).map_err(|source| format!("removing {}: {source}", path.display()))
}

fn remove_file(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(format!("deleting {}: {source}", path.display())),
    }
}
