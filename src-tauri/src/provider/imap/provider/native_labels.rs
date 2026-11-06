//! Existing user-label membership, addressed by authoritative server identity.
//! Removing a label never deletes a physical copy or replaces the label set.
use super::{db_err, mutate::as_write_session, ImapProvider};
use crate::provider::imap::{
    write::{select_for_write, StoreOp},
    ImapLocation, MailboxRole, SyncRoundWrite,
};
use crate::provider::{ProviderError, ProviderResult};

impl ImapProvider {
    pub(super) async fn apply_native_label(
        &self,
        ids: &[String],
        label_id: &str,
        add: bool,
    ) -> ProviderResult<()> {
        let invalid = |message: &str| ProviderError::InvalidOperation(message.into());
        let plan = self.build_sync_plan()?;
        let catalog = self.store.mailboxes().map_err(db_err)?;
        // Resolve through the exact catalog ID, preserving existing folder:/lf:
        // references. An arbitrary caller string must never create a new label.
        let label = plan
            .entries
            .iter()
            .find(|entry| {
                entry.location_label_id.as_deref() == Some(label_id)
                    && !entry.is_inbox
                    && entry.role.is_none()
                    && catalog
                        .iter()
                        .any(|row| row.name == entry.mailbox && row.special_use.is_none())
            })
            .ok_or_else(|| {
                invalid("This user label is no longer in the account catalog; refresh labels first")
            })?;
        if ids.is_empty() {
            return Err(invalid("No messages were supplied for this label change"));
        }
        let targets = ids
            .iter()
            .map(|id| {
                self.store
                    .gmail_message_id(id)
                    .map_err(db_err)?
                    .map(|server_id| (id, server_id))
                    .ok_or_else(|| {
                        invalid("Refresh this message before changing its native labels")
                    })
            })
            .collect::<ProviderResult<Vec<_>>>()?;
        let mut session = self.open_session().await?;
        let result = async {
            if !session.supports_label_extensions() {
                return Err(invalid("The server no longer supports native label operations"));
            }
            // In particular, a renamed/deleted label must not be recreated by
            // STORE against an old catalog entry.
            session.examine(&label.mailbox).await?;
            for (id, server_id) in targets {
                let old = self.store.locations_for_message(id).map_err(db_err)?;
                // Prefer an aggregate/system mailbox. Never remove the label
                // whose own UID is being used for subsequent verification.
                let sources = plan.entries.iter().filter(|entry| entry.role == Some(MailboxRole::All))
                    .map(|entry| entry.mailbox.clone())
                    .chain(old.iter().filter(|location| location.mailbox != label.mailbox || add)
                        .map(|location| location.mailbox.clone()));
                let mut visited = std::collections::BTreeSet::new();
                let mut source = None;
                for mailbox in sources {
                    if !visited.insert(mailbox.clone()) { continue; }
                    if let Some((validity, row)) = self.label_location(session.as_mut(), &mailbox, server_id, id).await? {
                        source = Some((mailbox, validity, row));
                        break;
                    }
                }
                let (mailbox, validity, before) = source.ok_or_else(|| invalid(
                    "No independent live message location is available; refresh this message before changing its labels"))?;
                let original = before.labels.as_ref().expect("label_location validates membership");
                if original.contains(&label.mailbox) != add {
                    let writer = as_write_session(session.as_mut())?;
                    select_for_write(writer, &self.store, &mailbox, validity as u32).await?;
                    writer.uid_store_labels(&before.uid.to_string(), if add { StoreOp::Add } else { StoreOp::Remove },
                        std::slice::from_ref(&label.mailbox)).await?;
                }
                let (verified_validity, verified) = self.label_location(session.as_mut(), &mailbox, server_id, id).await?
                    .ok_or_else(|| ProviderError::TransientTransport("Message vanished while verifying its label change".into()))?;
                let labels = verified.labels.as_ref().expect("label_location validates membership");
                if verified_validity != validity || labels.contains(&label.mailbox) != add
                    || original.iter().filter(|value| *value != &label.mailbox).any(|value| !labels.contains(value)) {
                    return Err(ProviderError::TransientTransport("The server did not preserve the requested message labels".into()));
                }
                let mut round = SyncRoundWrite {
                    label_snapshots: vec![(id.clone(), labels.clone())],
                    locations: vec![ImapLocation {
                        mailbox: mailbox.clone(), uidvalidity: validity, uid: verified.uid as i64,
                        message_id: id.clone(), flags_json: serde_json::to_string(&verified.flags)
                            .map_err(|error| ProviderError::Other(error.to_string()))?, modseq: None,
                    }],
                    ..Default::default()
                };
                for location in old {
                    if (!add && location.mailbox == label.mailbox)
                        || (location.mailbox == mailbox && (location.uidvalidity != validity || location.uid != verified.uid as i64)) {
                        round.deletions.push((location.mailbox, location.uidvalidity, location.uid));
                    }
                }
                if let Some(thread) = self.store.thread_of_message(id).map_err(db_err)? {
                    round.changed_threads.push(thread.clone());
                    round.hot_threads.push(thread);
                }
                // Per-message verified commits make partial batches and lost
                // STORE replies safe to retry without repeating destructive work.
                self.store.commit_sync_round(&round).map_err(db_err)?;
            }
            Ok(())
        }.await;
        let _ = session.logout().await;
        result
    }
}
