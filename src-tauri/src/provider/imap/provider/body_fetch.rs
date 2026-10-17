//! Fetching message bodies and attachment bytes ([`MailFetch`]), from the body
//! cache or, on a miss, the server.

use async_trait::async_trait;

use crate::mime::RawMessage;
use crate::provider::{MailFetch, ProviderError, ProviderResult};

use crate::provider::imap::fetch::SessionBodyFetcher;
use crate::provider::imap::labels::{labels_for_location_ids, merge_label_sets};
use crate::provider::imap::rfc822::{attachment_bytes_from_raw, to_raw_message};
use crate::provider::imap::session::ImapSession;

use super::{db_err, ImapProvider, INBOX};

impl ImapProvider {
    /// Fetch a single message's body by locating one of its UIDs and running a
    /// bounded `BODY.PEEK[]`. Used by `fetch_thread`/`fetch_message` on a cache
    /// miss. The body is cached for next time.
    pub(super) async fn fetch_body_from_server(
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
                Some(crate::provider::imap::MailboxRole::Sent)
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
        use crate::provider::imap::fetch::BodyFetcher;
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
