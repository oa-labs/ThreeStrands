use std::{
    collections::{HashMap, HashSet},
    future::Future,
    sync::{Arc, Mutex as StdMutex},
    time::{Duration, Instant},
};

use rand::RngExt;
use tokio::sync::{watch, Mutex, Notify};

use crate::{
    auth::AccountAuth,
    backoff::retry_at,
    db::{Database, DatabaseError, PendingMutation, SentBackfillProgress},
    mime::{
        normalize, normalized_size, NormalizedMessage, MAX_NORMALIZED_THREAD_BYTES,
        MAX_THREAD_MESSAGES,
    },
    models::{Label, SyncStatus, ThreadMutation},
    provider::{MailMutate, MailSync, ProviderError, ProviderResult, SyncCursor},
};

/// Floor used by adaptive polling and by resume/foreground catch-up.
pub const MIN_POLL_INTERVAL: Duration = Duration::from_secs(15);
const MAX_POLL_INTERVAL: Duration = Duration::from_secs(300);
/// Retry delay while an account's credentials are unavailable.
const AUTH_UNAVAILABLE_RETRY_INTERVAL: Duration = Duration::from_secs(30);
/// Backoff after a non-retryable poll failure (retryable server errors back
/// off to [`MAX_POLL_INTERVAL`]).
const POLL_FAILURE_RETRY_INTERVAL: Duration = Duration::from_secs(60);

/// How often the background poll loop re-derives inbox membership from
/// Gmail's live INBOX listing, independent of history-based incremental
/// sync. History sync can miss a label change reaching us — e.g. Gmail-side
/// propagation lag on a change made outside ThreeStrands — and nothing else
/// self-heals that short of a historyId 404. This is a safety net, not the
/// primary sync path, so it runs rarely.
const RECONCILE_INTERVAL_SECS: i64 = 6 * 60 * 60;
const RECOVERY_BATCH_SIZE: usize = 50;
/// Upper bound on change pages in one incremental round. Change detection
/// holds the account's sync gate and persists nothing until its final page,
/// so a provider that never stops paging would otherwise stall the account
/// (and every manual refresh queued behind it) indefinitely. Far above any
/// real backlog; exceeding it falls back to a full resynchronization.
const MAX_INCREMENTAL_SYNC_PAGES: usize = 1_000;
// Fetch enough of Gmail's ranked result set to fill the first local page,
// while keeping a broad query from blocking the UI on hundreds of sequential
// `threads.get` calls (each one is deliberately paced for Gmail quota).
const REMOTE_SEARCH_SCAN_LIMIT: usize = 50;
/// Sent mail the address book learns from. The initial sync lists only the
/// inbox, so without this, people you wrote to in threads that were already
/// archived never become contacts. Trash and spam are left out.
const SENT_BACKFILL_QUERY: &str = "in:sent -in:trash -in:spam";
/// Newest sent threads scanned before the backfill stops for good.
pub(crate) const MAX_SENT_BACKFILL_THREADS: usize = 5_000;
/// Thread fetches per backfill step. Each step holds the account's sync gate,
/// so it stays short enough not to stall a manual refresh queued behind it.
const SENT_BACKFILL_FETCHES_PER_STEP: usize = 25;
/// Backfill steps per poll; the gate is released between steps.
const SENT_BACKFILL_STEPS_PER_POLL: usize = 4;

fn database_provider_error(error: DatabaseError) -> ProviderError {
    ProviderError::Other(error.to_string())
}

type SyncActivityListener = Arc<dyn Fn(&str, bool) + Send + Sync>;

/// Which accounts are checking their provider for mail right now, shared by
/// every account's [`SyncService`]. The listener hears each account's first
/// sync starting and its last one finishing, so the UI can show that mail is
/// being checked whether the polling loop or a manual refresh started it.
#[derive(Clone, Default)]
pub struct SyncActivity {
    active: Arc<StdMutex<HashMap<String, usize>>>,
    listener: Option<SyncActivityListener>,
}

impl SyncActivity {
    pub fn with_listener(listener: impl Fn(&str, bool) + Send + Sync + 'static) -> Self {
        Self {
            active: Arc::default(),
            listener: Some(Arc::new(listener)),
        }
    }

    /// Accounts with a sync in progress, sorted, for a UI that mounts after
    /// a sync already announced its start.
    pub fn active_accounts(&self) -> Vec<String> {
        let mut accounts = self
            .active
            .lock()
            .map(|active| active.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        accounts.sort();
        accounts
    }

    /// Marks `account_id` as syncing until the returned guard drops, which
    /// also covers a run that account retirement cancels mid-flight.
    fn begin(&self, account_id: &str) -> SyncActivityGuard {
        let started = self.active.lock().is_ok_and(|mut active| {
            let runs = active.entry(account_id.to_string()).or_default();
            *runs += 1;
            *runs == 1
        });
        if started {
            self.notify(account_id, true);
        }
        SyncActivityGuard {
            activity: self.clone(),
            account_id: account_id.to_string(),
        }
    }

    fn end(&self, account_id: &str) {
        let finished = self.active.lock().is_ok_and(|mut active| {
            let Some(runs) = active.get_mut(account_id) else {
                return false;
            };
            *runs -= 1;
            if *runs > 0 {
                return false;
            }
            active.remove(account_id);
            true
        });
        if finished {
            self.notify(account_id, false);
        }
    }

    fn notify(&self, account_id: &str, active: bool) {
        if let Some(listener) = &self.listener {
            listener(account_id, active);
        }
    }
}

struct SyncActivityGuard {
    activity: SyncActivity,
    account_id: String,
}

impl Drop for SyncActivityGuard {
    fn drop(&mut self) {
        self.activity.end(&self.account_id);
    }
}

#[derive(Clone)]
pub struct SyncService {
    database: Arc<Database>,
    auth: AccountAuth,
    gate: Arc<Mutex<()>>,
    /// Set while the account is being removed; see [`Self::retire`]. Shared
    /// by every clone, so it reaches runs started outside the polling loop.
    retired: Arc<watch::Sender<bool>>,
    last_attempt: Arc<StdMutex<Option<Instant>>>,
    activity: SyncActivity,
    /// Signalled when the user returns to the app; see
    /// [`Self::reset_poll_backoff`].
    wake: Arc<Notify>,
}

/// Returned by a sync attempt on an account that is being removed.
pub(crate) const ACCOUNT_REMOVED: &str = "This account was removed";

pub fn should_skip_stale_sync(
    last_attempt: Option<Instant>,
    now: Instant,
    min_age: Duration,
) -> bool {
    last_attempt.is_some_and(|attempt| now.saturating_duration_since(attempt) < min_age)
}

impl SyncService {
    pub fn new(database: Arc<Database>, auth: AccountAuth) -> Self {
        Self {
            database,
            auth,
            gate: Arc::new(Mutex::new(())),
            retired: Arc::new(watch::Sender::new(false)),
            last_attempt: Arc::new(StdMutex::new(None)),
            activity: SyncActivity::default(),
            wake: Arc::new(Notify::new()),
        }
    }

    /// Reports this account's syncs to the app-wide `activity` registry.
    pub fn with_activity(mut self, activity: SyncActivity) -> Self {
        self.activity = activity;
        self
    }

    /// Brings an idle account's polling back to [`MIN_POLL_INTERVAL`] when
    /// the user returns, instead of leaving the next check up to
    /// [`MAX_POLL_INTERVAL`] away. A signal sent while a poll is running is
    /// kept for the next wait.
    pub fn reset_poll_backoff(&self) {
        self.wake.notify_one();
    }

    /// Runs `work` holding the account's sync gate, unless the account is
    /// retired: then it returns `None` without starting, or drops `work` at
    /// its next await if retirement arrives mid-run. Every database write is
    /// its own synchronous transaction, so a dropped run leaves nothing
    /// half-applied. All work that writes the account's mail goes through
    /// here.
    async fn exclusive<T>(&self, work: impl Future<Output = T>) -> Option<T> {
        let mut retired = self.retired.subscribe();
        tokio::select! {
            _ = retired.wait_for(|retired| *retired) => None,
            result = async {
                let _guard = self.gate.lock().await;
                if *self.retired.borrow() {
                    return None;
                }
                Some(work.await)
            } => result,
        }
    }

    /// Stops sync work for this account across every clone of the service,
    /// and returns once no run is in progress. Account removal calls this
    /// before purging local data, so a run that was waiting on the provider
    /// cannot write the account's threads or cursor back afterwards.
    pub(crate) async fn retire(&self) {
        self.retired.send_replace(true);
        let _guard = self.gate.lock().await;
    }

    /// Undoes [`Self::retire`] when removal fails and the account stays.
    pub(crate) fn resume(&self) {
        self.retired.send_replace(false);
    }

