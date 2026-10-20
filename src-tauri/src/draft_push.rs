//! Pushes local drafts to the account's server drafts
//! (`docs/server-drafts-design.md`). The local `drafts` table stays the
//! composer's source of truth; this module copies each saved revision to the
//! server after a pause and removes the server copy once the draft has been
//! sent or discarded.
//!
//! Every server copy has a `Message-ID` derived from the draft id, so any
//! device holding the draft finds the same copy and updates it rather than
//! creating another one.
use std::{collections::HashMap, sync::Arc, time::Duration};

use rusqlite::{params, OptionalExtension, Transaction};
use serde::Serialize;

use crate::{
    correspondence::{build_draft_mime, Correspondence, Draft},
    db::{Database, DbResult},
    models::MailProviderKind,
    provider::{MailProvider, ProviderError, ServerDraft},
};

/// How long after the last save a draft waits before it is pushed.
pub(crate) const DRAFT_PUSH_DELAY_MS: i64 = 5_000;
/// The longest continuous typing can hold back a push.
pub(crate) const DRAFT_PUSH_MAX_WAIT_MS: i64 = 60_000;
/// How long quitting waits for pending pushes.
pub(crate) const EXIT_PUSH_TIMEOUT: Duration = Duration::from_secs(5);
/// How often the worker looks for due pushes and deletions.
const PUSH_POLL_INTERVAL: Duration = Duration::from_secs(1);
/// How long a sent draft's server copy waits while its send is unresolved.
const SEND_PENDING_RECHECK_MS: i64 = 30_000;
/// Outbox states after which the draft's server copy is no longer needed.
const SENT_STATES: [&str; 2] = ["sent", "unverifiable"];

/// The local part of the `Message-ID` every server copy of draft `id` carries.
pub(crate) fn server_message_local_part(draft_id: &str) -> String {
    format!("draft-{draft_id}")
}

fn server_message_id(draft_id: &str, domain: Option<&str>) -> String {
    format!(
        "{}@{}",
        server_message_local_part(draft_id),
        domain.unwrap_or("threestrands.local")
    )
}

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Whether drafts on accounts of this kind are kept on the server.
pub(crate) fn kind_keeps_server_drafts(kind: MailProviderKind) -> bool {
    matches!(kind, MailProviderKind::Gmail)
}

/// Where a draft's server copy stands, as the composer shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerDraftStatus {
    /// `pending` (waiting for the push delay), `retrying` (a temporary
    /// failure), `failed` (needs the user), `conflict` (changed here and on
    /// the server; the user picks a version), or `saved` (the server has this
    /// revision).
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What the push and pull worker tells the frontend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DraftEvent {
    /// A draft's server copy changed state.
    Status(ServerDraftEvent),
    /// Local drafts changed because of the server; the Drafts list should
    /// reload, and an open composer may need to follow.
    ListChanged,
}

/// Sent to the frontend whenever a push finishes or fails.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerDraftEvent {
    pub id: String,
    /// The draft revision `status` describes.
    pub revision: i64,
    pub status: ServerDraftStatus,
}

pub(crate) struct Link {
    pub(crate) draft_id: String,
    pub(crate) account: String,
    pub(crate) server_id: Option<String>,
    pub(crate) server_version: Option<String>,
    pub(crate) pushed_revision: i64,
    pub(crate) attempts: u32,
}

struct Deletion {
    id: i64,
    account: String,
    server_id: Option<String>,
    message_id: String,
    attempts: u32,
}

fn retry_at(attempts: u32, at: i64) -> i64 {
    at + crate::backoff::retry_delay_secs(attempts.max(1)) * 1_000
}

impl Database {
    pub(crate) fn keeps_server_drafts(tx: &Transaction, account: &str) -> DbResult<bool> {
        let provider: Option<String> = tx
            .query_row(
                "SELECT provider FROM accounts WHERE email=?1 AND on_this_device=1",
                [account],
                |row| row.get(0),
            )
            .optional()?;
        Ok(provider
            .as_deref()
            .and_then(MailProviderKind::parse)
            .is_some_and(kind_keeps_server_drafts))
    }

