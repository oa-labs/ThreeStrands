//! Pushes local drafts to the account's server drafts
//! (`docs/server-drafts-design.md`). The local `drafts` table stays the
//! composer's source of truth; this module copies each saved revision to the
//! server after a pause and removes the server copy once the draft has been
//! sent or discarded.
//!
//! Every server copy has a `Message-ID` derived from the draft id, so any
//! device holding the draft finds the same copy and updates it rather than
//! creating another one.
use crate::clock::unix_millis;
use std::{collections::HashMap, sync::Arc, time::Duration};

use serde::Serialize;

use crate::{
    correspondence::{build_draft_mime, Correspondence, Draft},
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

/// Whether drafts on accounts of this kind are kept on the server.
pub(crate) fn kind_keeps_server_drafts(kind: MailProviderKind) -> bool {
    matches!(kind, MailProviderKind::Gmail | MailProviderKind::Imap)
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

pub(crate) struct Deletion {
    pub(crate) id: i64,
    pub(crate) account: String,
    pub(crate) server_id: Option<String>,
    pub(crate) message_id: String,
    pub(crate) attempts: u32,
}

fn retry_at(attempts: u32, at: i64) -> i64 {
    at + crate::backoff::retry_delay_secs(attempts.max(1)) * 1_000
}


pub(crate) enum RemovedDraft {
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
            if let Err(error) = self.run_draft_round(unix_millis(), &mut last_pulled).await {
                log::warn!(target: "drafts", "server draft push failed: {error}");
            }
            tokio::time::sleep(PUSH_POLL_INTERVAL).await;
        }
    }

    /// Pushes every pending draft now, waiting at most [`EXIT_PUSH_TIMEOUT`].
    /// Drafts that don't make it stay saved locally and push on next launch.
    pub async fn push_drafts_before_exit(&self) {
        let at = unix_millis();
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
                RemovedDraft::Delete => self.database.retire_draft_link(link, unix_millis())?,
                RemovedDraft::Wait => self
                    .database
                    .defer_draft_link(&link.draft_id, unix_millis() + SEND_PENDING_RECHECK_MS)?,
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
                self.database.record_draft_push_failure(&draft.id, "retrying", &message, Some(retry_at(attempts, unix_millis())), attempts)?;
                self.notify_draft(&draft, "retrying", Some(message));
                return Ok(());
            }
        };
        if !provider.server_drafts() {
            // No server drafts on this account, such as an IMAP account
            // without a Drafts mailbox: the draft stays on this device.
            self.database.forget_server_draft_link(&draft.id)?;
            self.notify_draft(&draft, "local", None);
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
                let due = retries.then(|| retry_at(attempts, unix_millis()));
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
        // The copy this device last saw is gone. On a server where every save
        // is a new copy, that can mean another device or app replaced it.
        let mut replaced = false;
        if let (Some(id), Some(seen)) = (&server_id, &link.server_version) {
            match provider.server_draft_version(id).await.map_err(PushError::Provider)? {
                None => {
                    server_id = None;
                    replaced = true;
                }
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
            .message_id_domain(draft.sender_address())
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
            None => {
                let found = provider
                    .find_server_draft(&server_message_id(&draft.id, domain.as_deref()))
                    .await
                    .map_err(PushError::Provider)?;
                if let Some(found) = found.as_ref().filter(|_| replaced) {
                    self.hold_server_conflict(provider, draft, &found.id).await.map_err(PushError::Provider)?;
                    return Ok(None);
                }
                found.map(|found| found.id)
            }
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
        let retry = |attempts: u32| self.database.defer_draft_deletion(deletion.id, attempts, retry_at(attempts, unix_millis()));
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
        db::Database,
        mime::RawMessage,
        models::Label,
        provider::{
            Delivery, DeliveryReceipt, LabelModel, MailFetch, MailMutate, MailSend, MailSync,
            ProviderCapabilities, ProviderResult, SyncBatch, SyncCursor, ThreadPage,
        },
    };
    use rusqlite::params;

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
            self.service.push_due_drafts_with(unix_millis() + delay_ms, &self.providers()).await.unwrap();
        }
        /// Pulls the account's server drafts.
        pub(crate) async fn pull(&self) -> bool {
            self.service.pull_server_drafts(ACCOUNT, self.server.as_ref()).await.unwrap()
        }
        async fn run_disconnected(&self, delay_ms: i64) {
            self.service.push_due_drafts_with(unix_millis() + delay_ms, &HashMap::new()).await.unwrap();
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
                .test_connection()
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
            .test_connection()
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
            .test_connection()
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
        harness.db().push_drafts_now(Some(&draft.id), unix_millis()).unwrap();
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
        assert!(backoff >= unix_millis() + crate::backoff::retry_delay_secs(1) * 1_000 - 1_000);

        let draft = harness.edit(draft, "Two");
        assert_eq!(harness.link_due(&draft), Some(backoff), "editing must not skip the backoff");
        assert_eq!(harness.state(&draft), "retrying");

        harness.run_after(backoff - unix_millis() + 1).await;
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
    async fn imap_accounts_keep_server_drafts_too() {
        let harness = Harness::new();
        harness
            .db()
            .test_connection()
            .unwrap()
            .execute("UPDATE accounts SET provider='imap' WHERE email=?1", [ACCOUNT])
            .unwrap();
        let draft = harness.edit(harness.new_draft(), "Saved to Drafts");
        assert_eq!(harness.state(&draft), "pending");
        harness.run_after(PAST_DELAY).await;
        assert!(harness.only_server_body().contains("Saved to Drafts"));
    }

    #[tokio::test]
    async fn drafts_on_an_account_set_up_on_another_device_stay_on_this_one() {
        let harness = Harness::new();
        harness
            .db()
            .test_connection()
            .unwrap()
            .execute("UPDATE accounts SET on_this_device=0 WHERE email=?1", [ACCOUNT])
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
        let events = harness.events.lock().unwrap().clone();
        assert!(
            matches!(events.last(), Some(DraftEvent::Status(event)) if event.status.state == "local"),
            "the composer learns the draft stays here"
        );
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
    async fn discarding_deletes_the_server_copy_once_undo_has_passed() {
        let harness = Harness::new();
        let draft = harness.edit(harness.new_draft(), "One");
        harness.run_after(PAST_DELAY).await;
        harness.db().discard_draft(&draft.id).unwrap();
        harness.run_after(crate::correspondence::DISCARD_SERVER_DELAY_MS - 1_000).await;
        assert_eq!(harness.server.drafts().len(), 1, "Undo can still leave the server untouched");
        harness.run_after(crate::correspondence::DISCARD_SERVER_DELAY_MS + 1).await;
        harness.run_after(crate::correspondence::DISCARD_SERVER_DELAY_MS + 1).await;
        assert!(harness.server.drafts().is_empty());
        assert_eq!(harness.server.calls().last().unwrap(), "delete server-1");
    }

    #[tokio::test]
    async fn undoing_a_discard_within_the_window_leaves_the_server_copy_alone() {
        let harness = Harness::new();
        let draft = harness.edit(harness.new_draft(), "One");
        harness.run_after(PAST_DELAY).await;
        harness.db().discard_draft(&draft.id).unwrap();
        assert!(harness.db().draft(&draft.id).is_err());
        let restored = harness.db().restore_discarded_draft(&draft.id).unwrap();
        assert!(restored.body.contains("One"));
        assert!(restored.revision > draft.revision);
        assert_eq!(harness.state(&restored), "saved");
        harness.run_after(crate::correspondence::DISCARD_SERVER_DELAY_MS * 2).await;
        assert_eq!(harness.server.drafts().len(), 1);
        assert!(!harness.server.calls().iter().any(|call| call.starts_with("delete") || call.starts_with("update")));
        assert!(harness.db().restore_discarded_draft(&draft.id).is_err(), "only once");
    }

    #[tokio::test]
    async fn undoing_a_discard_after_the_copy_was_deleted_saves_it_again() {
        let harness = Harness::new();
        let draft = harness.edit(harness.new_draft(), "One");
        harness.run_after(PAST_DELAY).await;
        harness.db().discard_draft(&draft.id).unwrap();
        let late = crate::correspondence::DISCARD_SERVER_DELAY_MS + 1;
        harness.run_after(late).await;
        harness.run_after(late).await;
        assert!(harness.server.drafts().is_empty());
        let restored = harness.db().restore_discarded_draft(&draft.id).unwrap();
        assert_eq!(harness.state(&restored), "pending");
        harness.run_after(PAST_DELAY).await;
        assert!(harness.only_server_body().contains("One"));
    }

    #[tokio::test]
    async fn undoing_a_discard_before_its_deletion_ran_cancels_the_deletion() {
        let harness = Harness::new();
        let draft = harness.edit(harness.new_draft(), "One");
        harness.run_after(PAST_DELAY).await;
        harness.db().discard_draft(&draft.id).unwrap();
        // The link was retired into a queued deletion that hasn't run yet.
        let late = crate::correspondence::DISCARD_SERVER_DELAY_MS + 1;
        harness.service.database.test_connection().unwrap()
            .execute("UPDATE draft_server_deletions SET due_at=?1", [unix_millis() + 60 * 60 * 1000]).unwrap();
        harness.run_after(late).await;
        harness.db().restore_discarded_draft(&draft.id).unwrap();
        harness.run_after(60 * 60 * 1000 + 1).await;
        assert_eq!(harness.server.drafts().len(), 1, "the restored draft keeps a server copy");
    }

    #[tokio::test]
    async fn discarding_before_the_first_push_still_finds_and_deletes_a_copy() {
        let harness = Harness::new();
        let draft = harness.edit(harness.new_draft(), "One");
        harness.db().discard_draft(&draft.id).unwrap();
        let late = crate::correspondence::DISCARD_SERVER_DELAY_MS + 1;
        harness.run_after(late).await;
        harness.run_after(late).await;
        assert!(harness.server.drafts().is_empty());
        assert!(!harness.server.calls().contains(&"create".to_string()));
    }

    #[test]
    fn a_discarded_draft_keeps_its_files_until_it_can_no_longer_be_restored() {
        let harness = Harness::new();
        let mut draft = harness.edit(harness.new_draft(), "With a file");
        let file = uuid::Uuid::new_v4().to_string();
        std::fs::write(harness.root.0.join(&file), b"notes").unwrap();
        draft.attachments.push(crate::correspondence::Attachment {
            id: file.clone(), name: "notes.txt".into(), size: 5, mime: "text/plain".into(), ready: true,
            message_id: None, provider_id: None, inline: false, content_id: None,
        });
        let payload = serde_json::to_string(&draft).unwrap();
        harness.db().test_connection().unwrap().execute("UPDATE drafts SET payload=?2 WHERE id=?1", params![draft.id, payload]).unwrap();
        harness.db().discard_draft(&draft.id).unwrap();
        harness.service.cleanup().unwrap();
        assert!(harness.root.0.join(&file).exists(), "Undo brings the file back too");

        let expired = unix_millis() - crate::correspondence::DISCARDED_DRAFT_RETENTION_MS - 1;
        harness.db().test_connection().unwrap().execute("UPDATE discarded_drafts SET discarded_at=?1", [expired]).unwrap();
        harness.service.cleanup().unwrap();
        assert!(!harness.root.0.join(&file).exists());
        assert!(harness.db().restore_discarded_draft(&draft.id).is_err());
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
            .test_connection()
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
            .test_connection()
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
            .test_connection()
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
        harness.db().push_drafts_now(None, unix_millis()).unwrap();
        assert!(harness.link_due(&draft).unwrap() <= unix_millis());
    }

    #[test]
    fn saved_drafts_never_store_their_server_status() {
        let database = Database::open_memory();
        database.adopt_account(ACCOUNT).unwrap();
        let mut draft = database.create_draft("new", None, ACCOUNT, &[]).unwrap();
        draft.body = "Body".into();
        let saved = database.save_draft(draft).unwrap();
        let payload: String = database
            .test_connection()
            .unwrap()
            .query_row("SELECT payload FROM drafts WHERE id=?1", [&saved.id], |row| row.get(0))
            .unwrap();
        assert!(!payload.contains("serverDraft"));
    }
}
