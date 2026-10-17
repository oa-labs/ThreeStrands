//! Mail accounts: Google sign-in, IMAP setup and settings, and removal.

use crate::auth::{AccountAuth, GoogleConnection, OAuthProvider};
use crate::db::Database;
use crate::models::{Account, AuthStatus, MailProviderKind, SyncStatus};
use tauri::State;

// Phase 2 Slice 2 account-setup surface (autodiscovery, cert probe,
// non-secret settings, test-and-save). The provider connection/sync types
// stay in `provider::imap`.
use crate::provider::imap;
use crate::provider::imap::ImapSession as _;

use crate::auth;
use crate::{
    authorize_interactively, catalogued_mail_provider, log_failure, record_synced_value,
    resolve_account, run_database_task, spawn_synced_account, AccountRegistry, AppState,
    ConnectedAccount,
};

fn record_mail_account(state: &State<'_, AppState>, account: &Account) -> Result<(), String> {
    record_synced_value(
        state,
        threestrands_sync_protocol::EntityType::MailAccount,
        &account.email.to_ascii_lowercase(),
        &serde_json::json!({
            "email": account.email,
            "displayName": account.display_name,
            "nickname": account.nickname,
            "color": account.color,
            "provider": account.provider,
            "sortOrder": account.sort_order,
        }),
        None,
    )
}

#[tauri::command]
pub(crate) async fn google_auth_status(state: State<'_, AppState>) -> Result<AuthStatus, String> {
    let primary = state.database.primary_account_id();
    let auth = resolve_account(&state, Some(&primary), |account| account.auth.clone()).await;
    Ok(AuthStatus {
        configured: auth.is_ok(),
        connected: auth.is_ok_and(|auth| auth.available()),
    })
}

#[tauri::command]
pub(crate) async fn connect_google(state: State<'_, AppState>) -> Result<SyncStatus, String> {
    let primary = state.database.primary_account_id();
    let (auth, service) = resolve_account(&state, Some(&primary), |account| {
        (account.auth.clone(), account.sync.clone())
    })
    .await
    .map_err(|_| OAuthProvider::Google.not_configured())?;
    authorize_interactively(&state, auth.require_oauth_credential()?).await?;
    // Rekeys the placeholder registry entry onto the real address as a side
    // effect, so the account is addressable by email from here on.
    log_failure(
        "refreshing account identity",
        state.correspondence.refresh_identity().await,
    );
    service.sync().await
}

#[tauri::command]
pub(crate) async fn disconnect_google(state: State<'_, AppState>, app: tauri::AppHandle) -> Result<(), String> {
    let primary = state.database.primary_account_id();
    remove_account(primary, state, app).await
}

#[tauri::command]
pub(crate) async fn list_accounts(state: State<'_, AppState>) -> Result<Vec<Account>, String> {
    let database = state.database.clone();
    run_database_task(move || database.list_accounts()).await
}

