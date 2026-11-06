//! System actions on negotiated label-based mailboxes. Never use \Deleted,
//! EXPUNGE, MOVE, or a replacement label set: only the requested memberships
//! change. Verify through a live destination UID since the source can vanish.
use super::{db_err, moves::SystemMoveOp, mutate::as_write_session, ImapProvider};
use crate::provider::imap::{
    fetch::{fetch_identity, IdentityRow},
    plan::SyncPlan,
    session::ImapSession,
    write::{select_for_write, StoreOp},
    ImapLocation, MailboxRole, SyncRoundWrite,
};
use crate::provider::{ProviderError, ProviderResult};

fn has(labels: &[String], label: &str) -> bool {
    labels.iter().any(|value| value.eq_ignore_ascii_case(label))
}

fn membership(row: &IdentityRow) -> ProviderResult<&Vec<String>> {
    row.labels.as_ref().ok_or_else(|| {
        ProviderError::TransientTransport("Server omitted message label membership".into())
    })
}

impl ImapProvider {
    /// Find exactly one live coordinate by the authoritative server ID. A
    /// sender Message-ID or a stale cached UID must never address this write.
    async fn label_location(
        &self,
        session: &mut dyn ImapSession,
        mailbox: &str,
        server_id: u64,
        expected: &str,
    ) -> ProviderResult<Option<(i64, IdentityRow)>> {
        let status = session.examine(mailbox).await?;
        let validity = status
            .uid_validity
            .ok_or_else(|| ProviderError::TransientTransport("Mailbox has no UIDVALIDITY".into()))?
            as i64;
        let uids = session
            .uid_search(&format!("X-GM-MSGID {server_id}"))
            .await?;
        if uids.is_empty() {
            return Ok(None);
        }
        if uids.len() != 1 {
            return Err(ProviderError::InvalidOperation(
                "Conflicting message identities in destination mailbox".into(),
            ));
        }
        let rows = fetch_identity(session, &uids[0].to_string()).await?;
        let mut matching = rows
            .into_iter()
            .filter(|row| row.uid == uids[0] && row.inputs.gmail_message_id == Some(server_id));
        let row = matching
            .next()
            .filter(|_| matching.next().is_none())
            .ok_or_else(|| {
                ProviderError::TransientTransport(
                    "Message changed while locating its server identity".into(),
                )
            })?;
        if row.labels.is_none() {
            return Err(ProviderError::TransientTransport(
                "Server omitted message label membership".into(),
            ));
        }
        let resolved = self
            .store
            .resolve_message_identity(mailbox, validity, row.uid as i64, &row.inputs)
            .map_err(db_err)?;
        if resolved != expected {
            return Err(ProviderError::InvalidOperation(
                "Message identity does not match this action".into(),
            ));
        }
        Ok(Some((validity, row)))
    }

