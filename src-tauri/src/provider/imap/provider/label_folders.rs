//! User labels stored as label folders (`lf:` ids): adding and removing a
//! label copy, and creating and deleting the label mailboxes.

use crate::models::Label;
use crate::provider::{ProviderError, ProviderResult};

use crate::provider::imap::plan::{self, SyncPlan};
use crate::provider::imap::policy::{self};
use crate::provider::imap::session::ImapSession;
use crate::provider::imap::settings::LabelStorage;
use crate::provider::imap::ImapLocation;

use super::moves::dest_uid_for;
use super::moves::PlannedMove;
use super::mutate::as_write_session;
use super::{db_err, ImapProvider, INBOX};

// Label-folder membership uses copies under a configured container. Its
// deletion checks also protect ordinary folders, where DELETE removes mail
// copies rather than just removing a label from messages.

/// The hierarchy delimiter the account uses, from any catalog row (every
/// mailbox on a server shares one), defaulting to `/`.
fn account_delimiter(delimiter: Option<&str>) -> &str {
    delimiter.filter(|d| !d.is_empty()).unwrap_or("/")
}

/// Build the label-folder mailbox name for a label `name` relative to the
/// container: `<container><delim><name>`. Pure; the inverse of
/// `plan::label_folder_relative_name`.
fn label_mailbox_name(container: &str, delimiter: &str, name: &str) -> String {
    format!("{container}{delimiter}{name}")
}

impl ImapProvider {
    /// Re-check the negotiated server model before any physical folder-label
    /// operation, including the first command after upgrading or importing.
    pub(super) async fn open_plain_session(&self) -> ProviderResult<Box<dyn ImapSession>> {
        let mut session = self.open_session().await?;
        if session.supports_label_extensions() {
            let _ = session.logout().await;
            return Err(ProviderError::InvalidOperation(
                "This server uses native labels. Refresh the account before applying labels or editing folders."
                    .into(),
            ));
        }
        Ok(session)
    }

    /// This account's label container, or a permanent `InvalidOperation`
    /// naming the storage mode when label-folder mode is not configured. The
    /// single gate every `lf:` path passes through BEFORE any command, so a
    /// keyword-mode or no-label account refuses label-folder operations
    /// cleanly rather than touching the server.
    fn label_container(&self) -> ProviderResult<String> {
        match self.settings.label_storage {
            LabelStorage::Folders => self.settings.label_container.clone().ok_or_else(|| {
                ProviderError::InvalidOperation(
                    "this account stores labels as folders but has no label container configured; \
                     refusing the label operation permanently"
                        .into(),
                )
            }),
            LabelStorage::Keywords => Err(ProviderError::InvalidOperation(
                "this account uses keyword labels instead of label folders; \
                 refusing permanently"
                    .into(),
            )),
            LabelStorage::None => Err(ProviderError::InvalidOperation(
                "this account has no user-label storage (label_storage is 'none'); \
                 refusing permanently"
                    .into(),
            )),
        }
    }

    /// The hierarchy delimiter from the persisted catalog (defaulting to `/`).
    pub(super) fn delimiter(&self) -> ProviderResult<String> {
        let rows = self.store.mailboxes().map_err(db_err)?;
        Ok(account_delimiter(rows.iter().find_map(|r| r.delimiter.as_deref())).to_string())
    }

    // --- add / remove a user label (modify_messages lf: path) --------------

