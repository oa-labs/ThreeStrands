use std::{
    collections::HashSet,
    sync::{Arc, Mutex as StdMutex},
    time::{Duration, Instant},
};

use chrono::{Duration as ChronoDuration, Utc};
use rand::Rng;
use tokio::sync::Mutex;

use crate::{
    auth::GoogleAuth,
    db::{Database, PendingMutation},
    gmail::{GmailClient, GmailProvider, ProviderError, ProviderResult},
    mime::{
        normalize, normalized_size, NormalizedMessage, MAX_NORMALIZED_THREAD_BYTES,
        MAX_THREAD_MESSAGES,
    },
    models::{Label, SyncStatus, ThreadMutation},
};

/// Floor used by adaptive polling and by resume/foreground catch-up.
pub const MIN_POLL_INTERVAL: Duration = Duration::from_secs(15);
const MAX_POLL_INTERVAL: Duration = Duration::from_secs(300);
const MUTATION_RETRY_BASE_SECS: i64 = 30;
const MUTATION_RETRY_MAX_SECS: i64 = 60 * 60;

/// How often the background poll loop re-derives inbox membership from
/// Gmail's live INBOX listing, independent of history-based incremental
/// sync. History sync can miss a label change reaching us — e.g. Gmail-side
/// propagation lag on a change made outside Dispatch — and nothing else
/// self-heals that short of a historyId 404. This is a safety net, not the
/// primary sync path, so it runs rarely.
const RECONCILE_INTERVAL_SECS: i64 = 6 * 60 * 60;
const RECOVERY_BATCH_SIZE: usize = 50;

#[derive(Clone)]
pub struct SyncService {
    database: Arc<Database>,
    auth: GoogleAuth,
    gate: Arc<Mutex<()>>,
    last_attempt: Arc<StdMutex<Option<Instant>>>,
}

pub fn should_skip_stale_sync(
    last_attempt: Option<Instant>,
    now: Instant,
    min_age: Duration,
) -> bool {
    last_attempt.is_some_and(|attempt| now.saturating_duration_since(attempt) < min_age)
}

impl SyncService {
    pub fn new(database: Arc<Database>, auth: GoogleAuth) -> Self {
        Self {
            database,
            auth,
            gate: Arc::new(Mutex::new(())),
            last_attempt: Arc::new(StdMutex::new(None)),
        }
    }

    /// Whether this service's account currently has usable Google credentials.
    pub fn is_connected(&self) -> bool {
        self.auth.available()
            && self
                .database
                .account_is_connected(&self.account_id())
                .unwrap_or(false)
    }

    /// The local key for this account's cursor/threads/mutations rows. Reads
    /// through to `auth`'s live keychain key, so it stays correct across a
    /// rekey (e.g. the primary account resolving its real address after
    /// startup) without this service needing to be reconstructed.
    fn account_id(&self) -> String {
        self.auth.key()
    }

    pub async fn sync(&self) -> Result<SyncStatus, String> {
        self.sync_provider()
            .await
            .map_err(|error| error.to_string())
    }

    async fn sync_provider(&self) -> ProviderResult<SyncStatus> {
        let _guard = self.gate.lock().await;
        let account_id = self.account_id();
        let provider = GmailClient::new(self.auth.clone());
        let result = sync_with(self.database.as_ref(), &account_id, &provider).await;
        if let Ok(mut last_attempt) = self.last_attempt.lock() {
            *last_attempt = Some(Instant::now());
        }
        if let Err(error) = result {
            let message = error.to_string();
            self.database
                .fail_sync(&account_id, &message)
                .map_err(ProviderError::Other)?;
            return Err(error);
        }
        self.database
            .sync_status(&account_id)
            .map_err(ProviderError::Other)
    }

    /// Incremental catch-up for OS resume / window focus. Skips if polling
    /// or another catch-up already ran within [`MIN_POLL_INTERVAL`].
    pub async fn sync_if_stale(&self) -> Result<SyncStatus, String> {
        let last_attempt = self.last_attempt.lock().ok().and_then(|guard| *guard);
        if should_skip_stale_sync(last_attempt, Instant::now(), MIN_POLL_INTERVAL) {
            return self.database.sync_status(&self.account_id());
        }
        self.sync().await
    }

    pub async fn flush_pending(&self) -> Result<SyncStatus, String> {
        let account_id = self.account_id();
        if self.database.sync_status(&account_id)?.pending_mutations == 0 {
            return self.database.sync_status(&account_id);
        }
        if !self.auth.available() {
            return self.database.sync_status(&account_id);
        }
        let _guard = self.gate.lock().await;
        let provider = GmailClient::new(self.auth.clone());
        if let Err(error) = flush_pending_with(self.database.as_ref(), &account_id, &provider).await
        {
            let message = error.to_string();
            self.database.fail_sync(&account_id, &message)?;
            return Err(message);
        }
        self.database.sync_status(&account_id)
    }

