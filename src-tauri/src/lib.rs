mod ai;
mod availability;
mod attachment_reader;
mod attachment_text;
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
mod quoted_history;
mod replicated_sync;
mod s3_transport;
mod schema;
mod sync;
mod sync_connectors;
mod sync_folder;
mod sync_policy;
#[cfg(test)]
mod sync_sim;
mod sync_projection;
mod sync_state;
mod system_fonts;
mod transfer;
#[path = "unsubscribe.rs"]
mod unsubscribe_service;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use auth::{AccountAuth, AuthConfig, OAuthProvider};
use chrono::Utc;
use db::Database;
use models::{
    ActionAnalysis, ActionProposal, Account, AuthStatus, BusyInterval, CalendarAccount, CalendarOption, CheckProposedTimeRequest, ContactActivity, ContactFiles, ContactSuggestion, ContactProfile, ContactRecord, ContactTimelineItem, DomainContext, SaveContactRequest, CreateCalendarEventRequest, CreateLabelRequest,
    CreateSnippetRequest, CreateSplitInboxRequest, Label, MailProviderKind, MailboxUnreadCounts, ReplyAssistContext, ReplyAssistResult,
    FindAvailabilityRequest, ProposedTimeCheck, ScheduleEvent, ScheduleResult, SearchThreadsRequest, Snippet, SplitInbox, SummaryResult, SyncStatus, ThreadBriefResult, AiUsageDay, ChatAttachmentRef, ChatAttachmentSource, ChatSource, ThreadChatReply, ThreadChatRequest, Thread,
    ThreadDetail, ThreadMutation, ThreadPage, ThreadTask, TriageEvent, TriageSenderStats,
    UpdateLabelRequest, UpdateSnippetRequest, UpdateSplitInboxRequest, CreateTaskRequest, UpdateTaskRequest, Goal, CreateGoalRequest, UpdateGoalRequest,
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
/// `accept_identity` has learned it. `AccountAuth` and `SyncService` both read
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
    /// Each identity service's shared OAuth app, used to authorize any
    /// account connected through it.
    auth_config: AuthConfig,
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
    /// Attachment text recently shared with Thread Chat.
    attachment_text: attachment_text::ExtractCache,
    /// `Some` only when the local database had to be recovered at startup
    /// (restored from a backup, or recreated fresh) — see `open_with_recovery`.
    recovery: Option<db::RecoveryOutcome>,
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

    /// Tauri runs plain `#[tauri::command] fn` handlers on the main thread,
    /// which also services WebView input and painting. A command that waits
    /// on the shared database lock there freezes the window, so only
    /// commands that must stay on the main thread (native file dialogs) or do
    /// trivial in-memory work may be synchronous.
    #[test]
    fn only_allowlisted_commands_run_on_the_main_thread() {
        const MAIN_THREAD_COMMANDS: &[&str] = &[
            "export_settings",
            "import_settings",
            "ai_api_key_configured",
            "set_ai_api_key",
            "recovery_status",
            "replicated_sync_check_recovery_phrase",
            "replicated_sync_preview_join_code",
        ];
        let source = include_str!("lib.rs");
        let lines: Vec<&str> = source.lines().collect();
        let synchronous: Vec<&str> = lines
            .windows(2)
            .filter(|pair| pair[0].trim() == "#[tauri::command]")
            .filter_map(|pair| pair[1].trim().strip_prefix("fn "))
            .filter_map(|rest| rest.split('(').next())
            .collect();

        let unexpected: Vec<&&str> = synchronous
            .iter()
            .filter(|name| !MAIN_THREAD_COMMANDS.contains(name))
            .collect();
        assert!(
            unexpected.is_empty(),
            "make these commands async or #[tauri::command(async)]: {unexpected:?}"
        );
    }
}

#[cfg(test)]
mod account_startup_tests {
    use super::{catalogued_mail_provider, startup_account_credentials};
    use crate::{auth::{self, AuthConfig}, db::Database, models::MailProviderKind};

    #[test]
    fn a_fresh_install_starts_with_the_gmail_placeholder() {
        let database = Database::open_memory();
        let credentials = startup_account_credentials(&database, &AuthConfig::google_for_test());
        assert_eq!(credentials.len(), 1);
        assert_eq!(credentials[0].0, auth::LEGACY_KEY);
        assert_eq!(credentials[0].1.mail_provider(), MailProviderKind::Gmail);
    }

    #[test]
    fn catalogued_accounts_start_through_their_recorded_provider() {
        let database = Database::open_memory();
        database.adopt_mail_account("work@example.com", MailProviderKind::Gmail).unwrap();
        database.adopt_mail_account("home@example.com", MailProviderKind::Gmail).unwrap();
        let credentials = startup_account_credentials(&database, &AuthConfig::google_for_test());
        let started = credentials
            .iter()
            .map(|(key, auth)| (key.as_str(), auth.key(), auth.mail_provider()))
            .collect::<Vec<_>>();
        assert_eq!(
            started,
            [
                ("work@example.com", "work@example.com".to_string(), MailProviderKind::Gmail),
                ("home@example.com", "home@example.com".to_string(), MailProviderKind::Gmail),
            ]
        );
    }

    #[test]
    fn nothing_starts_without_a_configured_oauth_app() {
        let database = Database::open_memory();
        assert!(startup_account_credentials(&database, &AuthConfig::default()).is_empty());
        database.adopt_mail_account("work@example.com", MailProviderKind::Gmail).unwrap();
        assert!(startup_account_credentials(&database, &AuthConfig::default()).is_empty());
    }

    #[test]
    fn an_account_with_an_unsupported_provider_is_skipped_without_blocking_others() {
        let database = Database::open_memory();
        database.adopt_mail_account("work@example.com", MailProviderKind::Gmail).unwrap();
        database.adopt_mail_account("future@example.com", MailProviderKind::Gmail).unwrap();
        database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE accounts SET provider = 'from-a-newer-build' WHERE email = 'future@example.com'",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        let credentials = startup_account_credentials(&database, &AuthConfig::google_for_test());
        let keys = credentials.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>();
        assert_eq!(keys, ["work@example.com"]);
        assert!(catalogued_mail_provider(&database, "future@example.com")
            .unwrap_err()
            .contains("unsupported mail provider"));
    }

