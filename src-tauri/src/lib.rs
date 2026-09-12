mod ai;
mod auth;
mod correspondence;
mod db;
mod gmail;
mod mime;
mod models;
mod sync;

use std::collections::HashMap;
use std::sync::Arc;

use auth::{GoogleAuth, GoogleAuthConfig};
use db::Database;
use models::{
    Account, AuthStatus, CreateLabelRequest, Label, SearchThreadsRequest, SyncStatus, Thread,
    ThreadDetail, ThreadMutation, UpdateLabelRequest,
};
use sync::SyncService;
use tauri::{async_runtime::JoinHandle, Manager, State};

/// An account beyond the primary: its own credentials and the task running
/// its own sync loop. Removing the account aborts `poll_task`.
struct ConnectedAccount {
    auth: GoogleAuth,
    poll_task: JoinHandle<()>,
}

struct AppState {
    database: Arc<Database>,
    /// Shared OAuth app credentials, used to authorize any account.
    auth_config: Option<GoogleAuthConfig>,
    /// The account driving the compose/send pipeline and today's
    /// single-account UI (`google_auth_status`/`connect_google`/
    /// `disconnect_google`, and every label/list command below). Which
    /// email this is comes from the `accounts` table when one already
    /// exists there (any run after the first), or starts at a placeholder
    /// that rekeys onto the real address on first connect otherwise.
    auth: Option<GoogleAuth>,
    sync: Option<SyncService>,
    /// Every other connected account, keyed by email — each with its own
    /// sync cursor and polling loop, independent of the primary and of each
    /// other. Populated at startup from the `accounts` table and by
    /// `add_account`.
    additional_accounts: Arc<tokio::sync::Mutex<HashMap<String, ConnectedAccount>>>,
    correspondence: correspondence::Correspondence,
    exiting: std::sync::atomic::AtomicBool,
}

fn primary_account_id(state: &AppState) -> String {
    state
        .auth
        .as_ref()
        .map(GoogleAuth::key)
        .unwrap_or_else(|| "default".into())
}

/// Builds a `SyncService` for `auth`, runs an immediate sync if it's already
/// connected, and spawns its polling loop — the same startup behavior the
/// primary account gets, generalized so any account can get it.
fn spawn_synced_account(database: Arc<Database>, auth: GoogleAuth) -> ConnectedAccount {
    let service = SyncService::new(database, auth.clone());
    let poll_task = tauri::async_runtime::spawn(async move {
        if service.is_connected() {
            let _ = service.sync().await;
        }
        service.polling_loop().await;
    });
    ConnectedAccount { auth, poll_task }
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
    state.database.sync_status(&primary_account_id(&state))
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
        None => state.database.sync_status(&primary_account_id(&state)),
    }
}

fn spawn_pending_flush(handle: &tauri::AppHandle) {
    let Some(state) = handle.try_state::<AppState>() else {
        return;
    };
    let Some(service) = state.sync.clone() else {
        return;
    };
    if !service.is_connected() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        // Wait for an in-flight mutate_thread IPC to land in SQLite.
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        let _ = service.flush_pending().await;
    });
}

fn spawn_foreground_sync(handle: &tauri::AppHandle) {
    let Some(state) = handle.try_state::<AppState>() else {
        return;
    };
    let Some(service) = state.sync.clone() else {
        return;
    };
    if !service.is_connected() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let _ = service.sync_if_stale().await;
    });
}