    /// Whether this service's account currently has usable Google credentials.
    pub fn is_connected(&self) -> bool {
        self.auth.available()
            && !self
                .database
                .account_needs_reauth(&self.account_id())
                .unwrap_or(false)
    }

    /// The local key for this account's cursor/threads/mutations rows. Reads
    /// through to `auth`'s live keychain key, so it stays correct across a
    /// rekey (e.g. the primary account resolving its real address after
    /// startup) without this service needing to be reconstructed.
    pub(crate) fn account_id(&self) -> String {
        self.auth.key()
    }

    pub async fn sync(&self) -> Result<SyncStatus, String> {
        self.sync_provider()
            .await
            .map_err(|error| error.to_string())
    }

    async fn sync_provider(&self) -> ProviderResult<SyncStatus> {
        self.sync_provider_reporting_changes()
            .await
            .map(|(status, _)| status)
    }

    /// Like [`Self::sync_provider`], also reporting whether the sync changed
    /// any local mail, so the polling loop can skip notifying the UI after a
    /// poll that found nothing new.
    async fn sync_provider_reporting_changes(&self) -> ProviderResult<(SyncStatus, bool)> {
        self.exclusive(self.sync_provider_reporting_changes_locked())
            .await
            .unwrap_or_else(|| Err(ProviderError::Other(ACCOUNT_REMOVED.into())))
    }

    async fn sync_provider_reporting_changes_locked(&self) -> ProviderResult<(SyncStatus, bool)> {
        let account_id = self.account_id();
        let provider = self.auth.provider(&self.database)?;
        let _activity = self.activity.begin(&account_id);
        let result = sync_with(self.database.as_ref(), &account_id, provider.as_ref()).await;
        if let Ok(mut last_attempt) = self.last_attempt.lock() {
            *last_attempt = Some(Instant::now());
        }
        let changed = match result {
            Ok(changed) => changed,
            Err(error) => {
                let message = error.to_string();
                self.database
                    .fail_sync(&account_id, &message)
                    .map_err(database_provider_error)?;
                return Err(error);
            }
        };
        let status = self
            .database
            .sync_status(&account_id)
            .map_err(database_provider_error)?;
        Ok((status, changed))
    }

    /// Incremental catch-up for OS resume / window focus. Skips if polling
    /// or another catch-up already ran within [`MIN_POLL_INTERVAL`].
    pub async fn sync_if_stale(&self) -> Result<SyncStatus, String> {
        let last_attempt = self.last_attempt.lock().ok().and_then(|guard| *guard);
        if should_skip_stale_sync(last_attempt, Instant::now(), MIN_POLL_INTERVAL) {
            return Ok(self.database.sync_status(&self.account_id())?);
        }
        self.sync().await
    }

    pub async fn flush_pending(&self) -> Result<SyncStatus, String> {
        let account_id = self.account_id();
        if self.database.sync_status(&account_id)?.pending_mutations == 0 {
            return Ok(self.database.sync_status(&account_id)?);
        }
        if !self.auth.available() {
            return Ok(self.database.sync_status(&account_id)?);
        }
        let flushed = self
            .exclusive(async {
                let provider = self.auth.provider(&self.database)?;
                flush_pending_with(self.database.as_ref(), &account_id, provider.as_ref()).await
            })
            .await
            .ok_or_else(|| ACCOUNT_REMOVED.to_string())?;
        if let Err(error) = flushed {
            let message = error.to_string();
            self.database.fail_sync(&account_id, &message)?;
            return Err(message);
        }
        Ok(self.database.sync_status(&account_id)?)
    }

    /// Imports server-side search hits absent from the local cache. Existing
    /// local threads are deliberately skipped so an archived search does not
    /// turn into a costly refresh of every ordinary inbox match.
    pub async fn backfill_search(&self, query: &str) -> Result<(), String> {
        if query.trim().is_empty() || !self.is_connected() {
            return Ok(());
        }
        self.exclusive(async {
            let account_id = self.account_id();
            let provider = self
                .auth
                .provider(&self.database)
                .map_err(|error| error.to_string())?;
            // Local search still works without this; a provider that cannot
            // look past the local index simply has nothing to contribute, so
            // asking is a guaranteed round trip to an error.
            if !provider.capabilities().server_search {
                return Ok(());
            }
            search_and_ingest_missing(
                self.database.as_ref(),
                &account_id,
                provider.as_ref(),
                query,
                REMOTE_SEARCH_SCAN_LIMIT,
            )
            .await
            .map_err(|error| error.to_string())
        })
        .await
        .unwrap_or(Ok(()))
    }

