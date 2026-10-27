//! The live `ImapProvider` — the first IMAP code that plugs into the shared
//! `sync.rs` engine (Phase 2 Slice 5a).
//!
//! It implements the whole provider seam
//! ([`MailProvider`](crate::provider::MailProvider) =
//! `MailSync + MailFetch + MailMutate + MailSend`) for one IMAP account, INBOX
//! only, read-only, in the "neither" capability tier (no CONDSTORE/QRESYNC, no
//! IDLE). The engine polls it exactly as it polls Gmail; nothing above the
//! seam branches on the provider.
//!
//! ## Cursor semantics are AT-LEAST-ONCE
//!
//! The engine persists the cursor only after an ingest succeeds, so the
//! provider must tolerate being polled twice with the same cursor and return
//! the same changes. The cursor is a monotonic generation (see
//! [`SyncCursor::from_generation`]). One sync round:
//!
//! 1. EXAMINE INBOX (read-only — never SELECT, so `\Seen` is never set).
//! 2. UIDVALIDITY reset? Drop INBOX locations (bodies survive — keyed by the
//!    stable id), treat the whole mailbox as new.
//! 3. New mail: `UID FETCH` identity pass (reuse [`fetch_identity`]), derive
//!    sticky ids, cache bodies with [`ensure_body`].
//! 4. Flag sweep over the window; deletion check via `UID SEARCH ALL`. Without
//!    CONDSTORE these run EVERY poll (a flag change moves neither UIDNEXT nor
//!    MESSAGES, so there is no gate to skip them for INBOX).
//! 5. Thread the batch ([`threading`]), then commit one atomic round:
//!    message->thread rows, merge aliases, a generation bump, and the changed
//!    thread ids journalled under the new generation — all in one transaction.
//!
//! `poll(cursor=g)` then refreshes INBOX and returns every thread journalled
//! with generation > g, with the new generation as the next cursor.
//!
//! ## Connections and errors
//!
//! Connections go through the shared [`ImapConnectionManager`] (max 2/account;
//! sync takes the command slot). A dropped connection is retried once. Errors
//! are already mapped to [`ProviderError`] by the connection/session layer
//! (RFC 5530); only an explicit authentication failure yields
//! `ReauthenticationRequired` (which pauses the account), everything else is
//! transient.

use async_trait::async_trait;

use crate::provider::{
    LabelModel, MailProvider, ProviderCapabilities, ProviderError, ProviderResult,
};

use super::connection::{ConnectionRole, ImapConnectionManager};
use super::fetch::BodyCache;
use super::policy::SyncLimits;
use super::search;
use super::session::ImapSession;
use super::settings::{ImapAccountSettings, LabelStorage};
use super::threading::ThreadingInput;
use super::{ImapLocation, ImapStateStore};

mod body_fetch;
mod drafts;
mod label_folders;
mod mail_send;
mod moves;
mod mutate;
mod refresh;
mod server_search;

/// The INBOX name, the only mailbox this slice syncs.
const INBOX: &str = "INBOX";

/// How the provider obtains an authenticated session for one command. The
/// production source dials the real server through the
/// [`ImapConnectionManager`] (one reconnect on a dropped connection); a test
/// source hands back a scripted in-memory session. Boxing the session behind
/// the [`ImapSession`] trait is what lets the engine-level test drive the real
/// `sync.rs` loop against a fake server with no network.
#[async_trait]
pub(super) trait ImapSessionSource: Send + Sync {
    async fn open(&self) -> ProviderResult<Box<dyn ImapSession>>;
}

/// The production session source: dial through the connection manager on the
/// command slot, retrying ONCE on a dropped connection.
struct ManagerSessionSource {
    manager: ImapConnectionManager,
    username: String,
    password: String,
}

#[async_trait]
impl ImapSessionSource for ManagerSessionSource {
    async fn open(&self) -> ProviderResult<Box<dyn ImapSession>> {
        // The lease must outlive the session so the account's connection slot
        // stays accounted for; the connected session owns its stream, and the
        // lease is dropped when the session is, so we bundle them.
        match self
            .manager
            .connect_leased(ConnectionRole::Command, &self.username, &self.password)
            .await
        {
            Ok((session, lease)) => Ok(Box::new(LeasedSession { session, _lease: lease })),
            Err(error) if error.retry_mutation() || is_transient(&error) => {
                let (session, lease) = self
                    .manager
                    .connect_leased(ConnectionRole::Command, &self.username, &self.password)
                    .await?;
                Ok(Box::new(LeasedSession { session, _lease: lease }))
            }
            Err(error) => Err(error),
        }
    }
}