#[tauri::command]
fn google_auth_status(state: State<'_, AppState>) -> AuthStatus {
    AuthStatus {
        configured: state.auth.is_some(),
        connected: state.auth.as_ref().is_some_and(GoogleAuth::available),
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
    let email = state.auth.as_ref().ok_or_else(not_configured)?.key();
    remove_account(email, state).await
}

#[tauri::command]
fn list_accounts(state: State<'_, AppState>) -> Result<Vec<Account>, String> {
    state.database.list_accounts()
}

#[tauri::command]
async fn add_account(state: State<'_, AppState>) -> Result<Account, String> {
    let _guard = state.correspondence.gate.lock().await;
    let config = state.auth_config.as_ref().ok_or_else(not_configured)?;
    let auth = config.pending_account();
    let email = auth.authorize().await?;
    let account = state.database.adopt_account(&email)?;
    let connected = spawn_synced_account(state.database.clone(), auth);
    state
        .additional_accounts
        .lock()
        .await
        .insert(email, connected);
    Ok(account)
}

#[tauri::command]
async fn remove_account(email: String, state: State<'_, AppState>) -> Result<(), String> {
    let _guard = state.correspondence.gate.lock().await;
    let config = state.auth_config.as_ref().ok_or_else(not_configured)?;
    state.database.pause_ready_sends_for(&email)?;
    let removed = state.additional_accounts.lock().await.remove(&email);
    match (&state.auth, removed) {
        (Some(primary), _) if primary.key() == email => primary.disconnect()?,
        (_, Some(connected)) => {
            connected.poll_task.abort();
            connected.auth.disconnect()?;
        }
        _ => config.account(&email).disconnect()?,
    }
    state.database.remove_account(&email)
}

#[tauri::command]
async fn reconnect_account(email: String, state: State<'_, AppState>) -> Result<Account, String> {
    let _guard = state.correspondence.gate.lock().await;
    let config = state.auth_config.as_ref().ok_or_else(not_configured)?;
    let is_primary = state
        .auth
        .as_ref()
        .is_some_and(|primary| primary.key() == email);
    let auth = if is_primary {
        state.auth.clone().expect("checked by is_primary")
    } else {
        match state.additional_accounts.lock().await.get(&email) {
            Some(connected) => connected.auth.clone(),
            None => config.account(&email),
        }
    };
    auth.authorize().await?;
    if !is_primary {
        // Self-heal: an account already in the `accounts` table should
        // always have a live poller from startup, but reconnecting is a
        // reasonable place to notice and repair a missing one.
        let mut accounts = state.additional_accounts.lock().await;
        if !accounts.contains_key(&email) {
            let connected = spawn_synced_account(state.database.clone(), auth);
            accounts.insert(email.clone(), connected);
        }
    }
    state.database.adopt_account(&email)
}

#[tauri::command]
fn set_account_color(
    email: String,
    color: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.database.set_account_color(&email, &color)
}

#[tauri::command]
fn reorder_accounts(emails: Vec<String>, state: State<'_, AppState>) -> Result<(), String> {
    state.database.reorder_accounts(&emails)
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

#[tauri::command]
fn ai_api_key_configured() -> bool {
    ai::configured()
}

#[tauri::command]
fn set_ai_api_key(key: String) -> Result<(), String> {
    ai::set(&key)
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
            let auth_config = GoogleAuthConfig::from_environment().ok();
            // The primary account, used by the single sync loop and
            // correspondence pipeline. Any run after the first already has
            // it in the `accounts` table (rekeyed onto its real address by a
            // previous run), so construct it already keyed correctly rather
            // than restarting at the placeholder key each launch — that
            // placeholder only applies before the very first successful
            // connect/migration.
            let existing_primary = database
                .list_accounts()
                .ok()
                .and_then(|accounts| accounts.into_iter().next());
            let auth = auth_config.as_ref().map(|config| match &existing_primary {
                Some(account) => config.account(&account.email),
                None => config.legacy_account(),
            });
            let sync = auth
                .as_ref()
                .map(|auth| SyncService::new(database.clone(), auth.clone()));
            if let Some(service) = sync.clone() {
                tauri::async_runtime::spawn(async move {
                    if service.is_connected() {
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
            let additional_accounts = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
            let worker = correspondence.clone();
            {
                let database = database.clone();
                let auth_config = auth_config.clone();
                let additional_accounts = additional_accounts.clone();
                tauri::async_runtime::spawn(async move {
                    if worker.is_connected() {
                        let _ = worker.refresh_identity().await;
                    }
                    // Bring up every other already-connected account's own
                    // sync loop. Sequenced after identity resolution above
                    // so the primary's now-final key can be excluded here.
                    if let Some(config) = &auth_config {
                        let primary_email = worker.auth.as_ref().map(GoogleAuth::key);
                        if let Ok(accounts) = database.list_accounts() {
                            for account in accounts {
                                if Some(&account.email) == primary_email.as_ref() {
                                    continue;
                                }
                                let connected = spawn_synced_account(
                                    database.clone(),
                                    config.account(&account.email),
                                );
                                additional_accounts
                                    .lock()
                                    .await
                                    .insert(account.email, connected);
                            }
                        }
                    }
                    worker.run().await;
                });
            }
            app.manage(AppState {
                database,
                auth_config,
                auth,
                sync,
                additional_accounts,
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
            list_accounts,
            add_account,
            remove_account,
            reconnect_account,
            set_account_color,
            reorder_accounts,
            list_labels,
            create_label,
            update_label,
            delete_label,
            ai_api_key_configured,
            set_ai_api_key,
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
                    event: tauri::WindowEvent::Focused(focused),
                    ..
                } => {
                    if *focused {
                        spawn_foreground_sync(handle);
                    } else {
                        spawn_pending_flush(handle);
                    }
                }
                tauri::RunEvent::ExitRequested { api, .. } => {
                    if let Some(state) = handle.try_state::<AppState>() {
                        if !state.exiting.load(std::sync::atomic::Ordering::SeqCst) {
                            api.prevent_exit();
                            let _ = handle.emit("compose-before-exit", ());
                        }
                    }
                }
                tauri::RunEvent::Resumed => spawn_foreground_sync(handle),
                _ => {}
            }
        });
}
