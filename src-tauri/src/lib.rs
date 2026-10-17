mod ai;
mod app_update;
mod availability;
mod attachment_reader;
mod file_preview;
mod attachment_text;
mod attachment_security;
mod auth;
mod backoff;
mod calendar;
mod calendar_files;
mod commands;
mod correspondence;
mod signatures;
mod gmail_signatures;
mod scheduled_send;
mod contact_interchange;
mod credentials;
mod db;
mod default_apps;
mod endpoint_origin;
mod enrollment;
mod error_text;
mod http_client;
mod image_format;
mod image_proxy;
mod ipfs_transport;
mod limits;
mod mail_links;
mod models;
// Mail parsing and text handling live in their own crates; these keep `crate::mime::…` and the
// other module paths the app already uses.
use threestrands_mail_mime::{html_plain_text, mime, sender_trust};
use threestrands_mail_text::{quoted_history, search_query};
mod net_safety;
mod provider;
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

use auth::{AccountAuth, AuthConfig};
use db::Database;
use models::{MailProviderKind, SyncStatus};
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
        .on_navigation(|webview, url| {
            if allow_in_app_navigation(url) {
                return true;
            }
            // Mail links start a draft here rather than bouncing through the
            // OS mail handler, which may be ThreeStrands itself.
            if url.scheme() == "mailto" {
                mail_links::deliver(webview.app_handle(), url.as_str());
            } else if matches!(url.scheme(), "http" | "https" | "tel") {
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
    /// Scratch space for Quick Look previews in Files.
    file_previews: file_preview::PreviewRoot,
    /// Attachment text recently shared with Thread Chat.
    attachment_text: attachment_text::ExtractCache,
    /// `Some` only when the local database had to be recovered at startup
    /// (restored from a backup, or recreated fresh) — see `open_with_recovery`.
    recovery: Option<db::RecoveryOutcome>,
    /// A decrypted settings import awaiting the user's account choices.
    staged_import: transfer::StagedImport,
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
            "preview_settings_import",
            "ai_api_key_configured",
            "set_ai_api_key",
            "recovery_status",
            "replicated_sync_check_recovery_phrase",
            "replicated_sync_preview_join_code",
        ];
        // Commands live in `lib.rs` and every module under `commands/`.
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = vec![src.join("lib.rs")];
        for entry in std::fs::read_dir(src.join("commands")).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
        let mut synchronous = Vec::new();
        for file in &files {
            let source = std::fs::read_to_string(file).unwrap();
            let lines: Vec<&str> = source.lines().collect();
            synchronous.extend(
                lines
                    .windows(2)
                    .filter(|pair| pair[0].trim() == "#[tauri::command]")
                    .filter_map(|pair| {
                        let item = pair[1].trim();
                        item.strip_prefix("pub(crate) ").unwrap_or(item).strip_prefix("fn ")
                    })
                    .filter_map(|rest| rest.split('(').next())
                    .map(str::to_string),
            );
        }

        // Every allowlisted command is synchronous, so a scan that misses
        // one has stopped seeing the command definitions at all.
        let missed: Vec<&&str> = MAIN_THREAD_COMMANDS
            .iter()
            .filter(|name| !synchronous.iter().any(|found| found == *name))
            .collect();
        assert!(missed.is_empty(), "the scan no longer finds these commands: {missed:?}");

        let unexpected: Vec<&String> = synchronous
            .iter()
            .filter(|name| !MAIN_THREAD_COMMANDS.contains(&name.as_str()))
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

    // Phase 2 Slice 5a INTENTIONALLY changes the previous contract that IMAP
    // accounts are skipped at startup: they now have a real `MailProvider`, so
    // a catalogued IMAP account enters the runtime registry and syncs. (The
    // provider itself is built lazily at sync time from saved settings + the
    // keychain password; an account missing those reports needs-reauth there,
    // not here, so it still cannot block startup.) Unlike Gmail, an IMAP
    // account needs no configured Google OAuth app to start.
    #[test]
    fn an_imap_account_now_enters_the_startup_registry() {
        let database = Database::open_memory();
        database.adopt_mail_account("imap@example.com", MailProviderKind::Imap).unwrap();

        for config in [AuthConfig::default(), AuthConfig::google_for_test()] {
            let credentials = startup_account_credentials(&database, &config);
            let keys = credentials.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>();
            assert_eq!(keys, ["imap@example.com"], "IMAP accounts now start");
            assert_eq!(credentials[0].1.mail_provider(), MailProviderKind::Imap);
        }
        assert_eq!(database.list_accounts().unwrap().len(), 1);
        assert_eq!(
            catalogued_mail_provider(&database, "imap@example.com").unwrap(),
            MailProviderKind::Imap
        );
    }

    #[test]
    fn an_account_not_set_up_on_this_device_stays_out_of_the_startup_registry() {
        let database = Database::open_memory();
        database.adopt_mail_account("work@example.com", MailProviderKind::Gmail).unwrap();
        database.adopt_mail_account("personal@example.com", MailProviderKind::Imap).unwrap();
        database.disconnect_account_locally("personal@example.com").unwrap();

        let credentials = startup_account_credentials(&database, &AuthConfig::google_for_test());
        let keys = credentials.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>();
        assert_eq!(keys, ["work@example.com"]);
        assert_eq!(database.list_accounts().unwrap().len(), 2, "it stays in the catalog");
        assert_eq!(database.primary_account_id(), "work@example.com");
    }

    // The companion to the above: an IMAP PRIMARY account no longer keeps
    // Gmail accounts out of the registry — but now, under the new contract,
    // the IMAP account starts alongside Gmail rather than being skipped.
    #[test]
    fn an_imap_primary_provider_starts_alongside_gmail() {
        let database = Database::open_memory();
        database.adopt_mail_account("imap@example.com", MailProviderKind::Imap).unwrap();
        database.adopt_mail_account("work@example.com", MailProviderKind::Gmail).unwrap();

        assert_eq!(database.primary_account_id(), "imap@example.com");
        let credentials = startup_account_credentials(&database, &AuthConfig::google_for_test());
        let mut started: Vec<(&str, MailProviderKind)> = credentials
            .iter()
            .map(|(key, auth)| (key.as_str(), auth.mail_provider()))
            .collect();
        started.sort_by(|a, b| a.0.cmp(b.0));
        assert_eq!(
            started,
            [
                ("imap@example.com", MailProviderKind::Imap),
                ("work@example.com", MailProviderKind::Gmail),
            ],
            "both the IMAP primary and the Gmail account start"
        );
        assert_eq!(database.list_accounts().unwrap().len(), 2);
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
    let mut fetch_errors = Vec::new();

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
        fetch_errors.extend(status.fetch_errors);
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
        fetch_errors,
    }
}

#[cfg(test)]
mod mail_sync_activity_event_tests {
    use super::MailSyncActivityEvent;

    #[test]
    fn serializes_the_shape_the_frontend_listens_for() {
        assert_eq!(
            serde_json::to_value(MailSyncActivityEvent { account_id: "a@example.com", active: true }).unwrap(),
            serde_json::json!({ "accountId": "a@example.com", "active": true })
        );
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
            fetch_errors: vec![],
        }
    }

    #[test]
    fn keeps_each_accounts_fetch_errors_when_merging() {
        let mut failing = status(None, None, 0, Some("timed out"));
        failing.fetch_errors = vec![crate::models::AccountSyncError {
            account_id: "second@example.com".into(),
            error: "timed out".into(),
        }];
        let merged = merge_sync_statuses(
            vec![
                ("first@example.com".into(), status(Some("2026-09-18T12:00:00Z"), None, 0, None)),
                ("second@example.com".into(), failing),
            ],
            vec![],
        );
        assert_eq!(merged.fetch_errors.len(), 1);
        assert_eq!(merged.fetch_errors[0].account_id, "second@example.com");
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
    // Accounts not set up on this device have nothing to sync here.
    let accounts = accounts.into_iter().filter(|account| account.on_this_device);
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
    let activity = app.state::<sync::SyncActivity>().inner().clone();
    let service = SyncService::new(database, auth.clone()).with_activity(activity);
    let polling_service = service.clone();
    let poll_task = tauri::async_runtime::spawn(async move {
        polling_service
            .polling_loop(sync_immediately, move |account_id: &str| {
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

/// Payload of the `mail-sync-activity` event.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct MailSyncActivityEvent<'a> {
    account_id: &'a str,
    active: bool,
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

/// Lets an in-flight mutate_thread IPC land in SQLite before flushing.
const PENDING_FLUSH_SETTLE_DELAY: std::time::Duration = std::time::Duration::from_millis(150);

fn spawn_pending_flush(handle: &tauri::AppHandle) {
    let handle = handle.clone();
    tauri::async_runtime::spawn(async move {
        let Some(service) = primary_service(&handle).await else {
            return;
        };
        tokio::time::sleep(PENDING_FLUSH_SETTLE_DELAY).await;
        log_failure("flushing pending mutations", service.flush_pending().await);
    });
}

fn spawn_foreground_sync(handle: &tauri::AppHandle) {
    let handle = handle.clone();
    tauri::async_runtime::spawn(async move {
        if let Some(state) = handle.try_state::<AppState>() {
            for service in sync_services(&state, None).await {
                service.reset_poll_backoff();
            }
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
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(app_update::PendingUpdate::default())
        .manage(mail_links::MailLinkInbox::default())
        .manage(calendar_files::CalendarFileInbox::default())
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
    // Managed before any account's sync service is built, since each one
    // reports to it.
    let activity_handle = app.handle().clone();
    app.manage(sync::SyncActivity::with_listener(move |account_id, active| {
        use tauri::Emitter;
        let _ = activity_handle.emit(
            "mail-sync-activity",
            MailSyncActivityEvent { account_id, active },
        );
    }));
    spawn_badge_loop(database.clone(), app.handle().clone());
    spawn_storage_maintenance(database.clone());
    let root = data_dir.join("attachments");
    std::fs::create_dir_all(&root)?;
    restrict_dir_to_owner(&root);
    let attachment_reader = attachment_reader::ReaderCache::new(root.join("reader"))?;
    let file_previews = file_preview::PreviewRoot::new(root.join("previews"))?;
    // Built before `manage` so the registry is never observed empty by a
    // command racing startup.
    let registry = startup_account_registry(&database, &auth_config, app.handle());
    let accounts: AccountRegistry = Arc::new(tokio::sync::Mutex::new(registry));
    let correspondence = correspondence::Correspondence {
        sending_identity: Arc::new(scheduled_send::keychain_identity),
        database: database.clone(),
        accounts: accounts.clone(),
        root,
        gate: Arc::new(tokio::sync::Mutex::new(())),
        edits: Arc::new(tokio::sync::Mutex::new(())),
        scheduling: Arc::new(tokio::sync::Mutex::new(())),
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
        file_previews,
        attachment_text: attachment_text::ExtractCache::default(),
        recovery,
        staged_import: transfer::StagedImport::default(),
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
const STORAGE_MAINTENANCE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(6 * 60 * 60);
/// Pause between storage backfill batches so they don't starve the database
/// mutex that normal sync and read operations also need.
const BACKFILL_BATCH_PAUSE: std::time::Duration = std::time::Duration::from_millis(200);

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
            tokio::time::sleep(BACKFILL_BATCH_PAUSE).await;
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
            tokio::time::sleep(BACKFILL_BATCH_PAUSE).await;
        }
        // Same for queued search rows and inbox previews, repairing extracted
        // HTML text and removing quoted history the thread already contains.
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
            tokio::time::sleep(BACKFILL_BATCH_PAUSE).await;
        }
        // Stay clear of the launch window, when the inbox loads and every
        // account runs its first sync; maintenance is never urgent.
        tokio::time::sleep(FIRST_STORAGE_MAINTENANCE_DELAY).await;
        loop {
            let prune_db = database.clone();
            let _ = tokio::task::spawn_blocking(move || run_storage_maintenance(&prune_db)).await;
            tokio::time::sleep(STORAGE_MAINTENANCE_INTERVAL).await;
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
/// id — or, before the first connect, the Gmail placeholder. Providers that
/// are not implemented or configured stay in the catalog without entering
/// the runtime registry, so they cannot block other accounts.
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
        // An account only available from another device has no credential
        // here; it enters the registry when the user sets it up.
        .filter(|account| account.on_this_device)
        .filter_map(|account| {
            let provider = MailProviderKind::parse(&account.provider).or_else(|| {
                log::warn!(
                    "skipping {}: unsupported mail provider {:?}",
                    account.email,
                    account.provider
                );
                None
            })?;
            // A recognized kind can be imported before its MailProvider
            // exists. Only implemented providers may enter the registry.
            // Keep this match exhaustive so each new kind needs a decision.
            match provider {
                MailProviderKind::Gmail => {}
                // Phase 2 Slice 5a: IMAP accounts now have a real provider, so
                // they enter the runtime registry and sync their INBOX like
                // Gmail. An account with no saved settings or password is
                // caught when its provider is built (it reports needs-reauth),
                // not here — so a half-set-up account still does not block
                // others.
                MailProviderKind::Imap => {}
            }
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
        commands::replicated_sync::synced_preferences,
        commands::replicated_sync::update_synced_preferences,
        commands::replicated_sync::replicated_sync_enabled,
        commands::replicated_sync::replicated_sync_status,
        commands::replicated_sync::replicated_sync_beta_enabled,
        commands::replicated_sync::replicated_sync_set_beta_enabled,
        commands::replicated_sync::replicated_sync_enrollment_status,
        commands::replicated_sync::replicated_sync_pending_requests,
        commands::replicated_sync::replicated_sync_device_roster,
        commands::replicated_sync::replicated_sync_begin_genesis,
        commands::replicated_sync::replicated_sync_inspect_space,
        commands::replicated_sync::replicated_sync_protocol_reset_notice,
        commands::replicated_sync::replicated_sync_dismiss_protocol_reset_notice,
        commands::replicated_sync::replicated_sync_request_enrollment,
        commands::replicated_sync::replicated_sync_approve_request,
        commands::replicated_sync::replicated_sync_reject_request,
        commands::replicated_sync::replicated_sync_confirm_enrollment,
        commands::replicated_sync::replicated_sync_rotate_epoch,
        commands::replicated_sync::replicated_sync_join_with_recovery_phrase,
        commands::replicated_sync::replicated_sync_set_device_label,
        commands::replicated_sync::replicated_sync_leave,
        commands::replicated_sync::replicated_sync_check_recovery_phrase,
        commands::replicated_sync::replicated_sync_add_folder,
        commands::replicated_sync::replicated_sync_add_ipfs_rpc,
        commands::replicated_sync::replicated_sync_probe_ipfs_rpc,
        commands::replicated_sync::replicated_sync_probe_s3,
        commands::replicated_sync::replicated_sync_create_join_code,
        commands::replicated_sync::replicated_sync_list_join_codes,
        commands::replicated_sync::replicated_sync_cancel_join_code,
        commands::replicated_sync::replicated_sync_preview_join_code,
        commands::replicated_sync::replicated_sync_pick_join_folder,
        commands::replicated_sync::replicated_sync_join_with_code,
        commands::replicated_sync::replicated_sync_join_code_notices,
        commands::replicated_sync::replicated_sync_dismiss_join_code_notice,
        commands::replicated_sync::replicated_sync_add_s3,
        commands::replicated_sync::replicated_sync_update_connector,
        commands::replicated_sync::replicated_sync_remove_transport,
        commands::replicated_sync::replicated_sync_now,
        commands::replicated_sync::replicated_sync_conflicts,
        commands::replicated_sync::replicated_sync_resolve_conflict,
        // App lifecycle
        commands::mail::correspondence_request,
        commands::mail::finish_exit,
        commands::sync::recovery_status,
        // Mailbox reading
        commands::mail::list_threads,
        commands::mail::list_all_mail,
        commands::mail::list_trash,
        commands::mail::list_threads_page,
        commands::mail::list_unread_counts,
        commands::mail::mailbox_unread_counts,
        commands::mail::list_all_mail_page,
        commands::mail::list_trash_page,
        commands::mail::list_sent_page,
        commands::mail::get_thread,
        commands::mail::search_threads,
        commands::mail::backfill_search_threads,
        // Images and attachments
        commands::attachments::fetch_remote_image,
        commands::attachments::fetch_attachment_image,
        commands::attachments::file_preview,
        commands::attachments::preview_calendar_attachment,
        commands::attachments::open_attachment,
        commands::attachments::save_attachment,
        // Triage, contacts, and unsubscribe
        commands::mail::mutate_thread,
        commands::mail::mutate_threads,
        commands::mail::record_triage_event,
        commands::mail::list_triage_sender_stats,
        commands::contacts::list_contact_suggestions,
        commands::contacts::list_contact_profiles,
        commands::contacts::preview_contact_import,
        commands::contacts::import_contacts,
        commands::contacts::export_contacts,
        commands::contacts::merge_contacts,
        commands::contacts::list_contact_suppressions,
        commands::contacts::set_contact_suppressed,
        commands::contacts::resolve_contact_ids,
        commands::contacts::get_contact_profile,
        commands::contacts::save_contact_profile,
        commands::contacts::delete_contact_profile,
        commands::contacts::list_contact_groups,
        commands::contacts::list_contact_group_recipients,
        commands::contacts::create_contact_group,
        commands::contacts::rename_contact_group,
        commands::contacts::add_contact_group_members,
        commands::contacts::remove_contact_group_members,
        commands::contacts::delete_contact_group,
        commands::contacts::list_keep_in_touch,
        commands::contacts::set_keep_in_touch,
        commands::contacts::snooze_keep_in_touch,
        commands::contacts::mark_contacted,
        commands::contacts::contact_timeline,
        commands::contacts::contact_activity,
        commands::contacts::sender_history,
        commands::contacts::message_trust,
        commands::contacts::sender_familiarity,
        commands::settings::list_sender_rules,
        commands::settings::mute_sender,
        commands::settings::delete_sender_rule,
        commands::contacts::contact_files,
        commands::attachments::list_files,
        commands::contacts::context_candidates,
        commands::attachments::set_file_star,
        commands::attachments::file_stars,
        commands::contacts::list_contact_tasks,
        commands::contacts::pin_contact,
        commands::contacts::unpin_contact,
        commands::mail::unsubscribe,
        // Mail sync
        commands::sync::sync_status,
        commands::mail::mail_sync_activity,
        commands::sync::sync_account,
        commands::sync::flush_pending_mutations,
        commands::sync::retry_failed_mutations,
        commands::sync::dismiss_sync_problems,
        // Mail accounts
        commands::accounts::google_auth_status,
        commands::accounts::connect_google,
        commands::accounts::disconnect_google,
        commands::accounts::list_accounts,
        commands::accounts::add_account,
        commands::accounts::discover_imap_settings,
        commands::accounts::probe_imap_certificate,
        commands::accounts::probe_smtp_certificate,
        commands::accounts::test_and_save_imap_account,
        commands::accounts::discover_imap_mailboxes,
        commands::accounts::commit_imap_mailbox_mapping,
        commands::accounts::finish_imap_account_setup,
        commands::accounts::get_imap_sending_options,
        commands::accounts::set_imap_identities,
        commands::accounts::set_imap_server_saves_sent,
        commands::accounts::reset_imap_sent_copy_detection,
        commands::accounts::get_imap_connection_settings,
        commands::accounts::update_imap_connection,
        commands::accounts::remove_account,
        commands::accounts::remove_synced_mail_account,
        commands::accounts::reconnect_account,
        commands::accounts::set_account_display_name,
        commands::accounts::set_account_nickname,
        commands::accounts::set_account_color,
        commands::accounts::reorder_accounts,
        // Calendar and scheduling
        commands::calendar::list_calendar_accounts,
        commands::calendar::add_calendar_account,
        commands::calendar::reconnect_calendar_account,
        commands::calendar::list_calendar_options,
        commands::calendar::create_calendar_event,
        commands::calendar::update_calendar_event,
        commands::calendar::delete_calendar_event,
        commands::calendar::set_calendar_selection,
        commands::calendar::remove_calendar_account,
        commands::calendar::remove_synced_calendar_account,
        commands::calendar::list_schedule_events,
        commands::calendar::update_calendar_response,
        commands::calendar::find_availability,
        commands::calendar::check_proposed_time,
        // Settings transfer and retention
        commands::settings::export_settings,
        commands::settings::preview_settings_import,
        commands::settings::apply_settings_import,
        commands::settings::cancel_settings_import,
        commands::settings::get_retention_days,
        commands::settings::set_retention_days,
        // Split inboxes, snippets, and labels
        commands::settings::list_split_inboxes,
        commands::settings::create_split_inbox,
        commands::settings::update_split_inbox,
        commands::settings::delete_split_inbox,
        commands::settings::reorder_split_inboxes,
        commands::mail::list_split_inbox_page,
        commands::settings::signature_library, commands::settings::save_signature, commands::settings::delete_signature, commands::settings::save_signature_defaults, commands::settings::delete_signature_defaults,
        gmail_signatures::list_gmail_signatures, gmail_signatures::import_gmail_signature,
        commands::settings::list_snippets,
        commands::settings::create_snippet,
        commands::settings::update_snippet,
        commands::settings::delete_snippet,
        commands::mail::list_labels,
        commands::mail::create_label,
        commands::mail::update_label,
        commands::mail::delete_label,
        // AI assistance
        commands::ai::ai_api_key_configured,
        commands::ai::set_ai_api_key,
        commands::ai::ai_test_connection,
        commands::ai::ai_summarize_thread,
        commands::ai::ai_analyze_thread,
        commands::ai::remove_thread_suggestion,
        commands::ai::ai_brief_thread,
        commands::ai::ai_usage_summary,
        commands::ai::ai_thread_chat,
        commands::ai::ai_enrich_contact,
        commands::ai::ai_reply_assist_context,
        commands::ai::ai_generate_reply,
        commands::ai::ai_review_draft,
        commands::ai::ai_find_times,
        // Tasks and follow-ups
        commands::tasks::list_tasks,
        commands::tasks::create_task,
        commands::tasks::update_task,
        commands::tasks::list_goals,
        commands::tasks::create_goal,
        commands::tasks::update_goal,
        commands::tasks::delete_goal,
        commands::tasks::set_task_status,
        commands::tasks::record_follow_up,
        commands::tasks::reconcile_tasks,
        // System
        system_fonts::list_system_font_families,
        app_update::check_for_app_update,
        app_update::install_app_update,
        mail_links::take_pending_mail_links,
        calendar_files::take_pending_calendar_files,
        commands::calendar::find_calendar_invitation,
        default_apps::default_app_status,
        default_apps::make_default_app,
    ]
}

/// Brings the main window forward for something opened from outside the app
/// (a mail link or calendar file).
pub(crate) fn focus_main_window<R: tauri::Runtime>(handle: &tauri::AppHandle<R>) {
    if let Some(window) = handle.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
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
        tauri::RunEvent::Resumed => {
            if let Some(state) = handle.try_state::<AppState>() {
                log_failure("recovering scheduled sends after wake", state.correspondence.resume_schedules());
            }
            spawn_foreground_sync(handle);
        },
        // macOS opens `mailto:` links and `.ics` files here when ThreeStrands
        // handles them (Info.plist declares both), including the one that
        // launched it.
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Opened { urls } => {
            for url in urls {
                match url.scheme() {
                    "mailto" => mail_links::deliver(handle, url.as_str()),
                    "file" => {
                        if let Ok(path) = url.to_file_path() {
                            calendar_files::deliver(handle, &path);
                        }
                    }
                    _ => {}
                }
            }
        }
        tauri::RunEvent::Exit => {
            if let Some(state) = handle.try_state::<AppState>() {
                log_failure("checkpointing the WAL at exit", state.database.checkpoint_on_exit());
            }
        }
        _ => {}
    }
}
