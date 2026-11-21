//! Custom keywords are additive per-copy flags, independent of mailbox moves.

use std::collections::{BTreeMap, BTreeSet};

use super::{db_err, mutate::as_write_session, ImapProvider, INBOX};
use crate::models::Label;
use crate::provider::imap::{
    labels::keyword_label,
    settings::LabelStorage,
    write::{select_for_write, store_and_fetch_flags, StoreOp, WriteCaps},
    SyncRoundWrite,
};
use crate::provider::{ProviderError, ProviderResult};

impl ImapProvider {
    pub(super) async fn create_keyword_label(&self, name: &str) -> ProviderResult<Label> {
        let label = keyword_label(name.trim()).ok_or_else(|| {
            ProviderError::InvalidOperation(
                "Use an IMAP keyword with no spaces, non-ASCII characters, or reserved punctuation.".into(),
            )
        })?;
        let mut session = self.open_plain_session().await?;
        let result = async {
            let writer = as_write_session(session.as_mut())?;
            let status = writer.select(INBOX).await?;
            let caps = WriteCaps::derive(&writer.capabilities().await?, &status);
            if !caps.can_store_flag(&label.name) {
                return Err(ProviderError::InvalidOperation(
                    "This server does not permit this keyword in Inbox.".into(),
                ));
            }
            // IMAP has no account-wide CREATE for keywords. Remember the
            // declaration locally so it is available before any message uses it.
            self.store
                .database()
                .upsert_account_label(self.store.account_id(), &label)
                .map_err(db_err)?;
            Ok(label)
        }
        .await;
        let _ = session.logout().await;
        result
    }

    pub(super) async fn apply_keyword(
        &self,
        ids: &[String],
        keyword: &str,
        add: bool,
    ) -> ProviderResult<()> {
        if self.settings.label_storage != LabelStorage::Keywords || keyword_label(keyword).is_none()
        {
            return Err(ProviderError::InvalidOperation(
                "This account cannot apply this IMAP keyword.".into(),
            ));
        }
        let mut mailboxes = BTreeMap::new();
        let mut unique_ids = BTreeSet::new();
        for id in ids.iter().filter(|id| unique_ids.insert(id.as_str())) {
            let locations = self.store.locations_for_message(id).map_err(db_err)?;
            if locations.is_empty() {
                return Err(ProviderError::InvalidOperation(
                    "A message no longer has a synced copy to label.".into(),
                ));
            }
            for location in locations {
                mailboxes
                    .entry((location.mailbox.clone(), location.uidvalidity))
                    .or_insert_with(Vec::new)
                    .push(location);
            }
        }
        let mut session = self.open_plain_session().await?;
        let result = async {
            let writer = as_write_session(session.as_mut())?;
            // Validate every mailbox before starting a multi-mailbox edit.
            for (mailbox, validity) in mailboxes.keys() {
                let ctx = select_for_write(writer, &self.store, mailbox, *validity as u32).await?;
                if !ctx.caps.can_store_flag(keyword) {
                    return Err(ProviderError::InvalidOperation(format!(
                        "The server does not permit keyword {keyword:?} in {mailbox:?}."
                    )));
                }
            }
            for ((mailbox, validity), locations) in mailboxes {
                let ctx = select_for_write(writer, &self.store, &mailbox, validity as u32).await?;
                let uids: Vec<_> = locations.iter().map(|location| location.uid as u32).collect();
                let op = if add { StoreOp::Add } else { StoreOp::Remove };
                let flags = store_and_fetch_flags(writer, &ctx, &uids, op, &[keyword.into()]).await?;
                let mut round = SyncRoundWrite::default();
                for mut location in locations {
                    let actual = flags.get(&(location.uid as u32)).ok_or_else(|| {
                        ProviderError::TransientTransport(
                            "A message disappeared while verifying the keyword. Refresh the account.".into(),
                        )
                    })?;
                    let flags_json = serde_json::to_string(actual)
                        .map_err(|error| ProviderError::Other(error.to_string()))?;
                    if location.flags_json != flags_json {
                        location.flags_json = flags_json;
                        if let Some(thread) = self.store.thread_of_message(&location.message_id).map_err(db_err)? {
                            round.changed_threads.push(thread.clone());
                            round.hot_threads.push(thread);
                        }
                        round.locations.push(location);
                    }
                }
                self.store.commit_sync_round(&round).map_err(db_err)?;
            }
            Ok(())
        }.await;
        let _ = session.logout().await;
        result
    }
}
