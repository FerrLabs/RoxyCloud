mod failure;
mod keychain;
mod sync;

use std::path::PathBuf;

use stashden_client::sync::watch::Session as SyncSession;
use stashden_client::{Remote, RemoteError, free_path};
use stashden_core::grant::{Given, NewGrant, Received};
use stashden_core::node::{Node, Trashed};
use stashden_core::share::{Minted, NewShare, Share};
use stashden_core::user::User;
use stashden_core::version::Version;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_updater::UpdaterExt;
use tokio::sync::Mutex;

use failure::Failure;
use keychain::Credentials;
use uuid::Uuid;

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
) -> Result<Option<String>, String> {
    let (session, _) = Remote::login(&server, &email, &password)
        .await
        .map_err(|error| error.to_string())?;
    let minted = session
        .mint_app_password(&format!("Stashden desktop on {}", this_computer()))
        .await
        .map_err(|error| error.to_string())?;

    let credentials = Credentials {
        server,
        email,
        secret: minted.secret,
    };
    connect(&desktop, credentials.clone()).await?;
    Ok(keychain::keep(credentials)
        .await
        .err()
        .map(|reason| format!("Signed in until the app closes, then it will ask again: {reason}")))
}

#[tauri::command]
async fn resume(desktop: State<'_, Desktop>) -> Result<bool, String> {
    let Some(credentials) = keychain::kept().await? else {
        return Ok(false);
    };
    connect(&desktop, credentials).await?;
    Ok(true)
}

async fn connect(desktop: &Desktop, credentials: Credentials) -> Result<(), String> {
    let remote = remote_for(&credentials).map_err(|error| error.to_string())?;
    *desktop.remote.lock().await = Some(remote);
    *desktop.credentials.lock().await = Some(credentials);
    Ok(())
}

fn remote_for(credentials: &Credentials) -> Result<Remote, RemoteError> {
    Remote::with_app_password(
        &credentials.server,
        credentials.email.as_str(),
        credentials.secret.as_str(),
    )
}

fn this_computer() -> String {
    let name = gethostname::gethostname().to_string_lossy().into_owned();
    if name.is_empty() {
        "this computer".to_owned()
    } else {
        name
    }
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
    desktop.credentials.lock().await.take();
    let revoked = match desktop.remote.lock().await.take() {
        Some(remote) => match remote.revoke_own_app_password().await {
            Ok(()) | Err(RemoteError::Unauthenticated) => Ok(()),
            Err(error) => Err(format!(
                "the server did not revoke this computer's app password, revoke it from the web app: {error}"
            )),
        },
        None => Ok(()),
    };
    keychain::forget().await?;
    revoked
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

fn into_downloads(app: &AppHandle, path: &str) -> Result<PathBuf, String> {
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

#[tauri::command]
async fn list_shares(desktop: State<'_, Desktop>) -> Result<Vec<Share>, Failure> {
    let guard = desktop.remote.lock().await;
    let remote = guard.as_ref().ok_or("not connected to a server")?;
    Ok(remote.list_shares().await?)
}

#[tauri::command]
async fn share(desktop: State<'_, Desktop>, request: NewShare) -> Result<Minted, Failure> {
    let guard = desktop.remote.lock().await;
    let remote = guard.as_ref().ok_or("not connected to a server")?;
    Ok(remote.share(&request).await?)
}

#[tauri::command]
async fn revoke_share(desktop: State<'_, Desktop>, id: Uuid) -> Result<(), Failure> {
    let guard = desktop.remote.lock().await;
    let remote = guard.as_ref().ok_or("not connected to a server")?;
    Ok(remote.revoke_share(id).await?)
}

#[tauri::command]
async fn list_grants(desktop: State<'_, Desktop>) -> Result<Vec<Given>, Failure> {
    let guard = desktop.remote.lock().await;
    let remote = guard.as_ref().ok_or("not connected to a server")?;
    Ok(remote.list_grants().await?)
}

#[tauri::command]
async fn grant(desktop: State<'_, Desktop>, request: NewGrant) -> Result<Given, Failure> {
    let guard = desktop.remote.lock().await;
    let remote = guard.as_ref().ok_or("not connected to a server")?;
    Ok(remote.grant(&request).await?)
}

#[tauri::command]
async fn received_grants(desktop: State<'_, Desktop>) -> Result<Vec<Received>, Failure> {
    let guard = desktop.remote.lock().await;
    let remote = guard.as_ref().ok_or("not connected to a server")?;
    Ok(remote.received().await?)
}

#[tauri::command]
async fn withdraw_grant(desktop: State<'_, Desktop>, id: Uuid) -> Result<(), Failure> {
    let guard = desktop.remote.lock().await;
    let remote = guard.as_ref().ok_or("not connected to a server")?;
    Ok(remote.withdraw_grant(id).await?)
}

#[derive(serde::Serialize)]
struct Picked {
    name: String,
    source: PathBuf,
}

impl From<PathBuf> for Picked {
    fn from(source: PathBuf) -> Self {
        let name = source.file_name().map_or_else(
            || source.to_string_lossy().into_owned(),
            |name| name.to_string_lossy().into_owned(),
        );
        Self { name, source }
    }
}

#[tauri::command]
async fn pick_uploads(app: AppHandle) -> Result<Vec<Picked>, Failure> {
    let (chosen, answer) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_files(move |files| {
        let _ = chosen.send(files);
    });

    let files = answer
        .await
        .map_err(|_| "the file picker went away")?
        .unwrap_or_default();
    files
        .into_iter()
        .map(|file| {
            file.into_path().map(Picked::from).map_err(|error| {
                Failure::from(format!(
                    "that file is not one this computer can open: {error}"
                ))
            })
        })
        .collect()
}

#[tauri::command]
fn describe_drops(paths: Vec<PathBuf>) -> Vec<Picked> {
    paths.into_iter().map(Picked::from).collect()
}

#[tauri::command]
async fn upload_file(
    desktop: State<'_, Desktop>,
    path: String,
    source: PathBuf,
) -> Result<Node, Failure> {
    if source.is_dir() {
        return Err(Failure::from(format!(
            "{} is a folder; upload the files inside it, or sync the folder",
            source.display()
        )));
    }
    let guard = desktop.remote.lock().await;
    let remote = guard.as_ref().ok_or("not connected to a server")?;
    Ok(remote.upload(&path, &source).await?)
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .manage(Desktop::default())
        .invoke_handler(tauri::generate_handler![
            login,
            resume,
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
            list_shares,
            share,
            revoke_share,
            list_grants,
            grant,
            received_grants,
            withdraw_grant,
            pick_uploads,
            describe_drops,
            upload_file,
            sync::pick_folder,
            sync::start_sync,
            sync::sync_control,
            sync::sync_status,
            check_update,
            install_update
        ])
        .run(tauri::generate_context!())
        .expect("starting the Stashden window");
}