    /// Apply a single `lf:<name>` ADD or REMOVE to a set of messages, in
    /// label-folder mode, idempotently and with partial-failure honesty. The
    /// request has already been classified ([`classify_message_mutation`]) to a
    /// single label id and a direction.
    ///
    /// ADD: for each message, if a copy already lives in the label mailbox it
    /// is satisfied (no command); otherwise COPY one existing synced copy into
    /// the label mailbox (source priority INBOX > Archive > a user folder >
    /// Sent; never Trash/Junk). A message with no usable source is a permanent
    /// error naming it. Idempotent: the destination row is recorded as soon as
    /// COPYUID confirms, so a retry after a partial failure never double-copies.
    ///
    /// REMOVE: delete only the copy in the label mailbox (needs `\Deleted`
    /// storable + UIDPLUS `UID EXPUNGE`). Refuses permanently, sending NO
    /// write, when removing the label copy would delete the message's LAST
    /// synced copy. A message with no copy in the label mailbox is satisfied.
    pub(super) async fn apply_label_mutation(
        &self,
        message_ids: &[String],
        label_id: &str,
        add: bool,
    ) -> ProviderResult<()> {
        if !self
            .store
            .can_write_label_binding(label_id, &self.settings, true)
            .map_err(db_err)?
        {
            return Err(ProviderError::LabelBindingPending);
        }
        self.label_container()?;
        let plan = self.build_sync_plan()?;
        let label_mailbox = &plan.entries.iter().find(|entry| {
            entry.label_kind == plan::LabelKind::LabelFolder
                && entry.location_label_id.as_deref() == Some(label_id)
        }).ok_or_else(|| ProviderError::InvalidOperation(
            "This label folder does not exist in the current catalog. Refresh the account before applying it.".into()
        ))?.mailbox;

        if add {
            self.apply_label_add(message_ids, label_id, label_mailbox)
                .await
        } else {
            self.apply_label_remove(message_ids, label_id, label_mailbox)
                .await
        }
    }

    /// ADD `lf:<name>` to every message: COPY one existing copy into the label
    /// mailbox, grouped by source mailbox over one leased session.
    async fn apply_label_add(
        &self,
        message_ids: &[String],
        label_id: &str,
        label_mailbox: &str,
    ) -> ProviderResult<()> {
        // The label mailbox must exist in the local catalog (never auto-create
        // on add — only create_label does that).
        let catalog: std::collections::BTreeSet<String> = self
            .store
            .mailboxes()
            .map_err(db_err)?
            .into_iter()
            .map(|m| m.name)
            .collect();
        if !catalog.contains(label_mailbox) {
            return Err(ProviderError::InvalidOperation(format!(
                "cannot add label {label_id:?}: its mailbox {label_mailbox:?} does not exist in \
                 the local catalog; create the label first (refusing permanently)"
            )));
        }

        let sync_plan = self.build_sync_plan()?;

        // Decide each message's COPY from LOCAL state (plan-then-apply). A
        // message already in the label mailbox is satisfied; otherwise pick its
        // best source copy by priority. A message with no usable source is a
        // permanent error naming it.
        let mut by_source: std::collections::BTreeMap<String, (i64, Vec<PlannedMove>)> =
            std::collections::BTreeMap::new();
        for message_id in message_ids {
            let locations = self
                .store
                .locations_for_message(message_id)
                .map_err(db_err)?;
            if locations.iter().any(|l| l.mailbox == *label_mailbox) {
                self.store
                    .complete_copy(message_id, label_mailbox)
                    .map_err(db_err)?;
                continue; // Already labelled: no command.
            }
            let source = self
                .best_label_source(&sync_plan, &locations)
                .ok_or_else(|| {
                    ProviderError::InvalidOperation(format!(
                        "cannot add label {label_id:?} to message {message_id:?}: it has no synced \
                         copy outside Trash/Junk to copy from; refusing permanently"
                    ))
                })?;
            by_source
                .entry(source.mailbox.clone())
                .or_insert_with(|| (source.uidvalidity, Vec::new()))
                .1
                .push(PlannedMove {
                    message_id: message_id.clone(),
                    uid: source.uid,
                    flags_json: source.flags_json.clone(),
                    modseq: source.modseq,
                });
        }

        if by_source.is_empty() {
            // Every message is already in the label mailbox: no command.
            return Ok(());
        }

        let mut session = self.open_plain_session().await?;
        let result = async {
            if self
                .store
                .label_bindings()
                .map_err(db_err)?
                .iter()
                .any(|binding| binding.label_id == label_id)
            {
                self.verify_mailbox_for_label_write(session.as_mut(), label_id, label_mailbox)
                    .await?;
            }
            self.apply_label_add_on(session.as_mut(), label_mailbox, by_source)
                .await
        }
        .await;
        let _ = session.logout().await;
        result
    }

