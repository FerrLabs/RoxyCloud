use std::future::pending;
use std::path::Path;
use std::time::Instant;

use notify::{RecommendedWatcher, RecursiveMode, Watcher as _, recommended_watcher};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

use super::debounce::Debounce;
use super::engine::{Engine, Report, SyncError};
use super::state::STATE_FILE_NAME;
use super::transport::Transport;

const STATUS_BUFFER: usize = 64;
const PARTIAL_EXTENSION: &str = "roxypart";
const STATE_TEMP_NAME: &str = ".roxycloud-sync.tmp";

#[derive(Debug, thiserror::Error)]
pub enum WatchError {
    #[error("watching the folder failed")]
    Notify(#[from] notify::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Command {
    SyncNow,
    Pause,
    Resume,
    Stop,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum Status {
    Idle,
    Syncing,
    Synced(Report),
    Failed { reason: String },
    Paused,
    Stopped,
}

pub struct Session {
    commands: mpsc::Sender<Command>,
    status: broadcast::Sender<Status>,
    task: JoinHandle<()>,
}

impl Session {
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Status> {
        self.status.subscribe()
    }

    pub async fn send(&self, command: Command) {
        let _ = self.commands.send(command).await;
    }

    pub async fn stop(self) {
        self.send(Command::Stop).await;
        let _ = self.task.await;
    }
}

pub fn watch<T>(mut engine: Engine<T>, debounce: Debounce) -> Result<Session, WatchError>
where
    T: Transport + Send + Sync + 'static,
{
    let root = engine.root().to_path_buf();
    let (changes_in, mut changes) = mpsc::unbounded_channel();
    let mut watcher: RecommendedWatcher = recommended_watcher(move |event| {
        if let Ok(event) = event
            && interesting(&event)
        {
            let _ = changes_in.send(());
        }
    })?;
    watcher.watch(&root, RecursiveMode::Recursive)?;

    let (commands, mut inbox) = mpsc::channel(STATUS_BUFFER);
    let (status, _) = broadcast::channel(STATUS_BUFFER);
    let announce = status.clone();

    let task = tokio::spawn(async move {
        let _watcher = watcher;
        let mut debounce = debounce;
        let mut paused = false;
        let _ = announce.send(Status::Idle);

        loop {
            let deadline = debounce.deadline().map(tokio::time::Instant::from_std);

            let due = tokio::select! {
                command = inbox.recv() => match command {
                    None | Some(Command::Stop) => break,
                    Some(Command::Pause) => {
                        paused = true;
                        let _ = announce.send(Status::Paused);
                        false
                    }
                    Some(Command::Resume) => {
                        paused = false;
                        let _ = announce.send(Status::Idle);
                        debounce.is_pending()
                    }
                    Some(Command::SyncNow) => true,
                },
                change = changes.recv() => {
                    if change.is_none() {
                        break;
                    }
                    debounce.touched(Instant::now());
                    false
                }
                () = wait_until(deadline) => true,
            };

            if !due || paused {
                continue;
            }

            debounce.taken();
            let _ = announce.send(Status::Syncing);
            let _ = announce.send(match engine.sync_once().await {
                Ok(report) => Status::Synced(report),
                Err(error) => Status::Failed {
                    reason: describe(&error),
                },
            });
        }

        let _ = announce.send(Status::Stopped);
    });

    Ok(Session {
        commands,
        status,
        task,
    })
}

async fn wait_until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => pending().await,
    }
}

fn interesting(event: &notify::Event) -> bool {
    event.paths.iter().any(|path| !is_ours(path))
}

fn is_ours(path: &Path) -> bool {
    let name = path.file_name().and_then(|name| name.to_str());
    let extension = path.extension().and_then(|extension| extension.to_str());
    matches!(name, Some(STATE_FILE_NAME | STATE_TEMP_NAME))
        || matches!(extension, Some(PARTIAL_EXTENSION))
}

fn describe(error: &SyncError) -> String {
    let mut message = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn event(path: &str) -> notify::Event {
        notify::Event {
            kind: notify::EventKind::Any,
            paths: vec![PathBuf::from(path)],
            attrs: notify::event::EventAttributes::default(),
        }
    }

    #[test]
    fn the_state_file_does_not_trigger_another_run() {
        assert!(!interesting(&event("/folder/.roxycloud-sync.json")));
        assert!(!interesting(&event("/folder/.roxycloud-sync.tmp")));
    }

    #[test]
    fn a_partial_download_does_not_trigger_another_run() {
        assert!(!interesting(&event("/folder/photos/x.jpg.roxypart")));
    }

    #[test]
    fn an_ordinary_write_triggers_a_run() {
        assert!(interesting(&event("/folder/photos/x.jpg")));
    }

    #[test]
    fn an_event_touching_both_is_still_worth_a_run() {
        let mut both = event("/folder/.roxycloud-sync.json");
        both.paths.push(PathBuf::from("/folder/a.txt"));
        assert!(interesting(&both));
    }
}
