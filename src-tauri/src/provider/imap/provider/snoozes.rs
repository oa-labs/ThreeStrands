//! Reacquire snoozed conversations after a location cache rebuild or device join.
use super::{db_err, ImapProvider};
use crate::provider::imap::policy;
use crate::provider::{ProviderError, ProviderResult};

impl ImapProvider {
    pub(super) async fn recover_snoozed_thread(
        &self,
        id: &str,
        references: &[String],
    ) -> ProviderResult<()> {
        tokio::time::timeout(
            policy::SEARCH_DEADLINE,
            self.recover_snoozed_thread_with(id, references),
        )
        .await
        .map_err(|_| {
            ProviderError::TransientTransport(
                "Snoozed mail recovery timed out; its return remains pending".into(),
            )
        })?
    }

    async fn recover_snoozed_thread_with(
        &self,
        id: &str,
        references: &[String],
    ) -> ProviderResult<()> {
        let resolved = self.store.resolve_thread_alias(id).map_err(db_err)?;
        for message in self.store.messages_in_thread(&resolved).map_err(db_err)? {
            if !self
                .store
                .locations_for_message(&message)
                .map_err(db_err)?
                .is_empty()
            {
                return Ok(());
            }
        }
        // UID locations can be absent on a newly joined device or after a
        // mailbox reset. Search only reference identifiers, then acquire via
        // the existing bounded, UIDVALIDITY-checked search path. Never assume
        // a cache miss means the provider deleted the email.
        let plan = self.build_sync_plan()?;
        let mut session = self.open_session().await?;
        let native_id = if session.supports_label_extensions() {
            self.store
                .gmail_thread_lookup_id(&resolved)
                .map_err(db_err)?
        } else {
            None
        };
        let queries = if let Some(thread) = native_id {
            vec![format!("X-GM-THRID {thread}")]
        } else {
            references
                .iter()
                .map(|reference| {
                    if reference.len() > 2048
                        || !reference.is_ascii()
                        || reference.chars().any(char::is_control)
                    {
                        return Err(ProviderError::InvalidOperation(
                            "Invalid snooze message reference".into(),
                        ));
                    }
                    Ok(format!(
                        "HEADER Message-ID {}",
                        super::super::search::quote_search_string(reference)
                    ))
                })
                .collect::<ProviderResult<Vec<_>>>()?
        };
        for entry in plan.entries.iter().filter(|entry| entry.synced_now) {
            let status = session.examine(&entry.mailbox).await?;
            let validity = status.uid_validity.unwrap_or(0) as i64;
            for query in &queries {
                let mut uids = session.uid_search(query).await?;
                if let Some(thread) = native_id {
                    if uids.len() > policy::MAX_SEARCH_ACQUIRE {
                        return Err(ProviderError::TransientTransport(
                            "Snoozed mail recovery reached the account's search limit".into(),
                        ));
                    }
                    let verified = session.uid_gmail_threads(&uids).await?;
                    uids.retain(|uid| verified.get(uid).is_some_and(|(_, id)| *id == thread));
                }
                if uids.is_empty() {
                    continue;
                }
                let acquired = self
                    .acquire_uids(session.as_mut(), entry, &uids, validity)
                    .await
                    .map_err(|error| ProviderError::TransientTransport(error.to_string()))?;
                if acquired.skipped_over_cap > 0 {
                    return Err(ProviderError::TransientTransport(
                        "Snoozed mail recovery reached the account's search limit".into(),
                    ));
                }
            }
        }
        let _ = session.logout().await;
        let resolved = self.store.resolve_thread_alias(id).map_err(db_err)?;
        for message in self.store.messages_in_thread(&resolved).map_err(db_err)? {
            if !self
                .store
                .locations_for_message(&message)
                .map_err(db_err)?
                .is_empty()
            {
                return Ok(());
            }
        }
        Err(ProviderError::TransientTransport(
            "Snoozed conversation could not be located on the server; its return remains pending"
                .into(),
        ))
    }
}