/// Signs in a new mail account through `provider`'s OAuth flow. Callers
/// that predate provider choice omit it and get Gmail, the only provider
/// they could have meant.
#[tauri::command]
pub(crate) async fn add_account(
    provider: Option<MailProviderKind>,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<Account, String> {
    let provider = provider.unwrap_or(MailProviderKind::Gmail);
    let auth = state.auth_config.pending_mail_account(provider)?;
    let email = authorize_interactively(&state, auth.require_oauth_credential()?).await?;
    let account = state.database.adopt_mail_account(&email, provider)?;
    let connected = spawn_synced_account(state.database.clone(), auth, true, app);
    state.accounts.lock().await.insert(email, connected);
    record_mail_account(&state, &account)?;
    Ok(account)
}

/// Autodiscover IMAP/SMTP settings for an email address. Returns `None` when
/// no HTTPS source answers, which the UI treats as "fall through to manual
/// setup" — a first-class path, since Proton Bridge and self-hosted servers
/// cannot be discovered from the email domain. The discovery fetches are
/// SSRF-filtered and HTTPS-only (see `provider::imap::discovery`); the final
/// user-chosen host is NOT filtered — that happens in `test_and_save_imap`.
#[tauri::command]
pub(crate) async fn discover_imap_settings(
    email: String,
) -> Result<Option<imap::DiscoveryResult>, String> {
    imap::discover(&email)
        .await
        .map_err(|error| error.to_string())
}

/// Probe a user-entered IMAP server's certificate WITHOUT logging in, for the
/// cert-trust step. Surfaces the leaf's subject / issuer / SHA-256 fingerprint
/// and whether it already chains to a public root (so no pin is needed). The
/// SSRF filter deliberately does NOT apply here: the host was entered by the
/// user, so a LAN / Bridge / `127.0.0.1` server must be reachable.
#[tauri::command]
pub(crate) async fn probe_imap_certificate(
    host: String,
    port: u16,
    security: imap::SecurityMode,
) -> Result<imap::CertificateProbe, String> {
    imap::probe_imap_certificate(&host, port, security)
        .await
        .map(|probe| probe) // CertificateProbe is Serialize
        .map_err(|error| imap::plain_language(&error))
}

#[tauri::command]
pub(crate) async fn probe_smtp_certificate(
    host: String,
    port: u16,
    security: imap::SecurityMode,
) -> Result<imap::CertificateProbe, String> {
    imap::probe_smtp_certificate(&host, port, security)
        .await
        .map_err(|error| imap::plain_language(&error))
}

/// The payload the IMAP setup form sends for "test and save".
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImapSetupRequest {
    email: String,
    imap_host: String,
    imap_port: u16,
    imap_security: imap::SecurityMode,
    imap_username: String,
    imap_password: String,
    smtp_host: String,
    smtp_port: u16,
    smtp_security: imap::SecurityMode,
    smtp_username: String,
    /// `None` when SMTP shares the IMAP password (the common case).
    smtp_password: Option<String>,
    /// The SHA-256 fingerprint the user chose to trust at the cert step, if
    /// the server was not already CA-trusted. Hex (any separators).
    imap_pinned_fingerprint: Option<String>,
    smtp_pinned_fingerprint: Option<String>,
    /// Whether this account stores user labels as keywords or label folders;
    /// decided from the server's PERMANENTFLAGS at the cert/test step.
    label_storage: imap::LabelStorage,
    /// Container mailbox for label-folder mode.
    label_container: Option<String>,
}

/// Test the entered IMAP + SMTP settings over the wire, and on success adopt
/// the account, persist its non-secret settings, and store the password in the
/// keychain. Nothing is written until BOTH the IMAP login and the SMTP AUTH
/// dry-run succeed, so a failed test leaves no half-made account.
///
/// SSRF note: the user-entered mail host is used directly here, deliberately
/// NOT through `net_safety`'s SSRF filter — a LAN server, a self-hosted box,
/// or Proton Bridge on `127.0.0.1` MUST connect (`docs/imap-design.md`,
/// "Account setup", final paragraph). The SSRF filter only guards the
/// autodiscovery HTTPS fetches, which are driven by the email domain.
#[tauri::command]
pub(crate) async fn test_and_save_imap_account(
    request: ImapSetupRequest,
    state: State<'_, AppState>,
) -> Result<Account, String> {
    let imap_pinned = match &request.imap_pinned_fingerprint {
        Some(hex) => Some(imap::parse_sha256_fingerprint(hex)?),
        None => None,
    };
    let smtp_pinned = match &request.smtp_pinned_fingerprint {
        Some(hex) => Some(imap::parse_sha256_fingerprint(hex)?),
        None => None,
    };
    let smtp_password = request
        .smtp_password
        .clone()
        .unwrap_or_else(|| request.imap_password.clone());

    // 1. Test IMAP: CAPABILITY / LOGIN / SELECT INBOX / LIST.
    let (capabilities, mailbox_count, supports_keywords) = imap::test_imap(
        &request.imap_host,
        request.imap_port,
        request.imap_security,
        imap_pinned,
        &request.imap_username,
        &request.imap_password,
    )
    .await
    .map_err(|error| imap::plain_language(&error))?;

    // 2. Test SMTP: EHLO / AUTH dry-run (no mail sent).
    imap::test_smtp(
        &request.smtp_host,
        request.smtp_port,
        request.smtp_security,
        smtp_pinned,
        &request.smtp_username,
        &smtp_password,
    )
    .await
    .map_err(|error| imap::plain_language(&error))?;

    let _report = imap::TestReport {
        imap_capabilities: capabilities,
        mailbox_count,
        supports_keywords,
        smtp_ok: true,
    };

    // 3. Persist only after both tests pass. Password -> keychain; non-secret
    //    settings -> the imap_account_settings table; account row adopted.
    // Store the password in the keychain under the account's address. The
    // IMAP setup path does NOT go through `require_oauth_credential`:
    // `AccountAuth::credential()` has been OAuth-optional since Phase 1 Slice 2.
    let auth = state
        .auth_config
        .mail_account(MailProviderKind::Imap, &request.email)?;
    let AccountAuth::Imap(credential) = &auth else {
        return Err("expected an IMAP credential for an IMAP account".to_string());
    };
    credential.save(&crate::credentials::ImapPassword {
        imap: request.imap_password.clone(),
        smtp: request.smtp_password.clone(),
    })?;

    // Adopt the account row and persist the settings. If the settings write
    // fails, roll back the credential so a retry starts clean.
    let adopt_result = save_tested_imap_settings(&state.database, &request, imap_pinned, smtp_pinned);
    let account = match adopt_result {
        Ok(account) => account,
        Err(error) => {
            let _ = credential.disconnect();
            return Err(error);
        }
    };

    // Polling starts when the mapping review finishes, so the first sync
    // uses the user's confirmed role destinations.

    record_mail_account(&state, &account)?;
    Ok(account)
}

// Persist first; finish_imap_account_setup activates the saved account after
// mapping review (including its skip path).
fn save_tested_imap_settings(
    database: &Database,
    request: &ImapSetupRequest,
    imap_pinned: Option<imap::Sha256Fingerprint>,
    smtp_pinned: Option<imap::Sha256Fingerprint>,
) -> Result<Account, String> {
    let mut pinned_fingerprints = std::collections::BTreeMap::new();
    if let Some(fp) = imap_pinned {
        pinned_fingerprints.insert(
            imap::host_port_key(&request.imap_host, request.imap_port),
            fp.to_hex(),
        );
    }
    if let Some(fp) = smtp_pinned {
        pinned_fingerprints.insert(
            imap::host_port_key(&request.smtp_host, request.smtp_port),
            fp.to_hex(),
        );
    }
    let settings = imap::ImapAccountSettings {
        imap_host: request.imap_host.clone(),
        imap_port: request.imap_port,
        imap_security: request.imap_security,
        imap_username: request.imap_username.clone(),
        smtp_host: request.smtp_host.clone(),
        smtp_port: request.smtp_port,
        smtp_security: request.smtp_security,
        smtp_username: request.smtp_username.clone(),
        mailbox_overrides: std::collections::BTreeMap::new(),
        archive_mailbox: None,
        label_storage: request.label_storage,
        label_container: request.label_container.clone(),
        identities: vec![imap::Identity {
            address: request.email.clone(),
            display_name: None,
        }],
        pinned_fingerprints,
        server_saves_sent: false,
    };

    let account = database.adopt_mail_account(&request.email, MailProviderKind::Imap)?;
    database.save_imap_account_settings(&request.email, &settings)?;
    Ok(account)
}

/// Discover a set-up IMAP account's mailboxes and propose a special-use
/// mapping (Phase 2 Slice 3). Reconnects with the account's stored, pinned
/// settings and keychain password, runs a plain `LIST`, EXAMINEs each
/// selectable mailbox read-only to persist the `imap_mailboxes` catalog, and
/// returns the proposed role → mailbox mapping for the user to confirm on the
/// mapping screen. Nothing in the settings row is written here — that is the
/// confirm step (`commit_imap_mailbox_mapping`). Read-only on the server.
#[tauri::command]
pub(crate) async fn discover_imap_mailboxes(
    email: String,
    state: State<'_, AppState>,
) -> Result<imap::MailboxMapping, String> {
    let settings = state
        .database
        .imap_account_settings(&email)?
        .ok_or_else(|| "This account has no saved IMAP settings.".to_string())?;
    // The password lives only in the keychain.
    let password = crate::auth::ImapCredential::for_account(&email)
        .load()?
        .imap;
    let mut session = imap::connect_with_settings(&settings, &password)
        .await
        .map_err(|error| imap::plain_language(&error))?;
    let store = imap::ImapStateStore::new(state.database.clone(), email.clone());
    let mapping = imap::discover_and_persist(&mut session, &store)
        .await
        .map_err(|error| imap::plain_language(&error));
    let _ = session.logout().await;
    mapping
}

/// The confirmed/edited mapping the user committed on the mapping screen.
/// `archive` names the Archive mailbox (or `createArchive` requests a new one);
/// `mailboxOverrides` carries the other confirmed role → mailbox choices by
/// role key (`sent`, `trash`, …); `labelContainer` updates the label-folder
/// container if the user changed it.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImapMailboxMappingCommit {
    email: String,
    /// The chosen Archive mailbox name, if an existing one was selected.
    archive: Option<String>,
    /// Create a new Archive mailbox with this name instead of choosing one.
    create_archive: Option<String>,
    /// Confirmed role → mailbox-name choices, keyed by `MailboxRole::key`.
    #[serde(default)]
    mailbox_overrides: std::collections::BTreeMap<String, String>,
    /// Updated label-folder container mailbox, if changed.
    label_container: Option<String>,
}