/// A connected session bundled with the connection lease it holds, so dropping
/// the session frees the account's slot. Delegates every [`ImapSession`] call
/// to the inner session.
struct LeasedSession {
    session: super::connection::ConnectedSession,
    _lease: super::connection::ConnectionLease,
}

#[async_trait]
impl ImapSession for LeasedSession {
    async fn select(&mut self, mailbox: &str) -> ProviderResult<super::session::MailboxStatus> {
        self.session.select(mailbox).await
    }
    async fn examine(&mut self, mailbox: &str) -> ProviderResult<super::session::MailboxStatus> {
        self.session.examine(mailbox).await
    }
    async fn uid_search(&mut self, query: &str) -> ProviderResult<Vec<u32>> {
        self.session.uid_search(query).await
    }
    async fn uid_fetch(
        &mut self,
        uid_set: &str,
        items: &str,
    ) -> ProviderResult<Vec<async_imap::types::Fetch>> {
        self.session.uid_fetch(uid_set, items).await
    }
    async fn capabilities(&mut self) -> ProviderResult<Vec<String>> {
        self.session.capabilities().await
    }
    async fn list_mailboxes(&mut self) -> ProviderResult<Vec<super::session::MailboxEntry>> {
        self.session.list_mailboxes().await
    }
    #[cfg(test)]
    async fn run_command_capture_code(&mut self, command: &str) -> ProviderResult<Option<String>> {
        self.session.run_command_capture_code(command).await
    }
    async fn create_mailbox(&mut self, mailbox: &str) -> ProviderResult<()> {
        self.session.create_mailbox(mailbox).await
    }
    async fn logout(&mut self) -> ProviderResult<()> {
        self.session.logout().await
    }
    fn as_write_session(&mut self) -> Option<&mut dyn super::write::ImapWriteSession> {
        // `LeasedSession` implements `ImapWriteSession` below (run 1), so a
        // writer can re-borrow it as the write surface.
        Some(self)
    }
}

#[async_trait]
impl super::write::ImapWriteSession for LeasedSession {
    async fn uid_store(
        &mut self,
        uid_set: &str,
        op: super::write::StoreOp,
        flags: &[String],
    ) -> ProviderResult<()> {
        self.session.uid_store(uid_set, op, flags).await
    }
    async fn uid_copy(
        &mut self,
        uid_set: &str,
        dest: &str,
    ) -> super::write::WriteResult<super::write::CopyOutcome> {
        self.session.uid_copy(uid_set, dest).await
    }
    async fn uid_move(
        &mut self,
        uid_set: &str,
        dest: &str,
    ) -> super::write::WriteResult<super::write::CopyOutcome> {
        self.session.uid_move(uid_set, dest).await
    }
    async fn uid_expunge(&mut self, uid_set: &str) -> ProviderResult<()> {
        self.session.uid_expunge(uid_set).await
    }
    async fn create_mailbox_arg(&mut self, arg: &str) -> super::write::WriteResult<()> {
        self.session.create_mailbox_arg(arg).await
    }
    async fn delete_mailbox_arg(&mut self, arg: &str) -> ProviderResult<()> {
        self.session.delete_mailbox_arg(arg).await
    }
    async fn append(
        &mut self,
        mailbox: &str,
        flags: &[String],
        content: &[u8],
    ) -> super::write::WriteResult<()> {
        // Delegate to the inner `AsyncImapSession`'s own `ImapWriteSession::
        // append` (which validates, encodes modified UTF-7, and uses the
        // library `append`); `ConnectedSession` is an `AsyncImapSession`.
        super::write::ImapWriteSession::append(&mut self.session, mailbox, flags, content).await
    }
}