    /// The best source copy to COPY into a label mailbox, by the brief's
    /// priority: INBOX, then Archive, then any user folder, then Sent. NEVER a
    /// copy whose only mailbox role is Trash or Junk (those are not a source).
    /// A copy in a label folder is not itself a source (we copy real mail, not
    /// a label view). Returns `None` when no usable source copy exists.
    fn best_label_source(
        &self,
        plan: &SyncPlan,
        locations: &[ImapLocation],
    ) -> Option<ImapLocation> {
        // Rank a location by its mailbox's resolved role: lower is better.
        // INBOX (0) > Archive (1) > user folder (2) > Sent (3); Trash/Junk and
        // label folders are NOT sources (None).
        let rank = |mailbox: &str| -> Option<u8> {
            if mailbox.eq_ignore_ascii_case(INBOX) {
                return Some(0);
            }
            let entry = plan.entry_for(mailbox)?;
            match entry.role {
                Some(crate::provider::imap::MailboxRole::Archive) => Some(1),
                Some(crate::provider::imap::MailboxRole::Sent) => Some(3),
                Some(crate::provider::imap::MailboxRole::Trash)
                | Some(crate::provider::imap::MailboxRole::Junk) => None,
                // Aggregates / Drafts carry no usable source either.
                Some(crate::provider::imap::MailboxRole::All)
                | Some(crate::provider::imap::MailboxRole::Drafts) => None,
                None => match entry.label_kind {
                    // A real user folder is a source; a label folder is not.
                    plan::LabelKind::UserFolder => Some(2),
                    _ => None,
                },
            }
        };
        locations
            .iter()
            .filter_map(|l| rank(&l.mailbox).map(|r| (r, l)))
            .min_by_key(|(r, l)| (*r, l.uid))
            .map(|(_, l)| l.clone())
    }

    /// The ADD against an open session: COPY each planned source copy into the
    /// label mailbox, one source mailbox at a time, recording the destination
    /// location row as soon as COPYUID confirms (idempotent retry).
    async fn apply_label_add_on(
        &self,
        session: &mut dyn ImapSession,
        label_mailbox: &str,
        by_source: std::collections::BTreeMap<String, (i64, Vec<PlannedMove>)>,
    ) -> ProviderResult<()> {
        for (source_mailbox, (uidvalidity, copies)) in by_source {
            let write_session = as_write_session(session)?;
            let ctx = crate::provider::imap::write::select_for_write(
                write_session,
                &self.store,
                &source_mailbox,
                uidvalidity as u32,
            )
            .await?;
            for planned in &copies {
                self.copy_one_label(write_session, &ctx, label_mailbox, planned)
                    .await?;
            }
        }
        Ok(())
    }

