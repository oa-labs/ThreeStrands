//! Existing user-label membership, addressed by authoritative server identity.
//! Removing a label never deletes a physical copy or replaces the label set.
use super::{db_err, mutate::as_write_session, ImapProvider};
use crate::models::Label;
use crate::provider::imap::{
    plan::{self, CatalogMailbox},
    policy,
    session::{ImapSession, MailboxEntry},
    write::{select_for_write, StoreOp},
    ImapLocation, MailboxRole, SyncRoundWrite,
};
use crate::provider::{ProviderError, ProviderResult};

impl ImapProvider {
    pub(super) async fn create_native_label(&self, name: &str) -> ProviderResult<Label> {
        // Validate scalar input before connecting; LIST supplies the actual
        // hierarchy delimiter (which may differ from the cached catalog).
        let clean = validate_native_label_name(name, "")?;
        let wire = crate::provider::imap::write::encode_modified_utf7(&clean);
        policy::check_mailbox_name_len(wire.len())?;
        let arg = crate::provider::imap::write::build_wire_mailbox_arg(&wire)?;
        let mut session = self.open_session().await?;
        let result = self
            .create_native_label_on(session.as_mut(), &clean, &wire, &arg)
            .await;
        let _ = session.logout().await;
        result
    }

    async fn create_native_label_on(
        &self,
        session: &mut dyn ImapSession,
        clean: &str,
        wire: &str,
        arg: &str,
    ) -> ProviderResult<Label> {
        if !session.supports_label_extensions() {
            return Err(ProviderError::InvalidOperation(
                "The server no longer supports native labels".into(),
            ));
        }
        let before = session.list_mailboxes().await?;
        let delimiter = before.iter().find_map(|entry| entry.delimiter.as_deref());
        if let Some(delimiter) = delimiter {
            validate_native_label_name(clean, delimiter)?;
        }
        if before.iter().any(|entry| entry.name == wire) {
            return self.record_native_label(&before, wire);
        }
        // An explicit system-role choice may name a missing mailbox. Creation
        // of user labels must not turn that role into a writable user label.
        self.native_label_from_listing(&before, wire, true)?;
        let created = as_write_session(session)?
            .create_mailbox_arg(arg)
            .await
            .map_err(|error| error.into_provider_error());
        // CREATE may race another client or lose its reply after committing.
        // A fresh listing is the authority; never invent a successful row.
        match session.list_mailboxes().await {
            Ok(after) if after.iter().any(|entry| entry.name == wire) => {
                self.record_native_label(&after, wire)
            }
            Ok(_) => match created {
                Err(error) => Err(error),
                Ok(()) => Err(ProviderError::TransientTransport(
                    "The server did not confirm the new label; refresh or retry".into(),
                )),
            },
            Err(error) => Err(created.err().unwrap_or(error)),
        }
    }

    /// Use the same ID resolution as sync, preserving configured lf: children
    /// and ordinary folder: IDs. Names are exact wire identities, not aliases.
    fn native_label_from_listing(
        &self,
        entries: &[MailboxEntry],
        wire: &str,
        allow_missing: bool,
    ) -> ProviderResult<Label> {
        let invalid = || {
            ProviderError::InvalidOperation(
                "This name is reserved for a system mailbox or a non-selectable container".into(),
            )
        };
        let row = entries.iter().find(|entry| entry.name == wire);
        if row.is_some_and(|entry| entry.no_select || entry.special_use.is_some()) {
            return Err(invalid());
        }
        if row.is_none() && !allow_missing {
            return Err(invalid());
        }
        let delimiter = row
            .and_then(|entry| entry.delimiter.as_deref())
            .or_else(|| entries.iter().find_map(|entry| entry.delimiter.as_deref()));
        let mut catalog = entries
            .iter()
            .filter(|entry| !entry.no_select)
            .map(|entry| CatalogMailbox {
                name: entry.name.clone(),
                special_use: entry.special_use.clone(),
            })
            .collect::<Vec<_>>();
        if row.is_none() {
            catalog.push(CatalogMailbox {
                name: wire.into(),
                special_use: None,
            });
        }
        let plan = plan::build_native_plan(&catalog, delimiter, &self.settings);
        let entry = plan
            .entry_for(wire)
            .filter(|entry| !entry.is_inbox && entry.role.is_none())
            .ok_or_else(invalid)?;
        let id = entry.location_label_id.clone().ok_or_else(invalid)?;
        let display = id
            .strip_prefix("lf:")
            .or_else(|| id.strip_prefix("folder:"))
            .ok_or_else(invalid)?;
        let name = crate::provider::imap::write::decode_modified_utf7(display)?;
        Ok(Label {
            id,
            name,
            kind: "user".into(),
        })
    }

    fn record_native_label(&self, entries: &[MailboxEntry], wire: &str) -> ProviderResult<Label> {
        let label = self.native_label_from_listing(entries, wire, false)?;
        let row = entries
            .iter()
            .find(|entry| entry.name == wire)
            .expect("validated live listing");
        self.store
            .upsert_mailbox_catalog(wire, row.delimiter.as_deref(), None)
            .map_err(db_err)?;
        Ok(label)
    }

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

fn validate_native_label_name(name: &str, delimiter: &str) -> ProviderResult<String> {
    let invalid = || {
        ProviderError::InvalidOperation("Use a non-empty label name with non-empty hierarchy segments and no control characters or reserved system names".into())
    };
    if name.chars().any(char::is_control) {
        return Err(invalid());
    }
    let clean = name.trim();
    if clean.is_empty() || clean.starts_with('\\') || clean.eq_ignore_ascii_case("INBOX") {
        return Err(invalid());
    }
    policy::check_label_name_len(clean.chars().count())?;
    policy::check_mailbox_name_len(clean.len())?;
    if !delimiter.is_empty() && clean.split(delimiter).any(|part| part.trim().is_empty()) {
        return Err(invalid());
    }
    Ok(clean.to_string())
}