/// A live IMAP account provider.
///
/// Holds the account-scoped state store and body cache, the session source,
/// and the account's label model. One per account; constructed by the auth
/// seam with everything it needs so it never reaches back for a database
/// handle at call time.
pub struct ImapProvider {
    store: ImapStateStore,
    cache: BodyCache,
    /// The account KEY (the email address entered at setup). May DIFFER from
    /// the IMAP LOGIN username — e.g. Proton Bridge login `joel` for the
    /// account `joel@proton.me`. This is the account's own sending address and
    /// the value `sender_identity()` / `identities()[0]` must report so the
    /// outbox tick does not pause every send whose `draft.account` is the
    /// address rather than the login. See `docs/imap-design.md` ("Identities")
    /// and the brief's fact 6.
    account_id: String,
    /// The IMAP password, reused as the SMTP password per the user's decision
    /// (`docs/imap-design.md`, "Sending": SMTP uses the IMAP login). Never
    /// formatted anywhere — it goes only into `SmtpCredentials`, which redacts
    /// it from `Debug`. `ImapProvider` has no derived `Debug`, so this field
    /// cannot leak through one.
    smtp_password: String,
    label_model: LabelModel,
    /// The account's non-secret IMAP settings — the plan needs its mailbox
    /// overrides, archive mailbox, label container and label storage to resolve
    /// roles. Carried here so no call reaches back for a database handle.
    settings: ImapAccountSettings,
    source: std::sync::Arc<dyn ImapSessionSource>,
    /// Injected clock for deterministic `fetched_at` in tests; wall clock in
    /// production.
    now: fn() -> i64,
    /// The sync window/batch limits for a round. Overridable in tests.
    limits: SyncLimits,
    /// TEST KNOB: override the Sent per-poll acquisition budget
    /// ([`policy::SENT_ROUND_MESSAGES`]) with a tiny value so the chunking
    /// boundary is exercised without a 100-message fixture. `None` in
    /// production uses the policy constant.
    sent_round_budget: Option<usize>,
    /// TEST KNOB: override the window ceiling for every NON-INBOX synced
    /// mailbox (Folder and Sent classes), keeping each class's eviction
    /// semantics, so a window boundary is testable without thousands of
    /// messages. `None` in production uses each class's policy limit.
    folder_window_override: Option<usize>,
    /// TEST KNOB: inject synthetic synced-mailbox candidates into the fair
    /// scheduler so a fairness test can exercise many user folders without a
    /// large plan. Each is scheduled by name but has no plan entry, so it is
    /// never driven against the server. `None` in production.
    extra_synced_candidates: Option<Vec<super::policy::ScheduleCandidate>>,
    /// TEST KNOB: force named catalog mailboxes to plan as USER-FOLDER class
    /// (role `None`, `synced_now = true`, `LabelKind::UserFolder`) so the
    /// coverage-epoch logic can be exercised with real, server-backed folders
    /// that are NOT system roles and so are subject to the per-poll budget.
    /// `None` in production (run B's plan will set this naturally).
    force_user_folders: Option<std::collections::BTreeSet<String>>,
    /// TEST KNOB: override `policy::FOLDER_ROUNDS_PER_POLL` so a test can set a
    /// tiny per-poll folder budget (e.g. 1) and watch the coverage epoch
    /// advance only after several polls. `None` in production.
    folder_rounds_per_poll_override: Option<usize>,
    /// TEST KNOB: override [`policy::SEARCH_DEADLINE`] with a tiny value (even
    /// zero) so a test can drive the search deadline's PARTIAL-result path
    /// deterministically instead of waiting out 20 seconds. `None` in
    /// production uses the policy constant.
    search_deadline_override: Option<std::time::Duration>,
    /// How many times a round rebuilt the threader's prior state from the
    /// store. That rebuild reads every cached body, so a round with no new
    /// mail must not do it; tests assert on this.
    thread_state_loads: std::sync::atomic::AtomicUsize,
}

/// Everything needed to build an [`ImapProvider`] for one account, assembled by
/// the auth seam from stored settings + the keychain password.
pub struct ImapProviderConfig {
    pub account_id: String,
    pub username: String,
    pub password: String,
    /// A separate SMTP password, when configured; otherwise reuse IMAP's.
    pub smtp_password: Option<String>,
    pub manager: ImapConnectionManager,
    pub store: ImapStateStore,
    pub cache: BodyCache,
    pub label_model: LabelModel,
    /// The account's non-secret IMAP settings, so the provider can build the
    /// sync plan (role resolution) without a database round trip.
    pub settings: ImapAccountSettings,
}

/// The final generation reached by a full refresh walk.
#[derive(Clone, Copy, Debug)]
pub(super) struct Refreshed {
    /// The generation after the last round that committed.
    pub last: u64,
}

/// The accumulated output of [`ImapProvider::ingest_new_uids`] — the location
/// rows, threading inputs and message-id lists one UID list produced, for the
/// caller to fold into its [`SyncRoundWrite`]. Shared by `refresh_mailbox_with`
/// (new mail) and `acquire_uids` (a search's explicit UID list).
#[derive(Debug, Default)]
pub(super) struct IngestOutcome {
    /// Location rows to insert (deferred to the atomic round commit).
    pub locations: Vec<ImapLocation>,
    /// Threading inputs for the batch's messages.
    pub threading_inputs: Vec<ThreadingInput>,
    /// Every message id touched this ingest (new mail), for change tracking.
    pub changed_message_ids: Vec<String>,
    /// Message ids newly located this ingest, for hotness/label transitions.
    pub new_message_ids: Vec<String>,
}