    pub async fn create_label(&self, name: &str) -> Result<Label, String> {
        validate_label_name(name)?;
        self.auth
            .provider(&self.database)
            .map_err(|error| error.to_string())?
            .create_label(name)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn update_label(&self, id: &str, name: &str) -> Result<Label, String> {
        validate_label_name(name)?;
        self.auth
            .provider(&self.database)
            .map_err(|error| error.to_string())?
            .update_label(id, name)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn delete_label(&self, id: &str) -> Result<(), String> {
        self.auth
            .provider(&self.database)
            .map_err(|error| error.to_string())?
            .delete_label(id)
            .await
            .map_err(|error| error.to_string())
    }

    /// Syncs once right away when `sync_immediately`, then polls the
    /// provider forever with adaptive backoff. After any sync that changed what the
    /// UI shows, including the startup one, calls `on_synced` with the
    /// account's id, so callers (e.g. the frontend's thread list and
    /// per-account unread badges) can refresh state that this account just
    /// changed without waiting for the user to switch to it. A poll that
    /// found nothing new, which is most of them, stays silent so the UI
    /// doesn't reload its lists and counts every interval.
    pub async fn polling_loop(
        self,
        sync_immediately: bool,
        on_synced: impl Fn(&str) + Send + Sync + 'static,
    ) {
        if sync_immediately && self.is_connected() {
            // The first poll after startup stays at the polling floor
            // whatever the result, since the user has just opened the app.
            self.poll_once(MIN_POLL_INTERVAL, &on_synced).await;
        }
        let mut delay = MIN_POLL_INTERVAL;
        loop {
            // Jitter avoids multiple accounts/instances recovering from the
            // same outage and retrying in lockstep; `next_poll_delay` itself
            // stays deterministic so its unit tests aren't flaky.
            let jitter = Duration::from_millis(rand::rng().random_range(0..250));
            delay = wait_for_next_poll(&self.wake, delay, jitter).await;
            if !self.auth.available() {
                delay = AUTH_UNAVAILABLE_RETRY_INTERVAL;
                continue;
            }
            delay = self.poll_once(delay, &on_synced).await;
            self.reconcile_if_due().await;
            self.backfill_sent_if_pending().await;
        }
    }

    /// One sync, notifying the UI if it changed anything; returns the delay
    /// before the next poll.
    async fn poll_once(&self, delay: Duration, on_synced: &impl Fn(&str)) -> Duration {
        let before = self
            .database
            .sync_status(&self.account_id())
            .map(|status| status.pending_mutations)
            .unwrap_or_default();
        let result = self.sync_provider_reporting_changes().await;
        if should_notify_after_poll(before, &result) {
            on_synced(&self.account_id());
        }
        next_poll_delay(delay, before, result.map(|(status, _)| status))
    }

    /// Advances the one-time sent-mail backfill by a few bounded steps once
    /// the account's initial sync has finished. Failures are swallowed and
    /// retried on a later poll; progress is saved after every step.
    async fn backfill_sent_if_pending(&self) {
        for _ in 0..SENT_BACKFILL_STEPS_PER_POLL {
            let account_id = self.account_id();
            let ready = !self.database.account_needs_reauth(&account_id).unwrap_or(true)
                && self.database.cursor(&account_id).ok().flatten().is_some()
                && self.database.recovery_cursor(&account_id).ok().flatten().is_none();
            if !ready {
                return;
            }
            let more = self
                .exclusive(async {
                    let Ok(provider) = self.auth.provider(&self.database) else {
                        return false;
                    };
                    if !provider.capabilities().server_search {
                        return false;
                    }
                    backfill_sent_step(self.database.as_ref(), &account_id, provider.as_ref())
                        .await
                        .unwrap_or(false)
                })
                .await
                .unwrap_or(false);
            if !more {
                return;
            }
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
        self.exclusive(async {
            let Ok(provider) = self.auth.provider(&self.database) else {
                return;
            };
            let _ = reconcile_and_mark(self.database.as_ref(), &account_id, provider.as_ref()).await;
        })
        .await;
    }
}

/// A poll changes what the UI shows when it ingested or removed mail, or
/// when it settled queued local mutations (whose pending state the UI
/// displays and which can fail and roll back).
fn should_notify_after_poll(
    pending_before: i64,
    result: &ProviderResult<(SyncStatus, bool)>,
) -> bool {
    match result {
        Ok((status, changed)) => *changed || status.pending_mutations != pending_before,
        Err(_) => false,
    }
}

/// Sleeps out `delay` (plus `jitter`) before the next poll. A wake signal
/// means the user is back: the delay drops to [`MIN_POLL_INTERVAL`] and the
/// poll comes no later than that floor from the signal. Returns the delay
/// the next backoff step grows from.
async fn wait_for_next_poll(wake: &Notify, mut delay: Duration, jitter: Duration) -> Duration {
    let mut deadline = tokio::time::Instant::now() + delay + jitter;
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => return delay,
            _ = wake.notified() => {
                delay = MIN_POLL_INTERVAL;
                deadline = woken_poll_deadline(deadline, tokio::time::Instant::now(), jitter);
            }
        }
    }
}

/// A wake signal only ever brings the next poll closer.
fn woken_poll_deadline(
    deadline: tokio::time::Instant,
    now: tokio::time::Instant,
    jitter: Duration,
) -> tokio::time::Instant {
    deadline.min(now + MIN_POLL_INTERVAL + jitter)
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
        Err(_) => POLL_FAILURE_RETRY_INTERVAL,
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
    provider: &(impl MailMutate + ?Sized),
) -> ProviderResult<()> {
    if database
        .account_needs_reauth(account_id)
        .map_err(database_provider_error)?
    {
        return Ok(());
    }
    if database
        .sync_status(account_id)
        .map_err(database_provider_error)?
        .pending_mutations
        == 0
    {
        return Ok(());
    }
    let result = deliver_mutations(database, account_id, provider).await;
    pause_for_permanent_auth_failure(database, account_id, result)
}

/// Returns whether the sync ingested or removed any local mail.
pub async fn sync_with(
    database: &Database,
    account_id: &str,
    provider: &(impl MailSync + MailMutate + ?Sized),
) -> ProviderResult<bool> {
    if database
        .account_needs_reauth(account_id)
        .map_err(database_provider_error)?
    {
        return Ok(false);
    }
    let result = sync_active_with(database, account_id, provider).await;
    pause_for_permanent_auth_failure(database, account_id, result)
}

async fn sync_active_with(
    database: &Database,
    account_id: &str,
    provider: &(impl MailSync + MailMutate + ?Sized),
) -> ProviderResult<bool> {
    deliver_mutations(database, account_id, provider).await?;
    match database.cursor(account_id).map_err(database_provider_error)? {
        Some(cursor) => match incremental_sync(database, account_id, provider, &SyncCursor::new(cursor))
            .await
        {
            Err(ProviderError::InvalidCursor) => full_sync(database, account_id, provider).await,
            result => result,
        },
        None => full_sync(database, account_id, provider).await,
    }
}

fn pause_for_permanent_auth_failure<T>(
    database: &Database,
    account_id: &str,
    result: ProviderResult<T>,
) -> ProviderResult<T> {
    if let Err(error) = &result {
        if error.requires_reauthentication() {
            database
                .mark_account_needs_reauth(account_id, &error.to_string())
                .map_err(database_provider_error)?;
        }
    }
    result
}

async fn full_sync(
    database: &Database,
    account_id: &str,
    provider: &(impl MailSync + MailMutate + ?Sized),
) -> ProviderResult<bool> {
    match full_sync_attempt(database, account_id, provider).await {
        Err(ProviderError::InvalidCursor) => {
            database
                .discard_sync_recovery(account_id)
                .map_err(database_provider_error)?;
            full_sync_attempt(database, account_id, provider).await
        }
        result => result,
    }
}

/// A full sync rebuilds the local snapshot, so it always reports a change.
async fn full_sync_attempt(
    database: &Database,
    account_id: &str,
    provider: &(impl MailSync + MailMutate + ?Sized),
) -> ProviderResult<bool> {
    // An interrupted recovery must keep the normal sync cursor cleared rather
    // than running incrementally against an incomplete snapshot. Its separate,
    // durable generation lets the expensive thread refresh resume safely.
    database
        .clear_cursor(account_id)
        .map_err(database_provider_error)?;
    let starting_cursor = match database
        .recovery_cursor(account_id)
        .map_err(database_provider_error)?
    {
        Some(cursor) => SyncCursor::new(cursor),
        None => {
            // Capture the cursor before listing. The following incremental
            // pass closes the race with mail arriving while the list is
            // downloaded.
            let cursor = provider.baseline_cursor().await?;
            let server_inbox_ids = list_inbox_thread_ids(provider).await?;
            let local_inbox_ids = local_inbox_thread_ids(database, account_id)?;
            let recovery_ids = server_inbox_ids
                .union(&local_inbox_ids)
                .cloned()
                .collect::<Vec<_>>();
            database
                .begin_sync_recovery(account_id, cursor.as_str(), &recovery_ids)
                .map_err(database_provider_error)?;
            cursor
        }
    };
    loop {
        let batch = database
            .pending_sync_recovery_threads(account_id, RECOVERY_BATCH_SIZE)
            .map_err(database_provider_error)?;
        if batch.is_empty() {
            break;
        }
        ingest_threads(database, account_id, provider, batch.clone()).await?;
        database
            .complete_sync_recovery_threads(account_id, &batch)
            .map_err(database_provider_error)?;
    }
    incremental_sync(database, account_id, provider, &starting_cursor).await?;
    Ok(true)
}

/// Returns whether the provider reported any changed threads.
async fn incremental_sync(
    database: &Database,
    account_id: &str,
    provider: &(impl MailSync + ?Sized),
    cursor: &SyncCursor,
) -> ProviderResult<bool> {
    let mut position = cursor.clone();
    let mut changed = HashSet::new();
    // Only the cursor returned by the final page is persisted: a provider may
    // report its newest position on every page, so advancing early would skip
    // the pages not yet read.
    let mut pages = 0;
    let final_cursor = loop {
        let batch = provider.poll(&position).await?;
        changed.extend(batch.changed_threads);
        if !batch.more {
            break batch.cursor;
        }
        pages += 1;
        // A page that does not advance, or a round that never ends, cannot
        // complete; start over from a fresh baseline instead of spinning.
        if batch.cursor == position || pages >= MAX_INCREMENTAL_SYNC_PAGES {
            return Err(ProviderError::InvalidCursor);
        }
        position = batch.cursor;
    };
    let any_changed = !changed.is_empty();
    ingest_threads(
        database,
        account_id,
        provider,
        changed.into_iter().collect(),
    )
    .await?;
    database
        .finish_sync(account_id, final_cursor.as_str())
        .map_err(database_provider_error)?;
    Ok(any_changed)
}

async fn list_inbox_thread_ids(
    provider: &(impl MailSync + ?Sized),
) -> ProviderResult<HashSet<String>> {
    let mut page = None;
    let mut server_inbox_ids = HashSet::new();
    loop {
        let result = provider.list_inbox(page.as_deref()).await?;
        server_inbox_ids.extend(result.thread_ids);
        page = result.next;
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
        .map_err(database_provider_error)?
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
    provider: &(impl MailSync + ?Sized),
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
    provider: &(impl MailSync + ?Sized),
) -> ProviderResult<()> {
    // Reconciliation failures are otherwise swallowed by the poller, so a
    // revoked grant must pause the account here just as a sync would.
    let result = match reconcile_inbox(database, account_id, provider).await {
        Ok(()) => database
            .mark_reconciled(account_id)
            .map_err(database_provider_error),
        Err(error) => Err(error),
    };
    pause_for_permanent_auth_failure(database, account_id, result)
}

async fn search_and_ingest_missing(
    database: &Database,
    account_id: &str,
    provider: &(impl MailSync + ?Sized),
    query: &str,
    scan_limit: usize,
) -> ProviderResult<()> {
    if query.trim().is_empty() || scan_limit == 0 {
        return Ok(());
    }

    let cached: HashSet<String> = database
        .local_provider_thread_ids(account_id)
        .map_err(database_provider_error)?
        .into_iter()
        .collect();
    let mut seen = HashSet::new();
    let mut missing = Vec::new();
    let mut scanned = 0;
    let mut page = None;
    loop {
        let result = provider.search(query, page.as_deref()).await?;
        for id in result.thread_ids {
            if scanned >= scan_limit {
                break;
            }
            scanned += 1;
            if seen.insert(id.clone()) && !cached.contains(&id) {
                missing.push(id);
            }
        }
        if scanned >= scan_limit || result.next.is_none() {
            break;
        }
        page = result.next;
    }

    if missing.is_empty() {
        return Ok(());
    }
    ingest_threads(database, account_id, provider, missing).await
}

/// Imports up to [`SENT_BACKFILL_FETCHES_PER_STEP`] uncached sent threads
/// from the saved resume point, then saves the next one. The offset within a
/// page advances past every result examined, fetched or not, so a thread that
/// never imports cannot pin the backfill to one page. Returns whether more
/// work remains.
async fn backfill_sent_step(
    database: &Database,
    account_id: &str,
    provider: &(impl MailSync + ?Sized),
) -> ProviderResult<bool> {
    let Some(progress) = database
        .sent_backfill_progress(account_id)
        .map_err(database_provider_error)?
    else {
        return Ok(false);
    };
    let result = match provider.search(SENT_BACKFILL_QUERY, progress.page.as_deref()).await {
        Ok(result) => result,
        // A page token the provider no longer honors. Starting over is cheap:
        // threads already cached are skipped without a fetch.
        Err(ProviderError::InvalidOperation(_) | ProviderError::InvalidCursor)
            if progress.page.is_some() =>
        {
            database
                .record_sent_backfill(account_id, Some(&SentBackfillProgress::default()))
                .map_err(database_provider_error)?;
            return Ok(true);
        }
        Err(error) => return Err(error),
    };

    let cached: HashSet<String> = database
        .local_provider_thread_ids(account_id)
        .map_err(database_provider_error)?
        .into_iter()
        .collect();
    let page_len = result
        .thread_ids
        .len()
        .min(MAX_SENT_BACKFILL_THREADS.saturating_sub(progress.scanned));
    let mut position = progress.offset.min(page_len);
    let mut missing = Vec::new();
    while position < page_len && missing.len() < SENT_BACKFILL_FETCHES_PER_STEP {
        let id = &result.thread_ids[position];
        position += 1;
        if !cached.contains(id) && !missing.contains(id) {
            missing.push(id.clone());
        }
    }
    ingest_threads(database, account_id, provider, missing).await?;

    let next = if position < page_len {
        Some(SentBackfillProgress {
            offset: position,
            ..progress
        })
    } else {
        let scanned = progress.scanned + page_len;
        result
            .next
            .filter(|_| scanned < MAX_SENT_BACKFILL_THREADS)
            .map(|page| SentBackfillProgress {
                page: Some(page),
                offset: 0,
                scanned,
            })
    };
    database
        .record_sent_backfill(account_id, next.as_ref())
        .map_err(database_provider_error)?;
    Ok(next.is_some())
}

// Flushed periodically, not just once at the end, so a) a transient
// mid-scan failure doesn't discard threads already fetched, and b) matches
// land in the local cache (and become searchable) before the whole scan
// finishes.
const INGEST_FLUSH_BATCH_SIZE: usize = 10;

async fn ingest_threads(
    database: &Database,
    account_id: &str,
    provider: &(impl MailSync + ?Sized),
    ids: Vec<String>,
) -> ProviderResult<()> {
    let mut ingested_threads = Vec::with_capacity(INGEST_FLUSH_BATCH_SIZE);
    let mut deleted = Vec::new();
    let mut pending_error = None;
    for id in ids {
        let messages = match provider.fetch_thread(&id).await {
            Ok(messages) => messages,
            // A thread fetch's only variable is the id, so a permanent
            // invalid-operation rejection — Gmail answers 400 "Invalid id
            // value" for ids it could never have minted, such as a locally
            // seeded fixture thread adopted onto a real account — means the
            // id is unusable forever, not momentarily failing. Treat it like
            // a thread the server no longer has: drop the local copy and
            // keep syncing. Aborting here instead would wedge every full
            // sync on the same id and the account would never record a
            // successful sync. Unknown or retryable errors still abort the
            // round so they are retried and surfaced.
            Err(ProviderError::NotFound | ProviderError::InvalidOperation(_)) => {
                deleted.push(id);
                continue;
            }
            Err(error) => {
                pending_error = Some(error);
                break;
            }
        };
        let (normalized, quarantined) = normalize_thread(&messages);
        ingested_threads.push((id, normalized, quarantined));
        if ingested_threads.len() >= INGEST_FLUSH_BATCH_SIZE {
            database
                .apply_ingested_threads(account_id, &ingested_threads)
                .map_err(database_provider_error)?;
            ingested_threads.clear();
        }
    }
    if !ingested_threads.is_empty() {
        database
            .apply_ingested_threads(account_id, &ingested_threads)
                .map_err(database_provider_error)?;
    }
    for id in deleted {
        database
            .delete_thread(account_id, &id)
                .map_err(database_provider_error)?;
    }
    if let Some(error) = pending_error {
        return Err(error);
    }
    Ok(())
}

fn normalize_thread(
    messages: &[crate::mime::RawMessage],
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
    provider: &(impl MailMutate + ?Sized),
) -> ProviderResult<()> {
    loop {
        let mutations = database
            .claim_mutations(account_id, 50)
                .map_err(database_provider_error)?;
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
                        .map_err(database_provider_error)?
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
                            .map_err(database_provider_error)?;
                    }
                }
                Err(error) if error.requires_reauthentication() => {
                    database
                        .mark_account_needs_reauth(account_id, &error.to_string())
                        .map_err(database_provider_error)?;
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
                            .map_err(database_provider_error)?;
                    }
                    return Err(error);
                }
                Err(error) => {
                    for item in &mutations[index..batch_end] {
                        database
                            .reject_mutation(&item.id, &error.to_string(), None)
                            .map_err(database_provider_error)?;
                    }
                }
            }
            index = batch_end;
        }
    }
}

