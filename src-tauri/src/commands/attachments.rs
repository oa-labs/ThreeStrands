//! Attachments, remote images, file previews, and the Files list.

use tauri::State;

use crate::{attachment_security, calendar, file_preview, image_format, image_proxy, mime, models};
use crate::{run_database_task, AppError, AppResult, AppState};

/// Fetches a remote image referenced by message HTML and returns it as a
/// `data:` URI, so the message iframe never contacts the sender's (or a
/// spoofed) host directly. See `image_proxy` for the validation this does.
#[tauri::command]
pub(crate) async fn fetch_remote_image(url: String, state: State<'_, AppState>) -> AppResult<String> {
    image_proxy::fetch(&url, &state.image_cache)
        .await
        .map_err(AppError::from)
}

pub(super) async fn load_attachment(
    message_id: &str,
    attachment_id: &str,
    state: &AppState,
) -> AppResult<(String, String, Vec<u8>)> {
    let database = state.database.clone();
    let stored_message_id = message_id.to_string();
    let stored_attachment_id = attachment_id.to_string();
    let (account_id, attachment, payload_bytes, cached_provider_id) = run_database_task(move || {
        let (account_id, message) = database.attachment_message(&stored_message_id)?;
        let reference = mime::attachment_part_reference(&message, &stored_attachment_id)?;
        let attachment = mime::normalize(&message)?
            .attachments
            .into_iter()
            .find(|attachment| attachment.id == reference)
            .ok_or("Attachment not found")?;
        let bytes = mime::attachment_bytes_from_payload(&message, &reference)?;
        let provider_id = mime::provider_attachment_id_from_payload(&message, &reference)?;
        Ok::<_, String>((account_id, attachment, bytes, provider_id))
    })
    .await?;
    let bytes = match payload_bytes {
        Some(bytes) => bytes,
        None => {
            let provider = state.correspondence.provider_for(&account_id).await?;
            // The attachment ID is the stable part reference; the provider ID
            // comes from the cached payload. Builds before MimeBody's Gmail
            // camelCase mapping was fixed cached payloads without one, so
            // refresh such a message on demand.
            let provider_id = match cached_provider_id {
                Some(provider_id) => provider_id,
                None => {
                    let fresh = provider
                        .fetch_message(message_id)
                        .await
                        .map_err(|error| error.to_string())?;
                    mime::provider_attachment_id_from_payload(&fresh, &attachment.id)?
                        .ok_or("Attachment data is unavailable")?
                }
            };
            provider
                .attachment_bytes(message_id, &provider_id)
                .await
                .map_err(|error| error.to_string())?
        }
    };
    let filename = attachment_security::normalize_filename(&attachment.filename);
    Ok((filename, attachment.mime_type, bytes))
}

#[tauri::command]
pub(crate) async fn fetch_attachment_image(
    message_id: String,
    attachment_id: String,
    state: State<'_, AppState>,
) -> AppResult<String> {
    use base64::Engine;

    let (_, mime_type, bytes) = load_attachment(&message_id, &attachment_id, &state).await?;
    let mime_type = mime_type.to_ascii_lowercase();
    if !image_format::is_supported_raster_mime(&mime_type) {
        return Err("Embedded attachment is not a supported image".into());
    }
    image_format::validate_raster(&bytes)?;
    Ok(format!(
        "data:{mime_type};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

/// A picture of the attachment for Files, or `None` when it has none. A
/// preview is optional, so a file the platform cannot draw is not an error.
#[tauri::command]
pub(crate) async fn file_preview(
    message_id: String,
    attachment_id: String,
    state: State<'_, AppState>,
) -> AppResult<Option<String>> {
    if !file_preview::HAS_BACKEND {
        return Ok(None);
    }
    let (filename, _, bytes) = load_attachment(&message_id, &attachment_id, &state).await?;
    file_preview::preview_data_uri(&state.file_previews, &filename, &bytes)
        .await
        .map_err(AppError::from)
}

#[tauri::command]
pub(crate) async fn preview_calendar_attachment(
    message_id: String,
    attachment_id: String,
    state: State<'_, AppState>,
) -> AppResult<calendar::CalendarPreview> {
    let (filename, mime_type, bytes) = load_attachment(&message_id, &attachment_id, &state).await?;
    let is_calendar = mime_type
        .split(';')
        .next()
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/calendar"))
        || filename.to_ascii_lowercase().ends_with(".ics");
    if !is_calendar {
        return Err("Attachment is not a calendar invitation".into());
    }
    calendar::parse(&bytes).map_err(AppError::from)
}

#[tauri::command]
pub(crate) async fn open_attachment(
    message_id: String,
    attachment_id: String,
    window: tauri::Window,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let (filename, _, bytes) = load_attachment(&message_id, &attachment_id, &state).await?;
    if let Some(description) = attachment_security::opening_confirmation(&filename, &bytes) {
        let confirmed = rfd::AsyncMessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Open potentially unsafe attachment?")
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
        .map_err(|error| format!("Unable to prepare attachment: {error}"))?;
    open::that(path).map_err(|error| AppError::message(format!("Unable to open attachment: {error}")))
}

#[tauri::command]
pub(crate) async fn save_attachment(
    message_id: String,
    attachment_id: String,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let (filename, _, bytes) = load_attachment(&message_id, &attachment_id, &state).await?;
    let Some(destination) = rfd::AsyncFileDialog::new()
        .set_file_name(&filename)
        .save_file()
        .await
    else {
        return Ok(());
    };
    std::fs::write(destination.path(), bytes)
        .map_err(|error| format!("Unable to save attachment: {error}"))?;
    if let Err(error) = attachment_security::quarantine(destination.path()) {
        return match std::fs::remove_file(destination.path()) {
            Ok(()) => Err(AppError::message(format!(
                "Unable to save attachment safely because quarantine metadata could not be applied: {error}"
            ))),
            Err(remove_error) => Err(AppError::message(format!(
                "Quarantine metadata could not be applied to the saved attachment ({error}), and the unsafe copy could not be removed ({remove_error})"
            ))),
        };
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn list_files(query: models::FileQuery, state: State<'_, AppState>) -> AppResult<models::FilePage> {
    let database = state.database.clone();
    run_database_task(move || database.list_files(&query)).await
}

#[tauri::command]
pub(crate) async fn file_stars(message_ids: Vec<String>, state: State<'_, AppState>) -> AppResult<Vec<(String,String)>> {
    let database = state.database.clone();
    run_database_task(move || database.file_stars(&message_ids)).await
}

#[tauri::command]
pub(crate) async fn set_file_star(account_id: Option<String>, message_id: String, attachment_id: String, starred: bool, state: State<'_, AppState>) -> AppResult<()> {
    let database = state.database.clone();
    run_database_task(move || {
        // Surfaces that show one message's attachments name no account; the message's own thread decides.
        let account_id = match account_id { Some(account_id) => account_id, None => database.message_account(&message_id)? };
        database.set_file_star(&account_id, &message_id, &attachment_id, starred)
    }).await
}
