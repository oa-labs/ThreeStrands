//! Meeting Recording commands. Settings here are device-local: they are
//! stored in this device's database only and never enter settings transfer
//! or sync.
//!
//! Recording work can block (starting or stopping a track waits for its
//! capture thread; deleting removes files), so each command runs on the
//! blocking pool.
use tauri::{Emitter, State};

use crate::db::recordings::MeetingRecording;
use crate::note_entities::NoteReference;
use crate::recording::capture::{AudioInputDevice, SystemAudioSupport};
use crate::recording::permissions::{self, RecordingPermissions};
use crate::recording::policy;
use crate::recording::runtime::RecordingTranscript;
use crate::recording::runtime::{RecordingCapabilities, RecordingsUsage};
use crate::recording::settings::{RecordingSettings, TranscriptionModelChoice};
use crate::recording::transcribe::models::TranscriptionModelStatus;
use crate::recording::RecordingStatus;
use crate::{run_database_task, AppResult, AppState};

#[tauri::command]
pub(crate) async fn recording_settings(state: State<'_, AppState>) -> AppResult<RecordingSettings> {
    let database = state.database.clone();
    run_database_task(move || database.recording_settings()).await
}

/// Saves the settings and shows or hides the tray icon to match.
#[tauri::command]
pub(crate) async fn set_recording_settings(
    settings: RecordingSettings,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let runtime = state.recording.clone();
    run_database_task(move || runtime.set_settings(settings)).await
}

#[tauri::command]
pub(crate) async fn recording_capabilities(
    state: State<'_, AppState>,
) -> AppResult<RecordingCapabilities> {
    Ok(state.recording.capabilities())
}

#[tauri::command]
pub(crate) async fn recording_input_devices(
    state: State<'_, AppState>,
) -> AppResult<Vec<AudioInputDevice>> {
    let runtime = state.recording.clone();
    run_database_task(move || Ok::<_, String>(runtime.input_devices())).await
}

#[tauri::command]
pub(crate) async fn open_system_audio_settings() -> AppResult<()> {
    #[cfg(target_os = "macos")]
    {
        open::that(policy::SYSTEM_AUDIO_SETTINGS_URL).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn recording_status(state: State<'_, AppState>) -> AppResult<RecordingStatus> {
    Ok(state.recording.status())
}

#[tauri::command]
pub(crate) async fn recording_start(
    event_ref: Option<NoteReference>,
    title: String,
    attendees: Option<Vec<String>>,
    state: State<'_, AppState>,
) -> AppResult<RecordingStatus> {
    let runtime = state.recording.clone();
    let attendees = attendees.unwrap_or_default();
    run_database_task(move || runtime.start_meeting(event_ref, &title, &attendees)).await
}

#[tauri::command]
pub(crate) async fn recording_pause(state: State<'_, AppState>) -> AppResult<RecordingStatus> {
    let runtime = state.recording.clone();
    run_database_task(move || runtime.pause()).await
}

#[tauri::command]
pub(crate) async fn recording_resume(state: State<'_, AppState>) -> AppResult<RecordingStatus> {
    let runtime = state.recording.clone();
    run_database_task(move || runtime.resume()).await
}

#[tauri::command]
pub(crate) async fn recording_stop(state: State<'_, AppState>) -> AppResult<RecordingStatus> {
    let runtime = state.recording.clone();
    run_database_task(move || runtime.stop()).await
}

#[tauri::command]
pub(crate) async fn list_recordings(
    event_ref: NoteReference,
    state: State<'_, AppState>,
) -> AppResult<Vec<MeetingRecording>> {
    let runtime = state.recording.clone();
    run_database_task(move || runtime.list_recordings(&event_ref)).await
}

#[tauri::command]
pub(crate) async fn delete_recording(id: String, state: State<'_, AppState>) -> AppResult<()> {
    let runtime = state.recording.clone();
    run_database_task(move || runtime.delete_recording(&id)).await
}

#[tauri::command]
pub(crate) async fn delete_recording_audio(
    id: String,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let runtime = state.recording.clone();
    run_database_task(move || runtime.delete_recording_audio(&id)).await
}

#[tauri::command]
pub(crate) async fn recordings_usage(state: State<'_, AppState>) -> AppResult<RecordingsUsage> {
    let runtime = state.recording.clone();
    run_database_task(move || runtime.usage()).await
}

#[tauri::command]
pub(crate) async fn delete_all_recording_audio(state: State<'_, AppState>) -> AppResult<()> {
    let runtime = state.recording.clone();
    run_database_task(move || runtime.delete_all_audio()).await
}

#[tauri::command]
pub(crate) async fn delete_all_recordings(state: State<'_, AppState>) -> AppResult<()> {
    let runtime = state.recording.clone();
    run_database_task(move || runtime.delete_all_recordings()).await
}

/// Asks for microphone permission (waiting for the user's answer) and, where
/// system audio is available, makes macOS ask for it now too.
#[tauri::command]
pub(crate) async fn request_recording_permissions(
    state: State<'_, AppState>,
) -> AppResult<RecordingPermissions> {
    let system_audio = state.recording.capabilities().system_audio == SystemAudioSupport::Available;
    run_database_task(move || Ok::<_, String>(permissions::request(system_audio))).await
}

/// The user confirmed quitting during a recording: stop it, then continue
/// the usual exit handshake (`compose-before-exit`, then `finish_exit`).
#[tauri::command]
pub(crate) async fn recording_confirm_quit(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let runtime = state.recording.clone();
    run_database_task(move || {
        runtime.stop_if_active();
        Ok::<_, String>(())
    })
    .await?;
    let _ = app.emit("compose-before-exit", ());
    Ok(())
}

#[tauri::command]
pub(crate) async fn transcription_models(
    state: State<'_, AppState>,
) -> AppResult<Vec<TranscriptionModelStatus>> {
    let runtime = state.recording.clone();
    run_database_task(move || runtime.transcription_models()).await
}

/// Starts the download in the background and returns.
#[tauri::command]
pub(crate) async fn download_transcription_model(
    model: TranscriptionModelChoice,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let runtime = state.recording.clone();
    run_database_task(move || runtime.download_transcription_model(model).map(drop)).await
}

#[tauri::command]
pub(crate) async fn cancel_transcription_model_download(
    model: TranscriptionModelChoice,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let runtime = state.recording.clone();
    run_database_task(move || runtime.cancel_transcription_model_download(model)).await
}

#[tauri::command]
pub(crate) async fn delete_transcription_model(
    model: TranscriptionModelChoice,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let runtime = state.recording.clone();
    run_database_task(move || runtime.delete_transcription_model(model)).await
}

/// Queues a recording for transcription (Retry).
#[tauri::command]
pub(crate) async fn transcribe_recording(id: String, state: State<'_, AppState>) -> AppResult<()> {
    let runtime = state.recording.clone();
    run_database_task(move || runtime.transcribe_recording(&id)).await
}

#[tauri::command]
pub(crate) async fn recording_transcript(
    id: String,
    state: State<'_, AppState>,
) -> AppResult<RecordingTranscript> {
    let runtime = state.recording.clone();
    run_database_task(move || runtime.recording_transcript(&id)).await
}
