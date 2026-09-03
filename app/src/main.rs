use roxycloud_client::Remote;
use roxycloud_core::node::Node;
use tokio::sync::Mutex;

#[derive(Default)]
struct Session {
    remote: Mutex<Option<Remote>>,
}

#[tauri::command]
async fn login(
    session: tauri::State<'_, Session>,
    server: String,
    email: String,
    password: String,
) -> Result<(), String> {
    let (remote, _) = Remote::login(&server, &email, &password)
        .await
        .map_err(|error| error.to_string())?;
    *session.remote.lock().await = Some(remote);
    Ok(())
}

#[tauri::command]
async fn list_folder(
    session: tauri::State<'_, Session>,
    path: String,
) -> Result<Vec<Node>, String> {
    let guard = session.remote.lock().await;
    let remote = guard.as_ref().ok_or("not connected to a server")?;
    remote
        .list(&path)
        .await
        .map_err(move |error| format!("{path}: {error}"))
}

fn main() {
    tauri::Builder::default()
        .manage(Session::default())
        .invoke_handler(tauri::generate_handler![login, list_folder])
        .run(tauri::generate_context!())
        .expect("starting the RoxyCloud window");
}
