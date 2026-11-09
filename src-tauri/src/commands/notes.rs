//! Markdown notes. Each change is recorded for other devices as it is
//! saved (see `db/notes.rs`); these commands then start a sync.

use tauri::State;

use crate::models::{
    CreateNoteRequest, Note, NoteLinks, NoteSave, NoteSearchHit, NoteSummary, NoteTitle,
    NotesStorage, UpdateNoteRequest,
};
use crate::{kick_replicated_sync, notes_export, run_database_task, AppResult, AppState};

#[tauri::command]
pub(crate) async fn list_recent_notes(
    limit: usize,
    state: State<'_, AppState>,
) -> AppResult<Vec<NoteSummary>> {
    let database = state.database.clone();
    run_database_task(move || database.list_recent_notes(limit)).await
}

#[tauri::command]
pub(crate) async fn list_trashed_notes(
    limit: usize,
    state: State<'_, AppState>,
) -> AppResult<Vec<NoteSummary>> {
    let database = state.database.clone();
    run_database_task(move || database.list_trashed_notes(limit)).await
}

#[tauri::command]
pub(crate) async fn get_note(id: String, state: State<'_, AppState>) -> AppResult<Option<Note>> {
    let database = state.database.clone();
    run_database_task(move || database.get_note(&id)).await
}

#[tauri::command]
pub(crate) async fn get_daily_note(
    date: String,
    state: State<'_, AppState>,
) -> AppResult<Option<Note>> {
    let database = state.database.clone();
    run_database_task(move || database.get_daily_note(&date)).await
}

#[tauri::command(async)]
pub(crate) fn create_note(
    request: CreateNoteRequest,
    state: State<'_, AppState>,
) -> AppResult<Note> {
    let note = state.database.create_note(&request.title, &request.body)?;
    kick_replicated_sync(&state);
    Ok(note)
}

#[tauri::command(async)]
pub(crate) fn update_note(
    request: UpdateNoteRequest,
    state: State<'_, AppState>,
) -> AppResult<NoteSave> {
    let outcome = state.database.save_note(
        &request.id,
        &request.title,
        &request.body,
        &request.expected_updated_at,
    )?;
    if outcome.saved {
        kick_replicated_sync(&state);
    }
    Ok(outcome)
}

#[tauri::command(async)]
pub(crate) fn save_daily_note(
    date: String,
    body: String,
    expected_updated_at: Option<String>,
    state: State<'_, AppState>,
) -> AppResult<NoteSave> {
    let outcome =
        state
            .database
            .save_daily_note_edit(&date, &body, expected_updated_at.as_deref())?;
    if outcome.saved {
        kick_replicated_sync(&state);
    }
    Ok(outcome)
}

#[tauri::command(async)]
pub(crate) fn trash_note(id: String, state: State<'_, AppState>) -> AppResult<Note> {
    let note = state.database.trash_note(&id)?;
    kick_replicated_sync(&state);
    Ok(note)
}

#[tauri::command(async)]
pub(crate) fn restore_note(id: String, state: State<'_, AppState>) -> AppResult<Note> {
    let note = state.database.restore_note(&id)?;
    kick_replicated_sync(&state);
    Ok(note)
}

#[tauri::command(async)]
pub(crate) fn delete_note(id: String, state: State<'_, AppState>) -> AppResult<()> {
    state.database.delete_note(&id)?;
    kick_replicated_sync(&state);
    Ok(())
}

#[tauri::command]
pub(crate) async fn record_note_visit(id: String, state: State<'_, AppState>) -> AppResult<()> {
    let database = state.database.clone();
    run_database_task(move || database.record_note_visit(&id)).await
}

#[tauri::command]
pub(crate) async fn search_notes(
    query: String,
    limit: usize,
    state: State<'_, AppState>,
) -> AppResult<Vec<NoteSearchHit>> {
    let database = state.database.clone();
    run_database_task(move || database.search_notes(&query, limit)).await
}

#[tauri::command]
pub(crate) async fn suggest_note_titles(
    query: String,
    limit: usize,
    state: State<'_, AppState>,
) -> AppResult<Vec<NoteTitle>> {
    let database = state.database.clone();
    run_database_task(move || database.suggest_note_titles(&query, limit)).await
}

#[tauri::command]
pub(crate) async fn find_note_by_title(
    title: String,
    state: State<'_, AppState>,
) -> AppResult<Option<NoteTitle>> {
    let database = state.database.clone();
    run_database_task(move || database.find_note_by_title(&title)).await
}

#[tauri::command]
pub(crate) async fn note_links(id: String, state: State<'_, AppState>) -> AppResult<NoteLinks> {
    let database = state.database.clone();
    run_database_task(move || database.note_links(&id)).await
}

/// Asks for the folder to export notes to, and says what it holds. The
/// folder stays here for [`notes_export_write`]; the page never names one.
#[tauri::command]
pub(crate) async fn notes_export_choose_folder(
    state: State<'_, AppState>,
) -> AppResult<Option<notes_export::ExportFolder>> {
    let Some(folder) = rfd::AsyncFileDialog::new()
        .set_title("Choose a folder to export notes to")
        .pick_folder()
        .await
    else {
        return Ok(None);
    };
    let path = folder.path().to_path_buf();
    let chosen = path.clone();
    let folder_state = tokio::task::spawn_blocking(move || notes_export::folder_state(&chosen))
        .await
        .map_err(|error| error.to_string())??;
    state.notes_export_folder.set(Some(path.clone()));
    Ok(Some(notes_export::ExportFolder {
        path: path.to_string_lossy().into_owned(),
        state: folder_state,
    }))
}

/// Writes every note outside Trash to the chosen folder. `replace` confirms
/// replacing an earlier export there.
#[tauri::command]
pub(crate) async fn notes_export_write(
    replace: bool,
    state: State<'_, AppState>,
) -> AppResult<notes_export::ExportOutcome> {
    let folder = state
        .notes_export_folder
        .get()
        .ok_or("Choose a folder to export notes to first")?;
    let database = state.database.clone();
    let notes = run_database_task(move || database.notes_for_export()).await?;
    let outcome =
        tokio::task::spawn_blocking(move || notes_export::export(&folder, &notes, replace))
            .await
            .map_err(|error| error.to_string())??;
    Ok(outcome)
}

#[tauri::command]
pub(crate) async fn notes_storage(state: State<'_, AppState>) -> AppResult<NotesStorage> {
    let database = state.database.clone();
    run_database_task(move || database.notes_storage()).await
}
