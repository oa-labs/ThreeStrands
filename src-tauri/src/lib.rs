mod ai;
mod availability;
mod attachment_reader;
mod attachment_security;
mod auth;
mod backoff;
mod calendar;
mod correspondence;
mod credentials;
mod db;
mod endpoint_origin;
mod enrollment;
mod error_text;
mod image_format;
mod image_proxy;
mod ipfs_transport;
mod limits;
mod mime;
mod models;
mod net_safety;
mod provider;
mod replicated_sync;
mod s3_transport;
mod schema;
mod sync;
mod sync_connectors;
mod sync_folder;
mod sync_projection;
mod system_fonts;
mod transfer;
#[path = "unsubscribe.rs"]
mod unsubscribe_service;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use auth::{AccountAuth, GoogleAuthConfig};
use chrono::Utc;
use db::Database;
use models::{
    ActionProposal, Account, AuthStatus, BusyInterval, CalendarAccount, CalendarOption, CheckProposedTimeRequest, ContactSuggestion, CreateLabelRequest,
    CreateSnippetRequest, CreateSplitInboxRequest, Label, MailboxUnreadCounts, ReplyAssistContext, ReplyAssistResult,
    FindAvailabilityRequest, ProposedTimeCheck, ScheduleResult, SearchThreadsRequest, Snippet, SplitInbox, SummaryResult, SyncStatus, Thread,
    ThreadDetail, ThreadMutation, ThreadPage, ThreadTask, TriageEvent, TriageSenderStats,
    UpdateLabelRequest, UpdateSnippetRequest, UpdateSplitInboxRequest, CreateTaskRequest, UpdateTaskRequest,
};
use sync::SyncService;
use tauri::{async_runtime::JoinHandle, Manager, State};
use tokio_util::sync::CancellationToken;
use url::Url;

/// Records a background operation's failure instead of silently discarding
/// it. Used where there is no caller to hand the error back to (spawned
/// loops, best-effort startup and maintenance steps), so failures still land
/// in the app log.
fn log_failure<T, E: std::fmt::Display>(operation: &str, result: Result<T, E>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            log::warn!(target: "threestrands", "{operation} failed: {error}");
            None
        }
    }
}

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
                log_failure("opening external link", open::that(url.as_str()));
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
    pub(crate) auth: AccountAuth,
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
    /// The pluggable replicated-sync engine (see `replicated_sync.rs`).
    /// Inert unless the beta toggle or `THREESTRANDS_REPLICATED_SYNC` turns
    /// it on — see `Database::replicated_sync_active`.
    replicated_sync: replicated_sync::ReplicatedSync,
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
    /// Explicit thread-action proposals are session-only. The cache key
    /// includes the newest message timestamp so a newly synced message can
    /// never reuse an older analysis.
    proposal_cache: Arc<std::sync::Mutex<HashMap<String, Vec<ActionProposal>>>>,
}

/// SQLite and the mutex guarding its connection are synchronous. Run database
/// work reached from async code on Tokio's blocking pool so contention or a
/// slow query cannot stall an async worker.
async fn run_database_task<T, E, F>(operation: F) -> Result<T, String>
where
    T: Send + 'static,
    E: Into<String> + Send + 'static,
    F: FnOnce() -> Result<T, E> + Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|error| format!("Database task stopped unexpectedly: {error}"))?
        .map_err(Into::into)
}

fn database_result<T>(result: db::DbResult<T>) -> Result<T, String> {
    result.map_err(String::from)
}

#[cfg(test)]
mod database_task_tests {
    use super::run_database_task;

    #[tokio::test(flavor = "current_thread")]
    async fn database_work_runs_off_the_async_worker() {
        let async_thread = std::thread::current().id();
        let database_thread = run_database_task(|| Ok::<_, String>(std::thread::current().id()))
            .await
            .unwrap();

        assert_ne!(database_thread, async_thread);
    }
}

#[cfg(test)]
mod background_failure_tests {
    use super::{force_full_resync, log_failure, run_storage_maintenance};
    use crate::db::Database;

    #[test]
    fn log_failure_passes_through_success_and_absorbs_errors() {
        assert_eq!(log_failure("test step", Ok::<_, String>(7)), Some(7));
        assert_eq!(log_failure("test step", Err::<u8, _>("boom".to_string())), None);
    }

    #[test]
    fn storage_maintenance_runs_every_step_cleanly_on_a_healthy_database() {
        let database = Database::open_memory();
        assert!(run_storage_maintenance(&database).is_empty());
    }