/// What one [`ImapProvider::acquire_uids`] call did (Phase 5 run 3). Run 4
/// consumes this to report a search's ingest result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcquireOutcome {
    /// The sync generation after the acquire round committed.
    pub generation: u64,
    /// The thread ids affected (resolved through merge aliases), sorted.
    pub affected_threads: Vec<String>,
    /// How many requested UIDs were REFUSED because pinning them would exceed
    /// [`policy::MAX_SEARCH_PINS`] for this account. Over-cap UIDs are refused
    /// (newest-first acceptance keeps the highest UIDs) rather than acquired,
    /// never silently dropped and never evicting an existing pin.
    pub skipped_over_cap: usize,
}

/// Why an [`ImapProvider::acquire_uids`] call failed. Distinguishes the
/// UIDVALIDITY-incarnation mismatch (the caller's UID list is addressed under a
/// different incarnation than the server now reports — acquiring would ingest
/// under the wrong incarnation) from any other provider error, so run 4 can
/// drop a stale UID list and re-search rather than retrying blindly.
#[derive(Debug)]
pub enum AcquireError {
    /// The server's current UIDVALIDITY differs from the one the UID list was
    /// addressed under. NOTHING is ingested.
    UidValidityMismatch {
        mailbox: String,
        expected: i64,
        found: i64,
    },
    /// The mailbox is outside this run's sync set, so a pin there would never
    /// be refreshed or released. Nothing is fetched or written.
    MailboxNotSynced { mailbox: String },
    /// Any other error from the EXAMINE / fetch / commit path.
    Provider(ProviderError),
}

