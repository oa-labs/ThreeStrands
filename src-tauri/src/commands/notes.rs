//! Markdown notes. Each change is recorded for other devices as it is
//! saved (see `db/notes.rs`); these commands then start a sync.

use tauri::State;

use crate::models::{
    CreateNoteRequest, Note, NoteLinks, NoteSave, NoteSearchHit, NoteSummary, NotesStorage,
    UpdateNoteRequest,
};
use crate::{kick_replicated_sync, run_database_task, AppResult, AppState};

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
pub(crate) async fn note_links(id: String, state: State<'_, AppState>) -> AppResult<NoteLinks> {
    let database = state.database.clone();
    run_database_task(move || database.note_links(&id)).await
}

#[tauri::command]
pub(crate) async fn notes_storage(state: State<'_, AppState>) -> AppResult<NotesStorage> {
    let database = state.database.clone();
    run_database_task(move || database.notes_storage()).await
}