    /// COPY one message's source copy into the label mailbox and record the
    /// destination location row (idempotent). A single-UID COPY so COPYUID maps
    /// unambiguously. If the destination already has this message locally (an
    /// earlier partial success), skip. A missing COPYUID or a durable intent
    /// from an uncertain attempt is reconciled by identity before success.
    async fn copy_one_label(
        &self,
        session: &mut dyn crate::provider::imap::write::ImapWriteSession,
        ctx: &crate::provider::imap::write::WriteContext,
        label_mailbox: &str,
        planned: &PlannedMove,
    ) -> ProviderResult<()> {
        let already = self
            .store
            .locations_for_message(&planned.message_id)
            .map_err(db_err)?
            .iter()
            .any(|l| l.mailbox == *label_mailbox);
        if already {
            return Ok(()); // The copy already landed (recorded) — idempotent.
        }
        let uid = planned.uid as u32;
        match self
            .copy_with_recovery(session, ctx, label_mailbox, planned)
            .await
        {
            Ok(outcome) => {
                if let Some(copyuid) = &outcome.copyuid {
                    if let Some(dest_uid) = dest_uid_for(copyuid, uid) {
                        self.store
                            .upsert_location(&ImapLocation {
                                mailbox: label_mailbox.to_string(),
                                uidvalidity: copyuid.uidvalidity as i64,
                                uid: dest_uid as i64,
                                message_id: planned.message_id.clone(),
                                flags_json: planned.flags_json.clone(),
                                modseq: planned.modseq,
                            })
                            .map_err(db_err)?;
                        self.store
                            .complete_copy(&planned.message_id, label_mailbox)
                            .map_err(db_err)?;
                    }
                } else {
                    log::warn!(
                        "imap add-label: no COPYUID for {} into {label_mailbox:?}; the next sweep \
                         will discover the labelled copy by Message-ID",
                        planned.message_id
                    );
                }
                Ok(())
            }
            Err(crate::provider::imap::write::WriteError::TryCreate(text)) => {
                // The label mailbox vanished between the catalog check and the
                // COPY. Permanent: create_label owns creation, not add.
                Err(ProviderError::InvalidOperation(format!(
                    "cannot add label into {label_mailbox:?}: the server reports it does not exist \
                     (TRYCREATE: {text}); create the label first (refusing permanently)"
                )))
            }
            Err(crate::provider::imap::write::WriteError::Provider(error)) => Err(error),
        }
    }

    /// REMOVE `lf:<name>` from every message: delete only the copy in the label
    /// mailbox, with the never-delete-the-last-copy guard and UIDPLUS
    /// requirement checked BEFORE any write.
    async fn apply_label_remove(
        &self,
        message_ids: &[String],
        label_id: &str,
        label_mailbox: &str,
    ) -> ProviderResult<()> {
        // Decide each message's delete from LOCAL state. A message with no copy
        // in the label mailbox is satisfied. The never-delete-the-last-copy
        // guard: if the label copy is the message's ONLY synced location,
        // refuse the WHOLE request permanently before any command.
        let mut by_uidvalidity: Option<i64> = None;
        let mut targets: Vec<PlannedMove> = Vec::new();
        for message_id in message_ids {
            let locations = self
                .store
                .locations_for_message(message_id)
                .map_err(db_err)?;
            let in_label: Vec<&ImapLocation> = locations
                .iter()
                .filter(|l| l.mailbox == *label_mailbox)
                .collect();
            if in_label.is_empty() {
                continue; // No copy in the label mailbox: satisfied.
            }
            // The invariant: never delete the message's last synced copy.
            if locations.len() == in_label.len() {
                return Err(ProviderError::InvalidOperation(format!(
                    "cannot remove label {label_id:?} from message {message_id:?}: the label copy \
                     is its only synced location, so removing the label would delete the message; \
                     refusing permanently"
                )));
            }
            for location in in_label {
                // All copies in one mailbox share its UIDVALIDITY.
                by_uidvalidity.get_or_insert(location.uidvalidity);
                targets.push(PlannedMove {
                    message_id: message_id.clone(),
                    uid: location.uid,
                    flags_json: location.flags_json.clone(),
                    modseq: location.modseq,
                });
            }
        }

        if targets.is_empty() {
            return Ok(()); // Every message already lacks the label: no command.
        }
        let uidvalidity = by_uidvalidity.expect("non-empty targets carry a UIDVALIDITY");
        if self
            .store
            .label_bindings()
            .map_err(db_err)?
            .iter()
            .any(|binding| {
                binding.label_id == label_id && binding.uidvalidity != Some(uidvalidity as u32)
            })
        {
            return Err(ProviderError::InvalidOperation(
                "This label folder changed on the server. Refresh the account before removing its label.".into(),
            ));
        }

        let mut session = self.open_plain_session().await?;
        let result = self
            .apply_label_remove_on(session.as_mut(), label_mailbox, uidvalidity, &targets)
            .await;
        let _ = session.logout().await;
        result
    }

