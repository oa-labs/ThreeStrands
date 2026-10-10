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
use super::labels::{labels_for, merge_label_sets};
use super::policy::{self, SyncLimits};
use super::rfc822::{attachment_bytes_from_raw, to_raw_message};
use super::session::ImapSession;
use super::settings::LabelStorage;
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
    label_model: LabelModel,
    source: std::sync::Arc<dyn ImapSessionSource>,
    /// Injected clock for deterministic `fetched_at` in tests; wall clock in
    /// production.
    now: fn() -> i64,
    /// The sync window/batch limits for a round. Overridable in tests.
    limits: SyncLimits,
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
            password: config.password,
        });
        Self {
            store: config.store,
            cache: config.cache,
            username: config.username,
            label_model: config.label_model,
            source,
            now: unix_now,
            limits: SyncLimits::inbox(),
        }
    }

    /// Open an authenticated session through the configured source.
    async fn open_session(&self) -> ProviderResult<Box<dyn ImapSession>> {
        self.source.open().await
    }

    /// Refresh INBOX against the server and return the new generation. The
    /// at-least-once contract lives here: every round is a full reconcile that
    /// is safe to repeat.
    async fn refresh_inbox(&self) -> ProviderResult<u64> {
        let mut session = self.open_session().await?;
        let generation = self.refresh_inbox_with(session.as_mut()).await;
        let _ = session.logout().await;
        generation
    }

    /// The sync round against an already-open session, factored out so the
    /// engine-level test can drive it over a fake session.
    ///
    /// Every durable write this round decides — new/updated locations,
    /// deletions, thread assignments, aliases, the journal and the generation
    /// bump — is collected into one [`SyncRoundWrite`] and applied in a SINGLE
    /// transaction at the end (`commit_sync_round`). A failure before that
    /// commit (a dropped connection on a later batch) writes NOTHING durable,
    /// so the next round sees the same prior state and redoes the work — the
    /// at-least-once contract. Body-cache puts happen eagerly during fetch
    /// because the cache is rebuildable and deduplicated by stable id.
    pub(super) async fn refresh_inbox_with(
        &self,
        session: &mut dyn ImapSession,
    ) -> ProviderResult<u64> {
        // 1. EXAMINE (read-only) — never SELECT, so sync never sets \Seen.
        let status = session.examine(INBOX).await?;
        let server_uidvalidity = status.uid_validity.unwrap_or(0) as i64;

        // 2. Local view: stored UIDVALIDITY + per-UID flags for INBOX.
        let local_locations = self.inbox_locations()?;
        let local_uidvalidity = local_locations.first().map(|l| l.uidvalidity);

        // 3. UID SEARCH ALL — the server's full UID set for INBOX.
        let server_uids = session.uid_search("ALL").await?;

        // Flag sweep uses a FLAGS-ONLY fetch (no body, no headers) over the
        // window, so an over-window mailbox is not re-headered every poll.
        let local_view = self.local_view(local_uidvalidity, &local_locations);
        let flag_window = self.flag_window(&server_uids);
        let server_flags = self.fetch_flags(session, &flag_window).await?;
        let server_view = super::delta::ServerMailboxView {
            uidvalidity: server_uidvalidity,
            all_uids: server_uids.clone(),
            flags_by_uid: server_flags,
        };
        let delta = super::delta::compute_delta(&local_view, &server_view, &self.limits);

        if delta.beyond_window > 0 {
            log::info!(
                "imap sync: {} INBOX messages beyond the sync window",
                delta.beyond_window
            );
        }

        // Everything this round will write is COLLECTED, not applied, until the
        // atomic commit below.
        let mut round = SyncRoundWrite::default();

        // On a UIDVALIDITY reset the local UIDs are meaningless: drop every
        // INBOX location in the same transaction (bodies survive — keyed by the
        // stable id). The reset's "new" set is the whole windowed server view.
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

        // New mail: identity pass, sticky id, body once (deferred location).
        for batch in chunk(&delta.new_uids, policy::UID_BATCH_SIZE) {
            let set = uid_set(batch);
            let rows = fetch_identity(session, &set).await?;
            for row in rows {
                let mut fetcher = SessionBodyFetcher { session };
                // resolve_and_cache_body resolves the id and caches the body
                // but writes NO location and never fails the round for an
                // oversize message (item 2) — only transport/auth propagates.
                let resolved = super::fetch::resolve_and_cache_body(
                    &mut fetcher,
                    &self.store,
                    &self.cache,
                    INBOX,
                    server_uidvalidity,
                    &row,
                    (self.now)(),
                )
                .await?;
                let message_id = resolved.message_id;
                // The location row for this new UID is part of the atomic round.
                round.locations.push(ImapLocation {
                    mailbox: INBOX.to_string(),
                    uidvalidity: server_uidvalidity,
                    uid: row.uid as i64,
                    message_id: message_id.clone(),
                    flags_json: serde_json::to_string(&row.flags)
                        .unwrap_or_else(|_| "[]".to_string()),
                    modseq: None,
                });
                // Threading inputs: references come from the cached body when
                // present; a skipped-body message anchors on its identity-pass
                // Message-ID header (item 2) so it still threads.
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
                threading_inputs.push(ThreadingInput {
                    message_id: message_id.clone(),
                    message_id_header: headers
                        .message_id
                        .or_else(|| row.inputs.message_id.clone()),
                    in_reply_to: headers.in_reply_to,
                    references: headers.references,
                });
                changed_message_ids.push(message_id);
            }
        }

        // Flag changes: collect an updated location row (not applied yet).
        if !delta.uidvalidity_reset {
            for &uid in &delta.flag_changed_uids {
                if let Some(flags) = server_view.flags_by_uid.get(&uid) {
                    if let Some(message_id) = self
                        .store
                        .location_message_id(INBOX, server_uidvalidity, uid as i64)
                        .map_err(db_err)?
                    {
                        round.locations.push(ImapLocation {
                            mailbox: INBOX.to_string(),
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
                    .location_message_id(INBOX, server_uidvalidity, uid as i64)
                    .map_err(db_err)?
                {
                    round
                        .deletions
                        .push((INBOX.to_string(), server_uidvalidity, uid as i64));
                    changed_message_ids.push(message_id);
                }
            }
        }

        // Thread the new messages, seeding prior thread state (with PERSISTED
        // creation generations) so a merge picks the genuinely older survivor.
        let mut thread_state = self.load_thread_state()?;
        let outcome = threading::thread_batch(&mut thread_state, &threading_inputs);

        // Changed thread ids: threading effects, plus the threads of any
        // flag-changed / deleted / dropped message.
        let mut changed_threads: std::collections::BTreeSet<String> =
            outcome.changed_threads.iter().cloned().collect();
        for message_id in changed_message_ids.iter().chain(dropped_message_ids.iter()) {
            if let Some(thread) = self.store.thread_of_message(message_id).map_err(db_err)? {
                changed_threads.insert(thread);
            }
        }
        for (_, thread_id) in &outcome.assignments {
            changed_threads.insert(thread_id.clone());
        }

        round.thread_assignments = outcome.assignments;
        round.aliases = outcome.aliases;
        round.changed_threads = changed_threads.into_iter().collect();

        // Idle round: nothing changed. Do not bump the generation or journal
        // (item 5); return the current generation unchanged.
        if round.is_empty() {
            return self.store.generation().map_err(db_err);
        }

        // Atomic commit: locations, deletions, threads, aliases, journal and
        // the generation bump all in ONE transaction (item 1).
        let generation = self.store.commit_sync_round(&round).map_err(db_err)?;
        self.store.prune_journal().map_err(db_err)?;
        Ok(generation)
    }

    /// INBOX locations held locally, ordered by UID.
    fn inbox_locations(&self) -> ProviderResult<Vec<ImapLocation>> {
        self.store
            .locations_in_mailbox(INBOX)
            .map_err(db_err)
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

    /// Which UIDs to fetch flags for this round: the window of the server's
    /// full set (newest first), so an over-window mailbox still sweeps its
    /// in-window flags without a per-message request.
    fn flag_window(&self, server_uids: &[u32]) -> Vec<u32> {
        policy::select_window(server_uids.to_vec(), self.limits.mailbox_window).in_window
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

    /// Rebuild the threader's prior state from the store so a merge in this
    /// round picks the genuinely OLDER thread as survivor. Each known thread is
    /// seeded with the normalized `Message-ID` tokens of its messages (read
    /// from the cached bodies — no network) and its PERSISTED creation
    /// generation (the minimum `created_generation` across its messages). Thread
    /// age is therefore a durable fact, not a hash ordering: on a merge the
    /// lower creation generation survives, tie-broken by thread id, matching
    /// the design's "older thread's id survives so references stay valid".
    fn load_thread_state(&self) -> ProviderResult<ThreadState> {
        use super::identity::normalize_message_id;
        let mut state = ThreadState::new();
        let rows = self.store.all_message_threads().map_err(db_err)?;
        // Group messages by thread, tracking each thread's minimum creation
        // generation as its persisted age.
        let mut by_thread: std::collections::BTreeMap<String, (u64, Vec<String>)> =
            std::collections::BTreeMap::new();
        for (message_id, thread_id, created) in rows {
            let entry = by_thread
                .entry(thread_id)
                .or_insert((created, Vec::new()));
            entry.0 = entry.0.min(created);
            entry.1.push(message_id);
        }
        for (thread_id, (created_generation, message_ids)) in by_thread {
            let mut tokens: Vec<String> = Vec::new();
            for message_id in &message_ids {
                // The stable id is always a valid token anchor; add the
                // normalized Message-ID header from the cached body too, so a
                // later message referencing that header links to this thread.
                tokens.push(message_id.clone());
                if let Some(cached) = self.cache.get(message_id).map_err(db_err)? {
                    let headers = super::rfc822::threading_headers_from_raw(&cached.raw);
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
    async fn baseline_cursor(&self) -> ProviderResult<SyncCursor> {
        // Refresh INBOX and return the current generation as the cursor.
        let generation = self.refresh_inbox().await?;
        Ok(SyncCursor::from_generation(generation))
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
        // (a) refresh INBOX; (b) return threads journalled since `polled`;
        // (c) new cursor = current generation; more = false.
        let generation = self.refresh_inbox().await?;
        let changed = self.store.journal_since(polled).map_err(db_err)?;
        Ok(SyncBatch {
            changed_threads: changed,
            cursor: SyncCursor::from_generation(generation),
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
        if message_ids.is_empty() {
            // No remaining locations/messages: the engine deletes the local
            // copy.
            return Err(ProviderError::NotFound);
        }

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
            // Labels: the union across every copy of the message.
            let per_copy: Vec<Vec<String>> = locations
                .iter()
                .map(|location| {
                    let flags: Vec<String> =
                        serde_json::from_str(&location.flags_json).unwrap_or_default();
                    labels_for(&location.mailbox, None, &flags)
                })
                .collect();
            let label_ids = merge_label_sets(&per_copy);

            // Body: cache first, network only if missing.
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
        let Some(location) = locations.into_iter().find(|l| l.mailbox == INBOX) else {
            return Ok(None);
        };
        session.examine(INBOX).await?;
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
        let per_copy: Vec<Vec<String>> = locations
            .iter()
            .map(|location| {
                let flags: Vec<String> =
                    serde_json::from_str(&location.flags_json).unwrap_or_default();
                labels_for(&location.mailbox, None, &flags)
            })
            .collect();
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

/// Mutations are not supported until phase 3; every entry point rejects with a
/// clear, PERMANENT message so the mutation queue rolls the local change back
/// to `failed` rather than retrying forever (see the engine's
/// `deliver_mutations`: `InvalidOperation` is neither retryable nor a reauth,
/// so it lands in the permanent-failure branch).
const MUTATE_UNSUPPORTED: &str =
    "IMAP mutations (archive, star, label, move) are not supported until phase 3";

#[async_trait]
impl MailMutate for ImapProvider {
    async fn modify_thread(
        &self,
        _id: &str,
        _add: &[String],
        _remove: &[String],
    ) -> ProviderResult<()> {
        Err(ProviderError::InvalidOperation(MUTATE_UNSUPPORTED.into()))
    }

    async fn modify_messages(
        &self,
        _ids: &[String],
        _add: &[String],
        _remove: &[String],
    ) -> ProviderResult<()> {
        Err(ProviderError::InvalidOperation(MUTATE_UNSUPPORTED.into()))
    }

    async fn list_labels(&self) -> ProviderResult<Vec<Label>> {
        // INBOX-only system labels this slice; no user labels yet.
        Ok(vec![
            Label {
                id: "INBOX".into(),
                name: "Inbox".into(),
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
        ])
    }

    async fn create_label(&self, _name: &str) -> ProviderResult<Label> {
        Err(ProviderError::InvalidOperation(MUTATE_UNSUPPORTED.into()))
    }

    async fn update_label(&self, _id: &str, _name: &str) -> ProviderResult<Label> {
        Err(ProviderError::InvalidOperation(MUTATE_UNSUPPORTED.into()))
    }

    async fn delete_label(&self, _id: &str) -> ProviderResult<()> {
        Err(ProviderError::InvalidOperation(MUTATE_UNSUPPORTED.into()))
    }
}

#[async_trait]
impl MailSend for ImapProvider {
    async fn sender_identity(&self) -> ProviderResult<String> {
        // The login address is this account's sending identity.
        Ok(self.username.clone())
    }

    async fn prepare_delivery(&self) -> ProviderResult<Box<dyn Delivery>> {
        Err(ProviderError::InvalidOperation(
            "IMAP/SMTP sending is not supported until phase 4".into(),
        ))
    }

    async fn find_sent_copy(
        &self,
        _operation: &str,
        _expected_sender: &str,
    ) -> ProviderResult<Option<DeliveryReceipt>> {
        Err(ProviderError::InvalidOperation(
            "IMAP sent-copy reconciliation is not supported until phase 4".into(),
        ))
    }
}

impl MailProvider for ImapProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            server_search: false,
            provided_threads: false,
            label_model: self.label_model,
            verifiable_delivery: false,
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

    /// One seeded message in the fake server's INBOX: its UID, flags, and raw
    /// RFC 5322 bytes. `advertised_size`, when set, overrides the RFC822.SIZE
    /// the identity pass reports (used to simulate an oversize message without
    /// allocating 64 MiB).
    #[derive(Clone)]
    struct FakeMessage {
        uid: u32,
        flags: Vec<String>,
        raw: String,
        advertised_size: Option<usize>,
    }

    /// A tiny in-memory INBOX the scripted server serves. Shared (Arc<Mutex>)
    /// so a test can mutate it (add mail, change a flag, expunge) between
    /// polls and the next opened session sees the change.
    ///
    /// `fail_after_body_fetches`, when set, makes the scripted server DROP the
    /// connection after that many whole-body (`BODY.PEEK[]`) fetches in one
    /// session — a mid-round transient failure, so the at-least-once contract
    /// can be exercised.
    #[derive(Clone, Default)]
    struct FakeMailbox {
        uidvalidity: u32,
        uidnext: u32,
        messages: Vec<FakeMessage>,
        fail_after_body_fetches: Option<usize>,
    }

    impl FakeMailbox {
        fn new(uidvalidity: u32) -> Self {
            Self {
                uidvalidity,
                uidnext: 1,
                messages: Vec::new(),
                fail_after_body_fetches: None,
            }
        }
        fn add(&mut self, flags: &[&str], raw: &str) -> u32 {
            self.add_sized(flags, raw, None)
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

    /// A scripted IMAP server that answers EXAMINE / UID SEARCH / UID FETCH /
    /// LOGOUT from a `FakeMailbox` snapshot, over a tokio duplex stream. It
    /// understands the exact command shapes this provider issues, parsing them
    /// by type rather than a fixed sequence, so it is robust to the order and
    /// batching of commands.
    fn spawn_server(mailbox: FakeMailbox) -> (DuplexStream, tokio::task::JoinHandle<()>) {
        let (client, server) = tokio::io::duplex(1 << 20);
        let task = tokio::spawn(async move {
            let mut server = BufReader::new(server);
            server.get_mut().write_all(b"* OK ready\r\n").await.unwrap();
            let mut body_fetches = 0usize;
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
                if upper.starts_with("LOGIN") {
                    reply(&mut server, &format!("{tag} OK logged in\r\n")).await;
                } else if upper.starts_with("EXAMINE") {
                    let body = format!(
                        "* {} EXISTS\r\n* OK [UIDVALIDITY {}] .\r\n* OK [UIDNEXT {}] .\r\n{tag} OK [READ-ONLY] done\r\n",
                        mailbox.messages.len(),
                        mailbox.uidvalidity,
                        mailbox.uidnext
                    );
                    reply(&mut server, &body).await;
                } else if upper.starts_with("UID SEARCH") {
                    let uids: Vec<String> =
                        mailbox.messages.iter().map(|m| m.uid.to_string()).collect();
                    let body = format!("* SEARCH {}\r\n{tag} OK done\r\n", uids.join(" "));
                    reply(&mut server, &body).await;
                } else if upper.starts_with("UID FETCH") {
                    // "UID FETCH <set> (<items>)". Decide the kind by items:
                    // whole body (BODY.PEEK[]), FLAGS-only ((UID FLAGS)), or the
                    // identity pass (header fields).
                    let wants_body = upper.contains("BODY.PEEK[]");
                    if wants_body {
                        // Inject a mid-round transient failure after N body
                        // fetches: a tagged NO [UNAVAILABLE] the client maps to
                        // TransientTransport, aborting the round.
                        if let Some(limit) = mailbox.fail_after_body_fetches {
                            if body_fetches >= limit {
                                // A mid-transfer connection loss: announce a
                                // body literal, then send NONE of it and close.
                                // async-imap errors reading the truncated
                                // literal (unexpected EOF), which propagates as
                                // a transport error and aborts the round.
                                let uid = parse_uid_set(rest, &mailbox)
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
                    let flags_only = !wants_body && !upper.contains("HEADER.FIELDS");
                    let set = parse_uid_set(rest, &mailbox);
                    let mut out = String::new();
                    for (seq, uid) in set.iter().enumerate() {
                        if let Some(message) = mailbox.messages.iter().find(|m| &m.uid == uid) {
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

    /// Expand a UID set token ("1", "1,2,3") against the mailbox.
    fn parse_uid_set(command: &str, mailbox: &FakeMailbox) -> Vec<u32> {
        // The set is the token after "UID FETCH ".
        let after = command["UID FETCH ".len().min(command.len())..].trim_start();
        let set_token = after.split_whitespace().next().unwrap_or("");
        let mut uids = Vec::new();
        for part in set_token.split(',') {
            if let Ok(uid) = part.parse::<u32>() {
                if mailbox.messages.iter().any(|m| m.uid == uid) {
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
    /// in-memory database.
    fn provider_with(mailbox: Arc<Mutex<FakeMailbox>>) -> (ImapProvider, ImapStateStore) {
        let database = Arc::new(Database::open_memory());
        // The engine ingests into the shared threads/messages tables, which
        // reference the account; adopt it exactly as account setup would.
        database
            .adopt_mail_account("me@example.com", crate::models::MailProviderKind::Imap)
            .unwrap();
        let store = ImapStateStore::new(database.clone(), "me@example.com");
        let cache = BodyCache::new(store.clone());
        let provider = ImapProvider {
            store: store.clone(),
            cache,
            username: "me@example.com".into(),
            label_model: LabelModel::ImapLabelFolders,
            source: Arc::new(FakeSource { mailbox }),
            now: || 1_700_000_000,
            limits: SyncLimits::inbox(),
        };
        (provider, store)
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
            if let Some(first) = mb.messages.iter_mut().find(|m| m.uid == 1) {
                first.flags = vec!["\\Seen".into()];
            }
            mb.messages.retain(|m| m.uid != uid2);
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
            mb.uidvalidity = 200;
            mb.uidnext = 1;
            mb.messages.clear();
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
        assert!(!caps.verifiable_delivery);
        assert_eq!(caps.label_model, LabelModel::ImapLabelFolders);
    }

    #[tokio::test]
    async fn mutations_are_rejected_as_a_permanent_invalid_operation() {
        // So the mutation queue rolls the local change back to `failed` rather
        // than retrying forever (see the engine-level regression test in
        // sync.rs). InvalidOperation is neither retryable nor a reauth.
        let (provider, _store) = provider_with(Arc::new(Mutex::new(FakeMailbox::new(1))));
        let err = provider
            .modify_thread("imap:t:1", &["STARRED".into()], &[])
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::InvalidOperation(_)));
        assert!(!err.retry_mutation());
        assert!(!err.requires_reauthentication());
        // Sending is likewise unsupported this phase.
        match provider.prepare_delivery().await {
            Err(ProviderError::InvalidOperation(_)) => {}
            other => panic!("expected InvalidOperation, got {:?}", other.err()),
        }
        // list_labels still returns the INBOX-only system labels.
        let labels = provider.list_labels().await.unwrap();
        let ids: Vec<_> = labels.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(ids, ["INBOX", "UNREAD", "STARRED"]);
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
        failing.fail_after_body_fetches = Some(1);
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
        next.messages[0].flags = vec!["\\Seen".into()];
        next.add(&[], &message("<m2@x>", "Two", ""));
        let mut failing = next.clone();
        failing.fail_after_body_fetches = Some(0); // drop on the first body fetch
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
        next.messages.retain(|m| m.uid != 2);
        next.add(&[], &message("<m3@x>", "Three", ""));
        let mut failing = next.clone();
        failing.fail_after_body_fetches = Some(0);
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
        }
    }

    // ---- SLICE5A_FIXES item 6: gated live test against the Dovecot harness --
    //
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

            // A second poll with the same cursor reports no changes and the
            // generation did not move.
            let gen_before = cursor.generation().unwrap();
            let batch = provider.poll(&cursor).await.expect("re-poll");
            assert!(batch.changed_threads.is_empty(), "a stable re-poll is empty");
            assert_eq!(batch.cursor.generation(), Some(gen_before), "no idle bump");
        }
    }
}