    #[test]
    fn full_resync_after_recovery_tolerates_a_database_without_cursors() {
        let database = Database::open_memory();
        force_full_resync(&database);
        assert!(database.list_accounts().is_ok());
    }
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
async fn authorize_interactively(state: &AppState, auth: &AccountAuth) -> Result<String, String> {
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
    log_failure(
        "restricting data directory permissions",
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)),
    );
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
fn combined_sync_status(database: &Database) -> Result<SyncStatus, String> {
    let accounts = database.list_accounts()?;
    if accounts.is_empty() {
        return database_result(database.sync_status(auth::LEGACY_KEY));
    }
    let mut statuses = Vec::new();
    let mut errors = Vec::new();
    for account in accounts {
        match database.sync_status(&account.email) {
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
    auth: AccountAuth,
    sync_immediately: bool,
    app: tauri::AppHandle,
) -> ConnectedAccount {
    let service = SyncService::new(database, auth.clone());
    let polling_service = service.clone();
    let poll_task = tauri::async_runtime::spawn(async move {
        if sync_immediately && polling_service.is_connected() {
            log_failure("initial account sync", polling_service.sync().await);
        }
        polling_service
            .polling_loop(move |account_id: &str| {
                use tauri::Emitter;
                let _ = app.emit("unread-counts-changed", account_id);
            })
            .await;
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
    database_result(state.database.list_threads(account_id.as_deref()))
}

#[tauri::command]
fn list_all_mail(
    account_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<Thread>, String> {
    database_result(state.database.list_all_mail(account_id.as_deref()))
}

#[tauri::command]
fn list_trash(
    account_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<Thread>, String> {
    database_result(state.database.list_trash(account_id.as_deref()))
}

#[tauri::command]
fn list_threads_page(
    account_id: Option<String>,
    offset: usize,
    limit: usize,
    state: State<'_, AppState>,
) -> Result<ThreadPage, String> {
    database_result(state.database.list_threads_page(account_id.as_deref(), offset, limit))
}

#[tauri::command]
fn list_unread_counts(state: State<'_, AppState>) -> Result<HashMap<String, i64>, String> {
    database_result(state.database.list_unread_counts())
}

#[tauri::command]
fn mailbox_unread_counts(
    account_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<MailboxUnreadCounts, String> {
    database_result(state.database.mailbox_unread_counts(account_id.as_deref()))
}

#[tauri::command]
fn list_all_mail_page(
    account_id: Option<String>,
    offset: usize,
    limit: usize,
    state: State<'_, AppState>,
) -> Result<ThreadPage, String> {
    database_result(state.database.list_all_mail_page(account_id.as_deref(), offset, limit))
}

#[tauri::command]
fn list_trash_page(
    account_id: Option<String>,
    offset: usize,
    limit: usize,
    state: State<'_, AppState>,
) -> Result<ThreadPage, String> {
    database_result(state.database.list_trash_page(account_id.as_deref(), offset, limit))
}

#[tauri::command]
fn get_thread(id: String, state: State<'_, AppState>) -> Result<ThreadDetail, String> {
    database_result(state.database.get_thread(&id))
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
    let database = state.database.clone();
    let stored_message_id = message_id.to_string();
    let stored_attachment_id = attachment_id.to_string();
    let (account_id, attachment, payload_bytes) = run_database_task(move || {
        let (account_id, message) = database.attachment_message(&stored_message_id)?;
        let attachment = mime::normalize(&message)?
            .attachments
            .into_iter()
            .find(|attachment| attachment.id == stored_attachment_id)
            .ok_or("Attachment not found")?;
        let bytes = mime::attachment_bytes_from_payload(&message, &stored_attachment_id)?;
        Ok::<_, String>((account_id, attachment, bytes))
    })
    .await?;
    let bytes = match payload_bytes {
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
    database_result(state.database.search_threads(&request, account_id.as_deref()))
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
    database_result(state.database.mutate_thread(&mutation))
}

#[tauri::command]
fn mutate_threads(
    mutations: Vec<ThreadMutation>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    database_result(state.database.mutate_threads(&mutations))
}

#[tauri::command]
fn record_triage_event(event: TriageEvent, state: State<'_, AppState>) -> Result<(), String> {
    database_result(state.database.record_triage_event(&event))
}

#[tauri::command]
fn list_triage_sender_stats(
    account_id: String,
    limit: Option<usize>,
    state: State<'_, AppState>,
) -> Result<Vec<TriageSenderStats>, String> {
    database_result(state.database.list_triage_sender_stats(&account_id, limit.unwrap_or(100)))
}

#[tauri::command]
fn list_contact_suggestions(
    account_id: String,
    query: String,
    limit: Option<usize>,
    state: State<'_, AppState>,
) -> Result<Vec<ContactSuggestion>, String> {
    database_result(state.database.list_contact_suggestions(&account_id, &query, limit.unwrap_or(8)))
}

#[tauri::command]
fn pin_contact(
    account_id: String,
    email: String,
    display_name: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    database_result(state.database.pin_contact(&account_id, &email, display_name.as_deref()))
}

#[tauri::command]
fn unpin_contact(
    account_id: String,
    email: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    database_result(state.database.unpin_contact(&account_id, &email))
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
    combined_sync_status(state.database.as_ref())
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
    log_failure(
        "refreshing account identity",
        state.correspondence.refresh_identity().await,
    );
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

    let database = state.database.clone();
    let mut status = run_database_task(move || combined_sync_status(database.as_ref())).await?;
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
        Err(_) => database_result(state.database.sync_status(&primary)),
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
        log_failure("flushing pending mutations", service.flush_pending().await);
    });
}

fn spawn_foreground_sync(handle: &tauri::AppHandle) {
    let handle = handle.clone();
    tauri::async_runtime::spawn(async move {
        if let Some(state) = handle.try_state::<AppState>() {
            let engine = state.replicated_sync.clone();
            log_failure("foreground replicated sync", engine.sync_once().await);
            reconcile_account_registry(&handle);
        }
        if let Some(service) = primary_service(&handle).await {
            log_failure("foreground mail sync", service.sync_if_stale().await);
        }
    });
}

/// Stops the polling loop of every connected account that a synchronized
/// deletion from another device removed from the local catalog.
fn reconcile_account_registry(handle: &tauri::AppHandle) {
    let handle = handle.clone();
    tauri::async_runtime::spawn(async move {
        let Some(state) = handle.try_state::<AppState>() else { return; };
        let catalog = state.database.list_accounts().unwrap_or_default().into_iter().map(|account| account.email).collect::<HashSet<_>>();
        let mut accounts = state.accounts.lock().await;
        let removed = accounts.keys().filter(|email| *email != auth::LEGACY_KEY && !catalog.contains(*email)).cloned().collect::<Vec<_>>();
        for email in removed {
            if let Some(account) = accounts.remove(&email) { account.poll_task.abort(); }
        }
    });
}

fn kick_replicated_sync(state: &State<'_, AppState>) {
    if !state.database.replicated_sync_active().unwrap_or(false) {
        return;
    }
    let engine = state.replicated_sync.clone();
    tauri::async_runtime::spawn(async move {
        log_failure("replicated sync", engine.sync_once().await);
    });
}

fn record_synced_value<T: serde::Serialize>(
    state: &State<'_, AppState>,
    entity_type: threestrands_sync_protocol::EntityType,
    entity_id: &str,
    value: &T,
    fields: Option<std::collections::BTreeSet<String>>,
) -> Result<(), String> {
    state.database.record_local_entity_write(
        entity_type,
        entity_id,
        serde_json::to_value(value).map_err(|error| error.to_string())?,
        fields,
    )?;
    kick_replicated_sync(state);
    Ok(())
}

fn record_mail_account(state: &State<'_, AppState>, account: &Account) -> Result<(), String> {
    record_synced_value(
        state,
        threestrands_sync_protocol::EntityType::MailAccount,
        &account.email.to_ascii_lowercase(),
        &serde_json::json!({
            "email": account.email,
            "displayName": account.display_name,
            "color": account.color,
            "provider": account.provider,
            "sortOrder": account.sort_order,
        }),
        None,
    )
}

#[tauri::command]
fn synced_preferences(state: State<'_, AppState>) -> Result<Option<serde_json::Value>, String> {
    state.database.synced_preferences()
}

#[tauri::command]
fn update_synced_preferences(preferences: serde_json::Value, state: State<'_, AppState>) -> Result<(), String> {
    let current = state.database.synced_preferences()?;
    let has_synced_record = state.database.synced_preferences_recorded()?;
    let fields = preferences
        .as_object()
        .ok_or_else(|| "Synced preferences must be an object".to_string())?
        .iter()
        .filter_map(|(key, value)| {
            (!has_synced_record || current.as_ref().and_then(|current| current.get(key)) != Some(value)).then_some(key.clone())
        })
        .collect::<std::collections::BTreeSet<_>>();
    if fields.is_empty() {
        return Ok(());
    }
    state.database.record_local_entity_write(
        threestrands_sync_protocol::EntityType::Preferences,
        "portable",
        preferences,
        Some(fields),
    )?;
    kick_replicated_sync(&state);
    Ok(())
}

#[tauri::command]
fn replicated_sync_enabled(state: State<'_, AppState>) -> Result<bool, String> {
    state.database.replicated_sync_active()
}

#[tauri::command]
fn replicated_sync_beta_enabled(state: State<'_, AppState>) -> Result<bool, String> {
    state.database.beta_features_enabled()
}

#[tauri::command]
fn replicated_sync_set_beta_enabled(on: bool, state: State<'_, AppState>) -> Result<(), String> {
    state.database.set_beta_features_enabled(on)?;
    kick_replicated_sync(&state);
    Ok(())
}

#[tauri::command]
async fn replicated_sync_status(
    state: State<'_, AppState>,
) -> Result<Vec<replicated_sync::ReplicatedSyncTransportStatus>, String> {
    state.replicated_sync.status().await
}

#[tauri::command]
async fn replicated_sync_add_folder(
    state: State<'_, AppState>,
) -> Result<Option<replicated_sync::ReplicatedSyncTransportStatus>, String> {
    if !state.database.replicated_sync_active()? {
        return Err("Replicated sync is not enabled in this build".to_string());
    }
    let Some(folder) = rfd::AsyncFileDialog::new()
        .set_title("Select a folder to sync through")
        .pick_folder()
        .await
    else {
        return Ok(None);
    };
    let instance_id = format!("folder-{}", uuid::Uuid::new_v4());
    state.database.add_folder_transport(&instance_id, folder.path())?;
    kick_replicated_sync(&state);
    let statuses = state.replicated_sync.status().await?;
    Ok(statuses.into_iter().find(|status| status.instance_id == instance_id))
}

#[tauri::command]
async fn replicated_sync_add_ipfs_rpc(
    base_url: String,
    token: Option<String>,
    state: State<'_, AppState>,
) -> Result<Option<replicated_sync::ReplicatedSyncTransportStatus>, String> {
    if !state.database.replicated_sync_active()? {
        return Err("Replicated sync is not enabled in this build".to_string());
    }
    let instance_id = format!("ipfs-rpc-{}", uuid::Uuid::new_v4());
    state.database.add_ipfs_rpc_transport(&instance_id, &base_url, token.as_deref())?;
    kick_replicated_sync(&state);
    let statuses = state.replicated_sync.status().await?;
    Ok(statuses.into_iter().find(|status| status.instance_id == instance_id))
}

#[tauri::command]
async fn replicated_sync_probe_s3(
    config: s3_transport::S3Config,
    credentials: s3_transport::S3Credentials,
    state: State<'_, AppState>,
) -> Result<replicated_sync::S3ConnectionTest, String> {
    if !state.database.replicated_sync_active()? {
        return Err("Replicated sync is not enabled in this build".to_string());
    }
    state.replicated_sync.probe_s3(&config, &credentials).await
}

#[tauri::command]
async fn replicated_sync_add_s3(
    config: s3_transport::S3Config,
    credentials: s3_transport::S3Credentials,
    state: State<'_, AppState>,
) -> Result<Option<replicated_sync::ReplicatedSyncTransportStatus>, String> {
    if !state.database.replicated_sync_active()? {
        return Err("Replicated sync is not enabled in this build".to_string());
    }
    let instance_id = format!("s3-{}", uuid::Uuid::new_v4());
    state.database.add_s3_transport(&instance_id, &config, &credentials)?;
    kick_replicated_sync(&state);
    let statuses = state.replicated_sync.status().await?;
    Ok(statuses.into_iter().find(|status| status.instance_id == instance_id))
}

#[tauri::command]
async fn replicated_sync_update_connector(
    instance_id: String,
    label: Option<String>,
    credentials: Option<sync_connectors::ConnectorCredentials>,
    state: State<'_, AppState>,
) -> Result<Option<replicated_sync::ReplicatedSyncTransportStatus>, String> {
    if !state.database.replicated_sync_active()? {
        return Err("Replicated sync is not enabled in this build".to_string());
    }
    state.replicated_sync.update_connector(&instance_id, label.as_deref(), credentials)?;
    kick_replicated_sync(&state);
    let statuses = state.replicated_sync.status().await?;
    Ok(statuses.into_iter().find(|status| status.instance_id == instance_id))
}

#[tauri::command]
async fn replicated_sync_create_join_code(
    expires_in_hours: u32,
    connectors: Vec<enrollment::JoinCodeConnectorChoice>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    if !state.database.replicated_sync_active()? {
        return Err("Replicated sync is not enabled in this build".to_string());
    }
    state.replicated_sync.create_join_code(&connectors, expires_in_hours).await
}

#[tauri::command]
fn replicated_sync_list_join_codes(state: State<'_, AppState>) -> Result<Vec<enrollment::OutstandingJoinCode>, String> {
    state.database.outstanding_join_codes()
}

#[tauri::command]
async fn replicated_sync_cancel_join_code(invitation_cid: String, state: State<'_, AppState>) -> Result<(), String> {
    if !state.database.replicated_sync_active()? {
        return Err("Replicated sync is not enabled in this build".to_string());
    }
    state.replicated_sync.cancel_join_code(&invitation_cid).await
}

/// Parses pasted join code text without saving or contacting anything.
#[tauri::command]
fn replicated_sync_preview_join_code(code: String) -> Result<enrollment::JoinCodePreview, String> {
    enrollment::preview_join_code(&code, chrono::Utc::now().timestamp_millis())
}

/// Opens the native folder picker for a join code's shared-folder
/// connector. `None` if the user cancels.
#[tauri::command]
async fn replicated_sync_pick_join_folder() -> Result<Option<String>, String> {
    Ok(rfd::AsyncFileDialog::new()
        .set_title("Choose this device's copy of the shared sync folder")
        .pick_folder()
        .await
        .map(|folder| folder.path().to_string_lossy().into_owned()))
}

#[tauri::command]
async fn replicated_sync_join_with_code(
    code: String,
    folders: Vec<enrollment::JoinFolderChoice>,
    credentials: Vec<enrollment::JoinCredentialsChoice>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.replicated_sync.join_with_code(&code, &folders, credentials).await?;
    kick_replicated_sync(&state);
    Ok(())
}

#[tauri::command]
fn replicated_sync_join_code_notices(state: State<'_, AppState>) -> Result<Vec<enrollment::JoinCodeNotice>, String> {
    state.database.join_code_notices()
}

#[tauri::command]
fn replicated_sync_dismiss_join_code_notice(redemption_cid: String, state: State<'_, AppState>) -> Result<(), String> {
    state.database.dismiss_join_code_notice(&redemption_cid)
}

#[tauri::command]
async fn replicated_sync_probe_ipfs_rpc(
    base_url: String,
    token: Option<String>,
    state: State<'_, AppState>,
) -> Result<ipfs_transport::ProbeReport, String> {
    state.replicated_sync.probe_ipfs_rpc_endpoint(&base_url, token.as_deref()).await
}

#[tauri::command]
fn replicated_sync_enrollment_status(state: State<'_, AppState>) -> Result<enrollment::EnrollmentStatus, String> {
    state.database.enrollment_status()
}

#[tauri::command]
fn replicated_sync_pending_requests(state: State<'_, AppState>) -> Result<Vec<enrollment::IncomingEnrollmentRequest>, String> {
    state.database.pending_incoming_enrollment_requests()
}

#[tauri::command]
fn replicated_sync_device_roster(state: State<'_, AppState>) -> Result<Vec<enrollment::DeviceRosterEntry>, String> {
    state.database.device_roster()
}

#[tauri::command]
async fn replicated_sync_begin_genesis(allow_existing_space: bool, state: State<'_, AppState>) -> Result<String, String> {
    let phrase = state.replicated_sync.begin_genesis(allow_existing_space).await?;
    kick_replicated_sync(&state);
    Ok(phrase)
}

#[tauri::command]
async fn replicated_sync_inspect_space(state: State<'_, AppState>) -> Result<enrollment::SyncSpacePresence, String> {
    Ok(state.replicated_sync.inspect_sync_space().await)
}

#[tauri::command]
async fn replicated_sync_request_enrollment(state: State<'_, AppState>) -> Result<String, String> {
    let fingerprint = state.replicated_sync.request_enrollment().await?;
    kick_replicated_sync(&state);
    Ok(fingerprint)
}

#[tauri::command]
async fn replicated_sync_approve_request(request_id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.replicated_sync.approve_enrollment_request(&request_id).await
}

#[tauri::command]
fn replicated_sync_reject_request(request_id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.database.reject_enrollment_request(&request_id)
}

#[tauri::command]
async fn replicated_sync_confirm_enrollment(request_id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.replicated_sync.confirm_enrollment(&request_id).await?;
    kick_replicated_sync(&state);
    Ok(())
}

#[tauri::command]
async fn replicated_sync_rotate_epoch(revoke_device_id: Option<String>, state: State<'_, AppState>) -> Result<(), String> {
    state.replicated_sync.rotate_epoch(revoke_device_id.as_deref()).await
}

#[tauri::command]
fn replicated_sync_set_device_label(device_id: String, label: String, state: State<'_, AppState>) -> Result<(), String> {
    state.database.set_device_label(&device_id, &label)?;
    kick_replicated_sync(&state);
    Ok(())
}

#[tauri::command]
async fn replicated_sync_leave(state: State<'_, AppState>) -> Result<(), String> {
    state.replicated_sync.leave_sync_space().await
}

#[tauri::command]
fn replicated_sync_check_recovery_phrase(phrase: String) -> threestrands_sync_envelope::RecoveryPhraseCheck {
    threestrands_sync_envelope::check_recovery_phrase(&phrase)
}

#[tauri::command]
async fn replicated_sync_join_with_recovery_phrase(phrase: String, state: State<'_, AppState>) -> Result<(), String> {
    state.replicated_sync.join_with_recovery_phrase(&phrase).await
}

#[tauri::command]
async fn replicated_sync_remove_transport(
    instance_id: String,
    delete_data: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if delete_data {
        if let Some(row) = state
            .database
            .configured_transports()?
            .into_iter()
            .find(|row| row.instance_id == instance_id)
        {
            if row.config().is_some_and(|config| config.supports_delete_data()) {
                if let Some(connector) = row.open_connector().await {
                    connector.delete_all_corpus_data().await?;
                }
            }
        }
    }
    state.database.remove_transport(&instance_id)
}

#[tauri::command]
async fn replicated_sync_now(state: State<'_, AppState>) -> Result<(), String> {
    state.replicated_sync.sync_once().await
}

#[tauri::command]
fn replicated_sync_conflicts(state: State<'_, AppState>) -> Result<Vec<replicated_sync::FrontierConflict>, String> {
    state.database.list_frontier_conflicts()
}

#[tauri::command]
fn replicated_sync_resolve_conflict(
    entity_type: String,
    entity_id: String,
    field: String,
    operation_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let entity_type: threestrands_sync_protocol::EntityType =
        entity_type.parse().map_err(|error: String| error)?;
    state.database.resolve_frontier_conflict(entity_type, &entity_id, &field, &operation_id)?;
    kick_replicated_sync(&state);
    Ok(())
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
    log_failure(
        "refreshing account identity",
        state.correspondence.refresh_identity().await,
    );
    service.sync().await
}

#[tauri::command]
async fn disconnect_google(state: State<'_, AppState>, app: tauri::AppHandle) -> Result<(), String> {
    let primary = state.database.primary_account_id();
    remove_account(primary, state, app).await
}

#[tauri::command]
fn list_accounts(state: State<'_, AppState>) -> Result<Vec<Account>, String> {
    database_result(state.database.list_accounts())
}

#[tauri::command]
async fn add_account(state: State<'_, AppState>, app: tauri::AppHandle) -> Result<Account, String> {
    let config = state.auth_config.as_ref().ok_or_else(not_configured)?;
    let auth = AccountAuth::Google(config.pending_account());
    let email = authorize_interactively(&state, &auth).await?;
    let account = state.database.adopt_account(&email)?;
    let connected = spawn_synced_account(state.database.clone(), auth, true, app);
    state.accounts.lock().await.insert(email, connected);
    record_mail_account(&state, &account)?;
    Ok(account)
}

#[tauri::command]
async fn remove_account(
    email: String,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    remove_account_internal(email, state, app, false).await
}

async fn remove_account_internal(
    email: String,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    remove_catalog: bool,
) -> Result<(), String> {
    let _guard = state.correspondence.gate.lock().await;
    let config = state.auth_config.as_ref().ok_or_else(not_configured)?;
    state.database.pause_ready_sends_for(&email)?;
    // Purge local data first: if this fails, the account is untouched and
    // its credentials are still live, so the caller can safely retry rather
    // than being left with a still-listed account whose credentials are
    // already gone.
    if !remove_catalog && state.database.cross_device_sync_enrolled()? {
        state.database.disconnect_account_locally(&email)?;
    } else {
        state.database.remove_account(&email)?;
    }
    let result = match state.accounts.lock().await.remove(&email) {
        Some(connected) => {
            connected.poll_task.abort();
            connected.auth.disconnect()
        }
        None => AccountAuth::Google(config.account(&email)).disconnect(),
    };
    // Removing the last account returns the app to its pre-connect state, so
    // restore the placeholder entry `connect_google` authorizes against —
    // otherwise disconnecting would leave no way back in.
    if state.database.list_accounts()?.is_empty() {
        ensure_placeholder_account(&state, app).await;
    }
    result
}

/// Why "remove everywhere" is refused on a device that does not replicate.
const NOT_SYNC_ENROLLED: &str = "Turn on cross-device sync and finish enrolling this device before removing an account everywhere";

#[tauri::command]
async fn remove_synced_mail_account(
    email: String,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    if !state.database.cross_device_sync_enrolled()? {
        return Err(NOT_SYNC_ENROLLED.into());
    }
    state.database.record_local_entity_deletion(threestrands_sync_protocol::EntityType::MailAccount, &email.to_ascii_lowercase())?;
    for task in state.database.list_tasks(Some(&email), None)? {
        state.database.record_local_entity_deletion(threestrands_sync_protocol::EntityType::Task, &task.id)?;
    }
    for split in state.database.list_split_inboxes()?.into_iter().filter(|split| split.account_id == email) {
        state.database.record_local_entity_deletion(threestrands_sync_protocol::EntityType::SplitInbox, &split.id)?;
    }
    state.replicated_sync.sync_once().await?;
    remove_account_internal(email, state, app, true).await
}

/// Seeds the pre-connect placeholder entry when no account is connected, so
/// the registry is never empty while OAuth is configured.
async fn ensure_placeholder_account(state: &AppState, app: tauri::AppHandle) {
    let Some(config) = state.auth_config.as_ref() else {
        return;
    };
    let mut accounts = state.accounts.lock().await;
    if accounts.is_empty() {
        accounts.insert(
            auth::LEGACY_KEY.to_string(),
            spawn_synced_account(
                state.database.clone(),
                AccountAuth::Google(config.legacy_account()),
                false,
                app,
            ),
        );
    }
}

#[tauri::command]
async fn reconnect_account(
    email: String,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<Account, String> {
    let config = state.auth_config.as_ref().ok_or_else(not_configured)?;
    let auth = match state.accounts.lock().await.get(&email) {
        Some(connected) => connected.auth.clone(),
        None => AccountAuth::Google(config.account(&email)),
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
                let connected = spawn_synced_account(state.database.clone(), auth, false, app);
                let service = connected.sync.clone();
                accounts.insert(email.clone(), connected);
                service
            }
        }
    };
    // A completed reconnect means credentials were accepted *and* the first
    // mailbox synchronization finished (or returned a useful error).
    service.sync().await?;
    record_mail_account(&state, &account)?;
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
    let email = authorize_interactively(&state, &AccountAuth::Google(auth)).await?;
    state.database.adopt_calendar_account(&email)?;
    let account = state
        .database
        .list_calendar_accounts()?
        .into_iter()
        .find(|account| account.email == email)
        .ok_or_else(|| "Calendar account was not saved".to_string())?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::CalendarAccount, &email.to_ascii_lowercase(), &serde_json::json!({"email":email}), None)?;
    Ok(account)
}

#[tauri::command]
async fn reconnect_calendar_account(
    email: String,
    state: State<'_, AppState>,
) -> Result<CalendarAccount, String> {
    let config = state.auth_config.as_ref().ok_or_else(not_configured)?;
    let auth = config.calendar_account(&email);
    authorize_interactively(&state, &AccountAuth::Google(auth)).await?;
    state.database.adopt_calendar_account(&email)?;
    let account = state
        .database
        .list_calendar_accounts()?
        .into_iter()
        .find(|account| account.email == email)
        .ok_or_else(|| "Calendar account was not saved".to_string())?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::CalendarAccount, &email.to_ascii_lowercase(), &serde_json::json!({"email":email}), None)?;
    Ok(account)
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

async fn selected_calendar_ids(
    config: &GoogleAuthConfig,
    database: &Database,
    email: &str,
) -> Result<Vec<String>, String> {
    match database.calendar_selection(email)? {
        Some(selected) => Ok(selected),
        None => Ok(calendar_options_for_account(config, database, email)
            .await?
            .into_iter()
            .filter(|option| option.selected)
            .map(|option| option.id)
            .collect()),
    }
}

async fn freebusy_coverage(
    time_min: &str,
    time_max: &str,
    time_zone: &str,
    state: &AppState,
) -> Result<(Vec<BusyInterval>, usize, usize, Vec<String>), String> {
    let Some(config) = state.auth_config.as_ref() else {
        return Ok((Vec::new(), 0, 0, Vec::new()));
    };
    let mut busy = Vec::new();
    let mut checked = 0;
    let mut total = 0;
    let mut errors = Vec::new();
    for account in state.database.list_calendar_accounts()? {
        let calendar_ids = match selected_calendar_ids(config, &state.database, &account.email).await {
            Ok(ids) => ids,
            Err(error) => {
                errors.push(format!("{}: {error}", account.email));
                continue;
            }
        };
        total += calendar_ids.len();
        if calendar_ids.is_empty() {
            continue;
        }
        match calendar::fetch_freebusy(
            config.calendar_account(&account.email),
            &calendar_ids,
            time_min,
            time_max,
            time_zone,
        )
        .await
        {
            Ok(result) => {
                checked += result.checked_calendar_count;
                busy.extend(result.busy);
                errors.extend(result.errors.into_iter().map(|error| format!("{}: {error}", account.email)));
            }
            Err(error) => errors.push(format!("{}: {error}", account.email)),
        }
    }
    // Preserve an explicit partial state when an account/calendar discovery
    // call failed before it could tell us how many calendars were selected.
    if !errors.is_empty() && checked >= total {
        total = checked + 1;
    }
    Ok((busy, checked, total, errors))
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
    record_synced_value(
        &state,
        threestrands_sync_protocol::EntityType::CalendarSelection,
        &account_id.to_ascii_lowercase(),
        &serde_json::json!({ "accountId": account_id, "calendarIds": calendar_ids }),
        None,
    )?;
    for option in &mut available {
        option.selected = unique.contains(&option.id);
    }
    Ok(available)
}

#[tauri::command]
fn remove_calendar_account(email: String, state: State<'_, AppState>) -> Result<(), String> {
    let config = state.auth_config.as_ref().ok_or_else(not_configured)?;
    config.calendar_account(&email).disconnect()?;
    if state.database.cross_device_sync_enrolled()? {
        database_result(state.database.disconnect_calendar_account_locally(&email))
    } else {
        database_result(state.database.remove_calendar_account(&email))
    }
}

#[tauri::command]
async fn remove_synced_calendar_account(email: String, state: State<'_, AppState>) -> Result<(), String> {
    if !state.database.cross_device_sync_enrolled()? {
        return Err(NOT_SYNC_ENROLLED.into());
    }
    state.database.record_local_entity_deletion(threestrands_sync_protocol::EntityType::CalendarAccount, &email.to_ascii_lowercase())?;
    state.replicated_sync.sync_once().await?;
    let config = state.auth_config.as_ref().ok_or_else(not_configured)?;
    config.calendar_account(&email).disconnect()?;
    database_result(state.database.remove_calendar_account(&email))
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
async fn find_availability(
    request: FindAvailabilityRequest,
    state: State<'_, AppState>,
) -> Result<models::AvailabilityResult, String> {
    let (busy, checked, total, errors) = freebusy_coverage(
        &request.range_start,
        &request.range_end,
        &request.preferences.time_zone,
        &state,
    )
    .await?;
    let candidates = availability::find_candidates(
        &request.range_start,
        &request.range_end,
        &request.preferences,
        &busy,
        checked,
        total,
    )?;
    Ok(models::AvailabilityResult {
        candidates,
        checked_calendar_count: checked,
        total_calendar_count: total,
        errors,
    })
}

#[tauri::command]
async fn check_proposed_time(
    request: CheckProposedTimeRequest,
    state: State<'_, AppState>,
) -> Result<ProposedTimeCheck, String> {
    if request.time_zone.parse::<chrono_tz::Tz>().is_err() {
        return Err(format!("Unknown IANA timezone: {}", request.time_zone));
    }
    let (busy, checked, total, errors) = freebusy_coverage(
        &request.start,
        &request.end,
        &request.time_zone,
        &state,
    )
    .await?;
    let status = availability::check_time(&request.start, &request.end, &busy, checked, total)?;
    let start = chrono::DateTime::parse_from_rfc3339(&request.start)
        .map_err(|_| "Proposed time must be RFC3339 timestamps".to_string())?
        .with_timezone(&chrono::Utc);
    let end = chrono::DateTime::parse_from_rfc3339(&request.end)
        .map_err(|_| "Proposed time must be RFC3339 timestamps".to_string())?
        .with_timezone(&chrono::Utc);
    let conflicts = busy
        .into_iter()
        .filter(|interval| {
            let Ok(interval_start) = chrono::DateTime::parse_from_rfc3339(&interval.start) else { return false; };
            let Ok(interval_end) = chrono::DateTime::parse_from_rfc3339(&interval.end) else { return false; };
            interval_start.with_timezone(&chrono::Utc) < end && interval_end.with_timezone(&chrono::Utc) > start
        })
        .collect();
    Ok(ProposedTimeCheck {
        status,
        conflicts,
        checked_calendar_count: checked,
        total_calendar_count: total,
        errors,
    })
}

#[tauri::command]
fn set_account_color(
    email: String,
    color: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.database.set_account_color(&email, &color)?;
    let account = state.database.get_account(&email)?.ok_or_else(|| "Account not found".to_string())?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::MailAccount, &email.to_ascii_lowercase(),
        &serde_json::json!({"email":account.email,"displayName":account.display_name,"color":account.color,"provider":account.provider,"sortOrder":account.sort_order}),
        Some(std::collections::BTreeSet::from(["color".to_string()])))
}

#[tauri::command]
fn set_account_display_name(
    email: String,
    display_name: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .database
        .set_account_display_name(&email, display_name.as_deref())?;
    let account = state.database.get_account(&email)?.ok_or_else(|| "Account not found".to_string())?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::MailAccount, &email.to_ascii_lowercase(),
        &serde_json::json!({"email":account.email,"displayName":account.display_name,"color":account.color,"provider":account.provider,"sortOrder":account.sort_order}),
        Some(std::collections::BTreeSet::from(["displayName".to_string()])))
}

#[tauri::command]
fn reorder_accounts(emails: Vec<String>, state: State<'_, AppState>) -> Result<(), String> {
    state.database.reorder_accounts(&emails)?;
    for account in state.database.list_accounts()? {
        record_synced_value(&state, threestrands_sync_protocol::EntityType::MailAccount, &account.email.to_ascii_lowercase(),
            &serde_json::json!({"email":account.email,"displayName":account.display_name,"color":account.color,"provider":account.provider,"sortOrder":account.sort_order}),
            Some(std::collections::BTreeSet::from(["sortOrder".to_string()])))?;
    }
    Ok(())
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
    database_result(state.database.list_split_inboxes())
}

#[tauri::command]
fn create_split_inbox(
    request: CreateSplitInboxRequest,
    state: State<'_, AppState>,
) -> Result<SplitInbox, String> {
    let item = state.database.create_split_inbox(
        &request.name,
        &request.match_kind,
        &request.match_value,
        &request.account_id,
    )?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::SplitInbox, &item.id, &item, None)?;
    Ok(item)
}

#[tauri::command]
fn update_split_inbox(
    request: UpdateSplitInboxRequest,
    state: State<'_, AppState>,
) -> Result<SplitInbox, String> {
    let item = state.database.update_split_inbox(&request.id, &request.name)?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::SplitInbox, &item.id, &item,
        Some(std::collections::BTreeSet::from(["name".to_string()])))?;
    Ok(item)
}

#[tauri::command]
fn delete_split_inbox(id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.database.delete_split_inbox(&id)?;
    state.database.record_local_entity_deletion(threestrands_sync_protocol::EntityType::SplitInbox, &id)?;
    kick_replicated_sync(&state);
    Ok(())
}

#[tauri::command]
fn reorder_split_inboxes(ids: Vec<String>, state: State<'_, AppState>) -> Result<(), String> {
    state.database.reorder_split_inboxes(&ids)?;
    for item in state.database.list_split_inboxes()? {
        record_synced_value(&state, threestrands_sync_protocol::EntityType::SplitInbox, &item.id, &item,
            Some(std::collections::BTreeSet::from(["sortOrder".to_string()])))?;
    }
    Ok(())
}

#[tauri::command]
fn list_split_inbox_page(
    split_inbox_id: String,
    offset: usize,
    limit: usize,
    state: State<'_, AppState>,
) -> Result<ThreadPage, String> {
    database_result(state.database.list_split_inbox_page(&split_inbox_id, offset, limit))
}

#[tauri::command]
fn list_snippets(state: State<'_, AppState>) -> Result<Vec<Snippet>, String> {
    database_result(state.database.list_snippets())
}

#[tauri::command]
fn create_snippet(
    request: CreateSnippetRequest,
    state: State<'_, AppState>,
) -> Result<Snippet, String> {
    let item = state.database.create_snippet(&request.name, &request.body)?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::Snippet, &item.id, &item, None)?;
    Ok(item)
}

