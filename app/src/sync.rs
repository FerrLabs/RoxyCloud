use std::path::PathBuf;

use roxycloud_client::sync::watch::{Command, Status, watch};
use roxycloud_client::{Debounce, Engine, Remote, Report};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tokio::sync::oneshot;

use crate::Desktop;

const STATUS_EVENT: &str = "sync:status";

#[derive(Clone, Serialize)]
pub struct Syncing {
    folder: PathBuf,
    status: Status,
    last: Option<Report>,
}

#[derive(Default)]
pub struct Tracked {
    generation: u64,
    latest: Option<Syncing>,
}

impl Tracked {
    pub fn forget(&mut self) {
        self.generation += 1;
        self.latest = None;
    }
}

#[tauri::command]
pub async fn pick_folder(app: AppHandle) -> Option<PathBuf> {
    let (chosen, answer) = oneshot::channel();
    app.dialog().file().pick_folder(move |folder| {
        let _ = chosen.send(folder);
    });
    answer.await.ok().flatten()?.into_path().ok()
}

#[tauri::command]
pub async fn start_sync(
    app: AppHandle,
    desktop: State<'_, Desktop>,
    folder: PathBuf,
) -> Result<(), String> {
    let credentials = desktop
        .credentials
        .lock()
        .await
        .clone()
        .ok_or("not connected to a server")?;

    let remote =
        Remote::new(&credentials.server, credentials.token).map_err(|error| error.to_string())?;
    let engine = Engine::open(folder.clone(), remote).map_err(|error| error.to_string())?;
    let session = watch(engine, Debounce::default()).map_err(|error| error.to_string())?;

    let generation = {
        let mut tracked = desktop.tracked.lock().await;
        tracked.generation += 1;
        tracked.latest = Some(Syncing {
            folder: folder.clone(),
            status: Status::Idle,
            last: None,
        });
        tracked.generation
    };

    let mut status = session.subscribe();
    tauri::async_runtime::spawn(async move {
        while let Ok(update) = status.recv().await {
            let stopped = matches!(update, Status::Stopped);
            let desktop = app.state::<Desktop>();
            let mut tracked = desktop.tracked.lock().await;
            if tracked.generation != generation {
                break;
            }
            let last = match &update {
                Status::Synced(report) => Some(report.clone()),
                _ => tracked
                    .latest
                    .as_ref()
                    .and_then(|latest| latest.last.clone()),
            };
            let syncing = Syncing {
                folder: folder.clone(),
                status: update,
                last,
            };
            tracked.latest = Some(syncing.clone());
            let _ = app.emit(STATUS_EVENT, syncing);
            if stopped {
                break;
            }
        }
    });

    let previous = desktop.sync.lock().await.replace(session);
    if let Some(previous) = previous {
        previous.stop().await;
    }
    Ok(())
}

#[tauri::command]
pub async fn sync_control(desktop: State<'_, Desktop>, command: Command) -> Result<(), String> {
    if command == Command::Stop {
        let session = desktop.sync.lock().await.take();
        return match session {
            Some(session) => {
                session.stop().await;
                Ok(())
            }
            None => Err("no sync is running".to_owned()),
        };
    }

    let guard = desktop.sync.lock().await;
    let session = guard.as_ref().ok_or("no sync is running")?;
    session.send(command).await;
    Ok(())
}

#[tauri::command]
pub async fn sync_status(desktop: State<'_, Desktop>) -> Result<Option<Syncing>, String> {
    Ok(desktop.tracked.lock().await.latest.clone())
}