impl ImapMailboxMappingCommit {
    fn archive_choice(&self) -> Result<&str, String> {
        self.create_archive
            .as_deref()
            .filter(|name| !name.is_empty())
            .or(self.archive.as_deref().filter(|name| !name.is_empty()))
            .ok_or_else(|| "Choose an Archive mailbox or create one.".to_string())
    }
}

fn save_imap_mailbox_mapping(
    database: &Database,
    mut settings: imap::ImapAccountSettings,
    commit: ImapMailboxMappingCommit,
) -> Result<(), String> {
    // Mailbox names are opaque server identifiers. Only the empty string is
    // a cleared choice; whitespace can be part of an existing mailbox name.
    settings.archive_mailbox = Some(commit.archive_choice()?.to_string());
    for (role_key, mailbox) in commit.mailbox_overrides {
        if mailbox.is_empty() {
            settings.mailbox_overrides.remove(&role_key);
        } else {
            settings.mailbox_overrides.insert(role_key, mailbox);
        }
    }
    settings.label_container = commit.label_container.filter(|name| !name.is_empty());
    database.save_imap_account_settings(&commit.email, &settings)?;
    Ok(())
}

/// Persist the user's confirmed mailbox mapping into the account's settings
/// row (Phase 2 Slice 3): the Archive choice, the per-role overrides, and the
/// label-folder container. When the user asked to create a new Archive
/// mailbox, this is the ONE place Slice 3 creates a mailbox — a `CREATE` with
/// the design's `TRYCREATE` create-then-retry tolerance — before recording it
/// as the Archive override.
#[tauri::command]
pub(crate) async fn commit_imap_mailbox_mapping(
    commit: ImapMailboxMappingCommit,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let settings = state
        .database
        .imap_account_settings(&commit.email)?
        .ok_or_else(|| "This account has no saved IMAP settings.".to_string())?;

    // Resolve the Archive mailbox: an explicit create wins (and actually
    // creates it on the server), otherwise the chosen existing mailbox.
    let archive = commit.archive_choice()?;
    if commit
        .create_archive
        .as_deref()
        .is_some_and(|name| !name.is_empty())
    {
        let password = crate::auth::ImapCredential::for_account(&commit.email)
            .load()?
            .imap;
        let mut session = imap::connect_with_settings(&settings, &password)
            .await
            .map_err(|error| imap::plain_language(&error))?;
        let created = imap::create_mailbox_tolerant(&mut session, archive)
            .await
            .map_err(|error| imap::plain_language(&error));
        let _ = session.logout().await;
        created?;
    }

    save_imap_mailbox_mapping(&state.database, settings, commit)
}

/// Make a newly saved IMAP account usable without restarting the app. Both
/// mapping confirmation and skipping review finish through this command.
#[tauri::command]
pub(crate) async fn finish_imap_account_setup(
    email: String,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let provider = catalogued_mail_provider(&state.database, &email)?;
    if provider != MailProviderKind::Imap {
        return Err("This account is not an IMAP account.".into());
    }
    if state.database.imap_account_settings(&email)?.is_none() {
        return Err("This account has no saved IMAP settings.".into());
    }
    let auth = state.auth_config.mail_account(provider, &email)?;
    register_finished_imap_account(&state.accounts, email, || {
        spawn_synced_account(state.database.clone(), auth, true, app)
    })
    .await;
    Ok(())
}