#[tauri::command]
fn update_snippet(
    request: UpdateSnippetRequest,
    state: State<'_, AppState>,
) -> Result<Snippet, String> {
    let item = state
        .database
        .update_snippet(&request.id, &request.name, &request.body)?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::Snippet, &item.id, &item,
        Some(std::collections::BTreeSet::from(["name".to_string(), "body".to_string()])))?;
    Ok(item)
}

#[tauri::command]
fn delete_snippet(id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.database.delete_snippet(&id)?;
    state.database.record_local_entity_deletion(threestrands_sync_protocol::EntityType::Snippet, &id)?;
    kick_replicated_sync(&state);
    Ok(())
}

#[tauri::command]
async fn list_labels(
    account_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<Label>, String> {
    let auth = resolve_account(&state, account_id.as_deref(), |account| account.auth.clone()).await?;

    auth.provider()
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
    state.database.set_retention_days(days)?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::Retention, "mail",
        &serde_json::json!({"days":days}), None)
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
async fn ai_test_connection(
    provider: ai::AiProvider,
    model: String,
    endpoint: Option<String>,
) -> Result<(), String> {
    let api_key = ai::get_key()?.ok_or_else(|| "No AI API key configured".to_string())?;
    ai::test_connection(provider, &model, endpoint.as_deref(), &api_key).await
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
async fn ai_analyze_thread(
    thread_id: String,
    user_time_zone: String,
    provider: ai::AiProvider,
    model: String,
    endpoint: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<ActionProposal>, String> {
    if user_time_zone.parse::<chrono_tz::Tz>().is_err() {
        return Err(format!("Unknown IANA timezone: {user_time_zone}"));
    }
    if user_time_zone.chars().count() > 100 {
        return Err("Timezone value is too long".to_string());
    }
    let database = state.database.clone();
    let database_thread_id = thread_id.clone();
    let detail = run_database_task(move || database.get_thread(&database_thread_id)).await?;
    let cache_key = format!("{thread_id}\0{}", detail.thread.last_message_at);
    {
        let mut cache = state
            .proposal_cache
            .lock()
            .map_err(|_| "AI proposal cache is unavailable".to_string())?;
        cache.retain(|key, _| !key.starts_with(&format!("{thread_id}\0")));
        if let Some(cached) = cache.get(&cache_key) {
            return Ok(cached.clone());
        }
    }
    let api_key = ai::get_key()?.ok_or_else(|| "No AI API key configured".to_string())?;
    let request = ai::AnalyzeRequest {
        provider,
        model: model.clone(),
        endpoint,
        subject: detail.thread.subject,
        messages: detail
            .messages
            .into_iter()
            .map(|message| ai::ActionMessageInput {
                id: message.id,
                sender: message.sender,
                sent_at: message.sent_at,
                body_text: message.body_text,
            })
            .collect(),
        current_time: Utc::now().to_rfc3339(),
        user_time_zone,
    };
    log::info!(
        target: "ai_analyze_thread",
        "starting analysis for thread {thread_id} with provider {provider:?} model {model}"
    );
    let proposals = match ai::analyze(request, &api_key).await {
        Ok(proposals) => proposals,
        Err(error) => {
            log::error!(target: "ai_analyze_thread", "analysis failed for thread {thread_id}: {error}");
            return Err(error);
        }
    };
    log::info!(
        target: "ai_analyze_thread",
        "analysis succeeded for thread {thread_id} with {} proposal(s)",
        proposals.len()
    );
    let mut cache = state
        .proposal_cache
        .lock()
        .map_err(|_| "AI proposal cache is unavailable".to_string())?;
    cache.insert(cache_key, proposals.clone());
    Ok(proposals)
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

#[tauri::command]
fn list_tasks(
    account_id: Option<String>,
    status: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<ThreadTask>, String> {
    database_result(state.database.list_tasks(account_id.as_deref(), status.as_deref()))
}

#[tauri::command]
fn create_task(request: CreateTaskRequest, state: State<'_, AppState>) -> Result<ThreadTask, String> {
    let task = state.database.create_task(&request)?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::Task, &task.id, &task, None)?;
    Ok(task)
}

#[tauri::command]
fn update_task(request: UpdateTaskRequest, state: State<'_, AppState>) -> Result<ThreadTask, String> {
    let mut fields = std::collections::BTreeSet::new();
    if request.title.is_some() { fields.insert("title".to_string()); }
    if request.notes.is_some() { fields.insert("notes".to_string()); }
    if request.kind.is_some() { fields.insert("kind".to_string()); }
    if request.due_kind.is_some() { fields.insert("dueKind".to_string()); }
    if request.due_value.is_some() { fields.insert("dueValue".to_string()); }
    if request.time_zone.is_some() { fields.insert("timeZone".to_string()); }
    if request.repeat_interval_days.is_some() { fields.insert("repeatIntervalDays".to_string()); }
    fields.insert("updatedAt".to_string());
    let task = state.database.update_task(&request)?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::Task, &task.id, &task, Some(fields))?;
    Ok(task)
}

#[tauri::command]
fn set_task_status(
    id: String,
    status: String,
    source: String,
    state: State<'_, AppState>,
) -> Result<ThreadTask, String> {
    let task = state.database.set_task_status(&id, &status, &source)?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::Task, &task.id, &task,
        Some(std::collections::BTreeSet::from(["status".to_string(), "completionSource".to_string(), "completedAt".to_string(), "updatedAt".to_string()])))?;
    Ok(task)
}

#[tauri::command]
fn record_follow_up(id: String, state: State<'_, AppState>) -> Result<ThreadTask, String> {
    let task = state.database.record_follow_up(&id)?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::Task, &task.id, &task,
        Some(std::collections::BTreeSet::from(["dueValue".to_string(), "waitAfter".to_string(), "completionSource".to_string(), "completedAt".to_string(), "updatedAt".to_string()])))?;
    Ok(task)
}

#[tauri::command]
fn reconcile_tasks(state: State<'_, AppState>) -> Result<usize, String> {
    let count = state.database.reconcile_waiting_tasks()?;
    if count > 0 {
        for task in state.database.list_tasks(None, None)? {
            record_synced_value(&state, threestrands_sync_protocol::EntityType::Task, &task.id, &task,
                Some(std::collections::BTreeSet::from(["status".to_string(), "completionSource".to_string(), "completedAt".to_string(), "updatedAt".to_string()])))?;
        }
    }
    Ok(count)
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
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .targets([
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::LogDir {
                        file_name: None,
                    }),
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                ])
                .build(),
        )
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
        .setup(setup_app)
        .invoke_handler(invoke_handler())
        .build(tauri::generate_context!())
        .expect("error while building ThreeStrands")
        .run(handle_run_event);
}

