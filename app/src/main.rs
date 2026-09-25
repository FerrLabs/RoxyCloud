mod failure;
mod sync;

use roxycloud_client::sync::watch::Session as SyncSession;
use roxycloud_client::{Remote, free_path};
use roxycloud_core::node::{Node, Trashed};
use roxycloud_core::user::User;
use roxycloud_core::version::Version;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_updater::UpdaterExt;
use tokio::sync::Mutex;

use failure::Failure;
use uuid::Uuid;

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
    tracked: Mutex<sync::Tracked>,
    offered: Mutex<Option<tauri_plugin_updater::Update>>,
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

#[derive(serde::Serialize)]
struct Available {
    version: String,
    notes: Option<String>,
}

#[derive(serde::Serialize)]
struct Update {
    current: String,
    available: Option<Available>,
}

#[tauri::command]
async fn check_update(desktop: State<'_, Desktop>, app: AppHandle) -> Result<Update, String> {
    let current = app.package_info().version.to_string();
    let update = app
        .updater()
        .map_err(|error| error.to_string())?
        .check()
        .await
        .map_err(|error| error.to_string())?;

    let available = update.as_ref().map(|update| Available {
        version: update.version.clone(),
        notes: update.body.clone(),
    });
    *desktop.offered.lock().await = update;

    Ok(Update { current, available })
}

#[tauri::command]
async fn install_update(desktop: State<'_, Desktop>, app: AppHandle) -> Result<(), String> {
    let update = desktop
        .offered
        .lock()
        .await
        .take()
        .ok_or("check for updates before installing one")?;

    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|error| error.to_string())?;

    app.restart();
}

#[tauri::command]
async fn sign_out(desktop: State<'_, Desktop>) -> Result<(), String> {
    desktop.tracked.lock().await.forget();
    if let Some(session) = desktop.sync.lock().await.take() {
        session.stop().await;
    }
    desktop.remote.lock().await.take();
    desktop.credentials.lock().await.take();
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
async fn account(desktop: State<'_, Desktop>) -> Result<User, String> {
    let guard = desktop.remote.lock().await;
    let remote = guard.as_ref().ok_or("not connected to a server")?;
    remote.me().await.map_err(|error| error.to_string())
}

#[tauri::command]
async fn read_file(
    desktop: State<'_, Desktop>,
    path: String,
) -> Result<tauri::ipc::Response, String> {
    let guard = desktop.remote.lock().await;
    let remote = guard.as_ref().ok_or("not connected to a server")?;
    let bytes = remote
        .read(&path)
        .await
        .map_err(|error| format!("{path}: {error}"))?;
    Ok(tauri::ipc::Response::new(bytes.to_vec()))
}

#[tauri::command]
async fn download_file(
    app: AppHandle,
    desktop: State<'_, Desktop>,
    path: String,
) -> Result<String, String> {
    let guard = desktop.remote.lock().await;
    let remote = guard.as_ref().ok_or("not connected to a server")?;
    let destination = into_downloads(&app, &path)?;

    remote
        .download(&path, &destination)
        .await
        .map_err(|error| format!("{path}: {error}"))?;
    Ok(destination.to_string_lossy().into_owned())
}

fn into_downloads(app: &AppHandle, path: &str) -> Result<std::path::PathBuf, String> {
    let directory = app
        .path()
        .download_dir()
        .map_err(|error| error.to_string())?;
    let name = path.rsplit_once('/').map_or(path, |(_, name)| name);
    Ok(free_path(&directory, name))
}

#[tauri::command]
async fn list_versions(desktop: State<'_, Desktop>, path: String) -> Result<Vec<Version>, Failure> {
    let guard = desktop.remote.lock().await;
    let remote = guard.as_ref().ok_or("not connected to a server")?;
    Ok(remote.list_versions(&path).await?)
}

#[tauri::command]
async fn download_version(
    app: AppHandle,
    desktop: State<'_, Desktop>,
    path: String,
    id: Uuid,
) -> Result<String, Failure> {
    let guard = desktop.remote.lock().await;
    let remote = guard.as_ref().ok_or("not connected to a server")?;
    let destination = into_downloads(&app, &path)?;

    remote.download_version(&path, id, &destination).await?;
    Ok(destination.to_string_lossy().into_owned())
}

#[tauri::command]
async fn restore_version(
    desktop: State<'_, Desktop>,
    path: String,
    id: Uuid,
) -> Result<Node, Failure> {
    let guard = desktop.remote.lock().await;
    let remote = guard.as_ref().ok_or("not connected to a server")?;
    Ok(remote.restore_version(&path, id).await?)
}

#[tauri::command]
async fn move_node(desktop: State<'_, Desktop>, from: String, to: String) -> Result<Node, String> {
    let guard = desktop.remote.lock().await;
    let remote = guard.as_ref().ok_or("not connected to a server")?;
    remote
        .rename(&from, &to)
        .await
        .map_err(|error| format!("{from}: {error}"))
}

#[tauri::command]
async fn delete_node(desktop: State<'_, Desktop>, path: String) -> Result<(), String> {
    let guard = desktop.remote.lock().await;
    let remote = guard.as_ref().ok_or("not connected to a server")?;
    remote
        .delete(&path)
        .await
        .map_err(|error| format!("{path}: {error}"))
}

#[tauri::command]
async fn list_trash(desktop: State<'_, Desktop>) -> Result<Vec<Trashed>, Failure> {
    let guard = desktop.remote.lock().await;
    let remote = guard.as_ref().ok_or("not connected to a server")?;
    Ok(remote.trash().await?)
}

#[tauri::command]
async fn restore_from_trash(desktop: State<'_, Desktop>, id: Uuid) -> Result<Node, Failure> {
    let guard = desktop.remote.lock().await;
    let remote = guard.as_ref().ok_or("not connected to a server")?;
    Ok(remote.restore(id).await?)
}

#[tauri::command]
async fn purge_from_trash(desktop: State<'_, Desktop>, id: Uuid) -> Result<(), Failure> {
    let guard = desktop.remote.lock().await;
    let remote = guard.as_ref().ok_or("not connected to a server")?;
    Ok(remote.purge(id).await?)
}

#[tauri::command]
async fn empty_trash(desktop: State<'_, Desktop>) -> Result<(), Failure> {
    let guard = desktop.remote.lock().await;
    let remote = guard.as_ref().ok_or("not connected to a server")?;
    Ok(remote.empty_trash().await?)
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .manage(Desktop::default())
        .invoke_handler(tauri::generate_handler![
            login,
            sign_out,
            list_folder,
            account,
            read_file,
            download_file,
            move_node,
            delete_node,
            list_trash,
            restore_from_trash,
            purge_from_trash,
            empty_trash,
            list_versions,
            download_version,
            restore_version,
            sync::pick_folder,
            sync::start_sync,
            sync::sync_control,
            sync::sync_status,
            check_update,
            install_update
        ])
        .run(tauri::generate_context!())
        .expect("starting the RoxyCloud window");
}