    pub async fn create_label(&self, name: &str) -> Result<Label, String> {
        validate_label_name(name)?;
        GmailClient::new(self.auth.clone())
            .create_label(name)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn update_label(&self, id: &str, name: &str) -> Result<Label, String> {
        validate_label_name(name)?;
        GmailClient::new(self.auth.clone())
            .update_label(id, name)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn delete_label(&self, id: &str) -> Result<(), String> {
        GmailClient::new(self.auth.clone())
            .delete_label(id)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn polling_loop(self) {
        let mut delay = MIN_POLL_INTERVAL;
        loop {
            // Jitter avoids multiple accounts/instances recovering from the
            // same outage and retrying in lockstep; `next_poll_delay` itself
            // stays deterministic so its unit tests aren't flaky.
            let jitter = Duration::from_millis(rand::thread_rng().gen_range(0..250));
            tokio::time::sleep(delay + jitter).await;
            if !self.is_connected() {
                delay = Duration::from_secs(30);
                continue;
            }
            let before = self
                .database
                .sync_status(&self.account_id())
                .map(|status| status.pending_mutations)
                .unwrap_or_default();
            delay = next_poll_delay(delay, before, self.sync_provider().await);
            self.reconcile_if_due().await;
        }
    }

    /// Best-effort periodic reconciliation; see [`RECONCILE_INTERVAL_SECS`].
    /// Failures are swallowed and retried on a subsequent poll since this runs
    /// alongside the primary sync path, which already surfaces its own errors.
    async fn reconcile_if_due(&self) {
        let account_id = self.account_id();
        if self
            .database
            .account_needs_reauth(&account_id)
            .unwrap_or(false)
        {
            return;
        }
        let due = self
            .database
            .reconciliation_due(&account_id, RECONCILE_INTERVAL_SECS)
            .unwrap_or(false);
        if !due {
            return;
        }
        let _guard = self.gate.lock().await;
        let provider = GmailClient::new(self.auth.clone());
        let _ = reconcile_and_mark(self.database.as_ref(), &account_id, &provider).await;
    }
}

fn next_poll_delay(
    current: Duration,
    pending_before: i64,
    result: ProviderResult<SyncStatus>,
) -> Duration {
    match result {
        Ok(status) if pending_before > 0 || status.pending_mutations > 0 => MIN_POLL_INTERVAL,
        Ok(_) => (current * 2).min(MAX_POLL_INTERVAL),
        Err(ProviderError::RetryableServer(_)) => MAX_POLL_INTERVAL,
        Err(_) => Duration::from_secs(60),
    }
}

fn validate_label_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() || name.len() > 225 {
        Err("Label name must contain 1–225 characters".into())
    } else {
        Ok(())
    }
}

pub async fn flush_pending_with(
    database: &Database,
    account_id: &str,
    provider: &(impl GmailProvider + ?Sized),
) -> ProviderResult<()> {
    if database
        .account_needs_reauth(account_id)
        .map_err(ProviderError::Other)?
    {
        return Ok(());
    }
    if database
        .sync_status(account_id)
        .map_err(ProviderError::Other)?
        .pending_mutations
        == 0
    {
        return Ok(());
    }
    let result = deliver_mutations(database, account_id, provider).await;
    pause_for_permanent_auth_failure(database, account_id, result)
}

pub async fn sync_with(
    database: &Database,
    account_id: &str,
    provider: &(impl GmailProvider + ?Sized),
) -> ProviderResult<()> {
    if database
        .account_needs_reauth(account_id)
        .map_err(ProviderError::Other)?
    {
        return Ok(());
    }
    let result = sync_active_with(database, account_id, provider).await;
    pause_for_permanent_auth_failure(database, account_id, result)
}

async fn sync_active_with(
    database: &Database,
    account_id: &str,
    provider: &(impl GmailProvider + ?Sized),
) -> ProviderResult<()> {
    deliver_mutations(database, account_id, provider).await?;
    match database.cursor(account_id).map_err(ProviderError::Other)? {
        Some(cursor) => match incremental_sync(database, account_id, provider, &cursor).await {
            Err(ProviderError::InvalidCursor) => full_sync(database, account_id, provider).await,
            result => result,
        },
        None => full_sync(database, account_id, provider).await,
    }
}

fn pause_for_permanent_auth_failure(
    database: &Database,
    account_id: &str,
    result: ProviderResult<()>,
) -> ProviderResult<()> {
    if let Err(error) = &result {
        if error.requires_reauthentication() {
            database
                .mark_account_needs_reauth(account_id, &error.to_string())
                .map_err(ProviderError::Other)?;
        }
    }
    result
}

async fn full_sync(
    database: &Database,
    account_id: &str,
    provider: &(impl GmailProvider + ?Sized),
) -> ProviderResult<()> {
    match full_sync_attempt(database, account_id, provider).await {
        Err(ProviderError::InvalidCursor) => {
            database
                .discard_sync_recovery(account_id)
                .map_err(ProviderError::Other)?;
            full_sync_attempt(database, account_id, provider).await
        }
        result => result,
    }
}

async fn full_sync_attempt(
    database: &Database,
    account_id: &str,
    provider: &(impl GmailProvider + ?Sized),
) -> ProviderResult<()> {
    // An interrupted recovery must keep the normal sync cursor cleared rather
    // than running incrementally against an incomplete snapshot. Its separate,
    // durable generation lets the expensive thread refresh resume safely.
    database
        .clear_cursor(account_id)
        .map_err(ProviderError::Other)?;
    let starting_cursor = match database
        .recovery_cursor(account_id)
        .map_err(ProviderError::Other)?
    {
        Some(cursor) => cursor,
        None => {
            // Capture the cursor before listing. The following history pass
            // closes the race with mail arriving while the list is downloaded.
            let cursor = provider.profile_history_id().await?;
            let server_inbox_ids = list_inbox_thread_ids(provider).await?;
            let local_inbox_ids = local_inbox_thread_ids(database, account_id)?;
            let recovery_ids = server_inbox_ids
                .union(&local_inbox_ids)
                .cloned()
                .collect::<Vec<_>>();
            database
                .begin_sync_recovery(account_id, &cursor, &recovery_ids)
                .map_err(ProviderError::Other)?;
            cursor
        }
    };
    loop {
        let batch = database
            .pending_sync_recovery_threads(account_id, RECOVERY_BATCH_SIZE)
            .map_err(ProviderError::Other)?;
        if batch.is_empty() {
            break;
        }
        ingest_threads(database, account_id, provider, batch.clone()).await?;
        database
            .complete_sync_recovery_threads(account_id, &batch)
            .map_err(ProviderError::Other)?;
    }
    incremental_sync(database, account_id, provider, &starting_cursor).await
}

async fn incremental_sync(
    database: &Database,
    account_id: &str,
    provider: &(impl GmailProvider + ?Sized),
    cursor: &str,
) -> ProviderResult<()> {
    let mut page = None;
    let mut changed = HashSet::new();
    let final_cursor = loop {
        let result = provider.history(cursor, page.as_deref()).await?;
        changed.extend(result.thread_ids);
        page = result.next_page_token;
        if page.is_none() {
            break result.history_id;
        }
    };
    ingest_threads(
        database,
        account_id,
        provider,
        changed.into_iter().collect(),
    )
    .await?;
    database
        .finish_sync(account_id, &final_cursor)
        .map_err(ProviderError::Other)
}

async fn list_inbox_thread_ids(
    provider: &(impl GmailProvider + ?Sized),
) -> ProviderResult<HashSet<String>> {
    let mut page = None;
    let mut server_inbox_ids = HashSet::new();
    loop {
        let result = provider.list_threads(page.as_deref()).await?;
        server_inbox_ids.extend(result.thread_ids);
        page = result.next_page_token;
        if page.is_none() {
            break;
        }
    }
    Ok(server_inbox_ids)
}

fn local_inbox_thread_ids(
    database: &Database,
    account_id: &str,
) -> ProviderResult<HashSet<String>> {
    Ok(database
        .local_inbox_provider_thread_ids(account_id)
        .map_err(ProviderError::Other)?
        .into_iter()
        .collect())
}

/// Diffs Gmail's current INBOX thread listing against what's cached locally
/// and re-ingests only threads whose inbox membership drifted. Unlike cursor
/// recovery, this periodic safety net deliberately avoids refreshing common
/// threads.
async fn reconcile_inbox(
    database: &Database,
    account_id: &str,
    provider: &(impl GmailProvider + ?Sized),
) -> ProviderResult<()> {
    let server_inbox_ids = list_inbox_thread_ids(provider).await?;
    let local_inbox_ids = local_inbox_thread_ids(database, account_id)?;
    let drifted: Vec<String> = server_inbox_ids
        .symmetric_difference(&local_inbox_ids)
        .cloned()
        .collect();
    if drifted.is_empty() {
        return Ok(());
    }
    ingest_threads(database, account_id, provider, drifted).await
}

async fn reconcile_and_mark(
    database: &Database,
    account_id: &str,
    provider: &(impl GmailProvider + ?Sized),
) -> ProviderResult<()> {
    reconcile_inbox(database, account_id, provider).await?;
    database
        .mark_reconciled(account_id)
        .map_err(ProviderError::Other)
}

async fn ingest_threads(
    database: &Database,
    account_id: &str,
    provider: &(impl GmailProvider + ?Sized),
    ids: Vec<String>,
) -> ProviderResult<()> {
    let mut ingested_threads = Vec::with_capacity(ids.len());
    let mut deleted = Vec::new();
    for id in ids {
        let messages = match provider.get_thread(&id).await {
            Ok(messages) => messages,
            Err(ProviderError::NotFound) => {
                deleted.push(id);
                continue;
            }
            Err(error) => return Err(error),
        };
        let (normalized, quarantined) = normalize_thread(&messages);
        ingested_threads.push((id, normalized, quarantined));
    }
    database
        .apply_ingested_gmail_threads(account_id, &ingested_threads)
        .map_err(ProviderError::Other)?;
    for id in deleted {
        database
            .delete_gmail_thread(account_id, &id)
            .map_err(ProviderError::Other)?;
    }
    Ok(())
}

fn normalize_thread(
    messages: &[crate::mime::GmailMessage],
) -> (Vec<NormalizedMessage>, Vec<(String, String)>) {
    let mut normalized = Vec::with_capacity(messages.len().min(MAX_THREAD_MESSAGES));
    let mut quarantined = Vec::new();
    let mut thread_size = 0_usize;
    for (index, message) in messages.iter().enumerate() {
        if index >= MAX_THREAD_MESSAGES {
            quarantined.push((
                message.id.clone(),
                format!("Thread exceeds the {MAX_THREAD_MESSAGES} message limit"),
            ));
            continue;
        }
        match normalize(message) {
            Ok(candidate) => {
                let Some(candidate_size) = normalized_size(&candidate) else {
                    quarantined.push((
                        message.id.clone(),
                        "Normalized message size overflow".to_string(),
                    ));
                    continue;
                };
                let Some(next_size) = thread_size.checked_add(candidate_size) else {
                    quarantined.push((
                        message.id.clone(),
                        "Normalized thread size overflow".to_string(),
                    ));
                    continue;
                };
                if next_size > MAX_NORMALIZED_THREAD_BYTES {
                    quarantined.push((
                        message.id.clone(),
                        format!(
                            "Normalized thread exceeds the {} MB limit",
                            MAX_NORMALIZED_THREAD_BYTES / 1024 / 1024
                        ),
                    ));
                    continue;
                }
                thread_size = next_size;
                normalized.push(candidate);
            }
            Err(error) => quarantined.push((message.id.clone(), error)),
        }
    }
    (normalized, quarantined)
}

async fn deliver_mutations(
    database: &Database,
    account_id: &str,
    provider: &(impl GmailProvider + ?Sized),
) -> ProviderResult<()> {
    loop {
        let mutations = database
            .claim_mutations(account_id, 50)
            .map_err(ProviderError::Other)?;
        if mutations.is_empty() {
            return Ok(());
        }
        let mut index = 0;
        while index < mutations.len() {
            let mutation = &mutations[index];
            let (add, remove) = mutation_labels(mutation);
            let mut batch_end = index + 1;
            let result = match &mutation.mutation {
                ThreadMutation::Spam { value, .. } => {
                    while batch_end < mutations.len()
                        && matches!(
                            &mutations[batch_end].mutation,
                            ThreadMutation::Spam { value: next, .. } if next == value
                        )
                    {
                        batch_end += 1;
                    }
                    let message_ids = mutations[index..batch_end]
                        .iter()
                        .map(|item| database.message_ids_for_thread(item.mutation.thread_id()))
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(ProviderError::Other)?
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>();
                    provider.modify_messages(&message_ids, &add, &remove).await
                }
                ThreadMutation::Star { .. }
                | ThreadMutation::Label { .. }
                | ThreadMutation::Read { value: false, .. } => {
                    match mutation.target_message_id.as_ref() {
                        Some(message_id) => {
                            provider
                                .modify_messages(std::slice::from_ref(message_id), &add, &remove)
                                .await
                        }
                        None => Err(ProviderError::InvalidOperation(
                            "Mutation target message is unavailable".into(),
                        )),
                    }
                }
                _ => {
                    provider
                        .modify_thread(&mutation.provider_thread_id, &add, &remove)
                        .await
                }
            };
            match result {
                Ok(()) => {
                    for item in &mutations[index..batch_end] {
                        database
                            .complete_mutation(&item.id)
                            .map_err(ProviderError::Other)?;
                    }
                }
                Err(error) if error.requires_reauthentication() => {
                    database
                        .mark_account_needs_reauth(account_id, &error.to_string())
                        .map_err(ProviderError::Other)?;
                    return Err(error);
                }
                Err(error) if error.retry_mutation() => {
                    // Every mutation still claimed in this batch — not just
                    // the group that hit the temporary failure — must go back
                    // to `pending` with a durable retry time. Anything left `running` here would
                    // otherwise sit unclaimed and undelivered until the next
                    // app restart, since only startup recovery clears stuck
                    // `running` rows.
                    for item in &mutations[index..] {
                        let next_attempt_at = mutation_next_attempt_at(item.attempts);
                        database
                            .reject_mutation(&item.id, &error.to_string(), Some(&next_attempt_at))
                            .map_err(ProviderError::Other)?;
                    }
                    return Err(error);
                }
                Err(error) => {
                    for item in &mutations[index..batch_end] {
                        database
                            .reject_mutation(&item.id, &error.to_string(), None)
                            .map_err(ProviderError::Other)?;
                    }
                }
            }
            index = batch_end;
        }
    }
}

fn mutation_next_attempt_at(attempts: u32) -> String {
    let delay = mutation_retry_delay_secs(attempts);
    (Utc::now() + ChronoDuration::seconds(delay)).to_rfc3339()
}

fn mutation_retry_delay_secs(attempts: u32) -> i64 {
    let exponent = attempts.saturating_sub(1).min(16);
    MUTATION_RETRY_BASE_SECS
        .saturating_mul(1_i64 << exponent)
        .min(MUTATION_RETRY_MAX_SECS)
}

fn mutation_labels(mutation: &PendingMutation) -> (Vec<String>, Vec<String>) {
    // Trashing/untrashing moves the thread across INBOX as well as TRASH,
    // matching Gmail's own trash/untrash behavior, so it needs both arrays
    // rather than the single label toggle the other mutation kinds use.
    if let ThreadMutation::Trash { value, .. } = &mutation.mutation {
        return if *value {
            (vec!["TRASH".to_string()], vec!["INBOX".to_string()])
        } else {
            (vec!["INBOX".to_string()], vec!["TRASH".to_string()])
        };
    }
    if let ThreadMutation::Spam { value, .. } = &mutation.mutation {
        return if *value {
            (vec!["SPAM".to_string()], vec!["INBOX".to_string()])
        } else {
            (vec!["INBOX".to_string()], vec!["SPAM".to_string()])
        };
    }
    let (label, value) = match &mutation.mutation {
        ThreadMutation::Archive { value, .. } => ("INBOX", !value),
        ThreadMutation::Trash { .. } => unreachable!(),
        ThreadMutation::Spam { .. } => unreachable!(),
        ThreadMutation::Read { value, .. } => ("UNREAD", !value),
        ThreadMutation::Star { value, .. } => ("STARRED", *value),
        ThreadMutation::Label {
            label_id, value, ..
        } => (label_id.as_str(), *value),
    };
    if value {
        (vec![label.to_string()], vec![])
    } else {
        (vec![], vec![label.to_string()])
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Mutex as StdMutex,
    };

    use async_trait::async_trait;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

    use super::*;
    use crate::{
        gmail::{HistoryPage, ThreadPage},
        mime::{GmailMessage, MimeBody, MimeHeader, MimePart},
    };

    struct ContractProvider {
        invalidate_stale_cursor: AtomicBool,
        full_lists: AtomicUsize,
        modifies: AtomicUsize,
        message_modifies: StdMutex<Vec<(Vec<String>, Vec<String>, Vec<String>)>>,
        fail_mutation: bool,
        permanently_fail_mutation: bool,
        reauth_mutation: bool,
        thread_messages: Option<Vec<GmailMessage>>,
    }

    impl ContractProvider {
        fn normal() -> Self {
            Self {
                invalidate_stale_cursor: AtomicBool::new(false),
                full_lists: AtomicUsize::new(0),
                modifies: AtomicUsize::new(0),
                message_modifies: StdMutex::new(vec![]),
                fail_mutation: false,
                permanently_fail_mutation: false,
                reauth_mutation: false,
                thread_messages: None,
            }
        }

        fn message() -> GmailMessage {
            GmailMessage {
                id: "message-1".into(),
                thread_id: "gmail-thread".into(),
                label_ids: vec!["INBOX".into(), "UNREAD".into()],
                snippet: "provider snippet".into(),
                internal_date: "1700000000000".into(),
                payload: MimePart {
                    mime_type: "text/plain".into(),
                    headers: vec![
                        MimeHeader {
                            name: "Subject".into(),
                            value: "Provider subject".into(),
                        },
                        MimeHeader {
                            name: "From".into(),
                            value: "sender@example.com".into(),
                        },
                    ],
                    body: MimeBody {
                        data: Some(URL_SAFE_NO_PAD.encode("provider body")),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            }
        }
    }

    #[async_trait]
    impl GmailProvider for ContractProvider {
        async fn profile_history_id(&self) -> ProviderResult<String> {
            Ok("current".into())
        }

        async fn list_threads(&self, page: Option<&str>) -> ProviderResult<ThreadPage> {
            assert!(page.is_none());
            self.full_lists.fetch_add(1, Ordering::SeqCst);
            Ok(ThreadPage {
                thread_ids: vec!["gmail-thread".into()],
                next_page_token: None,
            })
        }

        async fn get_thread(&self, id: &str) -> ProviderResult<Vec<GmailMessage>> {
            // Anything besides the one thread this fake Gmail actually
            // knows about — e.g. `Database::open_memory()`'s local-only
            // demo/welcome thread — isn't real Gmail mail, so a real
            // history/inbox reconciliation would 404 on it, same as here.
            if id != "gmail-thread" {
                return Err(ProviderError::NotFound);
            }
            Ok(self
                .thread_messages
                .clone()
                .unwrap_or_else(|| vec![Self::message()]))
        }

        async fn history(&self, cursor: &str, page: Option<&str>) -> ProviderResult<HistoryPage> {
            assert!(page.is_none());
            if cursor == "stale" && self.invalidate_stale_cursor.swap(false, Ordering::SeqCst) {
                return Err(ProviderError::InvalidCursor);
            }
            Ok(HistoryPage {
                thread_ids: vec![],
                history_id: "current".into(),
                next_page_token: None,
            })
        }

        async fn modify_thread(
            &self,
            _id: &str,
            _add: &[String],
            _remove: &[String],
        ) -> ProviderResult<()> {
            self.modifies.fetch_add(1, Ordering::SeqCst);
            if self.permanently_fail_mutation {
                Err(ProviderError::PermanentClientRejection(
                    "invalid label".into(),
                ))
            } else if self.reauth_mutation {
                Err(ProviderError::ReauthenticationRequired(
                    "invalid_grant".into(),
                ))
            } else if self.fail_mutation {
                Err(ProviderError::RetryableServer("rate limited".into()))
            } else {
                Ok(())
            }
        }

        async fn modify_messages(
            &self,
            ids: &[String],
            add: &[String],
            remove: &[String],
        ) -> ProviderResult<()> {
            self.modifies.fetch_add(1, Ordering::SeqCst);
            self.message_modifies.lock().unwrap().push((
                ids.to_vec(),
                add.to_vec(),
                remove.to_vec(),
            ));
            if self.permanently_fail_mutation {
                Err(ProviderError::PermanentClientRejection(
                    "invalid label".into(),
                ))
            } else if self.reauth_mutation {
                Err(ProviderError::ReauthenticationRequired(
                    "persistent 401".into(),
                ))
            } else if self.fail_mutation {
                Err(ProviderError::RetryableServer("rate limited".into()))
            } else {
                Ok(())
            }
        }

        async fn list_labels(&self) -> ProviderResult<Vec<Label>> {
            Ok(vec![])
        }

        async fn create_label(&self, _name: &str) -> ProviderResult<Label> {
            unreachable!()
        }

        async fn update_label(&self, _id: &str, _name: &str) -> ProviderResult<Label> {
            unreachable!()
        }

        async fn delete_label(&self, _id: &str) -> ProviderResult<()> {
            unreachable!()
        }
    }

    #[test]
    fn polling_backoff_uses_structured_rate_limit_classification() {
        assert_eq!(
            next_poll_delay(
                MIN_POLL_INTERVAL,
                0,
                Err(ProviderError::RetryableServer("rate limited".into())),
            ),
            MAX_POLL_INTERVAL
        );
        assert_eq!(
            next_poll_delay(
                MIN_POLL_INTERVAL,
                0,
                Err(ProviderError::Other(
                    "display text happens to mention rate limit".into(),
                )),
            ),
            Duration::from_secs(60)
        );
    }

    #[tokio::test]
    async fn provider_contract_imports_and_advances_cursor() {
        let database = Database::open_memory();
        let provider = ContractProvider::normal();
        sync_with(&database, "default", &provider).await.unwrap();
        assert_eq!(
            database.cursor("default").unwrap().as_deref(),
            Some("current")
        );
        assert_eq!(provider.full_lists.load(Ordering::SeqCst), 1);
        assert_eq!(
            database.list_threads(None).unwrap()[0].id,
            "default:gmail-thread"
        );
    }

    #[tokio::test]
    async fn malformed_message_is_quarantined_without_blocking_cursor_or_thread() {
        let database = Database::open_memory();
        let valid = ContractProvider::message();
        let mut malformed = valid.clone();
        malformed.id = "malformed-message".into();
        malformed.payload.body.data = Some("*** invalid base64 ***".into());
        let provider = ContractProvider {
            thread_messages: Some(vec![valid, malformed]),
            ..ContractProvider::normal()
        };

        sync_with(&database, "default", &provider).await.unwrap();

        assert_eq!(
            database.cursor("default").unwrap().as_deref(),
            Some("current")
        );
        assert_eq!(database.list_threads(None).unwrap().len(), 1);
        let status = database.sync_status("default").unwrap();
        assert_eq!(status.quarantined_messages.len(), 1);
        assert_eq!(
            status.quarantined_messages[0].message_id,
            "malformed-message"
        );
        assert!(status.quarantined_messages[0]
            .error
            .contains("Invalid Gmail base64url body"));
    }

    #[tokio::test]
    async fn successful_reingestion_clears_a_message_quarantine() {
        let database = Database::open_memory();
        let mut malformed = ContractProvider::message();
        malformed.payload.body.data = Some("*** invalid base64 ***".into());
        let malformed_provider = ContractProvider {
            thread_messages: Some(vec![malformed]),
            ..ContractProvider::normal()
        };
        ingest_threads(
            &database,
            "default",
            &malformed_provider,
            vec!["gmail-thread".into()],
        )
        .await
        .unwrap();
        assert_eq!(
            database
                .sync_status("default")
                .unwrap()
                .quarantined_messages
                .len(),
            1
        );

        ingest_threads(
            &database,
            "default",
            &ContractProvider::normal(),
            vec!["gmail-thread".into()],
        )
        .await
        .unwrap();
        assert!(database
            .sync_status("default")
            .unwrap()
            .quarantined_messages
            .is_empty());
    }

    #[test]
    fn thread_message_limit_accepts_the_boundary_and_quarantines_the_remainder() {
        let messages: Vec<_> = (0..=MAX_THREAD_MESSAGES)
            .map(|index| {
                let mut message = ContractProvider::message();
                message.id = format!("message-{index}");
                message
            })
            .collect();

        let (normalized, quarantined) = normalize_thread(&messages);

        assert_eq!(normalized.len(), MAX_THREAD_MESSAGES);
        assert_eq!(quarantined.len(), 1);
        assert_eq!(quarantined[0].0, format!("message-{MAX_THREAD_MESSAGES}"));
        assert!(quarantined[0].1.contains("message limit"));
    }

    #[test]
    fn normalized_thread_size_quarantines_only_the_message_over_the_limit() {
        let mut first = ContractProvider::message();
        first.snippet = "a".repeat(5 * 1024 * 1024);
        let mut second = first.clone();
        second.id = "message-2".into();

        let (normalized, quarantined) = normalize_thread(&[first, second]);

        assert_eq!(normalized.len(), 1);
        assert_eq!(quarantined.len(), 1);
        assert_eq!(quarantined[0].0, "message-2");
        assert!(quarantined[0].1.contains("Normalized thread"));
    }

    #[tokio::test]
    async fn invalid_history_cursor_recovers_with_full_resync() {
        let database = Database::open_memory();
        database.finish_sync("default", "stale").unwrap();
        let provider = ContractProvider {
            invalidate_stale_cursor: AtomicBool::new(true),
            ..ContractProvider::normal()
        };
        sync_with(&database, "default", &provider).await.unwrap();
        assert_eq!(provider.full_lists.load(Ordering::SeqCst), 1);
        assert_eq!(
            database.cursor("default").unwrap().as_deref(),
            Some("current")
        );
    }

    #[tokio::test]
    async fn rate_limit_leaves_claimed_mutation_durable_for_retry() {
        let database = Database::open_memory();
        database
            .mutate_thread(&ThreadMutation::Star {
                thread_id: "welcome".into(),
                value: true,
            })
            .unwrap();
        let provider = ContractProvider {
            fail_mutation: true,
            ..ContractProvider::normal()
        };
        assert!(matches!(
            sync_with(&database, "default", &provider).await,
            Err(ProviderError::RetryableServer(_))
        ));
        assert_eq!(
            database.sync_status("default").unwrap().pending_mutations,
            1
        );
        assert!(
            database.claim_mutations("default", 10).unwrap().is_empty(),
            "a retry must not be claimable before its persisted next_attempt_at"
        );
    }

    #[tokio::test]
    async fn permanent_auth_failure_marks_account_and_pauses_queued_work() {
        let database = Database::open_memory();
        database.adopt_account("work@example.com").unwrap();
        database
            .mutate_thread(&ThreadMutation::Star {
                thread_id: "welcome".into(),
                value: true,
            })
            .unwrap();
        let provider = ContractProvider {
            reauth_mutation: true,
            ..ContractProvider::normal()
        };

        assert!(matches!(
            sync_with(&database, "work@example.com", &provider).await,
            Err(ProviderError::ReauthenticationRequired(_))
        ));
        assert_eq!(
            database
                .get_account("work@example.com")
                .unwrap()
                .unwrap()
                .status,
            "needs_reauth"
        );
        assert_eq!(
            database
                .sync_status("work@example.com")
                .unwrap()
                .pending_mutations,
            1
        );
        let attempts = provider.modifies.load(Ordering::SeqCst);
        sync_with(&database, "work@example.com", &provider)
            .await
            .unwrap();
        assert_eq!(
            provider.modifies.load(Ordering::SeqCst),
            attempts,
            "paused accounts must not retry provider work"
        );
        assert!(database
            .claim_mutations("work@example.com", 10)
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn transient_token_failure_keeps_account_connected_and_retryable() {
        let database = Database::open_memory();
        database.adopt_account("work@example.com").unwrap();
        database
            .mutate_thread(&ThreadMutation::Star {
                thread_id: "welcome".into(),
                value: true,
            })
            .unwrap();
        struct TransientAuthProvider;
        #[async_trait]
        impl GmailProvider for TransientAuthProvider {
            async fn profile_history_id(&self) -> ProviderResult<String> {
                unreachable!()
            }
            async fn list_threads(&self, _: Option<&str>) -> ProviderResult<ThreadPage> {
                unreachable!()
            }
            async fn get_thread(&self, _: &str) -> ProviderResult<Vec<GmailMessage>> {
                unreachable!()
            }
            async fn history(&self, _: &str, _: Option<&str>) -> ProviderResult<HistoryPage> {
                unreachable!()
            }
            async fn modify_thread(
                &self,
                _: &str,
                _: &[String],
                _: &[String],
            ) -> ProviderResult<()> {
                Err(ProviderError::Authentication(
                    "token endpoint unavailable".into(),
                ))
            }
            async fn modify_messages(
                &self,
                _: &[String],
                _: &[String],
                _: &[String],
            ) -> ProviderResult<()> {
                Err(ProviderError::Authentication(
                    "token endpoint unavailable".into(),
                ))
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

        assert!(matches!(
            sync_with(&database, "work@example.com", &TransientAuthProvider).await,
            Err(ProviderError::Authentication(_))
        ));
        assert_eq!(
            database
                .get_account("work@example.com")
                .unwrap()
                .unwrap()
                .status,
            "connected"
        );
        assert_eq!(
            database
                .sync_status("work@example.com")
                .unwrap()
                .pending_mutations,
            1
        );
    }

    #[test]
    fn mutation_backoff_is_exponential_and_bounded() {
        assert_eq!(mutation_retry_delay_secs(1), 30);
        assert_eq!(mutation_retry_delay_secs(2), 60);
        assert_eq!(mutation_retry_delay_secs(8), 3_600);
        assert_eq!(mutation_retry_delay_secs(u32::MAX), 3_600);
    }

    #[tokio::test]
    async fn permanent_mutation_failure_is_exposed_by_sync_status() {
        let database = Database::open_memory();
        database
            .mutate_thread(&ThreadMutation::Archive {
                thread_id: "welcome".into(),
                value: true,
            })
            .unwrap();
        let provider = ContractProvider {
            permanently_fail_mutation: true,
            ..ContractProvider::normal()
        };

        sync_with(&database, "default", &provider).await.unwrap();

        let status = database.sync_status("default").unwrap();
        assert_eq!(status.pending_mutations, 0);
        assert_eq!(status.failed_mutations.len(), 1);
        assert_eq!(status.failed_mutations[0].kind, "archive");
        assert!(status.failed_mutations[0].error.contains("invalid label"));
    }

    #[tokio::test]
    async fn rate_limit_resets_the_entire_remaining_claimed_batch_to_pending() {
        let database = Database::open_memory();
        let mut second = ContractProvider::message();
        second.id = "message-2".into();
        second.thread_id = "second-thread".into();
        database
            .upsert_gmail_thread("default", &[crate::mime::normalize(&second).unwrap()])
            .unwrap();
        // Two separately-delivered mutations (different threads, so each is
        // its own batch group). The provider fails every delivery attempt,
        // so only the first group is ever attempted — the regression this
        // guards against is the second, never-attempted group being left
        // `running` forever instead of also reset to `pending`.
        database
            .mutate_thread(&ThreadMutation::Star {
                thread_id: "welcome".into(),
                value: true,
            })
            .unwrap();
        database
            .mutate_thread(&ThreadMutation::Star {
                thread_id: "default:second-thread".into(),
                value: true,
            })
            .unwrap();
        let provider = ContractProvider {
            fail_mutation: true,
            ..ContractProvider::normal()
        };
        assert!(matches!(
            sync_with(&database, "default", &provider).await,
            Err(ProviderError::RetryableServer(_))
        ));
        assert_eq!(
            database.sync_status("default").unwrap().pending_mutations,
            2,
            "both mutations must be retryable, not just the one actually attempted"
        );
    }

    #[test]
    fn trashing_adds_trash_and_removes_inbox() {
        let mutation = PendingMutation {
            id: "m1".into(),
            provider_thread_id: "gmail-thread".into(),
            target_message_id: None,
            attempts: 1,
            mutation: ThreadMutation::Trash {
                thread_id: "welcome".into(),
                value: true,
            },
        };
        assert_eq!(
            mutation_labels(&mutation),
            (vec!["TRASH".to_string()], vec!["INBOX".to_string()]),
        );
    }

    #[test]
    fn untrashing_restores_inbox_and_removes_trash() {
        let mutation = PendingMutation {
            id: "m1".into(),
            provider_thread_id: "gmail-thread".into(),
            target_message_id: None,
            attempts: 1,
            mutation: ThreadMutation::Trash {
                thread_id: "welcome".into(),
                value: false,
            },
        };
        assert_eq!(
            mutation_labels(&mutation),
            (vec!["INBOX".to_string()], vec!["TRASH".to_string()]),
        );
    }

    #[test]
    fn marking_spam_adds_spam_and_removes_inbox() {
        let mutation = PendingMutation {
            id: "m1".into(),
            provider_thread_id: "gmail-thread".into(),
            target_message_id: None,
            attempts: 1,
            mutation: ThreadMutation::Spam {
                thread_id: "welcome".into(),
                value: true,
            },
        };
        assert_eq!(
            mutation_labels(&mutation),
            (vec!["SPAM".to_string()], vec!["INBOX".to_string()]),
        );
    }

    #[test]
    fn undoing_spam_restores_inbox_and_removes_spam() {
        let mutation = PendingMutation {
            id: "m1".into(),
            provider_thread_id: "gmail-thread".into(),
            target_message_id: None,
            attempts: 1,
            mutation: ThreadMutation::Spam {
                thread_id: "welcome".into(),
                value: false,
            },
        };
        assert_eq!(
            mutation_labels(&mutation),
            (vec!["INBOX".to_string()], vec!["SPAM".to_string()]),
        );
    }

    #[tokio::test]
    async fn spam_delivery_batches_every_message_with_the_gmail_payload() {
        let database = Database::open_memory();
        let mut first = ContractProvider::message();
        first.id = "message-1".into();
        let mut second = ContractProvider::message();
        second.id = "message-2".into();
        database
            .upsert_gmail_thread(
                "default",
                &[
                    crate::mime::normalize(&first).unwrap(),
                    crate::mime::normalize(&second).unwrap(),
                ],
            )
            .unwrap();
        database
            .mutate_thread(&ThreadMutation::Spam {
                thread_id: "default:gmail-thread".into(),
                value: true,
            })
            .unwrap();
        let provider = ContractProvider::normal();

        deliver_mutations(&database, "default", &provider)
            .await
            .unwrap();

        assert_eq!(
            *provider.message_modifies.lock().unwrap(),
            vec![(
                vec!["message-1".to_string(), "message-2".to_string()],
                vec!["SPAM".to_string()],
                vec!["INBOX".to_string()],
            )],
        );
    }

    #[tokio::test]
    async fn metadata_targets_root_and_mark_unread_targets_captured_latest_message() {
        let database = Database::open_memory();
        let mut root = ContractProvider::message();
        root.id = "root-message".into();
        root.internal_date = "1700000000000".into();
        let mut latest = ContractProvider::message();
        latest.id = "latest-message".into();
        latest.internal_date = "1700000001000".into();
        database
            .upsert_gmail_thread(
                "default",
                &[
                    crate::mime::normalize(&latest).unwrap(),
                    crate::mime::normalize(&root).unwrap(),
                ],
            )
            .unwrap();
        database
            .mutate_threads(&[
                ThreadMutation::Star {
                    thread_id: "default:gmail-thread".into(),
                    value: true,
                },
                ThreadMutation::Label {
                    thread_id: "default:gmail-thread".into(),
                    label_id: "Label_project".into(),
                    value: true,
                },
                ThreadMutation::Read {
                    thread_id: "default:gmail-thread".into(),
                    value: false,
                },
            ])
            .unwrap();

        // Delivery is deliberately delayed until a newer message exists. The
        // unread action must still target the message that was latest when the
        // user performed it.
        let mut newer = ContractProvider::message();
        newer.id = "newer-message".into();
        newer.internal_date = "1700000002000".into();
        database
            .upsert_gmail_thread(
                "default",
                &[
                    crate::mime::normalize(&root).unwrap(),
                    crate::mime::normalize(&latest).unwrap(),
                    crate::mime::normalize(&newer).unwrap(),
                ],
            )
            .unwrap();
        let provider = ContractProvider::normal();

        deliver_mutations(&database, "default", &provider)
            .await
            .unwrap();

        assert_eq!(
            *provider.message_modifies.lock().unwrap(),
            vec![
                (
                    vec!["root-message".to_string()],
                    vec!["STARRED".to_string()],
                    vec![],
                ),
                (
                    vec!["root-message".to_string()],
                    vec!["Label_project".to_string()],
                    vec![],
                ),
                (
                    vec!["latest-message".to_string()],
                    vec!["UNREAD".to_string()],
                    vec![],
                ),
            ],
        );
    }

    #[tokio::test]
    async fn mark_read_and_archive_remain_thread_wide() {
        let database = Database::open_memory();
        let message = ContractProvider::message();
        database
            .upsert_gmail_thread("default", &[crate::mime::normalize(&message).unwrap()])
            .unwrap();
        database
            .mutate_threads(&[
                ThreadMutation::Read {
                    thread_id: "default:gmail-thread".into(),
                    value: true,
                },
                ThreadMutation::Archive {
                    thread_id: "default:gmail-thread".into(),
                    value: true,
                },
            ])
            .unwrap();
        let provider = ContractProvider::normal();

        deliver_mutations(&database, "default", &provider)
            .await
            .unwrap();

        assert_eq!(provider.modifies.load(Ordering::SeqCst), 2);
        assert!(provider.message_modifies.lock().unwrap().is_empty());
    }

    #[test]
    fn resume_catch_up_skips_inside_the_poll_interval() {
        let start = Instant::now();
        assert!(should_skip_stale_sync(
            Some(start),
            start + Duration::from_secs(1),
            MIN_POLL_INTERVAL,
        ));
        assert!(!should_skip_stale_sync(
            Some(start),
            start + MIN_POLL_INTERVAL,
            MIN_POLL_INTERVAL,
        ));
        assert!(!should_skip_stale_sync(None, start, MIN_POLL_INTERVAL));
    }

    #[tokio::test]
    async fn inactivity_flush_skips_provider_when_nothing_is_pending() {
        let database = Database::open_memory();
        let provider = ContractProvider::normal();
        flush_pending_with(&database, "default", &provider)
            .await
            .unwrap();
        assert_eq!(provider.modifies.load(Ordering::SeqCst), 0);
        assert_eq!(provider.full_lists.load(Ordering::SeqCst), 0);
    }

    struct ReconcileProvider {
        inbox_ids: Vec<String>,
        threads: std::collections::HashMap<String, GmailMessage>,
        list_calls: AtomicUsize,
        fail_thread_once: StdMutex<Option<String>>,
    }

    #[async_trait]
    impl GmailProvider for ReconcileProvider {
        async fn profile_history_id(&self) -> ProviderResult<String> {
            Ok("resynced".into())
        }

        async fn list_threads(&self, page: Option<&str>) -> ProviderResult<ThreadPage> {
            assert!(page.is_none());
            self.list_calls.fetch_add(1, Ordering::SeqCst);
            Ok(ThreadPage {
                thread_ids: self.inbox_ids.clone(),
                next_page_token: None,
            })
        }

        async fn get_thread(&self, id: &str) -> ProviderResult<Vec<GmailMessage>> {
            let mut fail_thread = self.fail_thread_once.lock().unwrap();
            if fail_thread.as_deref() == Some(id) {
                fail_thread.take();
                return Err(ProviderError::RetryableServer("rate limited".into()));
            }
            drop(fail_thread);
            Ok(vec![self
                .threads
                .get(id)
                .unwrap_or_else(|| panic!("unexpected thread id: {id}"))
                .clone()])
        }

        async fn history(&self, _cursor: &str, page: Option<&str>) -> ProviderResult<HistoryPage> {
            assert!(page.is_none());
            Ok(HistoryPage {
                thread_ids: vec![],
                history_id: "resynced".into(),
                next_page_token: None,
            })
        }

        async fn modify_thread(
            &self,
            _id: &str,
            _add: &[String],
            _remove: &[String],
        ) -> ProviderResult<()> {
            // Delivering the setup mutation that produced this provider's
            // "already archived locally" fixture state (see
            // `full_sync_preserves_already_archived_threads`) — treat it as
            // a successful, already-applied Gmail-side change.
            Ok(())
        }

        async fn modify_messages(
            &self,
            _ids: &[String],
            _add: &[String],
            _remove: &[String],
        ) -> ProviderResult<()> {
            unreachable!()
        }

        async fn list_labels(&self) -> ProviderResult<Vec<Label>> {
            unreachable!()
        }

        async fn create_label(&self, _name: &str) -> ProviderResult<Label> {
            unreachable!()
        }

        async fn update_label(&self, _id: &str, _name: &str) -> ProviderResult<Label> {
            unreachable!()
        }

        async fn delete_label(&self, _id: &str) -> ProviderResult<()> {
            unreachable!()
        }
    }

    #[tokio::test]
    async fn reconcile_inbox_corrects_drift_without_touching_undrifted_threads() {
        // A distinct account id so the demo threads `Database::open_memory`
        // seeds under "default" don't factor into the inbox diff.
        let account = "acct";
        let database = Database::open_memory();

        // Locally archived, but Gmail's live INBOX listing says it's back —
        // the exact scenario a missed/delayed label change produces.
        let mut restored = ContractProvider::message();
        restored.id = "restored-message".into();
        restored.thread_id = "restored-thread".into();
        database
            .upsert_gmail_thread(account, &[crate::mime::normalize(&restored).unwrap()])
            .unwrap();
        database
            .mutate_thread(&ThreadMutation::Archive {
                thread_id: format!("{account}:restored-thread"),
                value: true,
            })
            .unwrap();

        // Locally still shown as inbox, but Gmail no longer lists it there.
        let mut orphaned = ContractProvider::message();
        orphaned.id = "orphaned-message".into();
        orphaned.thread_id = "orphaned-thread".into();
        database
            .upsert_gmail_thread(account, &[crate::mime::normalize(&orphaned).unwrap()])
            .unwrap();

        // Untouched by drift: stays archived, and reconcile must never fetch it.
        let mut settled = ContractProvider::message();
        settled.id = "settled-message".into();
        settled.thread_id = "settled-thread".into();
        settled.label_ids = vec![];
        database
            .upsert_gmail_thread(account, &[crate::mime::normalize(&settled).unwrap()])
            .unwrap();
        database
            .mutate_thread(&ThreadMutation::Archive {
                thread_id: format!("{account}:settled-thread"),
                value: true,
            })
            .unwrap();

        assert!(
            database
                .list_all_mail(Some(account))
                .unwrap()
                .iter()
                .find(|thread| thread.id == format!("{account}:restored-thread"))
                .unwrap()
                .archived
        );

        let mut server_orphaned = ContractProvider::message();
        server_orphaned.id = "orphaned-message".into();
        server_orphaned.thread_id = "orphaned-thread".into();
        server_orphaned.label_ids = vec![];

        let provider = ReconcileProvider {
            inbox_ids: vec!["restored-thread".into()],
            threads: [
                ("restored-thread".to_string(), restored),
                ("orphaned-thread".to_string(), server_orphaned),
            ]
            .into_iter()
            .collect(),
            list_calls: AtomicUsize::new(0),
            fail_thread_once: StdMutex::new(None),
        };

        reconcile_inbox(&database, account, &provider)
            .await
            .unwrap();

        let threads = database.list_all_mail(Some(account)).unwrap();
        let restored = threads
            .iter()
            .find(|thread| thread.id == format!("{account}:restored-thread"))
            .unwrap();
        assert!(
            !restored.archived,
            "drifted-back thread must be un-archived"
        );

        let orphaned = threads
            .iter()
            .find(|thread| thread.id == format!("{account}:orphaned-thread"))
            .unwrap();
        assert!(orphaned.archived, "drifted-away thread must be archived");
    }

    #[tokio::test]
    async fn failed_reconciliation_remains_due_until_a_successful_retry() {
        let account = "acct-reconcile-retry";
        let database = Database::open_memory();
        database
            .connection()
            .unwrap()
            .execute("INSERT INTO sync_state(account_id) VALUES (?1)", [account])
            .unwrap();

        let mut message = ContractProvider::message();
        message.id = "retry-message".into();
        message.thread_id = "retry-thread".into();
        let provider = ReconcileProvider {
            inbox_ids: vec!["retry-thread".into()],
            threads: [("retry-thread".to_string(), message)]
                .into_iter()
                .collect(),
            list_calls: AtomicUsize::new(0),
            fail_thread_once: StdMutex::new(Some("retry-thread".into())),
        };

        assert!(reconcile_and_mark(&database, account, &provider)
            .await
            .is_err());
        assert!(database.reconciliation_due(account, 1).unwrap());

        reconcile_and_mark(&database, account, &provider)
            .await
            .unwrap();
        assert!(!database.reconciliation_due(account, 1).unwrap());
        assert_eq!(provider.list_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn full_sync_preserves_already_archived_threads() {
        // A fresh account (no cursor yet), so `sync_with` routes through
        // `full_sync` — the regression this guards against previously wiped
        // every cached thread here before relisting only the server's
        // current INBOX, permanently losing anything the user had archived.
        let account = "acct-full-sync";
        let database = Database::open_memory();
        // A plain new-account sync_state row, deliberately not going through
        // `adopt_account` — that call migrates `Database::open_memory()`'s
        // local-only demo thread onto the first adopted account, which would
        // otherwise leak into this test's fixture.
        database
            .connection()
            .unwrap()
            .execute("INSERT INTO sync_state(account_id) VALUES (?1)", [account])
            .unwrap();

        let mut archived = ContractProvider::message();
        archived.id = "archived-message".into();
        archived.thread_id = "archived-thread".into();
        database
            .upsert_gmail_thread(account, &[crate::mime::normalize(&archived).unwrap()])
            .unwrap();
        database
            .mutate_thread(&ThreadMutation::Archive {
                thread_id: format!("{account}:archived-thread"),
                value: true,
            })
            .unwrap();

        // This thread exists in both inbox snapshots, but its cached contents
        // predate the expired cursor. Recovery must refresh it even though its
        // inbox membership did not drift.
        let mut stale_common = ContractProvider::message();
        stale_common.id = "common-message".into();
        stale_common.thread_id = "common-thread".into();
        stale_common.snippet = "stale snippet".into();
        database
            .upsert_gmail_thread(account, &[crate::mime::normalize(&stale_common).unwrap()])
            .unwrap();
        let mut fresh_common = stale_common;
        fresh_common.snippet = "fresh snippet".into();

        let mut inbox_message = ContractProvider::message();
        inbox_message.id = "inbox-message".into();
        inbox_message.thread_id = "inbox-thread".into();
        let provider = ReconcileProvider {
            inbox_ids: vec!["common-thread".into(), "inbox-thread".into()],
            threads: [
                ("common-thread".to_string(), fresh_common),
                ("inbox-thread".to_string(), inbox_message),
            ]
            .into_iter()
            .collect(),
            list_calls: AtomicUsize::new(0),
            fail_thread_once: StdMutex::new(None),
        };

        sync_with(&database, account, &provider).await.unwrap();

        let threads = database.list_all_mail(Some(account)).unwrap();
        let archived_thread = threads
            .iter()
            .find(|thread| thread.id == format!("{account}:archived-thread"))
            .expect("a full resync must never delete an already-archived thread");
        assert!(archived_thread.archived);
        assert!(
            threads
                .iter()
                .any(|thread| thread.id == format!("{account}:inbox-thread")),
            "the server's current inbox listing must still be ingested"
        );
        assert_eq!(
            threads
                .iter()
                .find(|thread| thread.id == format!("{account}:common-thread"))
                .unwrap()
                .snippet,
            "fresh snippet",
            "cursor recovery must refresh threads common to both inbox snapshots"
        );
    }

    #[tokio::test]
    async fn interrupted_full_sync_resumes_its_durable_recovery_generation() {
        let account = "acct-resume";
        let database = Database::open_memory();
        database
            .connection()
            .unwrap()
            .execute("INSERT INTO sync_state(account_id) VALUES (?1)", [account])
            .unwrap();

        let mut message = ContractProvider::message();
        message.id = "resume-message".into();
        message.thread_id = "resume-thread".into();
        let provider = ReconcileProvider {
            inbox_ids: vec!["resume-thread".into()],
            threads: [("resume-thread".to_string(), message)]
                .into_iter()
                .collect(),
            list_calls: AtomicUsize::new(0),
            fail_thread_once: StdMutex::new(Some("resume-thread".into())),
        };

        assert!(matches!(
            sync_with(&database, account, &provider).await,
            Err(ProviderError::RetryableServer(_))
        ));
        assert_eq!(provider.list_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            database.recovery_cursor(account).unwrap().as_deref(),
            Some("resynced")
        );

        sync_with(&database, account, &provider).await.unwrap();
        assert_eq!(
            provider.list_calls.load(Ordering::SeqCst),
            1,
            "recovery should resume its saved generation rather than relisting"
        );
        assert!(database.recovery_cursor(account).unwrap().is_none());
        assert_eq!(
            database.cursor(account).unwrap().as_deref(),
            Some("resynced")
        );
    }

    #[tokio::test]
    async fn inactivity_flush_delivers_pending_mutations_without_listing() {
        let database = Database::open_memory();
        database
            .mutate_thread(&ThreadMutation::Archive {
                thread_id: "welcome".into(),
                value: true,
            })
            .unwrap();
        let provider = ContractProvider::normal();
        flush_pending_with(&database, "default", &provider)
            .await
            .unwrap();
        assert_eq!(provider.modifies.load(Ordering::SeqCst), 1);
        assert_eq!(provider.full_lists.load(Ordering::SeqCst), 0);
        assert_eq!(
            database.sync_status("default").unwrap().pending_mutations,
            0
        );
    }
}