    /// Records that `draft` changed and its server copy needs the new
    /// revision. Runs in the transaction that saved the draft. A draft that
    /// moved to another account queues its old account's copy for deletion.
    pub(crate) fn mark_draft_push_pending(tx: &Transaction, draft: &Draft, at: i64) -> DbResult<()> {
        let existing: Option<(String, Option<String>, Option<i64>, String)> = tx
            .query_row(
                "SELECT account, server_id, pending_since, state FROM draft_server_links WHERE draft_id=?1",
                [&draft.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        if let Some((account, server_id, _, _)) = &existing {
            if *account != draft.account {
                Self::queue_server_draft_deletion(tx, account, server_id.as_deref(), &draft.id, at)?;
                tx.execute("DELETE FROM draft_server_links WHERE draft_id=?1", [&draft.id])?;
            }
        }
        if !Self::keeps_server_drafts(tx, &draft.account)? {
            return Ok(());
        }
        let existing = existing.filter(|(account, ..)| *account == draft.account);
        let pending_since = existing.as_ref().and_then(|(_, _, since, _)| *since).unwrap_or(at);
        let due = (at + DRAFT_PUSH_DELAY_MS).min(pending_since + DRAFT_PUSH_MAX_WAIT_MS);
        match existing {
            // A temporary failure keeps its backoff; editing doesn't hammer the server.
            Some((_, _, _, state)) if state == "retrying" => {
                tx.execute(
                    "UPDATE draft_server_links SET pending_since=?2, due_at=MAX(COALESCE(due_at, ?3), ?3) WHERE draft_id=?1",
                    params![draft.id, pending_since, due],
                )?;
            }
            Some(_) => {
                tx.execute(
                    "UPDATE draft_server_links SET pending_since=?2, due_at=?3, state='pending', error=NULL, attempts=0 WHERE draft_id=?1",
                    params![draft.id, pending_since, due],
                )?;
            }
            None => {
                tx.execute(
                    "INSERT INTO draft_server_links(draft_id, account, pending_since, due_at, state) VALUES (?1, ?2, ?3, ?4, 'pending')",
                    params![draft.id, draft.account, pending_since, due],
                )?;
            }
        }
        Ok(())
    }

    fn queue_server_draft_deletion(
        tx: &Transaction,
        account: &str,
        server_id: Option<&str>,
        draft_id: &str,
        at: i64,
    ) -> DbResult<()> {
        tx.execute(
            "INSERT INTO draft_server_deletions(account, server_id, message_id, due_at) VALUES (?1, ?2, ?3, ?4)",
            params![account, server_id, server_message_local_part(draft_id), at],
        )?;
        Ok(())
    }

    /// Makes the worker look at `draft_id`'s server copy now, because the
    /// draft left the drafts table: it was sent, scheduled, or discarded.
    pub(crate) fn draft_left_drafts(tx: &Transaction, draft_id: &str, at: i64) -> DbResult<()> {
        tx.execute("UPDATE draft_server_links SET due_at=?2 WHERE draft_id=?1", params![draft_id, at])?;
        Ok(())
    }

    /// Forgets the server link without touching the server. For a draft that
    /// another device removed: that device deletes the server copy.
    pub(crate) fn forget_server_draft_link(&self, draft_id: &str) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute("DELETE FROM draft_server_links WHERE draft_id=?1", [draft_id])?;
            Ok(())
        })
    }

    /// The composer's view of `draft`'s server copy, or `None` when the
    /// draft stays on this device.
    pub(crate) fn server_draft_status(&self, draft: &Draft) -> DbResult<Option<ServerDraftStatus>> {
        self.with_connection(|connection| {
            let row: Option<(i64, String, Option<String>, bool)> = connection
                .query_row(
                    "SELECT pushed_revision, state, error, conflict IS NOT NULL FROM draft_server_links WHERE draft_id=?1 AND account=?2",
                    params![draft.id, draft.account],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()?;
            Ok(row.map(|(pushed, state, error, conflict)| {
                if conflict {
                    ServerDraftStatus { state: "conflict".into(), error: None }
                } else if pushed >= draft.revision {
                    ServerDraftStatus { state: "saved".into(), error: None }
                } else if state == "saved" {
                    ServerDraftStatus { state: "pending".into(), error: None }
                } else {
                    ServerDraftStatus { state, error }
                }
            }))
        })
    }

    /// Makes every pending push of `draft_id` (or of every draft) due now.
    pub(crate) fn push_drafts_now(&self, draft_id: Option<&str>, at: i64) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE draft_server_links SET due_at=?1 WHERE state='pending' AND due_at IS NOT NULL AND (?2 IS NULL OR draft_id=?2)",
                params![at, draft_id],
            )?;
            Ok(())
        })
    }

    fn due_draft_accounts(&self, at: i64) -> DbResult<Vec<String>> {
        self.with_connection(|connection| {
            let mut query = connection.prepare(
                "SELECT account FROM draft_server_links WHERE due_at IS NOT NULL AND due_at<=?1
                 UNION SELECT account FROM draft_server_deletions WHERE due_at<=?1",
            )?;
            let rows = query.query_map([at], |row| row.get(0))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
    }

    fn due_draft_links(&self, at: i64) -> DbResult<Vec<Link>> {
        self.with_connection(|connection| {
            // A conflict waits for the user, not the clock.
            let mut query = connection.prepare(
                "SELECT draft_id, account, server_id, server_version, pushed_revision, attempts FROM draft_server_links WHERE due_at IS NOT NULL AND due_at<=?1 AND conflict IS NULL ORDER BY due_at",
            )?;
            let rows = query.query_map([at], link_from_row)?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
    }

    pub(crate) fn account_draft_links(&self, account: &str) -> DbResult<Vec<Link>> {
        self.with_connection(|connection| {
            let mut query = connection.prepare(
                "SELECT draft_id, account, server_id, server_version, pushed_revision, attempts FROM draft_server_links WHERE account=?1",
            )?;
            let rows = query.query_map([account], link_from_row)?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
    }

    fn due_draft_deletions(&self, at: i64) -> DbResult<Vec<Deletion>> {
        self.with_connection(|connection| {
            let mut query = connection.prepare(
                "SELECT id, account, server_id, message_id, attempts FROM draft_server_deletions WHERE due_at<=?1 ORDER BY id",
            )?;
            let rows = query.query_map([at], |row| {
                Ok(Deletion {
                    id: row.get(0)?,
                    account: row.get(1)?,
                    server_id: row.get(2)?,
                    message_id: row.get(3)?,
                    attempts: row.get(4)?,
                })
            })?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
    }

    pub(crate) fn local_draft(&self, id: &str) -> DbResult<Option<Draft>> {
        self.with_connection(|connection| {
            let payload: Option<String> = connection
                .query_row("SELECT payload FROM drafts WHERE id=?1", [id], |row| row.get(0))
                .optional()?;
            payload
                .map(|payload| serde_json::from_str(&payload).map_err(crate::db::serialization_error))
                .transpose()
        })
    }

    /// What should happen to the server copy of a draft no longer in the
    /// local drafts table.
    fn removed_draft_outcome(&self, draft_id: &str) -> DbResult<RemovedDraft> {
        self.with_connection(|connection| {
            let mut query = connection.prepare(
                "SELECT state FROM outbox_messages WHERE draft_id=?1 AND state<>'canceled'",
            )?;
            let states = query
                .query_map([draft_id], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(if states.iter().any(|state| SENT_STATES.contains(&state.as_str())) {
                RemovedDraft::Delete
            } else if states.is_empty() {
                // Discarded.
                RemovedDraft::Delete
            } else {
                // Waiting to send, sending, or failed and restorable: the
                // server copy stays so Undo and Restore bring the draft back.
                RemovedDraft::Wait
            })
        })
    }

    fn retire_draft_link(&self, link: &Link, at: i64) -> DbResult<()> {
        self.with_transaction(|tx| {
            Self::queue_server_draft_deletion(tx, &link.account, link.server_id.as_deref(), &link.draft_id, at)?;
            tx.execute("DELETE FROM draft_server_links WHERE draft_id=?1", [&link.draft_id])?;
            Ok(())
        })
    }

    fn defer_draft_link(&self, draft_id: &str, due: i64) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute("UPDATE draft_server_links SET due_at=?2 WHERE draft_id=?1", params![draft_id, due])?;
            Ok(())
        })
    }

    /// Records a confirmed push of `revision`. The link stays pending when
    /// the draft changed again meanwhile.
    pub(crate) fn record_draft_pushed(&self, draft_id: &str, server: &ServerDraft, revision: i64) -> DbResult<i64> {
        self.with_transaction(|tx| {
            let current: Option<i64> = tx
                .query_row("SELECT revision FROM drafts WHERE id=?1", [draft_id], |row| row.get(0))
                .optional()?;
            let newer = current.is_some_and(|current| current > revision);
            tx.execute(
                "UPDATE draft_server_links SET server_id=?2, server_version=COALESCE(?5, server_version), pushed_revision=MAX(pushed_revision, ?3), attempts=0, error=NULL,
                     state=CASE WHEN ?4 THEN 'pending' ELSE 'saved' END,
                     pending_since=CASE WHEN ?4 THEN pending_since ELSE NULL END,
                     due_at=CASE WHEN ?4 THEN due_at ELSE NULL END
                 WHERE draft_id=?1",
                params![draft_id, server.id, revision, newer, server.version],
            )?;
            Ok(current.unwrap_or(revision))
        })
    }

    fn record_draft_push_failure(
        &self,
        draft_id: &str,
        state: &str,
        error: &str,
        due: Option<i64>,
        attempts: u32,
    ) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE draft_server_links SET state=?2, error=?3, due_at=?4, attempts=?5 WHERE draft_id=?1",
                params![draft_id, state, error, due, attempts],
            )?;
            Ok(())
        })
    }

    fn finish_draft_deletion(&self, id: i64) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute("DELETE FROM draft_server_deletions WHERE id=?1", [id])?;
            Ok(())
        })
    }

    fn defer_draft_deletion(&self, id: i64, attempts: u32, due: i64) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE draft_server_deletions SET attempts=?2, due_at=?3 WHERE id=?1",
                params![id, attempts, due],
            )?;
            Ok(())
        })
    }
}