impl std::fmt::Display for AcquireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AcquireError::UidValidityMismatch {
                mailbox,
                expected,
                found,
            } => write!(
                f,
                "UIDVALIDITY mismatch acquiring {mailbox}: expected {expected}, server reports {found}"
            ),
            AcquireError::MailboxNotSynced { mailbox } => {
                write!(f, "{mailbox} is not synced, so search results there cannot be kept")
            }
            AcquireError::Provider(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for AcquireError {}

impl From<AcquireError> for ProviderError {
    /// Flatten to a `ProviderError` at the `MailSync` boundary: a UIDVALIDITY
    /// mismatch is a transient, retry-after-research condition (re-search under
    /// the new incarnation), so it maps to `InvalidOperation` with a clear
    /// message rather than silently succeeding.
    fn from(error: AcquireError) -> Self {
        match error {
            AcquireError::Provider(inner) => inner,
            other @ (AcquireError::UidValidityMismatch { .. }
            | AcquireError::MailboxNotSynced { .. }) => {
                ProviderError::InvalidOperation(other.to_string())
            }
        }
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Why a [`MailSync::search`] returned fewer results than a full scan would.
/// Carried on [`ImapSearchResult`] so run 5 can word the UI ("showing partial
/// results"); the public [`MailSync::search`] wrapper discards it (returning
/// only the thread ids) because `ThreadPage` has no partial channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PartialReason {
    /// The wall-clock [`policy::SEARCH_DEADLINE`] expired before every planned
    /// mailbox was searched; `mailboxes_searched` of `mailboxes_planned` ran.
    DeadlineExpired {
        mailboxes_searched: usize,
        mailboxes_planned: usize,
    },
    /// The [`policy::MAX_SEARCH_ACQUIRE`] budget was reached, so hits beyond it
    /// were not acquired (a page/refine away).
    BudgetReached,
    /// A mailbox errored mid-search; the hits acquired before it are returned
    /// and the mailbox that failed is named.
    MailboxError { mailbox: String, error: String },
}

/// The detailed outcome of one IMAP [`MailSync::search`]: the thread ids to
/// return plus everything run 5 needs to word the UI. `search()` is a thin
/// wrapper that returns only `thread_ids` as a [`ThreadPage`]; this inner
/// struct is what [`ImapProvider::search_detailed`] produces so the richer
/// result is testable without a provider-neutral surface for it yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ImapSearchResult {
    /// Thread ids to show, deduplicated, newest-first (highest acquired UID
    /// first, across mailboxes in plan order). These are the ids `fetch_thread`
    /// resolves — resolved through merge aliases exactly as `acquire_uids`
    /// reports them.
    pub thread_ids: Vec<String>,
    /// Set when the result is not exhaustive (deadline, budget, or a mailbox
    /// error). `None` means a complete search of every planned mailbox.
    pub partial: Option<PartialReason>,
    /// Reasons the server search was SKIPPED entirely (a term the server
    /// cannot honestly evaluate). Empty `thread_ids` + `Some(..)` here means
    /// "keep local results only", not "no matches".
    pub skipped: Option<Vec<search::SearchNotice>>,
    /// The server proved the answer is empty (an unknown label/folder/mailbox).
    pub no_matches: Option<search::SearchNotice>,
    /// Planned mailboxes that were not `synced_now` and so were skipped (a pin
    /// there would never be maintained). Reported, never silently ignored.
    pub unsynced_mailboxes: Vec<String>,
    /// How many hits were refused across all mailboxes because pinning them
    /// would exceed [`policy::MAX_SEARCH_PINS`] (summed from each
    /// `AcquireOutcome::skipped_over_cap`).
    pub skipped_over_cap: usize,
}


/// Choose the account's [`LabelModel`] from its saved `label_storage`. INBOX
/// has no user labels this slice, but the model still drives the frontend's
/// label actions, so it is reported faithfully from setup.
pub fn label_model_for(storage: LabelStorage) -> LabelModel {
    match storage {
        LabelStorage::Keywords => LabelModel::ImapKeywords,
        LabelStorage::Folders => LabelModel::ImapLabelFolders,
        LabelStorage::None => LabelModel::ImapNoUserLabels,
    }
}

impl ImapProvider {
    pub fn new(config: ImapProviderConfig) -> Self {
        let source = std::sync::Arc::new(ManagerSessionSource {
            manager: config.manager,
            username: config.username.clone(),
            password: config.password.clone(),
        });
        Self {
            store: config.store,
            cache: config.cache,
            account_id: config.account_id,
            smtp_password: config.smtp_password.unwrap_or(config.password),
            label_model: config.label_model,
            settings: config.settings,
            source,
            now: unix_now,
            limits: SyncLimits::inbox(),
            sent_round_budget: None,
            folder_window_override: None,
            extra_synced_candidates: None,
            force_user_folders: None,
            folder_rounds_per_poll_override: None,
            search_deadline_override: None,
            thread_state_loads: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Open an authenticated session through the configured source.
    async fn open_session(&self) -> ProviderResult<Box<dyn ImapSession>> {
        self.source.open().await
    }
}


/// Whether an error is a transient transport failure worth one reconnect.
fn is_transient(error: &ProviderError) -> bool {
    matches!(error, ProviderError::TransientTransport(_))
}

/// Chunk a UID slice into batches of at most `size`.
fn chunk(uids: &[u32], size: usize) -> impl Iterator<Item = &[u32]> {
    uids.chunks(size.max(1))
}

/// Render a UID batch as an IMAP UID set string (`1,2,3`).
fn uid_set(uids: &[u32]) -> String {
    uids.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn db_err(error: crate::db::DatabaseError) -> ProviderError {
    ProviderError::Other(error.to_string())
}

impl MailProvider for ImapProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            send_supported: true,
            server_search: true,
            provided_threads: false,
            label_model: self.label_model,
            // No server-side sent copy to verify an uncertain send against;
            // an uncertain IMAP send resolves to the terminal Unverifiable
            // state via `find_sent_copy` -> Ok(None), not a Gmail-style
            // lookup.
            verifiable_delivery: false,
            // IMAP saves the sender's own Sent copy itself, as a durable job
            // the engine drives after an accepted send (run 4b). The engine
            // seeds that job atomically with the `sent` transition because
            // this is true.
            client_saves_sent_copy: true,
        }
    }
}

#[cfg(test)]
mod tests;

// ===========================================================================
// Phase 3 run 2: read/unread + star/unstar flag mutations.
//
// These drive `modify_messages` / `modify_thread` through a REAL `ImapProvider`
// over a REAL `AsyncImapSession` (so async-imap's own command formatting and
// the `imap-proto` parser run) against a scripted server that understands the
// WRITE path (SELECT read-write + PERMANENTFLAGS + UIDVALIDITY, UID STORE, the
// verify UID FETCH FLAGS) AND the READ path (EXAMINE, UID SEARCH, identity +
// body + flags FETCH), so a mutation and a following sync sweep run against the
// SAME shared server state. The server shares its mailbox state by Arc<Mutex>,
// so a STORE on the live connection is visible to the next sweep — which is how
// "the local flags_json agrees and a following sweep sees no change" is proven.
// No network, no openssl / /dev/tcp.
// ===========================================================================
#[cfg(test)]
mod mutate_tests;
#[cfg(test)]
mod drafts_tests;