/// Opens (or recovers) the local database, starts every background loop, and
/// registers `AppState` before the first command can run.
fn setup_app(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
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
        force_full_resync(&database);
    }
    let auth_config = GoogleAuthConfig::from_environment().ok();
    spawn_badge_loop(database.clone(), app.handle().clone());
    spawn_storage_maintenance(database.clone());
    let root = data_dir.join("attachments");
    std::fs::create_dir_all(&root)?;
    restrict_dir_to_owner(&root);
    let attachment_reader = attachment_reader::ReaderCache::new(root.join("reader"))?;
    // Built before `manage` so the registry is never observed empty by a
    // command racing startup.
    let registry = startup_account_registry(&database, auth_config.as_ref(), app.handle());
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
            log_failure("refreshing account identity", worker.refresh_identity().await);
        }
        worker.run().await;
    });
    let replicated_sync = replicated_sync::ReplicatedSync::new(database.clone());
    replicated_sync.clone().spawn(app.handle().clone(), reconcile_account_registry);
    app.manage(AppState {
        database,
        replicated_sync,
        auth_config,
        accounts,
        correspondence,
        authorize_slot: AuthorizeSlot::default(),
        exiting: std::sync::atomic::AtomicBool::new(false),
        image_cache: image_proxy::ImageCache::new().map_err(std::io::Error::other)?,
        attachment_reader,
        recovery,
        proposal_cache: Arc::new(std::sync::Mutex::new(HashMap::new())),
    });
    Ok(())
}