async fn register_finished_imap_account(
    accounts: &AccountRegistry,
    email: String,
    start: impl FnOnce() -> ConnectedAccount,
) {
    // A retry must not leave two pollers running. Restart an existing service
    // as well: an account added before this run may hold older role settings.
    let mut accounts = accounts.lock().await;
    if let Some(previous) = accounts.remove(&email) {
        previous.poll_task.abort();
    }
    accounts.insert(email, start());
}

/// The account's sending-related options for the Settings "Sending" panel
/// (IMAP accounts only). `identities` is the ordered send-as list WITH display
/// names (login first); `serverSavesSentOverride` is the exported user override
/// (`imap_account_settings.server_saves_sent`, true = never save a copy);
/// `sentCopyDetected` is this device's LOCAL detection result (`true` the
/// server saves its own copy, `false` it does not, `null` not yet decided).
/// No secret ever appears here.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImapSendingOptions {
    identities: Vec<crate::provider::SendingIdentity>,
    server_saves_sent_override: bool,
    sent_copy_detected: Option<bool>,
}

/// One submitted sending-identity row from the Settings edit form.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SendingIdentityInput {
    address: String,
    #[serde(default)]
    display_name: Option<String>,
}

/// Load the IMAP settings for `account`, erroring clearly when the account is
/// not an IMAP account (no settings row) — shared by all four sending-option
/// commands so the "IMAP only" refusal is spelled once. The password is never
/// touched; only the non-secret settings row is read.
fn imap_settings_or_reject(
    database: &Database,
    account: &str,
) -> Result<imap::ImapAccountSettings, String> {
    database
        .imap_account_settings(account)?
        .ok_or_else(|| format!("{account} is not an IMAP account, so it has no sending settings."))
}

/// Read an IMAP account's sending options for the Settings panel. Refuses a
/// non-IMAP or unknown account (no settings row). Offline-safe.
#[tauri::command(async)]
pub(crate) fn get_imap_sending_options(
    account: String,
    state: State<'_, AppState>,
) -> Result<ImapSendingOptions, String> {
    let settings = imap_settings_or_reject(&state.database, &account)?;
    let identities = state.database.account_sending_identities(&account)?;
    let store = imap::ImapStateStore::new(state.database.clone(), account.clone());
    let sent_copy_detected = store.sent_copy_detection()?;
    Ok(ImapSendingOptions {
        identities,
        server_saves_sent_override: settings.server_saves_sent,
        sent_copy_detected,
    })
}

/// Replace an IMAP account's sending identities. Validates STRICTLY (unlike
/// the lenient read builder): each address must pass envelope validation,
/// display names reject CR/LF/NUL and are length-capped, duplicates are
/// rejected, the login address stays first and cannot be removed, and the list
/// is capped at `IMAP_MAX_IDENTITIES`. Persists ONLY the identities (every
/// other settings column is round-tripped unchanged). Hostile input is
/// rejected with ZERO writes. Returns the saved identities (login first, with
/// display names). Refuses a non-IMAP/unknown account.
#[tauri::command(async)]
pub(crate) fn set_imap_identities(
    account: String,
    identities: Vec<SendingIdentityInput>,
    state: State<'_, AppState>,
) -> Result<Vec<crate::provider::SendingIdentity>, String> {
    let mut settings = imap_settings_or_reject(&state.database, &account)?;
    let submitted: Vec<imap::Identity> = identities
        .into_iter()
        .map(|input| imap::Identity {
            address: input.address,
            display_name: input.display_name,
        })
        .collect();
    // Validate first; nothing is written unless this returns Ok. The login/
    // account address anchors the list and cannot be removed or renamed away.
    let validated = imap::validate_identity_edit(&account, &submitted)?;
    settings.identities = validated;
    state.database.save_imap_account_settings(&account, &settings)?;
    Ok(state.database.account_sending_identities(&account)?)
}

/// Set the "server saves its own sent mail" user override only (true = never
/// save a copy). Writes just that column; everything else is round-tripped.
/// Refuses a non-IMAP/unknown account.
#[tauri::command(async)]
pub(crate) fn set_imap_server_saves_sent(
    account: String,
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut settings = imap_settings_or_reject(&state.database, &account)?;
    settings.server_saves_sent = enabled;
    state.database.save_imap_account_settings(&account, &settings)?;
    Ok(())
}

/// Clear this device's local Sent-copy detection result, so the next send
/// re-detects from scratch ("Check again"). Refuses a non-IMAP/unknown
/// account. Confirm-free: undoable by nature (the next send re-detects).
#[tauri::command(async)]
pub(crate) fn reset_imap_sent_copy_detection(
    account: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    // Reject a non-IMAP/unknown account before touching local state.
    imap_settings_or_reject(&state.database, &account)?;
    let store = imap::ImapStateStore::new(state.database.clone(), account);
    store.clear_sent_copy_detection()?;
    Ok(())
}

/// An IMAP account's non-secret connection settings for the Settings
/// "Server settings" panel. No password or fingerprint value is returned;
/// only whether each endpoint has a pinned certificate.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImapConnectionSettings {
    imap_host: String,
    imap_port: u16,
    imap_security: imap::SecurityMode,
    imap_username: String,
    smtp_host: String,
    smtp_port: u16,
    smtp_security: imap::SecurityMode,
    smtp_username: String,
    imap_certificate_pinned: bool,
    smtp_certificate_pinned: bool,
}

impl From<&imap::ImapAccountSettings> for ImapConnectionSettings {
    fn from(settings: &imap::ImapAccountSettings) -> Self {
        Self {
            imap_host: settings.imap_host.clone(),
            imap_port: settings.imap_port,
            imap_security: settings.imap_security,
            imap_username: settings.imap_username.clone(),
            smtp_host: settings.smtp_host.clone(),
            smtp_port: settings.smtp_port,
            smtp_security: settings.smtp_security,
            smtp_username: settings.smtp_username.clone(),
            imap_certificate_pinned: settings
                .pinned_for(&settings.imap_host, settings.imap_port)
                .is_some(),
            smtp_certificate_pinned: settings
                .pinned_for(&settings.smtp_host, settings.smtp_port)
                .is_some(),
        }
    }
}