    /// The REMOVE against an open session: select the label mailbox for write
    /// (UIDPLUS gate + UIDVALIDITY guard), then per message mark `\Deleted` and
    /// `UID EXPUNGE` just its UID, deleting the local row only after the server
    /// delete confirms. A half-applied state (\Deleted stored, expunge failed)
    /// keeps the copy present+flagged and its retry just finishes it, with the
    /// cause's retry class preserved.
    async fn apply_label_remove_on(
        &self,
        session: &mut dyn ImapSession,
        label_mailbox: &str,
        uidvalidity: i64,
        targets: &[PlannedMove],
    ) -> ProviderResult<()> {
        self.confirm_surviving_copies(
            session,
            label_mailbox,
            targets.iter().map(|target| target.message_id.as_str()),
        )
        .await?;
        let write_session = as_write_session(session)?;
        let ctx = crate::provider::imap::write::select_for_write(
            write_session,
            &self.store,
            label_mailbox,
            uidvalidity as u32,
        )
        .await?;
        // UIDPLUS is required: without a targeted UID EXPUNGE a plain EXPUNGE
        // would remove other clients' \Deleted mail, so refuse (nothing sent).
        if !ctx.caps.can_expunge_uids() {
            return Err(ProviderError::InvalidOperation(format!(
                "cannot remove a label copy from {label_mailbox:?}: the server does not advertise \
                 UIDPLUS, so a targeted UID EXPUNGE is unavailable and a plain EXPUNGE could delete \
                 other clients' messages; refusing permanently"
            )));
        }
        for planned in targets {
            // expunge_source_copies marks \Deleted then UID EXPUNGE of just this
            // UID; a half-applied state (expunge failed after \Deleted) keeps
            // the copy and its retry re-runs the same step. Delete the local
            // row only after the server delete confirms.
            crate::provider::imap::write::expunge_source_copies(
                write_session,
                &ctx,
                &[planned.uid as u32],
            )
            .await?;
            self.store
                .delete_location(label_mailbox, ctx.uidvalidity as i64, planned.uid)
                .map_err(db_err)?;
        }
        Ok(())
    }