/// A restored-from-backup database's cursors reflect whatever history state
/// that snapshot was taken at, which may since have diverged from Gmail; a
/// fresh database has no cursor at all. Either way, force each known
/// account's next sync to be a full reconciliation rather than trusting a
/// possibly-stale incremental cursor.
fn force_full_resync(database: &Database) {
    let Some(accounts) = log_failure("listing accounts after recovery", database.list_accounts())
    else {
        return;
    };
    for account in &accounts {
        log_failure(
            "clearing sync cursor after recovery",
            database.clear_cursor(&account.email),
        );
    }
}

/// Keeps the dock/taskbar badge in step with the Inbox unread count.
fn spawn_badge_loop(database: Arc<Database>, handle: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            let badge_database = database.clone();
            if let Ok(count) =
                run_database_task(move || badge_database.count_unread_inbox()).await
            {
                if let Some(window) = handle.get_webview_window("main") {
                    let _ = window.set_badge_count((count > 0).then_some(count));
                }
            }
            tokio::time::sleep(sync::MIN_POLL_INTERVAL).await;
        }
    });
}

/// One-time, potentially slow (full file rewrite) conversion to incremental
/// auto-vacuum, then a recurring prune of mail past the user's retention
/// window with cheap incremental reclamation after. All off the blocking
/// pool so a large existing mailbox doesn't stall startup or the UI thread.
fn spawn_storage_maintenance(database: Arc<Database>) {
    tauri::async_runtime::spawn(async move {
        let upgrade_db = database.clone();
        let needs_upgrade =
            tokio::task::spawn_blocking(move || upgrade_db.needs_vacuum_upgrade())
                .await
                .ok()
                .and_then(|result| log_failure("checking auto-vacuum mode", result))
                .unwrap_or(false);
        if needs_upgrade {
            let upgrade_db = database.clone();
            if let Ok(result) =
                tokio::task::spawn_blocking(move || upgrade_db.vacuum_to_incremental()).await
            {
                log_failure("converting to incremental auto-vacuum", result);
            }
        }
        // One-time backfill: compress any message bodies left over from
        // before body compression shipped, a batch at a time with a short
        // pause between batches so this doesn't starve the database mutex
        // normal sync/read operations also need. Becomes a no-op once
        // everything has been converted.
        loop {
            let backfill_db = database.clone();
            let converted =
                tokio::task::spawn_blocking(move || backfill_db.compress_next_body_batch(500))
                    .await
                    .ok()
                    .and_then(|result| log_failure("compressing message bodies", result))
                    .unwrap_or(0);
            if converted == 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        loop {
            let prune_db = database.clone();
            let _ = tokio::task::spawn_blocking(move || run_storage_maintenance(&prune_db)).await;
            tokio::time::sleep(std::time::Duration::from_secs(6 * 60 * 60)).await;
        }
    });
}

/// One pass of the periodic maintenance loop. Every step runs even if an
/// earlier one fails; each failure is logged and its step name returned.
fn run_storage_maintenance(database: &Database) -> Vec<&'static str> {
    let mut failed = Vec::new();
    let mut step = |name: &'static str, result: db::DbResult<()>| {
        if log_failure(name, result).is_none() {
            failed.push(name);
        }
    };
    step("pruning expired threads", database.prune_expired_threads().map(|_| ()));
    step("reclaiming free pages", database.reclaim_space());
    // Bound WAL growth, then snapshot: checkpointing first means the backup
    // reflects the latest writes without carrying an ever-growing WAL of its
    // own.
    step("checkpointing the WAL", database.checkpoint_wal());
    step("creating the periodic backup", database.create_periodic_backup());
    failed
}