/// Read an IMAP account's server settings. Refuses a non-IMAP or unknown
/// account. Offline-safe; never touches the keychain.
#[tauri::command(async)]
pub(crate) fn get_imap_connection_settings(
    account: String,
    state: State<'_, AppState>,
) -> Result<ImapConnectionSettings, String> {
    let settings = imap_settings_or_reject(&state.database, &account)?;
    Ok(ImapConnectionSettings::from(&settings))
}

/// The edited server settings for an existing IMAP account.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImapConnectionUpdate {
    email: String,
    imap_host: String,
    imap_port: u16,
    imap_security: imap::SecurityMode,
    imap_username: String,
    smtp_host: String,
    smtp_port: u16,
    smtp_security: imap::SecurityMode,
    smtp_username: String,
    /// A new password for IMAP and SMTP; `None` or empty keeps the stored one.
    #[serde(default)]
    password: Option<String>,
    /// A newly trusted certificate fingerprint; `None` keeps any existing pin
    /// for the same `host:port`.
    #[serde(default)]
    imap_pinned_fingerprint: Option<String>,
    #[serde(default)]
    smtp_pinned_fingerprint: Option<String>,
}

/// The pin to test and save for one endpoint: a newly trusted certificate
/// wins, otherwise the pin already recorded for the same `host:port`.
fn resolve_connection_pin(
    settings: &imap::ImapAccountSettings,
    host: &str,
    port: u16,
    trusted: Option<&str>,
) -> Result<Option<imap::Sha256Fingerprint>, String> {
    match trusted.filter(|hex| !hex.is_empty()) {
        Some(hex) => Ok(Some(imap::parse_sha256_fingerprint(hex)?)),
        None => Ok(settings.pinned_for(host, port)),
    }
}

/// Apply a tested connection update to the stored settings. Only the server
/// fields and certificate pins change: identities, the mailbox mapping, label
/// storage and the sent-copy override are kept. Pins for endpoints no longer
/// in use are dropped.
fn merge_imap_connection_update(
    mut settings: imap::ImapAccountSettings,
    update: &ImapConnectionUpdate,
    imap_pin: Option<imap::Sha256Fingerprint>,
    smtp_pin: Option<imap::Sha256Fingerprint>,
) -> imap::ImapAccountSettings {
    settings.imap_host = update.imap_host.clone();
    settings.imap_port = update.imap_port;
    settings.imap_security = update.imap_security;
    settings.imap_username = update.imap_username.clone();
    settings.smtp_host = update.smtp_host.clone();
    settings.smtp_port = update.smtp_port;
    settings.smtp_security = update.smtp_security;
    settings.smtp_username = update.smtp_username.clone();
    settings.pinned_fingerprints.clear();
    if let Some(pin) = imap_pin {
        settings.pinned_fingerprints.insert(
            imap::host_port_key(&update.imap_host, update.imap_port),
            pin.to_hex(),
        );
    }
    if let Some(pin) = smtp_pin {
        settings.pinned_fingerprints.insert(
            imap::host_port_key(&update.smtp_host, update.smtp_port),
            pin.to_hex(),
        );
    }
    settings
}

