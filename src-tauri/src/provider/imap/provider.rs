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
            settings: config.settings,
            source,
            now: unix_now,
            limits: SyncLimits::inbox(),
            thread_state_loads: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Open an authenticated session through the configured source.
    async fn open_session(&self) -> ProviderResult<Box<dyn ImapSession>> {
        self.source.open().await
    }

    /// Refresh every synced mailbox against the server and return the new
    /// generation. INBOX is swept first and every poll; the other synced
    /// mailboxes (Trash, Junk this run) follow in plan order with cheap
    /// cadence gating. Each mailbox round is its OWN atomic commit, so a
    /// failure in one mailbox leaves the others' committed work intact and the
    /// at-least-once cursor contract whole.
    async fn refresh_all(&self) -> ProviderResult<u64> {
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
    ) -> ProviderResult<u64> {
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
                synced_now: true,
            });
        let mut generation = self.refresh_mailbox_with(session, &inbox_entry, true).await?;

        // Then the other synced mailboxes (Trash, Junk) in plan order, bounded
        // by the per-poll budget, each with cadence gating.
        let folders: Vec<_> = plan
            .synced()
            .filter(|e| !e.is_inbox)
            .take(policy::FOLDER_ROUNDS_PER_POLL)
            .cloned()
            .collect();
        for entry in folders {
            // Cadence gating decides whether a full sweep is needed. Only INBOX
            // may abort a poll: the engine ingests a poll's changes only after
            // the whole poll succeeds, so a folder that cannot be synced (its
            // mailbox renamed or deleted on the server, say) must not stop mail
            // flowing. A folder failure is logged and retried next poll, and a
            // dropped connection ends the folder walk (INBOX already committed).
            // Only a credential rejection, which pauses the account, propagates.
            match self.refresh_mailbox_with(session, &entry, false).await {
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
        Ok(generation)
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
        Ok(plan::build_plan(&catalog, Some(&delimiter), &self.settings))
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
    /// (`commit_sync_round`). A failure before that commit writes NOTHING
    /// durable for THIS mailbox, so the next round redoes only its work — the
    /// at-least-once contract, now per mailbox. Body-cache puts happen eagerly
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
        // there); every other mailbox uses its plan class's limits.
        let limits = if entry.is_inbox {
            self.limits
        } else {
            SyncLimits::for_class(entry.window_class)
        };
        // A thread is hot if it has a location in INBOX or Sent.
        let makes_hot = entry.is_inbox || matches!(entry.role, Some(super::MailboxRole::Sent));

        // 1. EXAMINE (read-only) — never SELECT, so sync never sets \Seen.
        let status = session.examine(mailbox).await?;
        let server_uidvalidity = status.uid_validity.unwrap_or(0) as i64;
        let server_exists = status.exists as i64;
        let server_uidnext = status.uid_next.unwrap_or(0) as i64;

        // Cadence gating for a non-INBOX mailbox: if the EXAMINE counters are
        // unchanged AND the periodic sweep is not yet due, skip the UID
        // SEARCH/FETCH entirely. INBOX always sweeps (force_sweep). A
        // UIDVALIDITY change is NEVER skipped — it invalidates every local UID.
        let now = (self.now)();
        let local_uidvalidity_pre = self
            .store
            .locations_in_mailbox(mailbox)
            .map_err(db_err)?
            .first()
            .map(|l| l.uidvalidity);
        let uidvalidity_changed =
            matches!(local_uidvalidity_pre, Some(stored) if stored != server_uidvalidity);
        let stored = self.store.mailbox_sync_state(mailbox).map_err(db_err)?;
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
        if !force_sweep && !uidvalidity_changed && counters_unchanged && !sweep_due {
            // Nothing to do: record the (unchanged) counters WITHOUT bumping
            // the generation, keeping last_sweep_at as stored.
            let last_sweep_at = stored.map(|(_, _, s)| s).unwrap_or(now);
            let mut round = SyncRoundWrite::default();
            round.mailbox_state =
                Some((mailbox.to_string(), server_exists, server_uidnext, last_sweep_at));
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
        let flag_window = policy::select_window(server_uids.clone(), limits.mailbox_window).in_window;
        let server_flags = self.fetch_flags(session, &flag_window).await?;
        let server_view = super::delta::ServerMailboxView {
            uidvalidity: server_uidvalidity,
            all_uids: server_uids.clone(),
            flags_by_uid: server_flags,
        };
        let delta = super::delta::compute_delta(&local_view, &server_view, &limits);

        if delta.beyond_window > 0 {
            log::info!(
                "imap sync: {} {mailbox} messages beyond the sync window",
                delta.beyond_window
            );
        }

        let mut round = SyncRoundWrite::default();
        round.mailbox_state =
            Some((mailbox.to_string(), server_exists, server_uidnext, new_last_sweep_at));

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

        // New mail: identity pass. Only INBOX+Sent (hot) mailboxes fetch
        // bodies at ingest; a non-hot (Trash/Junk) mailbox is index-only, so
        // its threading inputs come from the INDEX identity pass headers and no
        // body is fetched.
        for batch in chunk(&delta.new_uids, policy::UID_BATCH_SIZE) {
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
                    round.locations.push(ImapLocation {
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
                    threading_inputs.push(ThreadingInput {
                        message_id: message_id.clone(),
                        message_id_header: headers
                            .message_id
                            .or_else(|| row.inputs.message_id.clone()),
                        in_reply_to: headers.in_reply_to,
                        references: headers.references,
                    });
                    changed_message_ids.push(message_id.clone());
                    new_message_ids.push(message_id);
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
                    round.locations.push(ImapLocation {
                        mailbox: mailbox.to_string(),
                        uidvalidity: server_uidvalidity,
                        uid: row.uid as i64,
                        message_id: message_id.clone(),
                        flags_json: serde_json::to_string(&row.flags)
                            .unwrap_or_else(|_| "[]".to_string()),
                        modseq: None,
                    });
                    threading_inputs.push(ThreadingInput {
                        message_id: message_id.clone(),
                        message_id_header: threading
                            .message_id
                            .or_else(|| row.inputs.message_id.clone()),
                        in_reply_to: threading.in_reply_to,
                        references: threading.references,
                    });
                    changed_message_ids.push(message_id.clone());
                    new_message_ids.push(message_id);
                }
            }
        }

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
        // Refresh every synced mailbox (INBOX first) and return the current
        // generation as the cursor.
        let generation = self.refresh_all().await?;
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
        // (a) refresh all synced mailboxes (INBOX first); (b) return threads
        // journalled since `polled`; (c) new cursor = current generation.
        let generation = self.refresh_all().await?;
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
        let plan = self.build_sync_plan()?;

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
            // mailbox's RESOLVED ROLE (never its name).
            let per_copy: Vec<Vec<String>> = locations
                .iter()
                .map(|location| {
                    let flags: Vec<String> =
                        serde_json::from_str(&location.flags_json).unwrap_or_default();
                    labels_for(plan.label_for_mailbox(&location.mailbox), &flags)
                })
                .collect();
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
        let per_copy: Vec<Vec<String>> = locations
            .iter()
            .map(|location| {
                let flags: Vec<String> =
                    serde_json::from_str(&location.flags_json).unwrap_or_default();
                labels_for(plan.label_for_mailbox(&location.mailbox), &flags)
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
        // System labels this slice (no user labels yet). Slice 5b-1 adds the
        // Sent/Spam/Trash roles alongside INBOX.
        Ok(vec![
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
    }

    impl FakeMailbox {
        /// A server with just an INBOX at the given UIDVALIDITY.
        fn new(uidvalidity: u32) -> Self {
            let mut folders = std::collections::BTreeMap::new();
            folders.insert("INBOX".to_string(), FakeFolder::new(uidvalidity));
            Self {
                folders,
                command_counts: Arc::new(Mutex::new(Default::default())),
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
            label_model: LabelModel::ImapLabelFolders,
            settings: test_settings(),
            source: Arc::new(FakeSource { mailbox }),
            now: || 1_700_000_000,
            limits: SyncLimits::inbox(),
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

    /// (f) Drafts, All Mail, Starred (\Flagged, no role), the label container
    /// and Archive are never selected or fetched — only INBOX, Trash, Junk are.
    #[tokio::test]
    async fn only_inbox_trash_and_junk_are_ever_synced() {
        let mut mb = FakeMailbox::new(100);
        mb.add(&["\\Seen"], &message("<i1@x>", "Inbox", ""));
        mb.add_to("Trash", 50, &["\\Seen"], &message("<t1@x>", "Trashed", ""));
        mb.add_to("Junk", 60, &["\\Seen"], &message("<j1@x>", "Spam", ""));
        // Folders that MUST never be synced:
        mb.add_to("Drafts", 70, &["\\Seen"], &message("<d1@x>", "Draft", ""));
        mb.add_to("All Mail", 80, &["\\Seen"], &message("<a1@x>", "All", ""));
        mb.add_to("Sent", 90, &["\\Seen"], &message("<s1@x>", "Sent", "")); // run 3, not now
        mb.add_to("Archive", 95, &["\\Seen"], &message("<ar1@x>", "Arch", "")); // 5b-2
        let mailbox = Arc::new(Mutex::new(mb));
        let (provider, store) = provider_with(mailbox.clone());
        let db = store.database();

        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider).await.unwrap();

        // Synced mailboxes have locations.
        for synced in ["INBOX", "Trash", "Junk"] {
            assert!(!store.locations_in_mailbox(synced).unwrap().is_empty(), "{synced} synced");
        }
        // Never-synced mailboxes have NO locations and were never SEARCHed.
        let counts = mailbox.lock().unwrap();
        for never in ["Drafts", "All Mail", "Sent", "Archive"] {
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

            // A second poll with the same cursor reports no changes and the
            // generation did not move.
            let gen_before = cursor.generation().unwrap();
            let batch = provider.poll(&cursor).await.expect("re-poll");
            assert!(batch.changed_threads.is_empty(), "a stable re-poll is empty");
            assert_eq!(batch.cursor.generation(), Some(gen_before), "no idle bump");
        }
    }
}
