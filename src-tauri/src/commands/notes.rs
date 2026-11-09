//! Markdown notes. Each change is recorded for other devices as it is
//! saved (see `db/notes.rs`); these commands then start a sync.

use tauri::State;

use crate::models::{
    CreateNoteRequest, Note, NoteAttachment, NoteLinks, NoteSave, NoteSearchHit, NoteSummary,
    NoteTitle, NotesStorage, UpdateNoteRequest,
};
use crate::{
    attachment_security, file_preview, image_format, kick_replicated_sync, notes_export,
    notes_import, run_database_task, AppError, AppResult, AppState,
};

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
    // Text saves while typing ride the next periodic sync (every 30
    // seconds) instead of each re-uploading the snapshot; quitting syncs
    // once more (`finish_exit`).
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
    // Like `update_note`, this waits for the next periodic sync.
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
    let (notes, attachments) = run_database_task(move || {
        Ok::<_, crate::db::DatabaseError>((
            database.notes_for_export()?,
            database.note_attachments_for_export()?,
        ))
    })
    .await?;
    let database = state.database.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        notes_export::export(
            &folder,
            &notes,
            &attachments,
            |id| {
                database
                    .note_attachment_bytes(id)
                    .map(|found| found.map(|(_, bytes)| bytes))
                    .map_err(|error| error.to_string())
            },
            replace,
        )
    })
    .await
    .map_err(|error| error.to_string())??;
    Ok(outcome)
}

#[tauri::command]
pub(crate) async fn notes_storage(state: State<'_, AppState>) -> AppResult<NotesStorage> {
    let database = state.database.clone();
    run_database_task(move || database.notes_storage()).await
}

/// A request header's text: percent-encoded by the page, so a file name
/// in any script survives the header's ASCII-only rules.
fn header_text(request: &tauri::ipc::Request<'_>, name: &str) -> AppResult<String> {
    let value = request
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| AppError::message(format!("The {name} header is missing")))?;
    Ok(percent_encoding::percent_decode_str(value)
        .decode_utf8()
        .map_err(|_| AppError::message(format!("The {name} header isn't text")))?
        .into_owned())
}

/// Stores a file pasted or dropped into a note. The file's bytes are the
/// request body; the note, file name, and type travel in headers.
#[tauri::command]
pub(crate) async fn note_attachment_add(
    request: tauri::ipc::Request<'_>,
    state: State<'_, AppState>,
) -> AppResult<NoteAttachment> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err(AppError::message("The file must be sent as bytes"));
    };
    let bytes = bytes.clone();
    let note_id = header_text(&request, "x-note-id")?;
    let name = header_text(&request, "x-file-name")?;
    let file_type = header_text(&request, "x-file-type")?;
    let database = state.database.clone();
    run_database_task(move || database.add_note_attachment(&note_id, &name, &file_type, &bytes))
        .await
}

#[tauri::command]
pub(crate) async fn note_attachment(
    id: String,
    state: State<'_, AppState>,
) -> AppResult<Option<NoteAttachment>> {
    let database = state.database.clone();
    run_database_task(move || database.note_attachment(&id)).await
}

/// An attached image as a `data:` URI, or `None` when it isn't an image this
/// device has. Only raster formats whose bytes match their type are shown.
#[tauri::command]
pub(crate) async fn note_attachment_image(
    id: String,
    state: State<'_, AppState>,
) -> AppResult<Option<String>> {
    use base64::Engine;
    let database = state.database.clone();
    let Some((attachment, bytes)) =
        run_database_task(move || database.note_attachment_bytes(&id)).await?
    else {
        return Ok(None);
    };
    if !attachment.image
        || image_format::sniff_raster_mime(&bytes) != Some(attachment.mime_type.as_str())
        || image_format::validate_raster_with_limit(
            &bytes,
            threestrands_sync_protocol::MAX_NOTE_ATTACHMENT_BYTES as usize,
        )
        .is_err()
    {
        return Ok(None);
    }
    Ok(Some(format!(
        "data:{};base64,{}",
        attachment.mime_type,
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )))
}

/// Opens an attached file in its default app, with the same warning a risky
/// mail attachment gets. The copy opened is quarantined like one.
#[tauri::command]
pub(crate) async fn note_attachment_open(
    id: String,
    window: tauri::Window,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let database = state.database.clone();
    let (attachment, bytes) = run_database_task(move || database.note_attachment_bytes(&id))
        .await?
        .ok_or(
            "This file hasn't reached this device yet. It downloads the next time notes sync.",
        )?;
    let filename = attachment_security::normalize_filename(&attachment.name);
    if let Some(description) = attachment_security::opening_confirmation(&filename, &bytes) {
        let confirmed = rfd::AsyncMessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Open potentially unsafe file?")
            .set_description(description)
            .set_buttons(rfd::MessageButtons::OkCancelCustom(
                "Cancel".into(),
                "Open anyway".into(),
            ))
            .set_parent(&window)
            .show()
            .await;
        if !attachment_security::confirmation_allows_open(&confirmed) {
            return Ok(());
        }
    }
    let path = state
        .attachment_reader
        .write(&filename, &bytes)
        .map_err(|error| format!("Unable to prepare the file: {error}"))?;
    open::that(path).map_err(|error| AppError::message(format!("Unable to open the file: {error}")))
}

