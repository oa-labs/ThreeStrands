//! Existing user-label membership, addressed by authoritative server identity.
//! Removing a label never deletes a physical copy or replaces the label set.
use super::{db_err, mutate::as_write_session, ImapProvider};
use crate::db::imap::label_edits::{ImapLabelEdit, ImapLabelEditOutcome, ImapLabelEditTarget};
use crate::models::Label;
use crate::provider::imap::{
    plan::{self, CatalogMailbox},
    policy,
    session::{ImapSession, MailboxEntry},
    write::{build_wire_mailbox_arg, select_for_write, StoreOp},
    ImapLocation, MailboxRole, SyncRoundWrite,
};
use crate::provider::{ProviderError, ProviderResult};

impl ImapProvider {
    pub(super) async fn create_native_label(&self, name: &str) -> ProviderResult<Label> {
        // Validate scalar input before connecting; LIST supplies the actual
        // hierarchy delimiter (which may differ from the cached catalog).
        let clean = validate_mailbox_label_name(name, "")?;
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
        self.recover_mailbox_label_edit(session, &before).await?;
        let delimiter = before.iter().find_map(|entry| entry.delimiter.as_deref());
        if let Some(delimiter) = delimiter {
            validate_mailbox_label_name(clean, delimiter)?;
        }
        if before.iter().any(|entry| entry.name == wire) {
            return self.record_native_label(&before, wire);
        }
        // An explicit system-role choice may name a missing mailbox. Creation
        // of user labels must not turn that role into a writable user label.
        self.mailbox_label_from_listing(&before, wire, true, true)?;
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
    pub(super) fn mailbox_label_from_listing(
        &self,
        entries: &[MailboxEntry],
        wire: &str,
        allow_missing: bool,
        native: bool,
    ) -> ProviderResult<Label> {
        let invalid = || {
            ProviderError::InvalidOperation(
                "This name is reserved for a system mailbox or a non-selectable container".into(),
            )
        };
        let row = entries.iter().find(|entry| entry.name == wire);
        let delimiter = row
            .and_then(|entry| entry.delimiter.as_deref())
            .or_else(|| entries.iter().find_map(|entry| entry.delimiter.as_deref()));
        if native
            && ["[Gmail]", "[GoogleMail]"].iter().any(|reserved| {
                wire.eq_ignore_ascii_case(reserved)
                    || delimiter.is_some_and(|separator| {
                        wire.split(separator)
                            .next()
                            .is_some_and(|root| root.eq_ignore_ascii_case(reserved))
                    })
            })
        {
            return Err(invalid());
        }
        if row.is_some_and(|entry| entry.no_select || entry.special_use.is_some()) {
            return Err(invalid());
        }
        if row.is_none() && !allow_missing {
            return Err(invalid());
        }
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
        let plan = if native {
            plan::build_native_plan(&catalog, delimiter, &self.settings)
        } else {
            plan::build_plan(&catalog, delimiter, &self.settings)
        };
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
        let bindings = self.store.label_bindings().map_err(db_err)?;
        Ok(Label {
            id: plan::mailbox_label_id(&bindings, wire, &id),
            name,
            kind: if native || entry.label_kind == plan::LabelKind::LabelFolder {
                "user"
            } else {
                "folder"
            }
            .into(),
        })
    }

    fn record_native_label(&self, entries: &[MailboxEntry], wire: &str) -> ProviderResult<Label> {
        let label = self.mailbox_label_from_listing(entries, wire, false, true)?;
        let row = entries
            .iter()
            .find(|entry| entry.name == wire)
            .expect("validated live listing");
        self.store
            .upsert_mailbox_catalog(wire, row.delimiter.as_deref(), None)
            .map_err(db_err)?;
        Ok(label)
    }

    pub(super) async fn edit_mailbox_label(
        &self,
        id: &str,
        name: Option<&str>,
        native: bool,
    ) -> ProviderResult<Option<Label>> {
        let mut session = self.open_session().await?;
        let result = self
            .edit_mailbox_label_on(session.as_mut(), id, name, native)
            .await;
        let _ = session.logout().await;
        result
    }

    async fn edit_mailbox_label_on(
        &self,
        session: &mut dyn ImapSession,
        id: &str,
        name: Option<&str>,
        native: bool,
    ) -> ProviderResult<Option<Label>> {
        if session.supports_label_extensions() != native {
            return Err(invalid_label_edit(
                "The server’s label capabilities changed. Refresh the account before editing.",
            ));
        }
        let entries = session.list_mailboxes().await?;
        if !entries
            .iter()
            .any(|row| row.name.eq_ignore_ascii_case("INBOX") && !row.no_select)
        {
            return Err(invalid_label_edit(
                "The server has not confirmed its mailbox listing.",
            ));
        }
        self.recover_mailbox_label_edit(session, &entries).await?;
        self.verify_mailbox_label_bindings(session, &entries)
            .await?;
        let bindings = self.store.label_bindings().map_err(db_err)?;
        if name.is_none()
            && bindings
                .iter()
                .any(|binding| binding.label_id == id && binding.mailbox.is_none())
        {
            return Ok(None);
        }
        let plan = self.build_sync_plan()?;
        let path = plan
            .entries
            .iter()
            .find(|entry| {
                !entry.is_inbox
                    && entry.role.is_none()
                    && entry.location_label_id.as_deref() == Some(id)
            })
            .map(|entry| entry.mailbox.as_str());
        let source = entries
            .iter()
            .find(|row| Some(row.name.as_str()) == path)
            .ok_or_else(|| {
                invalid_label_edit(
                    "This label is no longer available. Refresh the account and try again.",
                )
            })?;
        let label = self.mailbox_label_from_listing(&entries, &source.name, false, native)?;
        if label.id != id {
            return Err(invalid_label_edit(
                "This label changed on the server. Refresh the account before editing it.",
            ));
        }
        let delimiter = source.delimiter.as_deref().unwrap_or_default();
        let label_container = plan
            .entry_for(&source.name)
            .filter(|entry| entry.label_kind == plan::LabelKind::LabelFolder)
            .and(self.settings.label_container.as_deref());
        let destination = name
            .map(|name| {
                let clean = if !native && label_container.is_some() {
                    super::label_folders::validate_label_name(name, delimiter)?
                } else {
                    validate_mailbox_label_name(name, delimiter)?
                };
                let wire = crate::provider::imap::write::encode_modified_utf7(&clean);
                Ok::<_, ProviderError>(match label_container {
                    Some(container) if !wire.starts_with(&format!("{container}{delimiter}")) => {
                        format!("{container}{delimiter}{wire}")
                    }
                    _ => wire,
                })
            })
            .transpose()?;
        if destination.as_deref() == Some(&source.name) {
            return Ok(Some(label));
        }
        let child_prefix = (!delimiter.is_empty()).then(|| format!("{}{delimiter}", source.name));
        let children: Vec<_> = entries
            .iter()
            .filter(|row| {
                child_prefix
                    .as_ref()
                    .is_some_and(|prefix| row.name.starts_with(prefix))
            })
            .collect();
        if child_prefix.as_ref().is_some_and(|prefix| {
            destination
                .as_deref()
                .is_some_and(|name| starts_with_ignore_ascii_case(name, prefix))
        }) {
            return Err(invalid_label_edit(
                "Choose a destination outside this label’s own hierarchy.",
            ));
        }
        if !native && !children.is_empty() {
            return Err(invalid_label_edit(
                "Rename or delete the nested folders before changing this parent folder.",
            ));
        }
        if let Some(wire) = &destination {
            if entries
                .iter()
                .any(|row| row.name != source.name && row.name.eq_ignore_ascii_case(wire))
            {
                return Err(invalid_label_edit("A label already uses that name."));
            }
            if !delimiter.is_empty() {
                let destination_prefix = format!("{wire}{delimiter}");
                if entries.iter().any(|row| {
                    starts_with_ignore_ascii_case(&row.name, &destination_prefix)
                        && !children.iter().any(|child| child.name == row.name)
                }) {
                    return Err(invalid_label_edit(
                        "The destination already has nested labels.",
                    ));
                }
            }
            if self
                .mailbox_label_from_listing(&entries, wire, true, native)?
                .kind
                != label.kind
            {
                return Err(invalid_label_edit(
                    "Choose a name in the same mailbox area.",
                ));
            }
        }

        if let Some(prefix) = child_prefix.as_deref().filter(|_| native) {
            let known = self
                .store
                .cached_native_label_descendants(prefix)
                .map_err(db_err)?;
            if known.iter().any(|name| {
                !children
                    .iter()
                    .any(|child| child.name == *name && !child.no_select)
            }) {
                return Err(invalid_label_edit(
                    "A nested label is missing from the IMAP listing. Show it in IMAP in Gmail’s Labels settings, or refresh before changing the parent.",
                ));
            }
            let mut configured_mailboxes = self
                .settings
                .mailbox_overrides
                .values()
                .chain(self.settings.archive_mailbox.iter())
                .chain(self.settings.label_container.iter());
            if children.iter().any(|row| row.special_use.is_some())
                || configured_mailboxes.any(|mailbox| mailbox.starts_with(prefix))
            {
                return Err(invalid_label_edit(
                    "A nested path is a system mailbox or an account’s label container.",
                ));
            }
        }
        let catalog = self.store.mailboxes().map_err(db_err)?;
        let expected_validity = |id: &str, path: &str| {
            bindings
                .iter()
                .find(|binding| binding.label_id == id)
                .and_then(|binding| binding.uidvalidity)
                .or_else(|| {
                    catalog
                        .iter()
                        .find(|row| row.name == path && row.uidvalidity > 0)
                        .map(|row| row.uidvalidity as u32)
                })
        };
        let source_validity =
            checked_mailbox_validity(session, &source.name, expected_validity(id, &source.name))
                .await?;
        let mut descendants = Vec::new();
        for child in children.into_iter().filter(|child| !child.no_select) {
            let child_label =
                self.mailbox_label_from_listing(&entries, &child.name, false, true)?;
            let child_destination = destination.as_ref().map(|wire| {
                // A child path has the exact source prefix plus the separator
                // and its remaining segments, including any deeper hierarchy.
                format!("{wire}{}", &child.name[source.name.len()..])
            });
            if let Some(wire) = child_destination.as_deref() {
                policy::check_mailbox_name_len(wire.len())?;
                self.mailbox_label_from_listing(&entries, wire, true, true)?;
            }
            let validity = checked_mailbox_validity(
                session,
                &child.name,
                expected_validity(&child_label.id, &child.name),
            )
            .await?;
            descendants.push(ImapLabelEditTarget {
                id: child_label.id,
                source: child.name.clone(),
                source_uidvalidity: validity,
                destination: child_destination,
            });
        }
        // Record the edit before any server write. Folder deletion verifies
        // surviving copies and source coordinates before sending DELETE.
        let source_arg = build_wire_mailbox_arg(&source.name)?;
        let destination_arg = destination
            .as_deref()
            .map(build_wire_mailbox_arg)
            .transpose()?;
        session.examine("INBOX").await?;
        let mut edit = ImapLabelEdit {
            root: ImapLabelEditTarget {
                id: id.into(),
                source: source.name.clone(),
                source_uidvalidity: source_validity,
                destination,
            },
            acknowledged: false,
            physical_folder: !native,
            descendants,
        };
        self.store.begin_label_edit(&edit).map_err(db_err)?;
        let result = match destination_arg {
            Some(to) => {
                as_write_session(session)?
                    .rename_mailbox_arg(&source_arg, &to)
                    .await
            }
            None if native => {
                as_write_session(session)?
                    .delete_mailbox_arg(&source_arg)
                    .await
            }
            None => {
                let locations = self
                    .store
                    .locations_in_mailbox(&source.name)
                    .map_err(db_err)?;
                self.delete_label_folder_on(session, &source.name, source_validity, &locations)
                    .await
            }
        };
        if result.is_ok() {
            edit.acknowledged = true;
            self.store.acknowledge_label_edit(&edit).map_err(db_err)?;
        }
        let after = session.list_mailboxes().await?;
        let reconciled = self.recover_mailbox_label_edit(session, &after).await?;
        match (result, reconciled) {
            (Ok(()), Some(label)) => Ok(Some(label)),
            (Ok(()), None) if edit.root.destination.is_none() => Ok(None),
            (Err(_), None)
                if edit.root.destination.is_none()
                    && !after
                        .iter()
                        .any(|row| row.name == edit.root.source && !row.no_select) =>
            {
                Ok(None)
            }
            (Err(error), _) => Err(error),
            _ => Err(invalid_label_edit(
                "The rename could not be verified. Review any affected Split Inbox in Settings.",
            )),
        }
    }

    /// Resolve only what the server has established. A lost RENAME response
    /// and a new name alone are insufficient evidence to move saved rules.
    pub(super) async fn recover_mailbox_label_edit(
        &self,
        session: &mut dyn ImapSession,
        listing: &[MailboxEntry],
    ) -> ProviderResult<Option<Label>> {
        let Some(edit) = self.store.pending_label_edit().map_err(db_err)? else {
            return Ok(None);
        };
        if session.supports_label_extensions() == edit.physical_folder
            || !listing
                .iter()
                .any(|row| row.name.eq_ignore_ascii_case("INBOX") && !row.no_select)
        {
            return Err(ProviderError::TransientTransport(
                "The server has not returned a complete label listing yet.".into(),
            ));
        }
        let mut outcomes = Vec::new();
        for target in edit.targets() {
            let source_exists = listing.iter().any(|row| {
                row.name == target.source && !row.no_select && row.special_use.is_none()
            });
            let outcome = if source_exists
                && mailbox_validity(session, &target.source).await? == target.source_uidvalidity
            {
                ImapLabelEditOutcome::Unchanged
            } else if let Some(row) = target
                .destination
                .as_ref()
                .filter(|_| edit.acknowledged)
                .and_then(|wire| {
                    listing.iter().find(|row| {
                        row.name == *wire && !row.no_select && row.special_use.is_none()
                    })
                })
            {
                let mut label = self.mailbox_label_from_listing(
                    listing,
                    &row.name,
                    false,
                    !edit.physical_folder,
                )?;
                label.id.clone_from(&target.id);
                ImapLabelEditOutcome::Renamed {
                    label,
                    mailbox: &row.name,
                    delimiter: row.delimiter.as_deref(),
                    uidvalidity: mailbox_validity(session, &row.name).await?,
                }
            } else {
                ImapLabelEditOutcome::Unavailable
            };
            outcomes.push((target, outcome));
        }
        self.store
            .finish_label_edit(&edit, &outcomes)
            .map_err(db_err)?;
        let (root, descendants) = outcomes.split_first().expect("every edit has a root");
        if edit.acknowledged
            && descendants.is_empty()
            && matches!(root.1, ImapLabelEditOutcome::Unchanged)
        {
            return Err(ProviderError::TransientTransport(
                "The server still lists the original label. Refresh and try again.".into(),
            ));
        }
        if edit.root.destination.is_some() {
            if let ImapLabelEditOutcome::Renamed { label, .. } = &root.1 {
                if descendants
                    .iter()
                    .all(|(_, outcome)| matches!(outcome, ImapLabelEditOutcome::Renamed { .. }))
                {
                    return Ok(Some(label.clone()));
                }
            }
            if edit.acknowledged {
                return Err(invalid_label_edit(
                    "The server did not confirm the full rename. Refresh labels and review any affected Split Inboxes.",
                ));
            }
        } else {
            if descendants
                .iter()
                .any(|(_, outcome)| !matches!(outcome, ImapLabelEditOutcome::Unchanged))
            {
                return Err(invalid_label_edit(
                    "Gmail reported a change to a nested label during deletion. Refresh labels and review any affected Split Inboxes.",
                ));
            }
            if edit.acknowledged && !matches!(root.1, ImapLabelEditOutcome::Unavailable) {
                return Err(invalid_label_edit(
                    "The server still lists the original label. Refresh and try again.",
                ));
            }
        }
        Ok(None)
    }

    /// A missing binding or changed UIDVALIDITY is an unresolved label.
    /// Neither proves where the server put a previous label.
    pub(super) async fn verify_mailbox_label_bindings(
        &self,
        session: &mut dyn ImapSession,
        listing: &[MailboxEntry],
    ) -> ProviderResult<()> {
        if session.supports_label_extensions() != self.store.uses_native_labels().map_err(db_err)?
            || !listing
                .iter()
                .any(|row| row.name.eq_ignore_ascii_case("INBOX") && !row.no_select)
        {
            return Ok(());
        }
        for binding in self.store.label_bindings().map_err(db_err)? {
            let Some(wire) = binding.mailbox else {
                continue;
            };
            let attached = if listing
                .iter()
                .any(|row| row.name == wire && !row.no_select && row.special_use.is_none())
            {
                session.examine(&wire).await?.uid_validity == binding.uidvalidity
            } else {
                false
            };
            if !attached {
                self.store
                    .retire_unavailable_label_binding(&binding.label_id)
                    .map_err(db_err)?;
            }
        }
        Ok(())
    }

    pub(super) async fn apply_native_label(
        &self,
        ids: &[String],
        label_id: &str,
        add: bool,
    ) -> ProviderResult<()> {
        if self
            .store
            .pending_label_edit()
            .map_err(db_err)?
            .is_some_and(|edit| edit.targets().any(|target| target.id == label_id))
        {
            return Err(ProviderError::TransientTransport(
                "ThreeStrands is still checking the label change.".into(),
            ));
        }
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
            let status = session.examine(&label.mailbox).await?;
            let bindings = self.store.label_bindings().map_err(db_err)?;
            if bindings.iter().find(|binding| binding.label_id == label_id)
                .and_then(|binding| binding.uidvalidity).is_some_and(|uid| Some(uid) != status.uid_validity)
            {
                return Err(invalid("This label changed on the server; refresh the account"));
            }
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

fn invalid_label_edit(message: &str) -> ProviderError {
    ProviderError::InvalidOperation(message.into())
}

fn starts_with_ignore_ascii_case(name: &str, prefix: &str) -> bool {
    name.get(..prefix.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
}

async fn checked_mailbox_validity(
    session: &mut dyn ImapSession,
    mailbox: &str,
    expected: Option<u32>,
) -> ProviderResult<u32> {
    let validity = mailbox_validity(session, mailbox).await?;
    if expected.is_some_and(|expected| expected != validity) {
        return Err(invalid_label_edit(
            "A label in the hierarchy changed on the server. Refresh the account before editing it.",
        ));
    }
    Ok(validity)
}

async fn mailbox_validity(session: &mut dyn ImapSession, mailbox: &str) -> ProviderResult<u32> {
    session
        .examine(mailbox)
        .await?
        .uid_validity
        .filter(|uid| *uid != 0)
        .ok_or_else(|| invalid_label_edit("The server did not report a stable label identity."))
}

pub(super) fn validate_mailbox_label_name(name: &str, delimiter: &str) -> ProviderResult<String> {
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
