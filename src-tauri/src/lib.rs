mod db;
mod models;

use db::Database;
use models::{SearchThreadsRequest, SyncStatus, Thread, ThreadDetail, ThreadMutation};
use tauri::{Manager, State};

#[tauri::command]
fn list_threads(database: State<'_, Database>) -> Result<Vec<Thread>, String> {
    database.list_threads()
}

#[tauri::command]
fn get_thread(id: String, database: State<'_, Database>) -> Result<ThreadDetail, String> {
    database.get_thread(&id)
}

#[tauri::command]
fn search_threads(
    request: SearchThreadsRequest,
    database: State<'_, Database>,
) -> Result<Vec<Thread>, String> {
    database.search_threads(&request)
}

#[tauri::command]
fn mutate_thread(
    mutation: ThreadMutation,
    database: State<'_, Database>,
) -> Result<(), String> {
    database.mutate_thread(&mutation)
}

#[tauri::command]
fn sync_status(database: State<'_, Database>) -> Result<SyncStatus, String> {
    database.sync_status()
}

#[tauri::command]
fn sync_account(database: State<'_, Database>) -> Result<SyncStatus, String> {
    // The Phase 1 shell proves the local synchronization boundary. Gmail OAuth,
    // history retrieval, and outbox reconciliation attach here next.
    database.record_local_sync()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .map_err(|error| format!("Unable to find app data directory: {error}"))?;
            std::fs::create_dir_all(&data_dir)?;
            let database = Database::open(&data_dir.join("dispatch.sqlite"))
                .map_err(|error| format!("Unable to open local database: {error}"))?;
            app.manage(database);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_threads,
            get_thread,
            search_threads,
            mutate_thread,
            sync_status,
            sync_account,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Dispatch");
}