/// Brings up every account in the catalog, each with its own credentials and
/// polling loop. Before the very first connect the catalog is empty and the
/// placeholder key stands in for the account about to be added, so
/// `connect_google` has something to authorize; it rekeys onto the real
/// address once the identity is known.
fn startup_account_registry(
    database: &Arc<Database>,
    auth_config: Option<&GoogleAuthConfig>,
    handle: &tauri::AppHandle,
) -> HashMap<String, ConnectedAccount> {
    let mut registry = HashMap::new();
    let Some(config) = auth_config else {
        return registry;
    };
    let catalog = log_failure("listing accounts at startup", database.list_accounts())
        .unwrap_or_default();
    let keys = if catalog.is_empty() {
        vec![auth::LEGACY_KEY.to_string()]
    } else {
        catalog.into_iter().map(|account| account.email).collect()
    };
    for key in keys {
        let auth = AccountAuth::Google(if key == auth::LEGACY_KEY {
            config.legacy_account()
        } else {
            config.account(&key)
        });
        registry.insert(
            key,
            spawn_synced_account(database.clone(), auth, true, handle.clone()),
        );
    }
    registry
}

/// Every IPC command the frontend may invoke, grouped by feature area.
fn invoke_handler() -> impl Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        // Replicated (peer) sync
        synced_preferences,
        update_synced_preferences,
        replicated_sync_enabled,
        replicated_sync_status,
        replicated_sync_beta_enabled,
        replicated_sync_set_beta_enabled,
        replicated_sync_enrollment_status,
        replicated_sync_pending_requests,
        replicated_sync_device_roster,
        replicated_sync_begin_genesis,
        replicated_sync_inspect_space,
        replicated_sync_request_enrollment,
        replicated_sync_approve_request,
        replicated_sync_reject_request,
        replicated_sync_confirm_enrollment,
        replicated_sync_rotate_epoch,
        replicated_sync_join_with_recovery_phrase,
        replicated_sync_set_device_label,
        replicated_sync_leave,
        replicated_sync_check_recovery_phrase,
        replicated_sync_add_folder,
        replicated_sync_add_ipfs_rpc,
        replicated_sync_probe_ipfs_rpc,
        replicated_sync_probe_s3,
        replicated_sync_create_join_code,
        replicated_sync_list_join_codes,
        replicated_sync_cancel_join_code,
        replicated_sync_preview_join_code,
        replicated_sync_pick_join_folder,
        replicated_sync_join_with_code,
        replicated_sync_join_code_notices,
        replicated_sync_dismiss_join_code_notice,
        replicated_sync_add_s3,
        replicated_sync_update_connector,
        replicated_sync_remove_transport,
        replicated_sync_now,
        replicated_sync_conflicts,
        replicated_sync_resolve_conflict,
        // App lifecycle
        correspondence_request,
        finish_exit,
        recovery_status,
        // Mailbox reading
        list_threads,
        list_all_mail,
        list_trash,
        list_threads_page,
        list_unread_counts,
        mailbox_unread_counts,
        list_all_mail_page,
        list_trash_page,
        get_thread,
        search_threads,
        backfill_search_threads,
        // Images and attachments
        fetch_remote_image,
        fetch_attachment_image,
        preview_calendar_attachment,
        open_attachment,
        save_attachment,
        // Triage, contacts, and unsubscribe
        mutate_thread,
        mutate_threads,
        record_triage_event,
        list_triage_sender_stats,
        list_contact_suggestions,
        pin_contact,
        unpin_contact,
        unsubscribe,
        // Mail sync
        sync_status,
        sync_account,
        flush_pending_mutations,
        // Mail accounts
        google_auth_status,
        connect_google,
        disconnect_google,
        list_accounts,
        add_account,
        remove_account,
        remove_synced_mail_account,
        reconnect_account,
        set_account_display_name,
        set_account_color,
        reorder_accounts,
        // Calendar and scheduling
        list_calendar_accounts,
        add_calendar_account,
        reconnect_calendar_account,
        list_calendar_options,
        set_calendar_selection,
        remove_calendar_account,
        remove_synced_calendar_account,
        list_schedule_events,
        find_availability,
        check_proposed_time,
        // Settings transfer and retention
        export_settings,
        import_settings,
        get_retention_days,
        set_retention_days,
        // Split inboxes, snippets, and labels
        list_split_inboxes,
        create_split_inbox,
        update_split_inbox,
        delete_split_inbox,
        reorder_split_inboxes,
        list_split_inbox_page,
        list_snippets,
        create_snippet,
        update_snippet,
        delete_snippet,
        list_labels,
        create_label,
        update_label,
        delete_label,
        // AI assistance
        ai_api_key_configured,
        set_ai_api_key,
        ai_test_connection,
        ai_summarize_thread,
        ai_analyze_thread,
        ai_reply_assist_context,
        ai_generate_reply,
        // Tasks and follow-ups
        list_tasks,
        create_task,
        update_task,
        set_task_status,
        record_follow_up,
        reconcile_tasks,
        // System
        system_fonts::list_system_font_families,
    ]
}

/// Holds window close and app exit until the frontend has had a chance to
/// save open compose drafts, and syncs or flushes on focus changes.
fn handle_run_event(handle: &tauri::AppHandle, event: tauri::RunEvent) {
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
}
