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

use crate::mime::RawMessage;
use crate::models::Label;
use crate::provider::{
    Delivery, DeliveryReceipt, LabelModel, MailFetch, MailMutate, MailProvider, MailSend,
    MailSync, ProviderCapabilities, ProviderError, ProviderResult, SyncBatch, SyncCursor,
    ThreadPage,
};

use super::connection::{ConnectionRole, ImapConnectionManager};
use super::fetch::{fetch_identity, BodyCache, SessionBodyFetcher};
use super::labels::{labels_for_location_ids, merge_label_sets};
use super::plan::{self, CatalogMailbox, SyncPlan};
use super::policy::{self, SyncLimits};
use super::rfc822::{attachment_bytes_from_raw, to_raw_message};
use super::session::ImapSession;
use super::settings::{ImapAccountSettings, LabelStorage};
use super::threading::{self, ThreadState, ThreadingInput};
use super::{ImapLocation, ImapStateStore, SyncRoundWrite};

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
    async fn run_command_capture_code(&mut self, command: &str) -> ProviderResult<Option<String>> {
        self.session.run_command_capture_code(command).await
    }
    async fn noop(&mut self) -> ProviderResult<()> {
        self.session.noop().await
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
    async fn rename_mailbox_arg(&mut self, from: &str, to: &str) -> ProviderResult<()> {
        self.session.rename_mailbox_arg(from, to).await
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
    username: String,
    /// The account KEY (the email address entered at setup). May DIFFER from
    /// `username` (the IMAP LOGIN) — e.g. Proton Bridge login `joel` for the
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
            username: config.username,
            account_id: config.account_id,
            smtp_password: config.password,
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
            thread_state_loads: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Open an authenticated session through the configured source.
    async fn open_session(&self) -> ProviderResult<Box<dyn ImapSession>> {
        self.source.open().await
    }

    /// Refresh every synced mailbox against the server and return the new
    /// generation. INBOX is swept first and every poll; the other synced
    /// mailboxes (Trash, Junk, then Sent) follow in plan order with cheap
    /// cadence gating. Each mailbox round is its OWN atomic commit, so a
    /// failure in one mailbox leaves the others' committed work intact and the
    /// at-least-once cursor contract whole.
    async fn refresh_all(&self) -> ProviderResult<Refreshed> {
        let mut session = self.open_session().await?;
        let result = self.refresh_all_with(session.as_mut()).await;
        let _ = session.logout().await;
        result
    }

    /// The full plan walk against an already-open session. INBOX first (always
    /// a full sweep), then each other `synced_now` mailbox in plan order up to
    /// the per-poll budget, each with cadence gating. The last-committed
    /// generation is returned. INBOX failures propagate (nothing is ingested
    /// without INBOX); a failing folder is logged and skipped, since mailboxes
    /// that already committed are durable and the folder is retried next poll.
    pub(super) async fn refresh_all_with(
        &self,
        session: &mut dyn ImapSession,
    ) -> ProviderResult<Refreshed> {
        // Catalog refresh (item 3): at baseline and whenever the account's
        // periodic sweep is due, LIST the server and upsert NAME/DELIMITER/
        // SPECIAL_USE so a mailbox created after setup (a new Sent/Trash/Junk)
        // is noticed and the plan picks it up. Catalog-only — never clobbers
        // counters/write-capabilities. A failure here is logged and IGNORED:
        // it must never abort a poll (INBOX mail keeps flowing).
        self.refresh_catalog_if_due(session).await;

        let plan = self.build_sync_plan()?;
        // INBOX first and always a full sweep.
        let inbox_entry = plan
            .entries
            .iter()
            .find(|e| e.is_inbox)
            .cloned()
            // If the catalog has no INBOX row yet (first ever sync before
            // discovery persisted it), synthesize an INBOX plan entry.
            .unwrap_or(plan::PlanEntry {
                mailbox: INBOX.to_string(),
                role: None,
                is_inbox: true,
                window_class: policy::SyncWindowClass::Inbox,
                label_kind: plan::LabelKind::SystemRole,
                location_label_id: Some("INBOX".to_string()),
                synced_now: true,
            });
        let mut generation = self.refresh_mailbox_with(session, &inbox_entry, true).await?;

        // FAIR SCHEDULING (Slice 5b-2). The other synced mailboxes are chosen
        // per poll by `policy::schedule_visits`, NOT by a fixed prefix: every
        // synced SYSTEM-ROLE mailbox (Sent, Trash, Junk, Archive) is visited
        // every poll (bounded, cheap when idle via the cadence gate), then
        // user/label folders LEAST-RECENTLY-ATTEMPTED first up to the remaining
        // FOLDER_ROUNDS_PER_POLL budget. Over MAX_SYNCED_MAILBOXES the set is
        // capped (system roles first, then by name) and the drop is LOGGED.
        let synced_non_inbox: Vec<plan::PlanEntry> =
            plan.synced().filter(|e| !e.is_inbox).cloned().collect();

        // Candidates for the cap + scheduler, carrying each mailbox's persisted
        // attempt order, successful visit clock and coverage stamp. Attempts
        // rotate even on failure; only successful visits count toward coverage.
        let current_epoch = self.store.complete_walks().map_err(db_err)? as i64;
        let mut candidates: Vec<policy::ScheduleCandidate> = Vec::new();
        for entry in &synced_non_inbox {
            candidates.push(policy::ScheduleCandidate {
                mailbox: entry.mailbox.clone(),
                last_attempt: self
                    .store
                    .mailbox_last_attempt(&entry.mailbox)
                    .map_err(db_err)?,
                last_visited_at: self.store.mailbox_last_visited_at(&entry.mailbox).map_err(db_err)?,
                visited_in_epoch: self.store.mailbox_visited_in_epoch(&entry.mailbox).map_err(db_err)?,
                is_system_role: entry.role.is_some(),
            });
        }
        // TEST KNOB: inject synthetic synced candidates so a fairness test can
        // exercise the scheduler with many user folders without a huge plan.
        // Never set in production.
        if let Some(extra) = &self.extra_synced_candidates {
            candidates.extend(extra.iter().cloned());
        }

        // Cap the synced set; LOG the count dropped, never silently.
        let (allowed, dropped) =
            policy::cap_synced_mailboxes(&candidates, policy::MAX_SYNCED_MAILBOXES);
        if dropped > 0 {
            log::warn!(
                "imap sync: {dropped} synced mailbox(es) over the {}-mailbox cap are not synced this poll",
                policy::MAX_SYNCED_MAILBOXES
            );
        }
        let allowed_set: std::collections::BTreeSet<&str> =
            allowed.iter().map(String::as_str).collect();
        let allowed_candidates: Vec<policy::ScheduleCandidate> = candidates
            .iter()
            .filter(|c| allowed_set.contains(c.mailbox.as_str()))
            .cloned()
            .collect();

        // The per-poll visit order (system roles first, then LRV folders up to
        // the budget). A COVERAGE EPOCH is coverage ACROSS polls, not a single
        // poll that visited every mailbox: run B syncs more user/label folders
        // than one poll's budget can visit, so no single poll covers them all.
        // Each mailbox visited this poll stamped `visited_in_epoch` with the
        // current epoch (inside refresh_mailbox_with). After the poll, the
        // epoch advances iff INBOX plus every allowed real plan entry has
        // been successfully visited in the current epoch. Capped-out entries
        // cannot hold coverage open because they are never scheduled.
        // Synthetic test candidates are NOT server-backed and are excluded.
        let budget = self
            .folder_rounds_per_poll_override
            .unwrap_or(policy::FOLDER_ROUNDS_PER_POLL);
        let visit_order = policy::schedule_visits(&allowed_candidates, current_epoch, budget);
        let entry_by_name: std::collections::BTreeMap<&str, &plan::PlanEntry> =
            synced_non_inbox.iter().map(|e| (e.mailbox.as_str(), e)).collect();
        let scheduled_real: Vec<&plan::PlanEntry> = visit_order
            .iter()
            .filter_map(|name| entry_by_name.get(name.as_str()).copied())
            .collect();
        for entry in scheduled_real {
            self.store
                .record_mailbox_attempt(&entry.mailbox)
                .map_err(db_err)?;
            // Cadence gating decides whether a full sweep is needed. Only INBOX
            // may abort a poll: the engine ingests a poll's changes only after
            // the whole poll succeeds, so a folder that cannot be synced (its
            // mailbox renamed or deleted on the server, say) must not stop mail
            // flowing. A folder failure is logged and retried next poll, and a
            // dropped connection ends the folder walk (INBOX already committed).
            // Only a credential rejection, which pauses the account, propagates.
            // A folder that errors is simply not stamped for this epoch, so the
            // epoch stays open until it succeeds (the safe direction: the
            // emptied-thread grace never advances early).
            match self.refresh_mailbox_with(session, entry, false).await {
                Ok(committed) => generation = committed,
                Err(error) if error.requires_reauthentication() => return Err(error),
                Err(error) => {
                    log::warn!(
                        "imap sync: skipping mailbox {:?} this poll: {error}",
                        entry.mailbox
                    );
                    if is_transient(&error) {
                        break;
                    }
                }
            }
        }

        // End-of-poll coverage check (Slice 5b-2 item 4). The full currently-
        // synced set is INBOX plus every real entry allowed by the cap (NOT
        // just this poll's schedule, and NOT synthetic test candidates). The epoch
        // advances only when every one has been visited in the current epoch;
        // on advance, emptied hot threads whose grace just expired are journaled
        // (same transaction) — bumping the generation only when something
        // actually expired. Visit recording never bumps the generation.
        let mut synced_all: Vec<String> = Vec::with_capacity(synced_non_inbox.len() + 1);
        synced_all.push(inbox_entry.mailbox.clone());
        synced_all.extend(
            synced_non_inbox
                .iter()
                .filter(|e| allowed_set.contains(e.mailbox.as_str()))
                .map(|e| e.mailbox.clone()),
        );
        let (after_epoch, outstanding) = self
            .store
            .advance_coverage_epoch_if_complete(&synced_all)
            .map_err(db_err)?;
        generation = generation.max(after_epoch);
        if !outstanding.is_empty() {
            // Coverage still open this epoch: the emptied-thread grace cannot
            // advance until these mailboxes are successfully visited. This
            // fires at most once per poll and only while coverage is genuinely
            // incomplete. A mailbox that VANISHED from the server no longer
            // holds the epoch open forever — the catalog refresh retires it
            // (item 5 / `retire_vanished_mailboxes`), so it drops out of the
            // synced set. A mailbox that is still LISTed but transiently fails
            // EXAMINE legitimately holds the epoch open until it recovers (the
            // safe direction: the grace never advances early).
            log::warn!(
                "imap sync: coverage epoch still open — {} mailbox(es) not yet visited this epoch: {:?}",
                outstanding.len(),
                outstanding
            );
        }
        Ok(Refreshed { last: generation })
    }

    /// Build the account's sync plan from the persisted catalog + settings.
    fn build_sync_plan(&self) -> ProviderResult<SyncPlan> {
        let rows = self.store.mailboxes().map_err(db_err)?;
        let delimiter = rows
            .iter()
            .find_map(|row| row.delimiter.clone())
            .unwrap_or_else(|| "/".to_string());
        let catalog: Vec<CatalogMailbox> = rows
            .iter()
            .map(|row| CatalogMailbox {
                name: row.name.clone(),
                special_use: row.special_use.clone(),
            })
            .collect();
        let mut plan = plan::build_plan(&catalog, Some(&delimiter), &self.settings);
        // TEST KNOB: re-class named mailboxes as synced USER FOLDERS (role
        // None, synced_now true) so the coverage-epoch logic can be exercised
        // with real server-backed folders that are subject to the per-poll
        // budget (run B produces such entries naturally).
        if let Some(forced) = &self.force_user_folders {
            for entry in plan.entries.iter_mut() {
                if forced.contains(&entry.mailbox) {
                    entry.role = None;
                    entry.synced_now = true;
                    entry.label_kind = plan::LabelKind::UserFolder;
                    entry.window_class = policy::SyncWindowClass::Folder;
                    entry.location_label_id = Some(format!("folder:{}", entry.mailbox));
                }
            }
        }
        Ok(plan)
    }

    /// Round-time catalog refresh (item 3) + vanished-mailbox retirement
    /// (item 5). When the account's periodic catalog sweep is due (always true
    /// at baseline), `LIST` the server and record NAME/DELIMITER/SPECIAL_USE
    /// for every SELECTABLE mailbox via the catalog-only upsert, so a mailbox
    /// created after account setup is noticed and the plan can act on it.
    /// `\Noselect` containers are skipped (they are never locations). After a
    /// SUCCESSFUL non-empty `LIST`, any catalog row the server no longer lists
    /// (and that is not INBOX) is RETIRED — its catalog row, sync-state row and
    /// locations are deleted and its hot threads journaled — so a deleted
    /// Proton label/folder stops failing EXAMINE forever and holding the
    /// coverage epoch open. An empty `LIST` is treated as a failure and retires
    /// nothing. Any failure — the `LIST` itself, a single upsert, or the
    /// retirement — is logged and swallowed, because this must NEVER abort a
    /// poll: if it did, a transient `LIST` error would stop INBOX mail flowing.
    async fn refresh_catalog_if_due(&self, session: &mut dyn ImapSession) {
        let now = (self.now)();
        match self.store.catalog_refresh_due(now) {
            Ok(true) => {}
            Ok(false) => return,
            Err(error) => {
                log::warn!("imap sync: catalog refresh skipped (state read failed): {error}");
                return;
            }
        }
        let entries = match session.list_mailboxes().await {
            Ok(entries) => entries,
            Err(error) => {
                log::warn!("imap sync: catalog refresh LIST failed, ignored this poll: {error}");
                return;
            }
        };
        // The SELECTABLE mailboxes the server reported this LIST (the catalog
        // only ever holds selectable rows; `\Noselect` containers are never
        // catalog locations). This is both the upsert set and the "present"
        // set for vanished-mailbox retirement.
        let present: Vec<String> = entries
            .iter()
            .filter(|entry| !entry.no_select)
            .map(|entry| entry.name.clone())
            .collect();
        for entry in &entries {
            if entry.no_select {
                continue; // \Noselect containers are never catalog locations.
            }
            if let Err(error) = self.store.upsert_mailbox_catalog(
                &entry.name,
                entry.delimiter.as_deref(),
                entry.special_use.as_deref(),
            ) {
                log::warn!(
                    "imap sync: catalog refresh could not record {:?}, ignored: {error}",
                    entry.name
                );
            }
        }
        // Vanished-mailbox retirement (item 5 / fact 5). A SUCCESSFUL LIST that
        // returned at least one selectable mailbox lets us retire any catalog
        // row the server no longer lists: it would otherwise fail EXAMINE
        // forever and hold the coverage epoch — and the emptied-thread grace —
        // open indefinitely. An empty `present` is treated as a FAILURE (a
        // server that momentarily lists nothing must not wipe the catalog), so
        // we skip retirement entirely; `retire_vanished_mailboxes` guards this
        // too. INBOX is never retired.
        if present.is_empty() {
            log::warn!(
                "imap sync: catalog refresh LIST returned no selectable mailboxes; \
                 treating as a failure and retiring nothing this poll"
            );
        } else {
            match self.store.retire_vanished_mailboxes(&present) {
                Ok(retired) if !retired.is_empty() => {
                    log::info!(
                        "imap sync: retired {} vanished mailbox(es) from the catalog: {:?}",
                        retired.len(),
                        retired
                    );
                }
                Ok(_) => {}
                Err(error) => {
                    log::warn!(
                        "imap sync: vanished-mailbox retirement failed, ignored this poll: {error}"
                    );
                }
            }
        }
        if let Err(error) = self.store.record_catalog_refresh(now) {
            log::warn!("imap sync: catalog refresh clock not recorded: {error}");
        }
    }

    /// Run the INBOX round against an already-open session (kept for the
    /// engine-level tests and callers that drive INBOX directly). Equivalent to
    /// a forced full sweep of the INBOX plan entry.
    pub(super) async fn refresh_inbox_with(
        &self,
        session: &mut dyn ImapSession,
    ) -> ProviderResult<u64> {
        let entry = plan::PlanEntry {
            mailbox: INBOX.to_string(),
            role: None,
            is_inbox: true,
            window_class: policy::SyncWindowClass::Inbox,
            label_kind: plan::LabelKind::SystemRole,
            location_label_id: Some("INBOX".to_string()),
            synced_now: true,
        };
        self.refresh_mailbox_with(session, &entry, true).await
    }

    /// The sync round for ONE mailbox against an already-open session, driven
    /// by its plan entry. INBOX passes `force_sweep = true` (swept every poll);
    /// a non-INBOX mailbox passes `false` and the cheap cadence check decides
    /// whether to do the full SEARCH/FETCH or skip to just recording its
    /// counters.
    ///
    /// Every durable write this round decides — new/updated locations,
    /// deletions, thread assignments, aliases, newly-hot threads, the journal,
    /// the mailbox cadence counters and the generation bump — is collected into
    /// one [`SyncRoundWrite`] and applied in a SINGLE transaction at the end
    /// (`commit_sync_round`). A failure before that commit changes no mail,
    /// cadence counters or coverage for this mailbox. Attempt ordering is
    /// recorded separately before network work so retries rotate fairly.
    /// Body-cache puts happen eagerly
    /// during fetch because the cache is rebuildable and deduplicated by stable
    /// id. EXAMINE only, so sync never sets `\Seen`.
    pub(super) async fn refresh_mailbox_with(
        &self,
        session: &mut dyn ImapSession,
        entry: &plan::PlanEntry,
        force_sweep: bool,
    ) -> ProviderResult<u64> {
        let mailbox = entry.mailbox.as_str();
        // INBOX honors the provider's `limits` knob (tests shrink the window
        // there); every other mailbox uses its plan class's limits, with an
        // optional test override of the window ceiling (eviction semantics
        // preserved).
        let limits = if entry.is_inbox {
            self.limits
        } else {
            let mut limits = SyncLimits::for_class(entry.window_class);
            if let Some(window) = self.folder_window_override {
                limits.mailbox_window = window;
            }
            limits
        };
        // A thread is hot if it has a location in INBOX or Sent.
        let makes_hot = entry.is_inbox || matches!(entry.role, Some(super::MailboxRole::Sent));
        // Sent is acquired in CHUNKS (item 2): newest-first, at most
        // SENT_ROUND_MESSAGES still-unacquired UIDs per round, with a NULL/UID
        // `backfill_low_uid` watermark marking completeness. It is non-evicting
        // (SyncLimits::sent) and its backfill-in-progress state must defeat the
        // cheap cadence skip below.
        let is_sent = matches!(entry.role, Some(super::MailboxRole::Sent));

        // 1. EXAMINE (read-only) — never SELECT, so sync never sets \Seen.
        let status = session.examine(mailbox).await?;
        let server_uidvalidity = status.uid_validity.unwrap_or(0) as i64;
        let server_exists = status.exists as i64;
        let server_uidnext = status.uid_next.unwrap_or(0) as i64;

        // Cadence gating for a non-INBOX mailbox: if the EXAMINE counters are
        // unchanged AND the periodic sweep is not yet due, skip the UID
        // SEARCH/FETCH entirely. INBOX always sweeps (force_sweep). A
        // UIDVALIDITY change is NEVER skipped — it invalidates every local UID.
        // A Sent mailbox whose backfill is still INCOMPLETE is NEVER skipped
        // either (item 2): EXISTS/UIDNEXT unchanged does not mean the window is
        // fully acquired, so there is still a chunk to pull this poll.
        let now = (self.now)();
        // The COVERAGE EPOCH this successful visit belongs to (Slice 5b-2):
        // stamped into `visited_in_epoch` so coverage can be judged ACROSS
        // polls. Stable during a poll — only the end-of-poll
        // `advance_coverage_epoch_if_complete` changes it.
        let epoch = self.store.complete_walks().map_err(db_err)? as i64;
        let local_uidvalidity_pre = self
            .store
            .locations_in_mailbox(mailbox)
            .map_err(db_err)?
            .first()
            .map(|l| l.uidvalidity);
        let uidvalidity_changed =
            matches!(local_uidvalidity_pre, Some(stored) if stored != server_uidvalidity);
        let stored = self.store.mailbox_sync_state(mailbox).map_err(db_err)?;
        let backfill_incomplete = is_sent
            && policy::sent_backfill_incomplete(
                self.store.sent_backfill_low_uid(mailbox).map_err(db_err)?,
            );
        // A mailbox with a TTL-EXPIRED search pin must NOT be cheap-skipped:
        // the pin's release (and the possible aging-out of its now-unprotected
        // location) is work this poll owes even when EXISTS/UIDNEXT are
        // unchanged. Mirrors the Sent-backfill-incomplete defeat of the gate.
        let has_expired_pins = !uidvalidity_changed
            && !self
                .store
                .expired_pins(mailbox, server_uidvalidity, now - policy::SEARCH_PIN_TTL)
                .map_err(db_err)?
                .is_empty();
        let sweep_due = match stored {
            Some((_, _, last_sweep_at)) => policy::folder_sweep_due(last_sweep_at, now),
            None => true, // never synced -> always sweep
        };
        let counters_unchanged = match stored {
            Some((last_exists, last_uidnext, _)) => policy::folder_counters_unchanged(
                last_exists,
                last_uidnext,
                server_exists,
                server_uidnext,
            ),
            None => false,
        };
        if !force_sweep
            && !uidvalidity_changed
            && !backfill_incomplete
            && !has_expired_pins
            && counters_unchanged
            && !sweep_due
        {
            // Nothing to do: record the (unchanged) counters WITHOUT bumping
            // the generation, keeping last_sweep_at as stored.
            let last_sweep_at = stored.map(|(_, _, s)| s).unwrap_or(now);
            let mut round = SyncRoundWrite::default();
            // A cadence-gate no-op still COUNTS as a visit: stamp
            // last_visited_at=now AND visited_in_epoch=epoch so the fair
            // scheduler rotates this mailbox forward and the coverage epoch
            // counts it as visited, even though no full sweep ran (item 2/4).
            round.mailbox_state =
                Some((mailbox.to_string(), server_exists, server_uidnext, last_sweep_at, now, epoch));
            return self.store.commit_sync_round(&round).map_err(db_err);
        }
        // A full sweep is happening now: advance the sweep clock when due (or
        // when forced for a non-INBOX mailbox); INBOX does not track a sweep
        // clock meaningfully but recording `now` is harmless.
        let new_last_sweep_at = if sweep_due || force_sweep {
            now
        } else {
            stored.map(|(_, _, s)| s).unwrap_or(now)
        };

        // 2. Local view: stored UIDVALIDITY + per-UID flags for this mailbox.
        let local_locations = self.store.locations_in_mailbox(mailbox).map_err(db_err)?;
        let local_uidvalidity = local_locations.first().map(|l| l.uidvalidity);

        // 3. UID SEARCH ALL — the server's full UID set for this mailbox.
        let server_uids = session.uid_search("ALL").await?;

        let local_view = self.local_view(local_uidvalidity, &local_locations);
        let mut flag_uids: std::collections::BTreeSet<u32> =
            policy::select_window(server_uids.clone(), limits.mailbox_window)
                .in_window
                .into_iter()
                .collect();
        // Non-evicting mailboxes keep acquired locations beyond the acquisition
        // window. Continue refreshing their flags while they remain on the
        // server, but never carry UIDs across a UIDVALIDITY reset.
        if !limits.evicts && !uidvalidity_changed {
            let live_uids: std::collections::BTreeSet<u32> = server_uids.iter().copied().collect();
            flag_uids.extend(
                local_view
                    .flags_by_uid
                    .keys()
                    .filter(|uid| live_uids.contains(uid))
                    .copied(),
            );
        }

        // SEARCH PINS (schema v70, Phase 5 run 3). A pinned location is a
        // search-acquired message that is usually OLD — outside this evicting
        // mailbox's window — and must NOT age out. The pinned set is passed to
        // the delta so a pinned out-of-window UID is retained (deleted only
        // when the SERVER expunged it), and pinned UIDs still on the server get
        // their flags refreshed like a non-evicting mailbox's retained UIDs.
        //
        // On a UIDVALIDITY reset the old incarnation's pins are meaningless
        // (its UIDs are renumbered); they are dropped alongside the locations,
        // and the EFFECTIVE pin set for this round's delta is empty.
        //
        // TTL (SEARCH_PIN_TTL): a pin older than the TTL is RELEASED this round
        // and, once released, no longer protects its UID — it drops from the
        // effective set, so an out-of-window evicting location ages out
        // normally. Re-acquiring (the acquire path) refreshes `pinned_at`.
        let mut pin_releases: Vec<(String, i64, i64)> = Vec::new();
        let effective_pins: std::collections::BTreeSet<u32> = if uidvalidity_changed {
            // Reset: drop every pin under the OLD incarnation. The local UID's
            // uidvalidity (what we stored) is the incarnation whose pins die.
            if let Some(old_uidvalidity) = local_uidvalidity {
                for uid in self
                    .store
                    .pinned_uids(mailbox, old_uidvalidity)
                    .map_err(db_err)?
                {
                    pin_releases.push((mailbox.to_string(), old_uidvalidity, uid as i64));
                }
            }
            std::collections::BTreeSet::new()
        } else {
            let now_secs = now;
            let cutoff = now_secs - policy::SEARCH_PIN_TTL;
            let expired: std::collections::BTreeSet<u32> = self
                .store
                .expired_pins(mailbox, server_uidvalidity, cutoff)
                .map_err(db_err)?
                .into_iter()
                .collect();
            for uid in &expired {
                pin_releases.push((mailbox.to_string(), server_uidvalidity, *uid as i64));
            }
            let all_pins: std::collections::BTreeSet<u32> = self
                .store
                .pinned_uids(mailbox, server_uidvalidity)
                .map_err(db_err)?
                .into_iter()
                .collect();
            // Effective = pinned AND not expired. Refresh flags for every live
            // effective pin so a flag-only change on a pinned message is seen.
            let live_server: std::collections::BTreeSet<u32> =
                server_uids.iter().copied().collect();
            let effective: std::collections::BTreeSet<u32> =
                all_pins.difference(&expired).copied().collect();
            flag_uids.extend(effective.iter().filter(|uid| live_server.contains(uid)).copied());
            effective
        };

        let server_flags = self
            .fetch_flags(session, &flag_uids.into_iter().collect::<Vec<_>>())
            .await?;
        let server_view = super::delta::ServerMailboxView {
            uidvalidity: server_uidvalidity,
            all_uids: server_uids.clone(),
            flags_by_uid: server_flags,
        };
        let mut delta =
            super::delta::compute_delta(&local_view, &server_view, &limits, &effective_pins);

        if delta.beyond_window > 0 {
            log::info!(
                "imap sync: {} {mailbox} messages beyond the sync window",
                delta.beyond_window
            );
        }

        // Sent chunked acquisition (item 2). `delta.new_uids` is every
        // still-unacquired in-window UID (ascending). For Sent we take only the
        // newest SENT_ROUND_MESSAGES this round and defer the rest, so a 5,000-
        // message Sent folder fills over many polls instead of one giant round.
        // Flag changes and deletions for already-acquired messages are NOT
        // budgeted — only the new-mail acquisition is chunked. Progress is
        // crash-safe because "still unacquired" is derived from local
        // locations every round: an interrupted round commits nothing (the
        // whole round is one transaction), so it loses nothing and the next
        // round recomputes the same remaining set and acquires nothing twice.
        //
        // The watermark (`backfill_low_uid`) is the lowest UID acquired while
        // the backfill is INCOMPLETE, NULL once complete. "Complete" = after
        // this chunk there is no still-unacquired in-window UID left AND none
        // fell beyond the window unseen. We record it in the SAME round, so the
        // cadence gate above reads an accurate pending flag next poll.
        let mut sent_backfill_write: Option<Option<i64>> = None;
        if is_sent {
            let budget = self.sent_round_budget.unwrap_or(policy::SENT_ROUND_MESSAGES);
            let acquiring_total = delta.new_uids.len();
            if acquiring_total > budget {
                // Keep the NEWEST (highest) `budget` UIDs; the ascending tail
                // is the newest.
                let drop_count = acquiring_total - budget;
                delta.new_uids.drain(0..drop_count);
            }
            // After this chunk, is anything still unacquired in-window? If this
            // round takes every remaining new UID, the backfill is complete.
            let remaining_after_chunk = acquiring_total.saturating_sub(delta.new_uids.len());
            if remaining_after_chunk == 0 {
                // Nothing left to acquire in-window: backfill complete (NULL).
                sent_backfill_write = Some(None);
            } else {
                // Still acquiring: the watermark is the lowest UID acquired SO
                // FAR, i.e. min(previous watermark, lowest UID in this chunk).
                let chunk_low = delta.new_uids.first().copied().map(|u| u as i64);
                // On a UIDVALIDITY reset the old incarnation's watermark is
                // meaningless (locations were dropped, UIDs renumbered), so the
                // backfill restarts from this chunk's low UID.
                let prior = if delta.uidvalidity_reset {
                    None
                } else {
                    self.store.sent_backfill_low_uid(mailbox).map_err(db_err)?
                };
                let low = match (prior, chunk_low) {
                    (Some(p), Some(c)) => Some(p.min(c)),
                    (Some(p), None) => Some(p),
                    (None, c) => c,
                };
                // A pending backfill always has a non-NULL marker so the gate
                // keeps re-running; fall back to UIDNEXT if we somehow have no
                // UID yet (empty chunk but work remains — shouldn't happen).
                sent_backfill_write = Some(Some(low.unwrap_or(server_uidnext.max(1))));
            }
            if delta.beyond_window > 0 {
                log::info!(
                    "imap sync: Sent backfill — {} UID(s) beyond the {}-message window will not be acquired",
                    delta.beyond_window,
                    limits.mailbox_window
                );
            }
            log::info!(
                "imap sync: Sent backfill — acquiring {} this round, {} still pending",
                delta.new_uids.len(),
                remaining_after_chunk
            );
        }

        let mut round = SyncRoundWrite::default();
        round.mailbox_state =
            Some((mailbox.to_string(), server_exists, server_uidnext, new_last_sweep_at, now, epoch));
        // Sent backfill watermark, written in the SAME transaction as this
        // round's acquired chunk so progress is exactly as crash-safe as the
        // locations it accompanies (item 2). `None` for every non-Sent mailbox.
        round.sent_backfill_low_uid = sent_backfill_write;

        // Search-pin releases decided above (TTL-expired pins, or every pin of
        // the old incarnation on a UIDVALIDITY reset), committed in this same
        // round's transaction so a released pin never outlives its decision.
        round.pin_releases.append(&mut pin_releases);

        // On a UIDVALIDITY reset the local UIDs are meaningless: drop every
        // location IN THIS MAILBOX only (bodies survive — keyed by stable id,
        // and other mailboxes' locations and message->thread mappings are
        // untouched). The reset's "new" set is the whole windowed server view.
        let dropped_message_ids: Vec<String> = if delta.uidvalidity_reset {
            round.deletions = local_locations
                .iter()
                .map(|l| (l.mailbox.clone(), l.uidvalidity, l.uid))
                .collect();
            local_locations.iter().map(|l| l.message_id.clone()).collect()
        } else {
            Vec::new()
        };

        let mut threading_inputs: Vec<ThreadingInput> = Vec::new();
        let mut changed_message_ids: Vec<String> = Vec::new();
        // Message ids newly located in this mailbox this round (new mail), used
        // to compute hotness and label transitions.
        let mut new_message_ids: Vec<String> = Vec::new();

        // New mail: identity pass + (for hot mailboxes) body fetch, threading
        // inputs, and location rows. Factored into `ingest_new_uids` so the
        // explicit-UID acquire path (`acquire_uids`, Phase 5 run 3) runs the
        // byte-identical ingest for a search's UID list; `refresh_mailbox_with`
        // keeps calling it with `delta.new_uids`, so the existing sync tests
        // (the contract) are unchanged.
        let ingest = self
            .ingest_new_uids(session, mailbox, server_uidvalidity, &delta.new_uids, makes_hot)
            .await?;
        round.locations.extend(ingest.locations);
        threading_inputs.extend(ingest.threading_inputs);
        changed_message_ids.extend(ingest.changed_message_ids);
        new_message_ids.extend(ingest.new_message_ids);

        // Flag changes: collect an updated location row (not applied yet).
        if !delta.uidvalidity_reset {
            for &uid in &delta.flag_changed_uids {
                if let Some(flags) = server_view.flags_by_uid.get(&uid) {
                    if let Some(message_id) = self
                        .store
                        .location_message_id(mailbox, server_uidvalidity, uid as i64)
                        .map_err(db_err)?
                    {
                        round.locations.push(ImapLocation {
                            mailbox: mailbox.to_string(),
                            uidvalidity: server_uidvalidity,
                            uid: uid as i64,
                            message_id: message_id.clone(),
                            flags_json: serde_json::to_string(flags)
                                .unwrap_or_else(|_| "[]".to_string()),
                            modseq: None,
                        });
                        changed_message_ids.push(message_id);
                    }
                }
            }

            // Deletions: collect the location coordinate to delete.
            for &uid in &delta.deleted_uids {
                if let Some(message_id) = self
                    .store
                    .location_message_id(mailbox, server_uidvalidity, uid as i64)
                    .map_err(db_err)?
                {
                    round
                        .deletions
                        .push((mailbox.to_string(), server_uidvalidity, uid as i64));
                    changed_message_ids.push(message_id);
                }
                // A pinned UID that the delta reports deleted was expunged on
                // the server (a pin never ages out of the window). Release its
                // pin in the same round so a stale pin cannot resurrect it.
                if effective_pins.contains(&uid) {
                    round
                        .pin_releases
                        .push((mailbox.to_string(), server_uidvalidity, uid as i64));
                }
            }
        }

        // Thread the new messages, seeding prior thread state from PERSISTED
        // tokens (never cached bodies), so index-tier messages thread fine.
        let outcome = if threading_inputs.is_empty() {
            threading::ThreadingOutcome::default()
        } else {
            self.ensure_tokens_backfilled()?;
            let batch_tokens: Vec<String> = threading_inputs
                .iter()
                .flat_map(|input| input.token_set())
                .collect();
            let mut thread_state = self.load_thread_state_from_tokens(&batch_tokens)?;
            threading::thread_batch(&mut thread_state, &threading_inputs)
        };

        // Resolve each message's thread id after this round's assignments, so
        // hotness and journaling see the final grouping.
        let mut assignment_of: std::collections::BTreeMap<String, String> =
            std::collections::BTreeMap::new();
        for (message_id, thread_id) in &outcome.assignments {
            assignment_of.insert(message_id.clone(), thread_id.clone());
        }

        // Changed thread ids: threading effects, plus the threads of any
        // flag-changed / deleted / dropped message.
        let mut changed_threads: std::collections::BTreeSet<String> =
            outcome.changed_threads.iter().cloned().collect();
        for message_id in changed_message_ids.iter().chain(dropped_message_ids.iter()) {
            if let Some(thread) = self.store.thread_of_message(message_id).map_err(db_err)? {
                changed_threads.insert(thread);
            }
            if let Some(thread) = assignment_of.get(message_id) {
                changed_threads.insert(thread.clone());
            }
        }
        for (_, thread_id) in &outcome.assignments {
            changed_threads.insert(thread_id.clone());
        }

        // Hotness (item 5): a thread with a NEW location in INBOX/Sent is hot.
        // Resolve each new message to its (post-assignment) thread id. A merge
        // survivor is carried across aliases by commit_sync_round's resolver.
        let mut hot_threads: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        if makes_hot {
            for message_id in &new_message_ids {
                let thread = match assignment_of.get(message_id) {
                    Some(thread) => Some(thread.clone()),
                    None => self.store.thread_of_message(message_id).map_err(db_err)?,
                };
                if let Some(thread) = thread {
                    hot_threads.insert(thread);
                }
            }
        }

        round.thread_assignments = outcome.assignments;
        round.aliases = outcome.aliases;
        round.message_tokens = threading_inputs
            .iter()
            .map(|input| (input.message_id.clone(), input.token_set()))
            .collect();
        round.hot_threads = hot_threads.into_iter().collect();

        // Journaling (item 5): only HOT threads are reported to the engine. A
        // changed thread that is not hot (and does not become hot this round)
        // is recorded but NOT journaled, so a Trash/Junk-only thread never
        // reaches the engine. A thread made hot THIS round is journaled.
        let newly_hot: std::collections::BTreeSet<&String> = round.hot_threads.iter().collect();
        let mut journaled: Vec<String> = Vec::new();
        for thread_id in &changed_threads {
            let resolved = self.store.resolve_thread_alias(thread_id).map_err(db_err)?;
            let already_hot = self.store.is_thread_hot(&resolved).map_err(db_err)?;
            if already_hot || newly_hot.contains(&resolved) || newly_hot.contains(thread_id) {
                journaled.push(thread_id.clone());
            }
        }
        round.changed_threads = journaled;

        // Idle round: nothing content-changed. Still persist cadence counters
        // (item 4) without bumping the generation. commit_sync_round handles
        // the mailbox_state write on an empty round.
        let generation = self.store.commit_sync_round(&round).map_err(db_err)?;
        self.store.prune_journal().map_err(db_err)?;
        Ok(generation)
    }

    /// Ingest a list of UIDs in one mailbox: for each UID run the SAME identity
    /// pass → (hot: body fetch) → threading-input derivation → location row the
    /// live-sync new-mail loop runs, in UID batches. Returns the accumulated
    /// location rows, threading inputs and message-id lists for the caller to
    /// fold into its round. Does NOT touch the round, the delta or the store's
    /// generation — it is the pure ingest step shared by `refresh_mailbox_with`
    /// (driven by `delta.new_uids`) and `acquire_uids` (driven by a search's
    /// explicit UID list), so both behave byte-identically. `makes_hot` selects
    /// the hot (identity+body) vs index-tier (headers only) path exactly as the
    /// live loop did.
    async fn ingest_new_uids(
        &self,
        session: &mut dyn ImapSession,
        mailbox: &str,
        server_uidvalidity: i64,
        uids: &[u32],
        makes_hot: bool,
    ) -> ProviderResult<IngestOutcome> {
        let mut outcome = IngestOutcome::default();
        for batch in chunk(uids, policy::UID_BATCH_SIZE) {
            let set = uid_set(batch);
            if makes_hot {
                // Hot mailbox: identity pass + body once (deferred location).
                let rows = fetch_identity(session, &set).await?;
                for row in rows {
                    let mut fetcher = SessionBodyFetcher { session };
                    let resolved = super::fetch::resolve_and_cache_body(
                        &mut fetcher,
                        &self.store,
                        &self.cache,
                        mailbox,
                        server_uidvalidity,
                        &row,
                        (self.now)(),
                    )
                    .await?;
                    let message_id = resolved.message_id;
                    outcome.locations.push(ImapLocation {
                        mailbox: mailbox.to_string(),
                        uidvalidity: server_uidvalidity,
                        uid: row.uid as i64,
                        message_id: message_id.clone(),
                        flags_json: serde_json::to_string(&row.flags)
                            .unwrap_or_else(|_| "[]".to_string()),
                        modseq: None,
                    });
                    let headers = if resolved.body_skipped {
                        super::rfc822::ThreadingHeaders {
                            message_id: row.inputs.message_id.clone(),
                            ..Default::default()
                        }
                    } else {
                        match self.cache.get(&message_id).map_err(db_err)? {
                            Some(cached) => super::rfc822::threading_headers_from_raw(&cached.raw),
                            None => super::rfc822::ThreadingHeaders {
                                message_id: row.inputs.message_id.clone(),
                                ..Default::default()
                            },
                        }
                    };
                    outcome.threading_inputs.push(ThreadingInput {
                        message_id: message_id.clone(),
                        message_id_header: headers
                            .message_id
                            .or_else(|| row.inputs.message_id.clone()),
                        in_reply_to: headers.in_reply_to,
                        references: headers.references,
                    });
                    outcome.changed_message_ids.push(message_id.clone());
                    outcome.new_message_ids.push(message_id);
                }
            } else {
                // Index-tier mailbox: INDEX identity pass (headers carry the
                // threading triple); NO body fetched. The stable id is derived
                // the same way, so a message in INBOX and in Trash is ONE id.
                let rows = super::fetch::fetch_identity_index(session, &set).await?;
                for (row, threading) in rows {
                    let message_id = {
                        // Sticky id: an id already recorded for this coordinate
                        // wins; otherwise derive it from the identity inputs.
                        match self
                            .store
                            .location_message_id(mailbox, server_uidvalidity, row.uid as i64)
                            .map_err(db_err)?
                        {
                            Some(existing) => existing,
                            None => super::identity::derive_message_id(
                                self.store.account_id(),
                                &row.inputs,
                            ),
                        }
                    };
                    outcome.locations.push(ImapLocation {
                        mailbox: mailbox.to_string(),
                        uidvalidity: server_uidvalidity,
                        uid: row.uid as i64,
                        message_id: message_id.clone(),
                        flags_json: serde_json::to_string(&row.flags)
                            .unwrap_or_else(|_| "[]".to_string()),
                        modseq: None,
                    });
                    outcome.threading_inputs.push(ThreadingInput {
                        message_id: message_id.clone(),
                        message_id_header: threading
                            .message_id
                            .or_else(|| row.inputs.message_id.clone()),
                        in_reply_to: threading.in_reply_to,
                        references: threading.references,
                    });
                    outcome.changed_message_ids.push(message_id.clone());
                    outcome.new_message_ids.push(message_id);
                }
            }
        }
        Ok(outcome)
    }

    /// Acquire an EXPLICIT list of UIDs in one mailbox and PIN them, for the
    /// server-search ingest path (Phase 5 run 3; run 4 is the only caller).
    ///
    /// A search hit is usually OLD — outside the mailbox's newest-N sync window
    /// — so acquiring it the way `refresh_mailbox_with` acquires new mail is not
    /// enough: the next poll's delta would age the out-of-window location out of
    /// an evicting mailbox and delete it. So every UID this acquires is also
    /// PINNED (schema v70): the delta then retains it until the SERVER expunges
    /// it or the pin's TTL releases it. Acquired threads are marked HOT so the
    /// engine ingests them (only hot threads are journaled) and the emptied-
    /// thread grace protects them if a pinned location later vanishes; hotness
    /// adds no INBOX label (labels come from the location's mailbox, not from
    /// hotness), so INBOX semantics stay correct — an Archive hit is labelled
    /// `folder:Archive`, never INBOX, and never appears in `list_inbox`.
    ///
    /// Steps, all in ONE transaction (a failure before commit writes nothing):
    ///   1. EXAMINE (read-only — never SELECT, so acquiring never sets `\Seen`).
    ///   2. Verify the server's UIDVALIDITY matches the caller's expectation
    ///      (the incarnation its UID list is addressed under); on a mismatch
    ///      REFUSE with [`AcquireError::UidValidityMismatch`] rather than
    ///      ingesting under the wrong incarnation.
    ///   3. Skip UIDs already located locally (no duplicate message/location),
    ///      but still PIN them (idempotently refreshing an existing pin's TTL).
    ///   4. Fetch + thread the rest (via the shared `ingest_new_uids`).
    ///   5. Enforce [`policy::MAX_SEARCH_PINS`] per account: pin up to the cap,
    ///      NEWEST (highest UID) first, and REFUSE the over-cap UIDs — reported
    ///      in [`AcquireOutcome::skipped_over_cap`], never silently dropped and
    ///      never evicting an existing pin. A refused UID is NOT acquired at all
    ///      (acquiring an unpinned out-of-window hit would just let it age out
    ///      next poll, which defeats the purpose), so partial acceptance is the
    ///      documented behaviour.
    ///
    /// Returns the affected thread ids (resolved through merge aliases) so the
    /// caller can report them. Bodies may be cached eagerly during the fetch
    /// (the body cache is rebuildable and keyed by stable id), exactly as the
    /// live sync does — only the durable location/thread/pin/hot/journal writes
    /// are deferred to the atomic commit.
    pub(super) async fn acquire_uids(
        &self,
        session: &mut dyn ImapSession,
        entry: &plan::PlanEntry,
        uids: &[u32],
        expected_uidvalidity: i64,
    ) -> Result<AcquireOutcome, AcquireError> {
        let mailbox = entry.mailbox.as_str();

        // 0. Only a mailbox the sync visits can hold a pin: TTL release, flag
        // refresh and expunge detection all run inside that mailbox's sync
        // round. A pin in a mailbox outside the sync cap would never be
        // maintained or released, so refuse before any network work.
        if !entry.synced_now {
            return Err(AcquireError::MailboxNotSynced {
                mailbox: mailbox.to_string(),
            });
        }

        // 1. EXAMINE (read-only).
        let status = session.examine(mailbox).await.map_err(AcquireError::Provider)?;
        let server_uidvalidity = status.uid_validity.unwrap_or(0) as i64;

        // 2. UIDVALIDITY guard: refuse rather than ingest under the wrong
        // incarnation. The caller's UID list is meaningful only under the
        // incarnation it was searched in.
        if server_uidvalidity != expected_uidvalidity {
            return Err(AcquireError::UidValidityMismatch {
                mailbox: mailbox.to_string(),
                expected: expected_uidvalidity,
                found: server_uidvalidity,
            });
        }

        let now = (self.now)();
        // Acquired threads are hot (see the doc comment): the search ingest
        // path always marks hot so the engine ingests the thread and the
        // emptied-thread grace protects it. This is independent of the
        // mailbox's role — an Archive hit is still hot (but unlabelled INBOX).
        let makes_hot = true;
        // Whether to fetch bodies at ingest: a message we will show needs its
        // body, so the acquire path always fetches like a hot mailbox.
        let fetch_bodies = true;

        // 3. Partition the requested UIDs into already-local (skip the fetch,
        // but still pin) and to-fetch. Dedup and sort so "newest first" for the
        // cap is well-defined.
        let mut unique: Vec<u32> = uids.to_vec();
        unique.sort_unstable();
        unique.dedup();

        let mut already_local: Vec<u32> = Vec::new();
        let mut to_fetch: Vec<u32> = Vec::new();
        for &uid in &unique {
            let located = self
                .store
                .location_message_id(mailbox, server_uidvalidity, uid as i64)
                .map_err(|e| AcquireError::Provider(db_err(e)))?
                .is_some();
            if located {
                already_local.push(uid);
            } else {
                to_fetch.push(uid);
            }
        }

        // 5 (cap, applied before the fetch so a refused UID is never fetched).
        // A UID already pinned refreshes in place and does NOT count against
        // new-pin room; count only the requested UIDs that are not already
        // pinned as "new". Accept NEWEST first.
        let mut new_pin_candidates: Vec<u32> = Vec::new();
        let mut refreshing: Vec<u32> = Vec::new();
        for &uid in &unique {
            if self
                .store
                .is_pinned(mailbox, server_uidvalidity, uid as i64)
                .map_err(|e| AcquireError::Provider(db_err(e)))?
            {
                refreshing.push(uid);
            } else {
                new_pin_candidates.push(uid);
            }
        }
        let existing_total = self
            .store
            .search_pin_count()
            .map_err(|e| AcquireError::Provider(db_err(e)))?;
        let cap = policy::cap_search_pins(existing_total, new_pin_candidates.len());
        // Newest-first acceptance: keep the highest `accepted` UIDs among the
        // new candidates; the rest are refused.
        new_pin_candidates.sort_unstable();
        let refused_count = cap.refused;
        let accepted_new: std::collections::BTreeSet<u32> = new_pin_candidates
            .iter()
            .rev()
            .take(cap.accepted)
            .copied()
            .collect();
        // The UIDs this acquisition will PIN: every refresh (already pinned)
        // plus the accepted new ones. A refused new UID is pinned by neither,
        // and is not fetched/located either.
        let pinned_now: std::collections::BTreeSet<u32> = refreshing
            .iter()
            .copied()
            .chain(accepted_new.iter().copied())
            .collect();

        // The UIDs we will actually fetch bodies for: to-fetch UIDs that are
        // being pinned this round (a refused UID is skipped entirely).
        let fetch_set: Vec<u32> = to_fetch
            .iter()
            .copied()
            .filter(|uid| pinned_now.contains(uid))
            .collect();

        // 4. Fetch + thread the to-acquire set via the shared ingest step.
        let ingest = self
            .ingest_new_uids(session, mailbox, server_uidvalidity, &fetch_set, fetch_bodies)
            .await
            .map_err(AcquireError::Provider)?;

        let mut round = SyncRoundWrite {
            locations: ingest.locations,
            ..Default::default()
        };

        // Pin every UID we are keeping (refresh or newly accepted), at `now`,
        // but ONLY if it has a location after this round: an already-local
        // UID, or one the fetch actually returned. A UID expunged between the
        // caller's SEARCH and this FETCH yields no location, and a pin without
        // a location is never released by the delta (which only deletes local
        // UIDs), so it would sit in the cap until its TTL.
        let located_after: std::collections::BTreeSet<u32> = already_local
            .iter()
            .copied()
            .chain(round.locations.iter().map(|l| l.uid as u32))
            .collect();
        for &uid in pinned_now.intersection(&located_after) {
            round
                .pin_upserts
                .push((mailbox.to_string(), server_uidvalidity, uid as i64, now));
        }

        // Thread the newly-fetched messages, seeding prior thread state from
        // PERSISTED tokens, exactly as the live loop does.
        let outcome = if ingest.threading_inputs.is_empty() {
            threading::ThreadingOutcome::default()
        } else {
            self.ensure_tokens_backfilled().map_err(AcquireError::Provider)?;
            let batch_tokens: Vec<String> = ingest
                .threading_inputs
                .iter()
                .flat_map(|input| input.token_set())
                .collect();
            let mut thread_state = self
                .load_thread_state_from_tokens(&batch_tokens)
                .map_err(AcquireError::Provider)?;
            threading::thread_batch(&mut thread_state, &ingest.threading_inputs)
        };

        let mut assignment_of: std::collections::BTreeMap<String, String> =
            std::collections::BTreeMap::new();
        for (message_id, thread_id) in &outcome.assignments {
            assignment_of.insert(message_id.clone(), thread_id.clone());
        }

        // Affected threads: every acquired message's thread, plus every
        // already-local pinned UID's thread (so re-pinning an existing hit also
        // keeps/affirms its thread hot), resolved through aliases.
        let mut affected_threads: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        for message_id in &ingest.new_message_ids {
            let thread = match assignment_of.get(message_id) {
                Some(t) => Some(t.clone()),
                None => self
                    .store
                    .thread_of_message(message_id)
                    .map_err(|e| AcquireError::Provider(db_err(e)))?,
            };
            if let Some(t) = thread {
                affected_threads.insert(t);
            }
        }
        // Already-local pinned UIDs: resolve their message -> thread so a
        // re-pin affirms hotness and journals the thread.
        for &uid in &already_local {
            if !pinned_now.contains(&uid) {
                continue;
            }
            if let Some(message_id) = self
                .store
                .location_message_id(mailbox, server_uidvalidity, uid as i64)
                .map_err(|e| AcquireError::Provider(db_err(e)))?
            {
                if let Some(t) = self
                    .store
                    .thread_of_message(&message_id)
                    .map_err(|e| AcquireError::Provider(db_err(e)))?
                {
                    affected_threads.insert(t);
                }
            }
        }
        for (_, thread_id) in &outcome.assignments {
            affected_threads.insert(thread_id.clone());
        }

        round.thread_assignments = outcome.assignments;
        round.aliases = outcome.aliases;
        round.message_tokens = ingest
            .threading_inputs
            .iter()
            .map(|input| (input.message_id.clone(), input.token_set()))
            .collect();

        // Mark every affected thread hot (the acquire decision) and journal it
        // so the engine ingests it. Resolve through aliases to the survivor.
        let mut hot_threads: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        if makes_hot {
            for thread_id in &affected_threads {
                let resolved = self
                    .store
                    .resolve_thread_alias(thread_id)
                    .map_err(|e| AcquireError::Provider(db_err(e)))?;
                hot_threads.insert(resolved);
            }
        }
        round.hot_threads = hot_threads.into_iter().collect();
        // Journal every affected thread (resolved) so the engine re-ingests the
        // acquired hit. Unlike the sync loop, the acquire path journals ALL
        // affected threads: they are all made hot here, so all are reportable.
        let mut journaled: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        for thread_id in &affected_threads {
            journaled.insert(thread_id.clone());
            let resolved = self
                .store
                .resolve_thread_alias(thread_id)
                .map_err(|e| AcquireError::Provider(db_err(e)))?;
            journaled.insert(resolved);
        }
        round.changed_threads = journaled.into_iter().collect();

        let generation = self
            .store
            .commit_sync_round(&round)
            .map_err(|e| AcquireError::Provider(db_err(e)))?;
        self.store
            .prune_journal()
            .map_err(|e| AcquireError::Provider(db_err(e)))?;

        // The affected thread ids (resolved) for the caller to report.
        let mut affected: Vec<String> = Vec::new();
        for thread_id in &affected_threads {
            let resolved = self
                .store
                .resolve_thread_alias(thread_id)
                .map_err(|e| AcquireError::Provider(db_err(e)))?;
            if !affected.contains(&resolved) {
                affected.push(resolved);
            }
        }
        affected.sort();

        Ok(AcquireOutcome {
            generation,
            affected_threads: affected,
            skipped_over_cap: refused_count,
        })
    }

    fn local_view(
        &self,
        uidvalidity: Option<i64>,
        locations: &[ImapLocation],
    ) -> super::delta::LocalMailboxView {
        let mut flags_by_uid = std::collections::BTreeMap::new();
        for location in locations {
            let flags: Vec<String> =
                serde_json::from_str(&location.flags_json).unwrap_or_default();
            flags_by_uid.insert(location.uid as u32, flags);
        }
        super::delta::LocalMailboxView {
            uidvalidity,
            flags_by_uid,
        }
    }

    /// Fetch FLAGS for a UID set, batched, returning uid -> wire flags. Uses
    /// the FLAGS-ONLY fetch item (no body, no headers, no size), so the
    /// per-poll flag sweep over the window is cheap even at the ceiling.
    async fn fetch_flags(
        &self,
        session: &mut dyn ImapSession,
        uids: &[u32],
    ) -> ProviderResult<std::collections::BTreeMap<u32, Vec<String>>> {
        let mut out = std::collections::BTreeMap::new();
        for batch in chunk(uids, policy::UID_BATCH_SIZE) {
            if batch.is_empty() {
                continue;
            }
            let set = uid_set(batch);
            for (uid, flags) in super::fetch::fetch_flags_only(session, &set).await? {
                out.insert(uid, flags);
            }
        }
        Ok(out)
    }

    /// Seed the threader's prior state from PERSISTED tokens — never from
    /// cached bodies. Given the tokens the incoming batch carries, the store
    /// finds existing messages that share any token, resolves their threads
    /// through aliases, and returns each seeded thread's full token set plus
    /// its persisted creation generation. Only the threads the batch can
    /// actually touch are seeded, so cost scales with the batch, not with all
    /// mail, and an index-tier message with no cached body threads fine.
    ///
    /// `thread_state_loads` still counts a real load here (an idle round
    /// skips it, as before), so the 75f2704 counter test stays meaningful.
    fn load_thread_state_from_tokens(&self, batch_tokens: &[String]) -> ProviderResult<ThreadState> {
        self.thread_state_loads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut state = ThreadState::new();
        let seeded = self.store.seed_state_for_tokens(batch_tokens).map_err(db_err)?;
        for (thread_id, (created_generation, tokens)) in seeded {
            state.seed(&thread_id, created_generation, &tokens);
        }
        Ok(state)
    }

    /// Run the one-time, idempotent, bounded-batch, crash-resumable upgrade
    /// backfill that fills `imap_message_tokens` for threads that predate v59
    /// (a database with threaded mail from 0.94.x has no token rows). This is
    /// the ONLY place that still parses cached bodies. A message with no cached
    /// body gets only its stable-id token (matching c77fa3f's absent-body
    /// behaviour). It runs just before the first threading that needs tokens;
    /// a new account is already marked done, so this returns immediately.
    ///
    /// Each message's tokens are written in their own transaction and the
    /// completion flag is set only after the queue drains, so a crash mid-way
    /// leaves every finished message whole and the next run resumes from the
    /// messages still missing tokens.
    fn ensure_tokens_backfilled(&self) -> ProviderResult<()> {
        if self.store.tokens_backfilled().map_err(db_err)? {
            return Ok(());
        }
        use super::identity::normalize_message_id;
        loop {
            let batch = self
                .store
                .messages_missing_tokens(policy::TOKEN_BACKFILL_BATCH_SIZE)
                .map_err(db_err)?;
            if batch.is_empty() {
                break;
            }
            for message_id in &batch {
                // Reconstruct this message's token set from its cached body.
                // No body (expunged, oversize-skipped, index-tier) => only the
                // stable-id anchor, exactly as c77fa3f seeded such a message.
                let tokens = match self.cache.get(message_id).map_err(db_err)? {
                    Some(cached) => {
                        let headers = super::rfc822::threading_headers_from_raw(&cached.raw);
                        let input = ThreadingInput {
                            message_id: message_id.clone(),
                            message_id_header: headers
                                .message_id
                                .as_deref()
                                .map(normalize_message_id)
                                .or_else(|| Some(message_id.clone())),
                            in_reply_to: headers.in_reply_to,
                            references: headers.references,
                        };
                        input.token_set()
                    }
                    None => vec![message_id.clone()],
                };
                self.store
                    .write_message_tokens(message_id, &tokens)
                    .map_err(db_err)?;
            }
            // A full batch that returned fewer than the limit means the queue
            // is drained; loop once more only if it was exactly the limit.
            if batch.len() < policy::TOKEN_BACKFILL_BATCH_SIZE {
                break;
            }
        }
        self.store.mark_tokens_backfilled().map_err(db_err)?;
        Ok(())
    }

    /// TEST-ONLY ORACLE: the pre-5b1 body-parsing seeder, kept verbatim so the
    /// equivalence tests can assert the token loader threads identically. It
    /// reads and parses every cached body (what 5b-1 removed from the hot
    /// path); it must never be called from production.
    #[cfg(test)]
    fn load_thread_state(&self) -> ProviderResult<ThreadState> {
        use super::identity::normalize_message_id;
        let mut state = ThreadState::new();
        let rows = self.store.all_message_threads().map_err(db_err)?;
        let mut by_thread: std::collections::BTreeMap<String, (u64, Vec<String>)> =
            std::collections::BTreeMap::new();
        for (message_id, thread_id, created) in rows {
            let entry = by_thread
                .entry(
                    self.store
                        .resolve_thread_alias(&thread_id)
                        .map_err(db_err)?,
                )
                .or_insert((created, Vec::new()));
            entry.0 = entry.0.min(created);
            entry.1.push(message_id);
        }
        for (thread_id, (created_generation, message_ids)) in by_thread {
            let mut tokens: Vec<String> = Vec::new();
            for message_id in &message_ids {
                tokens.push(message_id.clone());
                if let Some(cached) = self.cache.get(message_id).map_err(db_err)? {
                    let headers = super::rfc822::threading_headers_from_raw(&cached.raw);
                    let input = ThreadingInput {
                        message_id: message_id.clone(),
                        message_id_header: headers.message_id.clone(),
                        in_reply_to: headers.in_reply_to,
                        references: headers.references,
                    };
                    tokens.extend(input.reference_tokens().0);
                    if let Some(raw_id) = headers.message_id {
                        let normalized = normalize_message_id(&raw_id);
                        if !normalized.is_empty() {
                            tokens.push(normalized);
                        }
                    }
                }
            }
            state.seed(&thread_id, created_generation, &tokens);
        }
        Ok(state)
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

#[async_trait]
impl MailSync for ImapProvider {
    fn thread_aliases(&self) -> ProviderResult<Vec<(String, String)>> {
        self.store.thread_aliases().map_err(db_err)
    }

    async fn baseline_cursor(&self) -> ProviderResult<SyncCursor> {
        let refreshed = self.refresh_all().await?;
        // The engine seeds full recovery from list_inbox alone. Re-journal
        // every other hot thread, including acquired Sent mail from an earlier
        // interrupted baseline and threads whose last location disappeared.
        // Rebuild from durable hotness rather than old journal rows, which may
        // already have been pruned. Repeating this after a crash is harmless.
        let inbox: std::collections::BTreeSet<String> = self
            .store
            .thread_ids_in_mailbox(INBOX)
            .map_err(db_err)?
            .into_iter()
            .collect();
        let changed_threads = self
            .store
            .hot_thread_ids()
            .map_err(db_err)?
            .into_iter()
            .filter(|thread| !inbox.contains(thread))
            .collect();
        self.store
            .commit_sync_round(&SyncRoundWrite {
                changed_threads,
                ..Default::default()
            })
            .map_err(db_err)?;
        // The engine's follow-up incremental pass consumes the replay above.
        Ok(SyncCursor::from_generation(refreshed.last))
    }

    async fn poll(&self, cursor: &SyncCursor) -> ProviderResult<SyncBatch> {
        let Some(polled) = cursor.generation() else {
            // Not a generation cursor (e.g. a stale Gmail-style string): the
            // engine answers InvalidCursor with a full resync.
            return Err(ProviderError::InvalidCursor);
        };
        let before = self.store.generation().map_err(db_err)?;
        // A cursor newer than the store's generation (store reset behind the
        // engine) or older than retention cannot be answered completely.
        if !policy::journal_cursor_is_answerable(polled, before) {
            return Err(ProviderError::InvalidCursor);
        }
        // (a) refresh all synced mailboxes (INBOX first); (b) return threads
        // journalled since `polled`; (c) new cursor = current generation.
        let refreshed = self.refresh_all().await?;
        let changed = self.store.journal_since(polled).map_err(db_err)?;
        Ok(SyncBatch {
            changed_threads: changed,
            cursor: SyncCursor::from_generation(refreshed.last),
            more: false,
        })
    }

    async fn list_inbox(&self, _page: Option<&str>) -> ProviderResult<ThreadPage> {
        // A single page from the local store: thread ids with an INBOX
        // location. next = None (no server paging this slice).
        let thread_ids = self.store.thread_ids_in_mailbox(INBOX).map_err(db_err)?;
        Ok(ThreadPage {
            thread_ids,
            next: None,
        })
    }

    async fn fetch_thread(&self, id: &str) -> ProviderResult<Vec<RawMessage>> {
        // Resolve an aliased (merged-away) id to its survivor.
        let thread_id = self.store.resolve_thread_alias(id).map_err(db_err)?;
        let message_ids = self.store.messages_in_thread(&thread_id).map_err(db_err)?;

        // Does this thread have ANY live location right now? A message row can
        // outlive its last location (the location was expunged / the message
        // was moved to a mailbox not swept this poll), so "has messages" is not
        // "has a location". Zero locations is the trigger for the emptied-hot-
        // thread grace (Slice 5b-2 item 4).
        let mut has_any_location = false;
        for message_id in &message_ids {
            if !self
                .store
                .locations_for_message(message_id)
                .map_err(db_err)?
                .is_empty()
            {
                has_any_location = true;
                break;
            }
        }

        if !has_any_location {
            // The thread has no locations. If it is (or ever was) HOT, do NOT
            // report NotFound immediately: another client may have moved a hot
            // message out of INBOX into a mailbox not swept this poll, so the
            // thread momentarily has zero locations but regains one as soon as
            // the destination is visited. Return `Ok(empty)` during the grace
            // (the engine's ingest_threads applies nothing and leaves the local
            // thread, its tasks and labels untouched), and `NotFound` only once
            // EMPTIED_THREAD_GRACE_WALKS completed COVERAGE EPOCHS have passed
            // (an epoch = every synced mailbox visited across one or more polls).
            if self.store.is_thread_hot(&thread_id).map_err(db_err)? {
                let walks = self.store.complete_walks().map_err(db_err)?;
                match self.store.hot_thread_emptied_at_walk(&thread_id).map_err(db_err)? {
                    None => {
                        // First observation of emptiness: start the grace
                        // clock at the current completed-epoch count and keep
                        // the local copy for now.
                        self.store.mark_hot_thread_emptied(&thread_id, walks).map_err(db_err)?;
                        return Ok(Vec::new());
                    }
                    Some(emptied_at) => {
                        if policy::emptied_grace_expired(emptied_at, walks) {
                            // Grace spent: the thread really is gone.
                            return Err(ProviderError::NotFound);
                        }
                        // Still within the grace window.
                        return Ok(Vec::new());
                    }
                }
            }
            // Not a hot thread (cold threads are never journaled, so the engine
            // never asks about one that is gone — but keep the original
            // contract): the engine deletes the local copy.
            return Err(ProviderError::NotFound);
        }

        // The thread regained (or still has) a location: clear any emptied
        // marker so a future emptiness restarts the grace from scratch.
        self.store.clear_hot_thread_emptied(&thread_id).map_err(db_err)?;

        let plan = self.build_sync_plan()?;

        // Thread-wide union of DYNAMIC (`folder:` / `lf:`) label ids across
        // every message's every location (fact 1). The engine builds a thread's
        // non-system labels from the ROOT message only, so a `folder:`/`lf:`
        // label that lives on a reply (or on a message whose only location is a
        // user folder) would be dropped unless EVERY returned message carries
        // it. We gather the union here and append it to each message below.
        // System location ids (INBOX/SENT/SPAM/TRASH) are per-copy, NOT in this
        // union — they are unioned over all messages by the engine already.
        let mut thread_dynamic: Vec<String> = Vec::new();
        for message_id in &message_ids {
            for location in self.store.locations_for_message(message_id).map_err(db_err)? {
                for id in plan.location_label_ids_for_mailbox(&location.mailbox) {
                    if (id.starts_with("folder:") || id.starts_with("lf:"))
                        && !thread_dynamic.contains(&id)
                    {
                        thread_dynamic.push(id);
                    }
                }
            }
        }
        thread_dynamic.sort();

        let mut messages = Vec::new();
        for message_id in &message_ids {
            let locations = self
                .store
                .locations_for_message(message_id)
                .map_err(db_err)?;
            if locations.is_empty() {
                // The message's last location was expunged; skip it.
                continue;
            }
            // Labels: the union across every copy, each classified by the
            // mailbox's RESOLVED location (never an ad-hoc name parse), then
            // the thread-wide dynamic union appended so every message carries
            // the thread's `folder:`/`lf:` labels (fact 1).
            let mut per_copy: Vec<Vec<String>> = locations
                .iter()
                .map(|location| {
                    let flags: Vec<String> =
                        serde_json::from_str(&location.flags_json).unwrap_or_default();
                    labels_for_location_ids(
                        &plan.location_label_ids_for_mailbox(&location.mailbox),
                        &flags,
                    )
                })
                .collect();
            per_copy.push(thread_dynamic.clone());
            let label_ids = merge_label_sets(&per_copy);

            // Body: cache first, else fetch from the best live location.
            let raw = match self.cache.get(message_id).map_err(db_err)? {
                Some(cached) => cached.raw,
                None => {
                    let mut session = self.open_session().await?;
                    let fetched = self.fetch_body_from_server(session.as_mut(), message_id).await;
                    let _ = session.logout().await;
                    match fetched {
                        Ok(Some(raw)) => raw,
                        // No body on the server (expunged between sync and read).
                        Ok(None) => continue,
                        // Oversize/unfetchable body: skip this message with a
                        // reason, do NOT fail the whole thread (design rule /
                        // SLICE5A_FIXES item 2). Only transport/auth propagates.
                        Err(ProviderError::PermanentClientRejection(reason)) => {
                            log::warn!(
                                "imap fetch_thread: skipping {message_id} (oversize body): {reason}"
                            );
                            continue;
                        }
                        Err(error) => return Err(error),
                    }
                }
            };

            match to_raw_message(&raw, message_id, label_ids) {
                Ok(mut message) => {
                    // Point the message at the surviving thread id.
                    message.thread_id = thread_id.clone();
                    messages.push(message);
                }
                Err(ProviderError::PermanentClientRejection(reason)) => {
                    // Per-message oversize: skip this message with a recorded
                    // reason, do NOT fail the whole thread (design rule).
                    log::warn!("imap fetch_thread: skipping {message_id}: {reason}");
                    continue;
                }
                Err(error) => return Err(error),
            }
        }

        if messages.is_empty() {
            return Err(ProviderError::NotFound);
        }
        Ok(messages)
    }
}

impl ImapProvider {
    /// Fetch a single message's body by locating one of its UIDs and running a
    /// bounded `BODY.PEEK[]`. Used by `fetch_thread`/`fetch_message` on a cache
    /// miss. The body is cached for next time.
    async fn fetch_body_from_server(
        &self,
        session: &mut dyn ImapSession,
        message_id: &str,
    ) -> ProviderResult<Option<Vec<u8>>> {
        let locations = self
            .store
            .locations_for_message(message_id)
            .map_err(db_err)?;
        if locations.is_empty() {
            return Ok(None);
        }
        // Best live location: prefer INBOX, then Sent, then any other mailbox.
        let plan = self.build_sync_plan()?;
        let rank = |mailbox: &str| -> u8 {
            if mailbox.eq_ignore_ascii_case(INBOX) {
                0
            } else if matches!(
                plan.entry_for(mailbox).and_then(|e| e.role),
                Some(super::MailboxRole::Sent)
            ) {
                1
            } else {
                2
            }
        };
        let mut ordered = locations;
        ordered.sort_by_key(|l| rank(&l.mailbox));
        let location = ordered.into_iter().next().expect("non-empty");

        // Per-mailbox UIDVALIDITY check before fetching (generalized from the
        // INBOX-only c77fa3f check): EXAMINE the location's mailbox and confirm
        // its UIDVALIDITY still matches before trusting the stored UID.
        let status = session.examine(&location.mailbox).await?;
        if status.uid_validity.map(i64::from) != Some(location.uidvalidity) {
            return Err(ProviderError::TransientTransport(format!(
                "{} UIDVALIDITY changed; sync must refresh locations before fetching",
                location.mailbox
            )));
        }
        let mut fetcher = SessionBodyFetcher { session };
        use super::fetch::BodyFetcher;
        let raw = fetcher.fetch_raw_body(location.uid as u32).await?;
        if let Some(ref bytes) = raw {
            self.cache
                .put(message_id, bytes, (self.now)())
                .map_err(db_err)?;
        }
        Ok(raw)
    }
}

#[async_trait]
impl MailFetch for ImapProvider {
    async fn fetch_message(&self, id: &str) -> ProviderResult<RawMessage> {
        let locations = self.store.locations_for_message(id).map_err(db_err)?;
        if locations.is_empty() {
            return Err(ProviderError::NotFound);
        }
        let plan = self.build_sync_plan()?;
        let mut per_copy: Vec<Vec<String>> = locations
            .iter()
            .map(|location| {
                let flags: Vec<String> =
                    serde_json::from_str(&location.flags_json).unwrap_or_default();
                labels_for_location_ids(
                    &plan.location_label_ids_for_mailbox(&location.mailbox),
                    &flags,
                )
            })
            .collect();
        // Thread-wide dynamic (`folder:`/`lf:`) union, so a single message read
        // carries the thread's user/label-folder labels even when they live on
        // another message of the thread (fact 1).
        if let Some(thread) = self.store.thread_of_message(id).map_err(db_err)? {
            let resolved = self.store.resolve_thread_alias(&thread).map_err(db_err)?;
            let mut thread_dynamic: Vec<String> = Vec::new();
            for message_id in self.store.messages_in_thread(&resolved).map_err(db_err)? {
                for location in self.store.locations_for_message(&message_id).map_err(db_err)? {
                    for lid in plan.location_label_ids_for_mailbox(&location.mailbox) {
                        if (lid.starts_with("folder:") || lid.starts_with("lf:"))
                            && !thread_dynamic.contains(&lid)
                        {
                            thread_dynamic.push(lid);
                        }
                    }
                }
            }
            thread_dynamic.sort();
            per_copy.push(thread_dynamic);
        }
        let label_ids = merge_label_sets(&per_copy);
        let raw = match self.cache.get(id).map_err(db_err)? {
            Some(cached) => cached.raw,
            None => {
                let mut session = self.open_session().await?;
                let fetched = self.fetch_body_from_server(session.as_mut(), id).await;
                let _ = session.logout().await;
                fetched?.ok_or(ProviderError::NotFound)?
            }
        };
        let mut message = to_raw_message(&raw, id, label_ids)?;
        if let Some(thread) = self.store.thread_of_message(id).map_err(db_err)? {
            message.thread_id = self.store.resolve_thread_alias(&thread).map_err(db_err)?;
        }
        Ok(message)
    }

    async fn attachment_bytes(&self, message: &str, handle: &str) -> ProviderResult<Vec<u8>> {
        // Re-slice the attachment out of the cached raw bytes by MIME section.
        let raw = match self.cache.get(message).map_err(db_err)? {
            Some(cached) => cached.raw,
            None => {
                let mut session = self.open_session().await?;
                let fetched = self.fetch_body_from_server(session.as_mut(), message).await;
                let _ = session.logout().await;
                fetched?.ok_or(ProviderError::NotFound)?
            }
        };
        attachment_bytes_from_raw(&raw, handle)?.ok_or(ProviderError::NotFound)
    }
}

/// The desired end-state of one message flag after a mutation request is
/// reduced: the flag must end up PRESENT, or ABSENT, on every synced copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DesiredFlag {
    /// The flag must be set on the message (e.g. `STARRED` -> `\Flagged`
    /// present; `remove UNREAD` -> `\Seen` present).
    Present,
    /// The flag must be cleared from the message (e.g. `add UNREAD` ->
    /// `\Seen` absent; `unstar` -> `\Flagged` absent).
    Absent,
}

/// The reduced plan for a flag mutation: the net desired state of `\Seen`
/// and/or `\Flagged`. Only the flags the request actually names are `Some`;
/// a flag the request did not mention is left untouched everywhere.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct FlagPlan {
    /// Desired `\Seen` state, derived from `UNREAD` (add UNREAD = `\Seen`
    /// absent; remove UNREAD = `\Seen` present). `None` if UNREAD was not in
    /// the request.
    seen: Option<DesiredFlag>,
    /// Desired `\Flagged` state, derived from `STARRED` (add STARRED =
    /// `\Flagged` present; remove STARRED = `\Flagged` absent). `None` if
    /// STARRED was not in the request.
    flagged: Option<DesiredFlag>,
}

impl FlagPlan {
    /// Whether this plan asks for no change at all (neither flag named).
    fn is_empty(self) -> bool {
        self.seen.is_none() && self.flagged.is_none()
    }

    /// Whether a stored flag list ALREADY satisfies every part of this plan,
    /// so the location needs no write command (idempotency / no-op).
    fn already_satisfied_by(self, flags: &[String]) -> bool {
        let has = |flag: &str| flags.iter().any(|f| f.eq_ignore_ascii_case(flag));
        let ok = |desired: Option<DesiredFlag>, present: bool| match desired {
            None => true,
            Some(DesiredFlag::Present) => present,
            Some(DesiredFlag::Absent) => !present,
        };
        ok(self.seen, has("\\Seen")) && ok(self.flagged, has("\\Flagged"))
    }

    /// The stored flag list this plan would leave behind on a location that
    /// started from `flags`: the same list with `\Seen`/`\Flagged` forced to
    /// the desired presence (case-insensitively), every other flag kept. Used
    /// to update `flags_json` locally after the verified store, so the next
    /// sync sweep sees no difference and does not flap.
    fn apply_to(self, flags: &[String]) -> Vec<String> {
        let mut out: Vec<String> = flags.to_vec();
        let force = |out: &mut Vec<String>, flag: &str, desired: Option<DesiredFlag>| {
            match desired {
                None => {}
                Some(DesiredFlag::Present) => {
                    if !out.iter().any(|f| f.eq_ignore_ascii_case(flag)) {
                        out.push(flag.to_string());
                    }
                }
                Some(DesiredFlag::Absent) => {
                    out.retain(|f| !f.eq_ignore_ascii_case(flag));
                }
            }
        };
        force(&mut out, "\\Seen", self.seen);
        force(&mut out, "\\Flagged", self.flagged);
        out
    }
}

/// Reduce a `(add, remove)` request to a [`FlagPlan`], rejecting anything the
/// flag path does not support. Only `UNREAD` and `STARRED` are supported here;
/// every other label id (INBOX, TRASH, SPAM, SENT, `lf:`, `folder:`, or an
/// unknown id) is a permanent `InvalidOperation` naming it, so the engine
/// rolls the change back rather than retrying forever. A label appearing in
/// BOTH `add` and `remove` is contradictory and rejected (never guessed).
fn plan_flag_mutation(add: &[String], remove: &[String]) -> ProviderResult<FlagPlan> {
    // Every id must be a supported system flag label.
    let check_supported = |ids: &[String]| -> ProviderResult<()> {
        for id in ids {
            if id != "UNREAD" && id != "STARRED" {
                return Err(ProviderError::InvalidOperation(format!(
                    "label {id:?} is not a flag the IMAP provider can set/clear directly \
                     (only UNREAD and STARRED are); refusing permanently"
                )));
            }
        }
        Ok(())
    };
    check_supported(add)?;
    check_supported(remove)?;

    // No label may appear in both lists (contradictory; reject, do not guess).
    for id in add {
        if remove.iter().any(|r| r == id) {
            return Err(ProviderError::InvalidOperation(format!(
                "label {id:?} appears in both the add and remove lists; \
                 the request is contradictory and is refused"
            )));
        }
    }

    let adds = |label: &str| add.iter().any(|l| l == label);
    let removes = |label: &str| remove.iter().any(|l| l == label);

    // UNREAD present means the message LACKS \Seen, so:
    //   add UNREAD    -> \Seen must be ABSENT
    //   remove UNREAD -> \Seen must be PRESENT
    let seen = if adds("UNREAD") {
        Some(DesiredFlag::Absent)
    } else if removes("UNREAD") {
        Some(DesiredFlag::Present)
    } else {
        None
    };
    // STARRED == \Flagged, so:
    //   add STARRED    -> \Flagged PRESENT
    //   remove STARRED -> \Flagged ABSENT
    let flagged = if adds("STARRED") {
        Some(DesiredFlag::Present)
    } else if removes("STARRED") {
        Some(DesiredFlag::Absent)
    } else {
        None
    };

    Ok(FlagPlan { seen, flagged })
}

impl ImapProvider {
    /// Apply an `UNREAD`/`STARRED` flag change to a set of messages, at EVERY
    /// synced location of each message, idempotently and with partial-failure
    /// honesty. The shared implementation behind both `modify_thread`
    /// (thread-level, resolved to its messages) and `modify_messages`
    /// (message-level).
    ///
    /// Steps (plan-then-apply; nothing is sent until the whole request
    /// validates):
    /// 1. Reduce `(add, remove)` to a net [`FlagPlan`] via
    ///    [`plan_flag_mutation`] — reject an unsupported label id or a label
    ///    in both lists with a permanent `InvalidOperation`, before any
    ///    command. A request that reduces to no flag at all is `Ok(())`.
    /// 2. For every message, read its stored locations. A message with NO
    ///    location at all (an emptied/grace thread) is a permanent
    ///    `InvalidOperation` naming it — never a silent success. Skip any
    ///    location whose stored flags already satisfy the plan (idempotency;
    ///    avoids a pointless command).
    /// 3. Group the remaining locations by `(mailbox, uidvalidity)`. For each
    ///    mailbox, `select_for_write` (guards UIDVALIDITY; a stale mailbox is a
    ///    named transient error and NOTHING is sent), then run one
    ///    `verified_store` per store op (`+/-\Seen`, `+/-\Flagged`) using
    ///    run 1's capability-gated verified store — so a server that cannot
    ///    keep a flag fails cleanly with the permanent error naming it.
    /// 4. After each mailbox's ops verify, update that mailbox's locations'
    ///    stored `flags_json` so the next sync sweep sees no difference and
    ///    does NOT journal a spurious change. One session is reused for the
    ///    whole mutation (respecting the 2-connection cap — the command slot).
    /// 5. Partial failure: mailboxes are processed one at a time, and a
    ///    mailbox whose ops succeeded has its local flags already persisted; a
    ///    later mailbox's error is returned (transient stays transient,
    ///    permanent stays permanent), so a retry touches only the failed one.
    ///
    /// Does NOT bump the sync generation or journal anything: a flag the next
    /// sweep would observe anyway is kept consistent locally (step 4), and the
    /// engine already applied the optimistic local change that drove this
    /// mutation — the provider only makes the server and the local location
    /// rows agree. Journaling here would re-report a change the engine made.
    async fn apply_flag_mutation(
        &self,
        message_ids: &[String],
        add: &[String],
        remove: &[String],
    ) -> ProviderResult<()> {
        let plan = plan_flag_mutation(add, remove)?;
        if plan.is_empty() {
            return Ok(()); // Nothing to do (no supported flag named).
        }

        // Gather every location that still needs a write, grouped by mailbox,
        // validating messages up front (plan-then-apply). A message with no
        // location at all fails the whole request permanently.
        //
        // Keyed by mailbox name; each value carries the mailbox's UIDVALIDITY
        // (taken from its location rows) and the locations whose stored flags
        // do NOT yet satisfy the plan.
        let mut by_mailbox: std::collections::BTreeMap<String, (i64, Vec<ImapLocation>)> =
            std::collections::BTreeMap::new();
        for message_id in message_ids {
            let locations = self
                .store
                .locations_for_message(message_id)
                .map_err(db_err)?;
            if locations.is_empty() {
                return Err(ProviderError::InvalidOperation(format!(
                    "message {message_id:?} has no synced location to modify \
                     (it may have been expunged or moved to an unsynced mailbox); \
                     refusing the flag change permanently"
                )));
            }
            for location in locations {
                let flags: Vec<String> =
                    serde_json::from_str(&location.flags_json).unwrap_or_default();
                if plan.already_satisfied_by(&flags) {
                    continue; // Idempotent: this copy already matches.
                }
                by_mailbox
                    .entry(location.mailbox.clone())
                    .or_insert_with(|| (location.uidvalidity, Vec::new()))
                    .1
                    .push(location);
            }
        }

        // Everything is already satisfied: send no command at all (idempotent
        // no-op), as the brief requires.
        if by_mailbox.is_empty() {
            return Ok(());
        }

        // Reuse ONE session for the whole mutation (the command slot; the
        // 2-connection cap is respected because we lease exactly one).
        let mut session = self.open_session().await?;
        let result = self.apply_flag_mutation_on(session.as_mut(), &plan, by_mailbox).await;
        let _ = session.logout().await;
        result
    }

    /// The flag mutation against an already-open session, factored out so the
    /// session is logged out exactly once regardless of outcome. Processes one
    /// mailbox at a time; a mailbox that succeeds persists its locations' new
    /// flags before the next is attempted, so a partial failure leaves the
    /// succeeded mailboxes consistent and a retry re-derives only the rest.
    async fn apply_flag_mutation_on(
        &self,
        session: &mut dyn ImapSession,
        plan: &FlagPlan,
        by_mailbox: std::collections::BTreeMap<String, (i64, Vec<ImapLocation>)>,
    ) -> ProviderResult<()> {
        for (mailbox, (uidvalidity, locations)) in by_mailbox {
            // The write session must also expose the write trait; the live
            // LeasedSession and the test AsyncImapSession both implement it.
            // Guard UIDVALIDITY (a stale mailbox is transient; nothing sent).
            let write_session = as_write_session(session)?;
            let ctx = super::write::select_for_write(write_session, &self.store, &mailbox, uidvalidity as u32)
                .await?;

            // The UID set for this mailbox (every location that needs a write;
            // one store op applies to all of them).
            let uids: Vec<u32> = locations.iter().map(|l| l.uid as u32).collect();

            // One verified store per flag op the plan names. `\Seen` and
            // `\Flagged` are independent ops; each is a capability-gated,
            // re-fetched verified store (run 1), so a server that silently
            // ignores or cannot keep a flag fails with the permanent error
            // naming the flag + mailbox.
            if let Some(desired) = plan.seen {
                run_store(write_session, &ctx, &uids, "\\Seen", desired).await?;
            }
            if let Some(desired) = plan.flagged {
                run_store(write_session, &ctx, &uids, "\\Flagged", desired).await?;
            }

            // This mailbox's ops all verified: update each location's stored
            // flags so the next sweep sees no difference (no flap, no spurious
            // journal). Done BEFORE the next mailbox, so a later failure leaves
            // this mailbox's local state already consistent (partial-failure
            // honesty).
            for location in &locations {
                let flags: Vec<String> =
                    serde_json::from_str(&location.flags_json).unwrap_or_default();
                let updated = plan.apply_to(&flags);
                let new_json =
                    serde_json::to_string(&updated).unwrap_or_else(|_| location.flags_json.clone());
                self.store
                    .upsert_location(&ImapLocation {
                        flags_json: new_json,
                        ..location.clone()
                    })
                    .map_err(db_err)?;
            }
        }
        Ok(())
    }
}

/// Run one verified store that forces `flag` to `desired` on a UID set. An
/// `Absent` desired is a `-FLAGS` remove; a `Present` is a `+FLAGS` add. The
/// flag is validated to its wire spelling first (it is a fixed system flag
/// here, so this never rejects), then handed to run 1's capability-gated
/// verified store.
async fn run_store(
    session: &mut dyn super::write::ImapWriteSession,
    ctx: &super::write::WriteContext,
    uids: &[u32],
    flag: &str,
    desired: DesiredFlag,
) -> ProviderResult<()> {
    let wire = super::write::validate_flag(flag)?;
    let op = match desired {
        DesiredFlag::Present => super::write::StoreOp::Add,
        DesiredFlag::Absent => super::write::StoreOp::Remove,
    };
    super::write::verified_store(session, ctx, uids, op, &[wire]).await
}

/// Narrow a `&mut dyn ImapSession` to the write trait. Every `ImapSession`
/// the provider opens (the live `LeasedSession` and the test
/// `AsyncImapSession`) also implements `ImapWriteSession` and overrides
/// `ImapSession::as_write_session` to return `Some(self)`; a read-only session
/// (the test stub) returns `None`, which here is an internal error — the flag
/// path is only ever driven over a write-capable session, so `None` means a
/// programming mistake, surfaced as a non-retryable `Other` rather than a
/// panic.
fn as_write_session<'a>(
    session: &'a mut dyn ImapSession,
) -> ProviderResult<&'a mut dyn super::write::ImapWriteSession> {
    session.as_write_session().ok_or_else(|| {
        ProviderError::Other(
            "internal error: flag mutation driven over a non-write-capable IMAP session".into(),
        )
    })
}

// ===========================================================================
// System-label MOVES (Phase 3 run 3): archive / unarchive, trash / untrash,
// spam / not-spam. These are a MOVE of a message's copy between real mailboxes
// (INBOX <-> Archive / Trash / Junk), never a flag. The engine delivers them
// through `modify_thread` only, in exactly six (add, remove) shapes
// (verified against `sync.rs::mutation_labels`).
// ===========================================================================

/// The two kinds of request `modify_thread` serves, decided up front so a
/// malformed request is rejected (permanently) before any command is built.
enum ThreadMutationKind {
    /// A flag change (`UNREAD` / `STARRED`), handled by the run-2 flag path.
    Flag,
    /// A system-label move (archive/trash/spam and their reverses).
    SystemMove(SystemMovePlan),
}

/// A fully-validated system move: WHICH source mailboxes a message's copy may
/// move FROM (by role) and the single destination role it moves TO. "Already
/// satisfied" is defined by presence/absence of a copy in these roles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SystemMovePlan {
    /// The move being performed, for messages and no-op/`InvalidOperation`
    /// wording.
    op: SystemMoveOp,
}

/// The six system-move operations, each a (source role, destination role)
/// pair. Named so the no-command and nothing-movable messages are specific.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SystemMoveOp {
    /// `remove [INBOX]`: move the INBOX copy to Archive.
    Archive,
    /// `add [INBOX]`: move the Archive copy to INBOX.
    Unarchive,
    /// `add [TRASH], remove [INBOX]`: move the INBOX (or Archive) copy to Trash.
    Trash,
    /// `add [INBOX], remove [TRASH]`: move the Trash copy to INBOX.
    Untrash,
    /// `add [SPAM], remove [INBOX]`: move the INBOX (or Archive) copy to Junk.
    Spam,
    /// `add [INBOX], remove [SPAM]`: move the Junk copy to INBOX.
    NotSpam,
}

/// Where a message copy may be moved FROM and TO for one system move. A role
/// is resolved to a concrete mailbox name at apply time via the sync plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MoveRole {
    Inbox,
    Archive,
    Trash,
    Junk,
}

impl SystemMoveOp {
    /// The destination role a message moves TO.
    fn dest_role(self) -> MoveRole {
        match self {
            SystemMoveOp::Archive => MoveRole::Archive,
            SystemMoveOp::Trash => MoveRole::Trash,
            SystemMoveOp::Spam => MoveRole::Junk,
            SystemMoveOp::Unarchive | SystemMoveOp::Untrash | SystemMoveOp::NotSpam => {
                MoveRole::Inbox
            }
        }
    }

    /// The source roles whose copy is eligible to move, in PRIORITY order (the
    /// first present copy is the one that moves). archive/trash/spam move the
    /// INBOX copy, falling back to an Archive copy (trash/spam from the archive
    /// view); the reverses move the copy out of the single source mailbox.
    fn source_roles(self) -> &'static [MoveRole] {
        match self {
            SystemMoveOp::Archive => &[MoveRole::Inbox],
            SystemMoveOp::Trash | SystemMoveOp::Spam => &[MoveRole::Inbox, MoveRole::Archive],
            SystemMoveOp::Unarchive => &[MoveRole::Archive],
            SystemMoveOp::Untrash => &[MoveRole::Trash],
            SystemMoveOp::NotSpam => &[MoveRole::Junk],
        }
    }

    /// A plain-language name for error/no-op wording.
    fn describe(self) -> &'static str {
        match self {
            SystemMoveOp::Archive => "archive",
            SystemMoveOp::Unarchive => "unarchive",
            SystemMoveOp::Trash => "trash",
            SystemMoveOp::Untrash => "untrash",
            SystemMoveOp::Spam => "mark as spam",
            SystemMoveOp::NotSpam => "mark as not spam",
        }
    }
}

/// Classify a `modify_thread` `(add, remove)` request. Returns the flag path
/// for a request that names only `UNREAD`/`STARRED`; the move path for exactly
/// one of the six system-move shapes; and a permanent `InvalidOperation` for
/// everything else (a system id mixed with a flag or a `lf:`/`folder:`/SENT/
/// `kw:` id, an id in both lists, an unknown id, or a combination of system
/// ids that is not one of the six). Nothing is sent before this validates.
fn classify_thread_mutation(
    add: &[String],
    remove: &[String],
) -> ProviderResult<ThreadMutationKind> {
    // Does any list mention a system-MOVE label (INBOX/TRASH/SPAM)?
    const MOVE_IDS: [&str; 3] = ["INBOX", "TRASH", "SPAM"];
    let is_move_id = |id: &String| MOVE_IDS.contains(&id.as_str());
    let any_move = add.iter().chain(remove.iter()).any(is_move_id);

    if !any_move {
        // No system-move label present: this is the flag path. `plan_flag_
        // mutation` (run 2) still rejects an unsupported id (SENT, lf:,
        // folder:, kw:, …) and an id in both lists, exactly as before.
        return Ok(ThreadMutationKind::Flag);
    }

    // A system move is being requested. It must be EXACTLY one of the six
    // engine shapes — no flag label, no `lf:`/`folder:`/`SENT`/`kw:`, no extra
    // id, and no id in both lists. Match the whole request literally.
    if let Some(op) = match_system_move_shape(add, remove) {
        return Ok(ThreadMutationKind::SystemMove(SystemMovePlan { op }));
    }

    Err(ProviderError::InvalidOperation(format!(
        "the IMAP provider accepts a system move (archive/trash/spam and their \
         reverses) only in its exact engine shape; the request add={add:?} \
         remove={remove:?} is not one of those shapes (a system label combined \
         with a flag, a folder/label id, SENT, an extra id, or an id in both \
         lists); refusing permanently"
    )))
}

/// Match a `(add, remove)` request against the six exact system-move shapes
/// (`sync.rs::mutation_labels`): archive `([],[INBOX])`, unarchive
/// `([INBOX],[])`, trash `([TRASH],[INBOX])`, untrash `([INBOX],[TRASH])`,
/// spam `([SPAM],[INBOX])`, not-spam `([INBOX],[SPAM])`. Any other combination
/// (extra ids, duplicates, an id in both lists) returns `None`.
fn match_system_move_shape(add: &[String], remove: &[String]) -> Option<SystemMoveOp> {
    let a: Vec<&str> = add.iter().map(String::as_str).collect();
    let r: Vec<&str> = remove.iter().map(String::as_str).collect();
    match (a.as_slice(), r.as_slice()) {
        ([], ["INBOX"]) => Some(SystemMoveOp::Archive),
        (["INBOX"], []) => Some(SystemMoveOp::Unarchive),
        (["TRASH"], ["INBOX"]) => Some(SystemMoveOp::Trash),
        (["INBOX"], ["TRASH"]) => Some(SystemMoveOp::Untrash),
        (["SPAM"], ["INBOX"]) => Some(SystemMoveOp::Spam),
        (["INBOX"], ["SPAM"]) => Some(SystemMoveOp::NotSpam),
        _ => None,
    }
}

impl ImapProvider {
    /// Resolve a [`MoveRole`] to its concrete catalog mailbox name through the
    /// sync plan (roles come from the saved plan/catalog — overrides, RFC 6154
    /// attribute, then name match — never hard-coded). INBOX is the reserved
    /// `is_inbox` entry; Archive/Trash/Junk are the entries whose resolved
    /// role matches. Returns `None` when the account has no mailbox for the
    /// role (e.g. no Trash configured).
    fn mailbox_for_role(&self, plan: &SyncPlan, role: MoveRole) -> Option<String> {
        match role {
            MoveRole::Inbox => plan
                .entries
                .iter()
                .find(|e| e.is_inbox)
                .map(|e| e.mailbox.clone())
                // The reserved INBOX always exists even before discovery
                // persisted its catalog row.
                .or_else(|| Some(INBOX.to_string())),
            MoveRole::Archive => self.mailbox_with_role(plan, super::MailboxRole::Archive),
            MoveRole::Trash => self.mailbox_with_role(plan, super::MailboxRole::Trash),
            MoveRole::Junk => self.mailbox_with_role(plan, super::MailboxRole::Junk),
        }
    }

    /// The catalog mailbox name whose plan entry resolved to `role`, if any.
    fn mailbox_with_role(
        &self,
        plan: &SyncPlan,
        role: super::MailboxRole,
    ) -> Option<String> {
        plan.entries
            .iter()
            .find(|e| e.role == Some(role))
            .map(|e| e.mailbox.clone())
    }

    /// Save the sender's own copy of a just-sent message into the account's
    /// Sent mailbox, IDEMPOTENTLY (Phase 4 run 4a). Resolves the Sent mailbox
    /// from the sync plan (overrides > RFC 6154 `\Sent` > name match, exactly
    /// like the move paths resolve Archive/Trash/Junk), opens its OWN leased
    /// session (the command slot, like every other write path), and runs the
    /// [`super::write::save_sent_copy`] primitive.
    ///
    /// `raw` is the OUTBOX raw bytes UNMODIFIED — the Sent copy keeps its
    /// `Bcc:` header (only the SMTP wire copy stripped it). `message_id` is our
    /// own generated id for the message (its raw `Message-ID` header value).
    ///
    /// Run 4b owns WHEN this is called (first-send detection against
    /// `settings.server_saves_sent`, a durable retry job, per-message
    /// serialisation + settle delay); nothing in shipped code calls it yet, so
    /// it carries the module's scoped `dead_code` allow until then. A missing
    /// Sent mailbox is a typed permanent error naming it (this never creates
    /// one).
    async fn save_sent_copy_for(
        &self,
        raw: &[u8],
        message_id: &str,
        config: super::write::SaveSentConfig,
    ) -> super::write::WriteResult<super::write::SentCopy> {
        let plan = self
            .build_sync_plan()
            .map_err(super::write::WriteError::Provider)?;
        let mailbox = self
            .mailbox_with_role(&plan, super::MailboxRole::Sent)
            .ok_or_else(|| {
                super::write::WriteError::Provider(ProviderError::InvalidOperation(
                    "cannot save the Sent copy: this account has no Sent mailbox configured; \
                     refusing permanently"
                        .into(),
                ))
            })?;

        let mut session = self
            .open_session()
            .await
            .map_err(super::write::WriteError::Provider)?;
        let result = {
            let write_session =
                as_write_session(session.as_mut()).map_err(super::write::WriteError::Provider)?;
            super::write::save_sent_copy(write_session, &mailbox, raw, message_id, config).await
        };
        let _ = session.logout().await;
        result
    }

    /// Apply a system-label move to every (surviving) message of a thread,
    /// idempotently and with partial-failure honesty.
    ///
    /// Plan-then-apply: the whole request is validated and the per-message
    /// move decided from LOCAL state before any command is sent.
    ///
    /// Per message (brief "Which copy moves"): find the first present copy in
    /// the op's source roles (INBOX first, Archive as the archive-view
    /// fallback for trash/spam). If none is present but a copy already exists
    /// in the DESTINATION role, the message is already satisfied (no command).
    /// If neither, the message is skipped as nothing-movable here. A thread
    /// where EVERY message is already satisfied sends nothing (not even
    /// SELECT); a thread with NO movable and NO already-satisfied message is a
    /// permanent `InvalidOperation` naming the op (e.g. trash of a Sent-only
    /// thread). The destination mailbox is resolved from the plan; a missing
    /// Trash/Junk is a permanent error naming the role, a missing Archive is
    /// created and the move retried once.
    ///
    /// Moves are grouped BY SOURCE MAILBOX and run one mailbox at a time over a
    /// single leased session (2-connection cap). Each move is performed per
    /// message (a single-UID relocate) so a half-applied copy+delete+expunge
    /// records exactly that message's destination location and the retry is
    /// idempotent. After each source mailbox's moves commit, local state is
    /// persisted (source row deleted, destination row inserted when COPYUID is
    /// known) before the next mailbox, so a later failure leaves the finished
    /// ones consistent and the retry re-derives only the rest.
    async fn apply_system_move(
        &self,
        message_ids: &[String],
        plan_move: SystemMovePlan,
    ) -> ProviderResult<()> {
        let sync_plan = self.build_sync_plan()?;
        let op = plan_move.op;

        // Resolve the destination mailbox up front (same for every message).
        let dest_role = op.dest_role();
        let dest_mailbox = match self.mailbox_for_role(&sync_plan, dest_role) {
            Some(name) => name,
            None => {
                // Missing Trash/Junk is a permanent error naming the role; a
                // missing INBOX cannot happen (reserved). Archive's absence is
                // handled lazily (create-on-TRYCREATE), so it never reaches
                // here as None — but guard it with the same permanent error.
                return Err(ProviderError::InvalidOperation(format!(
                    "cannot {}: this account has no {} mailbox configured; refusing permanently",
                    op.describe(),
                    role_name(dest_role),
                )));
            }
        };

        // Decide each message's move from LOCAL state. A `PlannedMove` records
        // the source mailbox + uid(+uidvalidity) and the destination the row
        // should land in. `already_satisfied` counts messages needing nothing.
        let mut by_source: std::collections::BTreeMap<String, (i64, Vec<PlannedMove>)> =
            std::collections::BTreeMap::new();
        let mut already_satisfied = 0usize;
        let mut movable = 0usize;

        for message_id in message_ids {
            let locations = self
                .store
                .locations_for_message(message_id)
                .map_err(db_err)?;
            // Which mailbox name plays each role, for THIS message's locations.
            let in_role = |role: MoveRole| -> Option<ImapLocation> {
                let name = self.mailbox_for_role(&sync_plan, role)?;
                locations.iter().find(|l| l.mailbox == name).cloned()
            };

            // Already in the destination (and, for the reverses, no source
            // copy left) -> satisfied, no command.
            let dest_copy = locations.iter().any(|l| l.mailbox == dest_mailbox);

            // The first present source copy, in priority order.
            let source = op.source_roles().iter().find_map(|&role| in_role(role));

            match source {
                Some(src) => {
                    movable += 1;
                    by_source
                        .entry(src.mailbox.clone())
                        .or_insert_with(|| (src.uidvalidity, Vec::new()))
                        .1
                        .push(PlannedMove {
                            message_id: message_id.clone(),
                            uid: src.uid,
                            flags_json: src.flags_json.clone(),
                            modseq: src.modseq,
                        });
                }
                None if dest_copy => {
                    already_satisfied += 1;
                }
                None => {
                    // Nothing movable for this message (its only copies are in
                    // Sent / user folders / label folders). Skip it.
                }
            }
        }

        if movable == 0 {
            if already_satisfied > 0 {
                // Every message is already in the target state: no command.
                return Ok(());
            }
            // Nothing moved and nothing was already satisfied: permanent, so
            // the engine does not show a state the server never had.
            return Err(ProviderError::InvalidOperation(format!(
                "cannot {}: the thread has no message copy in a mailbox this move \
                 can act on (its messages live only in Sent, a user folder, or a \
                 label folder); refusing permanently",
                op.describe()
            )));
        }

        // One leased session for the whole move (the command slot).
        let mut session = self.open_session().await?;
        let result = self
            .apply_system_move_on(session.as_mut(), op, &dest_mailbox, by_source)
            .await;
        let _ = session.logout().await;
        result
    }

    /// The system move against an already-open session, factored out so the
    /// session is logged out exactly once. One source mailbox at a time; a
    /// source mailbox that finishes persists its local state before the next
    /// is attempted (partial-failure honesty).
    async fn apply_system_move_on(
        &self,
        session: &mut dyn ImapSession,
        op: SystemMoveOp,
        dest_mailbox: &str,
        by_source: std::collections::BTreeMap<String, (i64, Vec<PlannedMove>)>,
    ) -> ProviderResult<()> {
        for (source_mailbox, (uidvalidity, moves)) in by_source {
            let write_session = as_write_session(session)?;
            // Guard the SOURCE mailbox's UIDVALIDITY (stale -> transient,
            // nothing sent), as the flag path does.
            let ctx = super::write::select_for_write(
                write_session,
                &self.store,
                &source_mailbox,
                uidvalidity as u32,
            )
            .await?;

            // Move each message individually so a half-applied
            // copy+delete+expunge records exactly that message's destination
            // location (idempotent retry). The whole source mailbox shares one
            // SELECT/context.
            for planned in &moves {
                self.move_one_message(write_session, &ctx, op, &source_mailbox, dest_mailbox, planned)
                    .await?;
            }
        }
        Ok(())
    }

    /// Move ONE message's source copy to `dest_mailbox`, recording local state
    /// and keeping the retry idempotent. Steps:
    ///
    /// 1. If the destination ALREADY has this message locally (recorded by an
    ///    earlier partial failure), do not copy again: finish the half-applied
    ///    move by expunging the still-present source copy, then clear the
    ///    source row. (copy+delete+expunge strategy only; MOVE is atomic.)
    /// 2. Otherwise relocate the single UID with the recording relocate. On a
    ///    partial failure that already copied, PERSIST the destination row
    ///    (from the returned COPYUID) BEFORE propagating the error, so the
    ///    retry takes path 1.
    /// 3. On a `TRYCREATE` destination: Archive is created and the move retried
    ///    once; Trash/Junk are never created (permanent error naming the role).
    /// 4. On success, delete the source location row and (when COPYUID is
    ///    known) insert the destination row with the same flags and the new
    ///    UID/uidvalidity, so the next sweep sees no change and no duplicate.
    async fn move_one_message(
        &self,
        session: &mut dyn super::write::ImapWriteSession,
        ctx: &super::write::WriteContext,
        op: SystemMoveOp,
        source_mailbox: &str,
        dest_mailbox: &str,
        planned: &PlannedMove,
    ) -> ProviderResult<()> {
        let uid = planned.uid as u32;

        // Explicit: is there a destination location row for THIS message
        // already (recorded by an earlier partial failure)?
        let dest_has_message = self
            .store
            .locations_for_message(&planned.message_id)
            .map_err(db_err)?
            .iter()
            .any(|l| l.mailbox == dest_mailbox);

        if dest_has_message
            && matches!(
                ctx.caps.relocation_strategy(),
                super::write::RelocationStrategy::CopyDeleteExpunge
            )
        {
            // Finish the half-applied move: expunge the source copy only.
            super::write::expunge_source_copies(session, ctx, &[uid]).await?;
            self.store
                .delete_location(source_mailbox, planned.uid_validity_of(ctx), uid as i64)
                .map_err(db_err)?;
            return Ok(());
        }

        // Path 2/3: relocate the single UID, recording a partial failure's
        // destination so a retry is idempotent.
        match self
            .relocate_one_recording(session, ctx, dest_mailbox, planned)
            .await
        {
            Ok(outcome) => {
                self.commit_moved_location(ctx, source_mailbox, dest_mailbox, planned, &outcome.copyuid)?;
                Ok(())
            }
            Err(failure) => {
                // Record whatever landed in the destination BEFORE propagating,
                // so the retry (path 1) does not copy again.
                if let Some(copyuid) = &failure.copyuid {
                    self.record_dest_only(dest_mailbox, planned, copyuid)?;
                }
                match failure.error {
                    super::write::WriteError::TryCreate(text) => {
                        self.handle_trycreate_and_retry(
                            session,
                            ctx,
                            op,
                            source_mailbox,
                            dest_mailbox,
                            planned,
                            &text,
                        )
                        .await
                    }
                    super::write::WriteError::Provider(error) => Err(error),
                }
            }
        }
    }

    /// Create a missing destination (Archive only) and retry the move ONCE;
    /// Trash/Junk are never created. The create path reuses the session's
    /// `create_mailbox`; a second failure propagates.
    #[allow(clippy::too_many_arguments)]
    async fn handle_trycreate_and_retry(
        &self,
        session: &mut dyn super::write::ImapWriteSession,
        ctx: &super::write::WriteContext,
        op: SystemMoveOp,
        source_mailbox: &str,
        dest_mailbox: &str,
        planned: &PlannedMove,
        trycreate_text: &str,
    ) -> ProviderResult<()> {
        // Only Archive is auto-created. Trash/Junk missing is permanent.
        if op.dest_role() != MoveRole::Archive {
            return Err(ProviderError::InvalidOperation(format!(
                "cannot {}: the destination mailbox {dest_mailbox:?} does not exist \
                 (TRYCREATE: {trycreate_text}) and the provider does not create it; \
                 refusing permanently",
                op.describe()
            )));
        }
        // Create the Archive mailbox, then retry the relocate once.
        session.create_mailbox(dest_mailbox).await?;
        match self
            .relocate_one_recording(session, ctx, dest_mailbox, planned)
            .await
        {
            Ok(outcome) => {
                self.commit_moved_location(ctx, source_mailbox, dest_mailbox, planned, &outcome.copyuid)
            }
            Err(failure) => {
                if let Some(copyuid) = &failure.copyuid {
                    self.record_dest_only(dest_mailbox, planned, copyuid)?;
                }
                Err(failure.error.into_provider_error())
            }
        }
    }

    /// After a VERIFIED move: delete the source location row, and when COPYUID
    /// is known insert the destination row (same flags, new UID, destination
    /// UIDVALIDITY). Without COPYUID (never on the UIDPLUS copy path; possible
    /// only on a MOVE server that returned none) just delete the source — the
    /// next sweep of the destination re-discovers the message by Message-ID,
    /// and the grace rule keeps the thread meanwhile.
    fn commit_moved_location(
        &self,
        ctx: &super::write::WriteContext,
        source_mailbox: &str,
        dest_mailbox: &str,
        planned: &PlannedMove,
        copyuid: &Option<super::write::CopyUid>,
    ) -> ProviderResult<()> {
        // Insert the destination row first (so a crash between the two leaves
        // the message FINDABLE in both, never lost), then delete the source.
        if let Some(copyuid) = copyuid {
            if let Some(dest_uid) = dest_uid_for(copyuid, planned.uid as u32) {
                self.store
                    .upsert_location(&ImapLocation {
                        mailbox: dest_mailbox.to_string(),
                        uidvalidity: copyuid.uidvalidity as i64,
                        uid: dest_uid as i64,
                        message_id: planned.message_id.clone(),
                        flags_json: planned.flags_json.clone(),
                        modseq: planned.modseq,
                    })
                    .map_err(db_err)?;
            }
        }
        self.store
            .delete_location(source_mailbox, ctx.uidvalidity as i64, planned.uid)
            .map_err(db_err)?;
        self.store
            .complete_copy(&planned.message_id, dest_mailbox)
            .map_err(db_err)?;
        Ok(())
    }

    /// Record ONLY the destination location row (used on a partial failure, so
    /// the retry sees the copy and does not re-copy). The source row is left
    /// in place; the retry's expunge removes the server copy and then clears
    /// it.
    fn record_dest_only(
        &self,
        dest_mailbox: &str,
        planned: &PlannedMove,
        copyuid: &super::write::CopyUid,
    ) -> ProviderResult<()> {
        if let Some(dest_uid) = dest_uid_for(copyuid, planned.uid as u32) {
            self.store
                .upsert_location(&ImapLocation {
                    mailbox: dest_mailbox.to_string(),
                    uidvalidity: copyuid.uidvalidity as i64,
                    uid: dest_uid as i64,
                    message_id: planned.message_id.clone(),
                    flags_json: planned.flags_json.clone(),
                    modseq: planned.modseq,
                })
                .map_err(db_err)?;
            self.store
                .complete_copy(&planned.message_id, dest_mailbox)
                .map_err(db_err)?;
        }
        Ok(())
    }

    /// The COPY fallback records durable intent before the wire command. The
    /// native MOVE path is unchanged; it cannot create duplicate source copies.
    async fn relocate_one_recording(
        &self,
        session: &mut dyn super::write::ImapWriteSession,
        ctx: &super::write::WriteContext,
        dest: &str,
        planned: &PlannedMove,
    ) -> super::write::RecordingRelocateResult {
        use super::write::{RelocateFailure, RelocateOutcome, RelocationStrategy};
        if ctx.caps.relocation_strategy() != RelocationStrategy::CopyDeleteExpunge {
            return super::write::relocate_uids_recording(
                session,
                ctx,
                &[planned.uid as u32],
                dest,
            )
            .await;
        }
        super::write::require_targeted_delete(ctx).map_err(|error| RelocateFailure {
            copyuid: None,
            error: error.into(),
        })?;
        let outcome = self
            .copy_with_recovery(session, ctx, dest, planned)
            .await
            .map_err(|error| RelocateFailure {
                copyuid: None,
                error,
            })?;
        if let Err(error) =
            super::write::expunge_source_copies(session, ctx, &[planned.uid as u32]).await
        {
            return Err(RelocateFailure {
                copyuid: outcome.copyuid,
                error: super::write::classified_partial_relocate_error(
                    &ctx.mailbox,
                    dest,
                    &planned.uid.to_string(),
                    "COPY succeeded but verified source deletion failed",
                    &error,
                )
                .into(),
            });
        }
        Ok(RelocateOutcome {
            copyuid: outcome.copyuid,
        })
    }

    /// On retry after a lost reply or process exit, inspect the destination
    /// using normal sync identity before issuing another COPY. Only uncertain
    /// copies scan the destination, and FETCH is bounded and uses BODY.PEEK.
    async fn copy_with_recovery(
        &self,
        session: &mut dyn super::write::ImapWriteSession,
        ctx: &super::write::WriteContext,
        dest: &str,
        planned: &PlannedMove,
    ) -> super::write::WriteResult<super::write::CopyOutcome> {
        use super::write::WriteError;
        let arg = super::write::build_wire_mailbox_arg(dest)?;
        let pending = self
            .store
            .pending_copy_identity(&planned.message_id, dest)
            .map_err(db_err)?;
        if let Some(identity) = &pending {
            if let Some(outcome) = self
                .reconcile_copy(session, ctx, dest, planned.uid as u32, identity)
                .await?
            {
                return Ok(outcome);
            }
        }
        let set = planned.uid.to_string();
        let source = fetch_identity(session, &set)
            .await?
            .into_iter()
            .find(|row| row.uid == planned.uid as u32)
            .ok_or_else(|| {
                ProviderError::TransientTransport(
                    "COPY source is no longer present; resync before retrying".into(),
                )
            })?;
        let identity = super::identity::derive_message_id(self.store.account_id(), &source.inputs);
        if pending
            .as_ref()
            .is_some_and(|previous| previous != &identity)
        {
            return Err(ProviderError::InvalidOperation(
                "COPY source identity changed while recovering; refusing to copy another message"
                    .into(),
            )
            .into());
        }
        self.store
            .begin_copy(&planned.message_id, dest, &identity)
            .map_err(db_err)?;
        match session.uid_copy(&set, &arg).await {
            Ok(outcome) if outcome.copyuid.is_some() => Ok(outcome),
            Ok(_) => self.reconcile_copy(session, ctx, dest, planned.uid as u32, &identity).await?
                .ok_or_else(|| ProviderError::TransientTransport(
                    "COPY was acknowledged but its destination is not visible yet; retry reconciliation".into()
                ).into()),
            Err(WriteError::TryCreate(text)) => {
                // The server explicitly refused the COPY; no copy landed.
                self.store.complete_copy(&planned.message_id, dest).map_err(db_err)?;
                Err(WriteError::TryCreate(text))
            }
            Err(error) => Err(error), // Keep the intent: the outcome may be uncertain.
        }
    }

    async fn reconcile_copy(
        &self,
        session: &mut dyn super::write::ImapWriteSession,
        ctx: &super::write::WriteContext,
        dest: &str,
        source_uid: u32,
        identity: &str,
    ) -> ProviderResult<Option<super::write::CopyOutcome>> {
        let status = session.examine(dest).await?;
        let validity = status.uid_validity.ok_or_else(|| {
            ProviderError::InvalidOperation(
                "COPY destination supplied no UIDVALIDITY; cannot reconcile safely".into(),
            )
        })?;
        let uids = session.uid_search("ALL").await?;
        let mut found = None;
        for batch in super::write::build_uid_set_batches(&uids) {
            for row in fetch_identity(session, &batch).await? {
                if super::identity::derive_message_id(self.store.account_id(), &row.inputs)
                    == identity
                {
                    found = Some(super::write::CopyOutcome {
                        copyuid: Some(super::write::CopyUid {
                            uidvalidity: validity,
                            source: vec![source_uid],
                            dest: vec![row.uid],
                        }),
                    });
                    break;
                }
            }
            if found.is_some() {
                break;
            }
        }
        // EXAMINE changed the selection. Re-establish and guard the source
        // before the caller can COPY, STORE or EXPUNGE using its old context.
        super::write::select_for_write(session, &self.store, &ctx.mailbox, ctx.uidvalidity).await?;
        Ok(found)
    }
}

/// One message's planned move: the source UID (in the source mailbox the
/// `by_source` key names) and the row fields that carry over to the
/// destination (flags, modseq, stable id).
#[derive(Clone, Debug)]
struct PlannedMove {
    message_id: String,
    uid: i64,
    flags_json: String,
    modseq: Option<i64>,
}

impl PlannedMove {
    /// The source mailbox's UIDVALIDITY, taken from the write context (the
    /// guard confirmed it matches what the location rows recorded).
    fn uid_validity_of(&self, ctx: &super::write::WriteContext) -> i64 {
        ctx.uidvalidity as i64
    }
}

/// The destination UID a source UID mapped to, from a `COPYUID` pairing.
fn dest_uid_for(copyuid: &super::write::CopyUid, source_uid: u32) -> Option<u32> {
    copyuid
        .pairs()
        .find(|(src, _)| *src == source_uid)
        .map(|(_, dest)| dest)
}

/// A human name for a move role, for error wording.
fn role_name(role: MoveRole) -> &'static str {
    match role {
        MoveRole::Inbox => "Inbox",
        MoveRole::Archive => "Archive",
        MoveRole::Trash => "Trash",
        MoveRole::Junk => "Junk",
    }
}

// ===========================================================================
// Phase 3 run 4: USER LABELS in label-folder mode (`label_storage == Folders`).
//
// A `lf:<name>` label is a COPY of a message in the mailbox
// `<container><delim><name>` (Proton Bridge: a message labelled N times has N
// copies). The engine delivers a user-label change through `modify_messages`
// (verified in `sync.rs`: `ThreadMutation::Label` resolves to its target
// message and calls `modify_messages`, never `modify_thread`), as
// `([lf:X], [])` to ADD or `([], [lf:X])` to REMOVE. `create_label` /
// `update_label` / `delete_label` manage the label mailboxes themselves.
//
// Everything here applies ONLY when `label_storage == Folders` with a
// configured container. In any other mode these all refuse permanently with
// `InvalidOperation` naming the mode, BEFORE any command.
//
// RENAME is DELIBERATELY NOT IMPLEMENTED (see `update_label` below): the `lf:`
// id is derived from the name, and the engine/database have no label-id
// migration (no label alias table; `update_label`'s returned `Label` is handed
// to the frontend, never used to re-point `threads.labels_json` rows), so a
// rename would silently orphan every stored `lf:<old>` association.
// ===========================================================================

/// The hierarchy delimiter the account uses, from any catalog row (every
/// mailbox on a server shares one), defaulting to `/`.
fn account_delimiter(delimiter: Option<&str>) -> &str {
    delimiter.filter(|d| !d.is_empty()).unwrap_or("/")
}

/// Build the label-folder mailbox name for a label `name` relative to the
/// container: `<container><delim><name>`. Pure; the inverse of
/// `plan::label_folder_relative_name`.
fn label_mailbox_name(container: &str, delimiter: &str, name: &str) -> String {
    format!("{container}{delimiter}{name}")
}

impl ImapProvider {
    /// This account's label container, or a permanent `InvalidOperation`
    /// naming the storage mode when label-folder mode is not configured. The
    /// single gate every `lf:` path passes through BEFORE any command, so a
    /// keyword-mode or no-label account refuses label-folder operations
    /// cleanly rather than touching the server.
    fn label_container(&self) -> ProviderResult<String> {
        match self.settings.label_storage {
            LabelStorage::Folders => self.settings.label_container.clone().ok_or_else(|| {
                ProviderError::InvalidOperation(
                    "this account stores labels as folders but has no label container configured; \
                     refusing the label operation permanently"
                        .into(),
                )
            }),
            LabelStorage::Keywords => Err(ProviderError::InvalidOperation(
                "keyword labels are not supported yet (this account's label_storage is 'keywords'); \
                 refusing permanently"
                    .into(),
            )),
            LabelStorage::None => Err(ProviderError::InvalidOperation(
                "this account has no user-label storage (label_storage is 'none'); \
                 refusing permanently"
                    .into(),
            )),
        }
    }

    /// The hierarchy delimiter from the persisted catalog (defaulting to `/`).
    fn delimiter(&self) -> ProviderResult<String> {
        let rows = self.store.mailboxes().map_err(db_err)?;
        Ok(account_delimiter(rows.iter().find_map(|r| r.delimiter.as_deref())).to_string())
    }

    /// The mailbox name a `lf:<name>` id refers to, resolved against this
    /// account's container + delimiter. Returns `None` for an id without the
    /// `lf:` prefix. Never consults the catalog, so it names the mailbox a
    /// label WOULD live in even before it is created (used by `create_label`).
    fn label_mailbox_for_id(&self, id: &str, delimiter: &str, container: &str) -> Option<String> {
        id.strip_prefix("lf:")
            .map(|name| label_mailbox_name(container, delimiter, name))
    }

    // --- add / remove a user label (modify_messages lf: path) --------------

    /// Apply a single `lf:<name>` ADD or REMOVE to a set of messages, in
    /// label-folder mode, idempotently and with partial-failure honesty. The
    /// request has already been classified ([`classify_message_mutation`]) to a
    /// single label id and a direction.
    ///
    /// ADD: for each message, if a copy already lives in the label mailbox it
    /// is satisfied (no command); otherwise COPY one existing synced copy into
    /// the label mailbox (source priority INBOX > Archive > a user folder >
    /// Sent; never Trash/Junk). A message with no usable source is a permanent
    /// error naming it. Idempotent: the destination row is recorded as soon as
    /// COPYUID confirms, so a retry after a partial failure never double-copies.
    ///
    /// REMOVE: delete only the copy in the label mailbox (needs `\Deleted`
    /// storable + UIDPLUS `UID EXPUNGE`). Refuses permanently, sending NO
    /// write, when removing the label copy would delete the message's LAST
    /// synced copy. A message with no copy in the label mailbox is satisfied.
    async fn apply_label_mutation(
        &self,
        message_ids: &[String],
        label_id: &str,
        add: bool,
    ) -> ProviderResult<()> {
        let container = self.label_container()?;
        let delimiter = self.delimiter()?;
        let label_mailbox = self
            .label_mailbox_for_id(label_id, &delimiter, &container)
            .ok_or_else(|| {
                ProviderError::InvalidOperation(format!(
                    "label id {label_id:?} is not a label-folder id; refusing permanently"
                ))
            })?;

        if add {
            self.apply_label_add(message_ids, label_id, &label_mailbox).await
        } else {
            self.apply_label_remove(message_ids, label_id, &label_mailbox).await
        }
    }

    /// ADD `lf:<name>` to every message: COPY one existing copy into the label
    /// mailbox, grouped by source mailbox over one leased session.
    async fn apply_label_add(
        &self,
        message_ids: &[String],
        label_id: &str,
        label_mailbox: &str,
    ) -> ProviderResult<()> {
        // The label mailbox must exist in the local catalog (never auto-create
        // on add — only create_label does that).
        let catalog: std::collections::BTreeSet<String> = self
            .store
            .mailboxes()
            .map_err(db_err)?
            .into_iter()
            .map(|m| m.name)
            .collect();
        if !catalog.contains(label_mailbox) {
            return Err(ProviderError::InvalidOperation(format!(
                "cannot add label {label_id:?}: its mailbox {label_mailbox:?} does not exist in \
                 the local catalog; create the label first (refusing permanently)"
            )));
        }

        let sync_plan = self.build_sync_plan()?;

        // Decide each message's COPY from LOCAL state (plan-then-apply). A
        // message already in the label mailbox is satisfied; otherwise pick its
        // best source copy by priority. A message with no usable source is a
        // permanent error naming it.
        let mut by_source: std::collections::BTreeMap<String, (i64, Vec<PlannedMove>)> =
            std::collections::BTreeMap::new();
        for message_id in message_ids {
            let locations = self.store.locations_for_message(message_id).map_err(db_err)?;
            if locations.iter().any(|l| l.mailbox == *label_mailbox) {
                self.store
                    .complete_copy(message_id, label_mailbox)
                    .map_err(db_err)?;
                continue; // Already labelled: no command.
            }
            let source = self
                .best_label_source(&sync_plan, &locations)
                .ok_or_else(|| {
                    ProviderError::InvalidOperation(format!(
                        "cannot add label {label_id:?} to message {message_id:?}: it has no synced \
                         copy outside Trash/Junk to copy from; refusing permanently"
                    ))
                })?;
            by_source
                .entry(source.mailbox.clone())
                .or_insert_with(|| (source.uidvalidity, Vec::new()))
                .1
                .push(PlannedMove {
                    message_id: message_id.clone(),
                    uid: source.uid,
                    flags_json: source.flags_json.clone(),
                    modseq: source.modseq,
                });
        }

        if by_source.is_empty() {
            // Every message is already in the label mailbox: no command.
            return Ok(());
        }

        let mut session = self.open_session().await?;
        let result = self
            .apply_label_add_on(session.as_mut(), label_mailbox, by_source)
            .await;
        let _ = session.logout().await;
        result
    }

    /// The best source copy to COPY into a label mailbox, by the brief's
    /// priority: INBOX, then Archive, then any user folder, then Sent. NEVER a
    /// copy whose only mailbox role is Trash or Junk (those are not a source).
    /// A copy in a label folder is not itself a source (we copy real mail, not
    /// a label view). Returns `None` when no usable source copy exists.
    fn best_label_source(
        &self,
        plan: &SyncPlan,
        locations: &[ImapLocation],
    ) -> Option<ImapLocation> {
        // Rank a location by its mailbox's resolved role: lower is better.
        // INBOX (0) > Archive (1) > user folder (2) > Sent (3); Trash/Junk and
        // label folders are NOT sources (None).
        let rank = |mailbox: &str| -> Option<u8> {
            if mailbox.eq_ignore_ascii_case(INBOX) {
                return Some(0);
            }
            let entry = plan.entry_for(mailbox)?;
            match entry.role {
                Some(super::MailboxRole::Archive) => Some(1),
                Some(super::MailboxRole::Sent) => Some(3),
                Some(super::MailboxRole::Trash) | Some(super::MailboxRole::Junk) => None,
                // Aggregates / Drafts carry no usable source either.
                Some(super::MailboxRole::All) | Some(super::MailboxRole::Drafts) => None,
                None => match entry.label_kind {
                    // A real user folder is a source; a label folder is not.
                    plan::LabelKind::UserFolder => Some(2),
                    _ => None,
                },
            }
        };
        locations
            .iter()
            .filter_map(|l| rank(&l.mailbox).map(|r| (r, l)))
            .min_by_key(|(r, l)| (*r, l.uid))
            .map(|(_, l)| l.clone())
    }

    /// The ADD against an open session: COPY each planned source copy into the
    /// label mailbox, one source mailbox at a time, recording the destination
    /// location row as soon as COPYUID confirms (idempotent retry).
    async fn apply_label_add_on(
        &self,
        session: &mut dyn ImapSession,
        label_mailbox: &str,
        by_source: std::collections::BTreeMap<String, (i64, Vec<PlannedMove>)>,
    ) -> ProviderResult<()> {
        for (source_mailbox, (uidvalidity, copies)) in by_source {
            let write_session = as_write_session(session)?;
            let ctx = super::write::select_for_write(
                write_session,
                &self.store,
                &source_mailbox,
                uidvalidity as u32,
            )
            .await?;
            for planned in &copies {
                self.copy_one_label(write_session, &ctx, label_mailbox, planned).await?;
            }
        }
        Ok(())
    }

    /// COPY one message's source copy into the label mailbox and record the
    /// destination location row (idempotent). A single-UID COPY so COPYUID maps
    /// unambiguously. If the destination already has this message locally (an
    /// earlier partial success), skip. A missing COPYUID or a durable intent
    /// from an uncertain attempt is reconciled by identity before success.
    async fn copy_one_label(
        &self,
        session: &mut dyn super::write::ImapWriteSession,
        ctx: &super::write::WriteContext,
        label_mailbox: &str,
        planned: &PlannedMove,
    ) -> ProviderResult<()> {
        let already = self
            .store
            .locations_for_message(&planned.message_id)
            .map_err(db_err)?
            .iter()
            .any(|l| l.mailbox == *label_mailbox);
        if already {
            return Ok(()); // The copy already landed (recorded) — idempotent.
        }
        let uid = planned.uid as u32;
        match self
            .copy_with_recovery(session, ctx, label_mailbox, planned)
            .await
        {
            Ok(outcome) => {
                if let Some(copyuid) = &outcome.copyuid {
                    if let Some(dest_uid) = dest_uid_for(copyuid, uid) {
                        self.store
                            .upsert_location(&ImapLocation {
                                mailbox: label_mailbox.to_string(),
                                uidvalidity: copyuid.uidvalidity as i64,
                                uid: dest_uid as i64,
                                message_id: planned.message_id.clone(),
                                flags_json: planned.flags_json.clone(),
                                modseq: planned.modseq,
                            })
                            .map_err(db_err)?;
                        self.store
                            .complete_copy(&planned.message_id, label_mailbox)
                            .map_err(db_err)?;
                    }
                } else {
                    log::warn!(
                        "imap add-label: no COPYUID for {} into {label_mailbox:?}; the next sweep \
                         will discover the labelled copy by Message-ID",
                        planned.message_id
                    );
                }
                Ok(())
            }
            Err(super::write::WriteError::TryCreate(text)) => {
                // The label mailbox vanished between the catalog check and the
                // COPY. Permanent: create_label owns creation, not add.
                Err(ProviderError::InvalidOperation(format!(
                    "cannot add label into {label_mailbox:?}: the server reports it does not exist \
                     (TRYCREATE: {text}); create the label first (refusing permanently)"
                )))
            }
            Err(super::write::WriteError::Provider(error)) => Err(error),
        }
    }

    /// REMOVE `lf:<name>` from every message: delete only the copy in the label
    /// mailbox, with the never-delete-the-last-copy guard and UIDPLUS
    /// requirement checked BEFORE any write.
    async fn apply_label_remove(
        &self,
        message_ids: &[String],
        label_id: &str,
        label_mailbox: &str,
    ) -> ProviderResult<()> {
        // Decide each message's delete from LOCAL state. A message with no copy
        // in the label mailbox is satisfied. The never-delete-the-last-copy
        // guard: if the label copy is the message's ONLY synced location,
        // refuse the WHOLE request permanently before any command.
        let mut by_uidvalidity: Option<i64> = None;
        let mut targets: Vec<PlannedMove> = Vec::new();
        for message_id in message_ids {
            let locations = self.store.locations_for_message(message_id).map_err(db_err)?;
            let in_label: Vec<&ImapLocation> =
                locations.iter().filter(|l| l.mailbox == *label_mailbox).collect();
            if in_label.is_empty() {
                continue; // No copy in the label mailbox: satisfied.
            }
            // The invariant: never delete the message's last synced copy.
            if locations.len() == in_label.len() {
                return Err(ProviderError::InvalidOperation(format!(
                    "cannot remove label {label_id:?} from message {message_id:?}: the label copy \
                     is its only synced location, so removing the label would delete the message; \
                     refusing permanently"
                )));
            }
            for location in in_label {
                // All copies in one mailbox share its UIDVALIDITY.
                by_uidvalidity.get_or_insert(location.uidvalidity);
                targets.push(PlannedMove {
                    message_id: message_id.clone(),
                    uid: location.uid,
                    flags_json: location.flags_json.clone(),
                    modseq: location.modseq,
                });
            }
        }

        if targets.is_empty() {
            return Ok(()); // Every message already lacks the label: no command.
        }
        let uidvalidity = by_uidvalidity.expect("non-empty targets carry a UIDVALIDITY");

        let mut session = self.open_session().await?;
        let result = self
            .apply_label_remove_on(session.as_mut(), label_mailbox, uidvalidity, &targets)
            .await;
        let _ = session.logout().await;
        result
    }

    /// The REMOVE against an open session: select the label mailbox for write
    /// (UIDPLUS gate + UIDVALIDITY guard), then per message mark `\Deleted` and
    /// `UID EXPUNGE` just its UID, deleting the local row only after the server
    /// delete confirms. A half-applied state (\Deleted stored, expunge failed)
    /// keeps the copy present+flagged and its retry just finishes it, with the
    /// cause's retry class preserved.
    async fn apply_label_remove_on(
        &self,
        session: &mut dyn ImapSession,
        label_mailbox: &str,
        uidvalidity: i64,
        targets: &[PlannedMove],
    ) -> ProviderResult<()> {
        self.confirm_surviving_copies(
            session,
            label_mailbox,
            targets.iter().map(|target| target.message_id.as_str()),
        )
        .await?;
        let write_session = as_write_session(session)?;
        let ctx = super::write::select_for_write(
            write_session,
            &self.store,
            label_mailbox,
            uidvalidity as u32,
        )
        .await?;
        // UIDPLUS is required: without a targeted UID EXPUNGE a plain EXPUNGE
        // would remove other clients' \Deleted mail, so refuse (nothing sent).
        if !ctx.caps.can_expunge_uids() {
            return Err(ProviderError::InvalidOperation(format!(
                "cannot remove a label copy from {label_mailbox:?}: the server does not advertise \
                 UIDPLUS, so a targeted UID EXPUNGE is unavailable and a plain EXPUNGE could delete \
                 other clients' messages; refusing permanently"
            )));
        }
        for planned in targets {
            // expunge_source_copies marks \Deleted then UID EXPUNGE of just this
            // UID; a half-applied state (expunge failed after \Deleted) keeps
            // the copy and its retry re-runs the same step. Delete the local
            // row only after the server delete confirms.
            super::write::expunge_source_copies(write_session, &ctx, &[planned.uid as u32]).await?;
            self.store
                .delete_location(label_mailbox, ctx.uidvalidity as i64, planned.uid)
                .map_err(db_err)?;
        }
        Ok(())
    }

    /// Local coordinates are only candidates: another client can expunge a
    /// copy between syncs. Before destroying a label copy, confirm a live,
    /// non-Deleted UID in another mailbox at the same UIDVALIDITY. Check the
    /// whole request before any write, using read-only, flags-only commands.
    async fn confirm_surviving_copies<'a>(
        &self,
        session: &mut dyn ImapSession,
        label_mailbox: &str,
        message_ids: impl Iterator<Item = &'a str> + Send,
    ) -> ProviderResult<()> {
        let mut unconfirmed: std::collections::BTreeSet<String> =
            message_ids.map(str::to_owned).collect();
        let mut candidates: std::collections::BTreeMap<String, Vec<ImapLocation>> =
            std::collections::BTreeMap::new();
        for message_id in &unconfirmed {
            for location in self
                .store
                .locations_for_message(message_id)
                .map_err(db_err)?
            {
                if location.mailbox != label_mailbox {
                    candidates
                        .entry(location.mailbox.clone())
                        .or_default()
                        .push(location);
                }
            }
        }
        for (mailbox, locations) in candidates {
            let status = match session.examine(&mailbox).await {
                Ok(status) => status,
                // A vanished/inaccessible mailbox provides no evidence of a
                // survivor. Transport/auth failures retain their retry class.
                Err(ProviderError::InvalidOperation(_)) => continue,
                Err(error) => return Err(error),
            };
            let locations: Vec<_> = locations
                .iter()
                .filter(|location| {
                    unconfirmed.contains(&location.message_id)
                        && status.uid_validity == Some(location.uidvalidity as u32)
                })
                .collect();
            let uids: Vec<_> = locations
                .iter()
                .map(|location| location.uid as u32)
                .collect();
            let flags = self.fetch_flags(session, &uids).await?;
            for location in locations {
                if flags.get(&(location.uid as u32)).is_some_and(|flags| {
                    !flags
                        .iter()
                        .any(|flag| flag.eq_ignore_ascii_case("\\Deleted"))
                }) {
                    unconfirmed.remove(&location.message_id);
                }
            }
            if unconfirmed.is_empty() {
                return Ok(());
            }
        }
        if unconfirmed.is_empty() {
            return Ok(());
        }
        Err(ProviderError::InvalidOperation(format!(
            "cannot delete copies in {label_mailbox:?}: {} message(s) have no verified surviving \
             server copy; resync before removing the label",
            unconfirmed.len()
        )))
    }

    // --- create / delete a label -------------------------------------------

    /// Create a label mailbox for `name` and add its catalog row so
    /// `list_labels` shows it immediately. Validation + the case-insensitive
    /// idempotent-alias check happen BEFORE any command. Returns the existing
    /// label when `name` aliases one case-insensitively (Proton labels are
    /// case-insensitive).
    async fn create_label_folder(&self, name: &str) -> ProviderResult<Label> {
        let container = self.label_container()?;
        let delimiter = self.delimiter()?;
        let clean = validate_label_name(name, &delimiter)?;

        // Case-insensitive alias check against the EXISTING labels: a label
        // whose name matches (ignoring case) is returned as-is (idempotent).
        let plan = self.build_sync_plan()?;
        for (id, _) in plan.dynamic_labels() {
            if let Some(existing) = id.strip_prefix("lf:") {
                let display = super::write::decode_modified_utf7(existing)?;
                if display.eq_ignore_ascii_case(&clean) {
                    return Ok(Label {
                        id: id.clone(),
                        name: display,
                        kind: "user".into(),
                    });
                }
            }
        }

        // Persist LIST-compatible wire names and IDs, as previous versions
        // do. Encode only the newly typed leaf, never the saved container.
        let wire_name = super::write::encode_modified_utf7(&clean);
        let label_mailbox = label_mailbox_name(&container, &delimiter, &wire_name);
        let arg = super::write::build_wire_mailbox_arg(&label_mailbox)?;

        let mut session = self.open_session().await?;
        let result = self
            .create_label_folder_on(session.as_mut(), &container, &label_mailbox, &arg)
            .await;
        let _ = session.logout().await;
        result?;

        // Record the catalog row so list_labels reflects it with no sync. A
        // label folder is synced, Folder-class; born with zero counters (the
        // next sweep fills them), special_use NULL, delimiter carried.
        self.store
            .upsert_mailbox_catalog(&label_mailbox, Some(&delimiter), None)
            .map_err(db_err)?;

        Ok(Label {
            id: format!("lf:{wire_name}"),
            name: clean,
            kind: "user".into(),
        })
    }

    /// CREATE the label mailbox over an open session. If the container itself
    /// is missing (`TRYCREATE`/`NONEXISTENT`), create the container first and
    /// retry once — the user asked for a label, so creating the container it
    /// lives under is acceptable here.
    async fn create_label_folder_on(
        &self,
        session: &mut dyn ImapSession,
        container: &str,
        label_mailbox: &str,
        arg: &str,
    ) -> ProviderResult<()> {
        let write_session = as_write_session(session)?;
        match write_session.create_mailbox_arg(arg).await {
            Ok(()) => Ok(()),
            Err(super::write::WriteError::TryCreate(_)) => {
                // The container does not exist yet: create it, then retry once.
                let container_arg = super::write::build_wire_mailbox_arg(container)?;
                if let Err(error) = write_session.create_mailbox_arg(&container_arg).await {
                    return Err(error.into_provider_error());
                }
                write_session
                    .create_mailbox_arg(arg)
                    .await
                    .map_err(|e| e.into_provider_error())
                    .map(|()| {
                        log::info!(
                            "imap create_label: created missing container {container:?} before \
                             {label_mailbox:?}"
                        );
                    })
            }
            Err(super::write::WriteError::Provider(error)) => Err(error),
        }
    }

    /// Delete a label mailbox only when every known message has another
    /// synced copy and the server's UIDVALIDITY and UID set match our locations
    /// (no unsynced mail we cannot vouch for). Otherwise a
    /// permanent error. Deleting an already-absent label is idempotent success.
    async fn delete_label_folder(&self, id: &str) -> ProviderResult<()> {
        let container = self.label_container()?;
        let delimiter = self.delimiter()?;
        let label_mailbox = self
            .label_mailbox_for_id(id, &delimiter, &container)
            .ok_or_else(|| {
                ProviderError::InvalidOperation(format!(
                    "label id {id:?} is not a label-folder id; refusing permanently"
                ))
            })?;

        // Idempotent: an already-absent label (no catalog row) is success.
        let catalog: std::collections::BTreeSet<String> = self
            .store
            .mailboxes()
            .map_err(db_err)?
            .into_iter()
            .map(|m| m.name)
            .collect();
        if !catalog.contains(&label_mailbox) {
            return Ok(());
        }

        // Safety: every location in the label mailbox must belong to a message
        // with at least one OTHER synced location.
        let locations = self.store.locations_in_mailbox(&label_mailbox).map_err(db_err)?;
        let mut would_lose = 0usize;
        for location in &locations {
            let others = self
                .store
                .locations_for_message(&location.message_id)
                .map_err(db_err)?
                .into_iter()
                .filter(|l| l.mailbox != label_mailbox)
                .count();
            if others == 0 {
                would_lose += 1;
            }
        }
        if would_lose > 0 {
            return Err(ProviderError::InvalidOperation(format!(
                "cannot delete label {id:?}: {would_lose} message(s) have no other synced copy, so \
                 deleting its mailbox {label_mailbox:?} would lose them; refusing permanently"
            )));
        }

        // Check both the count and exact UID coordinates on the server;
        // counts alone miss replacements of known mail by unsynced mail.
        let mut session = self.open_session().await?;
        let result = self
            .delete_label_folder_on(session.as_mut(), &label_mailbox, &locations)
            .await;
        let _ = session.logout().await;
        result?;

        // Retire the catalog row, sync-state and locations in one transaction,
        // journaling affected hot threads — reusing the slice 5b-2 retirement
        // helper by passing the full present catalog MINUS the deleted label,
        // so ONLY that mailbox is retired (nothing else is absent from the set).
        let present: Vec<String> = catalog
            .into_iter()
            .filter(|name| name != &label_mailbox)
            .collect();
        self.store.retire_vanished_mailboxes(&present).map_err(db_err)?;
        Ok(())
    }

    /// EXAMINE and SEARCH verify the exact UID coordinates before DELETE.
    /// A mismatch refuses deletion. Select INBOX before deleting the mailbox.
    async fn delete_label_folder_on(
        &self,
        session: &mut dyn ImapSession,
        label_mailbox: &str,
        locations: &[ImapLocation],
    ) -> ProviderResult<()> {
        self.confirm_surviving_copies(
            session,
            label_mailbox,
            locations
                .iter()
                .map(|location| location.message_id.as_str()),
        )
        .await?;
        let local_count = locations.len();
        let status = session.examine(label_mailbox).await?;
        if status.exists as usize != local_count {
            return Err(ProviderError::InvalidOperation(format!(
                "cannot delete label mailbox {label_mailbox:?}: the server reports {} message(s) \
                 but the local store knows only {local_count}, so some mail is unsynced and could \
                 be lost; refusing permanently",
                status.exists
            )));
        }
        let server_uids: std::collections::BTreeSet<u32> =
            session.uid_search("ALL").await?.into_iter().collect();
        let local_uids: std::collections::BTreeSet<u32> =
            locations.iter().map(|l| l.uid as u32).collect();
        if locations
            .iter()
            .any(|l| status.uid_validity != Some(l.uidvalidity as u32))
            || server_uids != local_uids
        {
            return Err(ProviderError::InvalidOperation(format!(
                "cannot delete label mailbox {label_mailbox:?}: its UIDVALIDITY or message UIDs changed; \
                 unsynced mail could be lost, so resync first"
            )));
        }
        // Never DELETE the selected mailbox: select INBOX first.
        session.select(INBOX).await?;
        let write_session = as_write_session(session)?;
        let arg = super::write::build_wire_mailbox_arg(label_mailbox)?;
        write_session.delete_mailbox_arg(&arg).await
    }
}

/// Validate a user-supplied label name (the part relative to the container):
/// non-empty after trimming, no control characters, must not contain the
/// server delimiter, no leading/trailing space or delimiter, within
/// [`policy::MAX_LABEL_NAME_CHARS`]. Returns the TRIMMED name. A permanent
/// `InvalidOperation` describes any violation.
fn validate_label_name(name: &str, delimiter: &str) -> ProviderResult<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(ProviderError::InvalidOperation(
            "a label name cannot be empty; refusing permanently".into(),
        ));
    }
    policy::check_label_name_len(trimmed.chars().count())?;
    if trimmed.chars().any(|c| c.is_control()) {
        return Err(ProviderError::InvalidOperation(
            "a label name cannot contain a control character; refusing permanently".into(),
        ));
    }
    if !delimiter.is_empty() && trimmed.contains(delimiter) {
        return Err(ProviderError::InvalidOperation(format!(
            "a label name cannot contain the server hierarchy delimiter {delimiter:?}; \
             refusing permanently"
        )));
    }
    // The trimmed name has no leading/trailing space by construction; also
    // reject a leading/trailing delimiter char (e.g. a single-char delimiter).
    if let Some(first) = delimiter.chars().next() {
        if trimmed.starts_with(first) || trimmed.ends_with(first) {
            return Err(ProviderError::InvalidOperation(format!(
                "a label name cannot start or end with the hierarchy delimiter {delimiter:?}; \
                 refusing permanently"
            )));
        }
    }
    Ok(trimmed.to_string())
}

/// A classified `modify_messages` request in label-folder terms: either a flag
/// change (handled by run 2's flag path) or a single `lf:` add/remove.
enum MessageMutationKind {
    /// Only `UNREAD`/`STARRED` ids — the run-2 flag path.
    Flag,
    /// A single `lf:<name>` ADD (`add == true`) or REMOVE (`add == false`).
    Label { id: String, add: bool },
}

/// Classify a `modify_messages` `(add, remove)` request. A request naming no
/// `lf:` id is the flag path (run 2's `plan_flag_mutation` still rejects a
/// `folder:`/system/`kw:`/unknown id or an id in both lists). A request that
/// names exactly ONE `lf:` id — `([lf:X], [])` or `([], [lf:X])`, the two
/// shapes the engine delivers a user-label change in — is the label path.
/// Anything else (a label mixed with another id, several labels, a label in
/// both lists, a label alongside a flag) is a permanent `InvalidOperation`
/// BEFORE any command.
fn classify_message_mutation(
    add: &[String],
    remove: &[String],
) -> ProviderResult<MessageMutationKind> {
    let is_lf = |id: &String| id.starts_with("lf:");
    let any_lf = add.iter().chain(remove.iter()).any(is_lf);
    if !any_lf {
        return Ok(MessageMutationKind::Flag);
    }
    // Exactly one lf: id, alone, in exactly one list.
    match (add, remove) {
        ([only], []) if is_lf(only) => {
            Ok(MessageMutationKind::Label { id: only.clone(), add: true })
        }
        ([], [only]) if is_lf(only) => {
            Ok(MessageMutationKind::Label { id: only.clone(), add: false })
        }
        _ => Err(ProviderError::InvalidOperation(format!(
            "the IMAP provider accepts a user-label change only as a single lf: id added or \
             removed on its own ([lf:X],[]) or ([],[lf:X]); the request add={add:?} remove={remove:?} \
             mixes a label with other ids, names several, or repeats one; refusing permanently"
        ))),
    }
}

#[async_trait]
impl MailMutate for ImapProvider {
    async fn modify_thread(
        &self,
        id: &str,
        add: &[String],
        remove: &[String],
    ) -> ProviderResult<()> {
        // A thread id may be an alias: resolve it, then act on every message
        // of the surviving thread. An unknown/emptied thread resolves to no
        // messages and `apply_flag_mutation` rejects it as a permanent
        // InvalidOperation naming the thread.
        let thread_id = self.store.resolve_thread_alias(id).map_err(db_err)?;
        // `imap_threads` rows are permanent, so a thread still lists messages
        // that were expunged elsewhere. Only messages that still have a
        // location can be changed; skipping the rest keeps one dead sibling
        // from failing (and rolling back) the whole thread-level change.
        let mut message_ids = Vec::new();
        for message_id in self.store.messages_in_thread(&thread_id).map_err(db_err)? {
            if !self
                .store
                .locations_for_message(&message_id)
                .map_err(db_err)?
                .is_empty()
            {
                message_ids.push(message_id);
            }
        }
        if message_ids.is_empty() {
            return Err(ProviderError::InvalidOperation(format!(
                "thread {id:?} has no messages to modify (it may have been emptied); \
                 refusing a flag change permanently"
            )));
        }
        // A `modify_thread` request is EITHER a flag change (UNREAD/STARRED)
        // or a system-label MOVE (archive/trash/spam and their reverses). The
        // engine never mixes the two (`sync.rs::mutation_labels`), so classify
        // the request first and route it; a shape that is neither — a mix, a
        // system id combined with a flag or `lf:`/`folder:`/SENT, or an id in
        // both lists — is a permanent `InvalidOperation` BEFORE any command.
        match classify_thread_mutation(add, remove)? {
            ThreadMutationKind::Flag => self.apply_flag_mutation(&message_ids, add, remove).await,
            ThreadMutationKind::SystemMove(move_plan) => {
                self.apply_system_move(&message_ids, move_plan).await
            }
        }
    }

    async fn modify_messages(
        &self,
        ids: &[String],
        add: &[String],
        remove: &[String],
    ) -> ProviderResult<()> {
        if ids.is_empty() {
            return Err(ProviderError::InvalidOperation(
                "modify_messages called with no message ids".into(),
            ));
        }
        // A `modify_messages` request is EITHER a flag change (UNREAD/STARRED)
        // or a single user-label (`lf:`) add/remove — the two shapes the engine
        // delivers through this method. Classify first (rejecting a mixed /
        // multi-label / folder: / unknown shape permanently BEFORE any
        // command), then route.
        match classify_message_mutation(add, remove)? {
            MessageMutationKind::Flag => self.apply_flag_mutation(ids, add, remove).await,
            MessageMutationKind::Label { id, add } => {
                self.apply_label_mutation(ids, &id, add).await
            }
        }
    }

    async fn list_labels(&self) -> ProviderResult<Vec<Label>> {
        // The six system labels first, in a fixed order, then the account's
        // dynamic labels read from the LOCAL plan/catalog (no network): `lf:`
        // labels (kind `user`, name = path relative to the container) for every
        // label-folder child, then `folder:` labels (kind `folder`, name = full
        // mailbox name) for every user folder — whether or not any message is
        // currently in them (fact 2 / item 3). The frontend resolves label ids
        // through this catalog, so an id absent here renders as raw text.
        let mut labels = vec![
            Label {
                id: "INBOX".into(),
                name: "Inbox".into(),
                kind: "system".into(),
            },
            Label {
                id: "SENT".into(),
                name: "Sent".into(),
                kind: "system".into(),
            },
            Label {
                id: "SPAM".into(),
                name: "Spam".into(),
                kind: "system".into(),
            },
            Label {
                id: "TRASH".into(),
                name: "Trash".into(),
                kind: "system".into(),
            },
            Label {
                id: "UNREAD".into(),
                name: "Unread".into(),
                kind: "system".into(),
            },
            Label {
                id: "STARRED".into(),
                name: "Starred".into(),
                kind: "system".into(),
            },
        ];
        let plan = self.build_sync_plan()?;
        for (id, kind) in plan.dynamic_labels() {
            // Display name: the id with its `lf:` / `folder:` prefix stripped
            // (the relative label path, or the full user-folder name).
            let name = id
                .strip_prefix("lf:")
                .or_else(|| id.strip_prefix("folder:"))
                .unwrap_or(&id)
                .to_string();
            let name = super::write::decode_modified_utf7(&name).unwrap_or(name);
            labels.push(Label {
                id,
                name,
                kind: kind.to_string(),
            });
        }
        Ok(labels)
    }

    async fn create_label(&self, name: &str) -> ProviderResult<Label> {
        self.create_label_folder(name).await
    }

    async fn update_label(&self, _id: &str, _name: &str) -> ProviderResult<Label> {
        // RENAME is deliberately NOT implemented. The `lf:` label id is derived
        // from the label NAME, so a rename changes the id; but the engine and
        // database have no way to migrate stored label ids: there is no label
        // alias table (unlike thread aliases), and `update_label`'s returned
        // `Label` is handed straight to the frontend, never used to re-point
        // the `threads.labels_json` rows that carry `lf:<old>`. Renaming would
        // therefore silently orphan every message/thread labelled with the old
        // id until each is independently re-ingested. Until a label-id
        // migration exists, refuse permanently with a plain-language reason.
        Err(ProviderError::InvalidOperation(
            "renaming a label is not supported on this account: a label-folder label's id is \
             derived from its name, and there is no safe migration of the label id already stored \
             on your messages and threads, so a rename would detach them from the label. Create a \
             new label and move the messages instead. (Refusing permanently.)"
                .into(),
        ))
    }

    async fn delete_label(&self, id: &str) -> ProviderResult<()> {
        self.delete_label_folder(id).await
    }
}

#[async_trait]
impl MailSend for ImapProvider {
    async fn sender_identity(&self) -> ProviderResult<String> {
        // The ACCOUNT address (the key drafts are filed under), NOT the IMAP
        // login. The tick pauses every item whose `draft.account` differs from
        // this value, so returning the login would silently pause ALL sends
        // for an account whose login differs from its address (e.g. Proton
        // Bridge login `joel` for `joel@proton.me`). `docs/imap-design.md`
        // ("Identities"), brief fact 6. The login is still what IMAP LOGIN and
        // SMTP AUTH use — that is unchanged, it just is not the identity.
        Ok(self.account_id.clone())
    }

    async fn identities(&self, _account: &str) -> ProviderResult<Vec<super::SendingIdentity>> {
        // The one source of truth: the ACCOUNT address first and always, then
        // the stored list, normalized/validated. The first entry is the
        // account's own sending address (its key), not the login, for the same
        // reason `sender_identity()` returns the account address. No DB round
        // trip — the settings are carried in the provider.
        Ok(super::account_identities(
            &self.account_id,
            &self.settings.identities,
        ))
    }

    fn message_id_domain(&self, from_address: &str) -> ProviderResult<Option<String>> {
        // An IMAP send generates `<id@sender-domain>`, where the domain is the
        // domain of the From address actually used. An invalid domain is an
        // error BEFORE any send — never a silent fallback to the reserved
        // `threestrands.local` (which would leak the client name).
        super::message_id_domain_for_address(from_address).map(Some)
    }

    async fn prepare_delivery(&self) -> ProviderResult<Box<dyn Delivery>> {
        // Build the endpoint + credentials from the account's non-secret
        // settings and the IMAP password (reused as the SMTP password per the
        // user's decision). An empty host / zero port is InvalidOperation
        // BEFORE any network. The password is never formatted — it goes only
        // into SmtpCredentials (which redacts Debug).
        let endpoint = super::send::smtp_endpoint_from_settings(&self.settings)?;
        let creds = super::send::smtp_credentials(&self.settings, &self.smtp_password);

        // Connect + authenticate. Any failure here is a DEFINITE non-delivery
        // (nothing sent); map it to the existing ProviderError class the
        // run-1 client already chose (ReauthenticationRequired stays that
        // class, TLS/connect/timeout stays TransientTransport, etc.).
        let session = super::smtp::SmtpSession::connect(&endpoint, &creds)
            .await
            .map_err(super::send::prepare_failure_to_error)?;

        // The permit owns the live session plus what send_once needs to derive
        // the envelope and the receipt: the account address (for the provider
        // id) and the identity list (to validate the From).
        let identities = super::account_identities(&self.account_id, &self.settings.identities);
        Ok(Box::new(super::send::ImapDelivery::new(
            session,
            self.account_id.clone(),
            identities,
        )))
    }

    async fn find_sent_copy(
        &self,
        _operation: &str,
        _expected_sender: &str,
    ) -> ProviderResult<Option<DeliveryReceipt>> {
        // IMAP keeps no server-side sent copy we can confirm an uncertain send
        // against: only WE would APPEND to `\Sent`, and if the connection
        // dropped before we could, there is nothing to look up. So this is a
        // clean `Ok(None)`, NOT an error — `reconcile_with_provider` reads it
        // together with `verifiable_delivery = false` and resolves the send to
        // the terminal `Unverifiable` state ("may have been sent"). Returning
        // an error here instead would make `reconcile_with_provider` abort via
        // its `map_err(error)?` and the send would never reach that branch.
        // (Run 4 may add the Sent-copy APPEND; it would revisit this.)
        Ok(None)
    }

    /// Save the sender's own copy of a just-sent message into the Sent mailbox,
    /// or decide nothing is needed, driven by the engine's durable job (run 4b;
    /// `docs/imap-design.md`, "Sending" steps 3–4). The whole decision is the
    /// pure [`super::write::decide_sent_copy`] over a leased-session IO; this
    /// method only resolves the inputs (Message-ID, Sent mailbox, override) and
    /// leases the session.
    async fn save_sent_copy(
        &self,
        request: &crate::provider::SentCopyRequest,
    ) -> ProviderResult<crate::provider::SentCopyStep> {
        // Nothing needs locating or uploading when the server owns Sent
        // copies. This decision must precede mailbox resolution and network IO.
        if self.settings.server_saves_sent
            || self.store.sent_copy_detection().map_err(db_err)? == Some(true)
        {
            return Ok(crate::provider::SentCopyStep::Done);
        }
        // The Message-ID is required to locate our copy; a message without one
        // can never be found again, so refuse PERMANENTLY (abandons the job).
        let message_id = super::send::raw_message_id(&request.raw).ok_or_else(|| {
            ProviderError::InvalidOperation(
                "cannot save the Sent copy: the message has no Message-ID header to locate it by; \
                 refusing permanently"
                    .into(),
            )
        })?;

        // Resolve the Sent mailbox from the plan exactly as run 4a does (a WIRE
        // name; never re-encoded). A missing Sent mailbox is permanent.
        let plan = self.build_sync_plan()?;
        let mailbox = self
            .mailbox_with_role(&plan, super::MailboxRole::Sent)
            .ok_or_else(|| {
                ProviderError::InvalidOperation(
                    "cannot save the Sent copy: this account has no Sent mailbox configured; \
                     refusing permanently"
                        .into(),
                )
            })?;

        // Lease one session for the whole decision (the command slot, like
        // every other write path).
        let mut session = self.open_session().await?;
        let result = {
            let write_session = as_write_session(session.as_mut())?;
            let mut io = super::write::SessionSentCopyIo {
                session: write_session,
                store: &self.store,
                mailbox,
                raw: &request.raw,
                message_id,
                server_saves_sent: self.settings.server_saves_sent,
                config: super::write::SaveSentConfig::default(),
                now: self.now,
            };
            super::write::decide_sent_copy(&mut io, request, super::SENT_COPY_VERIFY_DELAY).await
        };
        let _ = session.logout().await;
        result
    }
}

impl MailProvider for ImapProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            send_supported: true,
            server_search: false,
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
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::provider::MailSync;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

    /// One seeded message in a fake folder: its UID, flags, and raw RFC 5322
    /// bytes. `advertised_size`, when set, overrides the RFC822.SIZE the
    /// identity pass reports (used to simulate an oversize message without
    /// allocating 64 MiB).
    #[derive(Clone)]
    struct FakeMessage {
        uid: u32,
        flags: Vec<String>,
        raw: String,
        advertised_size: Option<usize>,
    }

    /// One named mailbox (folder) on the fake server: its UIDVALIDITY, next
    /// UID, and messages. `fail_after_body_fetches`, when set, makes the server
    /// DROP the connection after that many whole-body fetches in one session.
    #[derive(Clone, Default)]
    struct FakeFolder {
        uidvalidity: u32,
        uidnext: u32,
        messages: Vec<FakeMessage>,
        fail_after_body_fetches: Option<usize>,
        /// EXAMINE of this folder answers `NO [NONEXISTENT]` (a catalog row
        /// whose mailbox was renamed or deleted on the server).
        examine_fails: bool,
    }

    impl FakeFolder {
        fn new(uidvalidity: u32) -> Self {
            Self {
                uidvalidity,
                uidnext: 1,
                messages: Vec::new(),
                fail_after_body_fetches: None,
                examine_fails: false,
            }
        }
        fn add_sized(&mut self, flags: &[&str], raw: &str, advertised_size: Option<usize>) -> u32 {
            let uid = self.uidnext;
            self.uidnext += 1;
            self.messages.push(FakeMessage {
                uid,
                flags: flags.iter().map(|f| f.to_string()).collect(),
                raw: raw.to_string(),
                advertised_size,
            });
            uid
        }
    }

    /// A tiny in-memory server with one or more named mailboxes, shared
    /// (Arc<Mutex>) so a test can mutate it (add mail, change a flag, expunge,
    /// move a message between folders) between polls and the next opened
    /// session sees the change. `FakeMailbox::new` creates an INBOX, and the
    /// backward-compatible `add`/`add_sized` operate on INBOX, so the Slice 5a
    /// tests are unchanged; Slice 5b-1 tests add Trash/Junk/Sent folders.
    ///
    /// `command_counts` records how many SEARCH / whole-body / identity fetches
    /// the server answered per mailbox, so a cadence test can assert that an
    /// unchanged, not-due folder issued no SEARCH/FETCH.
    #[derive(Clone, Default)]
    struct FakeMailbox {
        folders: std::collections::BTreeMap<String, FakeFolder>,
        /// Per-mailbox `(searches, body_fetches, identity_fetches)` counters,
        /// accumulated across every session opened against this snapshot's
        /// shared handle. Shared so a test reads them after sync.
        command_counts: Arc<Mutex<std::collections::BTreeMap<String, (usize, usize, usize)>>>,
        /// When true, the server answers `LIST` with a NO (catalog refresh must
        /// swallow it and never abort the poll).
        list_fails: bool,
        /// Extra `\Noselect` container names the server reports in `LIST` (they
        /// must never be added to the catalog).
        noselect: Vec<String>,
    }

    impl FakeMailbox {
        /// A server with just an INBOX at the given UIDVALIDITY.
        fn new(uidvalidity: u32) -> Self {
            let mut folders = std::collections::BTreeMap::new();
            folders.insert("INBOX".to_string(), FakeFolder::new(uidvalidity));
            Self {
                folders,
                command_counts: Arc::new(Mutex::new(Default::default())),
                list_fails: false,
                noselect: Vec::new(),
            }
        }
        /// Ensure a folder exists at `uidvalidity`, returning a mutable handle.
        fn folder(&mut self, name: &str, uidvalidity: u32) -> &mut FakeFolder {
            self.folders
                .entry(name.to_string())
                .or_insert_with(|| FakeFolder::new(uidvalidity))
        }
        /// Mutable INBOX handle (always present).
        fn inbox(&mut self) -> &mut FakeFolder {
            self.folders.get_mut("INBOX").expect("INBOX always present")
        }
        fn add(&mut self, flags: &[&str], raw: &str) -> u32 {
            self.inbox().add_sized(flags, raw, None)
        }
        fn add_sized(&mut self, flags: &[&str], raw: &str, advertised_size: Option<usize>) -> u32 {
            self.inbox().add_sized(flags, raw, advertised_size)
        }
        /// Add a message to a named folder (created at `uidvalidity` if new).
        fn add_to(&mut self, mailbox: &str, uidvalidity: u32, flags: &[&str], raw: &str) -> u32 {
            self.folder(mailbox, uidvalidity).add_sized(flags, raw, None)
        }
        /// The command counts seen for a mailbox so far.
        fn counts(&self, mailbox: &str) -> (usize, usize, usize) {
            self.command_counts
                .lock()
                .unwrap()
                .get(mailbox)
                .copied()
                .unwrap_or((0, 0, 0))
        }
    }

    /// A scripted IMAP server that answers EXAMINE / UID SEARCH / UID FETCH /
    /// LOGOUT from a `FakeMailbox` snapshot, over a tokio duplex stream.
    /// EXAMINE selects the active folder; subsequent SEARCH/FETCH act on it. It
    /// parses commands by TYPE, so it is robust to order and batching.
    fn spawn_server(mailbox: FakeMailbox) -> (DuplexStream, tokio::task::JoinHandle<()>) {
        let (client, server) = tokio::io::duplex(1 << 20);
        let counts = mailbox.command_counts.clone();
        let folders = mailbox.folders.clone();
        let list_fails = mailbox.list_fails;
        let noselect = mailbox.noselect.clone();
        let task = tokio::spawn(async move {
            let mut server = BufReader::new(server);
            server.get_mut().write_all(b"* OK ready\r\n").await.unwrap();
            let mut body_fetches = 0usize;
            // The currently EXAMINEd folder; defaults to INBOX.
            let mut selected = "INBOX".to_string();
            loop {
                let mut line = String::new();
                if server.read_line(&mut line).await.unwrap() == 0 {
                    break;
                }
                let line = line.trim_end();
                let Some((tag, rest)) = line.split_once(' ') else {
                    continue;
                };
                let upper = rest.to_ascii_uppercase();
                let empty = FakeFolder::default();
                if upper.starts_with("LOGIN") {
                    reply(&mut server, &format!("{tag} OK logged in\r\n")).await;
                } else if upper.starts_with("LIST") {
                    if list_fails {
                        reply(&mut server, &format!("{tag} NO LIST failed\r\n")).await;
                        continue;
                    }
                    // Enumerate every folder with its inferred special-use
                    // attribute, so the catalog refresh can notice new ones.
                    let mut body = String::new();
                    for name in folders.keys() {
                        let attrs = infer_special_use(name)
                            .map(|a| format!("{a} "))
                            .unwrap_or_default();
                        body.push_str(&format!("* LIST ({}) \"/\" \"{}\"\r\n", attrs.trim(), name));
                    }
                    // Any \Noselect containers the server advertises.
                    for name in &noselect {
                        body.push_str(&format!("* LIST (\\Noselect) \"/\" \"{name}\"\r\n"));
                    }
                    body.push_str(&format!("{tag} OK done\r\n"));
                    reply(&mut server, &body).await;
                } else if upper.starts_with("EXAMINE") {
                    // "EXAMINE <mailbox>" — select it. The name may be quoted.
                    let name = rest["EXAMINE".len().min(rest.len())..]
                        .trim()
                        .trim_matches('"')
                        .to_string();
                    selected = if name.is_empty() { "INBOX".into() } else { name };
                    let folder = folders.get(&selected).unwrap_or(&empty);
                    if folder.examine_fails {
                        reply(
                            &mut server,
                            &format!("{tag} NO [NONEXISTENT] Mailbox does not exist\r\n"),
                        )
                        .await;
                        continue;
                    }
                    let body = format!(
                        "* {} EXISTS\r\n* OK [UIDVALIDITY {}] .\r\n* OK [UIDNEXT {}] .\r\n{tag} OK [READ-ONLY] done\r\n",
                        folder.messages.len(),
                        folder.uidvalidity,
                        folder.uidnext
                    );
                    reply(&mut server, &body).await;
                } else if upper.starts_with("UID SEARCH") {
                    let folder = folders.get(&selected).unwrap_or(&empty);
                    counts.lock().unwrap().entry(selected.clone()).or_default().0 += 1;
                    let uids: Vec<String> =
                        folder.messages.iter().map(|m| m.uid.to_string()).collect();
                    let body = format!("* SEARCH {}\r\n{tag} OK done\r\n", uids.join(" "));
                    reply(&mut server, &body).await;
                } else if upper.starts_with("UID FETCH") {
                    let folder = folders.get(&selected).unwrap_or(&empty);
                    let wants_body = upper.contains("BODY.PEEK[]");
                    let is_identity = upper.contains("HEADER.FIELDS");
                    {
                        let mut c = counts.lock().unwrap();
                        let entry = c.entry(selected.clone()).or_default();
                        if wants_body {
                            entry.1 += 1;
                        } else if is_identity {
                            entry.2 += 1;
                        }
                    }
                    if wants_body {
                        if let Some(limit) = folder.fail_after_body_fetches {
                            if body_fetches >= limit {
                                let uid = parse_uid_set(rest, folder)
                                    .first()
                                    .copied()
                                    .unwrap_or(0);
                                reply(
                                    &mut server,
                                    &format!("* 1 FETCH (UID {uid} BODY[]<0> {{4096}}\r\n"),
                                )
                                .await;
                                break;
                            }
                        }
                        body_fetches += 1;
                    }
                    let flags_only = !wants_body && !is_identity;
                    let set = parse_uid_set(rest, folder);
                    let mut out = String::new();
                    for (seq, uid) in set.iter().enumerate() {
                        if let Some(message) = folder.messages.iter().find(|m| &m.uid == uid) {
                            out.push_str(&fetch_response(
                                seq as u32 + 1,
                                message,
                                wants_body,
                                flags_only,
                            ));
                        }
                    }
                    out.push_str(&format!("{tag} OK done\r\n"));
                    reply(&mut server, &out).await;
                } else if upper.starts_with("LOGOUT") {
                    reply(&mut server, &format!("* BYE\r\n{tag} OK logout\r\n")).await;
                    break;
                } else {
                    reply(&mut server, &format!("{tag} OK done\r\n")).await;
                }
            }
        });
        (client, task)
    }

    async fn reply(server: &mut BufReader<DuplexStream>, text: &str) {
        server.get_mut().write_all(text.as_bytes()).await.unwrap();
    }

    /// Expand a UID set token ("1", "1,2,3") against a folder.
    fn parse_uid_set(command: &str, folder: &FakeFolder) -> Vec<u32> {
        // The set is the token after "UID FETCH ".
        let after = command["UID FETCH ".len().min(command.len())..].trim_start();
        let set_token = after.split_whitespace().next().unwrap_or("");
        let mut uids = Vec::new();
        for part in set_token.split(',') {
            if let Ok(uid) = part.parse::<u32>() {
                if folder.messages.iter().any(|m| m.uid == uid) {
                    uids.push(uid);
                }
            }
        }
        uids
    }

    /// Render one FETCH response line for a message. `wants_body` is the whole
    /// `BODY.PEEK[]` pass; `flags_only` is the FLAGS-only sweep; otherwise it
    /// is the identity pass.
    fn fetch_response(seq: u32, message: &FakeMessage, wants_body: bool, flags_only: bool) -> String {
        let flags = message.flags.join(" ");
        if wants_body {
            format!(
                "* {seq} FETCH (UID {} BODY[]<0> {{{}}}\r\n{})\r\n",
                message.uid,
                message.raw.len(),
                message.raw
            )
        } else if flags_only {
            format!("* {seq} FETCH (UID {} FLAGS ({}))\r\n", message.uid, flags)
        } else {
            // Header fields block = the message's header section (up to the
            // blank line). The real server returns only the requested fields,
            // but returning the whole header block is a valid superset the
            // parser tolerates.
            let header_block = message
                .raw
                .split_once("\r\n\r\n")
                .map(|(h, _)| format!("{h}\r\n\r\n"))
                .unwrap_or_else(|| message.raw.clone());
            let size = message.advertised_size.unwrap_or(message.raw.len());
            format!(
                "* {seq} FETCH (UID {} FLAGS ({}) RFC822.SIZE {} BODY[HEADER.FIELDS (MESSAGE-ID DATE FROM SUBJECT)] {{{}}}\r\n{})\r\n",
                message.uid,
                flags,
                size,
                header_block.len(),
                header_block
            )
        }
    }

    /// A session source that, each time it is opened, logs into a freshly
    /// spawned scripted server serving the CURRENT shared mailbox snapshot.
    struct FakeSource {
        mailbox: Arc<Mutex<FakeMailbox>>,
    }

    #[async_trait]
    impl ImapSessionSource for FakeSource {
        async fn open(&self) -> ProviderResult<Box<dyn ImapSession>> {
            let snapshot = self.mailbox.lock().unwrap().clone();
            let (client, _task) = spawn_server(snapshot);
            let session = async_imap::Client::new(client)
                .login("user", "password")
                .await
                .map_err(|(error, _)| super::super::error::map_imap_error(&error))?;
            Ok(Box::new(super::super::session::AsyncImapSession::new(session)))
        }
    }

    /// Open a bare scripted session over a one-shot server serving `mailbox`,
    /// for tests that drive `refresh_inbox_with` directly (so a failure can be
    /// injected into ONE round).
    async fn open_scripted_session(mailbox: FakeMailbox) -> Box<dyn ImapSession> {
        let (client, _task) = spawn_server(mailbox);
        let session = async_imap::Client::new(client)
            .login("user", "password")
            .await
            .map_err(|(error, _)| super::super::error::map_imap_error(&error))
            .expect("scripted login");
        Box::new(super::super::session::AsyncImapSession::new(session))
    }

    /// Build an ImapProvider backed by the shared fake mailbox and an
    /// in-memory database. Seeds a catalog row for every folder present on the
    /// fake server (inferring the special-use attribute from the folder name)
    /// so the sync plan resolves Trash/Junk/Sent roles exactly as discovery
    /// would. INBOX-only tests get an INBOX catalog row and nothing else.
    fn provider_with(mailbox: Arc<Mutex<FakeMailbox>>) -> (ImapProvider, ImapStateStore) {
        let database = Arc::new(Database::open_memory());
        database
            .adopt_mail_account("me@example.com", crate::models::MailProviderKind::Imap)
            .unwrap();
        let store = ImapStateStore::new(database.clone(), "me@example.com");

        // Seed the catalog from the fake server's folders.
        for (name, folder) in &mailbox.lock().unwrap().folders {
            let special_use = infer_special_use(name);
            store
                .upsert_mailbox(&super::super::ImapMailbox {
                    name: name.clone(),
                    delimiter: Some("/".into()),
                    special_use,
                    uidvalidity: folder.uidvalidity as i64,
                    uidnext: folder.uidnext as i64,
                    highestmodseq: None,
                    permanent_flags_json: None,
                    permanent_keywords: None,
                })
                .unwrap();
        }

        let cache = BodyCache::new(store.clone());
        let provider = ImapProvider {
            store: store.clone(),
            cache,
            username: "me@example.com".into(),
            account_id: "me@example.com".into(),
            smtp_password: "test-password".into(),
            label_model: LabelModel::ImapLabelFolders,
            settings: test_settings(),
            source: Arc::new(FakeSource { mailbox }),
            now: || 1_700_000_000,
            limits: SyncLimits::inbox(),
            sent_round_budget: None,
            folder_window_override: None,
            extra_synced_candidates: None,
            force_user_folders: None,
            folder_rounds_per_poll_override: None,
            thread_state_loads: std::sync::atomic::AtomicUsize::new(0),
        };
        (provider, store)
    }

    /// Infer an RFC 6154 attribute from a well-known folder name for the test
    /// catalog (so the plan resolves roles by attribute, the primary path).
    fn infer_special_use(name: &str) -> Option<String> {
        match name {
            "Trash" => Some("\\Trash".into()),
            "Junk" | "Spam" => Some("\\Junk".into()),
            "Sent" => Some("\\Sent".into()),
            "Archive" => Some("\\Archive".into()),
            "Drafts" => Some("\\Drafts".into()),
            "All Mail" => Some("\\All".into()),
            _ => None,
        }
    }

    /// Minimal non-secret settings for the fake provider.
    fn test_settings() -> ImapAccountSettings {
        use super::super::settings::SecurityMode;
        ImapAccountSettings {
            imap_host: "127.0.0.1".into(),
            imap_port: 1143,
            imap_security: SecurityMode::StartTls,
            imap_username: "me@example.com".into(),
            smtp_host: "127.0.0.1".into(),
            smtp_port: 1025,
            smtp_security: SecurityMode::StartTls,
            smtp_username: "me@example.com".into(),
            mailbox_overrides: Default::default(),
            archive_mailbox: None,
            label_storage: LabelStorage::Folders,
            label_container: Some("Labels".into()),
            identities: vec![],
            pinned_fingerprints: Default::default(),
            server_saves_sent: false,
        }
    }

    fn message(message_id: &str, subject: &str, extra_headers: &str) -> String {
        format!(
            "Message-ID: {message_id}\r\nDate: Mon, 06 Oct 2025 09:00:00 +0000\r\nFrom: a@b.com\r\nSubject: {subject}\r\n{extra_headers}\r\n{subject} body",
        )
    }

    #[tokio::test]
    async fn initial_full_sync_ingests_inbox_threads_through_the_engine() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&[], &message("<m1@x>", "Hello", ""));
        mb.add(&["\\Seen"], &message("<m2@x>", "Second", ""));
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox);

        // Drive the real engine loop. `sync_with` needs a Database handle; the
        // provider carries its own store over the same in-memory db.
        let changed = crate::sync::sync_with(provider.store.database().as_ref(), "me@example.com", &provider)
            .await
            .unwrap();
        assert!(changed, "a full sync reports a change");

        // Two INBOX threads are now listed from the local store.
        let threads = provider.list_inbox(None).await.unwrap();
        assert_eq!(threads.thread_ids.len(), 2);

        // Each thread's messages are fetchable with the right labels.
        for thread_id in &threads.thread_ids {
            let messages = provider.fetch_thread(thread_id).await.unwrap();
            assert_eq!(messages.len(), 1);
            assert!(messages[0].label_ids.contains(&"INBOX".to_string()));
        }
        // The generation advanced and both messages are cached once.
        assert!(store.generation().unwrap() >= 1);
    }

    #[tokio::test]
    async fn a_second_poll_sees_new_mail_a_flag_change_and_a_deletion() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&[], &message("<m1@x>", "One", ""));
        let uid2 = mb.add(&[], &message("<m2@x>", "Two", ""));
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, _store) = provider_with(mailbox.clone());
        let db = provider.store.database().clone();

        // First sync establishes the cursor and ingests both.
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        let cursor_after_first = db.cursor("me@example.com").unwrap().unwrap();

        // Mutate the server: new mail, flag change on m1, expunge m2.
        {
            let mut mb = mailbox.lock().unwrap();
            mb.add(&[], &message("<m3@x>", "Three", ""));
            if let Some(first) = mb.inbox().messages.iter_mut().find(|m| m.uid == 1) {
                first.flags = vec!["\\Seen".into()];
            }
            mb.inbox().messages.retain(|m| m.uid != uid2);
        }

        // Second sync ingests the delta.
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        let cursor_after_second = db.cursor("me@example.com").unwrap().unwrap();
        assert_ne!(cursor_after_first, cursor_after_second, "the generation advanced");

        // m3 is now present; m2's thread lost its only location.
        let threads = provider.list_inbox(None).await.unwrap();
        // Two live INBOX threads remain (m1 and m3); m2 was expunged.
        assert_eq!(threads.thread_ids.len(), 2);
    }

    #[tokio::test]
    async fn an_ingest_failure_then_a_re_poll_with_the_same_cursor_loses_nothing() {
        // The at-least-once property: polling twice with the SAME cursor
        // returns the same changes, so an ingest that failed before the engine
        // persisted the cursor is retried intact.
        let mut mb = FakeMailbox::new(100);
        mb.add(&[], &message("<m1@x>", "One", ""));
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox);

        // Refresh once so m1 is journalled under generation 1.
        provider.baseline_cursor().await.unwrap();
        let before = store.generation().unwrap();
        assert!(before >= 1);

        // Poll from generation 0 (the pre-ingest position): m1's thread is
        // returned because it was journalled at a generation > 0.
        let g0 = SyncCursor::from_generation(0);
        let batch1 = provider.poll(&g0).await.unwrap();
        assert_eq!(batch1.changed_threads.len(), 1, "m1's thread is returned");
        assert!(!batch1.more);

        // Simulate the engine NOT persisting the new cursor (ingest failed),
        // then polling AGAIN with the SAME cursor g0. The same thread must
        // come back — nothing is lost.
        let batch2 = provider.poll(&g0).await.unwrap();
        assert_eq!(
            batch1.changed_threads, batch2.changed_threads,
            "the same cursor returns the same changes (at-least-once)"
        );
    }

    #[tokio::test]
    async fn poll_rejects_a_non_generation_or_future_cursor_as_invalid() {
        let mb = FakeMailbox::new(100);
        let (provider, _store) = provider_with(Arc::new(Mutex::new(mb)));
        // A non-generation cursor (e.g. a Gmail historyId) is InvalidCursor.
        let err = provider
            .poll(&SyncCursor::new("12345 pagetok"))
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::InvalidCursor));
        // A cursor newer than the store's generation (store reset) is invalid.
        let err = provider
            .poll(&SyncCursor::from_generation(999))
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::InvalidCursor));
    }

    #[tokio::test]
    async fn a_uidvalidity_reset_drops_locations_but_keeps_cached_bodies() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&[], &message("<m1@x>", "One", ""));
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox.clone());
        let db = provider.store.database().clone();

        provider.baseline_cursor().await.unwrap();
        let before_locations = store.locations_in_mailbox("INBOX").unwrap();
        assert_eq!(before_locations.len(), 1);
        let cached_before = store
            .locations_for_message(&before_locations[0].message_id)
            .unwrap();
        assert_eq!(cached_before.len(), 1);
        let message_id = before_locations[0].message_id.clone();
        assert!(provider.cache.contains(&message_id).unwrap());

        // Bump UIDVALIDITY and re-seed the SAME message at a new UID.
        {
            let mut mb = mailbox.lock().unwrap();
            mb.inbox().uidvalidity = 200;
            mb.inbox().uidnext = 1;
            mb.inbox().messages.clear();
            mb.add(&[], &message("<m1@x>", "One", ""));
        }
        provider.baseline_cursor().await.unwrap();

        // The body cache survived the reset (keyed by the stable id).
        assert!(
            provider.cache.contains(&message_id).unwrap(),
            "a UIDVALIDITY reset keeps cached bodies"
        );
        // And the mailbox still has exactly one live location.
        assert_eq!(store.locations_in_mailbox("INBOX").unwrap().len(), 1);
        let _ = db;
    }

    #[tokio::test]
    async fn body_peek_only_the_fetch_items_never_set_seen() {
        // Guiding rule 3, at the provider level: the only body request this
        // provider issues is BODY.PEEK[] (via SessionBodyFetcher/BODY_ITEMS),
        // and sync uses EXAMINE, never SELECT. Prove the item constant is peek.
        assert!(super::super::fetch::BODY_ITEMS.contains("BODY.PEEK["));
        assert!(!super::super::fetch::BODY_ITEMS
            .replace("BODY.PEEK[", "")
            .contains("BODY["));
    }

    #[tokio::test]
    async fn fetch_thread_resolves_an_alias_to_the_surviving_thread() {
        let mb = FakeMailbox::new(100);
        let (provider, store) = provider_with(Arc::new(Mutex::new(mb)));
        // Seed a thread with one message, and an alias old -> survivor.
        store
            .upsert_location(&ImapLocation {
                mailbox: "INBOX".into(),
                uidvalidity: 1,
                uid: 1,
                message_id: "imap:me@example.com:m1".into(),
                flags_json: "[\"\\\\Seen\"]".into(),
                modseq: None,
            })
            .unwrap();
        provider
            .cache
            .put(
                "imap:me@example.com:m1",
                message("<m1@x>", "Hi", "").as_bytes(),
                1,
            )
            .unwrap();
        let round = SyncRoundWrite {
            thread_assignments: vec![(
                "imap:me@example.com:m1".into(),
                "imap:t:survivor".into(),
            )],
            aliases: vec![("imap:t:old".into(), "imap:t:survivor".into())],
            changed_threads: vec!["imap:t:survivor".into()],
            ..Default::default()
        };
        store.commit_sync_round(&round).unwrap();

        // Fetching by the OLD (aliased) id resolves to the survivor's messages.
        let messages = provider.fetch_thread("imap:t:old").await.unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].thread_id, "imap:t:survivor");
    }

    #[tokio::test]
    async fn fetch_thread_of_an_empty_thread_is_not_found() {
        let mb = FakeMailbox::new(100);
        let (provider, _store) = provider_with(Arc::new(Mutex::new(mb)));
        let err = provider.fetch_thread("imap:t:gone").await.unwrap_err();
        assert!(matches!(err, ProviderError::NotFound));
    }

    #[tokio::test]
    async fn capabilities_report_the_neither_tier_and_the_accounts_label_model() {
        let (provider, _store) = provider_with(Arc::new(Mutex::new(FakeMailbox::new(1))));
        let caps = provider.capabilities();
        assert!(!caps.server_search);
        assert!(!caps.provided_threads);
        // Phase 4 run 3: IMAP accounts can now send, so send_supported is true.
        // verifiable_delivery stays false — there is no server-side sent copy
        // to confirm an uncertain send against.
        assert!(caps.send_supported);
        assert!(!caps.verifiable_delivery);
        // Run 4b: IMAP drives its own Sent-copy job after an accepted send.
        assert!(caps.client_saves_sent_copy);
        assert_eq!(caps.label_model, LabelModel::ImapLabelFolders);
    }

    #[tokio::test]
    async fn unsupported_mutations_are_rejected_as_a_permanent_invalid_operation() {
        // Phase 3 run 2 changed the contract: STARRED/UNREAD are now SUPPORTED
        // flag mutations and no longer rejected blanket. What still fails
        // permanently is every OTHER label id (a system location, a user
        // folder, an unknown id) and the label CRUD methods — so the mutation
        // queue rolls those back to `failed` rather than retrying forever
        // (InvalidOperation is neither retryable nor a reauth).
        let (provider, _store) = provider_with(Arc::new(Mutex::new(FakeMailbox::new(1))));

        // An unsupported location/label id is still a permanent rejection,
        // sent through the (supported-label) validation BEFORE any network.
        for bad in [
            vec!["INBOX".to_string()],
            vec!["TRASH".to_string()],
            vec!["SENT".to_string()],
            vec!["SPAM".to_string()],
            vec!["lf:Work".to_string()],
            vec!["folder:Clients/Acme".to_string()],
            vec!["kw:anything".to_string()],
        ] {
            let err = provider
                .modify_messages(&["imap:me@example.com:x".into()], &bad, &[])
                .await
                .unwrap_err();
            assert!(
                matches!(err, ProviderError::InvalidOperation(_)),
                "{bad:?} must be a permanent InvalidOperation, got {err:?}"
            );
            assert!(!err.retry_mutation());
            assert!(!err.requires_reauthentication());
        }

        // A SUPPORTED flag change on a thread that resolves to NO messages
        // (emptied / unknown) is a permanent InvalidOperation naming the
        // thread — never a silent success.
        let err = provider
            .modify_thread("imap:t:gone", &["STARRED".into()], &[])
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::InvalidOperation(_)), "{err:?}");
        assert!(!err.retry_mutation());

        // (Phase 4 run 3 CONTRACT CHANGE: `prepare_delivery` is now
        // IMPLEMENTED — it connects + authenticates — so it no longer returns
        // the old "not supported until phase 4" InvalidOperation. The stale
        // sub-assertion that used to live here has been removed; the real
        // prepare_delivery behaviour is covered by
        // `provider::imap::send::provider_level_tests`.)
        // Label RENAME (`update_label`) is refused PERMANENTLY in run 4 (the
        // `lf:` id is name-derived and there is no label-id migration), and the
        // refusal is decided before any session, so it holds on this fake too.
        let err = provider.update_label("lf:Old", "New").await.unwrap_err();
        assert!(matches!(err, ProviderError::InvalidOperation(_)), "{err:?}");
        assert!(!err.retry_mutation(), "rename refusal is permanent");
        // list_labels returns the system labels including the Slice 5b-1
        // Sent/Spam/Trash roles.
        let labels = provider.list_labels().await.unwrap();
        let ids: Vec<_> = labels.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(ids, ["INBOX", "SENT", "SPAM", "TRASH", "UNREAD", "STARRED"]);
    }

    #[test]
    fn label_model_is_chosen_from_the_accounts_label_storage() {
        assert_eq!(label_model_for(LabelStorage::Keywords), LabelModel::ImapKeywords);
        assert_eq!(label_model_for(LabelStorage::Folders), LabelModel::ImapLabelFolders);
        assert_eq!(label_model_for(LabelStorage::None), LabelModel::ImapNoUserLabels);
    }

    // ---- SLICE5A_FIXES item 1: at-least-once across a mid-round failure ----

    /// Snapshot of the durable sync state the round touches, so a test can
    /// assert a failed round left NOTHING behind.
    fn durable_snapshot(store: &ImapStateStore) -> (u64, Vec<ImapLocation>, Vec<String>) {
        let generation = store.generation().unwrap();
        let locations = store.locations_in_mailbox("INBOX").unwrap();
        let threads: Vec<String> = store
            .all_message_threads()
            .unwrap()
            .into_iter()
            .map(|(m, t, _)| format!("{m}->{t}"))
            .collect();
        (generation, locations, threads)
    }

    #[tokio::test]
    async fn a_failure_after_the_first_batch_of_new_mail_leaves_no_partial_state() {
        // Two new messages; the server drops the connection after the FIRST
        // body fetch. The round must fail having written NOTHING durable, and
        // a re-run against a healthy server must end exactly as a clean round.
        let mut mb = FakeMailbox::new(100);
        mb.add(&[], &message("<m1@x>", "One", ""));
        mb.add(&[], &message("<m2@x>", "Two", ""));
        let (provider, store) = provider_with(Arc::new(Mutex::new(mb.clone())));

        let before = durable_snapshot(&store);

        // Inject the mid-round drop.
        let mut failing = mb.clone();
        failing.inbox().fail_after_body_fetches = Some(1);
        let mut session = open_scripted_session(failing).await;
        let result = provider.refresh_inbox_with(session.as_mut()).await;
        assert!(result.is_err(), "a dropped connection fails the round");

        // Durable state is UNCHANGED — no location, thread, journal or
        // generation write survived the partial round.
        assert_eq!(durable_snapshot(&store), before, "a failed round writes nothing");

        // Re-run cleanly: both messages are threaded, journaled, ingestible.
        let mut healthy = open_scripted_session(mb).await;
        provider.refresh_inbox_with(healthy.as_mut()).await.unwrap();
        assert_eq!(store.locations_in_mailbox("INBOX").unwrap().len(), 2);
        let threads = provider.list_inbox(None).await.unwrap();
        assert_eq!(threads.thread_ids.len(), 2);
        for thread_id in &threads.thread_ids {
            assert_eq!(provider.fetch_thread(thread_id).await.unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn a_failure_after_a_flag_update_leaves_no_partial_state() {
        // One message synced; then its flag changes AND the connection drops
        // on the (new) body fetch of a second new message in the same round.
        let mut mb = FakeMailbox::new(100);
        mb.add(&[], &message("<m1@x>", "One", ""));
        let mailbox = Arc::new(Mutex::new(mb.clone()));
        let (provider, store) = provider_with(mailbox.clone());

        // Clean first round establishes m1.
        let mut s0 = open_scripted_session(mb.clone()).await;
        provider.refresh_inbox_with(s0.as_mut()).await.unwrap();
        let before = durable_snapshot(&store);

        // Next round: m1 is marked \Seen AND a new m2 arrives; the server
        // drops the connection on m2's body fetch, after the flag change was
        // computed but before the atomic commit.
        let mut next = mb.clone();
        next.inbox().messages[0].flags = vec!["\\Seen".into()];
        next.add(&[], &message("<m2@x>", "Two", ""));
        let mut failing = next.clone();
        failing.inbox().fail_after_body_fetches = Some(0); // drop on the first body fetch
        let mut session = open_scripted_session(failing).await;
        assert!(provider.refresh_inbox_with(session.as_mut()).await.is_err());

        // The flag change was NOT applied (round rolled back whole).
        assert_eq!(durable_snapshot(&store), before, "flag change did not leak");

        // A healthy re-run applies both the flag change and the new message.
        let mut healthy = open_scripted_session(next).await;
        provider.refresh_inbox_with(healthy.as_mut()).await.unwrap();
        let m1 = store
            .locations_in_mailbox("INBOX")
            .unwrap()
            .into_iter()
            .find(|l| l.uid == 1)
            .unwrap();
        let flags: Vec<String> = serde_json::from_str(&m1.flags_json).unwrap();
        assert!(flags.iter().any(|f| f == "\\Seen"), "flag change applied on re-run");
        assert_eq!(store.locations_in_mailbox("INBOX").unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_failure_after_a_deletion_leaves_no_partial_state() {
        // Two messages synced; then one is expunged AND a new message arrives,
        // and the connection drops on the new body fetch.
        let mut mb = FakeMailbox::new(100);
        mb.add(&[], &message("<m1@x>", "One", ""));
        mb.add(&[], &message("<m2@x>", "Two", ""));
        let (provider, store) = provider_with(Arc::new(Mutex::new(mb.clone())));

        let mut s0 = open_scripted_session(mb.clone()).await;
        provider.refresh_inbox_with(s0.as_mut()).await.unwrap();
        let before = durable_snapshot(&store);
        assert_eq!(before.1.len(), 2);

        // m2 expunged, m3 new; drop on m3's body fetch.
        let mut next = mb.clone();
        next.inbox().messages.retain(|m| m.uid != 2);
        next.add(&[], &message("<m3@x>", "Three", ""));
        let mut failing = next.clone();
        failing.inbox().fail_after_body_fetches = Some(0);
        let mut session = open_scripted_session(failing).await;
        assert!(provider.refresh_inbox_with(session.as_mut()).await.is_err());

        // The deletion did NOT leak: m2 is still present locally.
        assert_eq!(durable_snapshot(&store), before, "deletion did not leak");

        // Healthy re-run: m2 gone, m3 present.
        let mut healthy = open_scripted_session(next).await;
        provider.refresh_inbox_with(healthy.as_mut()).await.unwrap();
        let uids: Vec<i64> = store
            .locations_in_mailbox("INBOX")
            .unwrap()
            .into_iter()
            .map(|l| l.uid)
            .collect();
        assert!(!uids.contains(&2), "m2 expunged on re-run");
        assert_eq!(store.locations_in_mailbox("INBOX").unwrap().len(), 2);
    }

    // ---- SLICE5A_FIXES item 2: one bad message does not wedge the round ----

    #[tokio::test]
    async fn an_oversize_message_in_the_middle_of_a_batch_does_not_fail_the_round() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&[], &message("<ok1@x>", "First", ""));
        // Middle message advertises a size over the cache limit, so its body
        // is skipped during sync (we never allocate 64 MiB in a unit test).
        let huge_uid = mb.add_sized(
            &[],
            &message("<huge@x>", "Huge", ""),
            Some(super::super::policy::MAX_RAW_MESSAGE_BYTES + 1),
        );
        mb.add(&[], &message("<ok2@x>", "Third", ""));
        let (provider, store) = provider_with(Arc::new(Mutex::new(mb.clone())));

        let mut session = open_scripted_session(mb).await;
        // The round SUCCEEDS despite the oversize message in the middle: one
        // bad message must not wedge or abort the round.
        provider.refresh_inbox_with(session.as_mut()).await.unwrap();

        // All three are recorded as locations (the oversize one too) and
        // threaded — nothing was lost.
        assert_eq!(store.locations_in_mailbox("INBOX").unwrap().len(), 3);
        let threads = provider.list_inbox(None).await.unwrap();
        assert_eq!(threads.thread_ids.len(), 3, "every message threaded");

        // The oversize message's body was SKIPPED (never cached); the two
        // normal messages' bodies are cached.
        let huge_id = store
            .locations_in_mailbox("INBOX")
            .unwrap()
            .into_iter()
            .find(|l| l.uid == huge_uid as i64)
            .unwrap()
            .message_id;
        assert!(
            !provider.cache.contains(&huge_id).unwrap(),
            "the oversize message's body is skipped, not cached"
        );
        // The two normal messages are fully fetchable.
        let mut fetched = 0;
        for thread_id in &threads.thread_ids {
            if let Ok(messages) = provider.fetch_thread(thread_id).await {
                fetched += messages.len();
            }
        }
        assert!(fetched >= 2, "the two normal messages fetch");
    }

    // ---- SLICE5A_FIXES item 4: thread age is persisted, not hash order ----

    #[tokio::test]
    async fn the_genuinely_older_thread_survives_a_merge_regardless_of_id_hash_order() {
        // Two persisted threads whose id-hash order is the OPPOSITE of their
        // creation order. A late linking message must keep the genuinely OLDER
        // thread's id (lower creation generation), alias the other to it, and
        // report both changed.
        let mb = FakeMailbox::new(100);
        let (provider, store) = provider_with(Arc::new(Mutex::new(mb)));

        // Find two message tokens whose thread ids sort in a known hash order,
        // then seed them with creation generations OPPOSITE to that order.
        use super::super::threading::new_thread_id_for_test;
        let t_a = new_thread_id_for_test("a@x");
        let t_b = new_thread_id_for_test("b@x");
        // Pick the one with the LARGER id hash to be the OLDER thread, so hash
        // order and age disagree.
        let (older_token, older_tid, newer_token, newer_tid) = if t_a > t_b {
            ("a@x", t_a, "b@x", t_b)
        } else {
            ("b@x", t_b, "a@x", t_a)
        };

        // Older thread created at generation 1, newer at generation 2.
        store
            .commit_sync_round(&SyncRoundWrite {
                thread_assignments: vec![(
                    "imap:me@example.com:older".into(),
                    older_tid.clone(),
                )],
                changed_threads: vec![older_tid.clone()],
                ..Default::default()
            })
            .unwrap();
        provider
            .cache
            .put(
                "imap:me@example.com:older",
                message(&format!("<{older_token}>"), "Older", "").as_bytes(),
                1,
            )
            .unwrap();
        store
            .commit_sync_round(&SyncRoundWrite {
                thread_assignments: vec![(
                    "imap:me@example.com:newer".into(),
                    newer_tid.clone(),
                )],
                changed_threads: vec![newer_tid.clone()],
                ..Default::default()
            })
            .unwrap();
        provider
            .cache
            .put(
                "imap:me@example.com:newer",
                message(&format!("<{newer_token}>"), "Newer", "").as_bytes(),
                1,
            )
            .unwrap();

        // A late message referencing both links the two threads.
        let mut state = provider.load_thread_state().unwrap();
        let linker = super::super::threading::ThreadingInput {
            message_id: "imap:me@example.com:link".into(),
            message_id_header: Some("<link@x>".into()),
            in_reply_to: None,
            references: Some(format!("<{older_token}> <{newer_token}>")),
        };
        let outcome = super::super::threading::thread_batch(&mut state, &[linker]);

        // The genuinely OLDER thread id survives, the newer is aliased to it,
        // and both are reported changed.
        assert_eq!(
            outcome.assignments[0].1, older_tid,
            "the older thread's id survives the merge"
        );
        assert_eq!(outcome.aliases, vec![(newer_tid.clone(), older_tid.clone())]);
        assert!(outcome.changed_threads.contains(&older_tid));
        assert!(outcome.changed_threads.contains(&newer_tid));
    }

    // ---- SLICE5A_FIXES item 5: idle polls do not move the generation -------

    #[tokio::test]
    async fn repeated_idle_polls_do_not_bump_the_generation_or_journal() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&[], &message("<m1@x>", "One", ""));
        let (provider, store) = provider_with(Arc::new(Mutex::new(mb)));

        // Establish a baseline (ingests m1, generation moves to 1).
        let baseline = provider.baseline_cursor().await.unwrap();
        let gen_after_baseline = store.generation().unwrap();
        assert!(gen_after_baseline >= 1);
        let journal_after_baseline = store.journal_since(0).unwrap();

        // Several idle polls (nothing changed on the server).
        let loads_after_baseline = provider
            .thread_state_loads
            .load(std::sync::atomic::Ordering::Relaxed);
        assert_eq!(loads_after_baseline, 1, "the baseline threaded one new message");
        for _ in 0..3 {
            let batch = provider.poll(&baseline).await.unwrap();
            assert!(
                batch.changed_threads.is_empty(),
                "an idle poll from the current cursor reports no changes"
            );
            // The generation did NOT move, and the journal is unchanged.
            assert_eq!(store.generation().unwrap(), gen_after_baseline, "idle: no bump");
            assert_eq!(store.journal_since(0).unwrap(), journal_after_baseline);
            // The returned cursor is the (unchanged) current generation.
            assert_eq!(batch.cursor.generation(), Some(gen_after_baseline));
            // An idle round must not rebuild the threader state (which reads
            // every cached body).
            assert_eq!(
                provider
                    .thread_state_loads
                    .load(std::sync::atomic::Ordering::Relaxed),
                loads_after_baseline,
                "idle poll must not reload thread state"
            );
        }
    }

    #[tokio::test]
    async fn merge_moves_all_existing_messages() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&[], &message("<a@x>", "A", ""));
        mb.add(&[], &message("<b@x>", "B", ""));
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox.clone());
        provider.baseline_cursor().await.unwrap();
        mailbox.lock().unwrap().add(
            &[],
            &message("<c@x>", "Link", "References: <a@x> <b@x>\r\n"),
        );
        let cursor = SyncCursor::from_generation(store.generation().unwrap());
        provider.poll(&cursor).await.unwrap();
        let rows = store.all_message_threads().unwrap();
        let survivor = store.resolve_thread_alias(&rows[0].1).unwrap();
        let fetched = provider.fetch_thread(&survivor).await.unwrap();
        assert_eq!(
            fetched.len(),
            3,
            "merging two threads must retain both roots and the linker"
        );
    }

    #[tokio::test]
    async fn merged_threads_ingest_atomically_and_replay_through_the_engine() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&[], &message("<a@x>", "A", ""));
        mb.add(&[], &message("<b@x>", "B", ""));
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox.clone());
        let db = store.database();
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
            .await
            .unwrap();
        let previous_cursor = db.cursor("me@example.com").unwrap().unwrap();
        assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 2);
        mailbox.lock().unwrap().add(
            &[],
            &message("<c@x>", "Link", "References: <a@x> <b@x>\r\n"),
        );
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
            .await
            .unwrap();
        let threads = db.list_all_mail(Some("me@example.com")).unwrap();
        assert_eq!(threads.len(), 1);
        assert_eq!(db.get_thread(&threads[0].id).unwrap().messages.len(), 3);
        // Replay the journal after a simulated crash before cursor storage.
        db.with_connection(|connection| {
            connection.execute(
                "UPDATE sync_state SET cursor = ?1 WHERE account_id = ?2",
                rusqlite::params![previous_cursor, "me@example.com"],
            )?;
            Ok(())
        })
        .unwrap();
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
            .await
            .unwrap();
        assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 1);
        assert_eq!(db.get_thread(&threads[0].id).unwrap().messages.len(), 3);
        // A later reply to either original root reaches the same survivor.
        mailbox
            .lock()
            .unwrap()
            .add(&[], &message("<d@x>", "Reply", "In-Reply-To: <b@x>\r\n"));
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
            .await
            .unwrap();
        assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 1);
        assert_eq!(db.get_thread(&threads[0].id).unwrap().messages.len(), 4);
    }

    #[tokio::test]
    async fn out_of_order_references_survive_between_rounds() {
        let mut mb = FakeMailbox::new(100);
        mb.add(
            &[],
            &message("<reply@x>", "Reply", "References: <missing@x>\r\n"),
        );
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox.clone());
        let cursor = provider.baseline_cursor().await.unwrap();
        mailbox.lock().unwrap().add(
            &[],
            &message("<sibling@x>", "Sibling", "References: <missing@x>\r\n"),
        );
        provider.poll(&cursor).await.unwrap();
        assert_eq!(
            store.thread_ids_in_mailbox("INBOX").unwrap().len(),
            1,
            "replies to the same absent parent are one thread across polls"
        );
        mailbox.lock().unwrap().add(
            &[],
            &message("<third@x>", "Third", "In-Reply-To: <missing@x>\r\n"),
        );
        provider.poll(&cursor).await.unwrap();
        mailbox
            .lock()
            .unwrap()
            .add(&[], &message("<missing@x>", "Late parent", ""));
        provider.poll(&cursor).await.unwrap();
        let threads = store.thread_ids_in_mailbox("INBOX").unwrap();
        assert_eq!(threads.len(), 1);
        assert_eq!(provider.fetch_thread(&threads[0]).await.unwrap().len(), 4);
    }

    #[tokio::test]
    async fn window_bounds_the_mailbox_across_polls() {
        let mut mb = FakeMailbox::new(100);
        for n in 1..=3 {
            mb.add(&[], &message(&format!("<m{n}@x>"), "Subject", ""));
        }
        let mailbox = Arc::new(Mutex::new(mb));
        let (mut provider, store) = provider_with(mailbox.clone());
        provider.limits.mailbox_window = 2;
        let cursor = provider.baseline_cursor().await.unwrap();
        assert_eq!(store.locations_in_mailbox("INBOX").unwrap().len(), 2);
        provider.poll(&cursor).await.unwrap();
        assert_eq!(
            store.locations_in_mailbox("INBOX").unwrap().len(),
            2,
            "an unchanged mailbox must stay within its window"
        );
        mailbox
            .lock()
            .unwrap()
            .add(&[], &message("<m4@x>", "Fourth", ""));
        provider.poll(&cursor).await.unwrap();
        let uids: Vec<_> = store
            .locations_in_mailbox("INBOX")
            .unwrap()
            .iter()
            .map(|l| l.uid)
            .collect();
        assert_eq!(uids, vec![3, 4]);
    }

    #[tokio::test]
    async fn cache_miss_checks_uidvalidity_before_fetching() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&[], &message("<original@x>", "Original", ""));
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox.clone());
        provider.baseline_cursor().await.unwrap();
        let id = store.locations_in_mailbox("INBOX").unwrap()[0]
            .message_id
            .clone();
        store
            .database()
            .with_connection(|c| {
                c.execute("DELETE FROM imap_bodies", [])?;
                Ok(())
            })
            .unwrap();
        let mut reset = FakeMailbox::new(200);
        reset.add(&[], &message("<different@x>", "Different", ""));
        *mailbox.lock().unwrap() = reset;
        let result = provider.fetch_message(&id).await;
        assert!(
            result.is_err(),
            "a stale UID must not load another message as the original: {result:?}"
        );
        assert!(!provider.cache.contains(&id).unwrap());
        provider.baseline_cursor().await.unwrap();
        let new_id = &store.locations_in_mailbox("INBOX").unwrap()[0].message_id;
        assert_ne!(new_id, &id);
        let raw = provider.fetch_message(new_id).await.unwrap();
        assert_eq!(crate::mime::normalize(&raw).unwrap().body_text, "Different body");
    }

    #[tokio::test]
    async fn understated_oversize_does_not_abort_the_round() {
        for advertised_size in [Some(10), None] {
            let mut mb = FakeMailbox::new(100);
            mb.add(&[], &message("<ok@x>", "Good", ""));
            let mut huge = format!(
                "Message-ID: <huge@x>\r\n\r\n{}",
                "x".repeat(policy::MAX_RAW_MESSAGE_BYTES)
            );
            huge.truncate(policy::MAX_RAW_FETCH_BYTES);
            mb.add_sized(&[], &huge, advertised_size);
            let (provider, store) = provider_with(Arc::new(Mutex::new(mb)));
            let result = provider.baseline_cursor().await;
            assert!(
                result.is_ok(),
                "oversize should be skipped, not abort sync: {result:?}"
            );
            let locations = store.locations_in_mailbox("INBOX").unwrap();
            assert_eq!(locations.len(), 2);
            assert!(provider.cache.contains(&locations[0].message_id).unwrap());
            assert!(!provider.cache.contains(&locations[1].message_id).unwrap());
            let generation = store.generation().unwrap();
            provider
                .poll(&SyncCursor::from_generation(generation))
                .await
                .unwrap();
            assert_eq!(store.generation().unwrap(), generation);
        }
    }

    #[tokio::test]
    async fn removing_account_purges_live_provider_state() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&[], &message("<private@x>", "Private mail", ""));
        let (provider, store) = provider_with(Arc::new(Mutex::new(mb)));
        provider.baseline_cursor().await.unwrap();
        store.database().remove_account("me@example.com").unwrap();
        assert!(
            store.all_message_threads().unwrap().is_empty(),
            "removal must clear provider thread mappings"
        );
        assert!(
            store.locations_in_mailbox("INBOX").unwrap().is_empty(),
            "removal must clear provider locations"
        );
        assert_eq!(
            provider.cache.total_size().unwrap(),
            0,
            "removal must clear raw cached mail"
        );
    }

    #[tokio::test]
    async fn removing_account_deletes_its_sent_copy_detection_row() {
        // The per-device detection result is LOCAL provider state; removing the
        // account must purge it like every other imap_* table.
        let (_provider, store) = provider_with(Arc::new(Mutex::new(FakeMailbox::new(1))));
        store.set_sent_copy_detection(true, 1_000).unwrap();
        assert_eq!(store.sent_copy_detection().unwrap(), Some(true));
        store.database().remove_account("me@example.com").unwrap();
        assert!(
            store.sent_copy_detection().unwrap().is_none(),
            "account removal must delete the Sent-copy detection row"
        );
    }

    // ---- SLICE5B1 item 3/5: the token loader threads like the body oracle --
    //
    // Each corpus below syncs a sequence of messages through the provider
    // (which writes tokens at assignment and seeds from them), then threads one
    // more "probe" message TWO ways against the resulting store: once with the
    // production token loader and once with the retained body-parsing oracle.
    // The two ThreadState seeds must drive thread_batch to the SAME assignment,
    // aliases and changed-thread set — proving the token loader is equivalent
    // to the body loader it replaced, without ever reading imap_bodies.

    /// Thread `probe` against the store with BOTH loaders and assert the
    /// outcomes are identical. Returns the (identical) outcome.
    async fn assert_loaders_agree(
        provider: &ImapProvider,
        probe: ThreadingInput,
    ) -> threading::ThreadingOutcome {
        let batch_tokens = probe.token_set();
        let mut token_state = provider
            .load_thread_state_from_tokens(&batch_tokens)
            .unwrap();
        let mut oracle_state = provider.load_thread_state().unwrap();
        let token_outcome = threading::thread_batch(&mut token_state, &[probe.clone()]);
        let oracle_outcome = threading::thread_batch(&mut oracle_state, &[probe]);
        assert_eq!(
            token_outcome, oracle_outcome,
            "token loader and body oracle must thread identically"
        );
        token_outcome
    }

    /// Sync a sequence of messages (each added, then one poll) so each lands in
    /// its own round, exercising cross-round persistence.
    async fn sync_each(provider: &ImapProvider, mailbox: &Arc<Mutex<FakeMailbox>>, msgs: &[String]) {
        provider.baseline_cursor().await.unwrap();
        for raw in msgs {
            mailbox.lock().unwrap().add(&[], raw);
            let cursor = SyncCursor::from_generation(provider.store.generation().unwrap());
            provider.poll(&cursor).await.unwrap();
        }
    }

    fn probe(id: &str, msgid: &str, in_reply_to: Option<&str>, references: Option<&str>) -> ThreadingInput {
        ThreadingInput {
            message_id: id.to_string(),
            message_id_header: Some(msgid.to_string()),
            in_reply_to: in_reply_to.map(str::to_string),
            references: references.map(str::to_string),
        }
    }

    #[tokio::test]
    async fn loader_equivalence_late_parent() {
        // A reply to an absent parent, then a sibling: a late parent probe must
        // join the same thread under both loaders.
        let mb = FakeMailbox::new(100);
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, _store) = provider_with(mailbox.clone());
        sync_each(
            &provider,
            &mailbox,
            &[
                message("<reply@x>", "Reply", "References: <missing@x>\r\n"),
                message("<sibling@x>", "Sibling", "References: <missing@x>\r\n"),
            ],
        )
        .await;
        assert_loaders_agree(&provider, probe("imap:p:late", "<missing@x>", None, None)).await;
    }

    #[tokio::test]
    async fn loader_equivalence_absent_parent_siblings_across_rounds() {
        let mb = FakeMailbox::new(100);
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, _store) = provider_with(mailbox.clone());
        sync_each(
            &provider,
            &mailbox,
            &[
                message("<s1@x>", "S1", "In-Reply-To: <ghost@x>\r\n"),
                message("<s2@x>", "S2", "In-Reply-To: <ghost@x>\r\n"),
                message("<s3@x>", "S3", "References: <ghost@x>\r\n"),
            ],
        )
        .await;
        let outcome =
            assert_loaders_agree(&provider, probe("imap:p:s4", "<s4@x>", Some("<ghost@x>"), None))
                .await;
        // All siblings share one thread, so the probe joins exactly it.
        assert!(outcome.aliases.is_empty(), "no merge: all share one thread");
    }

    #[tokio::test]
    async fn loader_equivalence_merge_of_two_persisted_threads() {
        // Two unrelated roots become two persisted threads; a probe that
        // references both must merge them identically under both loaders.
        let mb = FakeMailbox::new(100);
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, _store) = provider_with(mailbox.clone());
        sync_each(
            &provider,
            &mailbox,
            &[message("<a@x>", "A", ""), message("<b@x>", "B", "")],
        )
        .await;
        let outcome = assert_loaders_agree(
            &provider,
            probe("imap:p:link", "<link@x>", None, Some("<a@x> <b@x>")),
        )
        .await;
        assert_eq!(outcome.aliases.len(), 1, "the probe merges the two threads");
    }

    #[tokio::test]
    async fn loader_equivalence_capped_ancestry() {
        // A seeded message whose References exceed the cap: the token set
        // persisted is the capped set, and the loader must still thread a reply
        // that points at the nearest (kept) ancestor identically.
        let near = "<near@x>";
        let huge: String = (0..policy::MAX_REFERENCES + 20)
            .map(|i| {
                if i == policy::MAX_REFERENCES + 19 {
                    near.to_string()
                } else {
                    format!("<old{i}@x>")
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        let mb = FakeMailbox::new(100);
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, _store) = provider_with(mailbox.clone());
        sync_each(
            &provider,
            &mailbox,
            &[
                message(near, "Near", ""),
                message("<capped@x>", "Capped", &format!("References: {huge}\r\n")),
            ],
        )
        .await;
        assert_loaders_agree(
            &provider,
            probe("imap:p:reply", "<reply2@x>", Some(near), None),
        )
        .await;
    }

    #[tokio::test]
    async fn loader_equivalence_duplicate_message_id() {
        let mb = FakeMailbox::new(100);
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, _store) = provider_with(mailbox.clone());
        // Same Message-ID header twice (distinct stable ids via distinct UIDs).
        sync_each(
            &provider,
            &mailbox,
            &[message("<dup@x>", "First", ""), message("<dup@x>", "Second", "")],
        )
        .await;
        assert_loaders_agree(&provider, probe("imap:p:d", "<dup@x>", None, None)).await;
    }

    #[tokio::test]
    async fn loader_threads_a_reply_to_a_message_with_no_cached_body() {
        // An oversize message's body is skipped (never cached). At sync time it
        // still threads on its IDENTITY-PASS Message-ID header, and 5b-1
        // PERSISTS that token at assignment — so a later reply via that header
        // joins its thread using tokens alone, with no body ever read.
        //
        // This is the one corpus case where the retained body-parsing oracle
        // CANNOT match: reloading from bodies loses the header of a message
        // whose body was never cached (the oracle sees only the stable id).
        // That lossy reload is exactly the weakness 5b-1 removes, so here we
        // assert the token loader's correct, non-lossy outcome directly rather
        // than against the oracle.
        let mut mb = FakeMailbox::new(100);
        mb.add(&[], &message("<normal@x>", "Normal", ""));
        mb.add_sized(
            &[],
            &message("<nobody@x>", "NoBody", ""),
            Some(policy::MAX_RAW_MESSAGE_BYTES + 1),
        );
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox.clone());
        provider.baseline_cursor().await.unwrap();

        // The oversize message really has no cached body.
        let nobody_id = store
            .locations_in_mailbox("INBOX")
            .unwrap()
            .into_iter()
            .find(|l| !provider.cache.contains(&l.message_id).unwrap())
            .map(|l| l.message_id)
            .expect("the oversize message's body was skipped");

        // Its persisted token set includes the identity-pass header AND its
        // stable id — written at assignment, with no body read. The header is
        // stored in the threader's NORMALIZED form (brackets/case stripped).
        let tokens = store
            .seed_state_for_tokens(&["nobody@x".to_string()])
            .unwrap();
        assert_eq!(tokens.len(), 1, "exactly the no-body message's thread is seeded");
        let (_, (_, token_set)) = tokens.into_iter().next().unwrap();
        assert!(token_set.contains(&"nobody@x".to_string()));
        assert!(token_set.contains(&nobody_id));

        // A reply via that header joins the no-body message's thread.
        let mut state = provider
            .load_thread_state_from_tokens(&["nobody@x".to_string()])
            .unwrap();
        let outcome = threading::thread_batch(
            &mut state,
            &[probe("imap:p:r", "<reply3@x>", Some("<nobody@x>"), None)],
        );
        let nobody_thread = store
            .thread_of_message(&nobody_id)
            .unwrap()
            .map(|t| store.resolve_thread_alias(&t).unwrap())
            .unwrap();
        assert_eq!(
            outcome.assignments[0].1, nobody_thread,
            "the reply joins the no-body message's thread from tokens alone"
        );
    }

    // ---- SLICE5B1 item 3: the hot path never reads imap_bodies -------------

    #[tokio::test]
    async fn the_round_loader_does_not_read_cached_bodies() {
        // After an initial sync (which backfills + writes tokens), delete the
        // whole body cache, then sync a NEW linking message. Threading must
        // still merge correctly using ONLY persisted tokens — proving the per
        // round loader reads no bodies. (The backfill already ran at the first
        // threading round, so a later round never parses a body.)
        let mut mb = FakeMailbox::new(100);
        mb.add(&[], &message("<a@x>", "A", ""));
        mb.add(&[], &message("<b@x>", "B", ""));
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox.clone());
        provider.baseline_cursor().await.unwrap();
        // Drop every cached body: a body read now would fail to find anything.
        store
            .database()
            .with_connection(|c| {
                c.execute("DELETE FROM imap_bodies", [])?;
                Ok(())
            })
            .unwrap();
        // A late linker referencing both roots arrives and is synced.
        mailbox
            .lock()
            .unwrap()
            .add(&[], &message("<c@x>", "Link", "References: <a@x> <b@x>\r\n"));
        let cursor = SyncCursor::from_generation(store.generation().unwrap());
        provider.poll(&cursor).await.unwrap();
        // The two roots merged into one thread using tokens alone.
        let rows = store.all_message_threads().unwrap();
        let survivor = store.resolve_thread_alias(&rows[0].1).unwrap();
        let members = store.messages_in_thread(&survivor).unwrap();
        assert_eq!(members.len(), 3, "the merge happened from tokens, no body read");
    }

    // ---- SLICE5B1 item 4: the upgrade backfill ------------------------------

    #[tokio::test]
    async fn upgrade_backfill_fills_tokens_then_a_late_linker_threads_like_before() {
        // Simulate a pre-5b1 (schema-58-era) database: threads + cached bodies
        // exist but NO token rows, and the account is marked backfill-needed.
        // Running a threading round must backfill tokens from the bodies, then
        // thread a late linking message to the SAME result as the oracle.
        let mb = FakeMailbox::new(100);
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox.clone());

        // Insert two threaded messages WITH cached bodies but no tokens, via
        // the store (as a 0.94.x database would hold them).
        for (id, msgid, tid) in [
            ("imap:me@example.com:a", "<a@x>", "imap:t:a"),
            ("imap:me@example.com:b", "<b@x>", "imap:t:b"),
        ] {
            store
                .commit_sync_round(&SyncRoundWrite {
                    thread_assignments: vec![(id.into(), tid.into())],
                    changed_threads: vec![tid.into()],
                    // Deliberately NO message_tokens — the pre-5b1 shape.
                    ..Default::default()
                })
                .unwrap();
            provider
                .cache
                .put(id, message(msgid, "S", "").as_bytes(), 1)
                .unwrap();
        }
        // Clear the token rows commit_sync_round would never have written here
        // anyway, and force "backfill needed".
        store
            .database()
            .with_connection(|c| {
                c.execute("DELETE FROM imap_message_tokens", [])?;
                c.execute(
                    "UPDATE imap_sync_state SET tokens_backfilled = 0 WHERE account_id = ?1",
                    ["me@example.com"],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(!store.tokens_backfilled().unwrap(), "set up as needing backfill");
        assert_eq!(store.messages_missing_tokens(100).unwrap().len(), 2);

        // A late message linking <a@x> and <b@x>. Threading it runs the
        // backfill first, then merges from the now-present tokens.
        let linker = probe("imap:me@example.com:link", "<link@x>", None, Some("<a@x> <b@x>"));
        provider.ensure_tokens_backfilled().unwrap();
        assert!(store.tokens_backfilled().unwrap(), "backfill marked done");
        assert!(store.messages_missing_tokens(100).unwrap().is_empty());

        // Now the token loader and body oracle agree, and the probe merges.
        let outcome = assert_loaders_agree(&provider, linker).await;
        assert_eq!(outcome.aliases.len(), 1, "the late linker merged both threads");
    }

    #[tokio::test]
    async fn upgrade_backfill_is_idempotent_and_crash_resumable() {
        let mb = FakeMailbox::new(100);
        let (provider, store) = provider_with(Arc::new(Mutex::new(mb)));
        // One threaded message with a body, no tokens, backfill needed.
        store
            .commit_sync_round(&SyncRoundWrite {
                thread_assignments: vec![("imap:me@example.com:m".into(), "imap:t:m".into())],
                changed_threads: vec!["imap:t:m".into()],
                ..Default::default()
            })
            .unwrap();
        provider
            .cache
            .put("imap:me@example.com:m", message("<m@x>", "M", "").as_bytes(), 1)
            .unwrap();
        store
            .database()
            .with_connection(|c| {
                c.execute("DELETE FROM imap_message_tokens", [])?;
                c.execute("UPDATE imap_sync_state SET tokens_backfilled = 0", [])?;
                Ok(())
            })
            .unwrap();

        // Simulate a crash AFTER one message's tokens were written but BEFORE
        // the done-flag was set: write the row directly, leave the flag off.
        store
            .write_message_tokens("imap:me@example.com:m", &["<m@x>".into()])
            .unwrap();
        assert!(!store.tokens_backfilled().unwrap());
        // The queue is now empty (that message has tokens), so resume finishes
        // and marks done without re-doing work.
        assert!(store.messages_missing_tokens(100).unwrap().is_empty());
        provider.ensure_tokens_backfilled().unwrap();
        assert!(store.tokens_backfilled().unwrap());
        // Running again is a no-op.
        provider.ensure_tokens_backfilled().unwrap();
        assert!(store.tokens_backfilled().unwrap());
    }

    #[tokio::test]
    async fn a_message_with_no_cached_body_backfills_only_its_stable_id() {
        let mb = FakeMailbox::new(100);
        let (provider, store) = provider_with(Arc::new(Mutex::new(mb)));
        store
            .commit_sync_round(&SyncRoundWrite {
                thread_assignments: vec![("imap:me@example.com:nb".into(), "imap:t:nb".into())],
                changed_threads: vec!["imap:t:nb".into()],
                ..Default::default()
            })
            .unwrap();
        // No cache.put — the message has no cached body.
        store
            .database()
            .with_connection(|c| {
                c.execute("DELETE FROM imap_message_tokens", [])?;
                c.execute("UPDATE imap_sync_state SET tokens_backfilled = 0", [])?;
                Ok(())
            })
            .unwrap();
        provider.ensure_tokens_backfilled().unwrap();
        let tokens: Vec<String> = store
            .database()
            .with_connection(|c| {
                let mut st = c.prepare(
                    "SELECT token FROM imap_message_tokens WHERE message_id = 'imap:me@example.com:nb'",
                )?;
                let rows = st
                    .query_map([], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .unwrap();
        assert_eq!(tokens, vec!["imap:me@example.com:nb".to_string()],
            "no body => only the stable-id anchor token");
    }

    // ---- SLICE5A_FIXES item 6: gated live test against the Dovecot harness --
    //
    // ------------------------------------------------------------------
    // Slice 5b-1: multi-mailbox machinery + Trash/Junk
    // ------------------------------------------------------------------

    /// Labels the engine ended up with for a thread, via fetch_thread's union.
    async fn thread_labels(provider: &ImapProvider, thread_id: &str) -> Vec<String> {
        let messages = provider.fetch_thread(thread_id).await.unwrap();
        messages.into_iter().flat_map(|m| m.label_ids).collect()
    }

    /// (a) A hot INBOX message moved to Trash by another client -> TRASH, no
    /// INBOX, and its thread is re-journaled so the engine re-ingests it.
    #[tokio::test]
    async fn a_hot_inbox_message_moved_to_trash_becomes_trash_and_is_rejournaled() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<m1@x>", "Hello", ""));
        mb.folder("Trash", 50); // present but empty for now
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox.clone());
        let db = store.database();

        // First sync: m1 is in INBOX, hot, ingested.
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        let threads = provider.list_inbox(None).await.unwrap();
        assert_eq!(threads.thread_ids.len(), 1);
        let thread_id = threads.thread_ids[0].clone();
        assert!(store.is_thread_hot(&thread_id).unwrap(), "an INBOX thread is hot");
        assert_eq!(thread_labels(&provider, &thread_id).await, vec!["INBOX"]);

        // Another client moves m1 from INBOX to Trash (same Message-ID -> same
        // stable id, so it is the same message in a new mailbox).
        {
            let mut mb = mailbox.lock().unwrap();
            mb.inbox().messages.retain(|m| m.uid != 1);
            mb.add_to("Trash", 50, &["\\Seen"], &message("<m1@x>", "Hello", ""));
        }
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();

        // The INBOX location is gone; the Trash location is present.
        assert!(store.locations_in_mailbox("INBOX").unwrap().is_empty());
        assert_eq!(store.locations_in_mailbox("Trash").unwrap().len(), 1);
        // The thread was already hot, so it was journaled and re-ingested; its
        // labels are now TRASH WITHOUT INBOX — never silently "archived".
        let labels = thread_labels(&provider, &thread_id).await;
        assert!(labels.contains(&"TRASH".to_string()), "has TRASH: {labels:?}");
        assert!(!labels.contains(&"INBOX".to_string()), "no INBOX: {labels:?}");
    }

    #[tokio::test]
    async fn an_index_mailbox_merge_inherits_hotness_and_keeps_reporting_changes() {
        for folder in ["Trash", "Junk"] {
            let mut mb = FakeMailbox::new(100);
            mb.add_to(folder, 50, &["\\Seen"], &message("<cold@x>", "Cold", ""));
            let mailbox = Arc::new(Mutex::new(mb));
            let (provider, store) = provider_with(mailbox.clone());
            let db = store.database();
            crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
                .await
                .unwrap();
            let cold_id = store.locations_in_mailbox(folder).unwrap()[0]
                .message_id
                .clone();
            let survivor = store.thread_of_message(&cold_id).unwrap().unwrap();
            assert!(!store.is_thread_hot(&survivor).unwrap());

            mailbox
                .lock()
                .unwrap()
                .add(&["\\Seen"], &message("<hot@x>", "Hot", ""));
            crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
                .await
                .unwrap();
            let hot_id = store.locations_in_mailbox("INBOX").unwrap()[0]
                .message_id
                .clone();
            let old_hot = store.thread_of_message(&hot_id).unwrap().unwrap();
            assert!(store.is_thread_hot(&old_hot).unwrap());
            let before_merge = store.generation().unwrap();

            mailbox.lock().unwrap().add_to(
                folder,
                50,
                &["\\Seen"],
                &message("<link@x>", "Link", "References: <cold@x> <hot@x>\r\n"),
            );
            crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
                .await
                .unwrap();
            assert_eq!(store.resolve_thread_alias(&old_hot).unwrap(), survivor);
            assert!(
                store.is_thread_hot(&survivor).unwrap(),
                "{folder} merge retains hotness"
            );
            let changed = store.journal_since(before_merge).unwrap();
            assert!(changed.contains(&old_hot), "retired hot id is journaled");
            assert!(
                changed.contains(&survivor),
                "previously cold survivor is journaled"
            );
            let cached = db
                .list_all_mail(Some("me@example.com"))
                .unwrap()
                .into_iter()
                .chain(db.list_trash(Some("me@example.com")).unwrap())
                .collect::<Vec<_>>();
            assert_eq!(cached.len(), 1, "the engine ingests one surviving thread");
            assert_eq!(cached[0].provider_thread_id, survivor);
            assert_eq!(db.get_thread(&cached[0].id).unwrap().messages.len(), 3);

            let after_merge = store.generation().unwrap();
            mailbox.lock().unwrap().inbox().messages[0].flags =
                vec!["\\Seen".into(), "\\Flagged".into()];
            crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
                .await
                .unwrap();
            assert!(
                store
                    .journal_since(after_merge)
                    .unwrap()
                    .contains(&survivor),
                "later INBOX flag changes must still reach the engine"
            );
            let updated = db
                .list_all_mail(Some("me@example.com"))
                .unwrap()
                .into_iter()
                .chain(db.list_trash(Some("me@example.com")).unwrap())
                .find(|thread| thread.provider_thread_id == survivor)
                .unwrap();
            assert!(
                updated.starred,
                "the engine applies the flag change after the merge"
            );
        }
    }

    /// (b) A never-hot thread that only ever appears in Trash -> locations,
    /// threads and tokens recorded, NOT journaled, NOT ingested, no body
    /// fetched; fetch_thread still works (cache-miss server fetch).
    #[tokio::test]
    async fn a_never_hot_trash_only_thread_is_recorded_but_not_journaled() {
        let mut mb = FakeMailbox::new(100); // empty INBOX
        mb.add_to("Trash", 50, &["\\Seen"], &message("<t1@x>", "Trashed", ""));
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox);
        let db = store.database();

        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();

        // Recorded: a Trash location + a thread + tokens.
        let trash = store.locations_in_mailbox("Trash").unwrap();
        assert_eq!(trash.len(), 1);
        let message_id = trash[0].message_id.clone();
        let thread_id = store.thread_of_message(&message_id).unwrap().unwrap();
        assert!(!store.is_thread_hot(&thread_id).unwrap(), "a Trash-only thread is not hot");

        // NOT journaled: the engine saw no mail.
        assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 0);

        // No body was fetched during index-only sync.
        assert!(provider.cache.get(&message_id).unwrap().is_none(), "no body cached at ingest");

        // fetch_thread still works: it fetches the body from Trash on demand.
        let messages = provider.fetch_thread(&thread_id).await.unwrap();
        assert_eq!(messages.len(), 1);
        assert!(messages[0].label_ids.contains(&"TRASH".to_string()));
    }

    /// (c) Junk likewise, with the SPAM label.
    #[tokio::test]
    async fn a_never_hot_junk_only_thread_is_recorded_with_the_spam_label() {
        let mut mb = FakeMailbox::new(100);
        mb.add_to("Junk", 60, &["\\Seen"], &message("<j1@x>", "Spammy", ""));
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox);
        let db = store.database();

        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        let junk = store.locations_in_mailbox("Junk").unwrap();
        assert_eq!(junk.len(), 1);
        let thread_id = store.thread_of_message(&junk[0].message_id).unwrap().unwrap();
        assert!(!store.is_thread_hot(&thread_id).unwrap());
        assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 0, "not journaled");

        let labels = thread_labels(&provider, &thread_id).await;
        assert!(labels.contains(&"SPAM".to_string()), "SPAM label: {labels:?}");
    }

    /// (d) Cadence gating: an unchanged, not-due folder issues no SEARCH/FETCH
    /// on the second poll; when the sweep is due a full sweep runs again and
    /// catches an external flag change.
    #[tokio::test]
    async fn cadence_gating_skips_an_unchanged_folder_until_the_sweep_is_due() {
        use std::sync::atomic::{AtomicI64, Ordering};
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<i1@x>", "Inbox", ""));
        mb.add_to("Trash", 50, &["\\Seen"], &message("<t1@x>", "Trashed", ""));
        let mailbox = Arc::new(Mutex::new(mb));
        let (mut provider, store) = provider_with(mailbox.clone());

        // A controllable clock so we can advance past the sweep interval.
        static CLOCK: AtomicI64 = AtomicI64::new(1_700_000_000);
        CLOCK.store(1_700_000_000, Ordering::SeqCst);
        provider.now = || CLOCK.load(Ordering::SeqCst);
        let db = store.database();

        // First sync sweeps Trash once (counts: 1 search).
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        let after_first = mailbox.lock().unwrap().counts("Trash").0;
        assert_eq!(after_first, 1, "Trash swept once on the baseline");

        // Second poll, counters unchanged and not sweep-due: NO new Trash
        // SEARCH is issued.
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        assert_eq!(
            mailbox.lock().unwrap().counts("Trash").0,
            after_first,
            "an unchanged, not-due Trash issues no SEARCH"
        );

        // An external client marks the Trash message \Flagged — this moves
        // NEITHER EXISTS nor UIDNEXT, so only the periodic sweep catches it.
        mailbox.lock().unwrap().folder("Trash", 50).messages[0].flags =
            vec!["\\Seen".into(), "\\Flagged".into()];
        // Advance the clock past the sweep interval.
        CLOCK.store(1_700_000_000 + super::super::policy::FOLDER_SWEEP_INTERVAL_SECS + 1, Ordering::SeqCst);
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        assert!(
            mailbox.lock().unwrap().counts("Trash").0 > after_first,
            "the due sweep issues a SEARCH again"
        );
        // The flag change is now reflected locally.
        let trash = store.locations_in_mailbox("Trash").unwrap();
        let flags: Vec<String> = serde_json::from_str(&trash[0].flags_json).unwrap();
        assert!(flags.iter().any(|f| f == "\\Flagged"), "the sweep caught the flag change");
    }

    /// (e) A UIDVALIDITY reset in Trash drops ONLY Trash's locations; INBOX is
    /// untouched.
    #[tokio::test]
    async fn a_uidvalidity_reset_in_trash_only_drops_trash_locations() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<i1@x>", "Inbox", ""));
        mb.add_to("Trash", 50, &["\\Seen"], &message("<t1@x>", "Trashed", ""));
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox.clone());
        let db = store.database();

        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        assert_eq!(store.locations_in_mailbox("INBOX").unwrap().len(), 1);
        assert_eq!(store.locations_in_mailbox("Trash").unwrap().len(), 1);

        // Reset Trash's UIDVALIDITY (new mailbox incarnation) and force a sweep.
        {
            let mut mb = mailbox.lock().unwrap();
            let trash = mb.folder("Trash", 999);
            trash.uidvalidity = 999;
            trash.uidnext = 1;
            trash.messages.clear();
            trash.add_sized(&["\\Seen"], &message("<t2@x>", "FreshTrash", ""), None);
        }
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();

        // INBOX is untouched; Trash was dropped and resynced to the new UID.
        assert_eq!(store.locations_in_mailbox("INBOX").unwrap().len(), 1, "INBOX untouched");
        let trash = store.locations_in_mailbox("Trash").unwrap();
        assert_eq!(trash.len(), 1);
        assert_eq!(trash[0].uidvalidity, 999, "Trash resynced under the new UIDVALIDITY");
    }

    /// (f) INBOX, Trash, Junk AND Sent are synced; Drafts, All Mail, Starred
    /// (\Flagged, no role), the label container and Archive are never selected
    /// or fetched. (Sent joined the synced set in run 3 and Archive in 5b-2 —
    /// intentional contract changes; this test previously named only
    /// INBOX/Trash/Junk/Sent as synced and asserted Archive was never synced.)
    #[tokio::test]
    async fn only_inbox_trash_junk_sent_and_archive_are_ever_synced() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<i1@x>", "Inbox", ""));
        mb.add_to("Trash", 50, &["\\Seen"], &message("<t1@x>", "Trashed", ""));
        mb.add_to("Junk", 60, &["\\Seen"], &message("<j1@x>", "Spam", ""));
        mb.add_to("Sent", 90, &["\\Seen"], &message("<s1@x>", "Sent", "")); // run 3: synced
        mb.add_to("Archive", 95, &["\\Seen"], &message("<ar1@x>", "Arch", "")); // 5b-2: synced
        // Folders that MUST never be synced:
        mb.add_to("Drafts", 70, &["\\Seen"], &message("<d1@x>", "Draft", ""));
        mb.add_to("All Mail", 80, &["\\Seen"], &message("<a1@x>", "All", ""));
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox.clone());
        let db = store.database();

        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();

        // Synced mailboxes have locations (Sent and Archive included this run).
        for synced in ["INBOX", "Trash", "Junk", "Sent", "Archive"] {
            assert!(!store.locations_in_mailbox(synced).unwrap().is_empty(), "{synced} synced");
        }
        // Never-synced mailboxes have NO locations and were never SEARCHed.
        let counts = mailbox.lock().unwrap();
        for never in ["Drafts", "All Mail"] {
            assert!(store.locations_in_mailbox(never).unwrap().is_empty(), "{never} not synced");
            assert_eq!(counts.counts(never).0, 0, "{never} never SEARCHed");
        }
    }

    /// (g) A folder that cannot be synced (its mailbox was renamed or deleted on
    /// the server, so EXAMINE answers NO) must NOT wedge the account. INBOX is
    /// the only mailbox whose failure aborts a poll: the engine ingests only
    /// after a poll succeeds, so an aborted poll would stop ALL mail flowing
    /// because of an index-tier folder. Folder failures are skipped (and
    /// logged) and retried next poll.
    #[tokio::test]
    async fn a_failing_folder_does_not_wedge_inbox_sync() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<i1@x>", "Inbox", ""));
        mb.folder("Trash", 50).examine_fails = true;
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox.clone());
        let db = store.database();

        // Initial sync succeeds and INBOX mail is ingested despite Trash failing.
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
            .await
            .expect("a broken folder must not fail the sync");
        assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 1);

        // New INBOX mail keeps flowing on later polls while Trash stays broken.
        mailbox
            .lock()
            .unwrap()
            .add(&["\\Seen"], &message("<i2@x>", "Second", ""));
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
            .await
            .expect("still not wedged");
        assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 2);

        // And once Trash is fixed it syncs on a later poll (retried, not dropped).
        {
            let mut mb = mailbox.lock().unwrap();
            let trash = mb.folder("Trash", 50);
            trash.examine_fails = false;
        }
        mailbox
            .lock()
            .unwrap()
            .add_to("Trash", 50, &["\\Seen"], &message("<t1@x>", "Trashed", ""));
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
            .await
            .unwrap();
        assert_eq!(store.locations_in_mailbox("Trash").unwrap().len(), 1);
    }

    /// (i) Idle polls do not bump the generation or rebuild threader state,
    /// even with Trash/Junk present (the multi-mailbox walk stays idle-safe).
    #[tokio::test]
    async fn idle_polls_with_folders_present_do_not_bump_the_generation() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<i1@x>", "Inbox", ""));
        mb.add_to("Trash", 50, &["\\Seen"], &message("<t1@x>", "Trashed", ""));
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox);
        let db = store.database();

        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        let generation_after_first = store.generation().unwrap();
        let loads_after_first =
            provider.thread_state_loads.load(std::sync::atomic::Ordering::Relaxed);

        // An idle re-sync: nothing changed anywhere.
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        assert_eq!(
            store.generation().unwrap(),
            generation_after_first,
            "an idle multi-mailbox poll does not bump the generation"
        );
        assert_eq!(
            provider.thread_state_loads.load(std::sync::atomic::Ordering::Relaxed),
            loads_after_first,
            "an idle poll does not rebuild threader state"
        );
    }

    // ------------------------------------------------------------------
    // Slice 5b-1 run 3: Sent sync, chunked acquisition, catalog refresh
    // ------------------------------------------------------------------

    /// Seed `n` Sent messages (UIDs 1..=n) with distinct Message-IDs.
    fn seed_sent(mb: &mut FakeMailbox, uidvalidity: u32, n: usize) {
        for i in 1..=n {
            mb.add_to("Sent", uidvalidity, &["\\Seen"], &message(&format!("<s{i}@x>"), "Sent", ""));
        }
    }

    /// Run one baseline + `polls` extra polls through the engine.
    async fn sync_n(provider: &ImapProvider, db: &crate::db::Database, polls: usize) {
        for _ in 0..=polls {
            crate::sync::sync_with(db, "me@example.com", provider).await.unwrap();
        }
    }

    /// The Sent backfill watermark NULL (complete) / Some (pending).
    fn sent_marker(store: &ImapStateStore) -> Option<i64> {
        store.sent_backfill_low_uid("Sent").unwrap()
    }

    /// (a) The SAME Message-ID in INBOX and Sent is ONE message id with both
    /// labels (INBOX+SENT) and exactly ONE body fetch (shared stable id).
    #[tokio::test]
    async fn the_same_message_in_inbox_and_sent_is_one_message_with_both_labels() {
        let shared = "<shared@x>";
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message(shared, "Bcc self", ""));
        mb.add_to("Sent", 90, &["\\Seen"], &message(shared, "Bcc self", ""));
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox.clone());
        let db = store.database();

        sync_n(&provider, db.as_ref(), 1).await;

        // One stable id with two locations (INBOX + Sent).
        let inbox = store.locations_in_mailbox("INBOX").unwrap();
        let sent = store.locations_in_mailbox("Sent").unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(sent.len(), 1);
        assert_eq!(inbox[0].message_id, sent[0].message_id, "one id, two locations");

        // fetch_thread unions the labels: INBOX and SENT, no duplicate message.
        let thread_id = store.thread_of_message(&inbox[0].message_id).unwrap().unwrap();
        let messages = provider.fetch_thread(&thread_id).await.unwrap();
        assert_eq!(messages.len(), 1, "one message, not two");
        let labels = &messages[0].label_ids;
        assert!(labels.contains(&"INBOX".to_string()), "has INBOX: {labels:?}");
        assert!(labels.contains(&"SENT".to_string()), "has SENT: {labels:?}");

        // Exactly one body cached for the shared id (bodies dedup by stable id).
        assert!(provider.cache.contains(&inbox[0].message_id).unwrap());
        // The body was fetched at most twice total across both mailboxes'
        // ingest, but cached ONCE (deduped). The key assertion is one message.
    }

    /// (b) Chunking: with a tiny per-round budget, N Sent messages are
    /// acquired NEWEST FIRST over several rounds, with no duplicates, and the
    /// engine ends with all of them. The engine drives several polls per
    /// `sync_with` (a full sync is baseline + an incremental pass, and later
    /// syncs poll once), so this asserts the INVARIANTS — the acquired set is
    /// always the newest contiguous suffix, grows monotonically, never
    /// duplicates, and the backfill marker clears exactly when all are in —
    /// rather than a brittle per-call count.
    #[tokio::test]
    async fn sent_is_acquired_in_newest_first_chunks_over_several_polls() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<i1@x>", "Inbox", "")); // keep INBOX non-empty
        seed_sent(&mut mb, 90, 5); // Sent UIDs 1..=5
        let mailbox = Arc::new(Mutex::new(mb));
        let (mut provider, store) = provider_with(mailbox.clone());
        provider.sent_round_budget = Some(1); // one UID per round
        let db = store.database();

        // Drive the account to completion, checking invariants after each
        // `sync_with` until the backfill marker clears.
        let mut last_len = 0usize;
        for _ in 0..10 {
            crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
            let uids = sent_uids(&store);
            // No duplicates (sorted, unique).
            let mut dedup = uids.clone();
            dedup.dedup();
            assert_eq!(dedup, uids, "no duplicate Sent locations");
            // Newest-first: the acquired set is always the top-K contiguous
            // suffix of 1..=5, i.e. [6-k ..= 5].
            if !uids.is_empty() {
                let k = uids.len() as i64;
                let expected: Vec<i64> = ((6 - k)..=5).collect();
                assert_eq!(uids, expected, "acquired set is the newest contiguous suffix");
            }
            // Monotonic growth.
            assert!(uids.len() >= last_len, "acquisition never loses ground");
            last_len = uids.len();
            if sent_marker(&store).is_none() && uids.len() == 5 {
                break;
            }
        }
        assert_eq!(sent_uids(&store), vec![1, 2, 3, 4, 5], "all five eventually acquired");
        assert!(sent_marker(&store).is_none(), "backfill complete, marker cleared");

        // Every Sent message is now fetchable with the SENT label.
        for location in store.locations_in_mailbox("Sent").unwrap() {
            let thread = store.thread_of_message(&location.message_id).unwrap().unwrap();
            let labels = thread_labels(&provider, &thread).await;
            assert!(labels.contains(&"SENT".to_string()), "SENT label: {labels:?}");
        }
    }

    /// The Sent UIDs currently held locally, ascending.
    fn sent_uids(store: &ImapStateStore) -> Vec<i64> {
        let mut uids: Vec<i64> = store
            .locations_in_mailbox("Sent")
            .unwrap()
            .into_iter()
            .map(|l| l.uid)
            .collect();
        uids.sort_unstable();
        uids
    }

    /// (c) Resume after a failure mid-chunk: a dropped connection on a body
    /// fetch commits NOTHING for that round (one transaction), so the next
    /// round re-derives the same remaining set, loses nothing and acquires
    /// nothing twice.
    #[tokio::test]
    async fn a_failure_mid_sent_chunk_loses_nothing_and_acquires_nothing_twice() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<i1@x>", "Inbox", ""));
        seed_sent(&mut mb, 90, 3);
        let mailbox = Arc::new(Mutex::new(mb));
        let (mut provider, store) = provider_with(mailbox.clone());
        provider.sent_round_budget = Some(3); // try to take all 3 at once

        // Baseline: INBOX commits; the Sent round drops its connection after
        // the first body fetch, so NO Sent location is committed.
        {
            let mut guard = mailbox.lock().unwrap();
            guard.folder("Sent", 90).fail_after_body_fetches = Some(1);
        }
        let db = store.database();
        // The folder failure is swallowed by refresh_all_with (not INBOX), so
        // the poll still succeeds overall.
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        assert!(sent_uids(&store).is_empty(), "a failed Sent chunk commits no locations");
        assert_eq!(store.locations_in_mailbox("INBOX").unwrap().len(), 1, "INBOX still committed");

        // Heal the server and poll again: all 3 Sent messages acquired, each
        // exactly once (no duplicate UID/message rows).
        {
            let mut guard = mailbox.lock().unwrap();
            guard.folder("Sent", 90).fail_after_body_fetches = None;
        }
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        assert_eq!(sent_uids(&store), vec![1, 2, 3], "resumed cleanly, nothing lost or doubled");
        assert!(sent_marker(&store).is_none(), "backfill complete after resume");
    }

    /// (d) Cadence gating does NOT skip Sent while the backfill is pending even
    /// when EXISTS/UIDNEXT are unchanged; once complete, the normal cadence
    /// skip resumes. (The engine drives several polls per `sync_with`, so this
    /// compares SEARCH activity while pending vs. after completion rather than
    /// per-call counts.)
    #[tokio::test]
    async fn cadence_does_not_skip_sent_while_backfill_pending_then_resumes() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<i1@x>", "Inbox", ""));
        seed_sent(&mut mb, 90, 4);
        let mailbox = Arc::new(Mutex::new(mb));
        let (mut provider, store) = provider_with(mailbox.clone());
        provider.sent_round_budget = Some(1); // one per round => several rounds
        let db = store.database();

        // Baseline leaves the backfill pending (not all 4 acquired yet with a
        // budget of 1 across baseline's two internal rounds).
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        assert!(sent_marker(&store).is_some(), "backfill still pending after baseline");

        // While pending, each sync_with keeps SEARCHing Sent (not skipped),
        // because EXISTS/UIDNEXT unchanged does not mean the window is fully
        // acquired. Drive to completion, confirming SEARCH count rises.
        let mut prev_searches = mailbox.lock().unwrap().counts("Sent").0;
        for _ in 0..10 {
            if sent_marker(&store).is_none() {
                break;
            }
            crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
            let searches = mailbox.lock().unwrap().counts("Sent").0;
            assert!(
                searches > prev_searches,
                "a pending Sent backfill is NOT skipped by cadence"
            );
            prev_searches = searches;
        }
        assert_eq!(sent_uids(&store), vec![1, 2, 3, 4], "all acquired");
        assert!(sent_marker(&store).is_none(), "complete now");

        // Now complete, counters unchanged, not sweep-due => Sent is SKIPPED.
        let before_skip = mailbox.lock().unwrap().counts("Sent").0;
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        assert_eq!(
            mailbox.lock().unwrap().counts("Sent").0,
            before_skip,
            "a complete, unchanged, not-due Sent is skipped again"
        );
    }

    /// (e) Sent is NON-evicting: a message acquired then pushed past the window
    /// by newer mail is NOT evicted, while an evicting class (Trash) in the
    /// same situation IS; a server-side expunge of an acquired Sent message
    /// DOES delete it.
    #[tokio::test]
    async fn sent_does_not_evict_aged_out_messages_but_trash_does() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<i1@x>", "Inbox", ""));
        seed_sent(&mut mb, 90, 2); // Sent UIDs 1,2
        mb.add_to("Trash", 50, &["\\Seen"], &message("<t1@x>", "T1", "")); // Trash UID 1
        mb.add_to("Trash", 50, &["\\Seen"], &message("<t2@x>", "T2", "")); // Trash UID 2
        let mailbox = Arc::new(Mutex::new(mb));
        // Tiny window (2) for BOTH Sent and Trash via the test knob below.
        let (mut provider, store) = provider_with(mailbox.clone());
        provider.sent_round_budget = Some(10);
        provider.folder_window_override = Some(2);
        let db = store.database();

        sync_n(&provider, db.as_ref(), 2).await;
        assert_eq!(sent_uids(&store), vec![1, 2], "both Sent acquired");
        assert_eq!(store.locations_in_mailbox("Trash").unwrap().len(), 2);

        // Newer mail pushes the oldest (UID 1) past the 2-message window in
        // BOTH mailboxes.
        {
            let mut g = mailbox.lock().unwrap();
            g.add_to("Sent", 90, &["\\Seen"], &message("<s3@x>", "S3", "")); // Sent UID 3
            g.add_to("Trash", 50, &["\\Seen"], &message("<t3@x>", "T3", "")); // Trash UID 3
        }
        sync_n(&provider, db.as_ref(), 2).await;

        // Sent keeps UID 1 (non-evicting acquisition bound): all three held.
        assert_eq!(sent_uids(&store), vec![1, 2, 3], "Sent never evicts an aged-out message");
        // Trash evicted UID 1 (sliding window): only the newest two remain.
        let trash_uids: Vec<i64> = {
            let mut u: Vec<i64> = store
                .locations_in_mailbox("Trash")
                .unwrap()
                .into_iter()
                .map(|l| l.uid)
                .collect();
            u.sort_unstable();
            u
        };
        assert_eq!(trash_uids, vec![2, 3], "Trash evicts the aged-out UID 1");

        // A server-side expunge of an acquired Sent message DOES delete it.
        {
            let mut g = mailbox.lock().unwrap();
            g.folder("Sent", 90).messages.retain(|m| m.uid != 2);
        }
        sync_n(&provider, db.as_ref(), 2).await;
        assert!(!sent_uids(&store).contains(&2), "an expunged Sent message is deleted");
    }

    /// (f) A Sent copy never makes the message UNREAD even without \Seen.
    #[tokio::test]
    async fn a_sent_copy_is_never_unread_even_without_seen() {
        let mut mb = FakeMailbox::new(100);
        // No \Seen flag on the Sent message.
        mb.add_to("Sent", 90, &[], &message("<s1@x>", "Unseen sent", ""));
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox);
        let db = store.database();

        sync_n(&provider, db.as_ref(), 1).await;
        let sent = store.locations_in_mailbox("Sent").unwrap();
        assert_eq!(sent.len(), 1);
        let thread = store.thread_of_message(&sent[0].message_id).unwrap().unwrap();
        let labels = thread_labels(&provider, &thread).await;
        assert!(labels.contains(&"SENT".to_string()), "SENT: {labels:?}");
        assert!(!labels.contains(&"UNREAD".to_string()), "a Sent copy is never UNREAD: {labels:?}");
    }

    /// (g) A UIDVALIDITY reset in Sent only: Sent's locations are dropped and
    /// resynced; INBOX locations and the message->thread mapping are untouched.
    #[tokio::test]
    async fn a_uidvalidity_reset_in_sent_only_drops_sent_locations() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<i1@x>", "Inbox", ""));
        seed_sent(&mut mb, 90, 2);
        let mailbox = Arc::new(Mutex::new(mb));
        let (mut provider, store) = provider_with(mailbox.clone());
        provider.sent_round_budget = Some(10);
        let db = store.database();

        sync_n(&provider, db.as_ref(), 1).await;
        assert_eq!(store.locations_in_mailbox("INBOX").unwrap().len(), 1);
        assert_eq!(sent_uids(&store), vec![1, 2]);
        let inbox_thread = {
            let inbox = store.locations_in_mailbox("INBOX").unwrap();
            store.thread_of_message(&inbox[0].message_id).unwrap().unwrap()
        };

        // Reset Sent's UIDVALIDITY and reseed with a fresh message.
        {
            let mut g = mailbox.lock().unwrap();
            let sent = g.folder("Sent", 999);
            sent.uidvalidity = 999;
            sent.uidnext = 1;
            sent.messages.clear();
            sent.add_sized(&["\\Seen"], &message("<s9@x>", "Fresh", ""), None);
        }
        sync_n(&provider, db.as_ref(), 2).await;

        // INBOX untouched; its thread mapping preserved.
        assert_eq!(store.locations_in_mailbox("INBOX").unwrap().len(), 1, "INBOX untouched");
        let inbox_after = store.locations_in_mailbox("INBOX").unwrap();
        assert_eq!(
            store.thread_of_message(&inbox_after[0].message_id).unwrap().unwrap(),
            inbox_thread,
            "INBOX message->thread mapping kept"
        );
        // Sent dropped and resynced under the new UIDVALIDITY.
        let sent = store.locations_in_mailbox("Sent").unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].uidvalidity, 999, "Sent resynced under the new UIDVALIDITY");
    }

    /// (h) Catalog refresh adds a mailbox created AFTER setup without
    /// clobbering counters, ignores \Noselect, and a failing LIST does not
    /// abort the poll.
    #[tokio::test]
    async fn catalog_refresh_notices_a_new_mailbox_without_clobbering_counters() {
        use std::sync::atomic::{AtomicI64, Ordering};
        // Start with just INBOX in the catalog; the server will later grow a
        // Junk folder that did not exist at setup.
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<i1@x>", "Inbox", ""));
        let mailbox = Arc::new(Mutex::new(mb));
        let (mut provider, store) = provider_with(mailbox.clone());
        static CLOCK_H: AtomicI64 = AtomicI64::new(1_700_000_000);
        CLOCK_H.store(1_700_000_000, Ordering::SeqCst);
        provider.now = || CLOCK_H.load(Ordering::SeqCst);
        let db = store.database();

        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        // Only INBOX in the catalog so far.
        let names: Vec<String> = store.mailboxes().unwrap().into_iter().map(|m| m.name).collect();
        assert_eq!(names, vec!["INBOX".to_string()]);

        // Record INBOX's learned counters, to prove the refresh never clobbers
        // them on a mailbox that already exists.
        let inbox_before = store
            .mailboxes()
            .unwrap()
            .into_iter()
            .find(|m| m.name == "INBOX")
            .unwrap();
        assert!(inbox_before.uidvalidity != 0, "INBOX learned a real uidvalidity");

        // A Junk folder appears on the server after setup. Advance the clock
        // past the catalog sweep interval so the refresh is due again.
        {
            let mut g = mailbox.lock().unwrap();
            g.add_to("Junk", 60, &["\\Seen"], &message("<j1@x>", "Spam", ""));
        }
        CLOCK_H.store(
            1_700_000_000 + super::super::policy::FOLDER_SWEEP_INTERVAL_SECS + 1,
            Ordering::SeqCst,
        );

        // Re-run with a due catalog refresh.
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();

        // Junk is now in the catalog (noticed by the refresh) and gets synced.
        let names: Vec<String> = store.mailboxes().unwrap().into_iter().map(|m| m.name).collect();
        assert!(names.contains(&"Junk".to_string()), "new Junk noticed: {names:?}");

        // INBOX's counters were not clobbered by the catalog-only upsert.
        let inbox_after = store
            .mailboxes()
            .unwrap()
            .into_iter()
            .find(|m| m.name == "INBOX")
            .unwrap();
        assert_eq!(
            inbox_after.uidvalidity, inbox_before.uidvalidity,
            "catalog refresh never clobbers an existing mailbox's counters"
        );
    }

    /// Catalog refresh: a failing LIST is swallowed and the poll still ingests
    /// INBOX; \Noselect containers are never added.
    #[tokio::test]
    async fn catalog_refresh_failure_does_not_abort_and_noselect_is_skipped() {
        use std::sync::atomic::{AtomicI64, Ordering};
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<i1@x>", "Inbox", ""));
        mb.list_fails = true; // the server's LIST answers an error
        mb.noselect.push("Folders".into()); // a \Noselect container the LIST reports
        let mailbox = Arc::new(Mutex::new(mb));
        let (mut provider, store) = provider_with(mailbox.clone());
        static CLOCK_C: AtomicI64 = AtomicI64::new(1_700_000_000);
        CLOCK_C.store(1_700_000_000, Ordering::SeqCst);
        provider.now = || CLOCK_C.load(Ordering::SeqCst);
        let db = store.database();

        // The poll succeeds and INBOX mail is ingested despite the LIST error.
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
            .await
            .expect("a failing catalog LIST must not abort the poll");
        assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 1);

        // Heal LIST; advance past the catalog sweep interval so the refresh is
        // due again. The \Noselect container is reported but never added.
        {
            let mut g = mailbox.lock().unwrap();
            g.list_fails = false;
        }
        CLOCK_C.store(
            1_700_000_000 + super::super::policy::FOLDER_SWEEP_INTERVAL_SECS + 1,
            Ordering::SeqCst,
        );
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        let names: Vec<String> = store.mailboxes().unwrap().into_iter().map(|m| m.name).collect();
        assert!(!names.contains(&"Folders".to_string()), "\\Noselect never added: {names:?}");
    }

    /// (i) A Sent-first reply whose parent arrives later in INBOX merges into
    /// ONE thread; the older id survives, an alias is recorded, and the engine
    /// ends with one local thread.
    #[tokio::test]
    async fn a_sent_first_reply_merges_with_a_later_inbox_parent_into_one_thread() {
        // The reply is in Sent first (we sent it), referencing a parent
        // Message-ID that only arrives in INBOX on a later poll.
        let mut mb = FakeMailbox::new(100);
        mb.add_to(
            "Sent",
            90,
            &["\\Seen"],
            &message("<reply@x>", "Re: Hi", "In-Reply-To: <parent@x>\r\nReferences: <parent@x>\r\n"),
        );
        let mailbox = Arc::new(Mutex::new(mb));
        let (mut provider, store) = provider_with(mailbox.clone());
        provider.sent_round_budget = Some(10);
        let db = store.database();

        sync_n(&provider, db.as_ref(), 1).await;
        // The reply made its thread hot (Sent location) and is one thread.
        let sent = store.locations_in_mailbox("Sent").unwrap();
        let reply_thread = store.thread_of_message(&sent[0].message_id).unwrap().unwrap();
        assert!(store.is_thread_hot(&reply_thread).unwrap(), "a Sent reply is hot");

        // The parent now arrives in INBOX, linking to the Sent reply.
        {
            let mut g = mailbox.lock().unwrap();
            g.add(&["\\Seen"], &message("<parent@x>", "Hi", ""));
        }
        sync_n(&provider, db.as_ref(), 1).await;

        // Exactly ONE local thread remains; both messages are in it.
        let rows = store.all_message_threads().unwrap();
        let survivors: std::collections::BTreeSet<String> = rows
            .iter()
            .map(|(_, t, _)| store.resolve_thread_alias(t).unwrap())
            .collect();
        assert_eq!(survivors.len(), 1, "the Sent reply and INBOX parent are one thread");
        let survivor = survivors.into_iter().next().unwrap();
        assert_eq!(store.messages_in_thread(&survivor).unwrap().len(), 2);
        // The thread carries both INBOX and SENT labels across its copies.
        let labels = thread_labels(&provider, &survivor).await;
        assert!(labels.contains(&"INBOX".to_string()) && labels.contains(&"SENT".to_string()),
            "merged thread spans INBOX and SENT: {labels:?}");
    }

    /// (j) A Sent-only thread is hot, journaled, ingestible with a body,
    /// carries SENT and no INBOX, and does NOT appear in list_inbox; an idle
    /// reconcile poll does not re-journal it or treat it as INBOX drift.
    ///
    /// Full recovery lists INBOX and then consumes the baseline's replay of
    /// other hot threads through its incremental pass.
    #[tokio::test]
    async fn a_sent_only_thread_is_hot_journaled_and_not_treated_as_inbox_drift() {
        let mut mb = FakeMailbox::new(100);
        mb.add_to("Sent", 90, &["\\Seen"], &message("<s1@x>", "Only sent", ""));
        let mailbox = Arc::new(Mutex::new(mb));
        let (mut provider, store) = provider_with(mailbox.clone());
        provider.sent_round_budget = Some(10);

        // baseline_cursor refreshes every synced mailbox and journals the Sent
        // thread (hot). It is reported to the engine on a poll from BELOW its
        // generation — the at-least-once path the engine ingests through.
        let baseline = provider.baseline_cursor().await.unwrap();
        let sent = store.locations_in_mailbox("Sent").unwrap();
        assert_eq!(sent.len(), 1, "the Sent message was acquired");
        let thread = store.thread_of_message(&sent[0].message_id).unwrap().unwrap();
        assert!(store.is_thread_hot(&thread).unwrap(), "a Sent-only thread is hot");

        // Journaled: a poll from generation 0 returns the Sent thread (this is
        // how the engine ingests a hot thread).
        let from_zero = provider.poll(&SyncCursor::from_generation(0)).await.unwrap();
        assert!(
            from_zero.changed_threads.iter().any(|t| {
                store.resolve_thread_alias(t).unwrap() == store.resolve_thread_alias(&thread).unwrap()
            }),
            "a hot Sent-only thread is journaled/reported: {:?}",
            from_zero.changed_threads
        );

        // SENT and no INBOX; a body is fetchable; and it is NOT in list_inbox.
        let labels = thread_labels(&provider, &thread).await;
        assert!(labels.contains(&"SENT".to_string()) && !labels.contains(&"INBOX".to_string()));
        assert_eq!(provider.fetch_thread(&thread).await.unwrap().len(), 1, "has a body");
        assert!(
            !provider.list_inbox(None).await.unwrap().thread_ids.contains(&thread),
            "a Sent-only thread is not in list_inbox"
        );

        // The incremental pass the engine runs right after the baseline delivers
        // the Sent-only thread (the baseline cursor sits before the hot-thread
        // replay). Once delivered, a poll from the NEW cursor
        // is idle: it neither bumps the generation nor re-journals the thread,
        // so the Sent-only thread is never treated as INBOX drift.
        let generation = store.generation().unwrap();
        let delivered = provider.poll(&baseline).await.unwrap();
        assert!(
            delivered.changed_threads.iter().any(|t| {
                store.resolve_thread_alias(t).unwrap() == store.resolve_thread_alias(&thread).unwrap()
            }),
            "the pass after the baseline delivers the Sent-only thread"
        );
        assert_eq!(store.generation().unwrap(), generation, "delivery does not bump");
        let idle = provider.poll(&delivered.cursor).await.unwrap();
        assert_eq!(store.generation().unwrap(), generation, "idle reconcile does not bump");
        assert!(idle.changed_threads.is_empty(), "idle reconcile re-journals nothing");
    }

    /// Full recovery ingests both INBOX's snapshot and the replay of hot
    /// threads outside INBOX on its first incremental pass.
    #[tokio::test]
    async fn sent_only_threads_acquired_at_baseline_reach_the_engine() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<i1@x>", "Inbox", ""));
        mb.add_to("Sent", 90, &["\\Seen"], &message("<s1@x>", "Only sent", ""));
        let mailbox = Arc::new(Mutex::new(mb));
        let (mut provider, store) = provider_with(mailbox);
        provider.sent_round_budget = Some(10);
        let db = store.database();

        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
            .await
            .unwrap();
        assert_eq!(
            db.list_all_mail(Some("me@example.com")).unwrap().len(),
            2,
            "the INBOX thread AND the Sent-only thread are ingested by the first sync"
        );
        // And nothing is lost or duplicated by the next pass.
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
            .await
            .unwrap();
        assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 2);
    }

    #[tokio::test]
    async fn sent_baseline_crash_before_engine_recovery_is_saved_replays_acquired_mail() {
        let mut mb = FakeMailbox::new(100);
        seed_sent(&mut mb, 90, 3);
        let (mut provider, store) = provider_with(Arc::new(Mutex::new(mb)));
        provider.sent_round_budget = Some(1);
        let db = store.database();
        // Two interruptions after baseline commits, before begin_sync_recovery.
        for expected in [vec![3], vec![2, 3]] {
            provider.baseline_cursor().await.unwrap();
            assert_eq!(sent_uids(&store), expected);
            assert!(db.cursor("me@example.com").unwrap().is_none());
            assert!(db.recovery_cursor("me@example.com").unwrap().is_none());
        }
        // Reconstruct the provider's transient state, keeping only persisted
        // state and the fake server across the simulated process restart.
        let provider = ImapProvider {
            cache: BodyCache::new(store.clone()),
            thread_state_loads: std::sync::atomic::AtomicUsize::new(0),
            ..provider
        };
        sync_n(&provider, db.as_ref(), 1).await;
        assert_eq!(
            db.list_all_mail(Some("me@example.com")).unwrap().len(),
            3,
            "every previously acquired chunk reaches the engine on recovery"
        );
        let generation = store.generation().unwrap();
        sync_n(&provider, db.as_ref(), 2).await;
        assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 3);
        assert_eq!(
            store.generation().unwrap(),
            generation,
            "idle polls do not replay"
        );
    }

    #[tokio::test]
    async fn baseline_replay_survives_journal_pruning_and_excludes_cold_threads() {
        let mut mb = FakeMailbox::new(100);
        seed_sent(&mut mb, 90, 1);
        mb.add_to("Trash", 80, &["\\Seen"], &message("<cold@x>", "Cold", ""));
        let (provider, store) = provider_with(Arc::new(Mutex::new(mb)));
        let db = store.database();
        provider.baseline_cursor().await.unwrap();
        let old = store.generation().unwrap();
        // Age the acquired Sent message out of journal retention.
        db.with_connection(|connection| {
            connection.execute(
                "UPDATE imap_sync_state SET generation = generation + ?1 WHERE account_id = ?2",
                rusqlite::params![(policy::JOURNAL_RETENTION_GENERATIONS + 1) as i64, "me@example.com"],
            )?;
            Ok(())
        })
        .unwrap();
        store.prune_journal().unwrap();
        assert!(store.journal_since(0).unwrap().is_empty());
        assert!(!policy::journal_cursor_is_answerable(
            old,
            store.generation().unwrap()
        ));

        sync_n(&provider, db.as_ref(), 1).await;
        assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 1);
        assert!(
            db.list_trash(Some("me@example.com")).unwrap().is_empty(),
            "a cold Trash-only thread remains index-only during baseline replay"
        );
    }

    #[tokio::test]
    async fn baseline_replays_deletions_of_previously_hot_sent_threads() {
        let mut mb = FakeMailbox::new(100);
        seed_sent(&mut mb, 90, 1);
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox.clone());
        let db = store.database();
        sync_n(&provider, db.as_ref(), 1).await;
        assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 1);
        mailbox.lock().unwrap().folder("Sent", 90).messages.clear();
        // The provider commits the expunge; the engine does not consume it.
        provider.refresh_all().await.unwrap();
        assert!(sent_uids(&store).is_empty());
        // A full recovery still needs the now-locationless hot thread id. Under
        // the Slice 5b-2 EMPTIED_THREAD_GRACE_WALKS grace the thread is NOT
        // deleted on first observation of emptiness — it SURVIVES the grace and
        // is deleted only after EMPTIED_THREAD_GRACE_WALKS completed coverage epochs pass
        // (this test previously asserted immediate deletion). Drive polls until
        // the grace expires and the engine deletes it, bounded.
        db.clear_cursor("me@example.com").unwrap();
        let mut deleted = false;
        for _ in 0..8 {
            crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
            if db.list_all_mail(Some("me@example.com")).unwrap().is_empty() {
                deleted = true;
                break;
            }
        }
        assert!(deleted, "the long-empty hot thread is deleted once the grace expires");
    }

    #[tokio::test]
    async fn retained_sent_flags_are_synced_outside_the_acquisition_window() {
        let mut mb = FakeMailbox::new(100);
        seed_sent(&mut mb, 90, 2);
        let mailbox = Arc::new(Mutex::new(mb));
        let (mut provider, store) = provider_with(mailbox.clone());
        provider.folder_window_override = Some(2);
        let db = store.database();
        sync_n(&provider, db.as_ref(), 1).await;
        mailbox
            .lock()
            .unwrap()
            .add_to("Sent", 90, &["\\Seen"], &message("<s3@x>", "New", ""));
        sync_n(&provider, db.as_ref(), 1).await;
        assert_eq!(
            sent_uids(&store),
            vec![1, 2, 3],
            "non-evicting Sent retains UID 1"
        );
        let before_flag_change = store.generation().unwrap();
        let old = store
            .locations_in_mailbox("Sent")
            .unwrap()
            .into_iter()
            .find(|l| l.uid == 1)
            .unwrap();
        let old_thread = store.thread_of_message(&old.message_id).unwrap().unwrap();
        let before_body_fetches = mailbox.lock().unwrap().counts("Sent").1;
        mailbox.lock().unwrap().folder("Sent", 90).messages[0].flags =
            vec!["\\Seen".into(), "\\Flagged".into()];
        // Unchanged EXISTS/UIDNEXT: the periodic sweep must discover the flag.
        provider.now = || 1_700_000_000 + policy::FOLDER_SWEEP_INTERVAL_SECS;
        sync_n(&provider, db.as_ref(), 1).await;
        let old = store
            .locations_in_mailbox("Sent")
            .unwrap()
            .into_iter()
            .find(|l| l.uid == 1)
            .unwrap();
        let flags: Vec<String> = serde_json::from_str(&old.flags_json).unwrap();
        assert!(
            flags.contains(&"\\Flagged".into()),
            "a Sent sweep updates retained messages outside the acquisition window: {flags:?}"
        );
        assert!(store
            .journal_since(before_flag_change)
            .unwrap()
            .contains(&old_thread));
        assert!(
            db.list_all_mail(Some("me@example.com"))
                .unwrap()
                .iter()
                .any(|thread| thread.provider_thread_id == old_thread && thread.starred),
            "the engine receives the retained Sent thread's updated star"
        );
        assert_eq!(
            mailbox.lock().unwrap().counts("Sent").1,
            before_body_fetches,
            "a retained flag update does not re-download bodies"
        );
    }

    /// (k) Idle polls with Sent complete: generation unchanged, no threader
    /// rebuild.
    #[tokio::test]
    async fn idle_polls_with_sent_complete_do_not_bump_the_generation() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<i1@x>", "Inbox", ""));
        seed_sent(&mut mb, 90, 2);
        let mailbox = Arc::new(Mutex::new(mb));
        let (mut provider, store) = provider_with(mailbox);
        provider.sent_round_budget = Some(10);
        let db = store.database();

        sync_n(&provider, db.as_ref(), 1).await;
        assert!(sent_marker(&store).is_none(), "backfill complete");
        let generation = store.generation().unwrap();
        let loads = provider.thread_state_loads.load(std::sync::atomic::Ordering::Relaxed);

        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        assert_eq!(store.generation().unwrap(), generation, "idle poll does not bump generation");
        assert_eq!(
            provider.thread_state_loads.load(std::sync::atomic::Ordering::Relaxed),
            loads,
            "idle poll does not rebuild threader state"
        );
    }

    // ------------------------------------------------------------------
    // Slice 5b-2: fair scheduling, Archive sync, emptied-hot-thread grace
    // ------------------------------------------------------------------

    /// Count the open tasks linked to a provider thread id (via its local id).
    fn task_count_for(store: &ImapStateStore, provider_thread_id: &str) -> i64 {
        let local = format!("me@example.com:{provider_thread_id}");
        store
            .database()
            .with_connection(|c| {
                Ok(c.query_row(
                    "SELECT COUNT(*) FROM tasks WHERE thread_id = ?1",
                    [&local],
                    |r| r.get(0),
                )?)
            })
            .unwrap()
    }

    /// Attach a task to a provider thread's LOCAL thread row (as the triage
    /// surface would), so a test can prove the grace leaves local state intact.
    fn attach_task(store: &ImapStateStore, provider_thread_id: &str, task_id: &str) {
        let local = format!("me@example.com:{provider_thread_id}");
        store
            .database()
            .with_connection(|c| {
                c.execute(
                    "INSERT INTO tasks(id, account_id, thread_id, subject_snapshot, title, kind,
                        due_kind, status, created_at, updated_at)
                     VALUES (?1, 'me@example.com', ?2, 'S', 'T', 'action', 'none', 'open',
                        '2026-09-20T00:00:00Z', '2026-09-20T00:00:00Z')",
                    rusqlite::params![task_id, local],
                )?;
                Ok(())
            })
            .unwrap();
    }

    /// (a) A hot INBOX message moved to ARCHIVE by another client: because
    /// Archive is now synced (5b-2), its new location is picked up, the thread
    /// survives as ONE local thread with NO INBOX label (archived == no system
    /// location label), and the engine never deletes it.
    #[tokio::test]
    async fn a_hot_inbox_message_moved_to_archive_survives_without_an_inbox_label() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<m1@x>", "Hello", ""));
        mb.folder("Archive", 95); // present but empty for now
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox.clone());
        let db = store.database();

        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        let threads = provider.list_inbox(None).await.unwrap();
        assert_eq!(threads.thread_ids.len(), 1);
        let thread_id = threads.thread_ids[0].clone();
        assert_eq!(thread_labels(&provider, &thread_id).await, vec!["INBOX"]);

        // Another client moves m1 from INBOX to Archive (same Message-ID ->
        // same stable id).
        {
            let mut mb = mailbox.lock().unwrap();
            mb.inbox().messages.retain(|m| m.uid != 1);
            mb.add_to("Archive", 95, &["\\Seen"], &message("<m1@x>", "Hello", ""));
        }
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();

        // INBOX location gone, Archive location present, ONE engine thread.
        assert!(store.locations_in_mailbox("INBOX").unwrap().is_empty());
        assert_eq!(store.locations_in_mailbox("Archive").unwrap().len(), 1);
        assert_eq!(
            db.list_all_mail(Some("me@example.com")).unwrap().len(),
            1,
            "the engine keeps exactly one local thread"
        );
        // The thread carries NO INBOX label — archived has no system location
        // label — and is not NotFound.
        let labels = thread_labels(&provider, &thread_id).await;
        assert!(!labels.contains(&"INBOX".to_string()), "no INBOX after archiving: {labels:?}");
    }

    /// (b) A hot INBOX message moved into a mailbox NOT swept this poll (forced
    /// via `examine_fails` on the destination): the thread has zero locations
    /// momentarily, SURVIVES the grace (its local copy — including a linked
    /// task — is untouched), then the destination recovers and the thread is
    /// re-journaled with the right labels.
    #[tokio::test]
    async fn a_move_into_an_unswept_mailbox_survives_the_grace_then_recovers() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<m1@x>", "Hello", ""));
        // Archive exists but EXAMINE fails this poll (stands in for "a mailbox
        // not swept this poll" — the scheduler visited it but it errored, so
        // the move's destination is not seen).
        mb.folder("Archive", 95).examine_fails = true;
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox.clone());
        let db = store.database();

        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        let thread_id = provider.list_inbox(None).await.unwrap().thread_ids[0].clone();
        assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 1);
        // Attach a task to the local thread; it must survive the grace.
        attach_task(&store, &thread_id, "task-b");
        assert_eq!(task_count_for(&store, &thread_id), 1);

        // Another client moves m1 out of INBOX into the (un-examinable) Archive.
        {
            let mut mb = mailbox.lock().unwrap();
            mb.inbox().messages.retain(|m| m.uid != 1);
            mb.add_to("Archive", 95, &["\\Seen"], &message("<m1@x>", "Hello", ""));
        }
        // Poll: INBOX location gone, Archive EXAMINE fails so the destination
        // is not seen -> the hot thread has zero locations. The grace keeps it.
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        assert!(store.locations_in_mailbox("INBOX").unwrap().is_empty());
        assert_eq!(
            db.list_all_mail(Some("me@example.com")).unwrap().len(),
            1,
            "the thread survives the grace while its destination is unswept"
        );
        assert_eq!(task_count_for(&store, &thread_id), 1, "the linked task survived the grace");

        // The destination recovers: Archive is examinable again.
        mailbox.lock().unwrap().folder("Archive", 95).examine_fails = false;
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        assert_eq!(store.locations_in_mailbox("Archive").unwrap().len(), 1, "Archive location found");
        // Still one thread, task intact, and now re-journaled with correct
        // labels (no INBOX; Archive contributes no system label).
        assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 1);
        assert_eq!(task_count_for(&store, &thread_id), 1, "task preserved after recovery");
        let labels = thread_labels(&provider, &thread_id).await;
        assert!(!labels.contains(&"INBOX".to_string()), "no INBOX after the move: {labels:?}");
    }

    /// (c) A true server-side expunge of the last copy: the local thread is
    /// kept for the grace, then deleted after EMPTIED_THREAD_GRACE_WALKS
    /// completed coverage epochs; idle polls during the grace do not bump the
    /// generation.
    #[tokio::test]
    async fn a_true_expunge_keeps_the_thread_for_the_grace_then_deletes_it() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<m1@x>", "Hello", ""));
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox.clone());
        let db = store.database();

        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 1);

        // The server expunges the only copy.
        mailbox.lock().unwrap().inbox().messages.clear();

        // First poll after the expunge: the thread has zero locations and is
        // hot, so the grace keeps it (NOT deleted yet).
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        assert_eq!(
            db.list_all_mail(Some("me@example.com")).unwrap().len(),
            1,
            "the first empty observation does not delete (grace)"
        );
        let gen_during_grace = store.generation().unwrap();

        // An idle poll during the grace (nothing changed) does not bump the
        // generation.
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        // Keep polling until the thread is finally deleted once the grace is spent.
        let mut deleted = false;
        for _ in 0..8 {
            crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
            if db.list_all_mail(Some("me@example.com")).unwrap().is_empty() {
                deleted = true;
                break;
            }
        }
        assert!(deleted, "the thread is deleted after EMPTIED_THREAD_GRACE_WALKS completed coverage epochs");
        // The generation moved only when the grace expired (the deletion was
        // journaled), not on the idle grace polls: it is >= the grace-era value.
        assert!(store.generation().unwrap() >= gen_during_grace);
    }

    /// (d) A folder that keeps failing EXAMINE is never visited in the current
    /// coverage epoch, so the epoch cannot advance and the emptied-thread grace
    /// does not progress: nothing is deleted early. Once the folder recovers,
    /// the epoch advances and the grace expires.
    #[tokio::test]
    async fn a_failing_folder_blocks_the_coverage_epoch_and_the_grace() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<m1@x>", "Hello", ""));
        // A Trash folder that always fails EXAMINE -> it is never visited in
        // the current epoch, so the epoch never completes.
        mb.folder("Trash", 50).examine_fails = true;
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox.clone());
        let db = store.database();

        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 1);
        let walks_before = store.complete_walks().unwrap();

        // Expunge the INBOX copy: the thread goes empty.
        mailbox.lock().unwrap().inbox().messages.clear();

        // Many polls, but Trash keeps failing so the coverage epoch cannot
        // advance and the grace never progresses: the thread is NOT deleted.
        for _ in 0..6 {
            crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        }
        assert_eq!(
            store.complete_walks().unwrap(),
            walks_before,
            "a failing folder blocks coverage-epoch advance"
        );
        assert_eq!(
            db.list_all_mail(Some("me@example.com")).unwrap().len(),
            1,
            "the emptied thread is NOT deleted while coverage cannot complete"
        );

        // Heal Trash: walks complete again, the grace advances, deletion lands.
        mailbox.lock().unwrap().folder("Trash", 50).examine_fails = false;
        let mut deleted = false;
        for _ in 0..8 {
            crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
            if db.list_all_mail(Some("me@example.com")).unwrap().is_empty() {
                deleted = true;
                break;
            }
        }
        assert!(deleted, "once walks complete again the grace expires and the thread is deleted");
    }

    /// (k) Archive is an EVICTING window (Folder class) at a tiny limit, while
    /// Sent (acquisition bound) is NOT, in the same account.
    #[tokio::test]
    async fn archive_evicts_at_a_tiny_window_while_sent_does_not() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<i1@x>", "Inbox", ""));
        // Archive and Sent each start with 2 messages.
        for i in 1..=2 {
            mb.add_to("Archive", 95, &["\\Seen"], &message(&format!("<a{i}@x>"), "A", ""));
            mb.add_to("Sent", 90, &["\\Seen"], &message(&format!("<s{i}@x>"), "S", ""));
        }
        let mailbox = Arc::new(Mutex::new(mb));
        let (mut provider, store) = provider_with(mailbox.clone());
        provider.sent_round_budget = Some(10);
        provider.folder_window_override = Some(2); // tiny window for both
        let db = store.database();

        sync_n(&provider, db.as_ref(), 2).await;
        let archive_uids = |s: &ImapStateStore| {
            let mut u: Vec<i64> = s.locations_in_mailbox("Archive").unwrap().into_iter().map(|l| l.uid).collect();
            u.sort_unstable();
            u
        };
        assert_eq!(archive_uids(&store), vec![1, 2], "both Archive acquired");
        assert_eq!(sent_uids(&store), vec![1, 2], "both Sent acquired");

        // Newer mail pushes the oldest (UID 1) past the 2-message window in both.
        {
            let mut g = mailbox.lock().unwrap();
            g.add_to("Archive", 95, &["\\Seen"], &message("<a3@x>", "A3", ""));
            g.add_to("Sent", 90, &["\\Seen"], &message("<s3@x>", "S3", ""));
        }
        sync_n(&provider, db.as_ref(), 2).await;

        // Archive EVICTS the aged-out UID 1 (sliding window); Sent keeps it.
        assert_eq!(archive_uids(&store), vec![2, 3], "Archive evicts the aged-out UID 1");
        assert_eq!(sent_uids(&store), vec![1, 2, 3], "Sent never evicts (acquisition bound)");
    }

    /// (l) Fairness integration: with 20 synthetic extra `synced_now` entries
    /// injected through the test-only scheduler knob and a small budget, every
    /// one is VISITED within ceil(20/budget) polls. The synthetic candidates
    /// are scheduled by name only (no plan entry, so no server round), proving
    /// the scheduler's rotation without a 20-folder fixture.
    #[tokio::test]
    async fn fairness_every_injected_folder_is_scheduled_within_ceil_n_over_budget_polls() {
        // The scheduler is a pure function; here we drive it the way
        // refresh_all_with does, through the injected candidates, and record
        // which names schedule_visits returns across polls. We assert on the
        // SELECTION (schedule_visits output) which is what bounds server work.
        let budget = policy::FOLDER_ROUNDS_PER_POLL; // production budget
        let n = 20usize;
        // Build 20 synthetic user-folder candidates (never covered at start).
        let names: Vec<String> = (0..n).map(|i| format!("folder-{i:02}")).collect();
        let epoch = 1i64;
        let mut visited_in_epoch: std::collections::BTreeMap<String, i64> =
            names.iter().map(|n| (n.clone(), 0i64)).collect();
        let mut ever: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let polls = n.div_ceil(budget);
        for _ in 0..polls {
            let candidates: Vec<policy::ScheduleCandidate> = names
                .iter()
                .map(|name| policy::ScheduleCandidate {
                    mailbox: name.clone(),
                    last_attempt: 0,
                    last_visited_at: 0,
                    visited_in_epoch: visited_in_epoch[name],
                    is_system_role: false,
                })
                .collect();
            let visits = policy::schedule_visits(&candidates, epoch, budget);
            for v in &visits {
                ever.insert(v.clone());
                visited_in_epoch.insert(v.clone(), epoch);
            }
        }
        assert_eq!(ever.len(), n, "every injected folder visited within ceil(N/budget) polls");

        // And prove the knob is wired into refresh_all_with: an injected
        // candidate is accepted (does not panic / abort the walk) and the walk
        // still completes INBOX. The synthetic names have no plan entry, so
        // they take no server round.
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<i1@x>", "Inbox", ""));
        let mailbox = Arc::new(Mutex::new(mb));
        let (mut provider, store) = provider_with(mailbox);
        provider.extra_synced_candidates = Some(
            names
                .iter()
                .map(|name| policy::ScheduleCandidate {
                    mailbox: name.clone(),
                    last_attempt: 0,
                    last_visited_at: 0,
                    visited_in_epoch: 0,
                    is_system_role: false,
                })
                .collect(),
        );
        let db = store.database();
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
            .await
            .expect("injected synthetic candidates must not wedge the walk");
        assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 1, "INBOX still synced");
    }

    // ------------------------------------------------------------------
    // Slice 5b-2 coverage-epoch fix: a "complete walk" is coverage ACROSS
    // polls, not a single poll that visited every mailbox. With more synced
    // mailboxes than the per-poll budget, no single poll covers them all, so
    // the epoch (and the emptied-thread grace) must still advance once every
    // synced mailbox has been visited across successive polls.
    // ------------------------------------------------------------------

    /// (1) More synced mailboxes than the per-poll budget: the coverage epoch
    /// advances only after EVERY synced mailbox was visited (over several
    /// polls), and a truly-expunged hot thread's last copy is deleted after
    /// exactly two completed epochs — not never (the old per-poll definition
    /// would have deadlocked the epoch here forever).
    #[tokio::test]
    async fn coverage_epoch_advances_across_polls_and_grace_eventually_deletes() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<m1@x>", "Hello", ""));
        // Two extra server-backed folders, forced to USER-FOLDER class so they
        // count against the per-poll budget (not system roles, which are always
        // visited). They start non-empty so they are examinable.
        mb.add_to("Junk", 60, &["\\Seen"], &message("<j1@x>", "J", ""));
        mb.add_to("Archive", 95, &["\\Seen"], &message("<a1@x>", "A", ""));
        let mailbox = Arc::new(Mutex::new(mb));
        let (mut provider, store) = provider_with(mailbox.clone());
        provider.force_user_folders =
            Some(["Junk".to_string(), "Archive".to_string()].into_iter().collect());
        provider.folder_rounds_per_poll_override = Some(1); // one user folder per poll
        let db = store.database();

        // Baseline + one poll. With a budget of 1 and two user folders, no
        // single poll covers both — the epoch must still advance across polls.
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 1);

        // Expunge the only INBOX copy: the hot thread goes empty.
        mailbox.lock().unwrap().inbox().messages.clear();

        // Drive polls until the thread is deleted. Each poll visits INBOX +
        // (budget 1) one user folder, so it takes two polls to complete one
        // coverage epoch, and two completed epochs to spend the grace. The old
        // per-poll logic never advances the epoch here, so the thread would
        // linger forever — this loop would never see the deletion.
        let mut deleted = false;
        for _ in 0..12 {
            crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
            if db.list_all_mail(Some("me@example.com")).unwrap().is_empty() {
                deleted = true;
                break;
            }
        }
        assert!(
            deleted,
            "a truly-expunged hot thread is deleted after the grace, even when no single poll covers every synced mailbox"
        );
        // The epoch really did advance beyond the baseline (coverage worked).
        assert!(store.complete_walks().unwrap() >= policy::EMPTIED_THREAD_GRACE_WALKS);
    }

    /// (2) A folder that keeps failing EXAMINE prevents the coverage epoch from
    /// advancing (that folder is never visited in the current epoch), so
    /// nothing is deleted early; once it recovers, the epoch advances and the
    /// grace expires. Uses forced user folders + budget 1 so coverage genuinely
    /// depends on the failing folder succeeding.
    #[tokio::test]
    async fn a_failing_folder_blocks_epoch_advance_until_it_recovers() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<m1@x>", "Hello", ""));
        mb.add_to("Junk", 60, &["\\Seen"], &message("<j1@x>", "J", ""));
        mb.add_to("Archive", 95, &["\\Seen"], &message("<a1@x>", "A", ""));
        // Archive (a forced user folder) always fails EXAMINE.
        mb.folder("Archive", 95).examine_fails = true;
        let mailbox = Arc::new(Mutex::new(mb));
        let (mut provider, store) = provider_with(mailbox.clone());
        provider.force_user_folders =
            Some(["Junk".to_string(), "Archive".to_string()].into_iter().collect());
        provider.folder_rounds_per_poll_override = Some(1);
        let db = store.database();

        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        let epoch_before = store.complete_walks().unwrap();

        // Expunge the INBOX copy: the thread goes empty.
        mailbox.lock().unwrap().inbox().messages.clear();

        // Many polls, but Archive never succeeds, so the epoch cannot advance
        // (Archive is never visited in the current epoch) and nothing is
        // deleted early.
        for _ in 0..10 {
            crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        }
        assert_eq!(
            store.complete_walks().unwrap(),
            epoch_before,
            "a persistently failing folder blocks coverage-epoch advance"
        );
        assert_eq!(
            db.list_all_mail(Some("me@example.com")).unwrap().len(),
            1,
            "nothing is deleted early while coverage cannot complete"
        );

        // Heal Archive: the epoch advances and the grace eventually expires.
        mailbox.lock().unwrap().folder("Archive", 95).examine_fails = false;
        let mut deleted = false;
        for _ in 0..12 {
            crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
            if db.list_all_mail(Some("me@example.com")).unwrap().is_empty() {
                deleted = true;
                break;
            }
        }
        assert!(deleted, "once the folder recovers the epoch advances and the thread is deleted");
    }

    /// (3) A poll that records visits but does NOT complete a coverage epoch
    /// (partial coverage under a tiny budget) must not bump the generation —
    /// visit recording is cadence/bookkeeping state, not a content change.
    #[tokio::test]
    async fn partial_coverage_polls_do_not_bump_the_generation() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<m1@x>", "Hello", ""));
        mb.add_to("Junk", 60, &["\\Seen"], &message("<j1@x>", "J", ""));
        mb.add_to("Archive", 95, &["\\Seen"], &message("<a1@x>", "A", ""));
        let mailbox = Arc::new(Mutex::new(mb));
        let (mut provider, store) = provider_with(mailbox.clone());
        provider.force_user_folders =
            Some(["Junk".to_string(), "Archive".to_string()].into_iter().collect());
        provider.folder_rounds_per_poll_override = Some(1);
        let db = store.database();

        // Baseline brings everything in and settles.
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        // Let it reach a steady state where nothing content-changes.
        for _ in 0..4 {
            crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        }
        let generation = store.generation().unwrap();
        let loads = provider.thread_state_loads.load(std::sync::atomic::Ordering::Relaxed);

        // Several more idle polls. Each records a visit for the one scheduled
        // user folder (partial coverage, budget 1) but nothing content-changed,
        // so the generation must not move and the threader is not rebuilt —
        // whether or not a given poll happens to complete a coverage epoch.
        for _ in 0..4 {
            crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
            assert_eq!(
                store.generation().unwrap(),
                generation,
                "an idle poll (partial or full coverage) does not bump the generation"
            );
        }
        assert_eq!(
            provider.thread_state_loads.load(std::sync::atomic::Ordering::Relaxed),
            loads,
            "idle polls do not rebuild threader state"
        );
    }

    // ------------------------------------------------------------------
    // Slice 5b-2 run B: folder:/lf: labels, user/label-folder sync, and
    // vanished-mailbox retirement, all driven through sync::sync_with over
    // the multi-mailbox fake.
    // ------------------------------------------------------------------

    /// Collect every label id across all messages of a thread, sorted + deduped
    /// (the shared `thread_labels` above returns them unsorted/undeduped).
    async fn thread_label_set(provider: &ImapProvider, thread_id: &str) -> Vec<String> {
        let mut labels = thread_labels(provider, thread_id).await;
        labels.sort();
        labels.dedup();
        labels
    }

    /// The stable message id of the (single) message currently in `mailbox`.
    fn message_id_in(store: &ImapStateStore, mailbox: &str) -> String {
        store
            .locations_in_mailbox(mailbox)
            .expect("locations")
            .first()
            .map(|l| l.message_id.clone())
            .unwrap_or_else(|| panic!("no message in {mailbox}"))
    }

    /// The resolved thread id for a stable message id.
    fn thread_of(store: &ImapStateStore, message_id: &str) -> String {
        let t = store
            .thread_of_message(message_id)
            .expect("thread_of_message")
            .expect("a thread");
        store.resolve_thread_alias(&t).expect("resolve")
    }

    /// (a) A hot INBOX message that another client MOVED into a user folder:
    /// the thread survives, its labels become `folder:<name>` WITHOUT INBOX,
    /// and the engine keeps ONE local thread with the folder label on its root.
    #[tokio::test]
    async fn an_inbox_message_moved_into_a_user_folder_keeps_the_thread_with_a_folder_label() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<u1@x>", "Hello", ""));
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox.clone());
        let db = store.database();

        // Baseline: the message is in INBOX, hot.
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        let mid = message_id_in(&store, "INBOX");
        let tid = thread_of(&store, &mid);
        assert!(store.is_thread_hot(&tid).unwrap(), "an INBOX message is hot");
        assert_eq!(thread_label_set(&provider, &tid).await, vec!["INBOX"]);

        // Another client moves it: INBOX copy gone, a copy appears in the user
        // folder Clients/Acme (same Message-ID = same stable id).
        {
            let mut m = mailbox.lock().unwrap();
            m.inbox().messages.clear();
            m.add_to("Clients/Acme", 70, &["\\Seen"], &message("<u1@x>", "Hello", ""));
        }
        // The destination folder is brand new on the server, so re-arm the
        // catalog refresh to notice it (the fixed test clock otherwise keeps the
        // periodic refresh from coming due again).
        store.force_catalog_refresh_due_for_test();
        // Poll enough times to visit INBOX (sees the deletion) and the user
        // folder (sees the new copy) within the emptied-thread grace.
        for _ in 0..4 {
            crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        }

        // One local thread survives, labelled folder:Clients/Acme, no INBOX.
        assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 1);
        let mid = message_id_in(&store, "Clients/Acme");
        let tid = thread_of(&store, &mid);
        let labels = thread_label_set(&provider, &tid).await;
        assert!(labels.contains(&"folder:Clients/Acme".to_string()), "{labels:?}");
        assert!(!labels.contains(&"INBOX".to_string()), "no INBOX after the move: {labels:?}");
    }

    /// (b) Proton-style label copy: an INBOX message with an additional copy in
    /// `Labels/Clients` -> `lf:Clients` + INBOX, and the `lf:` label is on
    /// EVERY message of the thread even when only a REPLY was labelled.
    #[tokio::test]
    async fn a_label_folder_copy_adds_lf_to_every_message_of_the_thread() {
        let mut mb = FakeMailbox::new(100);
        // Root in INBOX; a reply in INBOX; the REPLY also copied into the label
        // folder (Proton labels one message of the thread).
        mb.add(&["\\Seen"], &message("<root@x>", "Topic", ""));
        mb.add(&["\\Seen"], &message("<reply@x>", "Re: Topic", "In-Reply-To: <root@x>\r\n"));
        mb.add_to(
            "Labels/Clients",
            70,
            &["\\Seen"],
            &message("<reply@x>", "Re: Topic", "In-Reply-To: <root@x>\r\n"),
        );
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox.clone());
        let db = store.database();

        for _ in 0..4 {
            crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        }

        let mid = message_id_in(&store, "Labels/Clients");
        let tid = thread_of(&store, &mid);
        // The thread has INBOX and lf:Clients.
        let labels = thread_label_set(&provider, &tid).await;
        assert!(labels.contains(&"INBOX".to_string()), "{labels:?}");
        assert!(labels.contains(&"lf:Clients".to_string()), "{labels:?}");

        // Every message of the thread carries lf:Clients, even the root, which
        // was never itself in the label folder (fact 1 — thread-wide union).
        let messages = provider.fetch_thread(&tid).await.unwrap();
        assert_eq!(messages.len(), 2, "root + reply");
        for m in &messages {
            assert!(
                m.label_ids.contains(&"lf:Clients".to_string()),
                "message {} carries lf:Clients: {:?}",
                m.id,
                m.label_ids
            );
        }
    }

    /// (c) A message in TWO label folders gets both `lf:` labels; (i) dynamic
    /// ids with spaces/colon/non-ASCII round-trip through fetch_thread.
    #[tokio::test]
    async fn a_message_in_two_label_folders_gets_both_lf_labels() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<m@x>", "Two labels", ""));
        mb.add_to("Labels/Clients", 70, &["\\Seen"], &message("<m@x>", "Two labels", ""));
        mb.add_to("Labels/Zoë's: tag", 71, &["\\Seen"], &message("<m@x>", "Two labels", ""));
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox.clone());
        let db = store.database();

        for _ in 0..5 {
            crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        }
        let mid = message_id_in(&store, "Labels/Clients");
        let tid = thread_of(&store, &mid);
        let labels = thread_label_set(&provider, &tid).await;
        assert!(labels.contains(&"INBOX".to_string()), "{labels:?}");
        assert!(labels.contains(&"lf:Clients".to_string()), "{labels:?}");
        assert!(labels.contains(&"lf:Zoë's: tag".to_string()), "{labels:?}");
    }

    /// (e) A never-hot thread that only ever lives in a user folder is RECORDED
    /// (index-synced locations + a local thread) but NEVER hot — so it is never
    /// reported to the engine as a changed hot thread.
    #[tokio::test]
    async fn a_user_folder_only_thread_is_recorded_but_never_hot() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<i@x>", "Inbox", "")); // keeps INBOX non-trivial
        mb.add_to("Clients/Acme", 70, &["\\Seen"], &message("<folder-only@x>", "Folder only", ""));
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox.clone());
        let db = store.database();

        for _ in 0..4 {
            crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        }
        // The folder-only message is index-synced (its location is recorded).
        assert!(
            !store.locations_in_mailbox("Clients/Acme").unwrap().is_empty(),
            "the user folder is index-synced"
        );
        // Its thread exists locally but is NOT hot (never reported).
        let mid = message_id_in(&store, "Clients/Acme");
        let tid = thread_of(&store, &mid);
        assert!(!store.is_thread_hot(&tid).unwrap(), "a user-folder-only thread is never hot");
        // And it still resolves to its folder: label when fetched directly.
        let labels = thread_label_set(&provider, &tid).await;
        assert_eq!(labels, vec!["folder:Clients/Acme"]);
    }

    /// (f) `list_labels` content, order and kinds: the six system labels, then
    /// `lf:` labels (kind `user`, relative name) sorted, then `folder:` labels
    /// (kind `folder`, full name) sorted — INCLUDING a label folder with no
    /// messages in it.
    #[tokio::test]
    async fn list_labels_publishes_system_then_lf_then_folder_in_order() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<i@x>", "Inbox", ""));
        mb.add_to("Clients/Acme", 70, &["\\Seen"], &message("<c@x>", "C", ""));
        mb.add_to("Labels/Work", 71, &["\\Seen"], &message("<w@x>", "W", ""));
        // An EMPTY label folder (no messages) must still be published.
        mb.folder("Labels/Empty", 72);
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox.clone());
        let db = store.database();
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();

        let labels = provider.list_labels().await.unwrap();
        let shape: Vec<(&str, &str)> =
            labels.iter().map(|l| (l.id.as_str(), l.kind.as_str())).collect();
        assert_eq!(
            shape,
            vec![
                ("INBOX", "system"),
                ("SENT", "system"),
                ("SPAM", "system"),
                ("TRASH", "system"),
                ("UNREAD", "system"),
                ("STARRED", "system"),
                ("lf:Empty", "user"),
                ("lf:Work", "user"),
                ("folder:Clients/Acme", "folder"),
            ]
        );
        // The dynamic labels carry a display name = id without its prefix.
        let empty = labels.iter().find(|l| l.id == "lf:Empty").unwrap();
        assert_eq!(empty.name, "Empty");
        let acme = labels.iter().find(|l| l.id == "folder:Clients/Acme").unwrap();
        assert_eq!(acme.name, "Clients/Acme");
    }

    /// (g) With MORE real user folders than the per-poll budget, every one is
    /// visited within ceil(N/budget) polls and the coverage epoch advances so
    /// an expunged hot thread is deleted after the grace — proving user folders
    /// (not just the synthetic knob) feed the scheduler/epoch machinery.
    #[tokio::test]
    async fn many_real_user_folders_are_all_covered_and_the_epoch_advances() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<m1@x>", "Hello", ""));
        // Three real user folders, budget 1: no single poll covers them all.
        for (i, name) in ["Clients/A", "Clients/B", "Clients/C"].iter().enumerate() {
            mb.add_to(name, 60 + i as u32, &["\\Seen"], &message(&format!("<f{i}@x>"), "F", ""));
        }
        let mailbox = Arc::new(Mutex::new(mb));
        let (mut provider, store) = provider_with(mailbox.clone());
        provider.folder_rounds_per_poll_override = Some(1);
        let db = store.database();

        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 1);

        // Expunge the INBOX copy: the hot thread goes empty.
        mailbox.lock().unwrap().inbox().messages.clear();

        // Drive polls: three user folders at budget 1 means a coverage epoch
        // takes several polls, and two epochs to spend the grace. The thread is
        // deleted only once the epoch machinery advances across polls.
        let mut deleted = false;
        for _ in 0..20 {
            crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
            if db.list_all_mail(Some("me@example.com")).unwrap().is_empty() {
                deleted = true;
                break;
            }
        }
        assert!(deleted, "coverage over real user folders advances the epoch and spends the grace");
        assert!(store.complete_walks().unwrap() >= policy::EMPTIED_THREAD_GRACE_WALKS);
        // Every user folder was index-synced (all visited).
        for name in ["Clients/A", "Clients/B", "Clients/C"] {
            assert!(!store.locations_in_mailbox(name).unwrap().is_empty(), "{name} visited");
        }
    }

    /// (h) Vanished mailbox: a catalog row the server stops LISTing is retired
    /// (catalog + sync-state + locations deleted), the affected hot thread is
    /// journaled and kept through the grace, the epoch is no longer held open,
    /// an empty/failed LIST retires nothing, INBOX is never retired, and a
    /// re-appearing mailbox is re-added.
    #[tokio::test]
    async fn a_vanished_mailbox_is_retired_and_stops_holding_the_epoch_open() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<i@x>", "Inbox", ""));
        // A user folder holding a HOT thread's only non-INBOX copy, plus an
        // INBOX copy so the thread is hot.
        mb.add(&["\\Seen"], &message("<shared@x>", "Shared", ""));
        mb.add_to("Clients/Acme", 70, &["\\Seen"], &message("<shared@x>", "Shared", ""));
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox.clone());
        let db = store.database();

        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        assert!(!store.locations_in_mailbox("Clients/Acme").unwrap().is_empty());
        // The shared message is the one copied into Clients/Acme; resolve its
        // thread by that unique location (INBOX also holds the unrelated <i@x>).
        let shared_id = message_id_in(&store, "Clients/Acme");
        let tid = thread_of(&store, &shared_id);
        assert!(store.is_thread_hot(&tid).unwrap());

        // An EMPTY/failed LIST retires nothing: a LIST that fails is swallowed,
        // and the catalog is untouched.
        {
            let mut m = mailbox.lock().unwrap();
            m.list_fails = true;
        }
        store.force_catalog_refresh_due_for_test();
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        assert!(
            store.mailboxes().unwrap().iter().any(|mb| mb.name == "Clients/Acme"),
            "a failed LIST retires nothing"
        );

        // Now the user folder genuinely vanishes from the server and LIST
        // succeeds without it.
        {
            let mut m = mailbox.lock().unwrap();
            m.list_fails = false;
            m.folders.remove("Clients/Acme");
        }
        store.force_catalog_refresh_due_for_test();
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();

        // Retired: catalog row, sync-state row and locations are gone.
        assert!(
            !store.mailboxes().unwrap().iter().any(|mb| mb.name == "Clients/Acme"),
            "the vanished mailbox's catalog row is retired"
        );
        assert!(store.locations_in_mailbox("Clients/Acme").unwrap().is_empty());
        assert!(store.mailbox_sync_state("Clients/Acme").unwrap().is_none());
        // INBOX was never retired.
        assert!(store.mailboxes().unwrap().iter().any(|mb| mb.name == "INBOX"));

        // The shared thread still has its INBOX copy, so it stays hot and
        // present (the retirement journaled it; it did not delete it).
        let tid = thread_of(&store, &shared_id);
        assert!(store.is_thread_hot(&tid).unwrap());
        assert!(!thread_label_set(&provider, &tid).await.is_empty());

        // The epoch is no longer held open by the vanished folder: a couple of
        // polls complete coverage over the REMAINING synced set.
        let epoch_before = store.complete_walks().unwrap();
        for _ in 0..3 {
            crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        }
        assert!(
            store.complete_walks().unwrap() > epoch_before,
            "with the broken folder retired, the coverage epoch advances again"
        );

        // Re-appearance: the mailbox comes back and is re-added, its message
        // re-syncs (bodies untouched — threading is intact).
        {
            let mut m = mailbox.lock().unwrap();
            m.add_to("Clients/Acme", 99, &["\\Seen"], &message("<shared@x>", "Shared", ""));
        }
        store.force_catalog_refresh_due_for_test();
        for _ in 0..4 {
            crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        }
        assert!(
            store.mailboxes().unwrap().iter().any(|mb| mb.name == "Clients/Acme"),
            "a re-appearing mailbox is re-added"
        );
        assert!(!store.locations_in_mailbox("Clients/Acme").unwrap().is_empty());
    }

    /// A retirement that touches NO hot thread is a catalog-only change: it does
    /// not bump the generation (nothing for the engine to re-ingest).
    #[tokio::test]
    async fn retiring_a_cold_only_mailbox_does_not_bump_the_generation() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<i@x>", "Inbox", ""));
        // A user folder with a COLD (never-INBOX, never-hot) thread only.
        mb.add_to("Clients/Cold", 70, &["\\Seen"], &message("<cold@x>", "Cold", ""));
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox.clone());
        let db = store.database();
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        // Settle.
        for _ in 0..3 {
            crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        }
        let gen_before = store.generation().unwrap();

        // Vanish the cold-only folder.
        {
            let mut m = mailbox.lock().unwrap();
            m.folders.remove("Clients/Cold");
        }
        store.force_catalog_refresh_due_for_test();
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        assert!(
            !store.mailboxes().unwrap().iter().any(|mb| mb.name == "Clients/Cold"),
            "the cold-only folder is retired"
        );
        // No hot thread was affected, so the generation did not move for the
        // retirement itself (idle polls aside).
        assert_eq!(
            store.generation().unwrap(),
            gen_before,
            "retiring a cold-only mailbox is a catalog-only change"
        );
    }

    /// (k) Idle polls after the dynamic-label set is synced do not bump the
    /// generation or rebuild the threader.
    #[tokio::test]
    async fn idle_polls_with_user_and_label_folders_are_stable() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<i@x>", "Inbox", ""));
        mb.add_to("Clients/Acme", 70, &["\\Seen"], &message("<c@x>", "C", ""));
        mb.add_to("Labels/Work", 71, &["\\Seen"], &message("<w@x>", "W", ""));
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox.clone());
        let db = store.database();
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        for _ in 0..4 {
            crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
        }
        let generation = store.generation().unwrap();
        let loads = provider.thread_state_loads.load(std::sync::atomic::Ordering::Relaxed);
        for _ in 0..4 {
            crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();
            assert_eq!(store.generation().unwrap(), generation, "idle poll must not bump generation");
        }
        assert_eq!(
            provider.thread_state_loads.load(std::sync::atomic::Ordering::Relaxed),
            loads,
            "idle polls do not rebuild threader state"
        );
    }

    #[tokio::test]
    async fn expired_threads_do_not_rejournal_on_idle_polls() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<review-gone@x>", "Gone", ""));
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox.clone());
        let db = store.database();
        sync_n(&provider, db.as_ref(), 1).await;
        mailbox.lock().unwrap().inbox().messages.clear();
        sync_n(&provider, db.as_ref(), 5).await;
        assert!(db.list_all_mail(Some("me@example.com")).unwrap().is_empty());
        let before = store.generation().unwrap();
        sync_n(&provider, db.as_ref(), 3).await;
        assert_eq!(
            store.generation().unwrap(),
            before,
            "a deleted thread must not re-journal forever"
        );
    }

    #[tokio::test]
    async fn mailbox_cap_does_not_block_coverage_forever() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<review-cap@x>", "Cap", ""));
        for i in 0..=policy::MAX_SYNCED_MAILBOXES {
            mb.folder(&format!("Folder/{i:04}"), 10);
        }
        let mailbox = Arc::new(Mutex::new(mb));
        let (mut provider, store) = provider_with(mailbox.clone());
        provider.folder_rounds_per_poll_override = Some(policy::MAX_SYNCED_MAILBOXES);
        let db = store.database();
        sync_n(&provider, db.as_ref(), 1).await;
        mailbox.lock().unwrap().inbox().messages.clear();
        sync_n(&provider, db.as_ref(), 5).await;
        assert!(
            db.list_all_mail(Some("me@example.com")).unwrap().is_empty(),
            "capped-out folders must not make an actual expunge immortal; epoch={}",
            store.complete_walks().unwrap()
        );
    }

    #[tokio::test]
    async fn failed_folders_do_not_starve_healthy_folders() {
        let mut mb = FakeMailbox::new(100);
        for i in 0..policy::FOLDER_ROUNDS_PER_POLL {
            mb.folder(&format!("A-fails-{i:02}"), 10).examine_fails = true;
        }
        mb.add_to(
            "Z-healthy",
            10,
            &["\\Seen"],
            &message("<healthy1@x>", "Healthy", ""),
        );
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox.clone());
        sync_n(&provider, store.database().as_ref(), 5).await;
        assert_eq!(store.locations_in_mailbox("Z-healthy").unwrap().len(), 1);
        assert_eq!(
            store.complete_walks().unwrap(),
            0,
            "failed attempts do not claim coverage"
        );
        mailbox.lock().unwrap().add_to(
            "Z-healthy",
            10,
            &["\\Seen"],
            &message("<healthy2@x>", "More healthy mail", ""),
        );
        let provider = ImapProvider {
            cache: BodyCache::new(store.clone()),
            thread_state_loads: std::sync::atomic::AtomicUsize::new(0),
            ..provider
        };
        sync_n(&provider, store.database().as_ref(), 3).await;
        assert_eq!(
            store.locations_in_mailbox("Z-healthy").unwrap().len(),
            2,
            "persisted attempt ordering keeps servicing healthy folders"
        );
        assert_eq!(store.complete_walks().unwrap(), 0);
    }

    #[tokio::test]
    async fn expiry_journal_survives_restart_before_engine_consumption() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<expiry-replay@x>", "Gone", ""));
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox.clone());
        let db = store.database();
        sync_n(&provider, db.as_ref(), 1).await;
        let thread = store.thread_ids_in_mailbox("INBOX").unwrap().pop().unwrap();
        mailbox.lock().unwrap().inbox().messages.clear();
        sync_n(&provider, db.as_ref(), 1).await; // engine first observes emptiness
        let cursor = SyncCursor::new(db.cursor("me@example.com").unwrap().unwrap());
        // Commit expiry and its acknowledgment without letting the engine
        // consume the journal or advance its cursor.
        let mut pending = provider.poll(&cursor).await.unwrap();
        for _ in 0..policy::EMPTIED_THREAD_GRACE_WALKS {
            pending = provider.poll(&pending.cursor).await.unwrap();
        }
        assert!(store
            .journal_since(cursor.generation().unwrap())
            .unwrap()
            .contains(&thread));
        assert_eq!(db.list_all_mail(Some("me@example.com")).unwrap().len(), 1);
        assert!(matches!(
            provider.fetch_thread(&thread).await,
            Err(ProviderError::NotFound)
        ));
        let generation = store.generation().unwrap();
        let provider = ImapProvider {
            cache: BodyCache::new(store.clone()),
            thread_state_loads: std::sync::atomic::AtomicUsize::new(0),
            ..provider
        };
        sync_n(&provider, db.as_ref(), 1).await;
        assert!(db.list_all_mail(Some("me@example.com")).unwrap().is_empty());
        sync_n(&provider, db.as_ref(), 2).await;
        assert_eq!(store.generation().unwrap(), generation);
    }

    // Compiles always; runs only when THREESTRANDS_IMAP_IT=1 AND the Dovecot
    // container's fingerprint is in DOVECOT_TEST_FP. Skips cleanly otherwise,
    // exactly like the Slice 4 live tests in fetch.rs. The orchestrator runs it
    // with docker up.
    mod live {
        use super::super::super::connection::{ConnectionConfig, ImapConnectionManager, TlsMode};
        use super::super::super::session::ImapSession;
        use super::super::super::tls::parse_sha256_fingerprint;
        use super::super::{BodyCache, ImapProvider, ImapProviderConfig, ImapStateStore};
        use crate::db::Database;
        use crate::provider::{LabelModel, MailSync};
        use crate::provider::MailMutate;
        use std::sync::Arc;

        const HOST: &str = "127.0.0.1";
        const PORT: u16 = 11143;
        const USER: &str = "test@threestrands.test";
        const PASSWORD: &str = "testpassword";

        fn gated() -> Option<ConnectionConfig> {
            if std::env::var("THREESTRANDS_IMAP_IT").ok().as_deref() != Some("1") {
                eprintln!("skipping: THREESTRANDS_IMAP_IT != 1");
                return None;
            }
            let fp = parse_sha256_fingerprint(&std::env::var("DOVECOT_TEST_FP").ok()?)
                .expect("DOVECOT_TEST_FP must be 64 hex digits");
            Some(ConnectionConfig {
                host: HOST.into(),
                port: PORT,
                tls_mode: TlsMode::StartTls,
                pinned_fingerprint: Some(fp),
            })
        }

        fn live_provider(config: ConnectionConfig) -> ImapProvider {
            let database = Arc::new(Database::open_memory());
            let store = ImapStateStore::new(database.clone(), USER);
            let cache = BodyCache::new(store.clone());
            ImapProvider::new(ImapProviderConfig {
                account_id: USER.into(),
                username: USER.into(),
                password: PASSWORD.into(),
                manager: ImapConnectionManager::new(config),
                store,
                cache,
                label_model: LabelModel::ImapLabelFolders,
                settings: super::test_settings(),
            })
        }

        /// Flags carry their wire spelling including the `\` prefix.
        fn mentions_seen(flags: &[String]) -> bool {
            flags.iter().any(|f| f.eq_ignore_ascii_case("\\Seen"))
        }

        #[tokio::test]
        async fn live_provider_syncs_inbox_keeps_seen_absent_and_is_stable_on_re_poll() {
            let Some(config) = gated() else { return };
            let provider = live_provider(config.clone());

            // baseline_cursor + list_inbox + fetch_thread return the seeded mail.
            let cursor = provider.baseline_cursor().await.expect("baseline");
            let threads = provider.list_inbox(None).await.expect("list_inbox");
            assert!(!threads.thread_ids.is_empty(), "the harness seeds INBOX");
            let mut any_message = false;
            for thread_id in &threads.thread_ids {
                let messages = provider.fetch_thread(thread_id).await.expect("fetch_thread");
                any_message |= !messages.is_empty();
            }
            assert!(any_message, "seeded threads have messages");

            // \Seen is still absent on the server after sync (BODY.PEEK + EXAMINE).
            let mut probe = ImapConnectionManager::new(config)
                .connect_leased(
                    super::super::super::connection::ConnectionRole::Command,
                    USER,
                    PASSWORD,
                )
                .await
                .expect("probe login")
                .0;
            probe.examine("INBOX").await.expect("examine");
            let uids = probe.uid_search("ALL").await.expect("search");
            let set = uids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
            let rows = super::super::super::fetch::fetch_flags_only(&mut probe, &set)
                .await
                .expect("flags");
            assert!(
                rows.iter().all(|(_, flags)| !mentions_seen(flags)),
                "sync must not set \\Seen"
            );
            let _ = probe.logout().await;

            // The baseline cursor sits right after the INBOX round, so the first
            // poll from it delivers what the folder rounds (Sent, here the
            // shared Bcc-to-self copy) journaled during the same walk. After
            // that, a poll from the returned cursor is stable: it reports no
            // changes and the generation does not move.
            let first = provider.poll(&cursor).await.expect("delivery poll");
            let gen_before = first.cursor.generation().unwrap();
            let batch = provider.poll(&first.cursor).await.expect("re-poll");
            assert!(batch.changed_threads.is_empty(), "a stable re-poll is empty");
            assert_eq!(batch.cursor.generation(), Some(gen_before), "no idle bump");
        }

        /// (l, live) After baseline + enough polls to finish Sent, the shared
        /// Message-ID (seeded in BOTH INBOX and Sent) resolves to ONE message
        /// labelled INBOX+SENT, `\Seen` is still absent on the INBOX copy, and
        /// the seeded Trash + Junk messages are index-synced locally but never
        /// reported as changed_threads.
        #[tokio::test]
        async fn live_provider_syncs_sent_trash_and_junk_as_designed() {
            let Some(config) = gated() else { return };
            let provider = live_provider(config.clone());

            // Baseline, then several polls so the Sent chunked backfill (and
            // the Trash/Junk index rounds) all complete. The seed is tiny, so a
            // handful of polls is plenty; cursor is carried forward each time.
            let mut cursor = provider.baseline_cursor().await.expect("baseline");
            for _ in 0..5 {
                let batch = provider.poll(&cursor).await.expect("poll");
                cursor = batch.cursor;
            }

            // The shared Bcc-to-self Message-ID is ONE message with INBOX+SENT.
            // Find it by its defining property: a stable id with locations in
            // BOTH INBOX and Sent (no need to re-derive the id by hand).
            let inbox_locs = provider.store.locations_in_mailbox("INBOX").expect("inbox");
            let mut shared_message_id: Option<String> = None;
            for location in &inbox_locs {
                let all = provider
                    .store
                    .locations_for_message(&location.message_id)
                    .expect("locations");
                let mailboxes: std::collections::BTreeSet<&str> =
                    all.iter().map(|l| l.mailbox.as_str()).collect();
                if mailboxes.contains("INBOX") && mailboxes.contains("Sent") {
                    shared_message_id = Some(location.message_id.clone());
                    break;
                }
            }
            let shared_message_id =
                shared_message_id.expect("the shared Bcc-to-self message is in INBOX and Sent");

            let thread = provider
                .store
                .thread_of_message(&shared_message_id)
                .expect("thread_of_message")
                .expect("a thread for the shared message");
            let messages = provider.fetch_thread(&thread).await.expect("fetch_thread");
            // Exactly one message id for the shared Message-ID, with both labels.
            let shared_messages: Vec<_> = messages
                .iter()
                .filter(|m| m.id == shared_message_id)
                .collect();
            assert_eq!(shared_messages.len(), 1, "one message, not two copies");
            let labels = &shared_messages[0].label_ids;
            assert!(labels.contains(&"INBOX".to_string()), "INBOX: {labels:?}");
            assert!(labels.contains(&"SENT".to_string()), "SENT: {labels:?}");

            // \Seen is still absent on the INBOX copy of the shared message.
            let mut probe = ImapConnectionManager::new(config)
                .connect_leased(
                    super::super::super::connection::ConnectionRole::Command,
                    USER,
                    PASSWORD,
                )
                .await
                .expect("probe login")
                .0;
            probe.examine("INBOX").await.expect("examine");
            let uids = probe.uid_search("ALL").await.expect("search");
            let set = uids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
            let rows = super::super::super::fetch::fetch_flags_only(&mut probe, &set)
                .await
                .expect("flags");
            assert!(
                rows.iter().all(|(_, flags)| !mentions_seen(flags)),
                "sync must not set \\Seen on the INBOX copy"
            );
            let _ = probe.logout().await;

            // Trash and Junk are index-synced locally (TRASH / SPAM labels) …
            let trash = provider.store.locations_in_mailbox("Trash").expect("trash");
            let junk = provider.store.locations_in_mailbox("Junk").expect("junk");
            assert!(!trash.is_empty(), "Trash is index-synced");
            assert!(!junk.is_empty(), "Junk is index-synced");
            for (mailbox, location, expected) in [
                ("Trash", &trash[0], "TRASH"),
                ("Junk", &junk[0], "SPAM"),
            ] {
                let t = provider
                    .store
                    .thread_of_message(&location.message_id)
                    .expect("thread")
                    .expect("a thread");
                let msgs = provider.fetch_thread(&t).await.expect("fetch_thread");
                assert!(
                    msgs.iter().any(|m| m.label_ids.contains(&expected.to_string())),
                    "{mailbox} message carries {expected}"
                );
                // … but never hot, so never reported to the engine.
                assert!(
                    !provider.store.is_thread_hot(&t).expect("hot?"),
                    "{mailbox}-only thread is not hot (never reported)"
                );
            }
        }

        /// (Slice 5b-2, live) After baseline, UID MOVE an INBOX message to
        /// Archive through the test's OWN session, poll, and assert the thread
        /// is still present (now without INBOX, since Archive carries no system
        /// location label), with NO `NotFound`, and that `\Seen` is untouched.
        /// Archive is synced in 5b-2 so the move's destination is swept and the
        /// thread never even enters the emptied-thread grace. The Dovecot
        /// harness seeds Archive with one message (see seed.sh); this adds one
        /// more via UID MOVE. Skips cleanly without docker.
        #[tokio::test]
        async fn live_provider_follows_an_inbox_to_archive_move_without_notfound_or_seen() {
            let Some(config) = gated() else { return };
            let provider = live_provider(config.clone());

            // Baseline + a few polls so INBOX and Archive are both synced.
            let mut cursor = provider.baseline_cursor().await.expect("baseline");
            for _ in 0..5 {
                cursor = provider.poll(&cursor).await.expect("poll").cursor;
            }

            // Pick an INBOX message to move, remembering its stable id so we can
            // follow it across the move.
            let inbox = provider.store.locations_in_mailbox("INBOX").expect("inbox");
            assert!(!inbox.is_empty(), "the harness seeds INBOX");
            let moved_uid = inbox[0].uid as u32;
            let moved_id = inbox[0].message_id.clone();

            // UID MOVE it to Archive through our own command session. SELECT
            // (read-write) is required for MOVE; this is the TEST's session, not
            // the provider's EXAMINE-only sync session, so the \Seen invariant
            // is about what SYNC does, not this move.
            let mut mover = ImapConnectionManager::new(config.clone())
                .connect_leased(
                    super::super::super::connection::ConnectionRole::Command,
                    USER,
                    PASSWORD,
                )
                .await
                .expect("mover login")
                .0;
            mover
                .run_command_capture_code("SELECT INBOX")
                .await
                .expect("select inbox");
            mover
                .run_command_capture_code(&format!("UID MOVE {moved_uid} Archive"))
                .await
                .expect("uid move to Archive");
            let _ = mover.logout().await;

            // Poll until the move is reflected (INBOX location gone, Archive
            // location present for the moved id).
            let mut followed = false;
            for _ in 0..6 {
                cursor = provider.poll(&cursor).await.expect("poll").cursor;
                let locs = provider.store.locations_for_message(&moved_id).expect("locations");
                let mailboxes: std::collections::BTreeSet<&str> =
                    locs.iter().map(|l| l.mailbox.as_str()).collect();
                if !mailboxes.contains("INBOX") && mailboxes.contains("Archive") {
                    followed = true;
                    break;
                }
            }
            assert!(followed, "the move from INBOX to Archive was followed");

            // The thread is still present and fetchable (NO NotFound), with
            // Archive's no-INBOX labels.
            let thread = provider
                .store
                .thread_of_message(&moved_id)
                .expect("thread_of_message")
                .expect("a thread for the moved message");
            let messages = provider.fetch_thread(&thread).await.expect("fetch_thread not NotFound");
            assert!(!messages.is_empty(), "the moved thread still has a message");
            let labels: Vec<String> =
                messages.iter().flat_map(|m| m.label_ids.clone()).collect();
            assert!(!labels.contains(&"INBOX".to_string()), "no INBOX after archiving: {labels:?}");

            // \Seen is still absent on the moved message's Archive copy (sync is
            // EXAMINE + BODY.PEEK, so it never set it).
            let mut probe = ImapConnectionManager::new(config)
                .connect_leased(
                    super::super::super::connection::ConnectionRole::Command,
                    USER,
                    PASSWORD,
                )
                .await
                .expect("probe login")
                .0;
            probe.examine("Archive").await.expect("examine archive");
            let uids = probe.uid_search("ALL").await.expect("search");
            let set = uids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
            let rows = super::super::super::fetch::fetch_flags_only(&mut probe, &set)
                .await
                .expect("flags");
            assert!(
                rows.iter().all(|(_, flags)| !mentions_seen(flags)),
                "sync must not set \\Seen on the archived copy"
            );
            let _ = probe.logout().await;
        }

        /// (Slice 5b-2 run B, live) With `label_storage = Folders` and
        /// `label_container = Labels` (super::test_settings), after baseline +
        /// a few polls the labelled+foldered message (seeded in INBOX, the user
        /// folder Folders/Projects, and the label folder Labels/Clients)
        /// carries `INBOX` + `lf:Clients` + `folder:Folders/Projects`; the
        /// old message that exists ONLY in the user folder is index-synced but
        /// never hot (never reported); and `list_labels` contains both dynamic
        /// labels. Skips cleanly without docker.
        #[tokio::test]
        async fn live_provider_reports_folder_and_lf_labels_and_lists_them() {
            let Some(config) = gated() else { return };
            let provider = live_provider(config);

            // Baseline + a few polls so INBOX, the user folder and the label
            // folder are all visited.
            let mut cursor = provider.baseline_cursor().await.expect("baseline");
            for _ in 0..6 {
                cursor = provider.poll(&cursor).await.expect("poll").cursor;
            }

            // The labelled message is the one with a copy in all three of
            // INBOX, Folders/Projects and Labels/Clients. Find it by that
            // defining property.
            let inbox_locs = provider.store.locations_in_mailbox("INBOX").expect("inbox");
            let mut labelled: Option<String> = None;
            for location in &inbox_locs {
                let all = provider
                    .store
                    .locations_for_message(&location.message_id)
                    .expect("locations");
                let boxes: std::collections::BTreeSet<&str> =
                    all.iter().map(|l| l.mailbox.as_str()).collect();
                if boxes.contains("INBOX")
                    && boxes.contains("Folders/Projects")
                    && boxes.contains("Labels/Clients")
                {
                    labelled = Some(location.message_id.clone());
                    break;
                }
            }
            let labelled = labelled.expect("the labelled+foldered message is in all three places");

            let thread = provider
                .store
                .thread_of_message(&labelled)
                .expect("thread_of_message")
                .expect("a thread");
            let messages = provider.fetch_thread(&thread).await.expect("fetch_thread");
            let labels: Vec<String> =
                messages.iter().flat_map(|m| m.label_ids.clone()).collect();
            assert!(labels.contains(&"INBOX".to_string()), "INBOX: {labels:?}");
            assert!(labels.contains(&"lf:Clients".to_string()), "lf:Clients: {labels:?}");
            assert!(
                labels.contains(&"folder:Folders/Projects".to_string()),
                "folder:Folders/Projects: {labels:?}"
            );

            // The folder-only old message is index-synced (a location exists)
            // but its thread is NOT hot (it has no INBOX/Sent location), so it
            // is never reported to the engine.
            let folder_locs =
                provider.store.locations_in_mailbox("Folders/Projects").expect("folder");
            let folder_only = folder_locs
                .iter()
                .find(|l| {
                    let all = provider
                        .store
                        .locations_for_message(&l.message_id)
                        .expect("locations");
                    all.iter().all(|loc| loc.mailbox != "INBOX")
                })
                .expect("the folder-only message is index-synced");
            let t = provider
                .store
                .thread_of_message(&folder_only.message_id)
                .expect("thread")
                .expect("a thread");
            assert!(
                !provider.store.is_thread_hot(&t).expect("hot?"),
                "a folder-only thread is never hot (never reported)"
            );

            // list_labels contains the dynamic labels (read from the local plan).
            let listed = provider.list_labels().await.expect("list_labels");
            let ids: std::collections::BTreeSet<&str> =
                listed.iter().map(|l| l.id.as_str()).collect();
            assert!(ids.contains("lf:Clients"), "list_labels has lf:Clients: {ids:?}");
            assert!(
                ids.contains("folder:Folders/Projects"),
                "list_labels has folder:Folders/Projects: {ids:?}"
            );
            // Kinds are correct.
            let lf = listed.iter().find(|l| l.id == "lf:Clients").unwrap();
            assert_eq!(lf.kind, "user");
            let folder = listed.iter().find(|l| l.id == "folder:Folders/Projects").unwrap();
            assert_eq!(folder.kind, "folder");
        }

        /// (Phase 3 run 4, live) Create a scratch label, COPY a scratch INBOX
        /// message into it, sync and observe the `lf:` label, remove the label
        /// (expunging only that copy), then delete the scratch label. Operates
        /// ONLY on copies this test creates — the seeded fixtures are never
        /// moved, flagged or expunged. Panics loudly if cleanup cannot finish.
        /// Skips cleanly without docker.
        #[tokio::test]
        async fn live_provider_adds_removes_and_deletes_a_user_label() {
            let Some(config) = gated() else { return };
            let provider = live_provider(config.clone());

            // Baseline + a few polls so INBOX and Labels/Clients are synced.
            let mut cursor = provider.baseline_cursor().await.expect("baseline");
            for _ in 0..5 {
                cursor = provider.poll(&cursor).await.expect("poll").cursor;
            }

            // Create OUR OWN scratch copy inside INBOX (UID COPY of a seeded
            // INBOX message to a new UID in INBOX) so we never touch a fixture.
            let seeded = provider.store.locations_in_mailbox("INBOX").expect("inbox");
            assert!(!seeded.is_empty(), "the harness seeds INBOX");
            let source_uid = seeded[0].uid as u32;

            let mut worker = ImapConnectionManager::new(config.clone())
                .connect_leased(
                    super::super::super::connection::ConnectionRole::Command,
                    USER,
                    PASSWORD,
                )
                .await
                .expect("worker login")
                .0;
            // Make a scratch copy inside INBOX; read its new UID from the TYPED
            // COPYUID (the Debug string of the code is not parseable).
            use super::super::super::write::ImapWriteSession as _;
            worker.select("INBOX").await.expect("select inbox");
            let scratch_uid: u32 = worker
                .uid_copy(&source_uid.to_string(), "INBOX")
                .await
                .expect("scratch copy")
                .copyuid
                .expect("the harness advertises UIDPLUS, so COPYUID is returned")
                .dest
                .first()
                .copied()
                .expect("a scratch UID from COPYUID");

            // A scratch label name unlikely to collide with a fixture.
            let label_name = "ScratchP3R4";
            let label_mailbox = format!("Labels/{label_name}");

            // Guarantee cleanup even on assertion failure: a guard that removes
            // the scratch INBOX copy and deletes the scratch label mailbox.
            let cleanup = |config: super::super::super::connection::ConnectionConfig| async move {
                let mut c = ImapConnectionManager::new(config)
                    .connect_leased(
                        super::super::super::connection::ConnectionRole::Command,
                        USER,
                        PASSWORD,
                    )
                    .await
                    .expect("cleanup login")
                    .0;
                // Remove the scratch INBOX copy.
                c.run_command_capture_code("SELECT INBOX").await.expect("select inbox");
                let _ = c
                    .run_command_capture_code(&format!("UID STORE {scratch_uid} +FLAGS.SILENT (\\Deleted)"))
                    .await;
                let _ = c.run_command_capture_code(&format!("UID EXPUNGE {scratch_uid}")).await;
                // Delete the scratch label mailbox (after selecting INBOX).
                let _ = c.run_command_capture_code(&format!("DELETE \"{label_mailbox}\"")).await;
                let _ = c.logout().await;
            };

            // Create the label through the provider; add it to the scratch copy.
            let created = provider.create_label(label_name).await.expect("create_label");
            assert_eq!(created.id, format!("lf:{label_name}"));

            // Resync so the provider learns the scratch INBOX copy's id/thread.
            for _ in 0..6 {
                cursor = provider.poll(&cursor).await.expect("poll").cursor;
            }
            // Find the stable id of the scratch INBOX copy by its UID.
            let scratch_id = provider
                .store
                .locations_in_mailbox("INBOX")
                .expect("inbox")
                .into_iter()
                .find(|l| l.uid as u32 == scratch_uid)
                .map(|l| l.message_id);
            let scratch_id = match scratch_id {
                Some(id) => id,
                None => {
                    cleanup(config.clone()).await;
                    panic!("the scratch INBOX copy was not synced");
                }
            };

            // Add the label, sync, and observe lf:<name> on the message.
            if let Err(error) = provider.modify_messages(&[scratch_id.clone()], &[created.id.clone()], &[]).await {
                cleanup(config.clone()).await;
                panic!("add label failed: {error:?}");
            }
            for _ in 0..6 {
                cursor = provider.poll(&cursor).await.expect("poll").cursor;
            }
            let thread = provider
                .store
                .thread_of_message(&scratch_id)
                .expect("thread")
                .expect("a thread");
            let messages = provider.fetch_thread(&thread).await.expect("fetch_thread");
            let labelled = messages.iter().any(|m| m.label_ids.contains(&created.id));
            if !labelled {
                cleanup(config.clone()).await;
                panic!("the lf: label was not observed after adding it");
            }

            // Remove the label (expunges only the label copy), then delete the
            // now-empty scratch label.
            if let Err(error) = provider.modify_messages(&[scratch_id.clone()], &[], &[created.id.clone()]).await {
                cleanup(config.clone()).await;
                panic!("remove label failed: {error:?}");
            }
            if let Err(error) = provider.delete_label(&created.id).await {
                cleanup(config.clone()).await;
                panic!("delete_label failed: {error:?}");
            }

            // Clean up the scratch INBOX copy (the label mailbox is already
            // deleted by delete_label). Panic loudly if cleanup fails.
            let mut c = ImapConnectionManager::new(config.clone())
                .connect_leased(
                    super::super::super::connection::ConnectionRole::Command,
                    USER,
                    PASSWORD,
                )
                .await
                .expect("final cleanup login")
                .0;
            c.run_command_capture_code("SELECT INBOX").await.expect("select inbox");
            c.run_command_capture_code(&format!("UID STORE {scratch_uid} +FLAGS.SILENT (\\Deleted)"))
                .await
                .expect("flag scratch copy deleted");
            c.run_command_capture_code(&format!("UID EXPUNGE {scratch_uid}"))
                .await
                .expect("expunge scratch copy — cleanup must succeed");
            let _ = c.logout().await;
            let _ = worker.logout().await;
        }
    }

    // ======================================================================
    // Phase 5 run 3: acquire-by-UID + search pins.
    // ======================================================================
    //
    // These drive `acquire_uids` (the explicit-UID ingest run 4 will call) and
    // assert that a search-acquired OLD hit survives the delta's window
    // eviction and the emptied-thread grace, is removed when the server
    // expunges it or its UIDVALIDITY resets, pins idempotently, rolls back a
    // mid-acquire failure, and honours the cap and TTL. The provider is built
    // with a TINY `folder_window_override` so an acquired old UID is genuinely
    // out of window and WOULD be evicted without a pin.
    mod search_pins {
        use super::*;

        /// A provider whose non-INBOX window ceiling is `window`, so an
        /// acquired UID older than the newest-`window` is out of window and
        /// would be evicted on the next sweep without a pin.
        fn provider_window(
            mailbox: Arc<Mutex<FakeMailbox>>,
            window: usize,
        ) -> (ImapProvider, ImapStateStore) {
            let (mut provider, store) = provider_with(mailbox);
            provider.folder_window_override = Some(window);
            (provider, store)
        }

        /// Open a bare scripted session over the CURRENT shared mailbox so a
        /// test can call `acquire_uids` directly, the way run 4 will from
        /// inside a sync round.
        async fn session_for(mailbox: &Arc<Mutex<FakeMailbox>>) -> Box<dyn ImapSession> {
            let snapshot = mailbox.lock().unwrap().clone();
            open_scripted_session(snapshot).await
        }

        /// The plan entry for `mailbox` (role resolved from the catalog), or an
        /// INBOX entry when asked for INBOX.
        fn entry_for(provider: &ImapProvider, mailbox: &str) -> plan::PlanEntry {
            let plan = provider.build_sync_plan().expect("plan");
            plan.entries
                .iter()
                .find(|e| e.mailbox == mailbox)
                .cloned()
                .unwrap_or_else(|| panic!("no plan entry for {mailbox}"))
        }

        /// Thread ids currently listed from the local store for a mailbox (the
        /// DB thread list, not the location table).
        fn threads_in(store: &ImapStateStore, mailbox: &str) -> Vec<String> {
            store.thread_ids_in_mailbox(mailbox).unwrap()
        }

        /// (required) An acquired OLD UID in an EVICTING mailbox (Archive)
        /// survives: a full sweep, a window shift from new mail, a cadence-gated
        /// no-op poll, and a flag-only change (whose new flags are picked up).
        /// Its thread stays listed via the DB thread list after each sync.
        #[tokio::test]
        async fn an_acquired_old_uid_survives_sweeps_window_shifts_and_flag_changes() {
            use std::sync::atomic::{AtomicI64, Ordering};
            // A mutable clock so step 4 can advance past the folder-sweep
            // interval and let the periodic sweep refresh the pinned message's
            // flags — a flag-only change moves no counter, so (as for ANY
            // folder message) it is the periodic sweep, not an immediate poll,
            // that catches it.
            static CLOCK: AtomicI64 = AtomicI64::new(1_700_000_000);
            fn clock() -> i64 {
                CLOCK.load(Ordering::Relaxed)
            }

            // Archive with 3 messages; window 1, so only the newest (uid 3) is
            // in-window. We acquire the OLD uid 1 and pin it.
            let mut mb = FakeMailbox::new(100);
            mb.add_to("Archive", 50, &["\\Seen"], &message("<a1@x>", "Old one", ""));
            mb.add_to("Archive", 50, &["\\Seen"], &message("<a2@x>", "Old two", ""));
            mb.add_to("Archive", 50, &["\\Seen"], &message("<a3@x>", "New three", ""));
            let mailbox = Arc::new(Mutex::new(mb));
            let (mut provider, store) = provider_window(mailbox.clone(), 1);
            provider.now = clock;
            CLOCK.store(1_700_000_000, Ordering::Relaxed);
            let db = store.database();

            // Acquire the OLD uid 1 (out of the window-1).
            let archive = entry_for(&provider, "Archive");
            let mut session = session_for(&mailbox).await;
            let outcome = provider
                .acquire_uids(session.as_mut(), &archive, &[1], 50)
                .await
                .expect("acquire");
            let _ = session.logout().await;
            assert_eq!(outcome.skipped_over_cap, 0);
            assert_eq!(outcome.affected_threads.len(), 1, "one thread acquired");
            let acquired_thread = outcome.affected_threads[0].clone();

            // The acquired location + pin exist.
            assert_eq!(store.locations_in_mailbox("Archive").unwrap().len(), 1);
            assert_eq!(store.pinned_uids("Archive", 50).unwrap(), vec![1]);
            assert!(threads_in(&store, "Archive").contains(&acquired_thread));

            // 1) A full sweep (normal sync) — uid 1 is out of window but pinned,
            // so it is NOT evicted. (The window-1 sweep also acquires the newest
            // in-window UID, so the count grows; assert uid 1 specifically.)
            crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
                .await
                .unwrap();
            assert!(
                store
                    .locations_in_mailbox("Archive")
                    .unwrap()
                    .iter()
                    .any(|l| l.uid == 1),
                "pinned old UID survives a full sweep"
            );
            assert!(threads_in(&store, "Archive").contains(&acquired_thread));

            // 2) New mail arrives (window shifts): add uid 4. The window-1 now
            // covers only uid 4; uid 1 is still pinned and survives.
            mailbox
                .lock()
                .unwrap()
                .add_to("Archive", 50, &["\\Seen"], &message("<a4@x>", "Newest", ""));
            crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
                .await
                .unwrap();
            assert!(
                store
                    .locations_in_mailbox("Archive")
                    .unwrap()
                    .iter()
                    .any(|l| l.uid == 1),
                "pinned old UID survives a window shift"
            );

            // 3) A cadence-gated no-op poll (nothing changed): uid 1 still there.
            crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
                .await
                .unwrap();
            assert!(
                store
                    .locations_in_mailbox("Archive")
                    .unwrap()
                    .iter()
                    .any(|l| l.uid == 1),
                "pinned old UID survives an idle poll"
            );

            // 4) A flag-only change on the pinned message is picked up by the
            // periodic sweep. Advance the clock past the sweep interval so the
            // sweep runs (a flag-only change bumps no counter, exactly like any
            // other folder message), and the pinned UID's flags are refreshed
            // because the sweep's flag coverage includes live pinned UIDs.
            {
                let mut mb = mailbox.lock().unwrap();
                let folder = mb.folder("Archive", 50);
                let m = folder.messages.iter_mut().find(|m| m.uid == 1).unwrap();
                m.flags = vec!["\\Seen".into(), "\\Flagged".into()];
            }
            CLOCK.store(
                1_700_000_000 + super::super::policy::FOLDER_SWEEP_INTERVAL_SECS + 1,
                Ordering::Relaxed,
            );
            crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
                .await
                .unwrap();
            let flags: Vec<String> = {
                let loc = store.locations_in_mailbox("Archive").unwrap();
                let row = loc.iter().find(|l| l.uid == 1).unwrap();
                serde_json::from_str(&row.flags_json).unwrap()
            };
            assert!(
                flags.iter().any(|f| f == "\\Flagged"),
                "a flag-only change on the pinned message is picked up: {flags:?}"
            );
            // Still listed after all those syncs.
            assert!(threads_in(&store, "Archive").contains(&acquired_thread));
            assert!(
                provider.fetch_thread(&acquired_thread).await.is_ok(),
                "the acquired thread is still fetchable, not NotFound"
            );
        }

        /// (required) An acquired pinned UID is removed when the server expunges
        /// it (the pin never keeps a UID the server dropped), and its pin is
        /// released.
        #[tokio::test]
        async fn a_pinned_uid_is_removed_when_the_server_expunges_it() {
            let mut mb = FakeMailbox::new(100);
            mb.add_to("Archive", 50, &["\\Seen"], &message("<a1@x>", "Old", ""));
            mb.add_to("Archive", 50, &["\\Seen"], &message("<a2@x>", "New", ""));
            let mailbox = Arc::new(Mutex::new(mb));
            let (provider, store) = provider_window(mailbox.clone(), 1);
            let db = store.database();

            let archive = entry_for(&provider, "Archive");
            let mut session = session_for(&mailbox).await;
            provider
                .acquire_uids(session.as_mut(), &archive, &[1], 50)
                .await
                .expect("acquire");
            let _ = session.logout().await;
            assert_eq!(store.pinned_uids("Archive", 50).unwrap(), vec![1]);

            // The server expunges uid 1.
            mailbox
                .lock()
                .unwrap()
                .folder("Archive", 50)
                .messages
                .retain(|m| m.uid != 1);
            crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
                .await
                .unwrap();

            assert!(
                !store
                    .locations_in_mailbox("Archive")
                    .unwrap()
                    .iter()
                    .any(|l| l.uid == 1),
                "an expunged pinned UID is deleted"
            );
            assert!(
                store.pinned_uids("Archive", 50).unwrap().is_empty(),
                "its pin is released"
            );
        }

        /// (required) Pins drop on a UIDVALIDITY reset (with the locations).
        #[tokio::test]
        async fn pins_drop_on_a_uidvalidity_reset() {
            let mut mb = FakeMailbox::new(100);
            mb.add_to("Archive", 50, &["\\Seen"], &message("<a1@x>", "Old", ""));
            mb.add_to("Archive", 50, &["\\Seen"], &message("<a2@x>", "New", ""));
            let mailbox = Arc::new(Mutex::new(mb));
            let (provider, store) = provider_window(mailbox.clone(), 1);
            let db = store.database();

            let archive = entry_for(&provider, "Archive");
            let mut session = session_for(&mailbox).await;
            provider
                .acquire_uids(session.as_mut(), &archive, &[1], 50)
                .await
                .expect("acquire");
            let _ = session.logout().await;
            assert_eq!(store.pinned_uids("Archive", 50).unwrap(), vec![1]);

            // The server rebuilds Archive with a NEW UIDVALIDITY.
            {
                let mut mb = mailbox.lock().unwrap();
                let folder = mb.folder("Archive", 50);
                folder.uidvalidity = 77;
                folder.uidnext = 1;
                folder.messages.clear();
                mb.add_to("Archive", 77, &["\\Seen"], &message("<b1@x>", "Fresh", ""));
            }
            crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
                .await
                .unwrap();

            assert!(
                store.pinned_uids("Archive", 50).unwrap().is_empty(),
                "the old incarnation's pins are dropped"
            );
            assert!(
                store.pinned_uids("Archive", 77).unwrap().is_empty(),
                "no pins carried into the new incarnation"
            );
        }

        /// (required) Acquiring an already-local UID creates no duplicate
        /// message/location and pins it; acquiring twice is idempotent.
        #[tokio::test]
        async fn acquiring_an_already_local_uid_is_idempotent_and_pins_it() {
            // uid 1 is IN the window, so the normal sync already acquired it.
            let mut mb = FakeMailbox::new(100);
            mb.add_to("Archive", 50, &["\\Seen"], &message("<a1@x>", "Known", ""));
            let mailbox = Arc::new(Mutex::new(mb));
            let (provider, store) = provider_window(mailbox.clone(), 5);
            let db = store.database();

            // Normal sync ingests uid 1 (in window).
            crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
                .await
                .unwrap();
            assert_eq!(store.locations_in_mailbox("Archive").unwrap().len(), 1);
            let before_locations = store.locations_in_mailbox("Archive").unwrap().len();

            // Acquire the SAME uid 1 twice.
            let archive = entry_for(&provider, "Archive");
            for _ in 0..2 {
                let mut session = session_for(&mailbox).await;
                let outcome = provider
                    .acquire_uids(session.as_mut(), &archive, &[1], 50)
                    .await
                    .expect("acquire");
                let _ = session.logout().await;
                assert_eq!(outcome.skipped_over_cap, 0);
            }
            // No duplicate location/message, and it is pinned exactly once.
            assert_eq!(
                store.locations_in_mailbox("Archive").unwrap().len(),
                before_locations,
                "no duplicate location from acquiring an already-local UID"
            );
            assert_eq!(
                store.pinned_uids("Archive", 50).unwrap(),
                vec![1],
                "the already-local UID is pinned exactly once"
            );
        }

        /// (required) A failure injected mid-acquire (after the body fetch,
        /// before commit) leaves ZERO locations, threads or pins. The injection
        /// is asserted to have fired (the server dropped the connection).
        #[tokio::test]
        async fn a_failure_mid_acquire_leaves_no_locations_threads_or_pins() {
            let mut mb = FakeMailbox::new(100);
            mb.add_to("Archive", 50, &["\\Seen"], &message("<a1@x>", "Old", ""));
            mb.add_to("Archive", 50, &["\\Seen"], &message("<a2@x>", "New", ""));
            // The server DROPS the connection after the FIRST whole-body fetch,
            // so acquiring two UIDs fails mid-way (after a fetch, before the
            // round commits).
            mb.folder("Archive", 50).fail_after_body_fetches = Some(1);
            let mailbox = Arc::new(Mutex::new(mb));
            let (provider, store) = provider_window(mailbox.clone(), 1);

            let archive = entry_for(&provider, "Archive");
            let mut session = session_for(&mailbox).await;
            let result = provider
                .acquire_uids(session.as_mut(), &archive, &[1, 2], 50)
                .await;
            let _ = session.logout().await;

            // The injection fired: the acquire returned an error (the dropped
            // connection), proving we reached the mid-fetch failure point.
            assert!(
                result.is_err(),
                "the injected mid-acquire failure must surface as an error"
            );
            // Nothing durable was written: no locations, no threads, no pins.
            assert!(
                store.locations_in_mailbox("Archive").unwrap().is_empty(),
                "a failed acquire writes no locations"
            );
            assert!(
                threads_in(&store, "Archive").is_empty(),
                "a failed acquire writes no threads"
            );
            assert!(
                store.pinned_uids("Archive", 50).unwrap().is_empty(),
                "a failed acquire writes no pins"
            );
        }

        /// (required) A UIDVALIDITY mismatch at acquire time is refused with the
        /// typed error and ingests nothing.
        #[tokio::test]
        async fn acquire_refuses_a_uidvalidity_mismatch() {
            let mut mb = FakeMailbox::new(100);
            mb.add_to("Archive", 50, &["\\Seen"], &message("<a1@x>", "Old", ""));
            let mailbox = Arc::new(Mutex::new(mb));
            let (provider, store) = provider_window(mailbox.clone(), 1);

            let archive = entry_for(&provider, "Archive");
            let mut session = session_for(&mailbox).await;
            // We expect incarnation 999, but the server reports 50.
            let result = provider
                .acquire_uids(session.as_mut(), &archive, &[1], 999)
                .await;
            let _ = session.logout().await;
            match result {
                Err(AcquireError::UidValidityMismatch { expected, found, .. }) => {
                    assert_eq!((expected, found), (999, 50));
                }
                other => panic!("expected a UIDVALIDITY mismatch, got {other:?}"),
            }
            assert!(
                store.locations_in_mailbox("Archive").unwrap().is_empty(),
                "a refused acquire ingests nothing"
            );
            assert!(store.pinned_uids("Archive", 50).unwrap().is_empty());
        }

        /// Review fix: a mailbox outside the sync set would never refresh or
        /// release a pin, so acquiring into it is refused with no writes.
        #[tokio::test]
        async fn acquire_refuses_a_mailbox_outside_the_sync_set() {
            let mut mb = FakeMailbox::new(100);
            mb.add_to("Archive", 50, &["\\Seen"], &message("<a1@x>", "Old", ""));
            let mailbox = Arc::new(Mutex::new(mb));
            let (provider, store) = provider_window(mailbox.clone(), 1);

            let mut archive = entry_for(&provider, "Archive");
            archive.synced_now = false;
            let mut session = session_for(&mailbox).await;
            let result = provider
                .acquire_uids(session.as_mut(), &archive, &[1], 50)
                .await;
            let _ = session.logout().await;
            assert!(
                matches!(result, Err(AcquireError::MailboxNotSynced { .. })),
                "expected MailboxNotSynced, got {result:?}"
            );
            assert!(store.locations_in_mailbox("Archive").unwrap().is_empty());
            assert_eq!(store.search_pin_count().unwrap(), 0);
        }

        /// Review fix: a requested UID the server no longer has (expunged
        /// between the caller's SEARCH and this FETCH) gets no location, so it
        /// must get no pin either. The sibling UID that does exist is pinned.
        #[tokio::test]
        async fn acquire_pins_only_uids_that_ended_up_located() {
            let mut mb = FakeMailbox::new(100);
            mb.add_to("Archive", 50, &["\\Seen"], &message("<a1@x>", "Old", ""));
            mb.add_to("Archive", 50, &["\\Seen"], &message("<a2@x>", "New", ""));
            let mailbox = Arc::new(Mutex::new(mb));
            let (provider, store) = provider_window(mailbox.clone(), 1);

            let archive = entry_for(&provider, "Archive");
            let mut session = session_for(&mailbox).await;
            provider
                .acquire_uids(session.as_mut(), &archive, &[1, 99], 50)
                .await
                .expect("acquire");
            let _ = session.logout().await;
            let located: Vec<i64> = store
                .locations_in_mailbox("Archive")
                .unwrap()
                .iter()
                .map(|l| l.uid)
                .collect();
            assert_eq!(located, vec![1], "the vanished UID 99 has no location");
            assert_eq!(store.pinned_uids("Archive", 50).unwrap(), vec![1]);
            assert_eq!(store.search_pin_count().unwrap(), 1, "no orphan pin for 99");
        }

        /// Review fix: every path that removes a location removes its pin, so
        /// a local move/delete (`delete_location`) or a dropped mailbox
        /// (`drop_mailbox_locations`, also used on retirement-style resets)
        /// cannot leave a pin counting against the cap.
        #[tokio::test]
        async fn removing_a_pinned_location_by_any_path_drops_its_pin() {
            let mut mb = FakeMailbox::new(100);
            for uid in 1..=3u32 {
                mb.add_to("Archive", 50, &["\\Seen"], &message(&format!("<a{uid}@x>"), "m", ""));
            }
            let mailbox = Arc::new(Mutex::new(mb));
            let (provider, store) = provider_window(mailbox.clone(), 1);
            let archive = entry_for(&provider, "Archive");

            let mut session = session_for(&mailbox).await;
            provider
                .acquire_uids(session.as_mut(), &archive, &[1, 2, 3], 50)
                .await
                .expect("acquire");
            let _ = session.logout().await;
            assert_eq!(store.pinned_uids("Archive", 50).unwrap(), vec![1, 2, 3]);

            // A local write path removes one location.
            store.delete_location("Archive", 50, 1).unwrap();
            assert_eq!(store.pinned_uids("Archive", 50).unwrap(), vec![2, 3]);

            // Dropping the whole mailbox's locations drops the rest.
            store.drop_mailbox_locations("Archive").unwrap();
            assert!(store.pinned_uids("Archive", 50).unwrap().is_empty());
            assert_eq!(store.search_pin_count().unwrap(), 0);
        }

        /// (required) Cap: below / exact / above, with over-cap UIDs reported
        /// and existing pins untouched. Uses a tiny injected cap via seeding
        /// the pin table directly, so no 2,000-message fixture is needed.
        #[tokio::test]
        async fn acquire_honours_the_pin_cap_and_reports_over_cap_uids() {
            // Seed the account near the cap by pre-pinning UIDs in a DIFFERENT
            // mailbox so the per-account count is close to MAX_SEARCH_PINS,
            // then acquire into Archive and watch the cap bite.
            let mut mb = FakeMailbox::new(100);
            for uid in 1..=5u32 {
                mb.add_to("Archive", 50, &["\\Seen"], &message(&format!("<a{uid}@x>"), "m", ""));
            }
            let mailbox = Arc::new(Mutex::new(mb));
            let (provider, store) = provider_window(mailbox.clone(), 1);

            // Pre-pin (MAX_SEARCH_PINS - 2) pins in a sentinel mailbox so only
            // room for 2 new pins remains.
            let base = super::super::policy::MAX_SEARCH_PINS - 2;
            for uid in 0..base as u32 {
                store
                    .upsert_pin("OtherFolder", 1, uid as i64 + 1, 1_700_000_000)
                    .unwrap();
            }
            assert_eq!(store.search_pin_count().unwrap(), base);

            // Acquire 5 Archive UIDs; only room for 2 => 3 refused, and the
            // ACCEPTED ones are the NEWEST (uids 4 and 5).
            let archive = entry_for(&provider, "Archive");
            let mut session = session_for(&mailbox).await;
            let outcome = provider
                .acquire_uids(session.as_mut(), &archive, &[1, 2, 3, 4, 5], 50)
                .await
                .expect("acquire");
            let _ = session.logout().await;
            assert_eq!(outcome.skipped_over_cap, 3, "3 over-cap UIDs reported");
            // The accepted new pins are the newest two.
            assert_eq!(
                store.pinned_uids("Archive", 50).unwrap(),
                vec![4, 5],
                "newest-first acceptance under the cap"
            );
            // Existing pins in the sentinel mailbox are untouched.
            assert_eq!(store.search_pin_count().unwrap(), base + 2);
        }

        /// Exactly-at-cap acquires everything; below the cap acquires
        /// everything; above refuses the overflow (the boundary cases).
        #[tokio::test]
        async fn acquire_cap_below_exact_and_above() {
            let mut mb = FakeMailbox::new(100);
            for uid in 1..=3u32 {
                mb.add_to("Archive", 50, &["\\Seen"], &message(&format!("<a{uid}@x>"), "m", ""));
            }
            let mailbox = Arc::new(Mutex::new(mb));
            let (provider, store) = provider_window(mailbox.clone(), 1);
            let archive = entry_for(&provider, "Archive");

            // BELOW: room for many, acquire 2 => none refused.
            let mut session = session_for(&mailbox).await;
            let below = provider
                .acquire_uids(session.as_mut(), &archive, &[1, 2], 50)
                .await
                .expect("acquire");
            let _ = session.logout().await;
            assert_eq!(below.skipped_over_cap, 0);

            // Fill to EXACTLY one slot below the cap via sentinel pins, then
            // acquire 1 new => exact, none refused.
            let room_used = store.search_pin_count().unwrap();
            let fill = super::super::policy::MAX_SEARCH_PINS - room_used - 1;
            for i in 0..fill as u32 {
                store.upsert_pin("Sentinel", 1, i as i64 + 1, 1_700_000_000).unwrap();
            }
            let mut session = session_for(&mailbox).await;
            let exact = provider
                .acquire_uids(session.as_mut(), &archive, &[3], 50)
                .await
                .expect("acquire");
            let _ = session.logout().await;
            assert_eq!(exact.skipped_over_cap, 0, "exactly at the cap accepts the last pin");
            assert_eq!(store.search_pin_count().unwrap(), super::super::policy::MAX_SEARCH_PINS);

            // ABOVE: now full; a fresh new UID is refused.
            mailbox
                .lock()
                .unwrap()
                .add_to("Archive", 50, &["\\Seen"], &message("<a9@x>", "overflow", ""));
            let mut session = session_for(&mailbox).await;
            let above = provider
                .acquire_uids(session.as_mut(), &archive, &[4], 50)
                .await
                .expect("acquire");
            let _ = session.logout().await;
            assert_eq!(above.skipped_over_cap, 1, "over the cap refuses the overflow");
            assert!(
                !store.pinned_uids("Archive", 50).unwrap().contains(&4),
                "the over-cap UID is not pinned"
            );
        }

        /// (required) TTL: a pin just-before the TTL survives a sync; at/after
        /// the TTL it is released and (if out of window) its location follows
        /// normal window rules; re-acquiring refreshes `pinned_at`.
        #[tokio::test]
        async fn pin_ttl_releases_at_the_boundary_and_reacquire_refreshes_it() {
            use std::sync::atomic::{AtomicI64, Ordering};
            // A mutable clock so the test can advance `now` across the TTL.
            static CLOCK: AtomicI64 = AtomicI64::new(1_700_000_000);
            fn clock() -> i64 {
                CLOCK.load(Ordering::Relaxed)
            }

            let mut mb = FakeMailbox::new(100);
            mb.add_to("Archive", 50, &["\\Seen"], &message("<a1@x>", "Old", ""));
            mb.add_to("Archive", 50, &["\\Seen"], &message("<a2@x>", "New", ""));
            let mailbox = Arc::new(Mutex::new(mb));
            let (mut provider, store) = provider_window(mailbox.clone(), 1);
            provider.now = clock;
            let db = store.database();

            CLOCK.store(1_700_000_000, Ordering::Relaxed);
            // Acquire the OLD uid 1 at t0.
            let archive = entry_for(&provider, "Archive");
            let mut session = session_for(&mailbox).await;
            provider
                .acquire_uids(session.as_mut(), &archive, &[1], 50)
                .await
                .expect("acquire");
            let _ = session.logout().await;
            assert_eq!(store.pinned_uids("Archive", 50).unwrap(), vec![1]);

            // JUST BEFORE the TTL: sync keeps the pin and the location.
            CLOCK.store(1_700_000_000 + super::super::policy::SEARCH_PIN_TTL - 1, Ordering::Relaxed);
            crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
                .await
                .unwrap();
            assert_eq!(
                store.pinned_uids("Archive", 50).unwrap(),
                vec![1],
                "a pin one second short of the TTL survives"
            );
            assert!(store.locations_in_mailbox("Archive").unwrap().iter().any(|l| l.uid == 1));

            // AT the TTL: the pin is released and the (out-of-window) location
            // ages out under normal window rules.
            CLOCK.store(1_700_000_000 + super::super::policy::SEARCH_PIN_TTL, Ordering::Relaxed);
            crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
                .await
                .unwrap();
            assert!(
                store.pinned_uids("Archive", 50).unwrap().is_empty(),
                "a pin at the TTL boundary is released"
            );
            assert!(
                !store.locations_in_mailbox("Archive").unwrap().iter().any(|l| l.uid == 1),
                "after release the out-of-window location ages out"
            );

            // RE-ACQUIRE refreshes pinned_at: acquire again at a later time, and
            // a sync just-before the NEW TTL keeps it.
            let t2 = 1_700_000_000 + super::super::policy::SEARCH_PIN_TTL + 100;
            CLOCK.store(t2, Ordering::Relaxed);
            let archive = entry_for(&provider, "Archive");
            let mut session = session_for(&mailbox).await;
            provider
                .acquire_uids(session.as_mut(), &archive, &[1], 50)
                .await
                .expect("re-acquire");
            let _ = session.logout().await;
            assert_eq!(store.pinned_uids("Archive", 50).unwrap(), vec![1], "re-acquired");
            // Just before the refreshed TTL: still pinned.
            CLOCK.store(t2 + super::super::policy::SEARCH_PIN_TTL - 1, Ordering::Relaxed);
            crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
                .await
                .unwrap();
            assert_eq!(
                store.pinned_uids("Archive", 50).unwrap(),
                vec![1],
                "re-acquire refreshed pinned_at, so it survives relative to the new time"
            );
        }

        // ---- live Dovecot integration (opt-in; does not run without the
        // container). Marked exactly like the other live tests in this module:
        // gated on THREESTRANDS_IMAP_IT=1 + DOVECOT_TEST_FP, returns early
        // otherwise. It acquires an out-of-window UID and runs two syncs,
        // asserting the acquired hit survives. NOTE (reported): this did NOT
        // run here — Docker is unavailable in this environment, so it was
        // written and gated but not executed.
        mod live {
            use super::*;
            use crate::provider::imap::connection::{
                ConnectionConfig, ImapConnectionManager, TlsMode,
            };
            use crate::provider::imap::tls::parse_sha256_fingerprint;

            const HOST: &str = "127.0.0.1";
            const PORT: u16 = 11143;
            const USER: &str = "test@threestrands.test";
            const PASSWORD: &str = "testpassword";

            fn gated() -> Option<ConnectionConfig> {
                if std::env::var("THREESTRANDS_IMAP_IT").ok().as_deref() != Some("1") {
                    eprintln!("skipping: THREESTRANDS_IMAP_IT != 1");
                    return None;
                }
                let Ok(fp_str) = std::env::var("DOVECOT_TEST_FP") else {
                    eprintln!("skipping: DOVECOT_TEST_FP not set (Dovecot container absent)");
                    return None;
                };
                let fp = parse_sha256_fingerprint(&fp_str)
                    .expect("DOVECOT_TEST_FP must be a 64-hex-digit SHA-256 fingerprint");
                Some(ConnectionConfig {
                    host: HOST.into(),
                    port: PORT,
                    tls_mode: TlsMode::StartTls,
                    pinned_fingerprint: Some(fp),
                })
            }

            fn live_provider(config: ConnectionConfig) -> (ImapProvider, ImapStateStore) {
                let database = Arc::new(Database::open_memory());
                database
                    .adopt_mail_account(USER, crate::models::MailProviderKind::Imap)
                    .unwrap();
                let store = ImapStateStore::new(database.clone(), USER);
                let cache = BodyCache::new(store.clone());
                let provider = ImapProvider::new(ImapProviderConfig {
                    account_id: USER.into(),
                    username: USER.into(),
                    password: PASSWORD.into(),
                    manager: ImapConnectionManager::new(config),
                    store: store.clone(),
                    cache,
                    label_model: LabelModel::ImapLabelFolders,
                    settings: super::super::test_settings(),
                });
                (provider, store)
            }

            /// Acquire an out-of-window UID against the live Dovecot harness and
            /// run two syncs; the pinned hit must survive both. Gated — returns
            /// immediately without the container.
            #[tokio::test]
            async fn live_acquire_out_of_window_uid_survives_two_syncs() {
                let Some(config) = gated() else { return };
                let (mut provider, store) = live_provider(config);
                // Window of ONE in every mailbox, so the oldest UID of any
                // mailbox with two or more messages is genuinely out of window
                // and would be evicted by the next sweep without its pin.
                provider.limits.mailbox_window = 1;
                provider.folder_window_override = Some(1);
                let db = store.database();

                // Baseline so the catalog + INBOX are known.
                let _ = provider.baseline_cursor().await.expect("baseline");

                // INBOX: the harness seeds several messages there and it is an
                // EVICTING mailbox, so with a window of one its oldest UID is
                // out of window. (Archive holds a single seeded message.)
                let plan = provider.build_sync_plan().expect("plan");
                let target = plan
                    .entries
                    .iter()
                    .find(|e| e.is_inbox)
                    .cloned()
                    .expect("INBOX is always planned");

                // EXAMINE to learn the UIDVALIDITY and an existing UID.
                let mut session = provider.open_session().await.expect("session");
                let status = session.examine(&target.mailbox).await.expect("examine");
                let uidvalidity = status.uid_validity.unwrap_or(0) as i64;
                let all = session.uid_search("ALL").await.expect("search");
                assert!(
                    all.len() >= 2,
                    "{} needs two or more seeded messages for an out-of-window UID",
                    target.mailbox
                );
                let uid = *all.iter().min().unwrap();
                let outcome = provider
                    .acquire_uids(session.as_mut(), &target, &[uid], uidvalidity)
                    .await
                    .expect("acquire");
                let _ = session.logout().await;
                // Drop the session NOW: it owns one of the account's two
                // connection leases, and holding it makes the sync below wait
                // forever for a slot.
                drop(session);
                assert_eq!(outcome.skipped_over_cap, 0);
                assert!(store.is_pinned(&target.mailbox, uidvalidity, uid as i64).unwrap());

                // Two syncs: the pinned out-of-window hit must survive both,
                // as a LOCATION (not just a pin row).
                for _ in 0..2 {
                    crate::sync::sync_with(db.as_ref(), USER, &provider)
                        .await
                        .unwrap();
                    assert!(
                        store
                            .locations_in_mailbox(&target.mailbox)
                            .unwrap()
                            .iter()
                            .any(|l| l.uid == uid as i64),
                        "the acquired out-of-window location survives sync"
                    );
                    assert!(
                        store.is_pinned(&target.mailbox, uidvalidity, uid as i64).unwrap(),
                        "the acquired pin survives sync"
                    );
                }
            }
        }
    }
}

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
mod mutate_tests {
    use super::*;
    use crate::db::Database;
    use crate::provider::MailSync;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

    /// One message on the fake server: its UID, flags and raw bytes.
    #[derive(Clone)]
    struct MutMessage {
        uid: u32,
        flags: Vec<String>,
        raw: String,
    }

    /// One mailbox on the fake server.
    #[derive(Clone)]
    struct MutFolder {
        uidvalidity: u32,
        uidnext: u32,
        permanent_flags: Vec<String>,
        /// SELECT answers `NO` (an unselectable / vanished mailbox).
        select_fails: bool,
        /// SELECT answers a TRANSIENT `NO [INUSE]` (a busy mailbox the engine
        /// retries), used to exercise the partial-failure retry path.
        select_busy: bool,
        /// SELECT answers `OK [READ-ONLY]` (async-imap still treats this as a
        /// successful select).
        read_only: bool,
        messages: Vec<MutMessage>,
    }

    impl MutFolder {
        fn new(uidvalidity: u32) -> Self {
            Self {
                uidvalidity,
                uidnext: 1,
                permanent_flags: vec![
                    "\\Seen".into(),
                    "\\Flagged".into(),
                    "\\Deleted".into(),
                ],
                select_fails: false,
                select_busy: false,
                read_only: false,
                messages: Vec::new(),
            }
        }
        fn add(&mut self, flags: &[&str], raw: &str) -> u32 {
            let uid = self.uidnext;
            self.uidnext += 1;
            self.messages.push(MutMessage {
                uid,
                flags: flags.iter().map(|f| f.to_string()).collect(),
                raw: raw.to_string(),
            });
            uid
        }
    }

    /// The shared server state, mutated in place so a STORE on the live
    /// connection is visible to a later sweep.
    #[derive(Clone, Default)]
    struct MutServer {
        capabilities: Vec<String>,
        folders: BTreeMap<String, MutFolder>,
        /// Every write command the server saw (minus LOGIN/LOGOUT), so a test
        /// can assert what did — and did NOT — reach the wire.
        commands: Arc<Mutex<Vec<String>>>,
        /// When set, a STORE that would add/remove this flag is ACKed OK but
        /// NOT recorded, so the verify FETCH omits/keeps it — the silent
        /// ignore the verified store must catch.
        silently_ignore_flag: Option<String>,
        /// When set, `UID COPY <set> <this dest>` answers `NO [TRYCREATE]` the
        /// first time (and the server does NOT copy). Cleared automatically
        /// after it fires once, so a create-then-retry can succeed.
        copy_trycreate_once: Option<String>,
        /// When set, `UID EXPUNGE` fails with `NO <text>` the first time (and
        /// nothing is expunged); the \Deleted store that preceded it DID land.
        /// Cleared after it fires once so a retry can finish the move. Models
        /// the half-applied copy+delete+expunge move.
        fail_expunge_once: Option<String>,
        /// When set, `CREATE <this exact name>` answers `NO [TRYCREATE] <text>`
        /// the first time (and does NOT create), cleared after it fires once —
        /// so `create_label` creates the missing container and retries. Keyed
        /// by the decoded mailbox name.
        create_trycreate_once: Option<String>,
        drop_copy_response_once: bool,
        omit_copyuid: bool,
        ignore_expunge: bool,
    }

    impl MutServer {
        fn new() -> Self {
            Self {
                capabilities: vec![
                    "IMAP4REV1".into(),
                    "MOVE".into(),
                    "UIDPLUS".into(),
                ],
                folders: BTreeMap::new(),
                commands: Arc::new(Mutex::new(Vec::new())),
                silently_ignore_flag: None,
                copy_trycreate_once: None,
                fail_expunge_once: None,
                create_trycreate_once: None,
                drop_copy_response_once: false,
                omit_copyuid: false,
                ignore_expunge: false,
            }
        }
        fn folder(&mut self, name: &str, uidvalidity: u32) -> &mut MutFolder {
            self.folders
                .entry(name.to_string())
                .or_insert_with(|| MutFolder::new(uidvalidity))
        }
        fn flags_of(&self, mailbox: &str, uid: u32) -> Vec<String> {
            self.folders
                .get(mailbox)
                .and_then(|f| f.messages.iter().find(|m| m.uid == uid))
                .map(|m| m.flags.clone())
                .unwrap_or_default()
        }
        fn wire(&self) -> Vec<String> {
            self.commands.lock().unwrap().clone()
        }
        fn count_starting(&self, prefix: &str) -> usize {
            self.wire()
                .iter()
                .filter(|c| c.to_ascii_uppercase().starts_with(prefix))
                .count()
        }
    }

    /// Spawn the scripted mutation+read server over a duplex stream, sharing
    /// the live `Arc<Mutex<MutServer>>` so a STORE is visible to later sweeps.
    fn spawn(server_state: Arc<Mutex<MutServer>>) -> DuplexStream {
        let (client, server) = tokio::io::duplex(1 << 20);
        tokio::spawn(async move {
            let mut server = BufReader::new(server);
            server.get_mut().write_all(b"* OK ready\r\n").await.unwrap();
            let mut selected = String::new();
            loop {
                let mut line = String::new();
                if server.read_line(&mut line).await.unwrap() == 0 {
                    break;
                }
                let line = line.trim_end().to_string();
                let mut parts = line.splitn(2, ' ');
                let tag = parts.next().unwrap_or("").to_string();
                let rest = parts.next().unwrap_or("").to_string();
                let upper = rest.to_ascii_uppercase();

                // Build the whole response under the lock in a NON-async block,
                // drop the guard, then write it — a MutexGuard must never be
                // held across an await (the server task is Send).
                let (response, done) = {
                    let mut state = server_state.lock().unwrap();
                    if !upper.starts_with("LOGIN") && !upper.starts_with("LOGOUT") {
                        state.commands.lock().unwrap().push(rest.clone());
                    }
                    let response = build_response(&mut state, &mut selected, &tag, &rest, &upper);
                    if upper.starts_with("UID COPY") && state.drop_copy_response_once {
                        state.drop_copy_response_once = false;
                        (String::new(), true)
                    } else {
                        response
                    }
                };
                reply(&mut server, &response).await;
                if done {
                    break;
                }
            }
        });
        client
    }

    /// Build the server's response to one command (lock held, no await). Returns
    /// `(response_text, should_disconnect)`.
    fn build_response(
        state: &mut MutServer,
        selected: &mut String,
        tag: &str,
        rest: &str,
        upper: &str,
    ) -> (String, bool) {
        if upper.starts_with("LOGIN") {
            (format!("{tag} OK logged in\r\n"), false)
        } else if upper.starts_with("CAPABILITY") {
            let caps = state.capabilities.join(" ");
            (format!("* CAPABILITY {caps}\r\n{tag} OK done\r\n"), false)
        } else if upper.starts_with("SELECT") || upper.starts_with("EXAMINE") {
            let is_select = upper.starts_with("SELECT");
            let name = select_name(rest, is_select);
            *selected = name.clone();
            let Some(folder) = state.folders.get(&name) else {
                return (format!("{tag} NO [NONEXISTENT] no mailbox\r\n"), false);
            };
            if folder.select_fails {
                return (format!("{tag} NO [NONEXISTENT] select failed\r\n"), false);
            }
            if folder.select_busy {
                // A TRANSIENT failure the engine retries (RFC 5530 [INUSE]).
                return (format!("{tag} NO [INUSE] mailbox busy\r\n"), false);
            }
            let permanent = folder.permanent_flags.join(" ");
            let rw = if folder.read_only || !is_select {
                "[READ-ONLY]"
            } else {
                "[READ-WRITE]"
            };
            (
                format!(
                    "* {} EXISTS\r\n\
                     * OK [PERMANENTFLAGS ({permanent})] limited\r\n\
                     * OK [UIDVALIDITY {}] valid\r\n\
                     * OK [UIDNEXT {}] next\r\n\
                     {tag} OK {rw} done\r\n",
                    folder.messages.len(),
                    folder.uidvalidity,
                    folder.uidnext
                ),
                false,
            )
        } else if upper.starts_with("UID SEARCH") {
            let uids: Vec<String> = state
                .folders
                .get(selected.as_str())
                .map(|f| f.messages.iter().map(|m| m.uid.to_string()).collect())
                .unwrap_or_default();
            (format!("* SEARCH {}\r\n{tag} OK done\r\n", uids.join(" ")), false)
        } else if upper.starts_with("UID STORE") {
            let flags = store_flags(rest);
            let is_add = upper.contains("+FLAGS");
            let set = parse_uid_set(rest);
            let ignore = state.silently_ignore_flag.clone();
            if let Some(folder) = state.folders.get_mut(selected.as_str()) {
                for message in folder.messages.iter_mut() {
                    if !set.contains(&message.uid) {
                        continue;
                    }
                    for flag in &flags {
                        let ignored = ignore
                            .as_deref()
                            .map(|f| f.eq_ignore_ascii_case(flag))
                            .unwrap_or(false);
                        if ignored {
                            continue; // ACK OK but do not record.
                        }
                        if is_add {
                            if !message.flags.iter().any(|f| f.eq_ignore_ascii_case(flag)) {
                                message.flags.push(flag.clone());
                            }
                        } else {
                            message.flags.retain(|f| !f.eq_ignore_ascii_case(flag));
                        }
                    }
                }
            }
            (format!("{tag} OK STORE done\r\n"), false)
        } else if upper.starts_with("UID FETCH") {
            let set = parse_uid_set(rest);
            let wants_body = upper.contains("BODY.PEEK[]");
            let is_identity = upper.contains("HEADER.FIELDS");
            let empty = Vec::new();
            let messages = state
                .folders
                .get(selected.as_str())
                .map(|f| &f.messages)
                .unwrap_or(&empty);
            let mut out = String::new();
            for (seq, uid) in set.iter().enumerate() {
                if let Some(message) = messages.iter().find(|m| &m.uid == uid) {
                    out.push_str(&fetch_line(seq as u32 + 1, message, wants_body, is_identity));
                }
            }
            out.push_str(&format!("{tag} OK done\r\n"));
            (out, false)
        } else if upper.starts_with("CREATE") {
            // CREATE <mailbox>: create an empty folder (idempotent-ish — a
            // re-create of an existing one answers NO, which the caller never
            // triggers because create is reached only on TRYCREATE).
            let name = rest["CREATE".len().min(rest.len())..]
                .trim()
                .trim_matches('"')
                .to_string();
            // A scripted one-shot TRYCREATE (missing parent/container) fires
            // once for THIS exact name, so create_label can create the
            // container and retry.
            if state.create_trycreate_once.as_deref() == Some(name.as_str()) {
                state.create_trycreate_once = None;
                (format!("{tag} NO [TRYCREATE] missing parent\r\n"), false)
            } else if state.folders.contains_key(&name) {
                (format!("{tag} NO [ALREADYEXISTS] exists\r\n"), false)
            } else {
                // A freshly-created destination gets its own UIDVALIDITY.
                state.folders.insert(name, MutFolder::new(700));
                (format!("{tag} OK CREATE done\r\n"), false)
            }
        } else if upper.starts_with("DELETE") {
            // DELETE <mailbox>: remove the folder. Deleting an absent one
            // answers NO [NONEXISTENT]; the provider never issues that because
            // it checks the local catalog first.
            let name = rest["DELETE".len().min(rest.len())..]
                .trim()
                .trim_matches('"')
                .to_string();
            if state.folders.remove(&name).is_some() {
                (format!("{tag} OK DELETE done\r\n"), false)
            } else {
                (format!("{tag} NO [NONEXISTENT] no such mailbox\r\n"), false)
            }
        } else if upper.starts_with("RENAME") {
            // RENAME <from> <to>: move the folder (and its messages) to a new
            // name. Absent source -> NO; existing target -> NO. The provider
            // never issues RENAME (run 4 refuses rename), so this exists only
            // for completeness / future use.
            let args: Vec<&str> = rest["RENAME".len().min(rest.len())..].trim().split('"').collect();
            // Tokens are "from" "to"; split on quotes yields ["", from, " ", to, ""].
            let from = args.get(1).unwrap_or(&"").to_string();
            let to = args.get(3).unwrap_or(&"").to_string();
            match state.folders.remove(&from) {
                Some(folder) if !state.folders.contains_key(&to) => {
                    state.folders.insert(to, folder);
                    (format!("{tag} OK RENAME done\r\n"), false)
                }
                Some(folder) => {
                    state.folders.insert(from, folder);
                    (format!("{tag} NO [ALREADYEXISTS] target exists\r\n"), false)
                }
                None => (format!("{tag} NO [NONEXISTENT] no such mailbox\r\n"), false),
            }
        } else if upper.starts_with("UID COPY") {
            // UID COPY <set> <dest>: copy the named source UIDs (from the
            // selected mailbox) into <dest>, assigning new destination UIDs and
            // returning a tagged COPYUID (UIDPLUS). A scripted TRYCREATE fires
            // once if armed for this destination.
            let set = parse_uid_set(rest);
            let dest = copy_dest_name(rest);
            if let Some(text) = state.copy_trycreate_once.take() {
                return (format!("{tag} NO [TRYCREATE] {text}\r\n"), false);
            }
            if !state.folders.contains_key(&dest) {
                return (format!("{tag} NO [TRYCREATE] no such mailbox\r\n"), false);
            }
            let source_msgs: Vec<MutMessage> = state
                .folders
                .get(selected.as_str())
                .map(|f| f.messages.iter().filter(|m| set.contains(&m.uid)).cloned().collect())
                .unwrap_or_default();
            let dest_uidvalidity = state.folders.get(&dest).map(|f| f.uidvalidity).unwrap_or(0);
            let mut src_uids = Vec::new();
            let mut dst_uids = Vec::new();
            {
                let folder = state.folders.get_mut(&dest).expect("dest exists");
                for msg in source_msgs {
                    let new_uid = folder.uidnext;
                    folder.uidnext += 1;
                    src_uids.push(msg.uid);
                    dst_uids.push(new_uid);
                    folder.messages.push(MutMessage {
                        uid: new_uid,
                        flags: msg.flags.clone(),
                        raw: msg.raw.clone(),
                    });
                }
            }
            let src = src_uids
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",");
            let dst = dst_uids
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",");
            let response = if state.omit_copyuid {
                format!("{tag} OK copied\r\n")
            } else {
                format!("{tag} OK [COPYUID {dest_uidvalidity} {src} {dst}] copied\r\n")
            };
            (response, false)
        } else if upper.starts_with("UID MOVE") {
            // UID MOVE <set> <dest>: atomic server-side move. Copies to dest
            // (new UIDs), removes from source, emits an UNTAGGED COPYUID then
            // EXPUNGE lines, then the tagged OK.
            let set = parse_uid_set(rest);
            let dest = copy_dest_name(rest);
            if !state.folders.contains_key(&dest) {
                return (format!("{tag} NO [TRYCREATE] no such mailbox\r\n"), false);
            }
            let moved: Vec<MutMessage> = state
                .folders
                .get(selected.as_str())
                .map(|f| f.messages.iter().filter(|m| set.contains(&m.uid)).cloned().collect())
                .unwrap_or_default();
            let dest_uidvalidity = state.folders.get(&dest).map(|f| f.uidvalidity).unwrap_or(0);
            let mut src_uids = Vec::new();
            let mut dst_uids = Vec::new();
            {
                let folder = state.folders.get_mut(&dest).expect("dest exists");
                for msg in &moved {
                    let new_uid = folder.uidnext;
                    folder.uidnext += 1;
                    src_uids.push(msg.uid);
                    dst_uids.push(new_uid);
                    folder.messages.push(MutMessage {
                        uid: new_uid,
                        flags: msg.flags.clone(),
                        raw: msg.raw.clone(),
                    });
                }
            }
            // Remove the moved UIDs from the source.
            if let Some(folder) = state.folders.get_mut(selected.as_str()) {
                folder.messages.retain(|m| !set.contains(&m.uid));
            }
            let src = src_uids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
            let dst = dst_uids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
            let expunges: String = set.iter().map(|_| "* 1 EXPUNGE\r\n".to_string()).collect();
            (
                format!(
                    "* OK [COPYUID {dest_uidvalidity} {src} {dst}] moved\r\n{expunges}{tag} OK MOVE done\r\n"
                ),
                false,
            )
        } else if upper.starts_with("UID EXPUNGE") {
            // UID EXPUNGE <set>: remove the named UIDs from the selected
            // mailbox (they must already be \Deleted — the client stores that
            // first). A scripted one-shot failure models the half-applied move.
            if let Some(text) = state.fail_expunge_once.take() {
                return (format!("{tag} NO {text}\r\n"), false);
            }
            if state.ignore_expunge {
                return (format!("{tag} OK EXPUNGE done\r\n"), false);
            }
            let set = parse_uid_set(rest);
            if let Some(folder) = state.folders.get_mut(selected.as_str()) {
                folder
                    .messages
                    .retain(|m| !(set.contains(&m.uid) && m.flags.iter().any(|f| f.eq_ignore_ascii_case("\\Deleted"))));
            }
            (format!("{tag} OK EXPUNGE done\r\n"), false)
        } else if upper.starts_with("LOGOUT") {
            (format!("* BYE\r\n{tag} OK logout\r\n"), true)
        } else {
            (format!("{tag} OK done\r\n"), false)
        }
    }

    async fn reply(server: &mut BufReader<DuplexStream>, text: &str) {
        server.get_mut().write_all(text.as_bytes()).await.unwrap();
    }

    /// The mailbox name after `SELECT`/`EXAMINE`, unquoted.
    fn select_name(rest: &str, is_select: bool) -> String {
        let verb_len = if is_select { "SELECT".len() } else { "EXAMINE".len() };
        rest[verb_len.min(rest.len())..].trim().trim_matches('"').to_string()
    }

    /// The parenthesised flag list of a STORE command.
    fn store_flags(cmd: &str) -> Vec<String> {
        if let (Some(open), Some(close)) = (cmd.find('('), cmd.rfind(')')) {
            cmd[open + 1..close].split_whitespace().map(str::to_string).collect()
        } else {
            Vec::new()
        }
    }

    /// The destination mailbox name of a `UID COPY <set> <dest>` /
    /// `UID MOVE <set> <dest>` command, unquoted. The destination is token 4
    /// (`UID`, `COPY`/`MOVE`, `<set>`, `<dest>`).
    fn copy_dest_name(cmd: &str) -> String {
        cmd.split_whitespace()
            .nth(3)
            .unwrap_or("")
            .trim_matches('"')
            .to_string()
    }

    /// The UID set token after `UID FETCH`/`UID STORE` (comma + range forms).
    fn parse_uid_set(cmd: &str) -> Vec<u32> {
        // "UID FETCH <set> ..." / "UID STORE <set> ...": the set is token 3.
        let token = cmd.split_whitespace().nth(2).unwrap_or("");
        let mut uids = Vec::new();
        for part in token.split(',') {
            if let Some((lo, hi)) = part.split_once(':') {
                if let (Ok(lo), Ok(hi)) = (lo.parse::<u32>(), hi.parse::<u32>()) {
                    uids.extend(lo..=hi);
                }
            } else if let Ok(uid) = part.parse::<u32>() {
                uids.push(uid);
            }
        }
        uids
    }

    fn fetch_line(seq: u32, message: &MutMessage, wants_body: bool, is_identity: bool) -> String {
        let flags = message.flags.join(" ");
        if wants_body {
            format!(
                "* {seq} FETCH (UID {} BODY[]<0> {{{}}}\r\n{})\r\n",
                message.uid,
                message.raw.len(),
                message.raw
            )
        } else if is_identity {
            let header_block = message
                .raw
                .split_once("\r\n\r\n")
                .map(|(h, _)| format!("{h}\r\n\r\n"))
                .unwrap_or_else(|| message.raw.clone());
            format!(
                "* {seq} FETCH (UID {} FLAGS ({flags}) RFC822.SIZE {} BODY[HEADER.FIELDS (MESSAGE-ID DATE FROM SUBJECT)] {{{}}}\r\n{})\r\n",
                message.uid,
                message.raw.len(),
                header_block.len(),
                header_block
            )
        } else {
            // FLAGS-only (verify + sweep).
            format!("* {seq} FETCH (UID {} FLAGS ({flags}))\r\n", message.uid)
        }
    }

    /// A session source that logs into a fresh connection against the SHARED
    /// server state each time it is opened.
    struct MutSource {
        server: Arc<Mutex<MutServer>>,
    }

    #[async_trait]
    impl ImapSessionSource for MutSource {
        async fn open(&self) -> ProviderResult<Box<dyn ImapSession>> {
            let client = spawn(self.server.clone());
            let session = async_imap::Client::new(client)
                .login("user", "password")
                .await
                .map_err(|(error, _)| super::super::error::map_imap_error(&error))?;
            Ok(Box::new(super::super::session::AsyncImapSession::new(session)))
        }
    }

    fn test_settings() -> ImapAccountSettings {
        use super::super::settings::SecurityMode;
        ImapAccountSettings {
            imap_host: "127.0.0.1".into(),
            imap_port: 1143,
            imap_security: SecurityMode::StartTls,
            imap_username: "me@example.com".into(),
            smtp_host: "127.0.0.1".into(),
            smtp_port: 1025,
            smtp_security: SecurityMode::StartTls,
            smtp_username: "me@example.com".into(),
            mailbox_overrides: Default::default(),
            archive_mailbox: None,
            label_storage: LabelStorage::Folders,
            label_container: Some("Labels".into()),
            identities: vec![],
            pinned_fingerprints: Default::default(),
            server_saves_sent: false,
        }
    }

    fn infer_special_use(name: &str) -> Option<String> {
        match name {
            "Trash" => Some("\\Trash".into()),
            "Junk" | "Spam" => Some("\\Junk".into()),
            "Sent" => Some("\\Sent".into()),
            "Archive" => Some("\\Archive".into()),
            _ => None,
        }
    }

    /// Build a provider over the shared server, seeding a catalog row for every
    /// folder present (so SELECT's UIDVALIDITY guard matches) and returning
    /// the provider + store.
    fn provider_with(server: Arc<Mutex<MutServer>>) -> (ImapProvider, ImapStateStore) {
        let database = Arc::new(Database::open_memory());
        database
            .adopt_mail_account("me@example.com", crate::models::MailProviderKind::Imap)
            .unwrap();
        let store = ImapStateStore::new(database.clone(), "me@example.com");
        for (name, folder) in &server.lock().unwrap().folders {
            store
                .upsert_mailbox(&super::super::ImapMailbox {
                    name: name.clone(),
                    delimiter: Some("/".into()),
                    special_use: infer_special_use(name),
                    uidvalidity: folder.uidvalidity as i64,
                    uidnext: folder.uidnext as i64,
                    highestmodseq: None,
                    permanent_flags_json: None,
                    permanent_keywords: None,
                })
                .unwrap();
        }
        let cache = BodyCache::new(store.clone());
        let provider = ImapProvider {
            store: store.clone(),
            cache,
            username: "me@example.com".into(),
            account_id: "me@example.com".into(),
            smtp_password: "test-password".into(),
            label_model: LabelModel::ImapLabelFolders,
            settings: test_settings(),
            source: Arc::new(MutSource { server }),
            now: || 1_700_000_000,
            limits: SyncLimits::inbox(),
            sent_round_budget: None,
            folder_window_override: None,
            extra_synced_candidates: None,
            force_user_folders: None,
            folder_rounds_per_poll_override: None,
            thread_state_loads: std::sync::atomic::AtomicUsize::new(0),
        };
        (provider, store)
    }

    fn message(message_id: &str) -> String {
        format!(
            "Message-ID: {message_id}\r\nDate: Mon, 06 Oct 2025 09:00:00 +0000\r\nFrom: a@b.com\r\nSubject: S\r\n\r\nbody",
        )
    }

    /// Seed one message as a location + thread + a cached body, so modify_* can
    /// resolve it. Returns its stable message id. `mailbox`'s UIDVALIDITY must
    /// match the server's.
    fn seed(
        store: &ImapStateStore,
        provider: &ImapProvider,
        mailbox: &str,
        uidvalidity: i64,
        uid: i64,
        msgid: &str,
        flags: &[&str],
    ) -> String {
        let message_id = format!("imap:me@example.com:{msgid}");
        store
            .upsert_location(&ImapLocation {
                mailbox: mailbox.into(),
                uidvalidity,
                uid,
                message_id: message_id.clone(),
                flags_json: serde_json::to_string(
                    &flags.iter().map(|f| f.to_string()).collect::<Vec<_>>(),
                )
                .unwrap(),
                modseq: None,
            })
            .unwrap();
        store
            .commit_sync_round(&SyncRoundWrite {
                thread_assignments: vec![(message_id.clone(), format!("imap:t:{msgid}"))],
                changed_threads: vec![format!("imap:t:{msgid}")],
                ..Default::default()
            })
            .unwrap();
        provider
            .cache
            .put(&message_id, message(&format!("<{msgid}@x>")).as_bytes(), 1)
            .unwrap();
        message_id
    }

    /// The stored flags for a location, as the local store holds them.
    fn local_flags(store: &ImapStateStore, mailbox: &str, uid: i64) -> Vec<String> {
        let location = store
            .locations_in_mailbox(mailbox)
            .unwrap()
            .into_iter()
            .find(|l| l.uid == uid)
            .expect("location present");
        serde_json::from_str(&location.flags_json).unwrap()
    }

    fn server_with_inbox() -> Arc<Mutex<MutServer>> {
        let mut s = MutServer::new();
        s.folder("INBOX", 100);
        Arc::new(Mutex::new(s))
    }

    // --- mark read / unread ------------------------------------------------

    #[tokio::test]
    async fn mark_read_adds_seen_and_updates_local_flags() {
        let server = server_with_inbox();
        {
            let mut g = server.lock().unwrap();
            g.folder("INBOX", 100).add(&[], &message("<m1@x>")); // unread (no \Seen)
        }
        let (provider, store) = provider_with(server.clone());
        let id = seed(&store, &provider, "INBOX", 100, 1, "m1", &[]);

        // remove UNREAD == mark read == add \Seen.
        provider
            .modify_messages(&[id], &[], &["UNREAD".into()])
            .await
            .expect("mark read");

        // The server now holds \Seen, and the local flags agree.
        assert!(server.lock().unwrap().flags_of("INBOX", 1).iter().any(|f| f == "\\Seen"));
        assert!(local_flags(&store, "INBOX", 1).iter().any(|f| f == "\\Seen"));
    }

    #[tokio::test]
    async fn mark_unread_removes_seen() {
        let server = server_with_inbox();
        {
            let mut g = server.lock().unwrap();
            g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
        }
        let (provider, store) = provider_with(server.clone());
        let id = seed(&store, &provider, "INBOX", 100, 1, "m1", &["\\Seen"]);

        // add UNREAD == mark unread == remove \Seen.
        provider
            .modify_messages(&[id], &["UNREAD".into()], &[])
            .await
            .expect("mark unread");

        assert!(!server.lock().unwrap().flags_of("INBOX", 1).iter().any(|f| f == "\\Seen"));
        assert!(!local_flags(&store, "INBOX", 1).iter().any(|f| f == "\\Seen"));
    }

    // --- star / unstar -----------------------------------------------------

    #[tokio::test]
    async fn star_adds_flagged_and_unstar_removes_it() {
        let server = server_with_inbox();
        {
            let mut g = server.lock().unwrap();
            g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
        }
        let (provider, store) = provider_with(server.clone());
        let id = seed(&store, &provider, "INBOX", 100, 1, "m1", &["\\Seen"]);

        provider.modify_messages(&[id.clone()], &["STARRED".into()], &[]).await.expect("star");
        assert!(server.lock().unwrap().flags_of("INBOX", 1).iter().any(|f| f == "\\Flagged"));
        assert!(local_flags(&store, "INBOX", 1).iter().any(|f| f == "\\Flagged"));

        provider.modify_messages(&[id], &[], &["STARRED".into()]).await.expect("unstar");
        assert!(!server.lock().unwrap().flags_of("INBOX", 1).iter().any(|f| f == "\\Flagged"));
        assert!(!local_flags(&store, "INBOX", 1).iter().any(|f| f == "\\Flagged"));
    }

    // --- thread-level mark-read across messages and two mailboxes ----------

    #[tokio::test]
    async fn thread_mark_read_touches_every_message_in_every_mailbox() {
        // Two messages of one thread; one lives in INBOX, the other has copies
        // in INBOX and Archive (two locations, both must agree).
        let mut s = MutServer::new();
        s.folder("INBOX", 100).add(&[], &message("<a@x>")); // uid 1
        s.folder("INBOX", 100).add(&[], &message("<b@x>")); // uid 2
        s.folder("Archive", 95).add(&[], &message("<b@x>")); // uid 1 (copy of b)
        let server = Arc::new(Mutex::new(s));
        let (provider, store) = provider_with(server.clone());

        // Seed both messages into ONE thread.
        store
            .upsert_location(&ImapLocation {
                mailbox: "INBOX".into(), uidvalidity: 100, uid: 1,
                message_id: "imap:me@example.com:a".into(), flags_json: "[]".into(), modseq: None,
            }).unwrap();
        store
            .upsert_location(&ImapLocation {
                mailbox: "INBOX".into(), uidvalidity: 100, uid: 2,
                message_id: "imap:me@example.com:b".into(), flags_json: "[]".into(), modseq: None,
            }).unwrap();
        store
            .upsert_location(&ImapLocation {
                mailbox: "Archive".into(), uidvalidity: 95, uid: 1,
                message_id: "imap:me@example.com:b".into(), flags_json: "[]".into(), modseq: None,
            }).unwrap();
        store
            .commit_sync_round(&SyncRoundWrite {
                thread_assignments: vec![
                    ("imap:me@example.com:a".into(), "imap:t:thread".into()),
                    ("imap:me@example.com:b".into(), "imap:t:thread".into()),
                ],
                changed_threads: vec!["imap:t:thread".into()],
                ..Default::default()
            }).unwrap();

        provider
            .modify_thread("imap:t:thread", &[], &["UNREAD".into()])
            .await
            .expect("thread mark read");

        // Every copy, in BOTH mailboxes, now has \Seen on the server and locally.
        let g = server.lock().unwrap();
        assert!(g.flags_of("INBOX", 1).iter().any(|f| f == "\\Seen"));
        assert!(g.flags_of("INBOX", 2).iter().any(|f| f == "\\Seen"));
        assert!(g.flags_of("Archive", 1).iter().any(|f| f == "\\Seen"));
        drop(g);
        assert!(local_flags(&store, "INBOX", 1).iter().any(|f| f == "\\Seen"));
        assert!(local_flags(&store, "INBOX", 2).iter().any(|f| f == "\\Seen"));
        assert!(local_flags(&store, "Archive", 1).iter().any(|f| f == "\\Seen"));
    }

    // --- alias thread id ---------------------------------------------------

    #[tokio::test]
    async fn modify_thread_resolves_an_alias_to_the_survivor() {
        let server = server_with_inbox();
        {
            let mut g = server.lock().unwrap();
            g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
        }
        let (provider, store) = provider_with(server.clone());
        store
            .upsert_location(&ImapLocation {
                mailbox: "INBOX".into(), uidvalidity: 100, uid: 1,
                message_id: "imap:me@example.com:m1".into(), flags_json: r#"["\\Seen"]"#.into(),
                modseq: None,
            }).unwrap();
        store
            .commit_sync_round(&SyncRoundWrite {
                thread_assignments: vec![("imap:me@example.com:m1".into(), "imap:t:survivor".into())],
                aliases: vec![("imap:t:old".into(), "imap:t:survivor".into())],
                changed_threads: vec!["imap:t:survivor".into()],
                ..Default::default()
            }).unwrap();

        // Star via the OLD (aliased) id: it resolves to the survivor's message.
        provider.modify_thread("imap:t:old", &["STARRED".into()], &[]).await.expect("star via alias");
        assert!(server.lock().unwrap().flags_of("INBOX", 1).iter().any(|f| f == "\\Flagged"));
    }

    // --- a thread keeps rows for messages expunged elsewhere ---------------

    #[tokio::test]
    async fn modify_thread_skips_messages_with_no_remaining_location() {
        let server = server_with_inbox();
        {
            let mut g = server.lock().unwrap();
            g.folder("INBOX", 100).add(&[], &message("<m1@x>"));
        }
        let (provider, store) = provider_with(server.clone());
        let live = "imap:me@example.com:m1".to_string();
        store
            .upsert_location(&ImapLocation {
                mailbox: "INBOX".into(), uidvalidity: 100, uid: 1,
                message_id: live.clone(), flags_json: "[]".into(), modseq: None,
            })
            .unwrap();
        // m2 is in the same thread but was expunged on the server: its
        // imap_threads row is permanent, its locations are gone.
        store
            .commit_sync_round(&SyncRoundWrite {
                thread_assignments: vec![
                    (live.clone(), "imap:t:shared".into()),
                    ("imap:me@example.com:m2".into(), "imap:t:shared".into()),
                ],
                changed_threads: vec!["imap:t:shared".into()],
                ..Default::default()
            })
            .unwrap();

        provider
            .modify_thread("imap:t:shared", &[], &["UNREAD".into()])
            .await
            .expect("an expunged sibling must not fail a thread-level mark-read");
        assert!(server.lock().unwrap().flags_of("INBOX", 1).iter().any(|f| f == "\\Seen"));

        // A thread whose messages ALL lost their locations is still refused.
        let err = provider
            .modify_thread("imap:t:gone", &[], &["UNREAD".into()])
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::InvalidOperation(_)), "{err:?}");
    }

    // --- idempotent no-op sends nothing ------------------------------------

    #[tokio::test]
    async fn an_already_satisfied_request_sends_no_write_command() {
        let server = server_with_inbox();
        {
            let mut g = server.lock().unwrap();
            g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
        }
        let (provider, store) = provider_with(server.clone());
        let id = seed(&store, &provider, "INBOX", 100, 1, "m1", &["\\Seen"]);

        // Mark-read on an already-read message: Ok(()) and ZERO commands sent
        // (not even a SELECT), because the local flags already satisfy it.
        provider.modify_messages(&[id], &[], &["UNREAD".into()]).await.expect("no-op ok");
        let g = server.lock().unwrap();
        assert_eq!(g.count_starting("SELECT"), 0, "an idempotent no-op selects nothing");
        assert_eq!(g.count_starting("UID STORE"), 0, "and sends no STORE");
    }

    // --- stale UIDVALIDITY sends nothing and is transient ------------------

    #[tokio::test]
    async fn a_stale_uidvalidity_sends_no_write_and_is_transient() {
        let server = server_with_inbox();
        {
            let mut g = server.lock().unwrap();
            // The server's INBOX is at UIDVALIDITY 200, but the stored location
            // says 100 -> the select_for_write guard refuses.
            g.folder("INBOX", 100).uidvalidity = 200;
            g.folder("INBOX", 100).add(&[], &message("<m1@x>"));
        }
        let (provider, store) = provider_with(server.clone());
        let id = seed(&store, &provider, "INBOX", 100, 1, "m1", &[]); // stored UIDVALIDITY 100

        let err = provider
            .modify_messages(&[id], &[], &["UNREAD".into()])
            .await
            .unwrap_err();
        assert!(
            matches!(err, ProviderError::TransientTransport(ref m) if m.contains("UIDVALIDITY")),
            "a stale mailbox is a named transient error: {err:?}"
        );
        assert!(err.retry_mutation(), "transient so the engine retries");
        // The guard SELECTed to learn the UIDVALIDITY but issued NO STORE.
        assert_eq!(server.lock().unwrap().count_starting("UID STORE"), 0);
    }

    // --- flag missing from PERMANENTFLAGS is a permanent error naming it ----

    #[tokio::test]
    async fn a_flag_outside_permanentflags_is_a_permanent_error_naming_it() {
        let server = server_with_inbox();
        {
            let mut g = server.lock().unwrap();
            // PERMANENTFLAGS without \Flagged and no \* -> \Flagged unstorable.
            g.folder("INBOX", 100).permanent_flags = vec!["\\Seen".into(), "\\Deleted".into()];
            g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
        }
        let (provider, store) = provider_with(server.clone());
        let id = seed(&store, &provider, "INBOX", 100, 1, "m1", &["\\Seen"]);

        let err = provider
            .modify_messages(&[id], &["STARRED".into()], &[])
            .await
            .unwrap_err();
        match err {
            ProviderError::InvalidOperation(m) => {
                assert!(m.contains("Flagged"), "names the flag: {m}");
                assert!(m.contains("INBOX"), "names the mailbox: {m}");
            }
            other => panic!("expected a permanent InvalidOperation, got {other:?}"),
        }
        // No \Flagged was stored.
        assert!(!server.lock().unwrap().flags_of("INBOX", 1).iter().any(|f| f == "\\Flagged"));
    }

    // --- a silently-ignored STORE is caught --------------------------------

    #[tokio::test]
    async fn a_silently_ignored_store_is_caught() {
        let server = server_with_inbox();
        {
            let mut g = server.lock().unwrap();
            g.silently_ignore_flag = Some("\\Flagged".into());
            g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
        }
        let (provider, store) = provider_with(server.clone());
        let id = seed(&store, &provider, "INBOX", 100, 1, "m1", &["\\Seen"]);

        let err = provider
            .modify_messages(&[id], &["STARRED".into()], &[])
            .await
            .unwrap_err();
        match err {
            ProviderError::InvalidOperation(m) => assert!(m.contains("silently ignored"), "{m}"),
            other => panic!("expected InvalidOperation, got {other:?}"),
        }
    }

    // --- unsupported / mixed request rejected with ZERO commands -----------

    #[tokio::test]
    async fn an_unsupported_or_mixed_request_is_rejected_before_any_command() {
        let server = server_with_inbox();
        {
            let mut g = server.lock().unwrap();
            g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
        }
        let (provider, store) = provider_with(server.clone());
        let id = seed(&store, &provider, "INBOX", 100, 1, "m1", &["\\Seen"]);

        // STARRED + INBOX (an unsupported location id) is rejected permanently.
        let err = provider
            .modify_messages(&[id.clone()], &["STARRED".into(), "INBOX".into()], &[])
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::InvalidOperation(_)), "{err:?}");

        // A label in BOTH lists is contradictory and rejected.
        let err = provider
            .modify_messages(&[id], &["STARRED".into()], &["STARRED".into()])
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::InvalidOperation(_)), "{err:?}");

        // Not one command reached the wire for either rejected request.
        let g = server.lock().unwrap();
        assert_eq!(g.count_starting("SELECT"), 0, "no SELECT for a rejected request");
        assert_eq!(g.count_starting("UID STORE"), 0, "no STORE for a rejected request");
    }

    // --- a message with no location at all is a permanent error naming it ---

    #[tokio::test]
    async fn a_message_with_no_location_is_a_permanent_error_naming_it() {
        let server = server_with_inbox();
        let (provider, store) = provider_with(server.clone());
        // A message id with a thread row but NO location (emptied / expunged).
        store
            .commit_sync_round(&SyncRoundWrite {
                thread_assignments: vec![("imap:me@example.com:gone".into(), "imap:t:gone".into())],
                changed_threads: vec!["imap:t:gone".into()],
                ..Default::default()
            }).unwrap();

        let err = provider
            .modify_messages(&["imap:me@example.com:gone".into()], &["STARRED".into()], &[])
            .await
            .unwrap_err();
        match err {
            ProviderError::InvalidOperation(m) => assert!(m.contains("gone"), "names the message: {m}"),
            other => panic!("expected a permanent InvalidOperation, got {other:?}"),
        }
        assert_eq!(server.lock().unwrap().count_starting("UID STORE"), 0);
    }

    // --- partial failure across two mailboxes, then retry touches only B ----

    #[tokio::test]
    async fn a_partial_failure_persists_the_succeeded_mailbox_and_a_retry_touches_only_the_failed() {
        // One message with copies in INBOX (processed first — mailboxes are
        // visited in name order — and succeeds) and Trash (SELECT transiently
        // fails first time). The INBOX copy's local flags are updated; the
        // mutation returns Trash's error; a retry re-derives only Trash.
        let mut s = MutServer::new();
        s.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>")); // uid 1
        s.folder("Trash", 95).add(&["\\Seen"], &message("<m1@x>")); // uid 1
        s.folder("Trash", 95).select_busy = true;
        let server = Arc::new(Mutex::new(s));
        let (provider, store) = provider_with(server.clone());

        for (mailbox, uidvalidity, uid) in [("INBOX", 100, 1), ("Trash", 95, 1)] {
            store
                .upsert_location(&ImapLocation {
                    mailbox: mailbox.into(), uidvalidity, uid,
                    message_id: "imap:me@example.com:m1".into(),
                    flags_json: r#"["\\Seen"]"#.into(), modseq: None,
                }).unwrap();
        }
        store
            .commit_sync_round(&SyncRoundWrite {
                thread_assignments: vec![("imap:me@example.com:m1".into(), "imap:t:m1".into())],
                changed_threads: vec!["imap:t:m1".into()],
                ..Default::default()
            }).unwrap();

        // Star: INBOX (visited first) succeeds and persists; Trash SELECT
        // transiently fails and the mutation returns that error.
        let err = provider
            .modify_messages(&["imap:me@example.com:m1".into()], &["STARRED".into()], &[])
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::TransientTransport(_)), "Trash select busy: {err:?}");
        assert!(err.retry_mutation(), "transient so the engine retries");

        // INBOX's copy IS starred (server + local); Trash's is NOT yet.
        assert!(server.lock().unwrap().flags_of("INBOX", 1).iter().any(|f| f == "\\Flagged"));
        assert!(local_flags(&store, "INBOX", 1).iter().any(|f| f == "\\Flagged"));
        assert!(!server.lock().unwrap().flags_of("Trash", 1).iter().any(|f| f == "\\Flagged"));
        assert!(!local_flags(&store, "Trash", 1).iter().any(|f| f == "\\Flagged"));

        // Heal Trash and retry. INBOX is already satisfied (local flags say
        // starred), so the retry touches ONLY Trash: exactly one SELECT and
        // one STORE this time, both against Trash.
        server.lock().unwrap().commands.lock().unwrap().clear();
        server.lock().unwrap().folder("Trash", 95).select_busy = false;
        provider
            .modify_messages(&["imap:me@example.com:m1".into()], &["STARRED".into()], &[])
            .await
            .expect("retry succeeds");
        assert!(server.lock().unwrap().flags_of("Trash", 1).iter().any(|f| f == "\\Flagged"));
        let g = server.lock().unwrap();
        assert_eq!(g.count_starting("UID STORE"), 1, "retry stores once (Trash only)");
        // The single SELECT was Trash, never INBOX (INBOX was already done).
        assert!(g.wire().iter().any(|c| c.contains("Trash")));
        assert!(!g.wire().iter().any(|c| c.to_ascii_uppercase().starts_with("SELECT \"INBOX\"")));
    }

    // --- local flags_json updated; a following sync sweep reports no change -

    #[tokio::test]
    async fn after_a_mutation_a_following_sweep_sees_no_change() {
        // Mark a message read, then run a full INBOX sweep against the SAME
        // server (which now holds \Seen). The sweep must find the local flags
        // already agree and NOT bump the generation (no flap, no journal).
        let server = server_with_inbox();
        {
            let mut g = server.lock().unwrap();
            g.folder("INBOX", 100).add(&[], &message("<m1@x>"));
        }
        let (provider, store) = provider_with(server.clone());

        // A baseline sweep ingests m1 so the local store knows its id/thread.
        provider.baseline_cursor().await.unwrap();
        let id = store.locations_in_mailbox("INBOX").unwrap()[0].message_id.clone();
        let gen_before = store.generation().unwrap();

        // Mark it read through the mutation path.
        provider.modify_messages(&[id], &[], &["UNREAD".into()]).await.expect("mark read");
        let gen_after_mutation = store.generation().unwrap();
        assert_eq!(
            gen_after_mutation, gen_before,
            "the mutation itself does not bump the generation"
        );
        assert!(local_flags(&store, "INBOX", 1).iter().any(|f| f == "\\Seen"));

        // A following sweep over the same (now \Seen) server state sees the
        // local flags already agree: no content change, no generation bump.
        let cursor = SyncCursor::from_generation(gen_after_mutation);
        let batch = provider.poll(&cursor).await.unwrap();
        assert_eq!(
            store.generation().unwrap(),
            gen_after_mutation,
            "a following sweep sees no change after the mutation reconciled local flags"
        );
        assert!(batch.changed_threads.is_empty(), "and journals nothing");
    }

    // =======================================================================
    // Phase 3 run 3: system-label MOVES (archive / trash / spam + reverses).
    // =======================================================================

    /// Seed one message as a location + thread + cached body in `mailbox`,
    /// WITHOUT advancing a shared uid counter (callers choose the uid). Returns
    /// the stable message id. Unlike `seed`, this lets several copies of one
    /// message id live in different mailboxes.
    fn seed_loc(
        store: &ImapStateStore,
        provider: &ImapProvider,
        msgid: &str,
        thread: &str,
        mailbox: &str,
        uidvalidity: i64,
        uid: i64,
        flags: &[&str],
    ) -> String {
        let message_id = format!("imap:me@example.com:{msgid}");
        store
            .upsert_location(&ImapLocation {
                mailbox: mailbox.into(),
                uidvalidity,
                uid,
                message_id: message_id.clone(),
                flags_json: serde_json::to_string(
                    &flags.iter().map(|f| f.to_string()).collect::<Vec<_>>(),
                )
                .unwrap(),
                modseq: None,
            })
            .unwrap();
        store
            .commit_sync_round(&SyncRoundWrite {
                thread_assignments: vec![(message_id.clone(), thread.to_string())],
                changed_threads: vec![thread.to_string()],
                ..Default::default()
            })
            .unwrap();
        provider
            .cache
            .put(&message_id, message(&format!("<{msgid}@x>")).as_bytes(), 1)
            .unwrap();
        message_id
    }

    /// A server with INBOX + Archive + Trash + Junk, each with its own
    /// UIDVALIDITY, MOVE + UIDPLUS advertised (the default).
    fn server_with_system_mailboxes() -> Arc<Mutex<MutServer>> {
        let mut s = MutServer::new();
        s.folder("INBOX", 100);
        s.folder("Archive", 200);
        s.folder("Trash", 300);
        s.folder("Junk", 400);
        Arc::new(Mutex::new(s))
    }

    fn uids_in(server: &Arc<Mutex<MutServer>>, mailbox: &str) -> Vec<u32> {
        server
            .lock()
            .unwrap()
            .folders
            .get(mailbox)
            .map(|f| f.messages.iter().map(|m| m.uid).collect())
            .unwrap_or_default()
    }

    fn local_mailboxes_of(store: &ImapStateStore, message_id: &str) -> Vec<String> {
        let mut v: Vec<String> = store
            .locations_for_message(message_id)
            .unwrap()
            .into_iter()
            .map(|l| l.mailbox)
            .collect();
        v.sort();
        v
    }

    // --- archive: MOVE the INBOX copy to Archive ---------------------------

    #[tokio::test]
    async fn archive_moves_the_inbox_copy_to_archive_via_move() {
        let server = server_with_system_mailboxes();
        server.lock().unwrap().folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>")); // uid 1
        let (provider, store) = provider_with(server.clone());
        let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

        // archive == add [], remove [INBOX].
        provider
            .modify_thread("imap:t:m1", &[], &["INBOX".into()])
            .await
            .expect("archive");

        // The server moved it out of INBOX into Archive (one copy).
        assert!(uids_in(&server, "INBOX").is_empty(), "source emptied");
        assert_eq!(uids_in(&server, "Archive").len(), 1, "one copy in Archive");
        // MOVE was used (atomic), not COPY.
        let g = server.lock().unwrap();
        assert_eq!(g.count_starting("UID MOVE"), 1);
        assert_eq!(g.count_starting("UID COPY"), 0);
        drop(g);
        // Local state: the message now lives only in Archive.
        assert_eq!(local_mailboxes_of(&store, &id), vec!["Archive".to_string()]);
    }

    // --- unarchive: Archive -> INBOX ---------------------------------------

    #[tokio::test]
    async fn unarchive_moves_the_archive_copy_to_inbox() {
        let server = server_with_system_mailboxes();
        server.lock().unwrap().folder("Archive", 200).add(&["\\Seen"], &message("<m1@x>")); // uid 1
        let (provider, store) = provider_with(server.clone());
        let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "Archive", 200, 1, &["\\Seen"]);

        // unarchive == add [INBOX], remove [].
        provider
            .modify_thread("imap:t:m1", &["INBOX".into()], &[])
            .await
            .expect("unarchive");

        assert!(uids_in(&server, "Archive").is_empty());
        assert_eq!(uids_in(&server, "INBOX").len(), 1);
        assert_eq!(local_mailboxes_of(&store, &id), vec!["INBOX".to_string()]);
    }

    // --- trash / untrash ---------------------------------------------------

    #[tokio::test]
    async fn trash_then_untrash_round_trips_inbox_and_trash() {
        let server = server_with_system_mailboxes();
        server.lock().unwrap().folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>")); // uid 1
        let (provider, store) = provider_with(server.clone());
        let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

        // trash == add [TRASH], remove [INBOX].
        provider
            .modify_thread("imap:t:m1", &["TRASH".into()], &["INBOX".into()])
            .await
            .expect("trash");
        assert!(uids_in(&server, "INBOX").is_empty());
        assert_eq!(uids_in(&server, "Trash").len(), 1);
        assert_eq!(local_mailboxes_of(&store, &id), vec!["Trash".to_string()]);

        // untrash == add [INBOX], remove [TRASH]. The Trash copy's new UID:
        let trash_uid = uids_in(&server, "Trash")[0] as i64;
        // The local Trash location row carries that server UID (COPYUID path),
        // so untrash can SELECT Trash with the matching UIDVALIDITY.
        provider
            .modify_thread("imap:t:m1", &["INBOX".into()], &["TRASH".into()])
            .await
            .expect("untrash");
        assert!(uids_in(&server, "Trash").is_empty(), "trash uid {trash_uid} moved out");
        assert_eq!(uids_in(&server, "INBOX").len(), 1);
        assert_eq!(local_mailboxes_of(&store, &id), vec!["INBOX".to_string()]);
    }

    // --- spam / not-spam ---------------------------------------------------

    #[tokio::test]
    async fn spam_then_not_spam_round_trips_inbox_and_junk() {
        let server = server_with_system_mailboxes();
        server.lock().unwrap().folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
        let (provider, store) = provider_with(server.clone());
        let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

        provider
            .modify_thread("imap:t:m1", &["SPAM".into()], &["INBOX".into()])
            .await
            .expect("spam");
        assert_eq!(local_mailboxes_of(&store, &id), vec!["Junk".to_string()]);

        provider
            .modify_thread("imap:t:m1", &["INBOX".into()], &["SPAM".into()])
            .await
            .expect("not spam");
        assert_eq!(local_mailboxes_of(&store, &id), vec!["INBOX".to_string()]);
    }

    // --- a thread of several messages across INBOX + Sent: Sent is untouched

    #[tokio::test]
    async fn archive_moves_inbox_copies_and_leaves_sent_untouched() {
        let mut s = MutServer::new();
        s.folder("INBOX", 100).add(&["\\Seen"], &message("<a@x>")); // uid 1
        s.folder("INBOX", 100).add(&["\\Seen"], &message("<b@x>")); // uid 2
        s.folder("Sent", 500).add(&["\\Seen"], &message("<b@x>")); // uid 1 (b also in Sent)
        s.folder("Archive", 200);
        let server = Arc::new(Mutex::new(s));
        let (provider, store) = provider_with(server.clone());

        let a = seed_loc(&store, &provider, "a", "imap:t:th", "INBOX", 100, 1, &["\\Seen"]);
        let b = seed_loc(&store, &provider, "b", "imap:t:th", "INBOX", 100, 2, &["\\Seen"]);
        // b also has a Sent copy.
        store
            .upsert_location(&ImapLocation {
                mailbox: "Sent".into(), uidvalidity: 500, uid: 1,
                message_id: b.clone(), flags_json: r#"["\\Seen"]"#.into(), modseq: None,
            })
            .unwrap();

        provider
            .modify_thread("imap:t:th", &[], &["INBOX".into()])
            .await
            .expect("archive thread");

        // Both INBOX copies moved to Archive; the Sent copy of b is untouched.
        assert!(uids_in(&server, "INBOX").is_empty());
        assert_eq!(uids_in(&server, "Archive").len(), 2);
        assert_eq!(uids_in(&server, "Sent"), vec![1], "Sent copy left in place");
        assert_eq!(local_mailboxes_of(&store, &a), vec!["Archive".to_string()]);
        assert_eq!(local_mailboxes_of(&store, &b), vec!["Archive".to_string(), "Sent".to_string()]);
    }

    // --- a dead sibling (no location) is skipped ---------------------------

    #[tokio::test]
    async fn archive_skips_a_dead_sibling_and_moves_the_live_one() {
        let server = server_with_system_mailboxes();
        server.lock().unwrap().folder("INBOX", 100).add(&["\\Seen"], &message("<live@x>")); // uid 1
        let (provider, store) = provider_with(server.clone());
        let live = seed_loc(&store, &provider, "live", "imap:t:th", "INBOX", 100, 1, &["\\Seen"]);
        // A dead sibling: thread row only, no location.
        store
            .commit_sync_round(&SyncRoundWrite {
                thread_assignments: vec![("imap:me@example.com:dead".into(), "imap:t:th".into())],
                changed_threads: vec!["imap:t:th".into()],
                ..Default::default()
            })
            .unwrap();

        provider
            .modify_thread("imap:t:th", &[], &["INBOX".into()])
            .await
            .expect("the dead sibling must not fail the archive");
        assert_eq!(local_mailboxes_of(&store, &live), vec!["Archive".to_string()]);
    }

    // --- an alias thread id resolves ---------------------------------------

    #[tokio::test]
    async fn system_move_resolves_an_alias_thread_id() {
        let server = server_with_system_mailboxes();
        server.lock().unwrap().folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
        let (provider, store) = provider_with(server.clone());
        let id = seed_loc(&store, &provider, "m1", "imap:t:survivor", "INBOX", 100, 1, &["\\Seen"]);
        store
            .commit_sync_round(&SyncRoundWrite {
                aliases: vec![("imap:t:old".into(), "imap:t:survivor".into())],
                ..Default::default()
            })
            .unwrap();

        provider
            .modify_thread("imap:t:old", &[], &["INBOX".into()])
            .await
            .expect("archive via alias");
        assert_eq!(local_mailboxes_of(&store, &id), vec!["Archive".to_string()]);
    }

    // --- missing Archive is created then used ------------------------------

    #[tokio::test]
    async fn archive_creates_a_missing_archive_mailbox_then_moves() {
        // No Archive folder on the server; a UIDPLUS-only server so the move is
        // copy+delete+expunge and the COPY draws the TRYCREATE.
        let mut s = MutServer::new();
        s.capabilities = vec!["IMAP4REV1".into(), "UIDPLUS".into()]; // no MOVE
        s.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>")); // uid 1
        s.copy_trycreate_once = Some("Mailbox doesn't exist: Archive".into());
        let server = Arc::new(Mutex::new(s));
        let (provider, store) = provider_with(server.clone());
        // The provider resolves the Archive role by NAME; seed a catalog row
        // so the plan knows "Archive" is the archive mailbox even before it
        // exists on the server (discovery would have listed it; here the test
        // asserts the create-on-TRYCREATE path).
        store
            .upsert_mailbox(&super::super::ImapMailbox {
                name: "Archive".into(),
                delimiter: Some("/".into()),
                special_use: Some("\\Archive".into()),
                uidvalidity: 0,
                uidnext: 0,
                highestmodseq: None,
                permanent_flags_json: None,
                permanent_keywords: None,
            })
            .unwrap();
        let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

        provider
            .modify_thread("imap:t:m1", &[], &["INBOX".into()])
            .await
            .expect("archive creates the destination then moves");

        let g = server.lock().unwrap();
        assert_eq!(g.count_starting("CREATE"), 1, "Archive was created");
        assert!(g.folders.contains_key("Archive"), "Archive now exists");
        assert_eq!(
            g.folders.get("Archive").map(|f| f.messages.len()),
            Some(1),
            "the message landed in Archive"
        );
        drop(g);
        assert_eq!(local_mailboxes_of(&store, &id), vec!["Archive".to_string()]);
    }

    // --- missing Trash is a permanent error naming it, nothing sent --------

    #[tokio::test]
    async fn trash_with_no_trash_mailbox_is_a_permanent_error_naming_the_role() {
        // INBOX only — no Trash configured.
        let server = server_with_inbox();
        server.lock().unwrap().folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
        let (provider, store) = provider_with(server.clone());
        let _id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

        let err = provider
            .modify_thread("imap:t:m1", &["TRASH".into()], &["INBOX".into()])
            .await
            .unwrap_err();
        match err {
            ProviderError::InvalidOperation(m) => {
                assert!(m.to_ascii_lowercase().contains("trash"), "names the role: {m}");
            }
            other => panic!("expected permanent InvalidOperation, got {other:?}"),
        }
        // Nothing reached the wire (not even a SELECT): the destination is
        // resolved from the local plan before any command.
        let g = server.lock().unwrap();
        assert_eq!(g.count_starting("SELECT"), 0);
        assert_eq!(g.count_starting("UID MOVE"), 0);
        assert_eq!(g.count_starting("UID COPY"), 0);
    }

    // --- COPY+\Deleted+EXPUNGE strategy (UIDPLUS only, no MOVE) -------------

    #[tokio::test]
    async fn archive_uses_copy_delete_expunge_when_only_uidplus() {
        let mut s = MutServer::new();
        s.capabilities = vec!["IMAP4REV1".into(), "UIDPLUS".into()]; // no MOVE
        s.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>")); // uid 1
        s.folder("Archive", 200);
        let server = Arc::new(Mutex::new(s));
        let (provider, store) = provider_with(server.clone());
        let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

        provider
            .modify_thread("imap:t:m1", &[], &["INBOX".into()])
            .await
            .expect("archive via copy+delete+expunge");

        let g = server.lock().unwrap();
        assert_eq!(g.count_starting("UID COPY"), 1);
        assert_eq!(g.count_starting("UID MOVE"), 0, "no MOVE without the capability");
        assert_eq!(g.count_starting("UID EXPUNGE"), 1, "targeted expunge");
        drop(g);
        assert!(uids_in(&server, "INBOX").is_empty());
        assert_eq!(uids_in(&server, "Archive").len(), 1);
        assert_eq!(local_mailboxes_of(&store, &id), vec!["Archive".to_string()]);
    }

    // --- neither MOVE nor UIDPLUS -> permanent, no write command -----------

    #[tokio::test]
    async fn archive_with_no_move_or_uidplus_is_permanent_and_sends_no_write() {
        let mut s = MutServer::new();
        s.capabilities = vec!["IMAP4REV1".into()]; // neither
        s.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
        s.folder("Archive", 200);
        let server = Arc::new(Mutex::new(s));
        let (provider, store) = provider_with(server.clone());
        let _id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

        let err = provider
            .modify_thread("imap:t:m1", &[], &["INBOX".into()])
            .await
            .unwrap_err();
        match err {
            ProviderError::InvalidOperation(m) => assert!(m.contains("EXPUNGE"), "names the hazard: {m}"),
            other => panic!("expected Unsupported InvalidOperation, got {other:?}"),
        }
        // The guard SELECTed to read capabilities, but issued no relocation.
        let g = server.lock().unwrap();
        assert_eq!(g.count_starting("UID MOVE"), 0);
        assert_eq!(g.count_starting("UID COPY"), 0);
        assert_eq!(g.count_starting("UID EXPUNGE"), 0);
    }

    // --- already satisfied sends nothing -----------------------------------

    #[tokio::test]
    async fn archive_of_a_thread_with_no_inbox_copy_sends_nothing() {
        // The message is already in Archive (archived), not in INBOX. Archive
        // again is a no-op: Ok and NOT A SINGLE command (not even SELECT).
        let server = server_with_system_mailboxes();
        server.lock().unwrap().folder("Archive", 200).add(&["\\Seen"], &message("<m1@x>"));
        let (provider, store) = provider_with(server.clone());
        let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "Archive", 200, 1, &["\\Seen"]);

        provider
            .modify_thread("imap:t:m1", &[], &["INBOX".into()])
            .await
            .expect("archive of an already-archived thread is a no-op Ok");
        // archive's dest is Archive and the message is already there, so it is
        // already satisfied: nothing sent.
        let g = server.lock().unwrap();
        assert_eq!(g.count_starting("SELECT"), 0, "no SELECT for a satisfied archive");
        assert_eq!(g.count_starting("UID MOVE"), 0);
        assert_eq!(g.count_starting("UID COPY"), 0);
        drop(g);
        assert_eq!(local_mailboxes_of(&store, &id), vec!["Archive".to_string()]);
    }

    // --- nothing movable and not satisfied is permanent --------------------

    #[tokio::test]
    async fn trash_of_a_sent_only_thread_is_permanent_and_sends_nothing() {
        let mut s = MutServer::new();
        s.folder("INBOX", 100);
        s.folder("Trash", 300);
        s.folder("Sent", 500).add(&["\\Seen"], &message("<m1@x>"));
        let server = Arc::new(Mutex::new(s));
        let (provider, store) = provider_with(server.clone());
        let _id = seed_loc(&store, &provider, "m1", "imap:t:m1", "Sent", 500, 1, &["\\Seen"]);

        let err = provider
            .modify_thread("imap:t:m1", &["TRASH".into()], &["INBOX".into()])
            .await
            .unwrap_err();
        match err {
            ProviderError::InvalidOperation(m) => assert!(m.to_ascii_lowercase().contains("sent"), "{m}"),
            other => panic!("expected permanent InvalidOperation, got {other:?}"),
        }
        let g = server.lock().unwrap();
        assert_eq!(g.count_starting("UID MOVE") + g.count_starting("UID COPY"), 0);
    }

    // --- stale UIDVALIDITY on the source sends nothing and is transient ----

    #[tokio::test]
    async fn system_move_with_a_stale_source_uidvalidity_is_transient_and_sends_no_write() {
        let server = server_with_system_mailboxes();
        {
            let mut g = server.lock().unwrap();
            g.folder("INBOX", 100).uidvalidity = 999; // server moved on
            g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
        }
        let (provider, store) = provider_with(server.clone());
        let _id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]); // stored 100

        let err = provider
            .modify_thread("imap:t:m1", &[], &["INBOX".into()])
            .await
            .unwrap_err();
        assert!(
            matches!(err, ProviderError::TransientTransport(ref m) if m.contains("UIDVALIDITY")),
            "{err:?}"
        );
        assert!(err.retry_mutation());
        let g = server.lock().unwrap();
        assert_eq!(g.count_starting("UID MOVE"), 0);
        assert_eq!(g.count_starting("UID COPY"), 0);
    }

    // --- half-applied move (expunge fails), then retry: exactly one dest copy

    #[tokio::test]
    async fn a_half_applied_move_then_retry_yields_exactly_one_destination_copy() {
        // UIDPLUS-only (copy+delete+expunge). The first UID EXPUNGE fails after
        // the COPY and \Deleted store landed: the message is now in BOTH INBOX
        // and Archive, and the mutation returns the partial error. The retry
        // must NOT copy again (it sees the recorded Archive location) — it only
        // expunges the source — so Archive ends with exactly one copy.
        let mut s = MutServer::new();
        s.capabilities = vec!["IMAP4REV1".into(), "UIDPLUS".into()]; // no MOVE
        s.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>")); // uid 1
        s.folder("Archive", 200);
        s.fail_expunge_once = Some("expunge refused".into());
        let server = Arc::new(Mutex::new(s));
        let (provider, store) = provider_with(server.clone());
        let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

        // First attempt: COPY + \Deleted ok, EXPUNGE fails -> partial error.
        let err = provider
            .modify_thread("imap:t:m1", &[], &["INBOX".into()])
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::InvalidOperation(ref m) if m.contains("BOTH")), "{err:?}");
        // The server now has the copy in BOTH mailboxes...
        assert_eq!(uids_in(&server, "Archive").len(), 1, "copy landed in Archive");
        assert_eq!(uids_in(&server, "INBOX").len(), 1, "source \\Deleted but not yet expunged");
        // ...and the local store already records the Archive destination.
        assert!(local_mailboxes_of(&store, &id).contains(&"Archive".to_string()));

        // Retry: the destination copy is already recorded, so NO second COPY —
        // only the source is expunged. Archive stays at exactly one copy.
        let copies_before = server.lock().unwrap().count_starting("UID COPY");
        provider
            .modify_thread("imap:t:m1", &[], &["INBOX".into()])
            .await
            .expect("retry finishes the half-applied move");
        let g = server.lock().unwrap();
        assert_eq!(
            g.count_starting("UID COPY"),
            copies_before,
            "the retry copies NOTHING again"
        );
        drop(g);
        assert_eq!(uids_in(&server, "Archive").len(), 1, "exactly one destination copy");
        assert!(uids_in(&server, "INBOX").is_empty(), "source now expunged");
        assert_eq!(local_mailboxes_of(&store, &id), vec!["Archive".to_string()]);
    }

    // --- a TRANSIENT cause of a half-applied move stays transient -----------

    #[tokio::test]
    async fn a_transient_expunge_failure_after_the_copy_is_retryable_and_the_retry_completes() {
        let mut s = MutServer::new();
        s.capabilities = vec!["IMAP4REV1".into(), "UIDPLUS".into()]; // no MOVE
        s.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
        s.folder("Archive", 200);
        s.fail_expunge_once = Some("[UNAVAILABLE] try later".into());
        let server = Arc::new(Mutex::new(s));
        let (provider, store) = provider_with(server.clone());
        let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

        // The engine retries only retry_mutation() errors; a permanent class
        // here would roll the archive back and leave the message in BOTH
        // mailboxes with nobody to finish it.
        let err = provider
            .modify_thread("imap:t:m1", &[], &["INBOX".into()])
            .await
            .unwrap_err();
        assert!(err.retry_mutation(), "a transient cause must stay retryable: {err:?}");
        assert!(err.to_string().contains("BOTH"), "still names the partial state: {err}");
        assert!(local_mailboxes_of(&store, &id).contains(&"Archive".to_string()));

        provider
            .modify_thread("imap:t:m1", &[], &["INBOX".into()])
            .await
            .expect("the engine's retry finishes the move");
        assert_eq!(uids_in(&server, "Archive").len(), 1, "exactly one destination copy");
        assert!(uids_in(&server, "INBOX").is_empty());
        assert_eq!(local_mailboxes_of(&store, &id), vec!["Archive".to_string()]);
    }

    // --- partial failure across two source mailboxes -----------------------

    #[tokio::test]
    async fn trash_partial_failure_persists_first_source_and_retry_touches_only_the_second() {
        // Two messages of one thread: one whose movable copy is in Archive
        // (trash-from-archive-view), one in INBOX. Source mailboxes are
        // processed in name order (Archive before INBOX). Archive succeeds and
        // is persisted; INBOX's SELECT is transiently busy, so the mutation
        // returns that error AFTER persisting Archive's move.
        let mut s = MutServer::new();
        s.folder("Archive", 200).add(&["\\Seen"], &message("<a@x>")); // uid 1
        s.folder("INBOX", 100).add(&["\\Seen"], &message("<b@x>")); // uid 1
        s.folder("Trash", 300);
        s.folder("INBOX", 100).select_busy = true;
        let server = Arc::new(Mutex::new(s));
        let (provider, store) = provider_with(server.clone());
        let a = seed_loc(&store, &provider, "a", "imap:t:th", "Archive", 200, 1, &["\\Seen"]);
        let b = seed_loc(&store, &provider, "b", "imap:t:th", "INBOX", 100, 1, &["\\Seen"]);

        let err = provider
            .modify_thread("imap:t:th", &["TRASH".into()], &["INBOX".into()])
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::TransientTransport(_)), "{err:?}");
        // a (Archive, processed first) moved to Trash and is persisted; b is
        // still only in INBOX.
        assert_eq!(local_mailboxes_of(&store, &a), vec!["Trash".to_string()]);
        assert_eq!(local_mailboxes_of(&store, &b), vec!["INBOX".to_string()]);

        // Heal INBOX and retry: a is already in Trash (satisfied), so only b
        // is touched.
        server.lock().unwrap().commands.lock().unwrap().clear();
        server.lock().unwrap().folder("INBOX", 100).select_busy = false;
        provider
            .modify_thread("imap:t:th", &["TRASH".into()], &["INBOX".into()])
            .await
            .expect("retry trashes the remaining copy");
        assert_eq!(local_mailboxes_of(&store, &a), vec!["Trash".to_string()]);
        assert_eq!(local_mailboxes_of(&store, &b), vec!["Trash".to_string()]);
        // The retry SELECTed INBOX (b's source), never Archive again.
        let g = server.lock().unwrap();
        assert!(g.wire().iter().any(|c| c.to_ascii_uppercase().contains("SELECT \"INBOX\"")));
        assert!(!g.wire().iter().any(|c| c.contains("Archive")));
    }

    // --- combined with a flag / lf: / via modify_messages: rejected, 0 cmds -

    #[tokio::test]
    async fn a_system_id_mixed_or_via_modify_messages_is_rejected_with_zero_commands() {
        let server = server_with_system_mailboxes();
        server.lock().unwrap().folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
        let (provider, store) = provider_with(server.clone());
        let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

        // INBOX combined with UNREAD (a flag) — not an engine move shape.
        let err = provider
            .modify_thread("imap:t:m1", &[], &["INBOX".into(), "UNREAD".into()])
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::InvalidOperation(_)), "{err:?}");

        // TRASH combined with a label-folder id.
        let err = provider
            .modify_thread("imap:t:m1", &["TRASH".into(), "lf:Work".into()], &["INBOX".into()])
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::InvalidOperation(_)), "{err:?}");

        // A system id through modify_messages is rejected (flag path only).
        let err = provider
            .modify_messages(&[id], &["TRASH".into()], &["INBOX".into()])
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::InvalidOperation(_)), "{err:?}");

        // Not one command reached the wire for any rejected request.
        let g = server.lock().unwrap();
        assert_eq!(g.count_starting("SELECT"), 0);
        assert_eq!(g.count_starting("UID MOVE"), 0);
        assert_eq!(g.count_starting("UID COPY"), 0);
    }

    // --- a following sweep over the moved state sees no change / no dup ------

    #[tokio::test]
    async fn after_a_move_a_following_sweep_sees_no_change_and_no_duplicate() {
        // Archive a message, then run a full refresh over the SAME server
        // state (the message now lives only in Archive). INBOX's sweep must not
        // re-ingest the moved message, and Archive's sweep must find the local
        // location already recorded (same UID) — so no new message, no label
        // flap.
        let server = server_with_system_mailboxes();
        server.lock().unwrap().folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
        let (provider, store) = provider_with(server.clone());

        // Baseline so the local store knows m1 (in INBOX) and its thread.
        provider.baseline_cursor().await.unwrap();
        let id = store.locations_in_mailbox("INBOX").unwrap()[0].message_id.clone();

        // Archive it.
        provider
            .modify_thread(
                &store.thread_of_message(&id).unwrap().unwrap(),
                &[],
                &["INBOX".into()],
            )
            .await
            .expect("archive");
        assert_eq!(local_mailboxes_of(&store, &id), vec!["Archive".to_string()]);
        let gen_after_move = store.generation().unwrap();

        // A following full refresh over the same server state.
        let cursor = SyncCursor::from_generation(gen_after_move);
        let _ = provider.poll(&cursor).await.unwrap();

        // Exactly one copy locally (Archive), the same UID the move recorded;
        // no duplicate ingested, no stray INBOX row re-created.
        assert_eq!(
            local_mailboxes_of(&store, &id),
            vec!["Archive".to_string()],
            "the sweep neither duplicated the message nor resurrected the INBOX copy"
        );
        assert_eq!(
            store.locations_for_message(&id).unwrap().len(),
            1,
            "exactly one location after the sweep"
        );
    }

    // =======================================================================
    // Phase 3 run 4: USER LABELS in label-folder mode (lf: add/remove + label
    // create/delete; rename refused). Driven through a REAL ImapProvider over
    // the shared MutServer, so a COPY/EXPUNGE/CREATE/DELETE reaches the wire
    // and a following sweep runs against the same state. test_settings() sets
    // label_storage = Folders, container = Labels.
    // =======================================================================

    /// A server with INBOX + Archive + Sent + the label mailbox `Labels/Clients`,
    /// MOVE + UIDPLUS advertised. The label container `Labels` is \Noselect
    /// (never a folder row); its child `Labels/Clients` is the label mailbox.
    fn server_with_label_folder() -> Arc<Mutex<MutServer>> {
        let mut s = MutServer::new();
        s.folder("INBOX", 100);
        s.folder("Archive", 200);
        s.folder("Sent", 500);
        s.folder("Labels/Clients", 600);
        Arc::new(Mutex::new(s))
    }

    /// Local location count in one mailbox.
    fn local_count_in(store: &ImapStateStore, mailbox: &str) -> usize {
        store.locations_in_mailbox(mailbox).unwrap().len()
    }

    // --- add a label to one message: COPY into the label mailbox -----------

    #[tokio::test]
    async fn add_label_copies_the_inbox_message_into_the_label_mailbox() {
        let server = server_with_label_folder();
        server.lock().unwrap().folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>")); // uid 1
        let (provider, store) = provider_with(server.clone());
        let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

        provider
            .modify_messages(&[id.clone()], &["lf:Clients".into()], &[])
            .await
            .expect("add label");

        // The server now holds a copy in Labels/Clients, and the local store
        // recorded that destination location (from COPYUID) with the same flags.
        assert_eq!(uids_in(&server, "Labels/Clients").len(), 1, "one copy on the server");
        let boxes = local_mailboxes_of(&store, &id);
        assert!(boxes.contains(&"INBOX".to_string()));
        assert!(boxes.contains(&"Labels/Clients".to_string()), "{boxes:?}");
        // Exactly one COPY command, no MOVE, no EXPUNGE.
        let g = server.lock().unwrap();
        assert_eq!(g.count_starting("UID COPY"), 1);
        assert_eq!(g.count_starting("UID MOVE"), 0);
        assert_eq!(g.count_starting("UID EXPUNGE"), 0);
    }

    #[tokio::test]
    async fn add_label_to_several_messages_copies_each() {
        let server = server_with_label_folder();
        {
            let mut g = server.lock().unwrap();
            g.folder("INBOX", 100).add(&["\\Seen"], &message("<a@x>")); // uid 1
            g.folder("INBOX", 100).add(&["\\Seen"], &message("<b@x>")); // uid 2
        }
        let (provider, store) = provider_with(server.clone());
        let a = seed_loc(&store, &provider, "a", "imap:t:a", "INBOX", 100, 1, &["\\Seen"]);
        let b = seed_loc(&store, &provider, "b", "imap:t:b", "INBOX", 100, 2, &["\\Seen"]);

        provider
            .modify_messages(&[a.clone(), b.clone()], &["lf:Clients".into()], &[])
            .await
            .expect("add label to two");

        assert_eq!(uids_in(&server, "Labels/Clients").len(), 2, "two copies on the server");
        assert!(local_mailboxes_of(&store, &a).contains(&"Labels/Clients".to_string()));
        assert!(local_mailboxes_of(&store, &b).contains(&"Labels/Clients".to_string()));
    }

    // --- add is idempotent: a second add sends no command ------------------

    #[tokio::test]
    async fn add_label_is_idempotent_and_sends_no_command_the_second_time() {
        let server = server_with_label_folder();
        server.lock().unwrap().folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
        let (provider, store) = provider_with(server.clone());
        let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

        provider.modify_messages(&[id.clone()], &["lf:Clients".into()], &[]).await.expect("add 1");
        server.lock().unwrap().commands.lock().unwrap().clear();
        // Second add: already satisfied — ZERO commands (not even a SELECT).
        provider.modify_messages(&[id], &["lf:Clients".into()], &[]).await.expect("add 2 no-op");
        let g = server.lock().unwrap();
        assert_eq!(g.count_starting("SELECT"), 0, "idempotent add selects nothing");
        assert_eq!(g.count_starting("UID COPY"), 0, "and copies nothing");
    }

    // --- source priority: Archive used when the message is not in INBOX ----

    #[tokio::test]
    async fn add_label_source_priority_prefers_inbox_then_archive() {
        // A message with copies in Archive AND Sent (no INBOX): Archive wins.
        let server = server_with_label_folder();
        {
            let mut g = server.lock().unwrap();
            g.folder("Archive", 200).add(&["\\Seen"], &message("<m1@x>")); // uid 1
            g.folder("Sent", 500).add(&["\\Seen"], &message("<m1@x>")); // uid 1
        }
        let (provider, store) = provider_with(server.clone());
        let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "Archive", 200, 1, &["\\Seen"]);
        store
            .upsert_location(&ImapLocation {
                mailbox: "Sent".into(), uidvalidity: 500, uid: 1,
                message_id: id.clone(), flags_json: r#"["\\Seen"]"#.into(), modseq: None,
            })
            .unwrap();

        provider.modify_messages(&[id.clone()], &["lf:Clients".into()], &[]).await.expect("add");

        // The COPY was issued from Archive, not Sent.
        let g = server.lock().unwrap();
        assert!(
            g.wire().iter().any(|c| c.to_ascii_uppercase().starts_with("SELECT \"ARCHIVE\"")),
            "selected Archive as the source: {:?}", g.wire()
        );
        assert!(!g.wire().iter().any(|c| c.to_ascii_uppercase().starts_with("SELECT \"SENT\"")));
    }

    // --- a message whose only copy is in Trash is refused ------------------

    #[tokio::test]
    async fn add_label_from_a_trash_only_message_is_a_permanent_error() {
        let mut s = MutServer::new();
        s.folder("INBOX", 100);
        s.folder("Trash", 300).add(&["\\Seen"], &message("<m1@x>")); // uid 1
        s.folder("Labels/Clients", 600);
        let server = Arc::new(Mutex::new(s));
        let (provider, store) = provider_with(server.clone());
        let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "Trash", 300, 1, &["\\Seen"]);

        let err = provider
            .modify_messages(&[id], &["lf:Clients".into()], &[])
            .await
            .unwrap_err();
        assert!(!err.retry_mutation());
        match err {
            ProviderError::InvalidOperation(m) => assert!(m.contains("no synced copy outside Trash"), "{m}"),
            other => panic!("expected permanent InvalidOperation, got {other:?}"),
        }
        assert_eq!(server.lock().unwrap().count_starting("UID COPY"), 0, "nothing copied");
    }

    // --- a missing label mailbox is a permanent error ----------------------

    #[tokio::test]
    async fn add_label_to_a_missing_label_mailbox_is_permanent() {
        // No Labels/Clients folder exists in the catalog.
        let mut s = MutServer::new();
        s.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
        let server = Arc::new(Mutex::new(s));
        let (provider, store) = provider_with(server.clone());
        let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

        let err = provider
            .modify_messages(&[id], &["lf:Clients".into()], &[])
            .await
            .unwrap_err();
        match err {
            ProviderError::InvalidOperation(m) => assert!(m.contains("does not exist"), "{m}"),
            other => panic!("expected permanent InvalidOperation, got {other:?}"),
        }
        assert_eq!(server.lock().unwrap().count_starting("UID COPY"), 0);
    }

    // --- add with a stale source UIDVALIDITY sends nothing and is transient -

    #[tokio::test]
    async fn add_label_with_a_stale_source_uidvalidity_is_transient_and_sends_no_copy() {
        let server = server_with_label_folder();
        {
            let mut g = server.lock().unwrap();
            g.folder("INBOX", 100).uidvalidity = 999; // server moved on
            g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
        }
        let (provider, store) = provider_with(server.clone());
        // Local location says UIDVALIDITY 100 (stale).
        let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

        let err = provider
            .modify_messages(&[id], &["lf:Clients".into()], &[])
            .await
            .unwrap_err();
        assert!(
            matches!(err, ProviderError::TransientTransport(ref m) if m.contains("UIDVALIDITY")),
            "{err:?}"
        );
        assert!(err.retry_mutation());
        assert_eq!(server.lock().unwrap().count_starting("UID COPY"), 0, "nothing copied");
    }

    // --- remove a label: expunge only that copy, keep the others -----------

    #[tokio::test]
    async fn remove_label_expunges_only_the_label_copy_and_keeps_the_inbox_copy() {
        let server = server_with_label_folder();
        {
            let mut g = server.lock().unwrap();
            g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>")); // uid 1
            g.folder("Labels/Clients", 600).add(&["\\Seen"], &message("<m1@x>")); // uid 1
        }
        let (provider, store) = provider_with(server.clone());
        let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);
        store
            .upsert_location(&ImapLocation {
                mailbox: "Labels/Clients".into(), uidvalidity: 600, uid: 1,
                message_id: id.clone(), flags_json: r#"["\\Seen"]"#.into(), modseq: None,
            })
            .unwrap();

        provider
            .modify_messages(&[id.clone()], &[], &["lf:Clients".into()])
            .await
            .expect("remove label");

        // The label copy is gone (server + local); the INBOX copy is untouched.
        assert_eq!(uids_in(&server, "Labels/Clients").len(), 0, "label copy expunged");
        assert_eq!(uids_in(&server, "INBOX"), vec![1], "INBOX copy kept");
        assert_eq!(local_mailboxes_of(&store, &id), vec!["INBOX".to_string()]);
        let g = server.lock().unwrap();
        assert_eq!(g.count_starting("UID EXPUNGE"), 1);
    }

    // --- remove the LAST copy is refused with ZERO commands ----------------

    #[tokio::test]
    async fn remove_label_that_is_the_last_copy_is_refused_with_zero_commands() {
        let server = server_with_label_folder();
        server.lock().unwrap().folder("Labels/Clients", 600).add(&["\\Seen"], &message("<m1@x>"));
        let (provider, store) = provider_with(server.clone());
        // The ONLY synced location is the label copy.
        let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "Labels/Clients", 600, 1, &["\\Seen"]);

        let err = provider
            .modify_messages(&[id], &[], &["lf:Clients".into()])
            .await
            .unwrap_err();
        match err {
            ProviderError::InvalidOperation(m) => assert!(m.contains("only synced location"), "{m}"),
            other => panic!("expected permanent InvalidOperation, got {other:?}"),
        }
        let g = server.lock().unwrap();
        assert_eq!(g.count_starting("SELECT"), 0, "refused before any SELECT");
        assert_eq!(g.count_starting("UID STORE"), 0);
        assert_eq!(g.count_starting("UID EXPUNGE"), 0);
    }

    // --- remove without UIDPLUS refused with zero write commands -----------

    #[tokio::test]
    async fn remove_label_without_uidplus_is_refused_with_no_write() {
        let server = server_with_label_folder();
        {
            let mut g = server.lock().unwrap();
            g.capabilities = vec!["IMAP4REV1".into(), "MOVE".into()]; // NO UIDPLUS
            g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
            g.folder("Labels/Clients", 600).add(&["\\Seen"], &message("<m1@x>"));
        }
        let (provider, store) = provider_with(server.clone());
        let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);
        store
            .upsert_location(&ImapLocation {
                mailbox: "Labels/Clients".into(), uidvalidity: 600, uid: 1,
                message_id: id.clone(), flags_json: r#"["\\Seen"]"#.into(), modseq: None,
            })
            .unwrap();

        let err = provider
            .modify_messages(&[id], &[], &["lf:Clients".into()])
            .await
            .unwrap_err();
        match err {
            ProviderError::InvalidOperation(m) => assert!(m.contains("UIDPLUS"), "{m}"),
            other => panic!("expected permanent InvalidOperation, got {other:?}"),
        }
        let g = server.lock().unwrap();
        assert_eq!(g.count_starting("UID STORE"), 0, "no \\Deleted store without UIDPLUS");
        assert_eq!(g.count_starting("UID EXPUNGE"), 0, "and never a plain/UID EXPUNGE");
    }

    // --- remove with no label copy is a satisfied no-op --------------------

    #[tokio::test]
    async fn remove_label_with_no_label_copy_sends_nothing() {
        let server = server_with_label_folder();
        server.lock().unwrap().folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
        let (provider, store) = provider_with(server.clone());
        let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

        provider
            .modify_messages(&[id], &[], &["lf:Clients".into()])
            .await
            .expect("satisfied no-op");
        let g = server.lock().unwrap();
        assert_eq!(g.count_starting("SELECT"), 0);
        assert_eq!(g.count_starting("UID EXPUNGE"), 0);
    }

    // --- half-applied remove (expunge fails once) then retry finishes -------

    #[tokio::test]
    async fn remove_label_half_applied_expunge_failure_is_retryable_and_the_retry_finishes() {
        for (fail_text, expect_transient) in [
            ("[UNAVAILABLE] try later", true),
            ("[NONEXISTENT] gone", false),
        ] {
            let server = server_with_label_folder();
            {
                let mut g = server.lock().unwrap();
                g.fail_expunge_once = Some(fail_text.into());
                g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
                g.folder("Labels/Clients", 600).add(&["\\Seen"], &message("<m1@x>"));
            }
            let (provider, store) = provider_with(server.clone());
            let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);
            store
                .upsert_location(&ImapLocation {
                    mailbox: "Labels/Clients".into(), uidvalidity: 600, uid: 1,
                    message_id: id.clone(), flags_json: r#"["\\Seen"]"#.into(), modseq: None,
                })
                .unwrap();

            // First attempt: COPY-side \Deleted store lands, EXPUNGE fails once.
            let err = provider
                .modify_messages(&[id.clone()], &[], &["lf:Clients".into()])
                .await
                .unwrap_err();
            assert_eq!(
                err.retry_mutation(), expect_transient,
                "retry class preserved for {fail_text}: {err:?}"
            );
            // The label copy is still present on the server (flagged \Deleted),
            // and still recorded locally — the half state the retry finishes.
            assert_eq!(uids_in(&server, "Labels/Clients").len(), 1, "copy still present after the failed expunge");
            assert!(local_mailboxes_of(&store, &id).contains(&"Labels/Clients".to_string()));

            // Retry: the one-shot failure is cleared, the expunge completes,
            // the label copy and its local row are gone, INBOX is untouched.
            provider
                .modify_messages(&[id.clone()], &[], &["lf:Clients".into()])
                .await
                .expect("retry finishes the remove");
            assert_eq!(uids_in(&server, "Labels/Clients").len(), 0, "copy expunged on retry");
            assert_eq!(local_mailboxes_of(&store, &id), vec!["INBOX".to_string()]);
        }
    }

    // --- a partial add failure does not create a second copy on retry ------

    #[tokio::test]
    async fn add_label_retry_after_a_recorded_copy_does_not_double_copy() {
        // Simulate a partial success: the destination row is already recorded
        // locally (an earlier COPY confirmed) but the mutation is retried. The
        // retry must COPY nothing (idempotent).
        let server = server_with_label_folder();
        {
            let mut g = server.lock().unwrap();
            g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
            g.folder("Labels/Clients", 600).add(&["\\Seen"], &message("<m1@x>")); // the copy already landed
        }
        let (provider, store) = provider_with(server.clone());
        let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);
        // Record the destination row as a prior partial success would.
        store
            .upsert_location(&ImapLocation {
                mailbox: "Labels/Clients".into(), uidvalidity: 600, uid: 1,
                message_id: id.clone(), flags_json: r#"["\\Seen"]"#.into(), modseq: None,
            })
            .unwrap();

        provider.modify_messages(&[id], &["lf:Clients".into()], &[]).await.expect("retry add");
        // Still exactly one copy on the server (no second copy created).
        assert_eq!(uids_in(&server, "Labels/Clients").len(), 1, "no duplicate copy");
        assert_eq!(server.lock().unwrap().count_starting("UID COPY"), 0, "retry copies nothing");
    }

    // --- non-Folders storage mode rejects every lf: op with zero commands --

    #[tokio::test]
    async fn label_ops_in_non_folders_storage_mode_are_rejected_with_zero_commands() {
        use super::super::settings::LabelStorage;
        let server = server_with_label_folder();
        server.lock().unwrap().folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
        let (mut provider, store) = provider_with(server.clone());
        // Flip to keyword mode (no container in play).
        provider.settings.label_storage = LabelStorage::Keywords;
        provider.settings.label_container = None;
        let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

        for (add, remove) in [
            (vec!["lf:Clients".to_string()], vec![]),
            (vec![], vec!["lf:Clients".to_string()]),
        ] {
            let err = provider.modify_messages(&[id.clone()], &add, &remove).await.unwrap_err();
            match err {
                ProviderError::InvalidOperation(m) => assert!(m.contains("keyword labels"), "{m}"),
                other => panic!("expected InvalidOperation naming the mode, got {other:?}"),
            }
        }
        // create / delete likewise refuse naming the mode.
        assert!(matches!(provider.create_label("X").await.unwrap_err(), ProviderError::InvalidOperation(_)));
        assert!(matches!(provider.delete_label("lf:Clients").await.unwrap_err(), ProviderError::InvalidOperation(_)));
        assert_eq!(server.lock().unwrap().count_starting("UID COPY"), 0);
        assert_eq!(server.lock().unwrap().count_starting("CREATE"), 0);
    }

    // --- mixed / folder: / system / flag ids rejected with zero commands ---

    #[tokio::test]
    async fn a_mixed_or_multi_label_message_request_is_rejected_before_any_command() {
        let server = server_with_label_folder();
        server.lock().unwrap().folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
        let (provider, store) = provider_with(server.clone());
        let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

        let bad: Vec<(Vec<String>, Vec<String>)> = vec![
            (vec!["lf:Clients".into(), "STARRED".into()], vec![]), // label + flag
            (vec!["lf:A".into(), "lf:B".into()], vec![]),          // two labels
            (vec!["lf:Clients".into()], vec!["lf:Clients".into()]), // both lists
            (vec!["folder:Projects".into()], vec![]),               // folder: not accepted
            (vec!["lf:Clients".into(), "INBOX".into()], vec![]),    // label + system
        ];
        for (add, remove) in bad {
            let err = provider.modify_messages(&[id.clone()], &add, &remove).await.unwrap_err();
            assert!(matches!(err, ProviderError::InvalidOperation(_)), "add={add:?} remove={remove:?} -> {err:?}");
            assert!(!err.retry_mutation());
        }
        let g = server.lock().unwrap();
        assert_eq!(g.count_starting("SELECT"), 0, "nothing selected for a rejected request");
        assert_eq!(g.count_starting("UID COPY"), 0);
    }

    // --- modify_thread still rejects a lf: id (engine never sends it there) -

    #[tokio::test]
    async fn modify_thread_still_rejects_a_label_id_permanently() {
        let server = server_with_label_folder();
        server.lock().unwrap().folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
        let (provider, store) = provider_with(server.clone());
        let _id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);

        let err = provider
            .modify_thread("imap:t:m1", &["lf:Clients".into()], &[])
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::InvalidOperation(_)), "{err:?}");
        assert_eq!(server.lock().unwrap().count_starting("UID COPY"), 0);
    }

    // --- create a label: new, case-insensitive existing, hostile, limits ---

    #[tokio::test]
    async fn create_label_creates_the_mailbox_and_lists_it() {
        let server = server_with_label_folder();
        let (provider, _store) = provider_with(server.clone());

        let label = provider.create_label("Acme").await.expect("create");
        assert_eq!(label.id, "lf:Acme");
        assert_eq!(label.name, "Acme");
        assert_eq!(label.kind, "user");
        // The mailbox Labels/Acme exists on the server…
        assert!(server.lock().unwrap().folders.contains_key("Labels/Acme"));
        // …and list_labels shows it immediately, with no sync.
        let listed = provider.list_labels().await.unwrap();
        assert!(listed.iter().any(|l| l.id == "lf:Acme" && l.kind == "user"), "{listed:?}");
    }

    #[tokio::test]
    async fn create_label_is_case_insensitively_idempotent() {
        let server = server_with_label_folder(); // already has Labels/Clients
        let (provider, _store) = provider_with(server.clone());

        // "clients" aliases the existing "Clients" case-insensitively: returns
        // the EXISTING label and creates no new mailbox.
        let label = provider.create_label("clients").await.expect("idempotent create");
        assert_eq!(label.id, "lf:Clients", "returns the existing label id");
        assert!(!server.lock().unwrap().folders.contains_key("Labels/clients"), "no new mailbox");
        assert_eq!(server.lock().unwrap().count_starting("CREATE"), 0, "no CREATE issued");
    }

    #[tokio::test]
    async fn create_label_rejects_hostile_names_delimiter_and_over_length() {
        let server = server_with_label_folder();
        let (provider, _store) = provider_with(server.clone());

        for bad in [
            "",                 // empty
            "   ",              // whitespace only
            "a\rb",             // control (CR)
            "a\nb",             // control (LF)
            "Clients/Sub",      // contains the delimiter
            "/leading",         // leading delimiter
            "trailing/",        // trailing delimiter
        ] {
            let err = provider.create_label(bad).await.unwrap_err();
            assert!(matches!(err, ProviderError::InvalidOperation(_)), "{bad:?} -> {err:?}");
        }
        // Length: below / exact / above MAX_LABEL_NAME_CHARS.
        let max = super::super::policy::MAX_LABEL_NAME_CHARS;
        let below = "x".repeat(max - 1);
        let exact = "x".repeat(max);
        let above = "x".repeat(max + 1);
        assert!(provider.create_label(&below).await.is_ok(), "below the limit");
        assert!(provider.create_label(&exact).await.is_ok(), "exactly at the limit");
        let err = provider.create_label(&above).await.unwrap_err();
        assert!(matches!(err, ProviderError::InvalidOperation(_)), "above the limit must reject");
        // No mailbox was created for the over-length name.
        assert!(!server.lock().unwrap().folders.contains_key(&format!("Labels/{above}")));
    }

    #[tokio::test]
    async fn create_label_creates_a_missing_container_then_retries() {
        // The server has no Labels container child yet, and CREATE Labels/New
        // answers TRYCREATE once (missing parent). create_label then creates
        // the container and retries, succeeding.
        let mut s = MutServer::new();
        s.folder("INBOX", 100);
        s.create_trycreate_once = Some("Labels/New".into());
        let server = Arc::new(Mutex::new(s));
        let (provider, _store) = provider_with(server.clone());

        let label = provider.create_label("New").await.expect("create after container");
        assert_eq!(label.id, "lf:New");
        let g = server.lock().unwrap();
        // Both the container and the label mailbox exist now.
        assert!(g.folders.contains_key("Labels"), "container created");
        assert!(g.folders.contains_key("Labels/New"), "label mailbox created on retry");
        // Two CREATE lines reached the wire (the failed label, then container +
        // retried label) — at least the container + successful label.
        assert!(g.count_starting("CREATE") >= 2, "{:?}", g.wire());
    }

    // --- delete a label: allowed, refused (sole copy), refused (mismatch) --

    #[tokio::test]
    async fn delete_label_allowed_when_every_message_has_another_copy() {
        let server = server_with_label_folder();
        {
            let mut g = server.lock().unwrap();
            g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
            g.folder("Labels/Clients", 600).add(&["\\Seen"], &message("<m1@x>"));
        }
        let (provider, store) = provider_with(server.clone());
        let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);
        store
            .upsert_location(&ImapLocation {
                mailbox: "Labels/Clients".into(), uidvalidity: 600, uid: 1,
                message_id: id.clone(), flags_json: r#"["\\Seen"]"#.into(), modseq: None,
            })
            .unwrap();

        provider.delete_label("lf:Clients").await.expect("delete allowed");
        // The mailbox is gone on the server, the catalog row retired, the
        // label copy's local location removed; INBOX untouched.
        assert!(!server.lock().unwrap().folders.contains_key("Labels/Clients"));
        assert!(!store.mailboxes().unwrap().iter().any(|m| m.name == "Labels/Clients"));
        assert_eq!(local_mailboxes_of(&store, &id), vec!["INBOX".to_string()]);
        // DELETE was issued, and INBOX was selected first (never DELETE the
        // selected mailbox).
        let g = server.lock().unwrap();
        assert_eq!(g.count_starting("DELETE"), 1);
        let delete_pos = g.wire().iter().position(|c| c.to_ascii_uppercase().starts_with("DELETE")).unwrap();
        assert!(
            g.wire()[..delete_pos].iter().any(|c| c.to_ascii_uppercase().starts_with("SELECT \"INBOX\"")),
            "INBOX selected before DELETE: {:?}", g.wire()
        );
    }

    #[tokio::test]
    async fn delete_label_refused_when_a_message_has_no_other_copy() {
        let server = server_with_label_folder();
        server.lock().unwrap().folder("Labels/Clients", 600).add(&["\\Seen"], &message("<m1@x>"));
        let (provider, store) = provider_with(server.clone());
        // The ONLY copy of m1 is in the label mailbox.
        let _id = seed_loc(&store, &provider, "m1", "imap:t:m1", "Labels/Clients", 600, 1, &["\\Seen"]);

        let err = provider.delete_label("lf:Clients").await.unwrap_err();
        match err {
            ProviderError::InvalidOperation(m) => assert!(m.contains("no other synced copy"), "{m}"),
            other => panic!("expected permanent InvalidOperation, got {other:?}"),
        }
        // Nothing deleted on the server.
        assert!(server.lock().unwrap().folders.contains_key("Labels/Clients"));
        assert_eq!(server.lock().unwrap().count_starting("DELETE"), 0);
    }

    #[tokio::test]
    async fn delete_label_refused_when_server_count_exceeds_local_count() {
        // The server's label mailbox holds TWO messages but the local store
        // knows only one — unsynced mail we cannot vouch for, so refuse.
        let server = server_with_label_folder();
        {
            let mut g = server.lock().unwrap();
            g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
            g.folder("Labels/Clients", 600).add(&["\\Seen"], &message("<m1@x>")); // uid 1 (known)
            g.folder("Labels/Clients", 600).add(&["\\Seen"], &message("<unsynced@x>")); // uid 2 (unknown locally)
        }
        let (provider, store) = provider_with(server.clone());
        let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &["\\Seen"]);
        store
            .upsert_location(&ImapLocation {
                mailbox: "Labels/Clients".into(), uidvalidity: 600, uid: 1,
                message_id: id.clone(), flags_json: r#"["\\Seen"]"#.into(), modseq: None,
            })
            .unwrap();

        let err = provider.delete_label("lf:Clients").await.unwrap_err();
        match err {
            ProviderError::InvalidOperation(m) => assert!(m.contains("unsynced"), "{m}"),
            other => panic!("expected permanent InvalidOperation, got {other:?}"),
        }
        assert!(server.lock().unwrap().folders.contains_key("Labels/Clients"), "nothing deleted");
        assert_eq!(server.lock().unwrap().count_starting("DELETE"), 0);
    }

    #[tokio::test]
    async fn delete_label_already_absent_is_idempotent_success() {
        let server = server_with_label_folder();
        let (provider, _store) = provider_with(server.clone());
        // A label with no catalog row: deleting it is success, no command.
        provider.delete_label("lf:DoesNotExist").await.expect("idempotent absent delete");
        assert_eq!(server.lock().unwrap().count_starting("DELETE"), 0);
        assert_eq!(server.lock().unwrap().count_starting("EXAMINE"), 0);
    }

    // --- rename (update_label) is refused permanently ----------------------

    #[tokio::test]
    async fn update_label_rename_is_refused_permanently_with_a_reason() {
        let server = server_with_label_folder();
        let (provider, _store) = provider_with(server.clone());

        let err = provider.update_label("lf:Clients", "Customers").await.unwrap_err();
        assert!(!err.retry_mutation(), "a rename refusal is permanent");
        match err {
            ProviderError::InvalidOperation(m) => {
                assert!(m.to_ascii_lowercase().contains("renaming"), "{m}");
                assert!(m.contains("derived from its name"), "names the reason: {m}");
            }
            other => panic!("expected permanent InvalidOperation, got {other:?}"),
        }
        // Nothing touched the server (no RENAME, the label mailbox is intact).
        let g = server.lock().unwrap();
        assert_eq!(g.count_starting("RENAME"), 0);
        assert!(g.folders.contains_key("Labels/Clients"));
    }

    // --- add label then a following sweep: no change, no duplicate ---------

    #[tokio::test]
    async fn after_add_label_a_following_sweep_sees_no_change_and_no_duplicate() {
        let server = server_with_label_folder();
        server.lock().unwrap().folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
        let (provider, store) = provider_with(server.clone());

        // Baseline so INBOX's m1 is ingested with its id/thread.
        provider.baseline_cursor().await.unwrap();
        let id = store.locations_in_mailbox("INBOX").unwrap()[0].message_id.clone();

        provider.modify_messages(&[id.clone()], &["lf:Clients".into()], &[]).await.expect("add label");
        let copies_before = store.locations_for_message(&id).unwrap().len();
        assert_eq!(copies_before, 2, "INBOX + Labels/Clients");
        let gen_after = store.generation().unwrap();

        // A following full refresh over the same server state must not
        // duplicate the labelled copy nor flap labels.
        let cursor = SyncCursor::from_generation(gen_after);
        for _ in 0..4 {
            let _ = provider.poll(&cursor).await.unwrap();
        }
        assert_eq!(
            store.locations_for_message(&id).unwrap().len(),
            2,
            "exactly two locations after the sweep (no duplicate label copy)"
        );
    }

    #[tokio::test]
    async fn delete_label_refuses_a_uidvalidity_reset_even_when_count_and_uids_match() {
        let (server, provider, _store, _id) = paired_label_message();
        server
            .lock()
            .unwrap()
            .folder("Labels/Clients", 600)
            .uidvalidity = 601;
        assert!(provider.delete_label("lf:Clients").await.is_err());
        assert_eq!(server.lock().unwrap().count_starting("DELETE"), 0);
    }

    #[tokio::test]
    async fn remove_label_catches_a_silently_ignored_permanent_deleted_flag() {
        let (server, provider, store, id) = paired_label_message();
        server.lock().unwrap().silently_ignore_flag = Some("\\Deleted".into());
        let err = provider
            .modify_messages(&[id.clone()], &[], &["lf:Clients".into()])
            .await
            .unwrap_err();
        assert!(
            matches!(err, ProviderError::InvalidOperation(ref text) if text.contains("silently ignored"))
        );
        assert_eq!(server.lock().unwrap().count_starting("UID EXPUNGE"), 0);
        assert_eq!(uids_in(&server, "Labels/Clients"), vec![1]);
        assert!(local_mailboxes_of(&store, &id).contains(&"Labels/Clients".into()));
    }

    #[tokio::test]
    async fn remove_label_keeps_its_location_until_expunge_really_removes_the_copy() {
        let (server, provider, store, id) = paired_label_message();
        server.lock().unwrap().ignore_expunge = true;
        let err = provider
            .modify_messages(&[id.clone()], &[], &["lf:Clients".into()])
            .await
            .unwrap_err();
        assert!(err.retry_mutation());
        assert_eq!(uids_in(&server, "Labels/Clients"), vec![1]);
        assert!(local_mailboxes_of(&store, &id).contains(&"Labels/Clients".into()));
    }

    #[tokio::test]
    async fn fallback_move_requires_deleted_before_copy_and_verifies_it_before_expunge() {
        for supported in [false, true] {
            let server = server_with_system_mailboxes();
            {
                let mut g = server.lock().unwrap();
                g.capabilities = vec!["IMAP4REV1".into(), "UIDPLUS".into()];
                g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
                g.silently_ignore_flag = Some("\\Deleted".into());
                if !supported {
                    g.folder("INBOX", 100).permanent_flags = vec!["\\Seen".into()];
                }
            }
            let (provider, store) = provider_with(server.clone());
            let id = seed_loc(
                &store,
                &provider,
                "m1",
                "imap:t:m1",
                "INBOX",
                100,
                1,
                &["\\Seen"],
            );
            assert!(provider
                .modify_thread("imap:t:m1", &[], &["INBOX".into()])
                .await
                .is_err());
            let g = server.lock().unwrap();
            assert_eq!(g.count_starting("UID COPY"), usize::from(supported));
            assert_eq!(g.count_starting("UID EXPUNGE"), 0);
            drop(g);
            assert_eq!(uids_in(&server, "INBOX"), vec![1]);
            assert!(local_mailboxes_of(&store, &id).contains(&"INBOX".into()));
        }
    }

    #[tokio::test]
    async fn encoded_labels_round_trip_creation_catalog_flags_removal_and_deletion() {
        for (name, wire) in [
            ("R&D", "R&-D"),
            ("Café", "Caf&AOk-"),
            ("日本語", "&ZeVnLIqe-"),
        ] {
            let server = server_with_label_folder();
            server
                .lock()
                .unwrap()
                .folder("INBOX", 100)
                .add(&[], &message("<m1@x>"));
            let (provider, store) = provider_with(server.clone());
            let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &[]);
            let label = provider.create_label(name).await.unwrap();
            assert_eq!(label.id, format!("lf:{wire}"));
            assert_eq!(label.name, name);
            assert!(provider
                .list_labels()
                .await
                .unwrap()
                .iter()
                .any(|item| item.id == label.id && item.name == label.name));
            provider
                .modify_messages(&[id.clone()], &[label.id.clone()], &[])
                .await
                .unwrap();
            provider
                .modify_messages(&[id.clone()], &[], &["UNREAD".into()])
                .await
                .unwrap();
            assert!(server
                .lock()
                .unwrap()
                .flags_of(&format!("Labels/{wire}"), 1)
                .contains(&"\\Seen".into()));
            provider
                .modify_messages(&[id], &[], &[label.id.clone()])
                .await
                .unwrap();
            provider.delete_label(&label.id).await.unwrap();
            assert!(!server
                .lock()
                .unwrap()
                .folders
                .contains_key(&format!("Labels/{wire}")));
        }
    }

    #[tokio::test]
    async fn existing_wire_label_ids_and_encoded_container_are_preserved() {
        let server = server_with_inbox();
        {
            let mut g = server.lock().unwrap();
            g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
            g.folder("&ZeVnLIqe-/Caf&AOk-", 600);
        }
        let (mut provider, store) = provider_with(server.clone());
        provider.settings.label_container = Some("&ZeVnLIqe-".into());
        let id = seed_loc(
            &store,
            &provider,
            "m1",
            "imap:t:m1",
            "INBOX",
            100,
            1,
            &["\\Seen"],
        );
        let label = provider.create_label("café").await.unwrap();
        assert_eq!(label.id, "lf:Caf&AOk-");
        assert_eq!(label.name, "Café");
        assert_eq!(server.lock().unwrap().count_starting("CREATE"), 0);
        provider
            .modify_messages(&[id.clone()], &[label.id.clone()], &[])
            .await
            .unwrap();
        provider
            .modify_messages(&[id], &[], &[label.id.clone()])
            .await
            .unwrap();
        provider.delete_label(&label.id).await.unwrap();
    }

    #[tokio::test]
    async fn fallback_move_recovers_a_lost_copy_response_without_a_second_copy() {
        let server = server_with_system_mailboxes();
        {
            let mut g = server.lock().unwrap();
            g.capabilities = vec!["IMAP4REV1".into(), "UIDPLUS".into()];
            g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
            g.drop_copy_response_once = true;
        }
        let (provider, store) = provider_with(server.clone());
        let id = seed_loc(
            &store,
            &provider,
            "m1",
            "imap:t:m1",
            "INBOX",
            100,
            1,
            &["\\Seen"],
        );
        let err = provider
            .modify_thread("imap:t:m1", &[], &["INBOX".into()])
            .await
            .unwrap_err();
        assert!(err.retry_mutation());
        assert!(store
            .pending_copy_identity(&id, "Archive")
            .unwrap()
            .is_some());
        let restarted = ImapProvider {
            source: Arc::new(MutSource {
                server: server.clone(),
            }),
            ..provider
        };
        restarted
            .modify_thread("imap:t:m1", &[], &["INBOX".into()])
            .await
            .unwrap();
        assert_eq!(server.lock().unwrap().count_starting("UID COPY"), 1);
        assert_eq!(uids_in(&server, "Archive").len(), 1);
        assert!(uids_in(&server, "INBOX").is_empty());
        assert_eq!(local_mailboxes_of(&store, &id), vec!["Archive"]);
        assert_eq!(store.pending_copy_identity(&id, "Archive").unwrap(), None);
    }

    #[tokio::test]
    async fn copy_without_copyuid_reconciles_before_reporting_success() {
        let server = server_with_label_folder();
        {
            let mut g = server.lock().unwrap();
            g.capabilities = vec!["IMAP4REV1".into()];
            g.omit_copyuid = true;
            g.folder("INBOX", 100).add(&[], &message("<m1@x>"));
        }
        let (provider, store) = provider_with(server.clone());
        let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &[]);
        provider
            .modify_messages(&[id.clone()], &["lf:Clients".into()], &[])
            .await
            .unwrap();
        provider
            .modify_messages(&[id.clone()], &["lf:Clients".into()], &[])
            .await
            .unwrap();
        assert_eq!(server.lock().unwrap().count_starting("UID COPY"), 1);
        assert_eq!(
            local_mailboxes_of(&store, &id),
            vec!["INBOX", "Labels/Clients"]
        );
        assert_eq!(
            store.pending_copy_identity(&id, "Labels/Clients").unwrap(),
            None
        );
        assert!(
            server
                .lock()
                .unwrap()
                .flags_of("Labels/Clients", 1)
                .is_empty(),
            "PEEK must not set Seen"
        );
    }

    #[tokio::test]
    async fn an_intent_saved_before_copy_can_retry_when_no_copy_landed() {
        let server = server_with_label_folder();
        server
            .lock()
            .unwrap()
            .folder("INBOX", 100)
            .add(&[], &message("<m1@x>"));
        let (provider, store) = provider_with(server.clone());
        let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &[]);
        let identity = crate::provider::imap::derive_message_id(
            store.account_id(),
            &crate::provider::imap::IdentityInputs {
                message_id: Some("<m1@x>".into()),
                ..Default::default()
            },
        );
        store.begin_copy(&id, "Labels/Clients", &identity).unwrap();
        provider
            .modify_messages(&[id.clone()], &["lf:Clients".into()], &[])
            .await
            .unwrap();
        assert_eq!(server.lock().unwrap().count_starting("UID COPY"), 1);
        assert_eq!(uids_in(&server, "Labels/Clients").len(), 1);
        assert_eq!(
            store.pending_copy_identity(&id, "Labels/Clients").unwrap(),
            None
        );
    }

    fn paired_label_message() -> (Arc<Mutex<MutServer>>, ImapProvider, ImapStateStore, String) {
        let server = server_with_label_folder();
        {
            let mut g = server.lock().unwrap();
            g.folder("INBOX", 100).add(&["\\Seen"], &message("<m1@x>"));
            g.folder("Labels/Clients", 600)
                .add(&["\\Seen"], &message("<m1@x>"));
        }
        let (provider, store) = provider_with(server.clone());
        let id = seed_loc(
            &store,
            &provider,
            "m1",
            "imap:t:m1",
            "INBOX",
            100,
            1,
            &["\\Seen"],
        );
        store
            .upsert_location(&ImapLocation {
                mailbox: "Labels/Clients".into(),
                uidvalidity: 600,
                uid: 1,
                message_id: id.clone(),
                flags_json: r#"["\\Seen"]"#.into(),
                modseq: None,
            })
            .unwrap();
        (server, provider, store, id)
    }

    #[tokio::test]
    async fn delete_label_must_compare_uids_not_just_count() {
        let (server, provider, _store, _id) = paired_label_message();
        {
            let mut g = server.lock().unwrap();
            let f = g.folder("Labels/Clients", 600);
            f.messages.clear();
            f.add(&["\\Seen"], &message("<unsynced-only-copy@x>"));
        }
        let result = provider.delete_label("lf:Clients").await;
        assert!(result.is_err(), "must not delete a different unsynced message just because EXISTS still equals 1; got {result:?}");
        assert!(server
            .lock()
            .unwrap()
            .folders
            .contains_key("Labels/Clients"));
    }

    #[tokio::test]
    async fn remove_label_must_check_and_verify_deleted_flag() {
        let (server, provider, store, id) = paired_label_message();
        {
            let mut g = server.lock().unwrap();
            g.folder("Labels/Clients", 600).permanent_flags = vec!["\\Seen".into()];
            g.silently_ignore_flag = Some("\\Deleted".into());
        }
        let result = provider
            .modify_messages(&[id.clone()], &[], &["lf:Clients".into()])
            .await;
        assert!(result.is_err(), "must not report a successful removal when Deleted is unsupported and the server retained the message; got {result:?}; server UIDs {:?}; local copies {:?}", uids_in(&server, "Labels/Clients"), local_mailboxes_of(&store, &id));
    }

    #[tokio::test]
    async fn created_ampersand_label_must_be_removable() {
        let server = server_with_label_folder();
        server
            .lock()
            .unwrap()
            .folder("INBOX", 100)
            .add(&["\\Seen"], &message("<m1@x>"));
        let (provider, store) = provider_with(server.clone());
        let id = seed_loc(
            &store,
            &provider,
            "m1",
            "imap:t:m1",
            "INBOX",
            100,
            1,
            &["\\Seen"],
        );
        let label = provider.create_label("R&D").await.unwrap();
        provider
            .modify_messages(&[id.clone()], &[label.id.clone()], &[])
            .await
            .unwrap();
        assert_eq!(uids_in(&server, "Labels/R&-D").len(), 1);
        let result = provider.modify_messages(&[id], &[], &[label.id]).await;
        assert!(
            result.is_ok(),
            "created label must round-trip through SELECT when removing; got {result:?}; wire {:?}",
            server.lock().unwrap().wire()
        );
    }

    #[tokio::test]
    async fn copy_response_loss_must_not_duplicate_on_retry() {
        for raw in [
            message("<m1@x>"),
            message("<m1@x>").replace("Message-ID: <m1@x>\r\n", ""),
        ] {
            let server = server_with_label_folder();
            {
                let mut g = server.lock().unwrap();
                g.folder("INBOX", 100).add(&[], &raw);
                g.drop_copy_response_once = true;
            }
            let (provider, store) = provider_with(server.clone());
            let id = seed_loc(&store, &provider, "m1", "imap:t:m1", "INBOX", 100, 1, &[]);
            let first = provider
                .modify_messages(&[id.clone()], &["lf:Clients".into()], &[])
                .await
                .unwrap_err();
            assert!(first.retry_mutation(), "{first:?}");
            assert_eq!(uids_in(&server, "Labels/Clients").len(), 1);
            assert!(store
                .pending_copy_identity(&id, "Labels/Clients")
                .unwrap()
                .is_some());
            provider
                .modify_messages(&[id.clone()], &["lf:Clients".into()], &[])
                .await
                .unwrap();
            assert_eq!(server.lock().unwrap().count_starting("UID COPY"), 1);
            assert_eq!(
                uids_in(&server, "Labels/Clients").len(),
                1,
                "retry must recover both Message-ID and synthetic identity copies"
            );
            assert_eq!(
                store.pending_copy_identity(&id, "Labels/Clients").unwrap(),
                None
            );
        }
    }

    #[tokio::test]
    async fn remove_label_preserves_last_server_copy_when_backup_was_expunged() {
        let (server, provider, _store, id) = paired_label_message();
        server.lock().unwrap().folder("INBOX", 100).messages.clear();
        let result = provider
            .modify_messages(&[id], &[], &["lf:Clients".into()])
            .await;
        assert!(matches!(result, Err(ProviderError::InvalidOperation(_))));
        assert_eq!(
            uids_in(&server, "Labels/Clients"),
            vec![1],
            "stale local INBOX location is not evidence of a surviving server copy"
        );
    }
    #[tokio::test]
    async fn delete_label_preserves_last_server_copy_when_backup_was_expunged() {
        let (server, provider, _store, _id) = paired_label_message();
        server.lock().unwrap().folder("INBOX", 100).messages.clear();
        let result = provider.delete_label("lf:Clients").await;
        assert!(matches!(result, Err(ProviderError::InvalidOperation(_))));
        assert!(
            server
                .lock()
                .unwrap()
                .folders
                .contains_key("Labels/Clients"),
            "must not delete a label containing the only remaining server copy"
        );
    }

    #[tokio::test]
    async fn removing_or_deleting_a_label_refuses_stale_or_deleted_backup_coordinates() {
        for delete_mailbox in [false, true] {
            for scenario in ["uidvalidity", "deleted", "missing"] {
                let (server, provider, store, id) = paired_label_message();
                {
                    let mut server = server.lock().unwrap();
                    match scenario {
                        "uidvalidity" => server.folder("INBOX", 100).uidvalidity = 101,
                        "deleted" => server.folder("INBOX", 100).messages[0]
                            .flags
                            .push("\\Deleted".into()),
                        "missing" => {
                            server.folders.remove("INBOX");
                        }
                        _ => unreachable!(),
                    }
                }
                let result = if delete_mailbox {
                    provider.delete_label("lf:Clients").await
                } else {
                    provider
                        .modify_messages(&[id.clone()], &[], &["lf:Clients".into()])
                        .await
                };
                assert!(
                    matches!(result, Err(ProviderError::InvalidOperation(_))),
                    "{scenario}: {result:?}"
                );
                assert_eq!(uids_in(&server, "Labels/Clients"), vec![1]);
                assert_eq!(local_count_in(&store, "Labels/Clients"), 1);
                let server = server.lock().unwrap();
                for command in ["UID STORE", "UID EXPUNGE", "DELETE"] {
                    assert_eq!(server.count_starting(command), 0, "{scenario}: {command}");
                }
            }
        }
    }

    #[tokio::test]
    async fn label_removal_can_use_a_live_archive_backup_despite_a_stale_inbox_copy() {
        for delete_mailbox in [false, true] {
            let (server, provider, store, id) = paired_label_message();
            {
                let mut server = server.lock().unwrap();
                server.folder("INBOX", 100).messages.clear();
                server.folder("Archive", 200).add(&[], &message("<m1@x>"));
            }
            store
                .upsert_location(&ImapLocation {
                    mailbox: "Archive".into(),
                    uidvalidity: 200,
                    uid: 1,
                    message_id: id.clone(),
                    flags_json: "[]".into(),
                    modseq: None,
                })
                .unwrap();
            if delete_mailbox {
                provider.delete_label("lf:Clients").await.unwrap();
            } else {
                provider
                    .modify_messages(&[id], &[], &["lf:Clients".into()])
                    .await
                    .unwrap();
            }
            assert_eq!(uids_in(&server, "Archive"), vec![1]);
            assert!(
                server.lock().unwrap().folder("Archive", 200).messages[0]
                    .flags
                    .is_empty(),
                "checking a survivor must not mark it read"
            );
            assert_eq!(local_count_in(&store, "Labels/Clients"), 0);
        }
    }

    #[tokio::test]
    async fn label_removal_preserves_transient_backup_check_failures_without_writing() {
        let (server, provider, _store, id) = paired_label_message();
        server.lock().unwrap().folder("INBOX", 100).select_busy = true;
        let result = provider
            .modify_messages(&[id], &[], &["lf:Clients".into()])
            .await;
        assert!(
            matches!(result, Err(ProviderError::TransientTransport(_))),
            "{result:?}"
        );
        assert_eq!(uids_in(&server, "Labels/Clients"), vec![1]);
        let server = server.lock().unwrap();
        assert_eq!(server.count_starting("UID STORE"), 0);
        assert_eq!(server.count_starting("UID EXPUNGE"), 0);
    }

    #[tokio::test]
    async fn label_removal_checks_every_message_before_destroying_any_copy() {
        for delete_mailbox in [false, true] {
            let (server, provider, store, first_id) = paired_label_message();
            {
                let mut server = server.lock().unwrap();
                // The second INBOX location is stale, but its label copy is
                // live. A safe first message must not cause a partial delete.
                server
                    .folder("Labels/Clients", 600)
                    .add(&[], &message("<m2@x>"));
            }
            let second_id = seed_loc(&store, &provider, "m2", "imap:t:m2", "INBOX", 100, 2, &[]);
            store
                .upsert_location(&ImapLocation {
                    mailbox: "Labels/Clients".into(),
                    uidvalidity: 600,
                    uid: 2,
                    message_id: second_id.clone(),
                    flags_json: "[]".into(),
                    modseq: None,
                })
                .unwrap();
            let result = if delete_mailbox {
                provider.delete_label("lf:Clients").await
            } else {
                provider
                    .modify_messages(&[first_id, second_id], &[], &["lf:Clients".into()])
                    .await
            };
            assert!(
                matches!(result, Err(ProviderError::InvalidOperation(_))),
                "{result:?}"
            );
            assert_eq!(uids_in(&server, "Labels/Clients"), vec![1, 2]);
            let server = server.lock().unwrap();
            for command in ["UID STORE", "UID EXPUNGE", "DELETE"] {
                assert_eq!(server.count_starting(command), 0);
            }
        }
    }

    #[tokio::test]
    async fn server_saves_sent_override_skips_missing_mailbox() {
        let server = server_with_inbox();
        let (mut provider, _store) = provider_with(server.clone());
        provider.settings.server_saves_sent = true;
        let result = provider
            .save_sent_copy(&crate::provider::SentCopyRequest {
                raw: b"Message-ID: <override@x>\r\n\r\nbody".to_vec(),
                outbox_id: "override-test".into(),
                append_attempted: false,
                phase: crate::provider::SentCopyPhase::Pending,
            })
            .await;
        assert!(
            matches!(result, Ok(crate::provider::SentCopyStep::Done)),
            "explicit override must skip Sent-folder requirements: {result:?}"
        );
        assert_eq!(server.lock().unwrap().commands.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn server_owned_sent_copy_needs_neither_message_id_nor_an_imap_session() {
        for explicit_override in [false, true] {
            let server = server_with_inbox();
            let (mut provider, store) = provider_with(server.clone());
            if explicit_override {
                provider.settings.server_saves_sent = true;
            } else {
                store.set_sent_copy_detection(true, 1).unwrap();
            }
            for phase in [
                crate::provider::SentCopyPhase::Pending,
                crate::provider::SentCopyPhase::Verify,
            ] {
                let result = provider
                    .save_sent_copy(&crate::provider::SentCopyRequest {
                        raw: b"Subject: no message id\r\n\r\nbody".to_vec(),
                        outbox_id: "server-owned-sent-copy".into(),
                        append_attempted: false,
                        phase,
                    })
                    .await;
                assert!(
                    matches!(result, Ok(crate::provider::SentCopyStep::Done)),
                    "{result:?}"
                );
            }
            assert!(server.lock().unwrap().wire().is_empty());
        }
    }
}
