use std::{
    collections::HashSet,
    sync::{Arc, Mutex as StdMutex},
    time::{Duration, Instant},
};

use tokio::sync::Mutex;

use crate::{
    auth::GoogleAuth,
    db::{Database, PendingMutation},
    gmail::{GmailClient, GmailProvider, ProviderError, ProviderResult},
    mime::{normalize, NormalizedMessage},
    models::{Label, SyncStatus, ThreadMutation},
};

/// Floor used by adaptive polling and by resume/foreground catch-up.
pub const MIN_POLL_INTERVAL: Duration = Duration::from_secs(15);
const MAX_POLL_INTERVAL: Duration = Duration::from_secs(300);

/// How often the background poll loop re-derives inbox membership from
/// Gmail's live INBOX listing, independent of history-based incremental
/// sync. History sync can miss a label change reaching us — e.g. Gmail-side
/// propagation lag on a change made outside Dispatch — and nothing else
/// self-heals that short of a historyId 404. This is a safety net, not the
/// primary sync path, so it runs rarely.
const RECONCILE_INTERVAL_SECS: i64 = 6 * 60 * 60;

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
            tokio::time::sleep(delay).await;
            if !self.auth.available() {
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
    /// Failures are swallowed (and simply retried after the next interval)
    /// since this runs alongside the primary sync path, which already
    /// surfaces its own errors.
    async fn reconcile_if_due(&self) {
        let account_id = self.account_id();
        let due = self
            .database
            .reconciliation_due(&account_id, RECONCILE_INTERVAL_SECS)
            .unwrap_or(false);
        if !due {
            return;
        }
        let _guard = self.gate.lock().await;
        let provider = GmailClient::new(self.auth.clone());
        let _ = reconcile_inbox(self.database.as_ref(), &account_id, &provider).await;
        let _ = self.database.mark_reconciled(&account_id);
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
        Err(ProviderError::RateLimited) => MAX_POLL_INTERVAL,
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
        .sync_status(account_id)
        .map_err(ProviderError::Other)?
        .pending_mutations
        == 0
    {
        return Ok(());
    }
    deliver_mutations(database, account_id, provider).await
}

pub async fn sync_with(
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

async fn full_sync(
    database: &Database,
    account_id: &str,
    provider: &(impl GmailProvider + ?Sized),
) -> ProviderResult<()> {
    match full_sync_attempt(database, account_id, provider).await {
        Err(ProviderError::InvalidCursor) => {
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
    // Capture the cursor before listing. The following history pass closes the race
    // with mail arriving while the potentially long initial list is downloaded.
    let starting_cursor = provider.profile_history_id().await?;
    database
        .begin_full_sync(account_id)
        .map_err(ProviderError::Other)?;
    let mut page = None;
    loop {
        let result = provider.list_threads(page.as_deref()).await?;
        ingest_threads(database, account_id, provider, result.thread_ids).await?;
        page = result.next_page_token;
        if page.is_none() {
            break;
        }
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

/// Diffs Gmail's current INBOX thread listing against what's cached
/// locally and re-ingests only the threads that drifted, correcting their
/// labels/archived state. Unlike [`full_sync`], this never wipes the local
/// cache, so it's safe to run on a timer without disturbing Archive/All
/// Mail/Trash views (which `full_sync`'s INBOX-only relist would otherwise
/// leave empty until independently touched again).
async fn reconcile_inbox(
    database: &Database,
    account_id: &str,
    provider: &(impl GmailProvider + ?Sized),
) -> ProviderResult<()> {
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
    let local_inbox_ids: HashSet<String> = database
        .local_inbox_provider_thread_ids(account_id)
        .map_err(ProviderError::Other)?
        .into_iter()
        .collect();
    let drifted: Vec<String> = server_inbox_ids
        .symmetric_difference(&local_inbox_ids)
        .cloned()
        .collect();
    if drifted.is_empty() {
        return Ok(());
    }
    ingest_threads(database, account_id, provider, drifted).await
}

async fn ingest_threads(
    database: &Database,
    account_id: &str,
    provider: &(impl GmailProvider + ?Sized),
    ids: Vec<String>,
) -> ProviderResult<()> {
    let mut normalized_threads: Vec<Vec<NormalizedMessage>> = Vec::with_capacity(ids.len());
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
        normalized_threads.push(
            messages
                .iter()
                .map(normalize)
                .collect::<Result<Vec<_>, _>>()
                .map_err(ProviderError::Other)?,
        );
    }
    database
        .upsert_gmail_threads(account_id, &normalized_threads)
        .map_err(ProviderError::Other)?;
    for id in deleted {
        database
            .delete_gmail_thread(account_id, &id)
            .map_err(ProviderError::Other)?;
    }
    Ok(())
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
                                .modify_messages(
                                    std::slice::from_ref(message_id),
                                    &add,
                                    &remove,
                                )
                                .await
                        }
                        None => Err(ProviderError::Other(
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
                Err(error @ ProviderError::RateLimited) => {
                    for item in &mutations[index..batch_end] {
                        database
                            .reject_mutation(&item.id, &error.to_string(), true)
                            .map_err(ProviderError::Other)?;
                    }
                    return Err(error);
                }
                Err(error) => {
                    for item in &mutations[index..batch_end] {
                        database
                            .reject_mutation(&item.id, &error.to_string(), false)
                            .map_err(ProviderError::Other)?;
                    }
                }
            }
            index = batch_end;
        }
    }
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
    }

    impl ContractProvider {
        fn normal() -> Self {
            Self {
                invalidate_stale_cursor: AtomicBool::new(false),
                full_lists: AtomicUsize::new(0),
                modifies: AtomicUsize::new(0),
                message_modifies: StdMutex::new(vec![]),
                fail_mutation: false,
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
            assert_eq!(id, "gmail-thread");
            Ok(vec![Self::message()])
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
            if self.fail_mutation {
                Err(ProviderError::RateLimited)
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
            if self.fail_mutation {
                Err(ProviderError::RateLimited)
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
            next_poll_delay(MIN_POLL_INTERVAL, 0, Err(ProviderError::RateLimited),),
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
            Err(ProviderError::RateLimited)
        ));
        assert_eq!(
            database.sync_status("default").unwrap().pending_mutations,
            1
        );
    }

    #[test]
    fn trashing_adds_trash_and_removes_inbox() {
        let mutation = PendingMutation {
            id: "m1".into(),
            provider_thread_id: "gmail-thread".into(),
            target_message_id: None,
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
    }

    #[async_trait]
    impl GmailProvider for ReconcileProvider {
        async fn profile_history_id(&self) -> ProviderResult<String> {
            unreachable!()
        }

        async fn list_threads(&self, page: Option<&str>) -> ProviderResult<ThreadPage> {
            assert!(page.is_none());
            Ok(ThreadPage {
                thread_ids: self.inbox_ids.clone(),
                next_page_token: None,
            })
        }

        async fn get_thread(&self, id: &str) -> ProviderResult<Vec<GmailMessage>> {
            Ok(vec![self
                .threads
                .get(id)
                .unwrap_or_else(|| panic!("unexpected thread id: {id}"))
                .clone()])
        }

        async fn history(&self, _cursor: &str, _page: Option<&str>) -> ProviderResult<HistoryPage> {
            unreachable!()
        }

        async fn modify_thread(
            &self,
            _id: &str,
            _add: &[String],
            _remove: &[String],
        ) -> ProviderResult<()> {
            unreachable!()
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
        };

        reconcile_inbox(&database, account, &provider).await.unwrap();

        let threads = database.list_all_mail(Some(account)).unwrap();
        let restored = threads
            .iter()
            .find(|thread| thread.id == format!("{account}:restored-thread"))
            .unwrap();
        assert!(!restored.archived, "drifted-back thread must be un-archived");

        let orphaned = threads
            .iter()
            .find(|thread| thread.id == format!("{account}:orphaned-thread"))
            .unwrap();
        assert!(orphaned.archived, "drifted-away thread must be archived");
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
