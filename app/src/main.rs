use std::path::PathBuf;

use roxycloud_client::sync::watch::{Command, Session as SyncSession, Status, watch};
use roxycloud_client::{Debounce, Engine, Remote};
use roxycloud_core::node::Node;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Mutex;

const STATUS_EVENT: &str = "sync:status";

#[derive(Clone)]
struct Credentials {
    server: String,
    token: String,
}

#[derive(Default)]
struct Desktop {
    remote: Mutex<Option<Remote>>,
    credentials: Mutex<Option<Credentials>>,
    sync: Mutex<Option<SyncSession>>,
}

#[tauri::command]
async fn login(
    desktop: State<'_, Desktop>,
    server: String,
    email: String,
    password: String,
) -> Result<(), String> {
    let (remote, session) = Remote::login(&server, &email, &password)
        .await
        .map_err(|error| error.to_string())?;

    *desktop.remote.lock().await = Some(remote);
    *desktop.credentials.lock().await = Some(Credentials {
        server,
        token: session.token,
    });
    Ok(())
}

#[tauri::command]
async fn list_folder(desktop: State<'_, Desktop>, path: String) -> Result<Vec<Node>, String> {
    let guard = desktop.remote.lock().await;
    let remote = guard.as_ref().ok_or("not connected to a server")?;
    remote
        .list(&path)
        .await
        .map_err(move |error| format!("{path}: {error}"))
}

#[tauri::command]
async fn start_sync(
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
    let engine = Engine::open(folder, remote).map_err(|error| error.to_string())?;
    let session = watch(engine, Debounce::default()).map_err(|error| error.to_string())?;

    let mut status = session.subscribe();
    tauri::async_runtime::spawn(async move {
        while let Ok(update) = status.recv().await {
            let stopped = matches!(update, Status::Stopped);
            let _ = app.emit(STATUS_EVENT, update);
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
async fn sync_control(desktop: State<'_, Desktop>, command: Command) -> Result<(), String> {
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

fn main() {
    tauri::Builder::default()
        .manage(Desktop::default())
        .invoke_handler(tauri::generate_handler![
            login,
            list_folder,
            start_sync,
            sync_control
        ])
        .run(tauri::generate_context!())
        .expect("starting the RoxyCloud window");
}
