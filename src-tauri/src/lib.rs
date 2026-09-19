mod ai;
mod attachment_reader;
mod attachment_security;
mod auth;
mod calendar;
mod correspondence;
mod db;
mod image_format;
mod image_proxy;
mod mime;
mod models;
mod net_safety;
mod provider;
mod schema;
mod sync;
mod system_fonts;
mod transfer;
#[path = "unsubscribe.rs"]
mod unsubscribe_service;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use auth::{GoogleAuth, GoogleAuthConfig};
use chrono::Utc;
use db::Database;
use provider::{gmail::GmailClient, MailMutate};
use models::{
    Account, AuthStatus, CalendarAccount, CalendarOption, ContactSuggestion, CreateLabelRequest,
    CreateSplitInboxRequest, Label, MailboxUnreadCounts, ReplyAssistContext, ReplyAssistResult,
    ScheduleResult, SearchThreadsRequest, SplitInbox, SummaryResult, SyncStatus, Thread,
    ThreadDetail, ThreadMutation, ThreadPage, TriageEvent, TriageSenderStats, UpdateLabelRequest,
    UpdateSplitInboxRequest,
};
use sync::SyncService;
use tauri::{async_runtime::JoinHandle, Manager, State};
use tokio_util::sync::CancellationToken;
use url::Url;

/// Keep remote pages out of ThreeStrands even if a platform webview activates an
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

/// A connected account: its own credentials and the task running its own
/// sync loop. Removing the account aborts `poll_task`. Shared with
/// `Correspondence`, which resolves a draft's own account through the same
/// registry rather than assuming one account sends everything.
pub(crate) struct ConnectedAccount {
    pub(crate) auth: GoogleAuth,
    pub(crate) sync: SyncService,
    poll_task: JoinHandle<()>,
}

/// Every connected account, keyed by account id. Held by `AppState` and
/// shared with `Correspondence` as the same `Arc`.
pub(crate) type AccountRegistry = Arc<tokio::sync::Mutex<HashMap<String, ConnectedAccount>>>;

/// Moves the pre-connect placeholder entry onto the real address once
/// `accept_identity` has learned it. `GoogleAuth` and `SyncService` both read
/// their key through a shared cell, so the entry's contents already describe
/// the real account — only the map key is stale.
pub(crate) async fn rekey_placeholder_account(accounts: &AccountRegistry, identity: &str) {
    if identity == auth::LEGACY_KEY {
        return;
    }
    let mut accounts = accounts.lock().await;
    if let Some(connected) = accounts.remove(auth::LEGACY_KEY) {
        accounts.insert(identity.to_string(), connected);
    }
}

