mod auth;
mod correspondence;
mod db;
mod gmail;
mod mime;
mod models;
mod sync;

use std::sync::Arc;

use auth::GoogleAuth;
use db::Database;
use models::{
    AuthStatus, CreateLabelRequest, Label, SearchThreadsRequest, SyncStatus, Thread, ThreadDetail,
    ThreadMutation, UpdateLabelRequest,
};
use sync::SyncService;
use tauri::{Manager, State};

struct AppState {
    database: Arc<Database>,
    auth: Option<GoogleAuth>,
    sync: Option<SyncService>,
    correspondence: correspondence::Correspondence,
    exiting: std::sync::atomic::AtomicBool,
}

#[tauri::command]
async fn correspondence_request(
    request: correspondence::Request,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    state.correspondence.request(request).await
}

#[tauri::command]
async fn finish_exit(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    // Give pending undo windows time to complete while keeping the UI responsive.
    while state.database.pending_undo()? {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    // Do not terminate while a provider request is awaiting acknowledgement.
    let _send_guard = state.correspondence.gate.lock().await;
    state
        .exiting
        .store(true, std::sync::atomic::Ordering::SeqCst);
    app.exit(0);
    Ok(())
}

#[tauri::command]
fn list_threads(state: State<'_, AppState>) -> Result<Vec<Thread>, String> {
    state.database.list_threads()
}

#[tauri::command]
fn get_thread(id: String, state: State<'_, AppState>) -> Result<ThreadDetail, String> {
    state.database.get_thread(&id)
}

#[tauri::command]
fn search_threads(
    request: SearchThreadsRequest,
    state: State<'_, AppState>,
) -> Result<Vec<Thread>, String> {
    state.database.search_threads(&request)
}

#[tauri::command]
fn mutate_thread(mutation: ThreadMutation, state: State<'_, AppState>) -> Result<(), String> {
    state.database.mutate_thread(&mutation)
}

#[tauri::command]
fn sync_status(state: State<'_, AppState>) -> Result<SyncStatus, String> {
    state.database.sync_status()
}

#[tauri::command]
async fn sync_account(state: State<'_, AppState>) -> Result<SyncStatus, String> {
    let _ = state.correspondence.refresh_identity().await;
    state.sync.as_ref().ok_or_else(not_configured)?.sync().await
}

#[tauri::command]
async fn flush_pending_mutations(state: State<'_, AppState>) -> Result<SyncStatus, String> {
    match state.sync.as_ref() {
        Some(service) => service.flush_pending().await,
        None => state.database.sync_status(),
    }
}

fn spawn_pending_flush(handle: &tauri::AppHandle) {
    if !GoogleAuth::available() {
        return;
    }
    let Some(state) = handle.try_state::<AppState>() else {
        return;
    };
    let Some(service) = state.sync.clone() else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        // Wait for an in-flight mutate_thread IPC to land in SQLite.
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        let _ = service.flush_pending().await;
    });
}

#[tauri::command]
fn google_auth_status(state: State<'_, AppState>) -> AuthStatus {
    AuthStatus {
        configured: state.auth.is_some(),
        connected: GoogleAuth::available(),
    }
}

#[tauri::command]
async fn connect_google(state: State<'_, AppState>) -> Result<SyncStatus, String> {
    let _guard = state.correspondence.gate.lock().await;
    state
        .auth
        .as_ref()
        .ok_or_else(not_configured)?
        .authorize()
        .await?;
    let _ = state.correspondence.refresh_identity().await;
    state.sync.as_ref().ok_or_else(not_configured)?.sync().await
}

#[tauri::command]
async fn disconnect_google(state: State<'_, AppState>) -> Result<(), String> {
    let _guard = state.correspondence.gate.lock().await;
    state.database.pause_ready_sends()?;
    GoogleAuth::disconnect()
}

#[tauri::command]
async fn list_labels(state: State<'_, AppState>) -> Result<Vec<Label>, String> {
    state
        .sync
        .as_ref()
        .ok_or_else(not_configured)?
        .labels()
        .await
}

