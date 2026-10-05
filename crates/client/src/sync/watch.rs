use std::future::pending;
use std::path::Path;
use std::time::{Duration, Instant};

use notify::{RecommendedWatcher, RecursiveMode, Watcher as _, recommended_watcher};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

use super::debounce::Debounce;
use super::engine::{Engine, Report, SyncError};
use super::state::is_state_file;
use super::transport::Transport;

const STATUS_BUFFER: usize = 64;
pub const DEFAULT_POLL: Duration = Duration::from_secs(60);
const PARTIAL_EXTENSIONS: [&str; 2] = ["stashpart", "roxypart"];
const STATE_TEMP_NAMES: [&str; 2] = [".stashden-sync.tmp", ".roxycloud-sync.tmp"];

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

pub fn watch<T>(
    mut engine: Engine<T>,
    debounce: Debounce,
    poll: Duration,
) -> Result<Session, WatchError>
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
        let mut next_poll = tokio::time::Instant::now();
        let _ = announce.send(Status::Idle);

        loop {
            let deadline = if paused {
                None
            } else {
                Some(
                    debounce
                        .deadline()
                        .map(tokio::time::Instant::from_std)
                        .map_or(next_poll, |local| local.min(next_poll)),
                )
            };

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
                        debounce.is_pending() || next_poll <= tokio::time::Instant::now()
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
            next_poll = tokio::time::Instant::now() + poll;
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
    name.is_some_and(|name| is_state_file(name) || STATE_TEMP_NAMES.contains(&name))
        || extension.is_some_and(|extension| PARTIAL_EXTENSIONS.contains(&extension))
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
        assert!(!interesting(&event("/folder/.stashden-sync.json")));
        assert!(!interesting(&event("/folder/.stashden-sync.tmp")));
    }

    #[test]
    fn the_state_file_an_older_version_left_does_not_trigger_a_run_either() {
        assert!(!interesting(&event("/folder/.roxycloud-sync.json")));
        assert!(!interesting(&event("/folder/.roxycloud-sync.tmp")));
        assert!(!interesting(&event("/folder/photos/x.jpg.roxypart")));
    }

    #[test]
    fn a_partial_download_does_not_trigger_another_run() {
        assert!(!interesting(&event("/folder/photos/x.jpg.stashpart")));
    }

    #[test]
    fn an_ordinary_write_triggers_a_run() {
        assert!(interesting(&event("/folder/photos/x.jpg")));
    }

    #[test]
    fn an_event_touching_both_is_still_worth_a_run() {
        let mut both = event("/folder/.stashden-sync.json");
        both.paths.push(PathBuf::from("/folder/a.txt"));
        assert!(interesting(&both));
    }
}