struct AppState {
    database: Arc<Database>,
    /// Shared OAuth app credentials, used to authorize any account.
    auth_config: Option<GoogleAuthConfig>,
    /// Every connected account, keyed by account id — each with its own sync
    /// cursor and polling loop, independent of the others. Populated at
    /// startup from the `accounts` table and by `add_account`.
    ///
    /// Before the very first connect the map instead holds one entry under
    /// the placeholder key `auth::LEGACY_KEY`, which rekeys onto the real
    /// address once `refresh_identity` learns it. That placeholder is an
    /// account id like any other here, so no code path has to distinguish a
    /// "primary" account from the rest; where one genuinely is needed (the
    /// compose default and the legacy single-account commands) it is derived
    /// from the account catalog by `Database::primary_account_id`.
    accounts: AccountRegistry,
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
    attachment_reader: attachment_reader::ReaderCache,
    /// `Some` only when the local database had to be recovered at startup
    /// (restored from a backup, or recreated fresh) — see `open_with_recovery`.
    recovery: Option<db::RecoveryOutcome>,
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

/// Resolves the `ConnectedAccount` entry for `account_id`, defaulting to the
/// primary account when the caller didn't name one.
async fn resolve_account<T>(
    state: &AppState,
    account_id: Option<&str>,
    pick: impl FnOnce(&ConnectedAccount) -> T,
) -> Result<T, String> {
    let account_id = match account_id {
        Some(account_id) => account_id.to_string(),
        None => state.database.primary_account_id(),
    };
    state
        .accounts
        .lock()
        .await
        .get(&account_id)
        .map(pick)
        .ok_or_else(|| format!("{account_id} is not connected. Reconnect it before continuing."))
}

/// Every connected account's sync engine, or just one account's when named.
/// An unknown `account_id` yields no services rather than an error: callers
/// here are best-effort refreshes, not user-visible operations.
async fn sync_services(state: &AppState, account_id: Option<&str>) -> Vec<SyncService> {
    let accounts = state.accounts.lock().await;
    match account_id {
        Some(account_id) => accounts
            .get(account_id)
            .map(|account| vec![account.sync.clone()])
            .unwrap_or_default(),
        None => accounts
            .values()
            .map(|account| account.sync.clone())
            .collect(),
    }
}

fn merge_sync_statuses(
    statuses: Vec<(String, SyncStatus)>,
    mut additional_errors: Vec<String>,
) -> SyncStatus {
    let single_account = statuses.len() == 1;
    let mut last_successful_sync = None;
    let mut cursor = None;
    let mut pending_mutations = 0;
    let mut failed_mutations = Vec::new();
    let mut quarantined_messages = Vec::new();

    for (email, status) in statuses {
        if status.last_successful_sync > last_successful_sync {
            last_successful_sync = status.last_successful_sync;
        }
        if single_account {
            cursor = status.cursor;
        }
        pending_mutations += status.pending_mutations;
        failed_mutations.extend(status.failed_mutations);
        quarantined_messages.extend(status.quarantined_messages);
        if let Some(error) = status.error {
            additional_errors.push(format!("{email}: {error}"));
        }
    }

    let error = (!additional_errors.is_empty()).then(|| additional_errors.join("\n"));
    SyncStatus {
        state: if error.is_some() { "error" } else { "idle" },
        last_successful_sync,
        cursor,
        pending_mutations,
        failed_mutations,
        quarantined_messages,
        error,
    }
}

#[cfg(test)]
mod combined_sync_status_tests {
    use super::merge_sync_statuses;
    use crate::models::SyncStatus;

    fn status(
        last_successful_sync: Option<&str>,
        cursor: Option<&str>,
        pending_mutations: i64,
        error: Option<&str>,
    ) -> SyncStatus {
        SyncStatus {
            state: if error.is_some() { "error" } else { "idle" },
            last_successful_sync: last_successful_sync.map(str::to_string),
            cursor: cursor.map(str::to_string),
            pending_mutations,
            failed_mutations: vec![],
            quarantined_messages: vec![],
            error: error.map(str::to_string),
        }
    }

    #[test]
    fn combines_every_accounts_sync_state_and_labels_errors() {
        let merged = merge_sync_statuses(
            vec![
                (
                    "first@example.com".into(),
                    status(Some("2026-09-18T12:00:00Z"), Some("101"), 1, None),
                ),
                (
                    "second@example.com".into(),
                    status(
                        Some("2026-09-18T12:05:00Z"),
                        Some("202"),
                        2,
                        Some("token expired"),
                    ),
                ),
            ],
            vec![],
        );

        assert_eq!(merged.state, "error");
        assert_eq!(
            merged.last_successful_sync.as_deref(),
            Some("2026-09-18T12:05:00Z")
        );
        assert_eq!(merged.cursor, None, "a merged cursor would be misleading");
        assert_eq!(merged.pending_mutations, 3);
        assert_eq!(
            merged.error.as_deref(),
            Some("second@example.com: token expired")
        );
    }