    /// Local coordinates are only candidates: another client can expunge a
    /// copy between syncs. Before destroying a label copy, confirm a live,
    /// non-Deleted UID in another mailbox at the same UIDVALIDITY. Check the
    /// whole request before any write, using read-only, flags-only commands.
    async fn confirm_surviving_copies<'a>(
        &self,
        session: &mut dyn ImapSession,
        label_mailbox: &str,
        message_ids: impl Iterator<Item = &'a str> + Send,
    ) -> ProviderResult<()> {
        let mut unconfirmed: std::collections::BTreeSet<String> =
            message_ids.map(str::to_owned).collect();
        let mut candidates: std::collections::BTreeMap<String, Vec<ImapLocation>> =
            std::collections::BTreeMap::new();
        for message_id in &unconfirmed {
            for location in self
                .store
                .locations_for_message(message_id)
                .map_err(db_err)?
            {
                if location.mailbox != label_mailbox {
                    candidates
                        .entry(location.mailbox.clone())
                        .or_default()
                        .push(location);
                }
            }
        }
        for (mailbox, locations) in candidates {
            let status = match session.examine(&mailbox).await {
                Ok(status) => status,
                // A vanished/inaccessible mailbox provides no evidence of a
                // survivor. Transport/auth failures retain their retry class.
                Err(ProviderError::InvalidOperation(_)) => continue,
                Err(error) => return Err(error),
            };
            let locations: Vec<_> = locations
                .iter()
                .filter(|location| {
                    unconfirmed.contains(&location.message_id)
                        && status.uid_validity == Some(location.uidvalidity as u32)
                })
                .collect();
            let uids: Vec<_> = locations
                .iter()
                .map(|location| location.uid as u32)
                .collect();
            let (flags, _) = self.fetch_flags(session, &uids).await?;
            for location in locations {
                if flags.get(&(location.uid as u32)).is_some_and(|flags| {
                    !flags
                        .iter()
                        .any(|flag| flag.eq_ignore_ascii_case("\\Deleted"))
                }) {
                    unconfirmed.remove(&location.message_id);
                }
            }
            if unconfirmed.is_empty() {
                return Ok(());
            }
        }
        if unconfirmed.is_empty() {
            return Ok(());
        }
        Err(ProviderError::InvalidOperation(format!(
            "cannot delete copies in {label_mailbox:?}: {} message(s) have no verified surviving \
             server copy; resync before removing the label",
            unconfirmed.len()
        )))
    }

    // --- create / delete a label -------------------------------------------

    /// Create a label mailbox for `name` and add its catalog row so
    /// `list_labels` shows it immediately. Validation + the case-insensitive
    /// idempotent-alias check happen BEFORE any command. Returns the existing
    /// label when `name` aliases one case-insensitively (Proton labels are
    /// case-insensitive).
    pub(super) async fn create_label_folder(&self, name: &str) -> ProviderResult<Label> {
        let container = self.label_container()?;
        let delimiter = self.delimiter()?;
        let clean = validate_label_name(name, &delimiter)?;

        let coordinating = self.store.has_enrolled_peers().map_err(db_err)?;
        if !coordinating {
            // A label whose name matches ignoring case is an idempotent alias.
            let plan = self.build_sync_plan()?;
            if let Some(label) = plan
                .label_catalog(false, std::iter::empty())
                .into_iter()
                .find(|label| label.kind == "user" && label.name.eq_ignore_ascii_case(&clean))
            {
                return Ok(label);
            }
        }

        // Persist LIST-compatible wire names and IDs, as previous versions
        // do. Encode only the newly typed leaf, never the saved container.
        let wire_name = crate::provider::imap::write::encode_modified_utf7(&clean);
        let label_mailbox = label_mailbox_name(&container, &delimiter, &wire_name);
        let arg = crate::provider::imap::write::build_wire_mailbox_arg(&label_mailbox)?;

        let mut session = self.open_plain_session().await?;
        let result = async {
            if coordinating {
                let before = session.list_mailboxes().await?;
                if !before
                    .iter()
                    .any(|row| row.name.eq_ignore_ascii_case(INBOX) && !row.no_select)
                {
                    return Err(ProviderError::LabelBindingPending);
                }
                self.recover_mailbox_label_edit(session.as_mut(), &before)
                    .await?;
                self.verify_mailbox_label_bindings(session.as_mut(), &before)
                    .await?;
                if let Some(row) = before
                    .iter()
                    .find(|row| row.name.eq_ignore_ascii_case(&label_mailbox))
                {
                    let label =
                        self.mailbox_label_from_listing(&before, &row.name, false, false)?;
                    if label.kind != "user" {
                        return Err(ProviderError::InvalidOperation(
                            "This name is reserved for a system mailbox.".into(),
                        ));
                    }
                    self.store
                        .upsert_mailbox_catalog(&row.name, row.delimiter.as_deref(), None)
                        .map_err(db_err)?;
                    return Ok(Some(label));
                }
            }
            self.create_label_folder_on(session.as_mut(), &container, &label_mailbox, &arg)
                .await?;
            Ok(None)
        }
        .await;
        let _ = session.logout().await;
        if let Some(label) = result? {
            return Ok(label);
        }

        // Record the catalog row so list_labels reflects it with no sync. A
        // label folder is synced, Folder-class; born with zero counters (the
        // next sweep fills them), special_use NULL, delimiter carried.
        self.store
            .upsert_mailbox_catalog(&label_mailbox, Some(&delimiter), None)
            .map_err(db_err)?;

        Ok(Label {
            id: plan::mailbox_label_id(
                &self.store.label_bindings().map_err(db_err)?,
                &label_mailbox,
                &format!("lf:{wire_name}"),
            ),
            name: clean,
            kind: "user".into(),
        })
    }

    /// CREATE the label mailbox over an open session. If the container itself
    /// is missing (`TRYCREATE`/`NONEXISTENT`), create the container first and
    /// retry once — the user asked for a label, so creating the container it
    /// lives under is acceptable here.
    async fn create_label_folder_on(
        &self,
        session: &mut dyn ImapSession,
        container: &str,
        label_mailbox: &str,
        arg: &str,
    ) -> ProviderResult<()> {
        let write_session = as_write_session(session)?;
        match write_session.create_mailbox_arg(arg).await {
            Ok(()) => Ok(()),
            Err(crate::provider::imap::write::WriteError::TryCreate(_)) => {
                // The container does not exist yet: create it, then retry once.
                let container_arg =
                    crate::provider::imap::write::build_wire_mailbox_arg(container)?;
                if let Err(error) = write_session.create_mailbox_arg(&container_arg).await {
                    return Err(error.into_provider_error());
                }
                write_session
                    .create_mailbox_arg(arg)
                    .await
                    .map_err(|e| e.into_provider_error())
                    .map(|()| {
                        log::info!(
                            "imap create_label: created missing container {container:?} before \
                             {label_mailbox:?}"
                        );
                    })
            }
            Err(crate::provider::imap::write::WriteError::Provider(error)) => Err(error),
        }
    }

    /// Folder creation verifies the new path against live LIST, including
    /// its hierarchy delimiter and configured system and label mailboxes.
    pub(super) async fn create_user_folder(&self, name: &str) -> ProviderResult<Label> {
        let mut session = self.open_plain_session().await?;
        let result = async {
            let before = session.list_mailboxes().await?;
            if !before
                .iter()
                .any(|row| row.name.eq_ignore_ascii_case(INBOX) && !row.no_select)
            {
                return Err(ProviderError::TransientTransport(
                    "The server has not confirmed the folder listing.".into(),
                ));
            }
            self.recover_mailbox_label_edit(session.as_mut(), &before)
                .await?;
            self.verify_mailbox_label_bindings(session.as_mut(), &before)
                .await?;
            let delimiter = before
                .iter()
                .find_map(|row| row.delimiter.as_deref())
                .unwrap_or_default();
            let clean = super::native_labels::validate_mailbox_label_name(name, delimiter)?;
            let wire = crate::provider::imap::write::encode_modified_utf7(&clean);
            let label = self.mailbox_label_from_listing(&before, &wire, true, false)?;
            if label.kind != "folder" {
                return Err(ProviderError::InvalidOperation(
                    "Choose a folder name outside the configured label container.".into(),
                ));
            }
            if !before.iter().any(|row| row.name == wire) {
                let arg = crate::provider::imap::write::build_wire_mailbox_arg(&wire)?;
                let created = as_write_session(session.as_mut())?
                    .create_mailbox_arg(&arg)
                    .await
                    .map_err(|error| error.into_provider_error());
                let after = session.list_mailboxes().await?;
                if let Some(row) = after.iter().find(|row| row.name == wire) {
                    let label = self.mailbox_label_from_listing(&after, &wire, false, false)?;
                    self.store
                        .upsert_mailbox_catalog(&wire, row.delimiter.as_deref(), None)
                        .map_err(db_err)?;
                    return Ok(label);
                }
                created?;
                return Err(ProviderError::TransientTransport(
                    "The server did not confirm the new folder.".into(),
                ));
            }
            let row = before
                .iter()
                .find(|row| row.name == wire)
                .expect("verified LIST entry");
            self.store
                .upsert_mailbox_catalog(&wire, row.delimiter.as_deref(), None)
                .map_err(db_err)?;
            Ok(label)
        }
        .await;
        let _ = session.logout().await;
        result
    }

    /// EXAMINE and SEARCH verify the exact UID coordinates before DELETE.
    /// A mismatch refuses deletion. Select INBOX before deleting the mailbox.
    pub(super) async fn delete_label_folder_on(
        &self,
        session: &mut dyn ImapSession,
        label_mailbox: &str,
        expected_validity: u32,
        locations: &[ImapLocation],
    ) -> ProviderResult<()> {
        for location in locations {
            if !self
                .store
                .locations_for_message(&location.message_id)
                .map_err(db_err)?
                .iter()
                .any(|other| other.mailbox != label_mailbox)
            {
                return Err(ProviderError::InvalidOperation(
                    "This folder has mail with no other synced copy. Move that mail before deleting the folder.".into(),
                ));
            }
        }
        self.confirm_surviving_copies(
            session,
            label_mailbox,
            locations
                .iter()
                .map(|location| location.message_id.as_str()),
        )
        .await?;
        let local_count = locations.len();
        let status = session.examine(label_mailbox).await?;
        if status.exists as usize != local_count {
            return Err(ProviderError::InvalidOperation(format!(
                "cannot delete label mailbox {label_mailbox:?}: the server reports {} message(s) \
                 but the local store knows only {local_count}, so some mail is unsynced and could \
                 be lost; refusing permanently",
                status.exists
            )));
        }
        let server_uids: std::collections::BTreeSet<u32> =
            session.uid_search("ALL").await?.into_iter().collect();
        let local_uids: std::collections::BTreeSet<u32> =
            locations.iter().map(|l| l.uid as u32).collect();
        if status.uid_validity != Some(expected_validity)
            || locations
                .iter()
                .any(|location| location.uidvalidity != i64::from(expected_validity))
            || server_uids != local_uids
        {
            return Err(ProviderError::InvalidOperation(format!(
                "cannot delete label mailbox {label_mailbox:?}: its UIDVALIDITY or message UIDs changed; \
                 unsynced mail could be lost, so resync first"
            )));
        }
        // Never DELETE the selected mailbox: select INBOX first.
        session.select(INBOX).await?;
        let write_session = as_write_session(session)?;
        let arg = crate::provider::imap::write::build_wire_mailbox_arg(label_mailbox)?;
        write_session.delete_mailbox_arg(&arg).await
    }
}