/// An attached file and its bytes, or the reason it can't be read here.
async fn attachment_with_bytes(
    id: String,
    state: &AppState,
) -> AppResult<(NoteAttachment, Vec<u8>)> {
    let database = state.database.clone();
    Ok(
        run_database_task(move || database.note_attachment_bytes(&id))
            .await?
            .ok_or(
                "This file hasn't reached this device yet. It downloads the next time notes sync.",
            )?,
    )
}

/// Saves a copy of an attached file where the user chooses, quarantined
/// like a saved mail attachment.
#[tauri::command]
pub(crate) async fn note_attachment_save(id: String, state: State<'_, AppState>) -> AppResult<()> {
    let (attachment, bytes) = attachment_with_bytes(id, &state).await?;
    let filename = attachment_security::normalize_filename(&attachment.name);
    let Some(destination) = rfd::AsyncFileDialog::new()
        .set_file_name(&filename)
        .save_file()
        .await
    else {
        return Ok(());
    };
    std::fs::write(destination.path(), bytes)
        .map_err(|error| format!("Unable to save the file: {error}"))?;
    if let Err(error) = attachment_security::quarantine(destination.path()) {
        return match std::fs::remove_file(destination.path()) {
            Ok(()) => Err(AppError::message(format!(
                "Unable to save the file safely because quarantine metadata could not be applied: {error}"
            ))),
            Err(remove_error) => Err(AppError::message(format!(
                "Quarantine metadata could not be applied to the saved file ({error}), and the unsafe copy could not be removed ({remove_error})"
            ))),
        };
    }
    Ok(())
}

/// A picture of an attached file for Files, drawn the way mail attachment
/// previews are; `None` when it has none or isn't on this device yet.
#[tauri::command]
pub(crate) async fn note_attachment_preview(
    id: String,
    state: State<'_, AppState>,
) -> AppResult<Option<String>> {
    if !file_preview::HAS_BACKEND {
        return Ok(None);
    }
    let database = state.database.clone();
    let Some((attachment, bytes)) =
        run_database_task(move || database.note_attachment_bytes(&id)).await?
    else {
        return Ok(None);
    };
    let filename = attachment_security::normalize_filename(&attachment.name);
    file_preview::preview_data_uri(&state.file_previews, &filename, &bytes)
        .await
        .map_err(AppError::from)
}

#[tauri::command]
pub(crate) async fn set_note_attachment_star(
    id: String,
    starred: bool,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let database = state.database.clone();
    run_database_task(move || database.set_note_attachment_star(&id, starred)).await
}

/// Asks for a folder of Markdown notes to import and reads it, without
/// writing anything. The plan stays here for [`notes_import_run`]; the page
/// sees only its summary.
#[tauri::command]
pub(crate) async fn notes_import_choose_folder(
    state: State<'_, AppState>,
) -> AppResult<Option<notes_import::ImportSummary>> {
    state.notes_import.set(None);
    let Some(folder) = rfd::AsyncFileDialog::new()
        .set_title("Choose a folder of notes to import")
        .pick_folder()
        .await
    else {
        return Ok(None);
    };
    let path = folder.path().to_path_buf();
    let database = state.database.clone();
    let plan = tokio::task::spawn_blocking(move || {
        let (ids, titles) = database
            .note_identities()
            .map_err(|error| error.to_string())?;
        notes_import::plan(&path, &notes_import::Existing { ids, titles })
    })
    .await
    .map_err(|error| error.to_string())??;
    let summary = plan.summary();
    state.notes_import.set(Some(plan));
    Ok(Some(summary))
}

/// Imports the folder [`notes_import_choose_folder`] read, then syncs.
#[tauri::command]
pub(crate) async fn notes_import_run(
    state: State<'_, AppState>,
) -> AppResult<notes_import::ImportOutcome> {
    let plan = state
        .notes_import
        .take()
        .ok_or("Choose a folder of notes to import first")?;
    let database = state.database.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        notes_import::run(plan, |note, files| {
            database
                .import_note(note, files)
                .map_err(|error| error.to_string())
        })
    })
    .await
    .map_err(|error| error.to_string())?;
    if outcome.notes > 0 {
        kick_replicated_sync(&state);
    }
    Ok(outcome)
}
