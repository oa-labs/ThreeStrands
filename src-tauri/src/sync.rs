use std::{collections::HashSet, sync::Arc, time::Duration};

use tokio::sync::Mutex;

use crate::{
    auth::GoogleAuth,
    db::{Database, PendingMutation},
    gmail::{GmailClient, GmailProvider, ProviderError, ProviderResult},
    mime::normalize,
    models::{Label, SyncStatus, ThreadMutation},
};

#[derive(Clone)]
pub struct SyncService {
    database: Arc<Database>,
    auth: GoogleAuth,
    gate: Arc<Mutex<()>>,
}

impl SyncService {
    pub fn new(database: Arc<Database>, auth: GoogleAuth) -> Self {
        Self {
            database,
            auth,
            gate: Arc::new(Mutex::new(())),
        }
    }

    pub async fn sync(&self) -> Result<SyncStatus, String> {
        let _guard = self.gate.lock().await;
        let provider = GmailClient::new(self.auth.clone());
        if let Err(error) = sync_with(self.database.as_ref(), &provider).await {
            let message = error.to_string();
            self.database.fail_sync(&message)?;
            return Err(message);
        }
        self.database.sync_status()
    }

    pub async fn flush_pending(&self) -> Result<SyncStatus, String> {
        if self.database.sync_status()?.pending_mutations == 0 {
            return self.database.sync_status();
        }
        if !GoogleAuth::available() {
            return self.database.sync_status();
        }
        let _guard = self.gate.lock().await;
        let provider = GmailClient::new(self.auth.clone());
        if let Err(error) = flush_pending_with(self.database.as_ref(), &provider).await {
            let message = error.to_string();
            self.database.fail_sync(&message)?;
            return Err(message);
        }
        self.database.sync_status()
    }