fn link_from_row(row: &rusqlite::Row) -> rusqlite::Result<Link> {
    Ok(Link {
        draft_id: row.get(0)?,
        account: row.get(1)?,
        server_id: row.get(2)?,
        server_version: row.get(3)?,
        pushed_revision: row.get(4)?,
        attempts: row.get(5)?,
    })
}

enum RemovedDraft {
    Delete,
    Wait,
}

/// A push failure as the link records it: its state, the message shown to
/// the user, and whether it retries on its own.
fn classify_push_error(error: &ProviderError, account: &str) -> (&'static str, String, bool) {
    if error.requires_reauthentication() {
        ("failed", format!("Reconnect {account} to save this draft to the server."), true)
    } else if error.retry_mutation() || matches!(error, ProviderError::NotFound) {
        ("retrying", "The server couldn't be reached.".into(), true)
    } else {
        ("failed", error.to_string(), false)
    }
}

impl Correspondence {
    /// Pushes, deletes, and pulls server drafts as they come due, forever.
    pub async fn run_draft_pushes(self) {
        let mut last_pulled = HashMap::new();
        loop {
            if let Err(error) = self.run_draft_round(now(), &mut last_pulled).await {
                log::warn!(target: "drafts", "server draft push failed: {error}");
            }
            tokio::time::sleep(PUSH_POLL_INTERVAL).await;
        }
    }

    /// Pushes every pending draft now, waiting at most [`EXIT_PUSH_TIMEOUT`].
    /// Drafts that don't make it stay saved locally and push on next launch.
    pub async fn push_drafts_before_exit(&self) {
        let at = now();
        if self.database.push_drafts_now(None, at).is_err() {
            return;
        }
        let _ = tokio::time::timeout(EXIT_PUSH_TIMEOUT, self.push_due_drafts(at)).await;
    }

    pub(crate) async fn push_due_drafts(&self, at: i64) -> Result<(), String> {
        let _guard = self.draft_pushes.lock().await;
        let providers = self.draft_providers(self.database.due_draft_accounts(at)?).await;
        self.push_due_drafts_with(at, &providers).await
    }

