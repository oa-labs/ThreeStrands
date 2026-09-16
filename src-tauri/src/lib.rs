mod ai;
mod auth;
mod calendar;
mod correspondence;
mod db;
mod gmail;
mod image_proxy;
mod mime;
mod models;
mod net_safety;
mod sync;
mod system_fonts;
mod transfer;
#[path = "unsubscribe.rs"]
mod unsubscribe_service;

use std::collections::HashMap;
use std::sync::Arc;

use auth::{GoogleAuth, GoogleAuthConfig};
use chrono::Utc;
use db::Database;
use gmail::{GmailClient, GmailProvider};
use models::{
    Account, AuthStatus, ContactSuggestion, CreateLabelRequest, CreateSplitInboxRequest, Label,
    SearchThreadsRequest, SplitInbox, SummaryResult, SyncStatus, Thread, ThreadDetail,
    ThreadMutation, ThreadPage, TriageEvent, TriageSenderStats, UpdateLabelRequest,
    UpdateSplitInboxRequest,
};
use sync::SyncService;
use tauri::{async_runtime::JoinHandle, Manager, State};
use tokio_util::sync::CancellationToken;
use url::Url;

/// Keep remote pages out of Dispatch even if a platform webview activates an
/// email link before the iframe's DOM click handler can cancel it. This is a
/// final native boundary: app documents may navigate in the webview, ordinary
/// web/mail/telephone URLs are handed to the operating system, and other
/// navigation schemes are denied.
fn allow_in_app_navigation(url: &Url) -> bool {
    match url.scheme() {
        "about" | "tauri" => true,
        "http" | "https" => {
            let host = url.host_str();
            host == Some("tauri.localhost")
                || (cfg!(debug_assertions)
                    && host == Some("localhost")
                    && url.port_or_known_default() == Some(1420))
        }
        _ => false,
    }
}

fn external_navigation_guard<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::new("external-navigation")
        .on_navigation(|_, url| {
            if allow_in_app_navigation(url) {
                return true;
            }
            if matches!(url.scheme(), "http" | "https" | "mailto" | "tel") {
                let _ = open::that(url.as_str());
            }
            false
        })
        .build()
}

#[cfg(test)]
mod navigation_tests {
    use super::allow_in_app_navigation;
    use url::Url;

    #[test]
    fn keeps_only_application_documents_in_the_webview() {
        assert!(allow_in_app_navigation(
            &Url::parse("tauri://localhost/").unwrap()
        ));
        assert!(allow_in_app_navigation(
            &Url::parse("http://tauri.localhost/thread/1").unwrap()
        ));
        assert!(allow_in_app_navigation(
            &Url::parse("about:srcdoc").unwrap()
        ));
    }

    #[test]
    fn rejects_remote_and_active_navigation_from_the_webview() {
        for value in [
            "https://calendar.example/event?action=respond",
            "http://example.com/",
            "mailto:person@example.com",
            "tel:+15551234567",
            "file:///etc/passwd",
            "javascript:alert(1)",
        ] {
            assert!(!allow_in_app_navigation(&Url::parse(value).unwrap()));
        }
    }
}

/// An account beyond the primary: its own credentials and the task running
/// its own sync loop. Removing the account aborts `poll_task`. Shared with
/// `Correspondence`, which resolves a draft's own account through the same
/// registry rather than assuming the primary account sends everything.
pub(crate) struct ConnectedAccount {
    pub(crate) auth: GoogleAuth,
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
    /// Serializes interactive "sign in with Google" flows (connect/add/
    /// reconnect) so two never race two loopback listeners at once — without
    /// blocking unrelated work like outbox delivery, which used to share a
    /// lock with these. A new attempt cancels whichever one it replaces
    /// rather than queuing behind it, so a browser tab closed without
    /// finishing never silently blocks the next "Add account" click.
    authorize_slot: AuthorizeSlot,
    exiting: std::sync::atomic::AtomicBool,
    image_cache: image_proxy::ImageCache,
}