/// Change an existing IMAP account's server settings or password, or
/// reconnect one that needs it. Like setup, nothing is written until both the
/// IMAP login and the SMTP AUTH dry-run succeed. On success the account is
/// marked connected and its sync service restarts with the new settings.
#[tauri::command]
pub(crate) async fn update_imap_connection(
    update: ImapConnectionUpdate,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<Account, String> {
    let email = update.email.clone();
    let previous = imap_settings_or_reject(&state.database, &email)?;
    let imap_pin = resolve_connection_pin(
        &previous,
        &update.imap_host,
        update.imap_port,
        update.imap_pinned_fingerprint.as_deref(),
    )?;
    let smtp_pin = resolve_connection_pin(
        &previous,
        &update.smtp_host,
        update.smtp_port,
        update.smtp_pinned_fingerprint.as_deref(),
    )?;
    let new_password = update.password.clone().filter(|password| !password.is_empty());
    let credential = crate::auth::ImapCredential::for_account(&email);
    let saved_password = credential.load().ok();
    let password = match &new_password {
        Some(password) => crate::credentials::ImapPassword {
            imap: password.clone(),
            smtp: None,
        },
        None => saved_password
            .clone()
            .ok_or_else(|| "Enter your password to reconnect this account.".to_string())?,
    };
    let smtp_password = password.smtp.clone().unwrap_or_else(|| password.imap.clone());

    imap::test_imap(
        &update.imap_host,
        update.imap_port,
        update.imap_security,
        imap_pin,
        &update.imap_username,
        &password.imap,
    )
    .await
    .map_err(|error| imap::plain_language(&error))?;
    imap::test_smtp(
        &update.smtp_host,
        update.smtp_port,
        update.smtp_security,
        smtp_pin,
        &update.smtp_username,
        &smtp_password,
    )
    .await
    .map_err(|error| imap::plain_language(&error))?;

    let settings = merge_imap_connection_update(previous.clone(), &update, imap_pin, smtp_pin);
    let auth = state
        .auth_config
        .mail_account(MailProviderKind::Imap, &email)?;
    let _guard = state.correspondence.gate.lock().await;
    let service = state
        .accounts
        .lock()
        .await
        .get(&email)
        .map(|connected| connected.sync.clone());
    if let Some(service) = &service {
        service.retire().await;
    }
    let persisted = (|| -> Result<Account, String> {
        // Write credentials first: a keychain failure must not invalidate the
        // old server's cache. Roll back credentials if the DB transaction fails.
        if new_password.is_some() {
            credential.save(&password)?;
        }
        if let Err(error) = state
            .database
            .save_imap_connection_settings(&email, &settings)
        {
            if new_password.is_some() {
                match &saved_password {
                    Some(saved) => {
                        let _ = credential.save(saved);
                    }
                    None => {
                        let _ = credential.disconnect();
                    }
                }
            }
            return Err(error.to_string());
        }
        Ok(state
            .database
            .adopt_mail_account(&email, MailProviderKind::Imap)?)
    })();
    let account = match persisted {
        Ok(account) => account,
        Err(error) => {
            if let Some(service) = &service {
                service.resume();
            }
            return Err(error);
        }
    };
    register_finished_imap_account(&state.accounts, email, || {
        spawn_synced_account(state.database.clone(), auth, true, app)
    })
    .await;
    record_mail_account(&state, &account)?;
    Ok(account)
}

#[tauri::command]
pub(crate) async fn remove_account(
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
    let provider = catalogued_mail_provider(&state.database, &email)?;
    let fallback = state.auth_config.mail_account(provider, &email)?;
    let removes_everywhere = remove_catalog || !state.database.cross_device_sync_enrolled()?;
    // Sign out at Google before anything else changes, so a failed
    // revocation leaves the account intact and retryable instead of
    // discarding a refresh token that still works. Removing it from this
    // device only keeps the grant other devices rely on.
    if removes_everywhere && provider == MailProviderKind::Gmail {
        state
            .auth_config
            .revoke_google_grant(&email, GoogleConnection::Mail)
            .await?;
    }
    state.correspondence.pause_account_schedules(&email)?;
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
pub(super) const NOT_SYNC_ENROLLED: &str = "Turn on cross-device sync and finish enrolling this device before removing an account everywhere";

#[tauri::command]
pub(crate) async fn remove_synced_mail_account(
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
    for rule in state.database.list_sender_rules()?.into_iter().filter(|rule| rule.account_id.eq_ignore_ascii_case(&email)) {
        state.database.record_local_entity_deletion(threestrands_sync_protocol::EntityType::SenderRule, &rule.id)?;
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
pub(crate) async fn reconnect_account(
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
    authorize_interactively(&state, auth.require_oauth_credential()?).await?;
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
pub(crate) fn set_account_color(
    email: String,
    color: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.database.set_account_color(&email, &color)?;
    let account = state.database.get_account(&email)?.ok_or_else(|| "Account not found".to_string())?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::MailAccount, &email.to_ascii_lowercase(),
        &serde_json::json!({"email":account.email,"displayName":account.display_name,"nickname":account.nickname,"color":account.color,"provider":account.provider,"sortOrder":account.sort_order}),
        Some(std::collections::BTreeSet::from(["color".to_string()])))
}

#[tauri::command(async)]
pub(crate) fn set_account_display_name(
    email: String,
    display_name: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .database
        .set_account_display_name(&email, display_name.as_deref())?;
    let account = state.database.get_account(&email)?.ok_or_else(|| "Account not found".to_string())?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::MailAccount, &email.to_ascii_lowercase(),
        &serde_json::json!({"email":account.email,"displayName":account.display_name,"nickname":account.nickname,"color":account.color,"provider":account.provider,"sortOrder":account.sort_order}),
        Some(std::collections::BTreeSet::from(["displayName".to_string()])))
}

#[tauri::command(async)]
pub(crate) fn set_account_nickname(
    email: String,
    nickname: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.database.set_account_nickname(&email, nickname.as_deref())?;
    let account = state.database.get_account(&email)?.ok_or_else(|| "Account not found".to_string())?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::MailAccount, &email.to_ascii_lowercase(),
        &serde_json::json!({"email":account.email,"displayName":account.display_name,"nickname":account.nickname,"color":account.color,"provider":account.provider,"sortOrder":account.sort_order}),
        Some(std::collections::BTreeSet::from(["nickname".to_string()])))
}

#[tauri::command(async)]
pub(crate) fn reorder_accounts(emails: Vec<String>, state: State<'_, AppState>) -> Result<(), String> {
    state.database.reorder_accounts(&emails)?;
    for account in state.database.list_accounts()? {
        record_synced_value(&state, threestrands_sync_protocol::EntityType::MailAccount, &account.email.to_ascii_lowercase(),
            &serde_json::json!({"email":account.email,"displayName":account.display_name,"nickname":account.nickname,"color":account.color,"provider":account.provider,"sortOrder":account.sort_order}),
            Some(std::collections::BTreeSet::from(["sortOrder".to_string()])))?;
    }
    Ok(())
}

#[cfg(test)]
mod imap_setup_tests {
    use crate::{auth::AuthConfig, db::Database, models::MailProviderKind, startup_account_credentials};

    fn imap_setup_request() -> super::ImapSetupRequest {
        serde_json::from_value(serde_json::json!({
            "email": "imap@example.com", "imapHost": "incoming.example.com", "imapPort": 993,
            "imapSecurity": "implicit_tls", "imapUsername": "incoming", "imapPassword": "secret",
            "smtpHost": "outgoing.example.com", "smtpPort": 587, "smtpSecurity": "start_tls",
            "smtpUsername": "outgoing", "smtpPassword": null,
            "imapPinnedFingerprint": null, "smtpPinnedFingerprint": null,
            "labelStorage": "folders", "labelContainer": "Labels"
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn finishing_imap_setup_registers_live_sync_and_replaces_the_previous_poller() {
        use std::sync::Arc;
        let database = Arc::new(Database::open_memory());
        let request = imap_setup_request();
        super::save_tested_imap_settings(&database, &request, None, None).unwrap();
        let accounts: super::AccountRegistry = Arc::default();
        let (cancelled_tx, cancelled_rx) = tokio::sync::oneshot::channel::<()>();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let make_account = |task| {
            let auth = AuthConfig::default()
                .mail_account(MailProviderKind::Imap, &request.email)
                .unwrap();
            super::ConnectedAccount {
                sync: crate::sync::SyncService::new(database.clone(), auth.clone()),
                auth,
                poll_task: task,
            }
        };
        let task = tauri::async_runtime::spawn(async move {
            let _guard = cancelled_tx;
            started_tx.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        super::register_finished_imap_account(&accounts, request.email.clone(), || {
            make_account(task)
        })
        .await;
        started_rx.await.unwrap();
        {
            let accounts = accounts.lock().await;
            let account = accounts
                .get(&request.email)
                .expect("available immediately without restart");
            assert_eq!(account.auth.mail_provider(), MailProviderKind::Imap);
            assert_eq!(account.auth.key(), request.email);
        }
        super::register_finished_imap_account(&accounts, request.email.clone(), || {
            make_account(tauri::async_runtime::spawn(async {}))
        })
        .await;
        tokio::time::timeout(std::time::Duration::from_secs(2), cancelled_rx)
            .await
            .expect("previous poller must stop")
            .unwrap_err();
        assert_eq!(accounts.lock().await.len(), 1);
    }

    #[test]
    fn tested_imap_setup_saves_independent_pins_and_now_registers_the_account() {
        let database = Database::open_memory();
        let request = imap_setup_request();
        let incoming = super::imap::Sha256Fingerprint::from_bytes([1; 32]);
        let outgoing = super::imap::Sha256Fingerprint::from_bytes([2; 32]);
        let account =
            super::save_tested_imap_settings(&database, &request, Some(incoming), Some(outgoing))
                .unwrap();
        assert_eq!(account.provider, "imap");
        let saved = database
            .imap_account_settings(&request.email)
            .unwrap()
            .unwrap();
        assert_eq!(
            saved.pinned_fingerprints["incoming.example.com:993"],
            incoming.to_hex()
        );
        assert_eq!(
            saved.pinned_fingerprints["outgoing.example.com:587"],
            outgoing.to_hex()
        );
        // Phase 2 Slice 5a INTENTIONALLY changes the previous contract: a
        // set-up IMAP account now ENTERS the startup registry (it has a real
        // provider and syncs), rather than being skipped. Its password is in
        // the keychain from "test and save", so it starts ready to sync.
        let credentials =
            startup_account_credentials(&database, &AuthConfig::google_for_test());
        let keys: Vec<_> = credentials.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(keys, ["imap@example.com"]);
        assert_eq!(credentials[0].1.mail_provider(), MailProviderKind::Imap);

        // A CA-trusted SMTP endpoint has no pin even when IMAP needs one.
        super::save_tested_imap_settings(&database, &request, Some(incoming), None).unwrap();
        let saved = database
            .imap_account_settings(&request.email)
            .unwrap()
            .unwrap();
        assert_eq!(saved.pinned_fingerprints.len(), 1);
        assert!(!saved
            .pinned_fingerprints
            .contains_key("outgoing.example.com:587"));
        assert_eq!(database.list_accounts().unwrap().len(), 1);
    }

    #[test]
    fn confirmed_mailbox_mapping_preserves_identifiers_in_saved_settings() {
        let database = Database::open_memory();
        let request = imap_setup_request();
        super::save_tested_imap_settings(&database, &request, None, None).unwrap();
        let settings = database
            .imap_account_settings(&request.email)
            .unwrap()
            .unwrap();
        let commit: super::ImapMailboxMappingCommit = serde_json::from_value(serde_json::json!({
            "email": request.email, "archive": " Archive ", "createArchive": null,
            "mailboxOverrides": { "sent": "Sent ", "junk": " Junk", "all": " " },
            "labelContainer": " Labels "
        }))
        .unwrap();
        super::save_imap_mailbox_mapping(&database, settings, commit).unwrap();
        let settings = database
            .imap_account_settings(&request.email)
            .unwrap()
            .unwrap();
        assert_eq!(settings.archive_mailbox.as_deref(), Some(" Archive "));
        assert_eq!(settings.mailbox_overrides["sent"], "Sent ");
        assert_eq!(settings.mailbox_overrides["junk"], " Junk");
        assert_eq!(settings.mailbox_overrides["all"], " ");
        assert_eq!(settings.label_container.as_deref(), Some(" Labels "));

        let commit: super::ImapMailboxMappingCommit = serde_json::from_value(serde_json::json!({
            "email": request.email, "archive": null, "createArchive": " New Archive ",
            "mailboxOverrides": { "sent": "" }, "labelContainer": null
        }))
        .unwrap();
        assert_eq!(commit.archive_choice().unwrap(), " New Archive ");
        super::save_imap_mailbox_mapping(&database, settings, commit).unwrap();
        let settings = database
            .imap_account_settings(&request.email)
            .unwrap()
            .unwrap();
        assert_eq!(settings.archive_mailbox.as_deref(), Some(" New Archive "));
        assert!(!settings.mailbox_overrides.contains_key("sent"));
        assert_eq!(settings.mailbox_overrides["junk"], " Junk");
        assert_eq!(settings.label_container, None);
    }

    fn connection_update(json: serde_json::Value) -> super::ImapConnectionUpdate {
        let mut base = serde_json::json!({
            "email": "imap@example.com", "imapHost": "incoming.example.com", "imapPort": 993,
            "imapSecurity": "implicit_tls", "imapUsername": "incoming",
            "smtpHost": "outgoing.example.com", "smtpPort": 587, "smtpSecurity": "start_tls",
            "smtpUsername": "outgoing"
        });
        base.as_object_mut().unwrap().extend(json.as_object().unwrap().clone());
        serde_json::from_value(base).unwrap()
    }

    #[test]
    fn connection_update_changes_only_server_fields_and_keeps_account_choices() {
        let database = Database::open_memory();
        let request = imap_setup_request();
        let incoming = super::imap::Sha256Fingerprint::from_bytes([1; 32]);
        super::save_tested_imap_settings(&database, &request, Some(incoming), None).unwrap();
        let mut before = database.imap_account_settings(&request.email).unwrap().unwrap();
        before.archive_mailbox = Some("Archive".into());
        before.server_saves_sent = true;
        before.identities.push(super::imap::Identity {
            address: "alias@example.com".into(),
            display_name: Some("Alias".into()),
        });

        let update = connection_update(serde_json::json!({
            "smtpHost": "relay.example.com", "smtpPort": 465, "smtpSecurity": "implicit_tls",
            "smtpUsername": "relay", "password": null
        }));
        let imap_pin = super::resolve_connection_pin(&before, &update.imap_host, update.imap_port, None).unwrap();
        let smtp_pin = super::resolve_connection_pin(&before, &update.smtp_host, update.smtp_port, None).unwrap();
        // The unchanged incoming endpoint keeps its pin; the new SMTP host has none.
        assert_eq!(imap_pin, Some(incoming));
        assert_eq!(smtp_pin, None);

        let merged = super::merge_imap_connection_update(before.clone(), &update, imap_pin, smtp_pin);
        assert_eq!(merged.smtp_host, "relay.example.com");
        assert_eq!(merged.smtp_port, 465);
        assert_eq!(merged.smtp_username, "relay");
        assert_eq!(merged.imap_host, before.imap_host);
        assert_eq!(merged.identities, before.identities);
        assert_eq!(merged.archive_mailbox, before.archive_mailbox);
        assert_eq!(merged.label_storage, before.label_storage);
        assert_eq!(merged.label_container, before.label_container);
        assert!(merged.server_saves_sent);
        assert_eq!(merged.pinned_fingerprints.len(), 1);
        assert_eq!(merged.pinned_fingerprints["incoming.example.com:993"], incoming.to_hex());
    }

    #[test]
    fn connection_update_replaces_a_pin_only_when_a_new_certificate_is_trusted() {
        let database = Database::open_memory();
        let request = imap_setup_request();
        let old = super::imap::Sha256Fingerprint::from_bytes([1; 32]);
        let new = super::imap::Sha256Fingerprint::from_bytes([3; 32]);
        super::save_tested_imap_settings(&database, &request, Some(old), None).unwrap();
        let settings = database.imap_account_settings(&request.email).unwrap().unwrap();

        let trusted = super::resolve_connection_pin(&settings, "incoming.example.com", 993, Some(&new.to_hex())).unwrap();
        assert_eq!(trusted, Some(new));
        // An empty value is "nothing newly trusted", not a cleared pin.
        let kept = super::resolve_connection_pin(&settings, "incoming.example.com", 993, Some("")).unwrap();
        assert_eq!(kept, Some(old));
        // A pin never follows the user to a different port or host.
        assert_eq!(super::resolve_connection_pin(&settings, "incoming.example.com", 143, None).unwrap(), None);
        assert_eq!(super::resolve_connection_pin(&settings, "other.example.com", 993, None).unwrap(), None);
        assert!(super::resolve_connection_pin(&settings, "incoming.example.com", 993, Some("not hex")).is_err());

        // Moving the incoming port drops the pin for the old endpoint.
        let update = connection_update(serde_json::json!({ "imapPort": 1143 }));
        let merged = super::merge_imap_connection_update(settings, &update, None, None);
        assert!(merged.pinned_fingerprints.is_empty());
    }

    #[test]
    fn connection_settings_never_carry_a_password_or_fingerprint_value() {
        let database = Database::open_memory();
        let request = imap_setup_request();
        let incoming = super::imap::Sha256Fingerprint::from_bytes([1; 32]);
        super::save_tested_imap_settings(&database, &request, Some(incoming), None).unwrap();
        let settings = database.imap_account_settings(&request.email).unwrap().unwrap();
        let json = serde_json::to_value(super::ImapConnectionSettings::from(&settings)).unwrap();
        assert_eq!(json["imapHost"], "incoming.example.com");
        assert_eq!(json["imapSecurity"], "implicit_tls");
        assert_eq!(json["smtpUsername"], "outgoing");
        assert_eq!(json["imapCertificatePinned"], true);
        assert_eq!(json["smtpCertificatePinned"], false);
        let text = json.to_string();
        assert!(!text.contains("secret"));
        assert!(!text.contains(&incoming.to_hex()));
    }

    #[test]
    fn confirming_without_an_archive_choice_leaves_settings_unchanged() {
        let database = Database::open_memory();
        let request = imap_setup_request();
        super::save_tested_imap_settings(&database, &request, None, None).unwrap();
        let before = database
            .imap_account_settings(&request.email)
            .unwrap()
            .unwrap();
        for archive in [None, Some("")] {
            let commit = super::ImapMailboxMappingCommit {
                email: request.email.clone(),
                archive: archive.map(str::to_string),
                create_archive: None,
                mailbox_overrides: Default::default(),
                label_container: None,
            };
            assert!(super::save_imap_mailbox_mapping(&database, before.clone(), commit).is_err());
            assert_eq!(
                database
                    .imap_account_settings(&request.email)
                    .unwrap()
                    .unwrap(),
                before
            );
        }
    }
}