    pub async fn labels(&self) -> Result<Vec<Label>, String> {
        GmailClient::new(self.auth.clone())
            .list_labels()
            .await
            .map_err(|error| error.to_string())
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
        let mut delay = Duration::from_secs(15);
        loop {
            tokio::time::sleep(delay).await;
            if !GoogleAuth::available() {
                delay = Duration::from_secs(30);
                continue;
            }
            let before = self
                .database
                .sync_status()
                .map(|status| status.pending_mutations)
                .unwrap_or_default();
            delay = match self.sync().await {
                Ok(status) if before > 0 || status.pending_mutations > 0 => Duration::from_secs(15),
                Ok(_) => (delay * 2).min(Duration::from_secs(300)),
                Err(error) if error.contains("rate limit") => Duration::from_secs(300),
                Err(_) => Duration::from_secs(60),
            };
        }
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
    provider: &(impl GmailProvider + ?Sized),
) -> ProviderResult<()> {
    if database
        .sync_status()
        .map_err(ProviderError::Other)?
        .pending_mutations
        == 0
    {
        return Ok(());
    }
    deliver_mutations(database, provider).await
}

pub async fn sync_with(
    database: &Database,
    provider: &(impl GmailProvider + ?Sized),
) -> ProviderResult<()> {
    deliver_mutations(database, provider).await?;
    match database.cursor().map_err(ProviderError::Other)? {
        Some(cursor) => match incremental_sync(database, provider, &cursor).await {
            Err(ProviderError::InvalidCursor) => full_sync(database, provider).await,
            result => result,
        },
        None => full_sync(database, provider).await,
    }
}

async fn full_sync(
    database: &Database,
    provider: &(impl GmailProvider + ?Sized),
) -> ProviderResult<()> {
    match full_sync_attempt(database, provider).await {
        Err(ProviderError::InvalidCursor) => full_sync_attempt(database, provider).await,
        result => result,
    }
}

async fn full_sync_attempt(
    database: &Database,
    provider: &(impl GmailProvider + ?Sized),
) -> ProviderResult<()> {
    // Capture the cursor before listing. The following history pass closes the race
    // with mail arriving while the potentially long initial list is downloaded.
    let starting_cursor = provider.profile_history_id().await?;
    database.begin_full_sync().map_err(ProviderError::Other)?;
    let mut page = None;
    loop {
        let result = provider.list_threads(page.as_deref()).await?;
        for id in result.thread_ids {
            ingest_thread(database, provider, &id).await?;
        }
        page = result.next_page_token;
        if page.is_none() {
            break;
        }
    }
    incremental_sync(database, provider, &starting_cursor).await
}

async fn incremental_sync(
    database: &Database,
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
    for id in changed {
        ingest_thread(database, provider, &id).await?;
    }
    database
        .finish_sync(&final_cursor)
        .map_err(ProviderError::Other)
}

async fn ingest_thread(
    database: &Database,
    provider: &(impl GmailProvider + ?Sized),
    id: &str,
) -> ProviderResult<()> {
    let messages = match provider.get_thread(id).await {
        Ok(messages) => messages,
        Err(ProviderError::NotFound) => {
            return database
                .delete_gmail_thread(id)
                .map_err(ProviderError::Other)
        }
        Err(error) => return Err(error),
    };
    let normalized = messages
        .iter()
        .map(normalize)
        .collect::<Result<Vec<_>, _>>()
        .map_err(ProviderError::Other)?;
    database
        .upsert_gmail_thread(&normalized)
        .map_err(ProviderError::Other)
}

async fn deliver_mutations(
    database: &Database,
    provider: &(impl GmailProvider + ?Sized),
) -> ProviderResult<()> {
    loop {
        let mutations = database.claim_mutations(50).map_err(ProviderError::Other)?;
        if mutations.is_empty() {
            return Ok(());
        }
        for mutation in mutations {
            let (add, remove) = mutation_labels(&mutation);
            match provider
                .modify_thread(&mutation.provider_thread_id, &add, &remove)
                .await
            {
                Ok(()) => database
                    .complete_mutation(&mutation.id)
                    .map_err(ProviderError::Other)?,
                Err(error @ ProviderError::RateLimited) => {
                    database
                        .reject_mutation(&mutation.id, &error.to_string(), true)
                        .map_err(ProviderError::Other)?;
                    return Err(error);
                }
                Err(error) => {
                    database
                        .reject_mutation(&mutation.id, &error.to_string(), false)
                        .map_err(ProviderError::Other)?;
                }
            }
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
    let (label, value) = match &mutation.mutation {
        ThreadMutation::Archive { value, .. } => ("INBOX", !value),
        ThreadMutation::Trash { .. } => unreachable!(),
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
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

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
        fail_mutation: bool,
    }

    impl ContractProvider {
        fn normal() -> Self {
            Self {
                invalidate_stale_cursor: AtomicBool::new(false),
                full_lists: AtomicUsize::new(0),
                modifies: AtomicUsize::new(0),
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

    #[tokio::test]
    async fn provider_contract_imports_and_advances_cursor() {
        let database = Database::open_memory();
        let provider = ContractProvider::normal();
        sync_with(&database, &provider).await.unwrap();
        assert_eq!(database.cursor().unwrap().as_deref(), Some("current"));
        assert_eq!(provider.full_lists.load(Ordering::SeqCst), 1);
        assert_eq!(database.list_threads().unwrap()[0].id, "gmail-thread");
    }

    #[tokio::test]
    async fn invalid_history_cursor_recovers_with_full_resync() {
        let database = Database::open_memory();
        database.finish_sync("stale").unwrap();
        let provider = ContractProvider {
            invalidate_stale_cursor: AtomicBool::new(true),
            ..ContractProvider::normal()
        };
        sync_with(&database, &provider).await.unwrap();
        assert_eq!(provider.full_lists.load(Ordering::SeqCst), 1);
        assert_eq!(database.cursor().unwrap().as_deref(), Some("current"));
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
            sync_with(&database, &provider).await,
            Err(ProviderError::RateLimited)
        ));
        assert_eq!(database.sync_status().unwrap().pending_mutations, 1);
    }

    #[test]
    fn trashing_adds_trash_and_removes_inbox() {
        let mutation = PendingMutation {
            id: "m1".into(),
            provider_thread_id: "gmail-thread".into(),
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

    #[tokio::test]
    async fn inactivity_flush_skips_provider_when_nothing_is_pending() {
        let database = Database::open_memory();
        let provider = ContractProvider::normal();
        flush_pending_with(&database, &provider).await.unwrap();
        assert_eq!(provider.modifies.load(Ordering::SeqCst), 0);
        assert_eq!(provider.full_lists.load(Ordering::SeqCst), 0);
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
        flush_pending_with(&database, &provider).await.unwrap();
        assert_eq!(provider.modifies.load(Ordering::SeqCst), 1);
        assert_eq!(provider.full_lists.load(Ordering::SeqCst), 0);
        assert_eq!(database.sync_status().unwrap().pending_mutations, 0);
    }
}