    #[test]
    fn preserves_the_cursor_for_a_single_account() {
        let merged = merge_sync_statuses(
            vec![(
                "only@example.com".into(),
                status(Some("2026-09-18T12:00:00Z"), Some("101"), 0, None),
            )],
            vec![],
        );

        assert_eq!(merged.state, "idle");
        assert_eq!(merged.cursor.as_deref(), Some("101"));
    }
}

/// Diagnostics and refresh state represent every configured account. Once an
/// account catalog exists, never fall back to the pre-connect `default` row:
/// after an in-process settings import that row is not a real mailbox.
fn combined_sync_status(state: &AppState) -> Result<SyncStatus, String> {
    let accounts = state.database.list_accounts()?;
    if accounts.is_empty() {
        return state.database.sync_status(auth::LEGACY_KEY);
    }
    let mut statuses = Vec::new();
    let mut errors = Vec::new();
    for account in accounts {
        match state.database.sync_status(&account.email) {
            Ok(status) => statuses.push((account.email, status)),
            Err(error) if account.status == "connected" => {
                errors.push(format!("{}: {error}", account.email));
            }
            Err(_) => {}
        }
    }
    Ok(merge_sync_statuses(statuses, errors))
}

/// Builds a `SyncService` for `auth`, runs an immediate sync if it's already
/// connected, and spawns its polling loop. Every account is brought up this
/// way, including the pre-connect placeholder.
fn spawn_synced_account(
    database: Arc<Database>,
    auth: GoogleAuth,
    sync_immediately: bool,
) -> ConnectedAccount {
    let service = SyncService::new(database, auth.clone());
    let polling_service = service.clone();
    let poll_task = tauri::async_runtime::spawn(async move {
        if sync_immediately && polling_service.is_connected() {
            let _ = polling_service.sync().await;
        }
        polling_service.polling_loop().await;
    });
    ConnectedAccount {
        auth,
        sync: service,
        poll_task,
    }
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
fn mailbox_unread_counts(
    account_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<MailboxUnreadCounts, String> {
    state.database.mailbox_unread_counts(account_id.as_deref())
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
                    .fetch_message(message_id)
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
    let filename = attachment_security::normalize_filename(&attachment.filename);
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
    if !image_format::is_supported_raster_mime(&mime_type) {
        return Err("Embedded attachment is not a supported image".into());
    }
    image_format::validate_raster(&bytes)?;
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
    window: tauri::Window,
    state: State<'_, AppState>,
) -> Result<(), String> {
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
        .map_err(|error| format!("Unable to save attachment: {error}"))?;
    if let Err(error) = attachment_security::quarantine(destination.path()) {
        return match std::fs::remove_file(destination.path()) {
            Ok(()) => Err(format!(
                "Unable to save attachment safely because quarantine metadata could not be applied: {error}"
            )),
            Err(remove_error) => Err(format!(
                "Quarantine metadata could not be applied to the saved attachment ({error}), and the unsafe copy could not be removed ({remove_error})"
            )),
        };
    }
    Ok(())
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
async fn backfill_search_threads(
    query: String,
    account_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if query.trim().is_empty() {
        return Ok(());
    }

    let services = sync_services(&state, account_id.as_deref()).await;

    let mut errors = Vec::new();
    for service in services {
        if let Err(error) = service.backfill_search(&query).await {
            errors.push(error);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
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
    combined_sync_status(&state)
}

/// `Some` only when this launch had to recover the local database (restored
/// from a backup, or recreated fresh) — see `db::open_with_recovery`. The
/// frontend uses this once at startup to explain a resync/empty inbox rather
/// than leaving it unexplained.
#[tauri::command]
fn recovery_status(state: State<'_, AppState>) -> Option<db::RecoveryOutcome> {
    state.recovery.clone()
}

#[tauri::command]
async fn sync_account(state: State<'_, AppState>) -> Result<SyncStatus, String> {
    let _ = state.correspondence.refresh_identity().await;
    let services = {
        let accounts = state.accounts.lock().await;
        accounts
            .iter()
            .filter(|(_, connected)| connected.sync.is_connected())
            // Report against the account's live key rather than the map key,
            // which is still the placeholder until the first identity refresh
            // rekeys it.
            .map(|(_, connected)| (connected.sync.account_id(), connected.sync.clone()))
            .collect::<Vec<_>>()
    };
    if services.is_empty() {
        return Err("No connected Google accounts are available to sync".to_string());
    }

    // Accounts are independent Gmail sessions. Run them concurrently so a
    // full sync of one imported mailbox does not delay every other mailbox.
    let mut tasks = tokio::task::JoinSet::new();
    for (email, service) in services {
        tasks.spawn(async move { (email, service.sync().await) });
    }
    let mut task_errors = Vec::new();
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok((email, Err(error))) => task_errors.push(format!("{email}: {error}")),
            Err(error) => {
                task_errors.push(format!("A mail sync task stopped unexpectedly: {error}"))
            }
            Ok((_, Ok(_))) => {}
        }
    }

    let mut status = combined_sync_status(&state)?;
    if !task_errors.is_empty() {
        let mut errors = status.error.take().into_iter().collect::<Vec<_>>();
        errors.extend(task_errors);
        let mut seen = HashSet::new();
        errors.retain(|error| seen.insert(error.clone()));
        status.error = Some(errors.join("\n"));
        status.state = "error";
    }
    Ok(status)
}

#[tauri::command]
async fn flush_pending_mutations(state: State<'_, AppState>) -> Result<SyncStatus, String> {
    let primary = state.database.primary_account_id();
    match resolve_account(&state, Some(&primary), |account| account.sync.clone()).await {
        Ok(service) => service.flush_pending().await,
        Err(_) => state.database.sync_status(&primary),
    }
}

/// Resolves the primary account's sync engine for the background refresh
/// helpers below, which are best-effort and stay silent when there is nothing
/// connected to refresh.
async fn primary_service(handle: &tauri::AppHandle) -> Option<SyncService> {
    let state = handle.try_state::<AppState>()?;
    let primary = state.database.primary_account_id();
    let service = resolve_account(&state, Some(&primary), |account| account.sync.clone())
        .await
        .ok()?;
    service.is_connected().then_some(service)
}

fn spawn_pending_flush(handle: &tauri::AppHandle) {
    let handle = handle.clone();
    tauri::async_runtime::spawn(async move {
        let Some(service) = primary_service(&handle).await else {
            return;
        };
        // Wait for an in-flight mutate_thread IPC to land in SQLite.
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        let _ = service.flush_pending().await;
    });
}

fn spawn_foreground_sync(handle: &tauri::AppHandle) {
    let handle = handle.clone();
    tauri::async_runtime::spawn(async move {
        let Some(service) = primary_service(&handle).await else {
            return;
        };
        let _ = service.sync_if_stale().await;
    });
}

#[tauri::command]
async fn google_auth_status(state: State<'_, AppState>) -> Result<AuthStatus, String> {
    let primary = state.database.primary_account_id();
    let auth = resolve_account(&state, Some(&primary), |account| account.auth.clone()).await;
    Ok(AuthStatus {
        configured: auth.is_ok(),
        connected: auth.is_ok_and(|auth| auth.available()),
    })
}

#[tauri::command]
async fn connect_google(state: State<'_, AppState>) -> Result<SyncStatus, String> {
    let primary = state.database.primary_account_id();
    let (auth, service) = resolve_account(&state, Some(&primary), |account| {
        (account.auth.clone(), account.sync.clone())
    })
    .await
    .map_err(|_| not_configured())?;
    authorize_interactively(&state, &auth).await?;
    // Rekeys the placeholder registry entry onto the real address as a side
    // effect, so the account is addressable by email from here on.
    let _ = state.correspondence.refresh_identity().await;
    service.sync().await
}

#[tauri::command]
async fn disconnect_google(state: State<'_, AppState>) -> Result<(), String> {
    let primary = state.database.primary_account_id();
    remove_account(primary, state).await
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
    let connected = spawn_synced_account(state.database.clone(), auth, true);
    state.accounts.lock().await.insert(email, connected);
    Ok(account)
}

#[tauri::command]
async fn remove_account(email: String, state: State<'_, AppState>) -> Result<(), String> {
    let _guard = state.correspondence.gate.lock().await;
    let config = state.auth_config.as_ref().ok_or_else(not_configured)?;
    state.database.pause_ready_sends_for(&email)?;
    // Purge local data first: if this fails, the account is untouched and
    // its credentials are still live, so the caller can safely retry rather
    // than being left with a still-listed account whose credentials are
    // already gone.
    state.database.remove_account(&email)?;
    let result = match state.accounts.lock().await.remove(&email) {
        Some(connected) => {
            connected.poll_task.abort();
            connected.auth.disconnect()
        }
        None => config.account(&email).disconnect(),
    };
    // Removing the last account returns the app to its pre-connect state, so
    // restore the placeholder entry `connect_google` authorizes against —
    // otherwise disconnecting would leave no way back in.
    ensure_placeholder_account(&state).await;
    result
}

/// Seeds the pre-connect placeholder entry when no account is connected, so
/// the registry is never empty while OAuth is configured.
async fn ensure_placeholder_account(state: &AppState) {
    let Some(config) = state.auth_config.as_ref() else {
        return;
    };
    let mut accounts = state.accounts.lock().await;
    if accounts.is_empty() {
        accounts.insert(
            auth::LEGACY_KEY.to_string(),
            spawn_synced_account(state.database.clone(), config.legacy_account(), false),
        );
    }
}

#[tauri::command]
async fn reconnect_account(email: String, state: State<'_, AppState>) -> Result<Account, String> {
    let config = state.auth_config.as_ref().ok_or_else(not_configured)?;
    let auth = match state.accounts.lock().await.get(&email) {
        Some(connected) => connected.auth.clone(),
        None => config.account(&email),
    };
    authorize_interactively(&state, &auth).await?;
    // Make the persisted status authoritative before any newly spawned
    // service checks it. Previously the service could observe needs_reauth,
    // skip its initial sync, and sleep until the first polling interval.
    let account = state.database.adopt_account(&email)?;
    state.database.ensure_compose_identity(&email)?;
    let service = {
        // Self-heal: an account already in the `accounts` table should
        // always have a live poller from startup, but reconnecting is a
        // reasonable place to notice and repair a missing one.
        let mut accounts = state.accounts.lock().await;
        match accounts.get(&email) {
            Some(connected) => connected.sync.clone(),
            None => {
                // Reconnect awaits the first sync below, so the poller itself
                // must not launch a duplicate initial sync.
                let connected = spawn_synced_account(state.database.clone(), auth, false);
                let service = connected.sync.clone();
                accounts.insert(email.clone(), connected);
                service
            }
        }
    };
    // A completed reconnect means credentials were accepted *and* the first
    // mailbox synchronization finished (or returned a useful error).
    service.sync().await?;
    Ok(account)
}

#[tauri::command]
fn list_calendar_accounts(state: State<'_, AppState>) -> Result<Vec<CalendarAccount>, String> {
    let config = state.auth_config.as_ref();
    let mut accounts = state.database.list_calendar_accounts()?;
    for account in &mut accounts {
        if !config.is_some_and(|config| config.calendar_account(&account.email).available()) {
            account.status = "needs_reauth".to_string();
        }
    }
    Ok(accounts)
}

#[tauri::command]
async fn add_calendar_account(state: State<'_, AppState>) -> Result<CalendarAccount, String> {
    let config = state.auth_config.as_ref().ok_or_else(not_configured)?;
    let auth = config.pending_calendar_account();
    let email = authorize_interactively(&state, &auth).await?;
    state.database.adopt_calendar_account(&email)?;
    state
        .database
        .list_calendar_accounts()?
        .into_iter()
        .find(|account| account.email == email)
        .ok_or_else(|| "Calendar account was not saved".to_string())
}

#[tauri::command]
async fn reconnect_calendar_account(
    email: String,
    state: State<'_, AppState>,
) -> Result<CalendarAccount, String> {
    let config = state.auth_config.as_ref().ok_or_else(not_configured)?;
    let auth = config.calendar_account(&email);
    authorize_interactively(&state, &auth).await?;
    state.database.adopt_calendar_account(&email)?;
    state
        .database
        .list_calendar_accounts()?
        .into_iter()
        .find(|account| account.email == email)
        .ok_or_else(|| "Calendar account was not saved".to_string())
}

async fn calendar_options_for_account(
    config: &GoogleAuthConfig,
    database: &Database,
    email: &str,
) -> Result<Vec<CalendarOption>, String> {
    let mut options =
        calendar::list_calendar_options(config.calendar_account(email), email).await?;
    let selected = match database.calendar_selection(email)? {
        Some(selected) => selected,
        None => {
            let defaults = options
                .iter()
                .filter(|option| option.primary)
                .map(|option| option.id.clone())
                .collect::<Vec<_>>();
            database.set_calendar_selection(email, &defaults)?;
            defaults
        }
    };
    let selected = selected.into_iter().collect::<HashSet<_>>();
    for option in &mut options {
        option.selected = selected.contains(&option.id);
    }
    Ok(options)
}

#[tauri::command]
async fn list_calendar_options(
    state: State<'_, AppState>,
) -> Result<Vec<CalendarOption>, String> {
    let config = state.auth_config.as_ref().ok_or_else(not_configured)?;
    let mut options = Vec::new();
    for account in state.database.list_calendar_accounts()? {
        options.extend(
            calendar_options_for_account(config, &state.database, &account.email).await?,
        );
    }
    Ok(options)
}

#[tauri::command]
async fn set_calendar_selection(
    account_id: String,
    calendar_ids: Vec<String>,
    state: State<'_, AppState>,
) -> Result<Vec<CalendarOption>, String> {
    let config = state.auth_config.as_ref().ok_or_else(not_configured)?;
    let mut available =
        calendar::list_calendar_options(config.calendar_account(&account_id), &account_id).await?;
    let available_ids = available
        .iter()
        .map(|option| option.id.as_str())
        .collect::<HashSet<_>>();
    let unique = calendar_ids.iter().collect::<HashSet<_>>();
    if unique.len() != calendar_ids.len()
        || calendar_ids
            .iter()
            .any(|calendar_id| !available_ids.contains(calendar_id.as_str()))
    {
        return Err("Calendar selection contained an unknown or duplicate calendar".to_string());
    }
    state
        .database
        .set_calendar_selection(&account_id, &calendar_ids)?;
    for option in &mut available {
        option.selected = unique.contains(&option.id);
    }
    Ok(available)
}

#[tauri::command]
fn remove_calendar_account(email: String, state: State<'_, AppState>) -> Result<(), String> {
    let config = state.auth_config.as_ref().ok_or_else(not_configured)?;
    config.calendar_account(&email).disconnect()?;
    state.database.remove_calendar_account(&email)
}

#[tauri::command]
async fn list_schedule_events(
    time_min: String,
    time_max: String,
    time_zone: String,
    state: State<'_, AppState>,
) -> Result<ScheduleResult, String> {
    let config = state.auth_config.as_ref().ok_or_else(not_configured)?;
    let accounts = state.database.list_calendar_accounts()?;
    if accounts.is_empty() {
        return Err("Connect a Google Calendar account in Settings first.".to_string());
    }
    let mut merged = Vec::new();
    let mut errors = Vec::new();
    for account in accounts {
        let calendar_ids = match state.database.calendar_selection(&account.email)? {
            Some(selected) => selected,
            None => match calendar_options_for_account(
                config,
                &state.database,
                &account.email,
            )
            .await
            {
                Ok(options) => options
                    .into_iter()
                    .filter(|option| option.selected)
                    .map(|option| option.id)
                    .collect(),
                Err(error) => {
                    errors.push(format!("{}: {error}", account.email));
                    continue;
                }
            },
        };
        if calendar_ids.is_empty() {
            continue;
        }
        match calendar::fetch_schedule(
            config.calendar_account(&account.email),
            &account.email,
            &calendar_ids,
            &time_min,
            &time_max,
            &time_zone,
        )
        .await
        {
            Ok(mut events) => {
                merged.append(&mut events);
            }
            Err(error) => errors.push(format!("{}: {error}", account.email)),
        }
    }
    merged.sort_by(|left, right| left.start.cmp(&right.start));
    Ok(ScheduleResult {
        events: merged,
        errors,
    })
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
    state.database.create_split_inbox(
        &request.name,
        &request.match_kind,
        &request.match_value,
        &request.account_id,
    )
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
    offset: usize,
    limit: usize,
    state: State<'_, AppState>,
) -> Result<ThreadPage, String> {
    state.database.list_split_inbox_page(&split_inbox_id, offset, limit)
}

#[tauri::command]
async fn list_labels(
    account_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<Label>, String> {
    let auth = resolve_account(&state, account_id.as_deref(), |account| account.auth.clone()).await?;

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
    resolve_sync(&state, request.account_id.as_deref())
        .await?
        .create_label(&request.name)
        .await
}

#[tauri::command]
async fn update_label(
    request: UpdateLabelRequest,
    state: State<'_, AppState>,
) -> Result<Label, String> {
    resolve_sync(&state, request.account_id.as_deref())
        .await?
        .update_label(&request.id, &request.name)
        .await
}

#[tauri::command]
async fn delete_label(
    id: String,
    account_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    resolve_sync(&state, account_id.as_deref())
        .await?
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
    provider: ai::AiProvider,
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

#[tauri::command]
fn ai_reply_assist_context(
    draft_id: String,
    state: State<'_, AppState>,
) -> Result<ReplyAssistContext, String> {
    let draft = state.correspondence.database.draft(&draft_id)?;
    if !matches!(draft.mode.as_str(), "reply" | "replyAll") {
        return Err("Reply Assist is only available for reply drafts".to_string());
    }
    let source_id = draft
        .source_id
        .ok_or_else(|| "Reply draft has no source message".to_string())?;
    let detail = state.database.get_thread_for_message(&source_id)?;
    Ok(ai::reply_context(
        detail.thread.subject,
        detail
            .messages
            .into_iter()
            .map(|message| ai::ThreadMessageInput {
                sender: message.sender,
                sent_at: message.sent_at,
                body_text: message.body_text,
            })
            .collect(),
    ))
}

#[tauri::command]
async fn ai_generate_reply(
    context: ReplyAssistContext,
    instruction: String,
    provider: ai::AiProvider,
    model: String,
    endpoint: Option<String>,
) -> Result<ReplyAssistResult, String> {
    let api_key = ai::get_key()?.ok_or_else(|| "No AI API key configured".to_string())?;
    // Re-apply the native bounds even though normal callers received this
    // context from `ai_reply_assist_context`; the webview is not trusted to
    // enforce request size or cost limits.
    let bounded = ai::reply_context(
        context.subject,
        context
            .messages
            .into_iter()
            .map(|message| ai::ThreadMessageInput {
                sender: message.sender,
                sent_at: message.sent_at,
                body_text: message.body_text,
            })
            .collect(),
    );
    let body = ai::generate_reply(
        &bounded,
        &instruction,
        provider,
        &model,
        endpoint.as_deref(),
        &api_key,
    )
    .await?;
    Ok(ReplyAssistResult { body })
}

fn not_configured() -> String {
    "Google OAuth is not configured. Set THREESTRANDS_GOOGLE_CLIENT_ID and \
     THREESTRANDS_GOOGLE_CLIENT_SECRET from a Desktop app credential."
        .into()
}

/// Resolves the sync engine for a given account, falling back to the primary
/// account when `account_id` is `None`. Label mutations must run against the
/// account that actually owns the label, not always the primary account.
async fn resolve_sync(
    state: &State<'_, AppState>,
    account_id: Option<&str>,
) -> Result<SyncService, String> {
    resolve_account(state, account_id, |account| account.sync.clone()).await
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
            let (opened_database, recovery) =
                db::open_with_recovery(&data_dir.join("threestrands.sqlite"));
            let database = Arc::new(opened_database);
            let recovery = match recovery {
                db::RecoveryOutcome::Clean => None,
                other => Some(other),
            };
            if recovery.is_some() {
                // A restored-from-backup database's cursors reflect
                // whatever history state that snapshot was taken at, which
                // may since have diverged from Gmail; a fresh database has
                // no cursor at all. Either way, force each known account's
                // next sync to be a full reconciliation rather than trusting
                // a possibly-stale incremental cursor.
                if let Ok(accounts) = database.list_accounts() {
                    for account in &accounts {
                        let _ = database.clear_cursor(&account.email);
                    }
                }
            }
            let auth_config = GoogleAuthConfig::from_environment().ok();
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
                            // Bound WAL growth, then snapshot: checkpointing
                            // first means the backup reflects the latest
                            // writes without carrying an ever-growing WAL of
                            // its own.
                            let _ = prune_db.checkpoint_wal();
                            let _ = prune_db.create_periodic_backup();
                        })
                        .await;
                        tokio::time::sleep(std::time::Duration::from_secs(6 * 60 * 60)).await;
                    }
                });
            }
            let root = data_dir.join("attachments");
            std::fs::create_dir_all(&root)?;
            restrict_dir_to_owner(&root);
            let attachment_reader = attachment_reader::ReaderCache::new(root.join("reader"))?;
            // Bring up every account in the catalog, each with its own
            // credentials and polling loop. Before the very first connect the
            // catalog is empty and the placeholder key stands in for the
            // account about to be added, so `connect_google` has something to
            // authorize; it rekeys onto the real address once the identity is
            // known. Built before `manage` so the registry is never observed
            // empty by a command racing startup.
            let mut registry = HashMap::new();
            if let Some(config) = &auth_config {
                let catalog = database.list_accounts().unwrap_or_default();
                let keys = if catalog.is_empty() {
                    vec![auth::LEGACY_KEY.to_string()]
                } else {
                    catalog.into_iter().map(|account| account.email).collect()
                };
                for key in keys {
                    let auth = if key == auth::LEGACY_KEY {
                        config.legacy_account()
                    } else {
                        config.account(&key)
                    };
                    registry.insert(key, spawn_synced_account(database.clone(), auth, true));
                }
            }
            let accounts: AccountRegistry = Arc::new(tokio::sync::Mutex::new(registry));
            let correspondence = correspondence::Correspondence {
                database: database.clone(),
                accounts: accounts.clone(),
                root,
                gate: Arc::new(tokio::sync::Mutex::new(())),
                edits: Arc::new(tokio::sync::Mutex::new(())),
            };
            let worker = correspondence.clone();
            tauri::async_runtime::spawn(async move {
                if worker.is_connected().await {
                    let _ = worker.refresh_identity().await;
                }
                worker.run().await;
            });
            app.manage(AppState {
                database,
                auth_config,
                accounts,
                correspondence,
                authorize_slot: AuthorizeSlot::default(),
                exiting: std::sync::atomic::AtomicBool::new(false),
                image_cache: image_proxy::ImageCache::new().map_err(std::io::Error::other)?,
                attachment_reader,
                recovery,
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
            mailbox_unread_counts,
            list_all_mail_page,
            list_trash_page,
            get_thread,
            fetch_remote_image,
            fetch_attachment_image,
            preview_calendar_attachment,
            open_attachment,
            save_attachment,
            search_threads,
            backfill_search_threads,
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
            recovery_status,
            google_auth_status,
            connect_google,
            disconnect_google,
            list_accounts,
            add_account,
            remove_account,
            reconnect_account,
            list_calendar_accounts,
            add_calendar_account,
            reconnect_calendar_account,
            list_calendar_options,
            set_calendar_selection,
            remove_calendar_account,
            list_schedule_events,
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
            ai_reply_assist_context,
            ai_generate_reply,
            system_fonts::list_system_font_families,
        ])
        .build(tauri::generate_context!())
        .expect("error while building ThreeStrands")
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
