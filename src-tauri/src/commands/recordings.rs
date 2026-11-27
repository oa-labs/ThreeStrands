//! Meeting Recording commands. Settings here are device-local: they are
//! stored in this device's database only and never enter settings transfer
//! or sync.
use tauri::State;

use crate::recording::settings::RecordingSettings;
use crate::{run_database_task, AppResult, AppState};

#[tauri::command]
pub(crate) async fn recording_settings(state: State<'_, AppState>) -> AppResult<RecordingSettings> {
    let database = state.database.clone();
    run_database_task(move || database.recording_settings()).await
}

#[tauri::command]
pub(crate) async fn set_recording_settings(
    settings: RecordingSettings,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let database = state.database.clone();
    run_database_task(move || database.set_recording_settings(&settings)).await
}