    /// Asks the worker to pull every account's server drafts on its next
    /// round, as when the window comes to the front.
    pub fn request_draft_pull(&self) {
        self.draft_pull_requested.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    async fn run_draft_round(&self, at: i64, last_pulled: &mut HashMap<String, i64>) -> Result<(), String> {
        let _guard = self.draft_pushes.lock().await;
        let mut accounts: Vec<String> = self.accounts.lock().await.keys().cloned().collect();
        accounts.extend(self.database.due_draft_accounts(at)?);
        accounts.sort();
        accounts.dedup();
        let providers = self.draft_providers(accounts).await;
        let force = self.draft_pull_requested.swap(false, std::sync::atomic::Ordering::SeqCst);
        self.push_due_drafts_with(at, &providers).await?;
        self.pull_due_accounts(at, &providers, last_pulled, force).await
    }

    async fn draft_providers(&self, accounts: Vec<String>) -> HashMap<String, Arc<dyn MailProvider>> {
        let mut providers = HashMap::new();
        for account in accounts {
            if let Ok(provider) = self.provider_for(&account).await {
                providers.insert(account, provider);
            }
        }
        providers
    }

    /// Does the due work with the connected accounts' providers. An account
    /// missing from `providers` isn't connected on this device.
    pub(crate) async fn push_due_drafts_with(
        &self,
        at: i64,
        providers: &HashMap<String, Arc<dyn MailProvider>>,
    ) -> Result<(), String> {
        for link in self.database.due_draft_links(at)? {
            self.push_link(&link, providers.get(&link.account)).await?;
        }
        for deletion in self.database.due_draft_deletions(at)? {
            self.delete_server_copy(&deletion, providers.get(&deletion.account)).await?;
        }
        Ok(())
    }

    async fn push_link(
        &self,
        link: &Link,
        provider: Option<&Arc<dyn MailProvider>>,
    ) -> Result<(), String> {
        let Some(draft) = self.database.local_draft(&link.draft_id)? else {
            match self.database.removed_draft_outcome(&link.draft_id)? {
                RemovedDraft::Delete => self.database.retire_draft_link(link, now())?,
                RemovedDraft::Wait => self
                    .database
                    .defer_draft_link(&link.draft_id, now() + SEND_PENDING_RECHECK_MS)?,
            }
            return Ok(());
        };
        if draft.account != link.account {
            // The save that moved it already queued the old copy's deletion.
            return Ok(());
        }
        if let Some(server_id) = link.server_id.as_ref().filter(|_| link.pushed_revision >= draft.revision) {
            let server = ServerDraft { id: server_id.clone(), version: None };
            self.database.record_draft_pushed(&draft.id, &server, draft.revision)?;
            return Ok(());
        }
        let provider = match provider {
            Some(provider) => provider,
            None => {
                let attempts = link.attempts + 1;
                let message = format!("{} isn't connected on this device.", draft.account);
                self.database.record_draft_push_failure(&draft.id, "retrying", &message, Some(retry_at(attempts, now())), attempts)?;
                self.notify_draft(&draft, "retrying", Some(message));
                return Ok(());
            }
        };
        if !provider.server_drafts() {
            self.database.forget_server_draft_link(&draft.id)?;
            return Ok(());
        }
        match self.push_to_provider(provider.as_ref(), &draft, link).await {
            Ok(None) => {
                // Changed on the server too: the user picks a version first.
                self.notify_draft(&draft, "conflict", None);
            }
            Ok(Some(server)) => {
                let current = self.database.record_draft_pushed(&draft.id, &server, draft.revision)?;
                if current <= draft.revision {
                    self.notify_draft(&draft, "saved", None);
                }
            }
            Err(PushError::Message(message)) => {
                self.database.record_draft_push_failure(&draft.id, "failed", &message, None, link.attempts)?;
                self.notify_draft(&draft, "failed", Some(message));
            }
            Err(PushError::Provider(error)) => {
                let (state, message, retries) = classify_push_error(&error, &draft.account);
                let attempts = link.attempts + 1;
                let due = retries.then(|| retry_at(attempts, now()));
                self.database.record_draft_push_failure(&draft.id, state, &message, due, attempts)?;
                self.notify_draft(&draft, state, Some(message));
            }
        }
        Ok(())
    }

    /// Pushes `draft`, or returns `None` after recording a conflict because
    /// the server copy changed since this device last saw it.
    async fn push_to_provider(
        &self,
        provider: &dyn MailProvider,
        draft: &Draft,
        link: &Link,
    ) -> Result<Option<ServerDraft>, PushError> {
        let server_id = link.server_id.as_deref();
        let mut server_id = server_id.map(str::to_string);
        if let (Some(id), Some(seen)) = (&server_id, &link.server_version) {
            match provider.server_draft_version(id).await.map_err(PushError::Provider)? {
                None => server_id = None,
                Some(current) if current != *seen => {
                    self.hold_server_conflict(provider, draft, id).await.map_err(PushError::Provider)?;
                    return Ok(None);
                }
                Some(_) => {}
            }
        }
        let server_id = server_id.as_deref();
        let identities = provider.identities(&draft.account).await.map_err(PushError::Provider)?;
        let domain = provider
            .message_id_domain(draft.from_address())
            .map_err(PushError::Provider)?;
        let display_name = self
            .database
            .get_account(&draft.account)
            .map_err(|error| PushError::Message(error.to_string()))?
            .and_then(|account| account.display_name);
        let raw = build_draft_mime(draft, &identities, display_name.as_deref(), domain.as_deref(), &self.root)
            .map_err(PushError::Message)?;
        let thread = draft.thread_id.as_deref();
        let existing = match server_id {
            Some(id) => Some(id.to_string()),
            None => provider
                .find_server_draft(&server_message_id(&draft.id, domain.as_deref()))
                .await
                .map_err(PushError::Provider)?
                .map(|found| found.id),
        };
        if let Some(id) = existing {
            match provider.update_server_draft(&id, &raw, thread).await {
                // Deleted on the server: keep the user's work by creating it again.
                Err(ProviderError::NotFound) => {}
                result => return result.map(Some).map_err(PushError::Provider),
            }
        }
        provider.create_server_draft(&raw, thread).await.map(Some).map_err(PushError::Provider)
    }

    async fn delete_server_copy(
        &self,
        deletion: &Deletion,
        provider: Option<&Arc<dyn MailProvider>>,
    ) -> Result<(), String> {
        let retry = |attempts: u32| self.database.defer_draft_deletion(deletion.id, attempts, retry_at(attempts, now()));
        let Some(provider) = provider else {
            retry(deletion.attempts + 1)?;
            return Ok(());
        };
        if !provider.server_drafts() {
            self.database.finish_draft_deletion(deletion.id)?;
            return Ok(());
        }
        let server_id = match &deletion.server_id {
            Some(id) => Some(id.clone()),
            None => {
                let domain = provider.message_id_domain(&deletion.account).unwrap_or(None);
                let message_id = format!("{}@{}", deletion.message_id, domain.as_deref().unwrap_or("threestrands.local"));
                match provider.find_server_draft(&message_id).await {
                    Ok(found) => found.map(|found| found.id),
                    Err(_) => {
                        retry(deletion.attempts + 1)?;
                        return Ok(());
                    }
                }
            }
        };
        let result = match &server_id {
            Some(id) => provider.delete_server_draft(id).await,
            None => Ok(()),
        };
        match result {
            Ok(()) | Err(ProviderError::NotFound) => self.database.finish_draft_deletion(deletion.id)?,
            Err(error) if error.retry_mutation() || error.requires_reauthentication() => retry(deletion.attempts + 1)?,
            // The server refused for good; leaving the copy is safer than retrying forever.
            Err(error) => {
                log::warn!(target: "drafts", "server draft deletion refused: {error}");
                self.database.finish_draft_deletion(deletion.id)?;
            }
        }
        Ok(())
    }

    fn notify_draft(&self, draft: &Draft, state: &str, error: Option<String>) {
        (self.draft_events)(DraftEvent::Status(ServerDraftEvent {
            id: draft.id.clone(),
            revision: draft.revision,
            status: ServerDraftStatus { state: state.into(), error },
        }));
    }
}

enum PushError {
    /// The draft can't be turned into a message yet; the user must change it.
    Message(String),
    Provider(ProviderError),
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{
        path::PathBuf,
        sync::{Mutex as StdMutex, MutexGuard},
    };