#[derive(Clone, Default)]
struct AuthorizeSlot(Arc<tokio::sync::Mutex<Option<CancellationToken>>>);

impl AuthorizeSlot {
    /// Cancels any interactive sign-in flow currently in progress and claims
    /// the slot for a new one, returning the token this flow should race
    /// against.
    async fn claim(&self) -> CancellationToken {
        let mut slot = self.0.lock().await;
        if let Some(previous) = slot.take() {
            previous.cancel();
        }
        let token = CancellationToken::new();
        *slot = Some(token.clone());
        token
    }

    /// Releases the slot, but only if a newer attempt hasn't already claimed
    /// it (which would have canceled `token`) — otherwise this would clear a
    /// slot that isn't this flow's anymore.
    async fn release(&self, token: &CancellationToken) {
        let mut slot = self.0.lock().await;
        if !token.is_cancelled() {
            *slot = None;
        }
    }
}

/// Runs `auth.authorize()` behind the app-wide sign-in slot, canceling
/// whichever attempt it replaces and always releasing the slot afterward.
async fn authorize_interactively(state: &AppState, auth: &GoogleAuth) -> Result<String, String> {
    let token = state.authorize_slot.claim().await;
    let result = auth.authorize(&token).await;
    state.authorize_slot.release(&token).await;
    result
}

#[cfg(test)]
mod authorize_slot_tests {
    use super::AuthorizeSlot;

    #[tokio::test]
    async fn claiming_the_slot_cancels_whatever_it_replaces() {
        let slot = AuthorizeSlot::default();
        let first = slot.claim().await;
        assert!(!first.is_cancelled());
        let second = slot.claim().await;
        assert!(
            first.is_cancelled(),
            "a new claim must cancel the stale one"
        );
        assert!(!second.is_cancelled());
    }

    #[tokio::test]
    async fn releasing_a_superseded_token_does_not_clobber_the_newer_claim() {
        let slot = AuthorizeSlot::default();
        let first = slot.claim().await;
        let second = slot.claim().await;
        // `first` was canceled by `second`'s claim, so its (belated) release
        // must be a no-op rather than clearing `second`'s slot out from
        // under it.
        slot.release(&first).await;
        assert!(!second.is_cancelled());
        slot.release(&second).await;
        // The slot is free again now that its rightful owner released it.
        let third = slot.claim().await;
        assert!(!third.is_cancelled());
    }

    #[tokio::test]
    async fn releasing_the_current_claim_frees_the_slot_for_reuse() {
        let slot = AuthorizeSlot::default();
        let first = slot.claim().await;
        slot.release(&first).await;
        let second = slot.claim().await;
        assert!(
            !second.is_cancelled(),
            "a fresh claim must not start canceled"
        );
    }
}