    #[test]
    fn an_uncatalogued_address_is_treated_as_gmail() {
        let database = Database::open_memory();
        assert_eq!(
            catalogued_mail_provider(&database, "gone@example.com").unwrap(),
            MailProviderKind::Gmail
        );
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

    #[test]
    fn full_resync_after_recovery_clears_every_accounts_cursor() {
        let database = Database::open_memory();
        for (email, cursor) in [("work@example.com", "work-cursor"), ("personal@example.com", "personal-cursor")] {
            database.adopt_account(email).unwrap();
            database.finish_sync(email, cursor).unwrap();
            assert_eq!(database.cursor(email).unwrap().as_deref(), Some(cursor));
        }

        force_full_resync(&database);

        assert_eq!(database.list_accounts().unwrap().len(), 2);
        assert_eq!(database.cursor("work@example.com").unwrap(), None);
        assert_eq!(database.cursor("personal@example.com").unwrap(), None);
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
async fn authorize_interactively(
    state: &AppState,
    auth: &auth::OAuthCredential,
) -> Result<String, String> {
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
async fn list_threads(
    account_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<Thread>, String> {
    let database = state.database.clone();
    run_database_task(move || database.list_threads(account_id.as_deref())).await
}

#[tauri::command]
async fn list_all_mail(
    account_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<Thread>, String> {
    let database = state.database.clone();
    run_database_task(move || database.list_all_mail(account_id.as_deref())).await
}

#[tauri::command]
async fn list_trash(
    account_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<Thread>, String> {
    let database = state.database.clone();
    run_database_task(move || database.list_trash(account_id.as_deref())).await
}

#[tauri::command]
async fn list_threads_page(
    account_id: Option<String>,
    offset: usize,
    limit: usize,
    state: State<'_, AppState>,
) -> Result<ThreadPage, String> {
    let database = state.database.clone();
    run_database_task(move || database.list_threads_page(account_id.as_deref(), offset, limit)).await
}

#[tauri::command]
async fn list_unread_counts(state: State<'_, AppState>) -> Result<HashMap<String, i64>, String> {
    let database = state.database.clone();
    run_database_task(move || database.list_unread_counts()).await
}

#[tauri::command]
async fn mailbox_unread_counts(
    account_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<MailboxUnreadCounts, String> {
    let database = state.database.clone();
    run_database_task(move || database.mailbox_unread_counts(account_id.as_deref())).await
}

#[tauri::command]
async fn list_all_mail_page(
    account_id: Option<String>,
    offset: usize,
    limit: usize,
    state: State<'_, AppState>,
) -> Result<ThreadPage, String> {
    let database = state.database.clone();
    run_database_task(move || database.list_all_mail_page(account_id.as_deref(), offset, limit)).await
}

#[tauri::command]
async fn list_trash_page(
    account_id: Option<String>,
    offset: usize,
    limit: usize,
    state: State<'_, AppState>,
) -> Result<ThreadPage, String> {
    let database = state.database.clone();
    run_database_task(move || database.list_trash_page(account_id.as_deref(), offset, limit)).await
}

#[tauri::command]
async fn get_thread(id: String, state: State<'_, AppState>) -> Result<ThreadDetail, String> {
    let database = state.database.clone();
    run_database_task(move || database.get_thread(&id)).await
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
async fn search_threads(
    request: SearchThreadsRequest,
    account_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<Thread>, String> {
    let database = state.database.clone();
    run_database_task(move || database.search_threads(&request, account_id.as_deref())).await
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
async fn mutate_thread(mutation: ThreadMutation, state: State<'_, AppState>) -> Result<(), String> {
    let database = state.database.clone();
    run_database_task(move || database.mutate_thread(&mutation)).await
}

#[tauri::command]
async fn mutate_threads(
    mutations: Vec<ThreadMutation>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let database = state.database.clone();
    run_database_task(move || database.mutate_threads(&mutations)).await
}

#[tauri::command]
async fn record_triage_event(event: TriageEvent, state: State<'_, AppState>) -> Result<(), String> {
    let database = state.database.clone();
    run_database_task(move || database.record_triage_event(&event)).await
}

#[tauri::command]
async fn list_triage_sender_stats(
    account_id: String,
    limit: Option<usize>,
    state: State<'_, AppState>,
) -> Result<Vec<TriageSenderStats>, String> {
    let database = state.database.clone();
    run_database_task(move || database.list_triage_sender_stats(&account_id, limit.unwrap_or(100))).await
}

#[tauri::command]
async fn list_contact_suggestions(
    account_id: String,
    query: String,
    limit: Option<usize>,
    state: State<'_, AppState>,
) -> Result<Vec<ContactSuggestion>, String> {
    let database = state.database.clone();
    run_database_task(move || database.list_contact_suggestions(&account_id, &query, limit.unwrap_or(8))).await
}

#[tauri::command]
async fn list_contact_profiles(
    query: String,
    limit: Option<usize>,
    account_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<ContactProfile>, String> {
    let database = state.database.clone();
    run_database_task(move || database.list_contact_profiles_for_account(&query, limit.unwrap_or(500), account_id.as_deref())).await
}

#[tauri::command]
async fn resolve_contact_ids(
    emails: Vec<String>,
    state: State<'_, AppState>,
) -> Result<std::collections::HashMap<String, String>, String> {
    let database = state.database.clone();
    run_database_task(move || database.contact_ids_for_addresses(&emails)).await
}

#[tauri::command]
async fn get_contact_profile(
    id: String,
    state: State<'_, AppState>,
) -> Result<Option<ContactProfile>, String> {
    let database = state.database.clone();
    run_database_task(move || database.get_contact_profile(&id)).await
}

#[tauri::command(async)]
fn save_contact_profile(
    request: SaveContactRequest,
    state: State<'_, AppState>,
) -> Result<ContactProfile, String> {
    let profile = state.database.save_contact_profile(&request)?;
    record_synced_value(
        &state,
        threestrands_sync_protocol::EntityType::Contact,
        &profile.id,
        &ContactRecord::from(&profile),
        None,
    )?;
    Ok(profile)
}

#[tauri::command(async)]
fn delete_contact_profile(id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.database.delete_contact_profile(&id)?;
    state
        .database
        .record_local_entity_deletion(threestrands_sync_protocol::EntityType::Contact, &id)?;
    kick_replicated_sync(&state);
    Ok(())
}

#[tauri::command]
async fn contact_timeline(
    id: String,
    offset: usize,
    limit: usize,
    account_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<ContactTimelineItem>, String> {
    let database = state.database.clone();
    run_database_task(move || database.contact_timeline_for_account(&id, offset, limit, account_id.as_deref())).await
}

#[tauri::command]
async fn contact_activity(id: String, state: State<'_, AppState>) -> Result<ContactActivity, String> {
    let database = state.database.clone();
    run_database_task(move || database.contact_activity(&id)).await
}

#[tauri::command]
async fn contact_files(id: String, limit: usize, state: State<'_, AppState>) -> Result<ContactFiles, String> {
    let database = state.database.clone();
    run_database_task(move || database.contact_files(&id, limit)).await
}

#[tauri::command]
async fn domain_context(
    domain: String,
    exclude: Vec<String>,
    limit: usize,
    state: State<'_, AppState>,
) -> Result<DomainContext, String> {
    let database = state.database.clone();
    run_database_task(move || database.domain_context(&domain, &exclude, limit)).await
}

#[tauri::command]
async fn list_contact_tasks(id: String, state: State<'_, AppState>) -> Result<Vec<ThreadTask>, String> {
    let database = state.database.clone();
    run_database_task(move || database.list_contact_tasks(&id)).await
}

#[tauri::command(async)]
fn pin_contact(
    account_id: String,
    email: String,
    display_name: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    database_result(
        state
            .database
            .pin_contact(&account_id, &email, display_name.as_deref()),
    )?;
    if let Some(profile) = state
        .database
        .list_contact_profiles(&email, 100)?
        .into_iter()
        .find(|profile| {
            profile
                .addresses
                .iter()
                .any(|address| address.eq_ignore_ascii_case(&email))
        })
    {
        record_synced_value(
            &state,
            threestrands_sync_protocol::EntityType::Contact,
            &profile.id,
            &ContactRecord::from(&profile),
            None,
        )?;
    }
    Ok(())
}

#[tauri::command(async)]
fn unpin_contact(
    account_id: String,
    email: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    database_result(state.database.unpin_contact(&account_id, &email))?;
    if let Some(profile) = state
        .database
        .list_contact_profiles(&email, 100)?
        .into_iter()
        .find(|profile| {
            profile
                .addresses
                .iter()
                .any(|address| address.eq_ignore_ascii_case(&email))
        })
    {
        if !profile.id.starts_with("derived:") {
            record_synced_value(
                &state,
                threestrands_sync_protocol::EntityType::Contact,
                &profile.id,
                &ContactRecord::from(&profile),
                None,
            )?;
        }
    }
    Ok(())
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
async fn sync_status(state: State<'_, AppState>) -> Result<SyncStatus, String> {
    let database = state.database.clone();
    run_database_task(move || combined_sync_status(database.as_ref())).await
}

#[tauri::command]
async fn retry_failed_mutations(state: State<'_, AppState>) -> Result<SyncStatus, String> {
    let database = state.database.clone();
    run_database_task(move || {
        database_result(database.retry_failed_mutations())?;
        combined_sync_status(database.as_ref())
    })
    .await
}

#[tauri::command]
async fn dismiss_sync_problems(state: State<'_, AppState>) -> Result<SyncStatus, String> {
    let database = state.database.clone();
    run_database_task(move || {
        database_result(database.dismiss_sync_problems())?;
        combined_sync_status(database.as_ref())
    })
    .await
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
        let removed = {
            let mut accounts = state.accounts.lock().await;
            let emails = accounts.keys().filter(|email| *email != auth::LEGACY_KEY && !catalog.contains(*email)).cloned().collect::<Vec<_>>();
            emails.into_iter().filter_map(|email| accounts.remove(&email)).collect::<Vec<_>>()
        };
        // The deletion already purged local data, so also stop runs started
        // outside the polling loop before they write any more of it back.
        for account in removed {
            account.poll_task.abort();
            account.sync.retire().await;
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
async fn synced_preferences(state: State<'_, AppState>) -> Result<Option<serde_json::Value>, String> {
    let database = state.database.clone();
    run_database_task(move || database.synced_preferences()).await
}

#[tauri::command(async)]
fn update_synced_preferences(preferences: serde_json::Value, state: State<'_, AppState>) -> Result<(), String> {
    if state.database.update_synced_preferences(preferences)? {
        kick_replicated_sync(&state);
    }
    Ok(())
}

#[tauri::command]
async fn replicated_sync_enabled(state: State<'_, AppState>) -> Result<bool, String> {
    let database = state.database.clone();
    run_database_task(move || database.replicated_sync_active()).await
}

#[tauri::command]
async fn replicated_sync_beta_enabled(state: State<'_, AppState>) -> Result<bool, String> {
    let database = state.database.clone();
    run_database_task(move || database.beta_features_enabled()).await
}

#[tauri::command(async)]
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
async fn replicated_sync_list_join_codes(state: State<'_, AppState>) -> Result<Vec<enrollment::OutstandingJoinCode>, String> {
    let database = state.database.clone();
    run_database_task(move || database.outstanding_join_codes()).await
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
async fn replicated_sync_join_code_notices(state: State<'_, AppState>) -> Result<Vec<enrollment::JoinCodeNotice>, String> {
    let database = state.database.clone();
    run_database_task(move || database.join_code_notices()).await
}

#[tauri::command(async)]
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
async fn replicated_sync_enrollment_status(state: State<'_, AppState>) -> Result<enrollment::EnrollmentStatus, String> {
    let database = state.database.clone();
    run_database_task(move || database.enrollment_status()).await
}

#[tauri::command]
async fn replicated_sync_pending_requests(state: State<'_, AppState>) -> Result<Vec<enrollment::IncomingEnrollmentRequest>, String> {
    let database = state.database.clone();
    run_database_task(move || database.pending_incoming_enrollment_requests()).await
}

#[tauri::command]
async fn replicated_sync_device_roster(state: State<'_, AppState>) -> Result<Vec<enrollment::DeviceRosterEntry>, String> {
    let database = state.database.clone();
    run_database_task(move || database.device_roster()).await
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
async fn replicated_sync_protocol_reset_notice(state: State<'_, AppState>) -> Result<bool, String> {
    let database = state.database.clone();
    run_database_task(move || database.protocol_reset_notice()).await
}

#[tauri::command]
async fn replicated_sync_dismiss_protocol_reset_notice(state: State<'_, AppState>) -> Result<(), String> {
    let database = state.database.clone();
    run_database_task(move || database.dismiss_protocol_reset_notice()).await
}

#[tauri::command]
async fn replicated_sync_request_enrollment(state: State<'_, AppState>) -> Result<String, String> {
    let fingerprint = state.replicated_sync.request_enrollment().await?;
    kick_replicated_sync(&state);
    Ok(fingerprint)
}

#[tauri::command]
async fn replicated_sync_approve_request(request_id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.replicated_sync.approve_enrollment_request(&request_id).await?;
    kick_replicated_sync(&state);
    Ok(())
}

#[tauri::command]
async fn replicated_sync_reject_request(request_id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.replicated_sync.reject_enrollment_request(&request_id).await?;
    kick_replicated_sync(&state);
    Ok(())
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

#[tauri::command(async)]
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
async fn replicated_sync_conflicts(state: State<'_, AppState>) -> Result<Vec<replicated_sync::FrontierConflict>, String> {
    let database = state.database.clone();
    run_database_task(move || database.list_frontier_conflicts()).await
}

#[tauri::command(async)]
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
    .map_err(|_| OAuthProvider::Google.not_configured())?;
    authorize_interactively(&state, auth.credential()).await?;
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
async fn list_accounts(state: State<'_, AppState>) -> Result<Vec<Account>, String> {
    let database = state.database.clone();
    run_database_task(move || database.list_accounts()).await
}

/// Signs in a new mail account through `provider`'s OAuth flow. Callers
/// that predate provider choice omit it and get Gmail, the only provider
/// they could have meant.
#[tauri::command]
async fn add_account(
    provider: Option<MailProviderKind>,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<Account, String> {
    let provider = provider.unwrap_or(MailProviderKind::Gmail);
    let auth = state.auth_config.pending_mail_account(provider)?;
    let email = authorize_interactively(&state, auth.credential()).await?;
    let account = state.database.adopt_mail_account(&email, provider)?;
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
    // Resolved before any local data is purged, so an unconfigured provider
    // fails the removal while the account is still intact.
    let fallback = state
        .auth_config
        .mail_account(catalogued_mail_provider(&state.database, &email)?, &email)?;
    state.database.pause_ready_sends_for(&email)?;
    // Stop every sync run for the account before purging, including ones
    // started outside the polling loop (foreground, backfill, manual
    // refresh): a run still waiting on the provider would otherwise write
    // the account's threads and cursor back after the purge.
    let service = state.accounts.lock().await.get(&email).map(|connected| connected.sync.clone());
    if let Some(service) = &service {
        service.retire().await;
    }
    // Purge local data first: if this fails, the account is untouched and
    // its credentials are still live, so the caller can safely retry rather
    // than being left with a still-listed account whose credentials are
    // already gone.
    let purged = (|| -> Result<(), String> {
        if !remove_catalog && state.database.cross_device_sync_enrolled()? {
            state.database.disconnect_account_locally(&email)?;
        } else {
            state.database.remove_account(&email)?;
        }
        Ok(())
    })();
    if let Err(error) = purged {
        if let Some(service) = &service {
            service.resume();
        }
        return Err(error);
    }
    let result = match state.accounts.lock().await.remove(&email) {
        Some(connected) => {
            connected.poll_task.abort();
            connected.auth.disconnect()
        }
        None => fallback.disconnect(),
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
    for goal in state.database.list_goals(Some(&email))? {
        state.database.record_local_entity_deletion(threestrands_sync_protocol::EntityType::Goal, &goal.id)?;
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
    // First-run connect (`connect_google`) authorizes against this entry,
    // so it is a Gmail placeholder.
    let Ok(auth) = state
        .auth_config
        .mail_account(MailProviderKind::Gmail, auth::LEGACY_KEY)
    else {
        return;
    };
    let mut accounts = state.accounts.lock().await;
    if accounts.is_empty() {
        accounts.insert(
            auth::LEGACY_KEY.to_string(),
            spawn_synced_account(state.database.clone(), auth, false, app),
        );
    }
}

#[tauri::command]
async fn reconnect_account(
    email: String,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<Account, String> {
    let fallback = state
        .auth_config
        .mail_account(catalogued_mail_provider(&state.database, &email)?, &email)?;
    let auth = match state.accounts.lock().await.get(&email) {
        Some(connected) => connected.auth.clone(),
        None => fallback,
    };
    authorize_interactively(&state, auth.credential()).await?;
    // Make the persisted status authoritative before any newly spawned
    // service checks it. Previously the service could observe needs_reauth,
    // skip its initial sync, and sleep until the first polling interval.
    let account = state.database.adopt_mail_account(&email, auth.mail_provider())?;
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

#[tauri::command(async)]
fn list_calendar_accounts(state: State<'_, AppState>) -> Result<Vec<CalendarAccount>, String> {
    let config = state.auth_config.google().ok();
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
    let config = state.auth_config.google()?;
    let auth = config.pending_calendar_account();
    let email = authorize_interactively(&state, &auth).await?;
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
    let config = state.auth_config.google()?;
    let auth = config.calendar_account(&email);
    authorize_interactively(&state, &auth).await?;
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
    config: &auth::OAuthApp,
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
    config: &auth::OAuthApp,
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
    let Ok(config) = state.auth_config.google() else {
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
    let config = state.auth_config.google()?;
    let mut options = Vec::new();
    for account in state.database.list_calendar_accounts()? {
        options.extend(
            calendar_options_for_account(config, &state.database, &account.email).await?,
        );
    }
    Ok(options)
}

fn validate_calendar_event_request(request: &CreateCalendarEventRequest) -> Result<(), String> {
    if request.title.trim().is_empty() || request.title.len() > 1024 {
        return Err("Enter an event title of at most 1024 characters".into());
    }
    if request.description.len() > 32_768 {
        return Err("Event description is too long".into());
    }
    let start = chrono::DateTime::parse_from_rfc3339(&request.start)
        .map_err(|_| "Enter a valid event start time".to_string())?;
    let end = chrono::DateTime::parse_from_rfc3339(&request.end)
        .map_err(|_| "Enter a valid event end time".to_string())?;
    if end <= start {
        return Err("Event end must be after its start".into());
    }
    if request.attendees.len() > 100 || request.attendees.iter().any(|email| {
        email.len() > 254 || email.contains(char::is_whitespace)
            || !email.split_once('@').is_some_and(|(local, domain)| !local.is_empty() && domain.contains('.'))
    }) {
        return Err("Enter up to 100 valid attendee email addresses".into());
    }
    Ok(())
}

#[cfg(test)]
mod calendar_event_request_tests {
    use super::*;

    fn request() -> CreateCalendarEventRequest {
        CreateCalendarEventRequest {
            account_id: "work@example.com".into(), calendar_id: "primary".into(),
            title: "Planning".into(), start: "2026-09-22T09:00:00Z".into(),
            end: "2026-09-22T10:00:00Z".into(), description: "Agenda".into(),
            attendees: vec!["guest@example.com".into()],
        }
    }

    #[test]
    fn validates_event_times_and_invitees_before_the_network_request() {
        assert!(validate_calendar_event_request(&request()).is_ok());
        let mut invalid = request();
        invalid.end = invalid.start.clone();
        assert!(validate_calendar_event_request(&invalid).is_err());
        invalid = request();
        invalid.attendees = vec!["not-an-email".into()];
        assert!(validate_calendar_event_request(&invalid).is_err());
        invalid = request();
        invalid.title = " ".into();
        assert!(validate_calendar_event_request(&invalid).is_err());
    }
}

#[tauri::command]
async fn create_calendar_event(
    request: CreateCalendarEventRequest,
    state: State<'_, AppState>,
) -> Result<ScheduleEvent, String> {
    validate_calendar_event_request(&request)?;
    let account = state.database.list_calendar_accounts()?.into_iter()
        .find(|account| account.email == request.account_id && account.status == "connected")
        .ok_or_else(|| "Connect this calendar account in Settings first".to_string())?;
    let config = state.auth_config.google()?;
    let options = calendar::list_calendar_options(config.calendar_account(&account.email), &account.email).await?;
    if !options.iter().any(|option| option.id == request.calendar_id && option.writable) {
        return Err("Choose a calendar where you can create events".into());
    }
    calendar::create_event(config.calendar_account(&account.email), &request).await
}

#[tauri::command]
async fn set_calendar_selection(
    account_id: String,
    calendar_ids: Vec<String>,
    state: State<'_, AppState>,
) -> Result<Vec<CalendarOption>, String> {
    let config = state.auth_config.google()?;
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

#[tauri::command(async)]
fn remove_calendar_account(email: String, state: State<'_, AppState>) -> Result<(), String> {
    let config = state.auth_config.google()?;
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
    let config = state.auth_config.google()?;
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
    let config = state.auth_config.google()?;
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
async fn update_calendar_response(
    account_id: String,
    calendar_id: String,
    event_id: String,
    response_status: String,
    state: State<'_, AppState>,
) -> Result<models::ScheduleEvent, String> {
    let config = state.auth_config.google()?;
    if !state.database.list_calendar_accounts()?.iter().any(|account| account.email == account_id) {
        return Err("Calendar account is not connected".into());
    }
    calendar::update_response(
        config.calendar_account(&account_id),
        &account_id,
        &calendar_id,
        &event_id,
        &response_status,
    ).await
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
        request.max_per_day,
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

#[tauri::command(async)]
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

#[tauri::command(async)]
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

#[tauri::command(async)]
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
async fn list_split_inboxes(state: State<'_, AppState>) -> Result<Vec<SplitInbox>, String> {
    let database = state.database.clone();
    run_database_task(move || database.list_split_inboxes()).await
}

#[tauri::command(async)]
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

#[tauri::command(async)]
fn update_split_inbox(
    request: UpdateSplitInboxRequest,
    state: State<'_, AppState>,
) -> Result<SplitInbox, String> {
    let item = state.database.update_split_inbox(&request.id, &request.name)?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::SplitInbox, &item.id, &item,
        Some(std::collections::BTreeSet::from(["name".to_string()])))?;
    Ok(item)
}

#[tauri::command(async)]
fn delete_split_inbox(id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.database.delete_split_inbox(&id)?;
    state.database.record_local_entity_deletion(threestrands_sync_protocol::EntityType::SplitInbox, &id)?;
    kick_replicated_sync(&state);
    Ok(())
}

#[tauri::command(async)]
fn reorder_split_inboxes(ids: Vec<String>, state: State<'_, AppState>) -> Result<(), String> {
    state.database.reorder_split_inboxes(&ids)?;
    for item in state.database.list_split_inboxes()? {
        record_synced_value(&state, threestrands_sync_protocol::EntityType::SplitInbox, &item.id, &item,
            Some(std::collections::BTreeSet::from(["sortOrder".to_string()])))?;
    }
    Ok(())
}

#[tauri::command]
async fn list_split_inbox_page(
    split_inbox_id: String,
    offset: usize,
    limit: usize,
    state: State<'_, AppState>,
) -> Result<ThreadPage, String> {
    let database = state.database.clone();
    run_database_task(move || database.list_split_inbox_page(&split_inbox_id, offset, limit)).await
}

#[tauri::command]
async fn list_snippets(state: State<'_, AppState>) -> Result<Vec<Snippet>, String> {
    let database = state.database.clone();
    run_database_task(move || database.list_snippets()).await
}

#[tauri::command(async)]
fn create_snippet(
    request: CreateSnippetRequest,
    state: State<'_, AppState>,
) -> Result<Snippet, String> {
    let item = state.database.create_snippet(&request.name, &request.body)?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::Snippet, &item.id, &item, None)?;
    Ok(item)
}

#[tauri::command(async)]
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

#[tauri::command(async)]
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
async fn get_retention_days(state: State<'_, AppState>) -> Result<Option<i64>, String> {
    let database = state.database.clone();
    run_database_task(move || database.retention_days()).await
}

#[tauri::command(async)]
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
    reasoning: Option<ai::Reasoning>,
    state: State<'_, AppState>,
) -> Result<SummaryResult, String> {
    let api_key = ai::get_key()?.ok_or_else(|| "No AI API key configured".to_string())?;
    let detail = state.database.get_thread(&thread_id)?;
    let request = ai::SummarizeRequest {
        provider,
        model,
        endpoint,
        reasoning: reasoning.unwrap_or_default(),
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

/// Validates the timezone and loads the bounded analysis input shared by
/// thread analysis and the combined brief. Returns the thread's current
/// revision (its newest message time), which keys saved suggestions.
async fn thread_analysis_request(
    thread_id: &str,
    user_time_zone: String,
    provider: ai::AiProvider,
    model: String,
    endpoint: Option<String>,
    state: &AppState,
) -> Result<(String, ai::AnalyzeRequest), String> {
    if user_time_zone.parse::<chrono_tz::Tz>().is_err() {
        return Err(format!("Unknown IANA timezone: {user_time_zone}"));
    }
    if user_time_zone.chars().count() > 100 {
        return Err("Timezone value is too long".to_string());
    }
    let database = state.database.clone();
    let database_thread_id = thread_id.to_string();
    let detail = run_database_task(move || database.get_thread(&database_thread_id)).await?;
    let revision = detail.thread.last_message_at.clone();
    let goals = state
        .database
        .list_goals(Some(&detail.thread.account_id))?
        .into_iter()
        .filter(|goal| goal.status == "active")
        .take(ai::MAX_PROMPT_GOALS)
        .map(|goal| ai::GoalPromptInput { id: goal.id, title: goal.title, horizon: goal.horizon, period: goal.period })
        .collect();
    let request = ai::AnalyzeRequest {
        provider,
        model,
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
        goals,
    };
    Ok((revision, request))
}

/// Saves the suggestions for the thread's current revision, replacing any
/// saved for an older one.
fn cache_thread_analysis(
    state: &AppState,
    thread_id: &str,
    revision: &str,
    analysis: &ActionAnalysis,
) -> Result<(), String> {
    database_result(state.database.save_thread_analysis(thread_id, revision, analysis, &Utc::now().to_rfc3339()))
}

#[tauri::command]
async fn ai_analyze_thread(
    thread_id: String,
    user_time_zone: String,
    provider: ai::AiProvider,
    model: String,
    endpoint: Option<String>,
    state: State<'_, AppState>,
) -> Result<ActionAnalysis, String> {
    let (revision, request) = thread_analysis_request(
        &thread_id,
        user_time_zone,
        provider,
        model.clone(),
        endpoint,
        &state,
    )
    .await?;
    if let Some(saved) = state.database.thread_analysis(&thread_id, &revision)? {
        return Ok(saved);
    }
    let api_key = ai::get_key()?.ok_or_else(|| "No AI API key configured".to_string())?;
    log::info!(
        target: "ai_analyze_thread",
        "starting analysis for thread {thread_id} with provider {provider:?} model {model}"
    );
    let analysis = match ai::analyze(request, &api_key).await {
        Ok(analysis) => analysis,
        Err(error) => {
            log::error!(target: "ai_analyze_thread", "analysis failed for thread {thread_id}: {error}");
            return Err(error);
        }
    };
    log::info!(
        target: "ai_analyze_thread",
        "analysis succeeded for thread {thread_id} with {} proposal(s), {} withheld",
        analysis.proposals.len(),
        analysis.hidden_count
    );
    cache_thread_analysis(&state, &thread_id, &revision, &analysis)?;
    Ok(analysis)
}

/// Drops a handled suggestion from the thread's saved suggestions for
/// `revision` (the thread's newest message time when it was shown).
#[tauri::command]
async fn remove_thread_suggestion(
    thread_id: String,
    revision: String,
    proposal: ActionProposal,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    let database = state.database.clone();
    run_database_task(move || database.remove_thread_suggestion(&thread_id, &revision, &proposal)).await
}

/// One line per open task for chat context: its title and due value.
fn chat_task_line(task: &ThreadTask) -> String {
    match &task.due_value {
        Some(due) => format!("{} (due {due})", task.title),
        None => task.title.clone(),
    }
}

/// The attachments a question shares, once each, after checking each belongs
/// to a message in the open conversation.
fn chat_attachment_refs(
    requested: &[ChatAttachmentRef],
    thread_message_ids: &[&str],
) -> Result<Vec<ChatAttachmentRef>, String> {
    let mut unique: Vec<ChatAttachmentRef> = Vec::new();
    for reference in requested {
        if !unique.contains(reference) {
            unique.push(reference.clone());
        }
    }
    if unique.len() > attachment_text::MAX_CHAT_ATTACHMENTS {
        return Err(format!(
            "Share up to {} attachments at a time",
            attachment_text::MAX_CHAT_ATTACHMENTS
        ));
    }
    if unique.iter().any(|reference| !thread_message_ids.contains(&reference.message_id.as_str())) {
        return Err("That attachment isn't in this conversation".to_string());
    }
    Ok(unique)
}

/// Extracts an attachment's text off the async runtime, failing if the
/// parser runs past the timeout or panics on a hostile file.
async fn extract_chat_attachment(
    filename: &str,
    mime_type: &str,
    bytes: Vec<u8>,
    timeout: std::time::Duration,
) -> Result<attachment_text::ExtractedText, String> {
    let kind = attachment_text::kind_for(filename, mime_type).ok_or_else(|| {
        format!("Thread Chat can't read {filename}; it reads text, PDF, Word, Excel, and PowerPoint files")
    })?;
    run_attachment_extraction(filename, timeout, move || attachment_text::extract(kind, &bytes)).await
}

async fn run_attachment_extraction(
    filename: &str,
    timeout: std::time::Duration,
    job: impl FnOnce() -> Result<attachment_text::ExtractedText, String> + Send + 'static,
) -> Result<attachment_text::ExtractedText, String> {
    let task = tokio::task::spawn_blocking(job);
    let reason = match tokio::time::timeout(timeout, task).await {
        Ok(Ok(Ok(text))) => return Ok(text),
        Ok(Ok(Err(reason))) => reason,
        Ok(Err(_)) => "it isn't a readable file".to_string(),
        Err(_) => "it took too long to read".to_string(),
    };
    Err(format!("Couldn't read {filename}: {reason}"))
}

/// Loads and extracts the shared attachments, reusing recent extractions.
async fn chat_attachments(
    references: Vec<ChatAttachmentRef>,
    state: &AppState,
) -> Result<(Vec<ChatAttachmentSource>, Vec<ai::ChatAttachmentInput>), String> {
    let mut sources = Vec::new();
    let mut inputs = Vec::new();
    for reference in references {
        let (filename, mime_type, extracted) = match state.attachment_text.get(&reference.message_id, &reference.attachment_id) {
            Some(cached) => cached,
            None => {
                let (filename, mime_type, bytes) =
                    load_attachment(&reference.message_id, &reference.attachment_id, state).await?;
                let text = extract_chat_attachment(&filename, &mime_type, bytes, attachment_text::ATTACHMENT_EXTRACT_TIMEOUT).await?;
                let entry = (filename, mime_type, text);
                state.attachment_text.insert(&reference.message_id, &reference.attachment_id, entry.clone());
                entry
            }
        };
        sources.push(ChatAttachmentSource {
            message_id: reference.message_id.clone(),
            attachment_id: reference.attachment_id,
            filename: filename.clone(),
            truncated: extracted.truncated,
        });
        inputs.push(ai::ChatAttachmentInput {
            message_id: reference.message_id,
            filename,
            mime_type,
            text: extracted.text,
            truncated: extracted.truncated,
        });
    }
    Ok((sources, inputs))
}

#[cfg(test)]
mod chat_attachment_tests {
    use super::*;
    use std::time::Duration;

    fn reference(message_id: &str, attachment_id: &str) -> ChatAttachmentRef {
        ChatAttachmentRef { message_id: message_id.into(), attachment_id: attachment_id.into() }
    }

    #[test]
    fn shares_each_attachment_once_and_only_from_the_open_conversation() {
        let ids = ["m1", "m2"];
        let shared = chat_attachment_refs(&[reference("m1", "a"), reference("m2", "b"), reference("m1", "a")], &ids).unwrap();
        assert_eq!(shared, vec![reference("m1", "a"), reference("m2", "b")]);
        assert_eq!(
            chat_attachment_refs(&[reference("m1", "a"), reference("elsewhere", "a")], &ids).unwrap_err(),
            "That attachment isn't in this conversation"
        );
        assert!(chat_attachment_refs(&[], &ids).unwrap().is_empty());
    }

    #[test]
    fn limits_how_many_attachments_one_question_shares() {
        let ids = ["m1"];
        let refs: Vec<ChatAttachmentRef> = (0..=attachment_text::MAX_CHAT_ATTACHMENTS).map(|index| reference("m1", &index.to_string())).collect();
        assert_eq!(chat_attachment_refs(&refs[..attachment_text::MAX_CHAT_ATTACHMENTS - 1], &ids).unwrap().len(), attachment_text::MAX_CHAT_ATTACHMENTS - 1);
        assert_eq!(chat_attachment_refs(&refs[..attachment_text::MAX_CHAT_ATTACHMENTS], &ids).unwrap().len(), attachment_text::MAX_CHAT_ATTACHMENTS);
        assert_eq!(
            chat_attachment_refs(&refs, &ids).unwrap_err(),
            format!("Share up to {} attachments at a time", attachment_text::MAX_CHAT_ATTACHMENTS)
        );
    }

    #[tokio::test]
    async fn names_the_file_when_it_cannot_be_read() {
        let text = extract_chat_attachment("notes.txt", "text/plain", b"Ship Friday".to_vec(), Duration::from_secs(5)).await.unwrap();
        assert_eq!(text.text, "Ship Friday");
        assert_eq!(
            extract_chat_attachment("photo.jpg", "image/jpeg", vec![1, 2, 3], Duration::from_secs(5)).await.unwrap_err(),
            "Thread Chat can't read photo.jpg; it reads text, PDF, Word, Excel, and PowerPoint files"
        );
        assert_eq!(
            extract_chat_attachment("contract.docx", "", b"not a zip".to_vec(), Duration::from_secs(5)).await.unwrap_err(),
            "Couldn't read contract.docx: it isn't a readable Office file"
        );
    }

    #[tokio::test]
    async fn a_parser_that_hangs_or_panics_fails_only_the_question() {
        let slow = run_attachment_extraction("slow.pdf", Duration::from_millis(10), || {
            std::thread::sleep(Duration::from_millis(300));
            Ok(attachment_text::ExtractedText { text: "late".into(), truncated: false })
        })
        .await;
        assert_eq!(slow.unwrap_err(), "Couldn't read slow.pdf: it took too long to read");

        let panicked = run_attachment_extraction("bad.pdf", Duration::from_secs(5), || panic!("hostile file")).await;
        assert_eq!(panicked.unwrap_err(), "Couldn't read bad.pdf: it isn't a readable file");
    }
}

/// Answers a question about the open conversation. The context is the
/// conversation itself, open tasks linked to it or to the selected person,
/// the text of any attachments the user shared for the question, and, only
/// when `search_mailbox` is set, the best-matching other conversations from
/// local search.
#[tauri::command]
async fn ai_thread_chat(
    request: ThreadChatRequest,
    provider: ai::AiProvider,
    model: String,
    endpoint: Option<String>,
    state: State<'_, AppState>,
) -> Result<ThreadChatReply, String> {
    let question = request.question.trim().to_string();
    if question.is_empty() {
        return Err("Ask a question first".to_string());
    }
    if question.chars().count() > ai::MAX_CHAT_TURN_CHARS {
        return Err(format!("Questions can be up to {} characters", ai::MAX_CHAT_TURN_CHARS));
    }
    let thread_id = request.thread_id.clone();
    let (_revision, analysis_request) = thread_analysis_request(
        &thread_id,
        request.user_time_zone.clone(),
        provider,
        model.clone(),
        endpoint.clone(),
        &state,
    )
    .await?;
    let message_ids: Vec<&str> = analysis_request.messages.iter().map(|message| message.id.as_str()).collect();
    let attachment_refs = chat_attachment_refs(&request.attachments, &message_ids)?;
    let (shared_attachments, attachment_inputs) = chat_attachments(attachment_refs, &state).await?;

    let database = state.database.clone();
    let contact_id = request.contact_id.clone();
    let search_terms = request.search_mailbox.then(|| ai::chat_search_terms(&question));
    let lookup_thread_id = thread_id.clone();
    let (open_tasks, other_threads) = run_database_task(move || {
        let mut tasks: Vec<ThreadTask> = database
            .list_tasks(None, None)?
            .into_iter()
            .filter(|task| task.thread_id.as_deref() == Some(lookup_thread_id.as_str()) && matches!(task.status.as_str(), "open" | "in_progress"))
            .collect();
        if let Some(contact_id) = contact_id {
            for task in database.list_contact_tasks(&contact_id)? {
                if !tasks.iter().any(|existing| existing.id == task.id) {
                    tasks.push(task);
                }
            }
        }
        let mut others = Vec::new();
        if let Some(terms) = search_terms {
            for id in database.chat_search_thread_ids(&terms, &lookup_thread_id, ai::MAX_MAILBOX_CHAT_THREADS)? {
                others.push(database.get_thread(&id)?);
            }
        }
        Ok::<_, db::DatabaseError>((tasks, others))
    })
    .await?;

    let searched: Vec<ChatSource> = other_threads
        .iter()
        .map(|detail| ChatSource {
            thread_id: detail.thread.id.clone(),
            account_id: detail.thread.account_id.clone(),
            subject: detail.thread.subject.clone(),
            last_message_at: detail.thread.last_message_at.clone(),
        })
        .collect();
    let chat_request = ai::ChatRequest {
        provider,
        model: model.clone(),
        endpoint,
        question,
        history: request.history,
        subject: analysis_request.subject,
        messages: analysis_request.messages,
        open_tasks: open_tasks.iter().map(chat_task_line).collect(),
        other_threads: other_threads
            .into_iter()
            .map(|detail| ai::ChatThreadInput {
                thread_id: detail.thread.id,
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
            })
            .collect(),
        attachments: attachment_inputs,
        proposals_allowed: request.include_proposals,
        current_time: analysis_request.current_time,
        user_time_zone: analysis_request.user_time_zone,
    };
    let api_key = ai::get_key()?.ok_or_else(|| "No AI API key configured".to_string())?;
    log::info!(
        target: "ai_thread_chat",
        "asking about thread {thread_id} with provider {provider:?} model {model}, {} other thread(s), {} attachment(s)",
        searched.len(),
        shared_attachments.len()
    );
    let answer = match ai::chat(chat_request, &api_key).await {
        Ok(answer) => answer,
        Err(error) => {
            log::error!(target: "ai_thread_chat", "chat failed for thread {thread_id}: {error}");
            return Err(error);
        }
    };
    let sources = answer
        .source_thread_ids
        .iter()
        .filter_map(|id| searched.iter().find(|source| &source.thread_id == id).cloned())
        .collect();
    Ok(ThreadChatReply {
        answer: answer.answer,
        analysis: answer.analysis,
        reply_draft: answer.reply_draft,
        sources,
        searched,
        attachments: shared_attachments,
        availability: answer.availability,
    })
}

/// Most recent days of AI usage the settings screen may request.
const MAX_AI_USAGE_SUMMARY_DAYS: u32 = 31;

/// AI provider usage for the last `days` local days, including today.
#[tauri::command(async)]
fn ai_usage_summary(days: u32, state: State<'_, AppState>) -> Result<Vec<AiUsageDay>, String> {
    if days == 0 || days > MAX_AI_USAGE_SUMMARY_DAYS {
        return Err(format!("Usage can be shown for 1 to {MAX_AI_USAGE_SUMMARY_DAYS} days"));
    }
    let since = chrono::Local::now().date_naive() - chrono::Days::new(u64::from(days - 1));
    database_result(state.database.ai_usage_since(&since.format("%Y-%m-%d").to_string()))
}

/// Summarizes the thread and extracts proposals in one provider call. Always
/// calls the provider; the summary is persisted like `ai_summarize_thread`
/// and the proposals replace the thread's cached analysis.
#[tauri::command]
async fn ai_brief_thread(
    thread_id: String,
    user_time_zone: String,
    provider: ai::AiProvider,
    model: String,
    endpoint: Option<String>,
    state: State<'_, AppState>,
) -> Result<ThreadBriefResult, String> {
    let (revision, request) = thread_analysis_request(
        &thread_id,
        user_time_zone,
        provider,
        model.clone(),
        endpoint,
        &state,
    )
    .await?;
    let api_key = ai::get_key()?.ok_or_else(|| "No AI API key configured".to_string())?;
    log::info!(
        target: "ai_analyze_thread",
        "starting brief for thread {thread_id} with provider {provider:?} model {model}"
    );
    let brief = match ai::brief(request, &api_key).await {
        Ok(brief) => brief,
        Err(error) => {
            log::error!(target: "ai_analyze_thread", "brief failed for thread {thread_id}: {error}");
            return Err(error);
        }
    };
    log::info!(
        target: "ai_analyze_thread",
        "brief succeeded for thread {thread_id} with {} proposal(s), {} withheld",
        brief.analysis.proposals.len(),
        brief.analysis.hidden_count
    );
    let generated_at = Utc::now().to_rfc3339();
    state
        .database
        .set_thread_summary(&thread_id, &brief.summary, &generated_at)?;
    cache_thread_analysis(&state, &thread_id, &revision, &brief.analysis)?;
    Ok(ThreadBriefResult {
        summary: SummaryResult {
            summary: brief.summary,
            generated_at,
        },
        analysis: brief.analysis,
    })
}

#[tauri::command]
async fn ai_enrich_contact(
    id: String,
    provider: ai::AiProvider,
    model: String,
    endpoint: Option<String>,
    empty_fields: Option<Vec<String>>,
    search_more: Option<bool>,
    account_id: Option<String>,
    reasoning: Option<ai::Reasoning>,
    state: State<'_, AppState>,
) -> Result<ai::ContactEnrichmentResult, String> {
    let profile = state
        .database
        .get_contact_profile(&id)?
        .ok_or_else(|| "Contact could not be found".to_string())?;
    // Inspect more conversations locally so the first provider batch can
    // favor messages that the contact opened, even if those are less recent.
    let timeline = state.database.contact_timeline_for_account(&id, 0, 30, account_id.as_deref())?;
    let addresses = profile
        .addresses
        .iter()
        .map(|value| value.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let mut messages = Vec::new();
    for item in timeline {
        let detail = state.database.get_thread(&item.thread_id)?;
        for (index, message) in detail.messages.into_iter().enumerate() {
            let sender_matches = crate::correspondence::stored_addresses(&message.sender)
                .iter()
                .any(|(_, email)| addresses.contains(&email.to_ascii_lowercase()));
            let recipient_matches = message.recipients.iter().any(|raw| {
                crate::correspondence::stored_addresses(raw)
                    .iter()
                    .any(|(_, email)| addresses.contains(&email.to_ascii_lowercase()))
            });
            if sender_matches || recipient_matches {
                messages.push(ai::ContactMessageInput {
                    id: message.id,
                    thread_id: item.thread_id.clone(),
                    sender: message.sender,
                    sent_at: message.sent_at,
                    subject: detail.thread.subject.clone(),
                    body_text: message.body_text,
                    from_contact: sender_matches,
                    is_thread_starter: index == 0,
                });
            }
        }
    }
    let api_key = ai::get_key()?.ok_or_else(|| "No AI API key configured".to_string())?;
    ai::enrich_contact(
        ai::ContactEnrichmentRequest {
            provider,
            model,
            endpoint,
            reasoning: reasoning.unwrap_or_default(),
            profile,
            messages,
            search_more: search_more.unwrap_or(false),
            empty_fields,
        },
        &api_key,
    )
    .await
}

#[tauri::command(async)]
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
    reasoning: Option<ai::Reasoning>,
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
        reasoning.unwrap_or_default(),
        &api_key,
    )
    .await?;
    Ok(ReplyAssistResult { body })
}

#[tauri::command]
async fn list_tasks(
    account_id: Option<String>,
    status: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<ThreadTask>, String> {
    let database = state.database.clone();
    run_database_task(move || database.list_tasks(account_id.as_deref(), status.as_deref())).await
}

#[tauri::command(async)]
fn create_task(request: CreateTaskRequest, state: State<'_, AppState>) -> Result<ThreadTask, String> {
    let task = state.database.create_task(&request)?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::Task, &task.id, &task, None)?;
    Ok(task)
}

#[tauri::command(async)]
fn update_task(request: UpdateTaskRequest, state: State<'_, AppState>) -> Result<ThreadTask, String> {
    let mut fields = std::collections::BTreeSet::new();
    if request.title.is_some() { fields.insert("title".to_string()); }
    if request.notes.is_some() { fields.insert("notes".to_string()); }
    if request.kind.is_some() { fields.insert("kind".to_string()); }
    if request.due_kind.is_some() { fields.insert("dueKind".to_string()); }
    if request.due_value.is_some() { fields.insert("dueValue".to_string()); }
    if request.time_zone.is_some() { fields.insert("timeZone".to_string()); }
    if request.repeat_interval_days.is_some() { fields.insert("repeatIntervalDays".to_string()); }
    if request.goal_id.is_some() { fields.insert("goalId".to_string()); }
    fields.insert("updatedAt".to_string());
    let task = state.database.update_task(&request)?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::Task, &task.id, &task, Some(fields))?;
    Ok(task)
}

#[tauri::command]
async fn list_goals(account_id: Option<String>, state: State<'_, AppState>) -> Result<Vec<Goal>, String> {
    let database = state.database.clone();
    run_database_task(move || database.list_goals(account_id.as_deref())).await
}

#[tauri::command(async)]
fn create_goal(request: CreateGoalRequest, state: State<'_, AppState>) -> Result<Goal, String> {
    let goal = state.database.create_goal(&request)?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::Goal, &goal.id, &goal, None)?;
    Ok(goal)
}

#[tauri::command(async)]
fn update_goal(request: UpdateGoalRequest, state: State<'_, AppState>) -> Result<Goal, String> {
    let mut fields = std::collections::BTreeSet::from(["updatedAt".to_string()]);
    if request.title.is_some() { fields.insert("title".to_string()); }
    if request.notes.is_some() { fields.insert("notes".to_string()); }
    if request.horizon.is_some() { fields.insert("horizon".to_string()); }
    if request.period.is_some() { fields.insert("period".to_string()); }
    if request.status.is_some() { fields.extend(["status".to_string(), "closedAt".to_string()]); }
    if request.parent_goal_id.is_some() { fields.insert("parentGoalId".to_string()); }
    let goal = state.database.update_goal(&request)?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::Goal, &goal.id, &goal, Some(fields))?;
    Ok(goal)
}

#[tauri::command(async)]
fn delete_goal(id: String, state: State<'_, AppState>) -> Result<(), String> {
    let deletion = state.database.delete_goal(&id)?;
    state.database.record_local_entity_deletion(threestrands_sync_protocol::EntityType::Goal, &id)?;
    for task in &deletion.tasks {
        record_synced_value(&state, threestrands_sync_protocol::EntityType::Task, &task.id, task,
            Some(std::collections::BTreeSet::from(["goalId".to_string(), "updatedAt".to_string()])))?;
    }
    for goal in &deletion.children {
        record_synced_value(&state, threestrands_sync_protocol::EntityType::Goal, &goal.id, goal,
            Some(std::collections::BTreeSet::from(["parentGoalId".to_string(), "updatedAt".to_string()])))?;
    }
    kick_replicated_sync(&state);
    Ok(())
}

#[tauri::command(async)]
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

#[tauri::command(async)]
fn record_follow_up(id: String, state: State<'_, AppState>) -> Result<ThreadTask, String> {
    let task = state.database.record_follow_up(&id)?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::Task, &task.id, &task,
        Some(std::collections::BTreeSet::from(["dueValue".to_string(), "waitAfter".to_string(), "completionSource".to_string(), "completedAt".to_string(), "updatedAt".to_string()])))?;
    Ok(task)
}

#[tauri::command(async)]
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
    let usage_database = database.clone();
    ai::set_usage_recorder(Arc::new(move |event| {
        let day = chrono::Local::now().format("%Y-%m-%d").to_string();
        if let Err(error) = usage_database.record_ai_usage(&day, event.provider.id(), &event.model, event.input_tokens, event.output_tokens, event.cost_usd) {
            log::warn!(target: "ai_provider", "could not record AI usage: {error}");
        }
    }));
    let recovery = match recovery {
        db::RecoveryOutcome::Clean => None,
        other => Some(other),
    };
    if recovery.is_some() {
        force_full_resync(&database);
    }
    let auth_config = AuthConfig::from_environment();
    spawn_badge_loop(database.clone(), app.handle().clone());
    spawn_storage_maintenance(database.clone());
    let root = data_dir.join("attachments");
    std::fs::create_dir_all(&root)?;
    restrict_dir_to_owner(&root);
    let attachment_reader = attachment_reader::ReaderCache::new(root.join("reader"))?;
    // Built before `manage` so the registry is never observed empty by a
    // command racing startup.
    let registry = startup_account_registry(&database, &auth_config, app.handle());
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
        attachment_text: attachment_text::ExtractCache::default(),
        recovery,
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

const FIRST_STORAGE_MAINTENANCE_DELAY: std::time::Duration = std::time::Duration::from_secs(3 * 60);

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
        // Same for raw provider payloads cached before they were compressed.
        loop {
            let backfill_db = database.clone();
            let converted =
                tokio::task::spawn_blocking(move || backfill_db.compress_next_metadata_batch(100))
                    .await
                    .ok()
                    .and_then(|result| log_failure("compressing message metadata", result))
                    .unwrap_or(0);
            if converted == 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        // Same for search rows queued by schema v46, rewritten without
        // quoted history the thread already contains.
        loop {
            let backfill_db = database.clone();
            let reindexed =
                tokio::task::spawn_blocking(move || backfill_db.reindex_next_search_batch(200))
                    .await
                    .ok()
                    .and_then(|result| log_failure("reindexing search rows", result))
                    .unwrap_or(0);
            if reindexed == 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        // Stay clear of the launch window, when the inbox loads and every
        // account runs its first sync; maintenance is never urgent.
        tokio::time::sleep(FIRST_STORAGE_MAINTENANCE_DELAY).await;
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
    step(
        "pruning orphaned message metadata",
        database.prune_orphaned_message_metadata().map(|_| ()),
    );
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
    auth_config: &AuthConfig,
    handle: &tauri::AppHandle,
) -> HashMap<String, ConnectedAccount> {
    let mut registry = HashMap::new();
    for (key, auth) in startup_account_credentials(database, auth_config) {
        registry.insert(
            key,
            spawn_synced_account(database.clone(), auth, true, handle.clone()),
        );
    }
    registry
}

/// The credential each catalogued account starts with, keyed by account
/// id — or, before the first connect, the Gmail placeholder. An account
/// whose provider has no configured OAuth app is left out rather than
/// failing the others.
fn startup_account_credentials(
    database: &Database,
    auth_config: &AuthConfig,
) -> Vec<(String, AccountAuth)> {
    let catalog = log_failure("listing accounts at startup", database.list_accounts())
        .unwrap_or_default();
    if catalog.is_empty() {
        return auth_config
            .mail_account(MailProviderKind::Gmail, auth::LEGACY_KEY)
            .map(|auth| vec![(auth::LEGACY_KEY.to_string(), auth)])
            .unwrap_or_default();
    }
    catalog
        .into_iter()
        .filter_map(|account| {
            let provider = MailProviderKind::parse(&account.provider).or_else(|| {
                log::warn!(
                    "skipping {}: unsupported mail provider {:?}",
                    account.email,
                    account.provider
                );
                None
            })?;
            let auth = log_failure(
                "starting a catalogued account",
                auth_config.mail_account(provider, &account.email),
            )?;
            Some((account.email, auth))
        })
        .collect()
}

/// The provider a catalogued account was connected through. An address no
/// longer in the catalog (e.g. a retried removal) is treated as Gmail, the
/// only provider that existed before the column did.
fn catalogued_mail_provider(database: &Database, email: &str) -> Result<MailProviderKind, String> {
    match database.get_account(email)? {
        None => Ok(MailProviderKind::Gmail),
        Some(account) => MailProviderKind::parse(&account.provider)
            .ok_or_else(|| format!("{email} uses an unsupported mail provider: {}", account.provider)),
    }
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
        replicated_sync_protocol_reset_notice,
        replicated_sync_dismiss_protocol_reset_notice,
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
        list_contact_profiles,
        resolve_contact_ids,
        get_contact_profile,
        save_contact_profile,
        delete_contact_profile,
        contact_timeline,
        contact_activity,
        contact_files,
        domain_context,
        list_contact_tasks,
        pin_contact,
        unpin_contact,
        unsubscribe,
        // Mail sync
        sync_status,
        sync_account,
        flush_pending_mutations,
        retry_failed_mutations,
        dismiss_sync_problems,
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
        create_calendar_event,
        set_calendar_selection,
        remove_calendar_account,
        remove_synced_calendar_account,
        list_schedule_events,
        update_calendar_response,
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
        remove_thread_suggestion,
        ai_brief_thread,
        ai_usage_summary,
        ai_thread_chat,
        ai_enrich_contact,
        ai_reply_assist_context,
        ai_generate_reply,
        // Tasks and follow-ups
        list_tasks,
        create_task,
        update_task,
        list_goals,
        create_goal,
        update_goal,
        delete_goal,
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
        tauri::RunEvent::Exit => {
            if let Some(state) = handle.try_state::<AppState>() {
                log_failure("checkpointing the WAL at exit", state.database.checkpoint_on_exit());
            }
        }
        _ => {}
    }
}