/// Validate a user-supplied label name (the part relative to the container):
/// non-empty after trimming, no control characters, must not contain the
/// server delimiter, no leading/trailing space or delimiter, within
/// [`policy::MAX_LABEL_NAME_CHARS`]. Returns the TRIMMED name. A permanent
/// `InvalidOperation` describes any violation.
pub(super) fn validate_label_name(name: &str, delimiter: &str) -> ProviderResult<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(ProviderError::InvalidOperation(
            "a label name cannot be empty; refusing permanently".into(),
        ));
    }
    policy::check_label_name_len(trimmed.chars().count())?;
    if trimmed.chars().any(|c| c.is_control()) {
        return Err(ProviderError::InvalidOperation(
            "a label name cannot contain a control character; refusing permanently".into(),
        ));
    }
    if !delimiter.is_empty() && trimmed.contains(delimiter) {
        return Err(ProviderError::InvalidOperation(format!(
            "a label name cannot contain the server hierarchy delimiter {delimiter:?}; \
             refusing permanently"
        )));
    }
    // The trimmed name has no leading/trailing space by construction; also
    // reject a leading/trailing delimiter char (e.g. a single-char delimiter).
    if let Some(first) = delimiter.chars().next() {
        if trimmed.starts_with(first) || trimmed.ends_with(first) {
            return Err(ProviderError::InvalidOperation(format!(
                "a label name cannot start or end with the hierarchy delimiter {delimiter:?}; \
                 refusing permanently"
            )));
        }
    }
    Ok(trimmed.to_string())
}