#[tauri::command]
async fn create_label(
    request: CreateLabelRequest,
    state: State<'_, AppState>,
) -> Result<Label, String> {
    state
        .sync
        .as_ref()
        .ok_or_else(not_configured)?
        .create_label(&request.name)
        .await
}

#[tauri::command]
async fn update_label(
    request: UpdateLabelRequest,
    state: State<'_, AppState>,
) -> Result<Label, String> {
    state
        .sync
        .as_ref()
        .ok_or_else(not_configured)?
        .update_label(&request.id, &request.name)
        .await
}

#[tauri::command]
async fn delete_label(id: String, state: State<'_, AppState>) -> Result<(), String> {
    state
        .sync
        .as_ref()
        .ok_or_else(not_configured)?
        .delete_label(&id)
        .await
}

fn not_configured() -> String {
    "Google OAuth is not configured. Set DISPATCH_GOOGLE_CLIENT_ID and \
     DISPATCH_GOOGLE_CLIENT_SECRET from a Desktop app credential."
        .into()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::SIZE
                        | tauri_plugin_window_state::StateFlags::MAXIMIZED,
                )
                .build(),
        )
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .map_err(|error| format!("Unable to find app data directory: {error}"))?;
            std::fs::create_dir_all(&data_dir)?;
            let database = Arc::new(
                Database::open(&data_dir.join("dispatch.sqlite"))
                    .map_err(|error| format!("Unable to open local database: {error}"))?,
            );
            let auth = GoogleAuth::from_environment().ok();
            let sync = auth
                .as_ref()
                .map(|auth| SyncService::new(database.clone(), auth.clone()));
            if let Some(service) = sync.clone() {
                tauri::async_runtime::spawn(async move {
                    if GoogleAuth::available() {
                        let _ = service.sync().await;
                    }
                    service.polling_loop().await;
                });
            }
            let root = data_dir.join("attachments");
            std::fs::create_dir_all(&root)?;
            let correspondence = correspondence::Correspondence {
                database: database.clone(),
                auth: auth.clone(),
                root,
                gate: Arc::new(tokio::sync::Mutex::new(())),
                edits: Arc::new(tokio::sync::Mutex::new(())),
            };
            let worker = correspondence.clone();
            tauri::async_runtime::spawn(async move {
                if GoogleAuth::available() {
                    let _ = worker.refresh_identity().await;
                }
                worker.run().await;
            });
            app.manage(AppState {
                database,
                auth,
                sync,
                correspondence,
                exiting: std::sync::atomic::AtomicBool::new(false),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            correspondence_request,
            finish_exit,
            list_threads,
            get_thread,
            search_threads,
            mutate_thread,
            sync_status,
            sync_account,
            flush_pending_mutations,
            google_auth_status,
            connect_google,
            disconnect_google,
            list_labels,
            create_label,
            update_label,
            delete_label,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Dispatch")
        .run(|handle, event| {
            use tauri::Emitter;
            match &event {
                tauri::RunEvent::WindowEvent {
                    event: tauri::WindowEvent::CloseRequested { api, .. },
                    ..
                } => {
                    if let Some(state) = handle.try_state::<AppState>() {
                        if !state.exiting.load(std::sync::atomic::Ordering::SeqCst) {
                            api.prevent_close();
                            let _ = handle.emit("compose-before-exit", ());
                        }
                    }
                }
                tauri::RunEvent::WindowEvent {
                    event: tauri::WindowEvent::Focused(false),
                    ..
                } => spawn_pending_flush(handle),
                tauri::RunEvent::ExitRequested { api, .. } => {
                    if let Some(state) = handle.try_state::<AppState>() {
                        if !state.exiting.load(std::sync::atomic::Ordering::SeqCst) {
                            api.prevent_exit();
                            let _ = handle.emit("compose-before-exit", ());
                        }
                    }
                }
                _ => {}
            }
            if matches!(event, tauri::RunEvent::Resumed) {
                if let Some(state) = handle.try_state::<AppState>() {
                    if GoogleAuth::available() {
                        if let Some(service) = state.sync.clone() {
                            tauri::async_runtime::spawn(async move {
                                let _ = service.sync().await;
                            });
                        }
                    }
                }
            }
        });
}