    pub(super) async fn apply_label_move(
        &self,
        session: &mut dyn ImapSession,
        message_ids: &[String],
        plan: &SyncPlan,
        op: SystemMoveOp,
    ) -> ProviderResult<()> {
        if !session.supports_label_extensions() {
            session.capabilities().await?;
        }
        if !session.supports_label_extensions() {
            return Err(ProviderError::InvalidOperation(
                "The server no longer supports message label operations".into(),
            ));
        }
        let all = self
            .mailbox_with_role(plan, MailboxRole::All)
            .ok_or_else(|| {
                ProviderError::InvalidOperation(
                    "All Mail must be available for label-based archive actions".into(),
                )
            })?;
        let (destination, add, remove): (String, Option<&str>, &[&str]) = match op {
            SystemMoveOp::Archive => (all.clone(), None, &["\\Inbox"]),
            SystemMoveOp::Unarchive | SystemMoveOp::Untrash | SystemMoveOp::NotSpam => (
                plan.entries
                    .iter()
                    .find(|e| e.is_inbox)
                    .map(|e| e.mailbox.clone())
                    .unwrap_or_else(|| "INBOX".into()),
                Some("\\Inbox"),
                &["\\Trash", "\\Spam"],
            ),
            SystemMoveOp::Trash => (
                self.mailbox_with_role(plan, MailboxRole::Trash)
                    .ok_or_else(|| {
                        ProviderError::InvalidOperation("No Trash mailbox is configured".into())
                    })?,
                Some("\\Trash"),
                &["\\Inbox", "\\Spam"],
            ),
            SystemMoveOp::Spam => (
                self.mailbox_with_role(plan, MailboxRole::Junk)
                    .ok_or_else(|| {
                        ProviderError::InvalidOperation("No Junk mailbox is configured".into())
                    })?,
                Some("\\Spam"),
                &["\\Inbox", "\\Trash"],
            ),
        };
        // Check the destination before the first write. No implicit creation of
        // an Archive folder, Trash, or Junk on a label-based server.
        session.examine(&destination).await?;
        for id in message_ids {
            let server_id = self
                .store
                .gmail_message_id(id)
                .map_err(db_err)?
                .ok_or_else(|| {
                    ProviderError::InvalidOperation(
                        "Refresh this message before changing its labels".into(),
                    )
                })?;
            let old_locations = self.store.locations_for_message(id).map_err(db_err)?;
            let mut sources = vec![all.clone(), destination.clone()];
            sources.extend(old_locations.iter().map(|l| l.mailbox.clone()));
            let mut visited = std::collections::BTreeSet::new();
            let mut found = None;
            for source in sources {
                if !visited.insert(source.clone()) {
                    continue;
                }
                if let Some((validity, row)) =
                    self.label_location(session, &source, server_id, id).await?
                {
                    found = Some((source, validity, row));
                    break;
                }
            }
            let (source, validity, before) = found.ok_or_else(|| {
                ProviderError::TransientTransport(
                    "Message is not currently available for this action".into(),
                )
            })?;
            let original = membership(&before)?;
            if let Some(add) = add.filter(|add| !has(original, add)) {
                let writer = as_write_session(session)?;
                select_for_write(writer, &self.store, &source, validity as u32).await?;
                writer
                    .uid_store_labels(&before.uid.to_string(), StoreOp::Add, &[add.to_string()])
                    .await?;
            }
            // Adding Trash/Spam/Inbox can expunge the old selected-folder UID.
            // Re-locate in the destination before removing any other membership.
            let (dest_validity, target) = self
                .label_location(session, &destination, server_id, id)
                .await?
                .ok_or_else(|| {
                    ProviderError::TransientTransport(
                        "Destination membership has not appeared; the action will be retried"
                            .into(),
                    )
                })?;
            let current = membership(&target)?;
            let removing: Vec<String> = remove
                .iter()
                .filter(|label| has(current, label))
                .map(|label| label.to_string())
                .collect();
            if !removing.is_empty() {
                let writer = as_write_session(session)?;
                select_for_write(writer, &self.store, &destination, dest_validity as u32).await?;
                writer
                    .uid_store_labels(&target.uid.to_string(), StoreOp::Remove, &removing)
                    .await?;
            }
            let (verified_validity, verified) = self
                .label_location(session, &destination, server_id, id)
                .await?
                .ok_or_else(|| {
                    ProviderError::TransientTransport(
                        "Message vanished during label verification".into(),
                    )
                })?;
            if verified_validity != dest_validity {
                return Err(ProviderError::TransientTransport(
                    "Destination UIDVALIDITY changed during label verification".into(),
                ));
            }
            let labels = membership(&verified)?;
            if add.is_some_and(|label| !has(labels, label))
                || remove.iter().any(|label| has(labels, label))
                || original
                    .iter()
                    .filter(|label| {
                        !["\\inbox", "\\trash", "\\spam"]
                            .contains(&label.to_ascii_lowercase().as_str())
                    })
                    .any(|label| !labels.contains(label))
            {
                return Err(ProviderError::TransientTransport(
                    "The server did not preserve the requested message labels".into(),
                ));
            }
            let mut round = SyncRoundWrite {
                label_snapshots: vec![(id.clone(), labels.clone())],
                locations: vec![ImapLocation {
                    mailbox: destination.clone(),
                    uidvalidity: dest_validity,
                    uid: verified.uid as i64,
                    message_id: id.clone(),
                    flags_json: serde_json::to_string(&verified.flags)
                        .map_err(|e| ProviderError::Other(e.to_string()))?,
                    modseq: None,
                }],
                ..Default::default()
            };
            for old in old_locations {
                let absent = plan.entry_for(&old.mailbox).is_some_and(|entry| {
                    if entry.is_inbox {
                        !has(labels, "\\Inbox")
                    } else {
                        match entry.role {
                            Some(MailboxRole::All) => {
                                has(labels, "\\Trash") || has(labels, "\\Spam")
                            }
                            Some(MailboxRole::Trash) => !has(labels, "\\Trash"),
                            Some(MailboxRole::Junk) => !has(labels, "\\Spam"),
                            Some(MailboxRole::Sent) => !has(labels, "\\Sent"),
                            _ => false,
                        }
                    }
                });
                let replaced = old.mailbox == destination
                    && (old.uidvalidity != dest_validity || old.uid != verified.uid as i64);
                if absent || replaced {
                    round
                        .deletions
                        .push((old.mailbox, old.uidvalidity, old.uid));
                }
            }
            if let Some(thread) = self.store.thread_of_message(id).map_err(db_err)? {
                round.changed_threads.push(thread.clone());
                round.hot_threads.push(thread);
            }
            self.store.commit_sync_round(&round).map_err(db_err)?;
        }
        Ok(())
    }
}