    use super::*;
    use crate::{
        mime::RawMessage,
        models::Label,
        provider::{
            Delivery, DeliveryReceipt, LabelModel, MailFetch, MailMutate, MailSend, MailSync,
            ProviderCapabilities, ProviderResult, SyncBatch, SyncCursor, ThreadPage,
        },
    };

    pub(crate) const ACCOUNT: &str = "you@example.com";

    /// One draft held by [`FakeDrafts`]: raw message, thread, version.
    pub(crate) type FakeDraft = (String, Option<String>, String);

    /// An in-memory stand-in for an account's server drafts.
    #[derive(Default)]
    pub(crate) struct FakeDrafts {
        supported: bool,
        /// Server draft id -> its content.
        drafts: StdMutex<HashMap<String, FakeDraft>>,
        versions: StdMutex<usize>,
        calls: StdMutex<Vec<String>>,
        /// Returned, once, by the next create or update.
        fail_next: StdMutex<Option<ProviderError>>,
        created: StdMutex<usize>,
    }

    impl FakeDrafts {
        pub(crate) fn new() -> Arc<Self> {
            Arc::new(Self { supported: true, ..Self::default() })
        }
        pub(crate) fn drafts(&self) -> MutexGuard<'_, HashMap<String, FakeDraft>> {
            self.drafts.lock().unwrap()
        }
        fn next_version(&self) -> String {
            let mut versions = self.versions.lock().unwrap();
            *versions += 1;
            format!("v{versions}")
        }
        /// Stores `raw` as another app would: a new draft, or an edit to `id`.
        pub(crate) fn write_elsewhere(&self, id: &str, raw: &str, thread: Option<&str>) {
            let version = self.next_version();
            self.drafts().insert(id.into(), (raw.into(), thread.map(str::to_string), version));
        }
        pub(crate) fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
        fn fail_next(&self, error: ProviderError) {
            *self.fail_next.lock().unwrap() = Some(error);
        }
        fn take_failure(&self) -> ProviderResult<()> {
            match self.fail_next.lock().unwrap().take() {
                Some(error) => Err(error),
                None => Ok(()),
            }
        }
    }

    #[async_trait::async_trait]
    impl MailSend for FakeDrafts {
        async fn sender_identity(&self) -> ProviderResult<String> {
            Ok(ACCOUNT.into())
        }
        async fn prepare_delivery(&self) -> ProviderResult<Box<dyn Delivery>> {
            panic!("Pushing a draft must never send it");
        }
        async fn find_sent_copy(&self, _: &str, _: &str) -> ProviderResult<Option<DeliveryReceipt>> {
            panic!("Pushing a draft must never look for sent mail");
        }
        fn server_drafts(&self) -> bool {
            self.supported
        }
        async fn find_server_draft(&self, message_id: &str) -> ProviderResult<Option<ServerDraft>> {
            self.calls.lock().unwrap().push(format!("find {message_id}"));
            let header = format!("Message-ID: <{message_id}>");
            Ok(self
                .drafts()
                .iter()
                .find(|(_, (raw, ..))| raw.contains(&header))
                .map(|(id, (.., version))| ServerDraft { id: id.clone(), version: Some(version.clone()) }))
        }
        async fn create_server_draft(&self, raw: &[u8], thread: Option<&str>) -> ProviderResult<ServerDraft> {
            self.calls.lock().unwrap().push("create".into());
            self.take_failure()?;
            let mut created = self.created.lock().unwrap();
            *created += 1;
            let id = format!("server-{created}");
            drop(created);
            let version = self.next_version();
            self.drafts().insert(
                id.clone(),
                (String::from_utf8_lossy(raw).into_owned(), thread.map(str::to_string), version.clone()),
            );
            Ok(ServerDraft { id, version: Some(version) })
        }
        async fn update_server_draft(&self, id: &str, raw: &[u8], thread: Option<&str>) -> ProviderResult<ServerDraft> {
            self.calls.lock().unwrap().push(format!("update {id}"));
            self.take_failure()?;
            if !self.drafts().contains_key(id) {
                return Err(ProviderError::NotFound);
            }
            let version = self.next_version();
            self.drafts().insert(
                id.into(),
                (String::from_utf8_lossy(raw).into_owned(), thread.map(str::to_string), version.clone()),
            );
            Ok(ServerDraft { id: id.into(), version: Some(version) })
        }
        async fn delete_server_draft(&self, id: &str) -> ProviderResult<()> {
            self.calls.lock().unwrap().push(format!("delete {id}"));
            self.drafts().remove(id).map(|_| ()).ok_or(ProviderError::NotFound)
        }
        async fn list_server_drafts(&self) -> ProviderResult<Vec<ServerDraft>> {
            self.calls.lock().unwrap().push("list".into());
            Ok(self
                .drafts()
                .iter()
                .map(|(id, (.., version))| ServerDraft { id: id.clone(), version: Some(version.clone()) })
                .collect())
        }
        async fn fetch_server_draft(&self, id: &str) -> ProviderResult<crate::provider::FetchedServerDraft> {
            self.calls.lock().unwrap().push(format!("fetch {id}"));
            let (raw, thread, version) = self.drafts().get(id).cloned().ok_or(ProviderError::NotFound)?;
            Ok(crate::provider::FetchedServerDraft {
                draft: ServerDraft { id: id.into(), version: Some(version) },
                thread_id: thread,
                raw: raw.into_bytes(),
            })
        }
        async fn server_draft_version(&self, id: &str) -> ProviderResult<Option<String>> {
            self.calls.lock().unwrap().push(format!("version {id}"));
            Ok(self.drafts().get(id).map(|(.., version)| version.clone()))
        }
    }