fn mutation_next_attempt_at(attempts: u32) -> String {
    retry_at(attempts)
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
        mime::{MimeBody, MimeHeader, MimePart, RawMessage},
        provider::{SyncBatch, ThreadPage},
    };

    fn offline_service() -> SyncService {
        let credential = crate::auth::OAuthCredential::in_memory_for_test(
            "http://127.0.0.1:9/token",
            crate::auth::Tokens { access_token: "token".into(), refresh_token: Some("refresh".into()), expires_at: u64::MAX },
        );
        SyncService::new(Arc::new(Database::open_memory()), AccountAuth::Gmail(credential))
    }

    #[tokio::test]
    async fn retiring_an_account_drops_a_run_in_flight_on_another_clone_before_it_writes() {
        let service = offline_service();
        let foreground = service.clone();
        let writes = Arc::new(AtomicUsize::new(0));
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (provider_tx, provider_rx) = tokio::sync::oneshot::channel::<()>();
        let run = tokio::spawn({
            let writes = Arc::clone(&writes);
            async move {
                foreground
                    .exclusive(async {
                        entered_tx.send(()).unwrap();
                        // Waiting on the provider, then writing what it returned.
                        let _ = provider_rx.await;
                        writes.fetch_add(1, Ordering::SeqCst);
                    })
                    .await
            }
        });
        entered_rx.await.unwrap();

        tokio::time::timeout(Duration::from_secs(5), service.retire()).await.expect("retire waits only for the run to stop");
        let _ = provider_tx.send(());
        assert_eq!(run.await.unwrap(), None);
        assert_eq!(writes.load(Ordering::SeqCst), 0);

        // Later runs on any clone don't start.
        let later = service.clone().exclusive(async { writes.fetch_add(1, Ordering::SeqCst) }).await;
        assert_eq!(later, None);
        assert_eq!(writes.load(Ordering::SeqCst), 0);
        let error = service.sync().await.unwrap_err();
        assert!(error.contains(ACCOUNT_REMOVED), "{error}");
    }

    #[tokio::test]
    async fn a_failed_removal_resumes_sync_for_the_account() {
        let service = offline_service();
        service.retire().await;
        service.resume();
        assert_eq!(service.clone().exclusive(async { 7 }).await, Some(7));
    }

    struct ContractProvider {
        invalidate_stale_cursor: AtomicBool,
        full_lists: AtomicUsize,
        thread_fetches: AtomicUsize,
        modifies: AtomicUsize,
        message_modifies: StdMutex<Vec<(Vec<String>, Vec<String>, Vec<String>)>>,
        fail_mutation: bool,
        permanently_fail_mutation: bool,
        reauth_mutation: bool,
        /// Return `InvalidOperation` from every mutation entry point — exactly
        /// what the IMAP provider's phase-2 `MailMutate` stubs do. Used by the
        /// mutation-queue regression test that pins the engine's handling of a
        /// provider that cannot mutate.
        invalid_operation_mutation: bool,
        thread_messages: Option<Vec<RawMessage>>,
    }

    impl ContractProvider {
        fn normal() -> Self {
            Self {
                invalidate_stale_cursor: AtomicBool::new(false),
                full_lists: AtomicUsize::new(0),
                thread_fetches: AtomicUsize::new(0),
                modifies: AtomicUsize::new(0),
                message_modifies: StdMutex::new(vec![]),
                fail_mutation: false,
                permanently_fail_mutation: false,
                reauth_mutation: false,
                invalid_operation_mutation: false,
                thread_messages: None,
            }
        }

        fn message() -> RawMessage {
            RawMessage {
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
    impl MailSync for ContractProvider {
        async fn baseline_cursor(&self) -> ProviderResult<SyncCursor> {
            Ok(SyncCursor::new("current"))
        }

        async fn list_inbox(&self, page: Option<&str>) -> ProviderResult<ThreadPage> {
            assert!(page.is_none());
            self.full_lists.fetch_add(1, Ordering::SeqCst);
            Ok(ThreadPage {
                thread_ids: vec!["gmail-thread".into()],
                next: None,
            })
        }

        async fn search(&self, query: &str, page: Option<&str>) -> ProviderResult<ThreadPage> {
            assert_eq!(query, "126");
            assert!(page.is_none());
            Ok(ThreadPage {
                thread_ids: vec!["gmail-thread".into()],
                next: None,
            })
        }

        async fn fetch_thread(&self, id: &str) -> ProviderResult<Vec<RawMessage>> {
            self.thread_fetches.fetch_add(1, Ordering::SeqCst);
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

        async fn poll(&self, cursor: &SyncCursor) -> ProviderResult<SyncBatch> {
            if cursor.as_str() == "stale"
                && self.invalidate_stale_cursor.swap(false, Ordering::SeqCst)
            {
                return Err(ProviderError::InvalidCursor);
            }
            Ok(SyncBatch {
                changed_threads: vec![],
                cursor: SyncCursor::new("current"),
                more: false,
            })
        }

    }

    #[async_trait]
    impl MailMutate for ContractProvider {
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
            } else if self.invalid_operation_mutation {
                Err(ProviderError::InvalidOperation(
                    "mutations not supported until phase 3".into(),
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
            } else if self.invalid_operation_mutation {
                Err(ProviderError::InvalidOperation(
                    "mutations not supported until phase 3".into(),
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

    fn recording_activity() -> (SyncActivity, Arc<StdMutex<Vec<(String, bool)>>>) {
        let events = Arc::new(StdMutex::new(Vec::new()));
        let recorded = Arc::clone(&events);
        let activity = SyncActivity::with_listener(move |account_id, active| {
            recorded.lock().unwrap().push((account_id.to_string(), active));
        });
        (activity, events)
    }

    #[test]
    fn sync_activity_reports_each_accounts_first_start_and_last_finish() {
        let (activity, events) = recording_activity();

        let first = activity.begin("a@example.com");
        let overlapping = activity.begin("a@example.com");
        let other = activity.begin("b@example.com");
        assert_eq!(activity.active_accounts(), ["a@example.com", "b@example.com"]);

        drop(first);
        assert_eq!(activity.active_accounts(), ["a@example.com", "b@example.com"]);
        drop(other);
        drop(overlapping);
        assert!(activity.active_accounts().is_empty());

        assert_eq!(
            *events.lock().unwrap(),
            [
                ("a@example.com".to_string(), true),
                ("b@example.com".to_string(), true),
                ("b@example.com".to_string(), false),
                ("a@example.com".to_string(), false),
            ]
        );
    }

    #[tokio::test]
    async fn a_sync_reports_activity_while_it_runs_and_a_retired_account_reports_none() {
        let credential = crate::auth::OAuthCredential::in_memory_for_test(
            "http://127.0.0.1:9/token",
            crate::auth::Tokens { access_token: "token".into(), refresh_token: Some("refresh".into()), expires_at: u64::MAX },
        );
        let database = Arc::new(Database::open_memory());
        let (activity, events) = recording_activity();
        let service = SyncService::new(Arc::clone(&database), AccountAuth::Gmail(credential))
            .with_activity(activity.clone());
        let account_id = service.account_id();
        // A paused account syncs without contacting the provider.
        database.adopt_account(&account_id).unwrap();
        database.mark_account_needs_reauth(&account_id, "revoked").unwrap();

        service.sync().await.unwrap();
        assert_eq!(
            *events.lock().unwrap(),
            [(account_id.clone(), true), (account_id.clone(), false)]
        );
        assert!(activity.active_accounts().is_empty());

        service.retire().await;
        events.lock().unwrap().clear();
        assert!(service.sync().await.is_err());
        assert!(events.lock().unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn an_unwoken_wait_sleeps_out_the_backed_off_delay() {
        let wake = Notify::new();
        let started = tokio::time::Instant::now();

        let delay = wait_for_next_poll(&wake, MAX_POLL_INTERVAL, Duration::ZERO).await;

        assert_eq!(delay, MAX_POLL_INTERVAL);
        assert_eq!(started.elapsed(), MAX_POLL_INTERVAL);
    }

    #[tokio::test(start_paused = true)]
    async fn waking_a_backed_off_wait_polls_within_the_floor_and_resets_backoff() {
        let wake = Arc::new(Notify::new());
        let started = tokio::time::Instant::now();
        let waiter = tokio::spawn({
            let wake = Arc::clone(&wake);
            async move { wait_for_next_poll(&wake, MAX_POLL_INTERVAL, Duration::ZERO).await }
        });
        tokio::time::sleep(Duration::from_secs(60)).await;

        wake.notify_one();

        assert_eq!(waiter.await.unwrap(), MIN_POLL_INTERVAL);
        assert_eq!(started.elapsed(), Duration::from_secs(60) + MIN_POLL_INTERVAL);
    }

    #[tokio::test(start_paused = true)]
    async fn a_wake_sent_during_a_poll_applies_to_the_next_wait() {
        let wake = Notify::new();
        wake.notify_one();
        let started = tokio::time::Instant::now();

        let delay = wait_for_next_poll(&wake, MAX_POLL_INTERVAL, Duration::ZERO).await;

        assert_eq!(delay, MIN_POLL_INTERVAL);
        assert_eq!(started.elapsed(), MIN_POLL_INTERVAL);
    }

    #[test]
    fn a_wake_never_postpones_a_poll_that_was_already_closer() {
        let now = tokio::time::Instant::now();
        let soon = now + Duration::from_secs(3);
        assert_eq!(woken_poll_deadline(soon, now, Duration::ZERO), soon);
        let jitter = Duration::from_millis(100);
        assert_eq!(
            woken_poll_deadline(now + MAX_POLL_INTERVAL, now, jitter),
            now + MIN_POLL_INTERVAL + jitter
        );
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
            POLL_FAILURE_RETRY_INTERVAL
        );
    }

    #[tokio::test]
    async fn sync_reports_whether_local_mail_changed() {
        let database = Database::open_memory();
        let provider = ContractProvider::normal();

        // No cursor yet: the full sync rebuilds the local snapshot.
        assert!(sync_with(&database, "default", &provider).await.unwrap());
        // The follow-up incremental poll finds no changed threads.
        assert!(!sync_with(&database, "default", &provider).await.unwrap());
    }

    #[test]
    fn polls_notify_the_ui_only_when_something_changed() {
        let status = |pending_mutations| SyncStatus {
            state: "idle",
            last_successful_sync: None,
            cursor: None,
            pending_mutations,
            failed_mutations: vec![],
            quarantined_messages: vec![],
            error: None,
        };

        assert!(!should_notify_after_poll(0, &Ok((status(0), false))));
        assert!(should_notify_after_poll(0, &Ok((status(0), true))));
        // Settling queued mutations changes what the UI shows even when no
        // new mail arrived.
        assert!(should_notify_after_poll(2, &Ok((status(0), false))));
        assert!(!should_notify_after_poll(
            0,
            &Err(ProviderError::RetryableServer("offline".into()))
        ));
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

    /// A provider that reports changes across two pages and, like Gmail,
    /// names its newest position on every page — including the first.
    struct PagedProvider {
        polls: StdMutex<Vec<String>>,
    }

    #[async_trait]
    impl MailSync for PagedProvider {
        async fn baseline_cursor(&self) -> ProviderResult<SyncCursor> {
            unreachable!()
        }

        async fn list_inbox(&self, _: Option<&str>) -> ProviderResult<ThreadPage> {
            unreachable!()
        }

        async fn fetch_thread(&self, id: &str) -> ProviderResult<Vec<RawMessage>> {
            let mut message = ContractProvider::message();
            message.id = id.into();
            message.thread_id = id.into();
            Ok(vec![message])
        }

        async fn poll(&self, cursor: &SyncCursor) -> ProviderResult<SyncBatch> {
            self.polls.lock().unwrap().push(cursor.as_str().into());
            Ok(match cursor.as_str() {
                // First page: more to come, so the position carries a token
                // alongside the newest history id.
                "start" => SyncBatch {
                    changed_threads: vec!["gmail-thread".into()],
                    cursor: SyncCursor::new("newest page-2"),
                    more: true,
                },
                // Final page: a page-free cursor, safe to persist.
                _ => SyncBatch {
                    changed_threads: vec!["gmail-thread".into()],
                    cursor: SyncCursor::new("newest"),
                    more: false,
                },
            })
        }
    }

    #[tokio::test]
    async fn a_multi_page_poll_persists_only_the_cursor_from_its_final_page() {
        let database = Database::open_memory();
        let provider = PagedProvider {
            polls: StdMutex::new(Vec::new()),
        };
        incremental_sync(&database, "default", &provider, &SyncCursor::new("start"))
            .await
            .unwrap();

        // The second request resumes from the first page's position rather
        // than restarting, and the persisted cursor is the page-free one.
        assert_eq!(
            *provider.polls.lock().unwrap(),
            vec!["start".to_string(), "newest page-2".to_string()]
        );
        assert_eq!(
            database.cursor("default").unwrap().as_deref(),
            Some("newest"),
            "persisting a mid-round cursor would skip the pages not yet read"
        );
    }

    /// A provider whose change listing always reports another page, either
    /// repeating the position it was asked for or minting a new one forever.
    struct EndlessPagesProvider {
        repeat_position: bool,
        polls: AtomicUsize,
    }

    #[async_trait]
    impl MailSync for EndlessPagesProvider {
        async fn baseline_cursor(&self) -> ProviderResult<SyncCursor> {
            unreachable!()
        }

        async fn list_inbox(&self, _: Option<&str>) -> ProviderResult<ThreadPage> {
            unreachable!()
        }

        async fn fetch_thread(&self, _: &str) -> ProviderResult<Vec<RawMessage>> {
            unreachable!("an unfinished round must not ingest anything")
        }

        async fn poll(&self, cursor: &SyncCursor) -> ProviderResult<SyncBatch> {
            let poll = self.polls.fetch_add(1, Ordering::SeqCst);
            Ok(SyncBatch {
                changed_threads: vec!["gmail-thread".into()],
                cursor: if self.repeat_position {
                    cursor.clone()
                } else {
                    SyncCursor::new(format!("start page-{poll}"))
                },
                more: true,
            })
        }
    }

    #[tokio::test]
    async fn a_poll_page_that_does_not_advance_requests_a_full_resync() {
        let database = Database::open_memory();
        let provider = EndlessPagesProvider {
            repeat_position: true,
            polls: AtomicUsize::new(0),
        };

        let result =
            incremental_sync(&database, "default", &provider, &SyncCursor::new("start")).await;

        assert!(matches!(result, Err(ProviderError::InvalidCursor)));
        assert_eq!(provider.polls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn an_incremental_round_that_never_ends_is_bounded() {
        let database = Database::open_memory();
        let provider = EndlessPagesProvider {
            repeat_position: false,
            polls: AtomicUsize::new(0),
        };

        let result =
            incremental_sync(&database, "default", &provider, &SyncCursor::new("start")).await;

        assert!(matches!(result, Err(ProviderError::InvalidCursor)));
        assert_eq!(
            provider.polls.load(Ordering::SeqCst),
            MAX_INCREMENTAL_SYNC_PAGES
        );
    }

    #[tokio::test]
    async fn remote_search_ingests_only_missing_archived_matches() {
        let database = Database::open_memory();
        let mut archived = ContractProvider::message();
        archived.label_ids.clear();
        archived.snippet = "Historical reference 126".into();
        archived.payload.body.data = Some(URL_SAFE_NO_PAD.encode("Historical reference 126"));
        let provider = ContractProvider {
            thread_messages: Some(vec![archived]),
            ..ContractProvider::normal()
        };

        search_and_ingest_missing(&database, "default", &provider, "126", 200)
            .await
            .unwrap();
        search_and_ingest_missing(&database, "default", &provider, "126", 200)
            .await
            .unwrap();

        let matches = database
            .search_threads(
                &crate::models::SearchThreadsRequest {
                    query: "126".into(),
                    limit: None,
                    offset: None,
                    include_archived: Some(true),
                },
                Some("default"),
            )
            .unwrap();
        assert_eq!(matches.len(), 1);
        assert!(matches[0].archived);
        assert_eq!(provider.thread_fetches.load(Ordering::SeqCst), 1);
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

        database.dismiss_sync_problems().unwrap();
        assert!(database
            .sync_status("default")
            .unwrap()
            .quarantined_messages
            .is_empty());
        assert_eq!(database.list_threads(None).unwrap().len(), 1);
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
        impl MailSync for TransientAuthProvider {
            async fn baseline_cursor(&self) -> ProviderResult<SyncCursor> {
                unreachable!()
            }
            async fn list_inbox(&self, _: Option<&str>) -> ProviderResult<ThreadPage> {
                unreachable!()
            }
            async fn fetch_thread(&self, _: &str) -> ProviderResult<Vec<RawMessage>> {
                unreachable!()
            }
            async fn poll(&self, _: &SyncCursor) -> ProviderResult<SyncBatch> {
                unreachable!()
            }
        }

        #[async_trait]
        impl MailMutate for TransientAuthProvider {
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

    // Regression pin for SLICE5A_BRIEF work item 5: a provider whose
    // `MailMutate` returns `InvalidOperation` — exactly what the IMAP stubs do
    // this phase — must make the queued local mutation roll back to a PERMANENT
    // `failed` state, NOT retry forever and NOT wedge the queue. `InvalidOperation`
    // is neither `retry_mutation()` nor `requires_reauthentication()`, so
    // `deliver_mutations` rejects it with `next_attempt_at = None`, which
    // `reject_mutation` records as `failed` (never re-claimed by
    // `claim_mutations`, which only selects `pending`). This test fails loudly
    // if that engine behaviour ever regresses into a retry loop.
    #[tokio::test]
    async fn an_invalid_operation_mutation_rolls_back_permanently_and_does_not_wedge() {
        let database = Database::open_memory();
        database
            .mutate_thread(&ThreadMutation::Archive {
                thread_id: "welcome".into(),
                value: true,
            })
            .unwrap();
        let provider = ContractProvider {
            invalid_operation_mutation: true,
            ..ContractProvider::normal()
        };

        // One delivery attempt: the mutation is tried once and rejected.
        deliver_mutations(&database, "default", &provider)
            .await
            .unwrap();
        assert_eq!(
            provider.modifies.load(Ordering::SeqCst),
            1,
            "the mutation was attempted exactly once"
        );

        let status = database.sync_status("default").unwrap();
        assert_eq!(status.pending_mutations, 0, "nothing is left pending/running");
        assert_eq!(status.failed_mutations.len(), 1, "it failed permanently");
        assert_eq!(status.failed_mutations[0].kind, "archive");

        // A second delivery pass must NOT re-attempt it — a `failed` row is
        // never re-claimed, so the queue is not wedged and does not retry.
        deliver_mutations(&database, "default", &provider)
            .await
            .unwrap();
        assert_eq!(
            provider.modifies.load(Ordering::SeqCst),
            1,
            "a failed InvalidOperation mutation is never retried"
        );
        assert_eq!(
            database.sync_status("default").unwrap().failed_mutations.len(),
            1
        );
    }

    #[tokio::test]
    async fn failed_mutations_can_be_retried_or_dismissed() {
        let database = Database::open_memory();
        database
            .mutate_thread(&ThreadMutation::Archive {
                thread_id: "welcome".into(),
                value: true,
            })
            .unwrap();
        let failing = ContractProvider {
            permanently_fail_mutation: true,
            ..ContractProvider::normal()
        };
        sync_with(&database, "default", &failing).await.unwrap();
        assert_eq!(database.sync_status("default").unwrap().failed_mutations.len(), 1);

        assert_eq!(database.retry_failed_mutations().unwrap(), 1);
        let status = database.sync_status("default").unwrap();
        assert!(status.failed_mutations.is_empty());
        assert_eq!(status.pending_mutations, 1);
        assert_eq!(status.error, None);

        sync_with(&database, "default", &failing).await.unwrap();
        let status = database.sync_status("default").unwrap();
        assert_eq!(status.failed_mutations.len(), 1);
        assert_eq!(status.failed_mutations[0].attempts, 1);
        assert_eq!(status.state, "error");

        database.dismiss_sync_problems().unwrap();
        let status = database.sync_status("default").unwrap();
        assert!(status.failed_mutations.is_empty());
        assert_eq!(status.pending_mutations, 0);
        assert_eq!(status.error, None);
        assert_eq!(status.state, "idle");
    }

    #[tokio::test]
    async fn rate_limit_resets_the_entire_remaining_claimed_batch_to_pending() {
        let database = Database::open_memory();
        let mut second = ContractProvider::message();
        second.id = "message-2".into();
        second.thread_id = "second-thread".into();
        database
            .upsert_thread("default", &[crate::mime::normalize(&second).unwrap()])
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
            .upsert_thread(
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
            .upsert_thread(
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
            .upsert_thread(
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
            .upsert_thread("default", &[crate::mime::normalize(&message).unwrap()])
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
        threads: std::collections::HashMap<String, RawMessage>,
        list_calls: AtomicUsize,
        fail_thread_once: StdMutex<Option<String>>,
        /// Thread ids this provider rejects the way Gmail answers a locally
        /// minted id: a permanent 400 "Invalid id value".
        invalid_thread_ids: Vec<String>,
    }

    #[async_trait]
    impl MailSync for ReconcileProvider {
        async fn baseline_cursor(&self) -> ProviderResult<SyncCursor> {
            Ok(SyncCursor::new("resynced"))
        }

        async fn list_inbox(&self, page: Option<&str>) -> ProviderResult<ThreadPage> {
            assert!(page.is_none());
            self.list_calls.fetch_add(1, Ordering::SeqCst);
            Ok(ThreadPage {
                thread_ids: self.inbox_ids.clone(),
                next: None,
            })
        }

        async fn fetch_thread(&self, id: &str) -> ProviderResult<Vec<RawMessage>> {
            if self.invalid_thread_ids.iter().any(|invalid| invalid == id) {
                return Err(ProviderError::InvalidOperation(
                    "400 Bad Request: Invalid id value".into(),
                ));
            }
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

        async fn poll(&self, _cursor: &SyncCursor) -> ProviderResult<SyncBatch> {
            Ok(SyncBatch {
                changed_threads: vec![],
                cursor: SyncCursor::new("resynced"),
                more: false,
            })
        }

    }

    #[async_trait]
    impl MailMutate for ReconcileProvider {
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
            .upsert_thread(account, &[crate::mime::normalize(&restored).unwrap()])
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
            .upsert_thread(account, &[crate::mime::normalize(&orphaned).unwrap()])
            .unwrap();

        // Untouched by drift: stays archived, and reconcile must never fetch it.
        let mut settled = ContractProvider::message();
        settled.id = "settled-message".into();
        settled.thread_id = "settled-thread".into();
        settled.label_ids = vec![];
        database
            .upsert_thread(account, &[crate::mime::normalize(&settled).unwrap()])
            .unwrap();
        database
            .mutate_thread(&ThreadMutation::Archive {
                thread_id: format!("{account}:settled-thread"),
                value: true,
            })
            .unwrap();
        // Drift means the archives already reached Gmail (or were lost);
        // an undelivered archive is the user's pending intent, which
        // reconcile keeps instead (see
        // `reconcile_keeps_an_archive_that_has_not_been_delivered_yet`).
        database
            .connection()
            .unwrap()
            .execute("UPDATE mutations SET state = 'done'", [])
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
            invalid_thread_ids: vec![],
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
    async fn reconcile_keeps_an_archive_that_has_not_been_delivered_yet() {
        let account = "acct";
        let database = Database::open_memory();
        let mut pending = ContractProvider::message();
        pending.id = "pending-message".into();
        pending.thread_id = "pending-thread".into();
        database
            .upsert_thread(account, &[crate::mime::normalize(&pending).unwrap()])
            .unwrap();
        database
            .mutate_thread(&ThreadMutation::Archive {
                thread_id: format!("{account}:pending-thread"),
                value: true,
            })
            .unwrap();

        // Gmail still lists it in INBOX because the archive hasn't reached it.
        let provider = ReconcileProvider {
            inbox_ids: vec!["pending-thread".into()],
            threads: [("pending-thread".to_string(), pending)].into_iter().collect(),
            list_calls: AtomicUsize::new(0),
            fail_thread_once: StdMutex::new(None),
            invalid_thread_ids: vec![],
        };
        reconcile_inbox(&database, account, &provider).await.unwrap();

        let thread = database
            .list_all_mail(Some(account))
            .unwrap()
            .into_iter()
            .find(|thread| thread.id == format!("{account}:pending-thread"))
            .unwrap();
        assert!(thread.archived, "the user's pending archive must survive reconciliation");
        let pending: i64 = database
            .connection()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM mutations WHERE state = 'pending'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(pending, 1);
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
            invalid_thread_ids: vec![],
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
    async fn reconciliation_with_revoked_credentials_pauses_the_account() {
        struct RevokedProvider;
        #[async_trait]
        impl MailSync for RevokedProvider {
            async fn baseline_cursor(&self) -> ProviderResult<SyncCursor> {
                unreachable!()
            }
            async fn list_inbox(&self, _: Option<&str>) -> ProviderResult<ThreadPage> {
                Err(ProviderError::ReauthenticationRequired(
                    "401 Unauthorized".into(),
                ))
            }
            async fn fetch_thread(&self, _: &str) -> ProviderResult<Vec<RawMessage>> {
                unreachable!()
            }
            async fn poll(&self, _: &SyncCursor) -> ProviderResult<SyncBatch> {
                unreachable!()
            }
        }
        let account = "work@example.com";
        let database = Database::open_memory();
        database.adopt_account(account).unwrap();

        assert!(reconcile_and_mark(&database, account, &RevokedProvider)
            .await
            .is_err());

        assert!(database.account_needs_reauth(account).unwrap());
        assert!(database.reconciliation_due(account, 1).unwrap());
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
            .upsert_thread(account, &[crate::mime::normalize(&archived).unwrap()])
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
        // The inbox preview comes from the latest message's body, so the body
        // marks which copy of the thread is cached.
        stale_common.payload.body.data = Some(URL_SAFE_NO_PAD.encode("stale body"));
        database
            .upsert_thread(account, &[crate::mime::normalize(&stale_common).unwrap()])
            .unwrap();
        let mut fresh_common = stale_common;
        fresh_common.payload.body.data = Some(URL_SAFE_NO_PAD.encode("fresh body"));

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
            invalid_thread_ids: vec![],
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
            "fresh body",
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
            invalid_thread_ids: vec![],
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
    async fn a_thread_the_provider_permanently_rejects_is_dropped_not_wedged() {
        // Regression: a locally seeded fixture thread (provider id
        // "demo-welcome") adopted onto a real account joined every full
        // sync's recovery set. Gmail answers such an id with 400 "Invalid
        // id value" — `InvalidOperation`, not `NotFound` — which aborted
        // each round before `finish_sync` could run, so the account
        // re-fetched the same threads forever and never recorded a
        // successful sync.
        let account = "acct-invalid-thread-id";
        let database = Database::open_memory();
        database
            .connection()
            .unwrap()
            .execute("INSERT INTO sync_state(account_id) VALUES (?1)", [account])
            .unwrap();

        // The fixture thread: cached locally, in the inbox, and never
        // fetchable from the provider.
        let mut fixture = ContractProvider::message();
        fixture.id = "fixture-message".into();
        fixture.thread_id = "demo-welcome".into();
        database
            .upsert_thread(account, &[crate::mime::normalize(&fixture).unwrap()])
            .unwrap();

        let mut real = ContractProvider::message();
        real.id = "real-message".into();
        real.thread_id = "real-thread".into();
        let provider = ReconcileProvider {
            inbox_ids: vec!["real-thread".into()],
            threads: [("real-thread".to_string(), real)]
                .into_iter()
                .collect(),
            list_calls: AtomicUsize::new(0),
            fail_thread_once: StdMutex::new(None),
            invalid_thread_ids: vec!["demo-welcome".into()],
        };

        sync_with(&database, account, &provider).await.unwrap();

        let threads = database.list_all_mail(Some(account)).unwrap();
        assert!(
            threads
                .iter()
                .any(|thread| thread.id == format!("{account}:real-thread")),
            "the server's current inbox listing must still be ingested"
        );
        assert!(
            threads
                .iter()
                .all(|thread| thread.id != format!("{account}:demo-welcome")),
            "a thread the provider permanently rejects must be dropped locally"
        );
        // The round completed instead of wedging, so success was recorded.
        assert_eq!(
            database.cursor(account).unwrap().as_deref(),
            Some("resynced")
        );
        assert!(database
            .sync_status(account)
            .unwrap()
            .last_successful_sync
            .is_some());
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

    const BACKFILL_ACCOUNT: &str = "me@example.com";

    /// Fake Gmail whose `in:sent` search answers from fixed pages, keyed by
    /// page token (`""` for the first page).
    struct SentBackfillProvider {
        pages: std::collections::HashMap<String, (Vec<String>, Option<String>)>,
        missing_threads: Vec<String>,
        expired_tokens: Vec<String>,
        searches: AtomicUsize,
        fetched: StdMutex<Vec<String>>,
    }

    impl SentBackfillProvider {
        fn new(pages: &[(&str, &[&str], Option<&str>)]) -> Self {
            Self {
                pages: pages
                    .iter()
                    .map(|(token, ids, next)| {
                        (
                            token.to_string(),
                            (ids.iter().map(|id| id.to_string()).collect(), next.map(str::to_string)),
                        )
                    })
                    .collect(),
                missing_threads: vec![],
                expired_tokens: vec![],
                searches: AtomicUsize::new(0),
                fetched: StdMutex::new(vec![]),
            }
        }

        fn fetched(&self) -> Vec<String> {
            self.fetched.lock().unwrap().clone()
        }
    }

    fn sent_message(thread_id: &str) -> RawMessage {
        let mut message = ContractProvider::message();
        message.id = format!("{thread_id}-message");
        message.thread_id = thread_id.into();
        message.label_ids = vec!["SENT".into()];
        message.payload.headers = vec![
            MimeHeader { name: "Subject".into(), value: format!("About {thread_id}") },
            MimeHeader { name: "From".into(), value: format!("Me <{BACKFILL_ACCOUNT}>") },
            MimeHeader { name: "To".into(), value: format!("Person <{thread_id}@example.org>") },
        ];
        message
    }

    #[async_trait]
    impl MailSync for SentBackfillProvider {
        async fn baseline_cursor(&self) -> ProviderResult<SyncCursor> {
            unreachable!()
        }
        async fn list_inbox(&self, _: Option<&str>) -> ProviderResult<ThreadPage> {
            unreachable!()
        }
        async fn poll(&self, _: &SyncCursor) -> ProviderResult<SyncBatch> {
            unreachable!()
        }
        async fn search(&self, query: &str, page: Option<&str>) -> ProviderResult<ThreadPage> {
            assert_eq!(query, SENT_BACKFILL_QUERY);
            self.searches.fetch_add(1, Ordering::SeqCst);
            let token = page.unwrap_or_default();
            if self.expired_tokens.iter().any(|expired| expired == token) {
                return Err(ProviderError::InvalidOperation("400 Invalid pageToken".into()));
            }
            let (thread_ids, next) = self.pages.get(token).cloned().expect("known page token");
            Ok(ThreadPage { thread_ids, next })
        }
        async fn fetch_thread(&self, id: &str) -> ProviderResult<Vec<RawMessage>> {
            self.fetched.lock().unwrap().push(id.to_string());
            if self.missing_threads.iter().any(|missing| missing == id) {
                return Err(ProviderError::NotFound);
            }
            Ok(vec![sent_message(id)])
        }
    }

    fn backfill_database() -> Database {
        let database = Database::open_memory();
        database
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO sync_state(account_id, cursor) VALUES (?1, 'synced')",
                [BACKFILL_ACCOUNT],
            )
            .unwrap();
        database
    }

    async fn run_sent_backfill(database: &Database, provider: &SentBackfillProvider) -> usize {
        let mut steps = 0;
        while backfill_sent_step(database, BACKFILL_ACCOUNT, provider).await.unwrap() {
            steps += 1;
            assert!(steps < 100, "backfill never finished");
        }
        steps + 1
    }

    #[tokio::test]
    async fn sent_backfill_imports_uncached_sent_threads_as_contacts() {
        let database = backfill_database();
        let provider = SentBackfillProvider::new(&[
            ("", &["cached", "older"], Some("p2")),
            ("p2", &["oldest"], None),
        ]);
        ingest_threads(&database, BACKFILL_ACCOUNT, &provider, vec!["cached".into()])
            .await
            .unwrap();
        provider.fetched.lock().unwrap().clear();

        run_sent_backfill(&database, &provider).await;

        assert_eq!(provider.fetched(), ["older", "oldest"]);
        assert_eq!(database.sent_backfill_progress(BACKFILL_ACCOUNT).unwrap(), None);
        let contacts = database
            .list_contact_suggestions(BACKFILL_ACCOUNT, "", 50)
            .unwrap();
        for email in ["cached@example.org", "older@example.org", "oldest@example.org"] {
            let contact = contacts
                .iter()
                .find(|contact| contact.email == email)
                .unwrap_or_else(|| panic!("{email} missing from {contacts:?}"));
            assert_eq!(contact.sent_count, 1);
        }

        // Finished for good: later polls don't search again.
        let searches = provider.searches.load(Ordering::SeqCst);
        assert!(!backfill_sent_step(&database, BACKFILL_ACCOUNT, &provider).await.unwrap());
        assert_eq!(provider.searches.load(Ordering::SeqCst), searches);
    }

    #[tokio::test]
    async fn sent_backfill_bounds_fetches_per_step_and_resumes_mid_page() {
        let database = backfill_database();
        let ids = (0..SENT_BACKFILL_FETCHES_PER_STEP + 5)
            .map(|index| format!("thread-{index}"))
            .collect::<Vec<_>>();
        let id_refs = ids.iter().map(String::as_str).collect::<Vec<_>>();
        let provider = SentBackfillProvider::new(&[("", &id_refs, None)]);

        assert!(backfill_sent_step(&database, BACKFILL_ACCOUNT, &provider).await.unwrap());
        assert_eq!(provider.fetched().len(), SENT_BACKFILL_FETCHES_PER_STEP);
        assert_eq!(
            database.sent_backfill_progress(BACKFILL_ACCOUNT).unwrap(),
            Some(SentBackfillProgress { page: None, offset: SENT_BACKFILL_FETCHES_PER_STEP, scanned: 0 })
        );

        assert!(!backfill_sent_step(&database, BACKFILL_ACCOUNT, &provider).await.unwrap());
        assert_eq!(provider.fetched(), ids);
    }

    #[tokio::test]
    async fn sent_backfill_moves_past_threads_that_never_import() {
        let database = backfill_database();
        let ids = (0..SENT_BACKFILL_FETCHES_PER_STEP + 1)
            .map(|index| format!("gone-{index}"))
            .chain(["kept".to_string()])
            .collect::<Vec<_>>();
        let id_refs = ids.iter().map(String::as_str).collect::<Vec<_>>();
        let mut provider = SentBackfillProvider::new(&[("", &id_refs, None)]);
        provider.missing_threads = ids[..ids.len() - 1].to_vec();

        assert_eq!(run_sent_backfill(&database, &provider).await, 2);
        assert_eq!(provider.fetched(), ids);
        assert!(database
            .list_contact_suggestions(BACKFILL_ACCOUNT, "kept", 5)
            .unwrap()
            .iter()
            .any(|contact| contact.email == "kept@example.org"));
    }

    #[tokio::test]
    async fn sent_backfill_restarts_when_a_page_token_expires() {
        let database = backfill_database();
        let mut provider = SentBackfillProvider::new(&[("", &["fresh"], None)]);
        provider.expired_tokens = vec!["stale".into()];
        database
            .record_sent_backfill(
                BACKFILL_ACCOUNT,
                Some(&SentBackfillProgress { page: Some("stale".into()), offset: 3, scanned: 100 }),
            )
            .unwrap();

        assert!(backfill_sent_step(&database, BACKFILL_ACCOUNT, &provider).await.unwrap());
        assert_eq!(
            database.sent_backfill_progress(BACKFILL_ACCOUNT).unwrap(),
            Some(SentBackfillProgress::default())
        );
        run_sent_backfill(&database, &provider).await;
        assert_eq!(provider.fetched(), ["fresh"]);
    }

    #[tokio::test]
    async fn sent_backfill_stops_at_the_thread_limit() {
        let database = backfill_database();
        let provider = SentBackfillProvider::new(&[("p9", &["last", "beyond"], Some("p10"))]);
        database
            .record_sent_backfill(
                BACKFILL_ACCOUNT,
                Some(&SentBackfillProgress {
                    page: Some("p9".into()),
                    offset: 0,
                    scanned: MAX_SENT_BACKFILL_THREADS - 1,
                }),
            )
            .unwrap();

        assert!(!backfill_sent_step(&database, BACKFILL_ACCOUNT, &provider).await.unwrap());
        assert_eq!(provider.fetched(), ["last"]);
        assert_eq!(database.sent_backfill_progress(BACKFILL_ACCOUNT).unwrap(), None);
    }

    #[tokio::test]
    async fn sent_backfill_surfaces_transient_failures_without_losing_progress() {
        struct FlakySearch;
        #[async_trait]
        impl MailSync for FlakySearch {
            async fn baseline_cursor(&self) -> ProviderResult<SyncCursor> {
                unreachable!()
            }
            async fn list_inbox(&self, _: Option<&str>) -> ProviderResult<ThreadPage> {
                unreachable!()
            }
            async fn poll(&self, _: &SyncCursor) -> ProviderResult<SyncBatch> {
                unreachable!()
            }
            async fn fetch_thread(&self, _: &str) -> ProviderResult<Vec<RawMessage>> {
                unreachable!()
            }
            async fn search(&self, _: &str, _: Option<&str>) -> ProviderResult<ThreadPage> {
                Err(ProviderError::RetryableServer("503".into()))
            }
        }
        let database = backfill_database();
        let saved = SentBackfillProgress { page: Some("p3".into()), offset: 7, scanned: 200 };
        database.record_sent_backfill(BACKFILL_ACCOUNT, Some(&saved)).unwrap();

        assert!(backfill_sent_step(&database, BACKFILL_ACCOUNT, &FlakySearch).await.is_err());
        assert_eq!(database.sent_backfill_progress(BACKFILL_ACCOUNT).unwrap(), Some(saved));
    }
}