/// Best-effort: narrow a data directory to owner-only access, so nothing
/// inside it (the database, its WAL/SHM sidecar files, cached attachments)
/// is readable by other local users regardless of umask.
#[cfg(unix)]
fn restrict_dir_to_owner(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn restrict_dir_to_owner(_path: &std::path::Path) {}

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
fn list_threads(
    account_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<Thread>, String> {
    state.database.list_threads(account_id.as_deref())
}

#[tauri::command]
fn list_all_mail(
    account_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<Thread>, String> {
    state.database.list_all_mail(account_id.as_deref())
}

#[tauri::command]
fn list_trash(
    account_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<Thread>, String> {
    state.database.list_trash(account_id.as_deref())
}

#[tauri::command]
fn list_threads_page(
    account_id: Option<String>,
    offset: usize,
    limit: usize,
    state: State<'_, AppState>,
) -> Result<ThreadPage, String> {
    state
        .database
        .list_threads_page(account_id.as_deref(), offset, limit)
}

#[tauri::command]
fn list_unread_counts(state: State<'_, AppState>) -> Result<HashMap<String, i64>, String> {
    state.database.list_unread_counts()
}

#[tauri::command]
fn list_all_mail_page(
    account_id: Option<String>,
    offset: usize,
    limit: usize,
    state: State<'_, AppState>,
) -> Result<ThreadPage, String> {
    state
        .database
        .list_all_mail_page(account_id.as_deref(), offset, limit)
}

#[tauri::command]
fn list_trash_page(
    account_id: Option<String>,
    offset: usize,
    limit: usize,
    state: State<'_, AppState>,
) -> Result<ThreadPage, String> {
    state
        .database
        .list_trash_page(account_id.as_deref(), offset, limit)
}

#[tauri::command]
fn get_thread(id: String, state: State<'_, AppState>) -> Result<ThreadDetail, String> {
    state.database.get_thread(&id)
}

/// Fetches a remote image referenced by message HTML and returns it as a
/// `data:` URI, so the message iframe never contacts the sender's (or a
/// spoofed) host directly. See `image_proxy` for the validation this does.
#[tauri::command]
async fn fetch_remote_image(url: String, state: State<'_, AppState>) -> Result<String, String> {
    image_proxy::fetch(&url, &state.image_cache).await
}

async fn load_attachment(
    message_id: &str,
    attachment_id: &str,
    state: &AppState,
) -> Result<(String, String, Vec<u8>), String> {
    let (account_id, message) = state.database.attachment_message(message_id)?;
    let attachment = mime::normalize(&message)?
        .attachments
        .into_iter()
        .find(|attachment| attachment.id == attachment_id)
        .ok_or("Attachment not found")?;
    let bytes = match mime::attachment_bytes_from_payload(&message, attachment_id)? {
        Some(bytes) => bytes,
        None => {
            let provider = state.correspondence.provider_for(&account_id).await?;
            // Builds before MimeBody's Gmail camelCase mapping was fixed cached
            // remote attachments as `part:<mime path>`. Refresh that message on
            // demand so those existing rows keep working after an upgrade.
            let provider_id = if attachment_id.starts_with("part:") {
                let fresh = provider
                    .get_message(message_id)
                    .await
                    .map_err(|error| error.to_string())?;
                mime::provider_attachment_id_from_payload(&fresh, attachment_id)?
                    .ok_or("Attachment data is unavailable")?
            } else {
                attachment_id.to_string()
            };
            provider
                .attachment_bytes(message_id, &provider_id)
                .await
                .map_err(|error| error.to_string())?
        }
    };
    let filename = std::path::Path::new(&attachment.filename)
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty() && !value.chars().any(char::is_control))
        .unwrap_or("attachment")
        .to_string();
    Ok((filename, attachment.mime_type, bytes))
}

#[tauri::command]
async fn fetch_attachment_image(
    message_id: String,
    attachment_id: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    use base64::Engine;

    let (_, mime_type, bytes) = load_attachment(&message_id, &attachment_id, &state).await?;
    let mime_type = mime_type.to_ascii_lowercase();
    if !matches!(
        mime_type.as_str(),
        "image/avif" | "image/gif" | "image/jpeg" | "image/png" | "image/webp"
    ) {
        return Err("Embedded attachment is not a supported image".into());
    }
    Ok(format!(
        "data:{mime_type};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

#[tauri::command]
async fn preview_calendar_attachment(
    message_id: String,
    attachment_id: String,
    state: State<'_, AppState>,
) -> Result<calendar::CalendarPreview, String> {
    let (filename, mime_type, bytes) = load_attachment(&message_id, &attachment_id, &state).await?;
    let is_calendar = mime_type
        .split(';')
        .next()
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/calendar"))
        || filename.to_ascii_lowercase().ends_with(".ics");
    if !is_calendar {
        return Err("Attachment is not a calendar invitation".into());
    }
    calendar::parse(&bytes)
}

#[tauri::command]
async fn open_attachment(
    message_id: String,
    attachment_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let (filename, _, bytes) = load_attachment(&message_id, &attachment_id, &state).await?;
    let directory = state
        .correspondence
        .root
        .join("reader")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("Unable to prepare attachment: {error}"))?;
    let path = directory.join(filename);
    std::fs::write(&path, bytes).map_err(|error| format!("Unable to write attachment: {error}"))?;
    open::that(path).map_err(|error| format!("Unable to open attachment: {error}"))
}

#[tauri::command]
async fn save_attachment(
    message_id: String,
    attachment_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let (filename, _, bytes) = load_attachment(&message_id, &attachment_id, &state).await?;
    let Some(destination) = rfd::AsyncFileDialog::new()
        .set_file_name(&filename)
        .save_file()
        .await
    else {
        return Ok(());
    };
    std::fs::write(destination.path(), bytes)
        .map_err(|error| format!("Unable to save attachment: {error}"))
}

#[tauri::command]
fn search_threads(
    request: SearchThreadsRequest,
    account_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<Thread>, String> {
    state
        .database
        .search_threads(&request, account_id.as_deref())
}

#[tauri::command]
fn mutate_thread(mutation: ThreadMutation, state: State<'_, AppState>) -> Result<(), String> {
    state.database.mutate_thread(&mutation)
}

#[tauri::command]
fn mutate_threads(
    mutations: Vec<ThreadMutation>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.database.mutate_threads(&mutations)
}

#[tauri::command]
fn record_triage_event(event: TriageEvent, state: State<'_, AppState>) -> Result<(), String> {
    state.database.record_triage_event(&event)
}

#[tauri::command]
fn list_triage_sender_stats(
    account_id: String,
    limit: Option<usize>,
    state: State<'_, AppState>,
) -> Result<Vec<TriageSenderStats>, String> {
    state
        .database
        .list_triage_sender_stats(&account_id, limit.unwrap_or(100))
}

#[tauri::command]
fn list_contact_suggestions(
    account_id: String,
    query: String,
    limit: Option<usize>,
    state: State<'_, AppState>,
) -> Result<Vec<ContactSuggestion>, String> {
    state
        .database
        .list_contact_suggestions(&account_id, &query, limit.unwrap_or(8))
}

#[tauri::command]
fn pin_contact(
    account_id: String,
    email: String,
    display_name: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .database
        .pin_contact(&account_id, &email, display_name.as_deref())
}

#[tauri::command]
fn unpin_contact(
    account_id: String,
    email: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.database.unpin_contact(&account_id, &email)
}

#[tauri::command]
async fn unsubscribe(
    message_id: String,
    state: State<'_, AppState>,
) -> Result<models::UnsubscribeResult, String> {
    let target = state.database.begin_unsubscribe(&message_id)?;
    match unsubscribe_service::execute(&target).await {
        Ok(result) => {
            state.database.finish_unsubscribe(
                &target.request_id,
                if result.outcome == "opened" {
                    "opened"
                } else {
                    "succeeded"
                },
                result.http_status,
                None,
            )?;
            Ok(result)
        }
        Err(error) => {
            state
                .database
                .finish_unsubscribe(&target.request_id, "failed", None, Some(&error))?;
            Err(error)
        }
    }
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
    let auth = state.auth.clone().ok_or_else(not_configured)?;
    authorize_interactively(&state, &auth).await?;
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
    let config = state.auth_config.as_ref().ok_or_else(not_configured)?;
    let auth = config.pending_account();
    let email = authorize_interactively(&state, &auth).await?;
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
    authorize_interactively(&state, &auth).await?;
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
fn set_account_display_name(
    email: String,
    display_name: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .database
        .set_account_display_name(&email, display_name.as_deref())
}

#[tauri::command]
fn reorder_accounts(emails: Vec<String>, state: State<'_, AppState>) -> Result<(), String> {
    state.database.reorder_accounts(&emails)
}

#[tauri::command]
fn export_settings(
    preferences: transfer::TransferPreferences,
    password: String,
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    transfer::export(&state.database, preferences, &password)
}

#[tauri::command]
fn import_settings(
    password: String,
    state: State<'_, AppState>,
) -> Result<Option<transfer::ImportResult>, String> {
    transfer::import(&state.database, &password)
}

#[tauri::command]
fn list_split_inboxes(state: State<'_, AppState>) -> Result<Vec<SplitInbox>, String> {
    state.database.list_split_inboxes()
}

#[tauri::command]
fn create_split_inbox(
    request: CreateSplitInboxRequest,
    state: State<'_, AppState>,
) -> Result<SplitInbox, String> {
    state
        .database
        .create_split_inbox(&request.name, &request.match_kind, &request.match_value)
}

#[tauri::command]
fn update_split_inbox(
    request: UpdateSplitInboxRequest,
    state: State<'_, AppState>,
) -> Result<SplitInbox, String> {
    state.database.update_split_inbox(&request.id, &request.name)
}

#[tauri::command]
fn delete_split_inbox(id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.database.delete_split_inbox(&id)
}

#[tauri::command]
fn reorder_split_inboxes(ids: Vec<String>, state: State<'_, AppState>) -> Result<(), String> {
    state.database.reorder_split_inboxes(&ids)
}

#[tauri::command]
fn list_split_inbox_page(
    split_inbox_id: String,
    account_id: Option<String>,
    offset: usize,
    limit: usize,
    state: State<'_, AppState>,
) -> Result<ThreadPage, String> {
    state.database.list_split_inbox_page(
        &split_inbox_id,
        account_id.as_deref(),
        offset,
        limit,
    )
}

#[tauri::command]
async fn list_labels(
    account_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<Label>, String> {
    let auth = match account_id.as_deref() {
        None => state.auth.clone().ok_or_else(not_configured)?,
        Some(account_id)
            if state
                .auth
                .as_ref()
                .is_some_and(|auth| auth.key() == account_id) =>
        {
            state.auth.clone().expect("primary account matched")
        }
        Some(account_id) => state
            .additional_accounts
            .lock()
            .await
            .get(account_id)
            .map(|account| account.auth.clone())
            .ok_or_else(|| {
                format!("{account_id} is not connected. Reconnect it before continuing.")
            })?,
    };

    GmailClient::new(auth)
        .list_labels()
        .await
        .map_err(|error| error.to_string())
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
fn get_retention_days(state: State<'_, AppState>) -> Result<Option<i64>, String> {
    state.database.retention_days()
}

#[tauri::command]
fn set_retention_days(days: Option<i64>, state: State<'_, AppState>) -> Result<(), String> {
    state.database.set_retention_days(days)
}

#[tauri::command]
fn ai_api_key_configured() -> bool {
    ai::configured()
}

#[tauri::command]
fn set_ai_api_key(key: String) -> Result<(), String> {
    ai::set(&key)
}

#[tauri::command]
async fn ai_summarize_thread(
    thread_id: String,
    provider: String,
    model: String,
    endpoint: Option<String>,
    state: State<'_, AppState>,
) -> Result<SummaryResult, String> {
    let api_key = ai::get_key()?.ok_or_else(|| "No AI API key configured".to_string())?;
    let detail = state.database.get_thread(&thread_id)?;
    let request = ai::SummarizeRequest {
        provider,
        model,
        endpoint,
        subject: detail.thread.subject,
        messages: detail
            .messages
            .into_iter()
            .map(|message| ai::ThreadMessageInput {
                sender: message.sender,
                sent_at: message.sent_at,
                body_text: message.body_text,
            })
            .collect(),
    };
    let summary = ai::summarize(request, &api_key).await?;
    let generated_at = Utc::now().to_rfc3339();
    state
        .database
        .set_thread_summary(&thread_id, &summary, &generated_at)?;
    Ok(SummaryResult {
        summary,
        generated_at,
    })
}

fn not_configured() -> String {
    "Google OAuth is not configured. Set DISPATCH_GOOGLE_CLIENT_ID and \
     DISPATCH_GOOGLE_CLIENT_SECRET from a Desktop app credential."
        .into()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(external_navigation_guard())
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
            restrict_dir_to_owner(&data_dir);
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
            {
                let database = database.clone();
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        if let Ok(count) = database.count_unread_inbox() {
                            if let Some(window) = handle.get_webview_window("main") {
                                let _ = window.set_badge_count((count > 0).then_some(count));
                            }
                        }
                        tokio::time::sleep(sync::MIN_POLL_INTERVAL).await;
                    }
                });
            }
            {
                // One-time, potentially slow (full file rewrite) conversion
                // to incremental auto-vacuum, then a recurring prune of mail
                // past the user's retention window with cheap incremental
                // reclamation after. All off the blocking pool so a large
                // existing mailbox doesn't stall startup or the UI thread.
                let database = database.clone();
                tauri::async_runtime::spawn(async move {
                    let upgrade_db = database.clone();
                    let needs_upgrade =
                        tokio::task::spawn_blocking(move || upgrade_db.needs_vacuum_upgrade())
                            .await
                            .ok()
                            .and_then(|result| result.ok())
                            .unwrap_or(false);
                    if needs_upgrade {
                        let upgrade_db = database.clone();
                        let _ =
                            tokio::task::spawn_blocking(move || upgrade_db.vacuum_to_incremental())
                                .await;
                    }
                    // One-time backfill: compress any message bodies left
                    // over from before body compression shipped, a batch at
                    // a time with a short pause between batches so this
                    // doesn't starve the database mutex normal sync/read
                    // operations also need. Becomes a no-op once everything
                    // has been converted.
                    loop {
                        let backfill_db = database.clone();
                        let converted = tokio::task::spawn_blocking(move || {
                            backfill_db.compress_next_body_batch(500)
                        })
                        .await
                        .ok()
                        .and_then(|result| result.ok())
                        .unwrap_or(0);
                        if converted == 0 {
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    }
                    loop {
                        let prune_db = database.clone();
                        let _ = tokio::task::spawn_blocking(move || {
                            let _ = prune_db.prune_expired_threads();
                            let _ = prune_db.reclaim_space();
                        })
                        .await;
                        tokio::time::sleep(std::time::Duration::from_secs(6 * 60 * 60)).await;
                    }
                });
            }
            let root = data_dir.join("attachments");
            std::fs::create_dir_all(&root)?;
            restrict_dir_to_owner(&root);
            let additional_accounts = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
            let correspondence = correspondence::Correspondence {
                database: database.clone(),
                primary: auth.clone(),
                additional_accounts: additional_accounts.clone(),
                root,
                gate: Arc::new(tokio::sync::Mutex::new(())),
                edits: Arc::new(tokio::sync::Mutex::new(())),
            };
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
                        let primary_email = worker.primary.as_ref().map(GoogleAuth::key);
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
                authorize_slot: AuthorizeSlot::default(),
                exiting: std::sync::atomic::AtomicBool::new(false),
                image_cache: image_proxy::ImageCache::new().map_err(std::io::Error::other)?,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            correspondence_request,
            finish_exit,
            list_threads,
            list_all_mail,
            list_trash,
            list_threads_page,
            list_unread_counts,
            list_all_mail_page,
            list_trash_page,
            get_thread,
            fetch_remote_image,
            fetch_attachment_image,
            preview_calendar_attachment,
            open_attachment,
            save_attachment,
            search_threads,
            mutate_thread,
            mutate_threads,
            record_triage_event,
            list_triage_sender_stats,
            list_contact_suggestions,
            pin_contact,
            unpin_contact,
            unsubscribe,
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
            set_account_display_name,
            set_account_color,
            reorder_accounts,
            export_settings,
            import_settings,
            list_split_inboxes,
            create_split_inbox,
            update_split_inbox,
            delete_split_inbox,
            reorder_split_inboxes,
            list_split_inbox_page,
            list_labels,
            create_label,
            update_label,
            delete_label,
            get_retention_days,
            set_retention_days,
            ai_api_key_configured,
            set_ai_api_key,
            ai_summarize_thread,
            system_fonts::list_system_font_families,
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