    impl MailProvider for FakeDrafts {
        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                send_supported: true,
                server_search: false,
                provided_threads: true,
                label_model: LabelModel::GmailLabels,
                verifiable_delivery: true,
                client_saves_sent_copy: false,
            }
        }
    }

    #[async_trait::async_trait]
    impl MailSync for FakeDrafts {
        async fn baseline_cursor(&self) -> ProviderResult<SyncCursor> {
            unreachable!()
        }
        async fn poll(&self, _: &SyncCursor) -> ProviderResult<SyncBatch> {
            unreachable!()
        }
        async fn list_inbox(&self, _: Option<&str>) -> ProviderResult<ThreadPage> {
            unreachable!()
        }
        async fn fetch_thread(&self, _: &str) -> ProviderResult<Vec<RawMessage>> {
            unreachable!()
        }
    }

    #[async_trait::async_trait]
    impl MailFetch for FakeDrafts {
        async fn fetch_message(&self, _: &str) -> ProviderResult<RawMessage> {
            unreachable!()
        }
        async fn attachment_bytes(&self, _: &str, _: &str) -> ProviderResult<Vec<u8>> {
            unreachable!()
        }
    }

    #[async_trait::async_trait]
    impl MailMutate for FakeDrafts {
        async fn modify_thread(&self, _: &str, _: &[String], _: &[String]) -> ProviderResult<()> {
            unreachable!()
        }
        async fn modify_messages(&self, _: &[String], _: &[String], _: &[String]) -> ProviderResult<()> {
            unreachable!()
        }
        async fn list_labels(&self) -> ProviderResult<Vec<Label>> {
            unreachable!()
        }
        async fn create_label(&self, _: &str) -> ProviderResult<Label> {
            unreachable!()
        }
        async fn update_label(&self, _: &str, _: &str) -> ProviderResult<Label> {
            unreachable!()
        }
        async fn delete_label(&self, _: &str) -> ProviderResult<()> {
            unreachable!()
        }
    }

    pub(crate) struct Harness {
        pub(crate) service: Correspondence,
        pub(crate) server: Arc<FakeDrafts>,
        pub(crate) events: Arc<StdMutex<Vec<DraftEvent>>>,
        pub(crate) root: TempRoot,
    }

    /// An attachment root removed when the test ends.
    pub(crate) struct TempRoot(pub(crate) PathBuf);

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    impl Harness {
        pub(crate) fn new() -> Self {
            Self::with_server(FakeDrafts::new())
        }
        pub(crate) fn with_server(server: Arc<FakeDrafts>) -> Self {
            let database = Database::open_memory();
            database.set_compose_identity(ACCOUNT).unwrap();
            database.adopt_account(ACCOUNT).unwrap();
            let events = Arc::new(StdMutex::new(Vec::new()));
            let recorded = events.clone();
            let root = TempRoot(std::env::temp_dir().join(format!("drafts-{}", uuid::Uuid::new_v4())));
            std::fs::create_dir_all(&root.0).unwrap();
            let service = Correspondence {
                sending_identity: Arc::new(|_| Ok("00000000-0000-4000-8000-000000000001".into())),
                database: Arc::new(database),
                accounts: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
                root: root.0.clone(),
                gate: Arc::new(tokio::sync::Mutex::new(())),
                edits: Arc::new(tokio::sync::Mutex::new(())),
                scheduling: Arc::new(tokio::sync::Mutex::new(())),
                draft_pushes: Arc::new(tokio::sync::Mutex::new(())),
                draft_events: Arc::new(move |event| recorded.lock().unwrap().push(event)),
                draft_pull_requested: Arc::default(),
            };
            Self { service, server, events, root }
        }
        pub(crate) fn db(&self) -> &Database {
            &self.service.database
        }
        pub(crate) fn providers(&self) -> HashMap<String, Arc<dyn MailProvider>> {
            HashMap::from([(ACCOUNT.to_string(), self.server.clone() as Arc<dyn MailProvider>)])
        }
        pub(crate) fn new_draft(&self) -> Draft {
            self.db().create_draft("new", None, ACCOUNT, &[]).unwrap()
        }
        pub(crate) fn edit(&self, mut draft: Draft, body: &str) -> Draft {
            draft.subject = "Plans".into();
            draft.body = body.into();
            self.db().save_draft(draft).unwrap()
        }
        /// Runs the worker as if `delay_ms` had passed since now.
        pub(crate) async fn run_after(&self, delay_ms: i64) {
            self.service.push_due_drafts_with(now() + delay_ms, &self.providers()).await.unwrap();
        }
        /// Pulls the account's server drafts.
        pub(crate) async fn pull(&self) -> bool {
            self.service.pull_server_drafts(ACCOUNT, self.server.as_ref()).await.unwrap()
        }
        async fn run_disconnected(&self, delay_ms: i64) {
            self.service.push_due_drafts_with(now() + delay_ms, &HashMap::new()).await.unwrap();
        }
        pub(crate) fn status(&self, draft: &Draft) -> Option<ServerDraftStatus> {
            let current = self.db().draft(&draft.id).unwrap();
            self.db().server_draft_status(&current).unwrap()
        }
        pub(crate) fn state(&self, draft: &Draft) -> String {
            self.status(draft).unwrap().state
        }
        fn link_due(&self, draft: &Draft) -> Option<i64> {
            self.db()
                .connection()
                .unwrap()
                .query_row("SELECT due_at FROM draft_server_links WHERE draft_id=?1", [&draft.id], |row| row.get(0))
                .unwrap()
        }
        pub(crate) fn only_server_body(&self) -> String {
            let drafts = self.server.drafts();
            assert_eq!(drafts.len(), 1, "expected exactly one server draft");
            drafts.values().next().unwrap().0.clone()
        }
    }

    pub(crate) const PAST_DELAY: i64 = DRAFT_PUSH_DELAY_MS + 1;

    #[tokio::test]
    async fn a_saved_draft_waits_for_the_push_delay_then_reaches_the_server() {
        let harness = Harness::new();
        let draft = harness.edit(harness.new_draft(), "First version");
        assert_eq!(harness.state(&draft), "pending");

        // Well inside the delay, so the time the test itself takes can't cross it.
        harness.run_after(DRAFT_PUSH_DELAY_MS - 1_000).await;
        assert!(harness.server.calls().is_empty(), "nothing is pushed before the delay");

        harness.run_after(DRAFT_PUSH_DELAY_MS).await;
        assert!(harness.only_server_body().contains("First version"));
        assert_eq!(harness.state(&draft), "saved");
        let events = harness.events.lock().unwrap().clone();
        let Some(DraftEvent::Status(last)) = events.last() else { panic!("expected a status event") };
        assert_eq!(last.status.state, "saved");
        assert_eq!(last.revision, draft.revision);
    }

    #[tokio::test]
    async fn continuous_typing_pushes_once_the_longest_wait_has_passed() {
        let harness = Harness::new();
        let first = harness.edit(harness.new_draft(), "a");
        let since = harness.link_due(&first).unwrap() - DRAFT_PUSH_DELAY_MS;
        // Back-date the first pending edit so the next save is near the cap.
        harness
            .db()
            .connection()
            .unwrap()
            .execute(
                "UPDATE draft_server_links SET pending_since=?2 WHERE draft_id=?1",
                params![first.id, since - DRAFT_PUSH_MAX_WAIT_MS + 1_000],
            )
            .unwrap();
        let second = harness.edit(first, "ab");
        let due = harness.link_due(&second).unwrap();
        assert!(due <= since + 1_000 + 50, "the cap wins over the delay: {due} vs {since}");
    }

    #[tokio::test]
    async fn later_pushes_update_the_same_server_draft() {
        let harness = Harness::new();
        let draft = harness.edit(harness.new_draft(), "One");
        harness.run_after(PAST_DELAY).await;
        let draft = harness.edit(draft, "Two");
        assert_eq!(harness.state(&draft), "pending");
        harness.run_after(PAST_DELAY).await;
        assert!(harness.only_server_body().contains("Two"));
        assert_eq!(harness.server.calls()[2..], ["version server-1", "update server-1"], "checks for edits made elsewhere first");
        assert_eq!(harness.state(&draft), "saved");
    }

    #[tokio::test]
    async fn a_device_without_the_server_id_finds_the_existing_copy_by_message_id() {
        let harness = Harness::new();
        let draft = harness.edit(harness.new_draft(), "One");
        harness.run_after(PAST_DELAY).await;
        // Another device, or this one after a crash, knows the draft but not its server id.
        harness
            .db()
            .connection()
            .unwrap()
            .execute("UPDATE draft_server_links SET server_id=NULL WHERE draft_id=?1", [&draft.id])
            .unwrap();
        harness.edit(draft.clone(), "Two");
        harness.run_after(PAST_DELAY).await;
        assert!(harness.only_server_body().contains("Two"));
        assert!(harness
            .server
            .calls()
            .contains(&format!("find draft-{}@threestrands.local", draft.id)));
    }

    #[tokio::test]
    async fn a_server_copy_deleted_elsewhere_is_created_again_with_the_latest_edits() {
        let harness = Harness::new();
        let draft = harness.edit(harness.new_draft(), "One");
        harness.run_after(PAST_DELAY).await;
        harness.server.drafts().clear();
        harness.edit(draft, "Still writing");
        harness.run_after(PAST_DELAY).await;
        assert!(harness.only_server_body().contains("Still writing"));
    }

    #[tokio::test]
    async fn the_composer_closing_pushes_without_waiting() {
        let harness = Harness::new();
        let draft = harness.edit(harness.new_draft(), "Closing now");
        harness.db().push_drafts_now(Some(&draft.id), now()).unwrap();
        harness.run_after(0).await;
        assert!(harness.only_server_body().contains("Closing now"));
    }

    #[tokio::test]
    async fn a_temporary_failure_retries_with_backoff_and_keeps_it_through_edits() {
        let harness = Harness::new();
        let draft = harness.edit(harness.new_draft(), "One");
        harness.server.fail_next(ProviderError::TransientTransport("offline".into()));
        harness.run_after(PAST_DELAY).await;
        assert_eq!(harness.state(&draft), "retrying");
        let backoff = harness.link_due(&draft).unwrap();
        assert!(backoff >= now() + crate::backoff::retry_delay_secs(1) * 1_000 - 1_000);

        let draft = harness.edit(draft, "Two");
        assert_eq!(harness.link_due(&draft), Some(backoff), "editing must not skip the backoff");
        assert_eq!(harness.state(&draft), "retrying");

        harness.run_after(backoff - now() + 1).await;
        assert!(harness.only_server_body().contains("Two"));
        assert_eq!(harness.state(&draft), "saved");
    }

    #[tokio::test]
    async fn a_rejected_grant_asks_the_user_to_reconnect() {
        let harness = Harness::new();
        let draft = harness.edit(harness.new_draft(), "One");
        harness.server.fail_next(ProviderError::ReauthenticationRequired("revoked".into()));
        harness.run_after(PAST_DELAY).await;
        let status = harness.status(&draft).unwrap();
        assert_eq!(status.state, "failed");
        assert!(status.error.unwrap().contains("Reconnect you@example.com"));
    }

    #[tokio::test]
    async fn a_disconnected_account_keeps_the_draft_and_retries_later() {
        let harness = Harness::new();
        let draft = harness.edit(harness.new_draft(), "One");
        harness.run_disconnected(PAST_DELAY).await;
        let status = harness.status(&draft).unwrap();
        assert_eq!(status.state, "retrying");
        assert!(status.error.unwrap().contains("isn't connected"));
    }

    #[tokio::test]
    async fn a_draft_that_cannot_become_a_message_waits_for_an_edit() {
        let harness = Harness::new();
        let mut draft = harness.new_draft();
        draft.to = "not an address".into();
        let draft = harness.db().save_draft(draft).unwrap();
        harness.run_after(PAST_DELAY).await;
        assert_eq!(harness.state(&draft), "failed");
        assert_eq!(harness.link_due(&draft), None, "a failed build is not retried on its own");

        let mut fixed = draft;
        fixed.to = "jane@example.com".into();
        let fixed = harness.db().save_draft(fixed).unwrap();
        assert_eq!(harness.state(&fixed), "pending");
        harness.run_after(PAST_DELAY).await;
        assert_eq!(harness.state(&fixed), "saved");
    }

    #[tokio::test]
    async fn drafts_on_accounts_without_server_drafts_stay_on_this_device() {
        let harness = Harness::new();
        harness
            .db()
            .connection()
            .unwrap()
            .execute("UPDATE accounts SET provider='imap' WHERE email=?1", [ACCOUNT])
            .unwrap();
        let draft = harness.edit(harness.new_draft(), "Local");
        assert_eq!(harness.status(&draft), None);
        harness.run_after(PAST_DELAY).await;
        assert!(harness.server.calls().is_empty());
    }

    #[tokio::test]
    async fn a_provider_that_turns_out_not_to_keep_drafts_forgets_the_link() {
        let server = Arc::new(FakeDrafts::default());
        let harness = Harness::with_server(server);
        let draft = harness.edit(harness.new_draft(), "Local");
        harness.run_after(PAST_DELAY).await;
        assert_eq!(harness.status(&draft), None);
        assert!(harness.server.calls().is_empty());
    }

    #[tokio::test]
    async fn an_unedited_draft_is_never_pushed() {
        let harness = Harness::new();
        let draft = harness.new_draft();
        assert_eq!(harness.status(&draft), None);
        harness.run_after(DRAFT_PUSH_MAX_WAIT_MS).await;
        assert!(harness.server.calls().is_empty());
    }

    #[tokio::test]
    async fn discarding_deletes_the_server_copy() {
        let harness = Harness::new();
        let draft = harness.edit(harness.new_draft(), "One");
        harness.run_after(PAST_DELAY).await;
        harness.db().discard_draft(&draft.id).unwrap();
        harness.run_after(0).await;
        harness.run_after(0).await;
        assert!(harness.server.drafts().is_empty());
        assert_eq!(harness.server.calls().last().unwrap(), "delete server-1");
    }

    #[tokio::test]
    async fn discarding_before_the_first_push_still_finds_and_deletes_a_copy() {
        let harness = Harness::new();
        let draft = harness.edit(harness.new_draft(), "One");
        harness.db().discard_draft(&draft.id).unwrap();
        harness.run_after(0).await;
        harness.run_after(0).await;
        assert!(harness.server.drafts().is_empty());
        assert!(!harness.server.calls().contains(&"create".to_string()));
    }

    #[tokio::test]
    async fn the_server_copy_stays_until_the_send_completes() {
        let harness = Harness::new();
        let mut draft = harness.new_draft();
        draft.to = "jane@example.com".into();
        let draft = harness.edit(draft, "Sending");
        harness.run_after(PAST_DELAY).await;
        let item = harness
            .db()
            .queue(&draft.id, draft.revision, false, &[], None, std::path::Path::new("/unused"))
            .unwrap();
        harness.run_after(0).await;
        assert_eq!(harness.server.drafts().len(), 1, "Undo must still find the server copy");

        harness
            .db()
            .connection()
            .unwrap()
            .execute("UPDATE outbox_messages SET state='sent' WHERE id=?1", [&item.id])
            .unwrap();
        harness.run_after(SEND_PENDING_RECHECK_MS + 1).await;
        harness.run_after(SEND_PENDING_RECHECK_MS + 1).await;
        assert!(harness.server.drafts().is_empty());
    }

    #[tokio::test]
    async fn undoing_a_send_keeps_the_same_server_copy() {
        let harness = Harness::new();
        let mut draft = harness.new_draft();
        draft.to = "jane@example.com".into();
        let draft = harness.edit(draft, "Sending");
        harness.run_after(PAST_DELAY).await;
        let item = harness
            .db()
            .queue(&draft.id, draft.revision, false, &[], None, std::path::Path::new("/unused"))
            .unwrap();
        harness.run_after(0).await;
        let restored = harness.db().cancel_send(&item.id, false).unwrap();
        harness.run_after(PAST_DELAY).await;
        assert_eq!(harness.server.drafts().len(), 1);
        assert!(!harness.server.calls().iter().any(|call| call.starts_with("delete")));
        assert_eq!(harness.state(&restored), "saved");
    }

    #[tokio::test]
    async fn moving_a_draft_to_another_account_deletes_the_old_accounts_copy() {
        let harness = Harness::new();
        harness.db().adopt_account("other@example.com").unwrap();
        let draft = harness.edit(harness.new_draft(), "One");
        harness.run_after(PAST_DELAY).await;
        harness.db().set_draft_account(&draft.id, "other@example.com").unwrap();
        harness.run_after(0).await;
        assert!(harness.server.drafts().is_empty(), "the old account's copy is deleted");
        let pending: i64 = harness
            .db()
            .connection()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM draft_server_links WHERE draft_id=?1 AND account='other@example.com' AND server_id IS NULL",
                [&draft.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending, 1, "the new account gets its own copy");
    }

    #[tokio::test]
    async fn a_draft_removed_by_another_device_is_left_for_that_device_to_delete() {
        let harness = Harness::new();
        let draft = harness.edit(harness.new_draft(), "One");
        harness.run_after(PAST_DELAY).await;
        harness
            .db()
            .connection()
            .unwrap()
            .execute("DELETE FROM drafts WHERE id=?1", [&draft.id])
            .unwrap();
        harness.db().forget_server_draft_link(&draft.id).unwrap();
        harness.run_after(DRAFT_PUSH_MAX_WAIT_MS).await;
        assert_eq!(harness.server.drafts().len(), 1);
    }

    #[tokio::test]
    async fn quitting_pushes_pending_drafts_right_away() {
        let harness = Harness::new();
        let draft = harness.edit(harness.new_draft(), "Before quitting");
        harness.db().push_drafts_now(None, now()).unwrap();
        assert!(harness.link_due(&draft).unwrap() <= now());
    }

    #[test]
    fn saved_drafts_never_store_their_server_status() {
        let database = Database::open_memory();
        database.adopt_account(ACCOUNT).unwrap();
        let mut draft = database.create_draft("new", None, ACCOUNT, &[]).unwrap();
        draft.body = "Body".into();
        let saved = database.save_draft(draft).unwrap();
        let payload: String = database
            .connection()
            .unwrap()
            .query_row("SELECT payload FROM drafts WHERE id=?1", [&saved.id], |row| row.get(0))
            .unwrap();
        assert!(!payload.contains("serverDraft"));
    }
}
