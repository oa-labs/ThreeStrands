//! System-label moves between real mailboxes: archive, trash, spam, and their
//! inverses, plus saving the sender's own Sent copy.

use crate::provider::{ProviderError, ProviderResult};

use crate::provider::imap::fetch::fetch_identity;
use crate::provider::imap::plan::SyncPlan;
use crate::provider::imap::session::ImapSession;
use crate::provider::imap::ImapLocation;

use super::mutate::as_write_session;
use super::{db_err, ImapProvider, INBOX};

// ===========================================================================
// System-label MOVES (Phase 3 run 3): archive / unarchive, trash / untrash,
// spam / not-spam. These are a MOVE of a message's copy between real mailboxes
// (INBOX <-> Archive / Trash / Junk), never a flag. The engine delivers them
// through `modify_thread` only, in exactly six (add, remove) shapes
// (verified against `sync.rs::mutation_labels`).
// ===========================================================================

/// The two kinds of request `modify_thread` serves, decided up front so a
/// malformed request is rejected (permanently) before any command is built.
pub(super) enum ThreadMutationKind {
    /// A flag change (`UNREAD` / `STARRED`), handled by the run-2 flag path.
    Flag,
    /// A system-label move (archive/trash/spam and their reverses).
    SystemMove(SystemMovePlan),
}

/// A fully-validated system move: WHICH source mailboxes a message's copy may
/// move FROM (by role) and the single destination role it moves TO. "Already
/// satisfied" is defined by presence/absence of a copy in these roles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SystemMovePlan {
    /// The move being performed, for messages and no-op/`InvalidOperation`
    /// wording.
    op: SystemMoveOp,
}

/// The six system-move operations, each a (source role, destination role)
/// pair. Named so the no-command and nothing-movable messages are specific.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SystemMoveOp {
    /// `remove [INBOX]`: move the INBOX copy to Archive.
    Archive,
    /// `add [INBOX]`: move the Archive copy to INBOX.
    Unarchive,
    /// `add [TRASH], remove [INBOX]`: move the INBOX (or Archive) copy to Trash.
    Trash,
    /// `add [INBOX], remove [TRASH]`: move the Trash copy to INBOX.
    Untrash,
    /// `add [SPAM], remove [INBOX]`: move the INBOX (or Archive) copy to Junk.
    Spam,
    /// `add [INBOX], remove [SPAM]`: move the Junk copy to INBOX.
    NotSpam,
}

/// Where a message copy may be moved FROM and TO for one system move. A role
/// is resolved to a concrete mailbox name at apply time via the sync plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MoveRole {
    Inbox,
    Archive,
    Trash,
    Junk,
}

impl SystemMoveOp {
    /// The destination role a message moves TO.
    fn dest_role(self) -> MoveRole {
        match self {
            SystemMoveOp::Archive => MoveRole::Archive,
            SystemMoveOp::Trash => MoveRole::Trash,
            SystemMoveOp::Spam => MoveRole::Junk,
            SystemMoveOp::Unarchive | SystemMoveOp::Untrash | SystemMoveOp::NotSpam => {
                MoveRole::Inbox
            }
        }
    }

    /// The source roles whose copy is eligible to move, in PRIORITY order (the
    /// first present copy is the one that moves). archive/trash/spam move the
    /// INBOX copy, falling back to an Archive copy (trash/spam from the archive
    /// view); the reverses move the copy out of the single source mailbox.
    fn source_roles(self) -> &'static [MoveRole] {
        match self {
            SystemMoveOp::Archive => &[MoveRole::Inbox],
            SystemMoveOp::Trash | SystemMoveOp::Spam => &[MoveRole::Inbox, MoveRole::Archive],
            SystemMoveOp::Unarchive => &[MoveRole::Archive],
            SystemMoveOp::Untrash => &[MoveRole::Trash],
            SystemMoveOp::NotSpam => &[MoveRole::Junk],
        }
    }

    /// A plain-language name for error/no-op wording.
    fn describe(self) -> &'static str {
        match self {
            SystemMoveOp::Archive => "archive",
            SystemMoveOp::Unarchive => "unarchive",
            SystemMoveOp::Trash => "trash",
            SystemMoveOp::Untrash => "untrash",
            SystemMoveOp::Spam => "mark as spam",
            SystemMoveOp::NotSpam => "mark as not spam",
        }
    }
}

/// Classify a `modify_thread` `(add, remove)` request. Returns the flag path
/// for a request that names only `UNREAD`/`STARRED`; the move path for exactly
/// one of the six system-move shapes; and a permanent `InvalidOperation` for
/// everything else (a system id mixed with a flag or a `lf:`/`folder:`/SENT/
/// `kw:` id, an id in both lists, an unknown id, or a combination of system
/// ids that is not one of the six). Nothing is sent before this validates.
pub(super) fn classify_thread_mutation(
    add: &[String],
    remove: &[String],
) -> ProviderResult<ThreadMutationKind> {
    // Does any list mention a system-MOVE label (INBOX/TRASH/SPAM)?
    const MOVE_IDS: [&str; 3] = ["INBOX", "TRASH", "SPAM"];
    let is_move_id = |id: &String| MOVE_IDS.contains(&id.as_str());
    let any_move = add.iter().chain(remove.iter()).any(is_move_id);

    if !any_move {
        // No system-move label present: this is the flag path. `plan_flag_
        // mutation` (run 2) still rejects an unsupported id (SENT, lf:,
        // folder:, kw:, …) and an id in both lists, exactly as before.
        return Ok(ThreadMutationKind::Flag);
    }

    // A system move is being requested. It must be EXACTLY one of the six
    // engine shapes — no flag label, no `lf:`/`folder:`/`SENT`/`kw:`, no extra
    // id, and no id in both lists. Match the whole request literally.
    if let Some(op) = match_system_move_shape(add, remove) {
        return Ok(ThreadMutationKind::SystemMove(SystemMovePlan { op }));
    }

    Err(ProviderError::InvalidOperation(format!(
        "the IMAP provider accepts a system move (archive/trash/spam and their \
         reverses) only in its exact engine shape; the request add={add:?} \
         remove={remove:?} is not one of those shapes (a system label combined \
         with a flag, a folder/label id, SENT, an extra id, or an id in both \
         lists); refusing permanently"
    )))
}

/// Match a `(add, remove)` request against the six exact system-move shapes
/// (`sync.rs::mutation_labels`): archive `([],[INBOX])`, unarchive
/// `([INBOX],[])`, trash `([TRASH],[INBOX])`, untrash `([INBOX],[TRASH])`,
/// spam `([SPAM],[INBOX])`, not-spam `([INBOX],[SPAM])`. Any other combination
/// (extra ids, duplicates, an id in both lists) returns `None`.
fn match_system_move_shape(add: &[String], remove: &[String]) -> Option<SystemMoveOp> {
    let a: Vec<&str> = add.iter().map(String::as_str).collect();
    let r: Vec<&str> = remove.iter().map(String::as_str).collect();
    match (a.as_slice(), r.as_slice()) {
        ([], ["INBOX"]) => Some(SystemMoveOp::Archive),
        (["INBOX"], []) => Some(SystemMoveOp::Unarchive),
        (["TRASH"], ["INBOX"]) => Some(SystemMoveOp::Trash),
        (["INBOX"], ["TRASH"]) => Some(SystemMoveOp::Untrash),
        (["SPAM"], ["INBOX"]) => Some(SystemMoveOp::Spam),
        (["INBOX"], ["SPAM"]) => Some(SystemMoveOp::NotSpam),
        _ => None,
    }
}

impl ImapProvider {
    /// Resolve a [`MoveRole`] to its concrete catalog mailbox name through the
    /// sync plan (roles come from the saved plan/catalog — overrides, RFC 6154
    /// attribute, then name match — never hard-coded). INBOX is the reserved
    /// `is_inbox` entry; Archive/Trash/Junk are the entries whose resolved
    /// role matches. Returns `None` when the account has no mailbox for the
    /// role (e.g. no Trash configured).
    fn mailbox_for_role(&self, plan: &SyncPlan, role: MoveRole) -> Option<String> {
        match role {
            MoveRole::Inbox => plan
                .entries
                .iter()
                .find(|e| e.is_inbox)
                .map(|e| e.mailbox.clone())
                // The reserved INBOX always exists even before discovery
                // persisted its catalog row.
                .or_else(|| Some(INBOX.to_string())),
            MoveRole::Archive => self.mailbox_with_role(plan, crate::provider::imap::MailboxRole::Archive),
            MoveRole::Trash => self.mailbox_with_role(plan, crate::provider::imap::MailboxRole::Trash),
            MoveRole::Junk => self.mailbox_with_role(plan, crate::provider::imap::MailboxRole::Junk),
        }
    }

    /// The catalog mailbox name whose plan entry resolved to `role`, if any.
    pub(super) fn mailbox_with_role(
        &self,
        plan: &SyncPlan,
        role: crate::provider::imap::MailboxRole,
    ) -> Option<String> {
        plan.entries
            .iter()
            .find(|e| e.role == Some(role))
            .map(|e| e.mailbox.clone())
    }

    /// Apply a system-label move to every (surviving) message of a thread,
    /// idempotently and with partial-failure honesty.
    ///
    /// Plan-then-apply: the whole request is validated and the per-message
    /// move decided from LOCAL state before any command is sent.
    ///
    /// Per message (brief "Which copy moves"): find the first present copy in
    /// the op's source roles (INBOX first, Archive as the archive-view
    /// fallback for trash/spam). If none is present but a copy already exists
    /// in the DESTINATION role, the message is already satisfied (no command).
    /// If neither, the message is skipped as nothing-movable here. A thread
    /// where EVERY message is already satisfied sends nothing (not even
    /// SELECT); a thread with NO movable and NO already-satisfied message is a
    /// permanent `InvalidOperation` naming the op (e.g. trash of a Sent-only
    /// thread). The destination mailbox is resolved from the plan; a missing
    /// Trash/Junk is a permanent error naming the role, a missing Archive is
    /// created and the move retried once.
    ///
    /// Moves are grouped BY SOURCE MAILBOX and run one mailbox at a time over a
    /// single leased session (2-connection cap). Each move is performed per
    /// message (a single-UID relocate) so a half-applied copy+delete+expunge
    /// records exactly that message's destination location and the retry is
    /// idempotent. After each source mailbox's moves commit, local state is
    /// persisted (source row deleted, destination row inserted when COPYUID is
    /// known) before the next mailbox, so a later failure leaves the finished
    /// ones consistent and the retry re-derives only the rest.
    pub(super) async fn apply_system_move(
        &self,
        message_ids: &[String],
        plan_move: SystemMovePlan,
    ) -> ProviderResult<()> {
        let sync_plan = self.build_sync_plan()?;
        let op = plan_move.op;

        // Resolve the destination mailbox up front (same for every message).
        let dest_role = op.dest_role();
        let dest_mailbox = match self.mailbox_for_role(&sync_plan, dest_role) {
            Some(name) => name,
            None => {
                // Missing Trash/Junk is a permanent error naming the role; a
                // missing INBOX cannot happen (reserved). Archive's absence is
                // handled lazily (create-on-TRYCREATE), so it never reaches
                // here as None — but guard it with the same permanent error.
                return Err(ProviderError::InvalidOperation(format!(
                    "cannot {}: this account has no {} mailbox configured; refusing permanently",
                    op.describe(),
                    role_name(dest_role),
                )));
            }
        };

        // Decide each message's move from LOCAL state. A `PlannedMove` records
        // the source mailbox + uid(+uidvalidity) and the destination the row
        // should land in. `already_satisfied` counts messages needing nothing.
        let mut by_source: std::collections::BTreeMap<String, (i64, Vec<PlannedMove>)> =
            std::collections::BTreeMap::new();
        let mut already_satisfied = 0usize;
        let mut movable = 0usize;

        for message_id in message_ids {
            let locations = self
                .store
                .locations_for_message(message_id)
                .map_err(db_err)?;
            // Which mailbox name plays each role, for THIS message's locations.
            let in_role = |role: MoveRole| -> Option<ImapLocation> {
                let name = self.mailbox_for_role(&sync_plan, role)?;
                locations.iter().find(|l| l.mailbox == name).cloned()
            };

            // Already in the destination (and, for the reverses, no source
            // copy left) -> satisfied, no command.
            let dest_copy = locations.iter().any(|l| l.mailbox == dest_mailbox);

            // The first present source copy, in priority order.
            let source = op.source_roles().iter().find_map(|&role| in_role(role));

            match source {
                Some(src) => {
                    movable += 1;
                    by_source
                        .entry(src.mailbox.clone())
                        .or_insert_with(|| (src.uidvalidity, Vec::new()))
                        .1
                        .push(PlannedMove {
                            message_id: message_id.clone(),
                            uid: src.uid,
                            flags_json: src.flags_json.clone(),
                            modseq: src.modseq,
                        });
                }
                None if dest_copy => {
                    already_satisfied += 1;
                }
                None => {
                    // Nothing movable for this message (its only copies are in
                    // Sent / user folders / label folders). Skip it.
                }
            }
        }

        if movable == 0 {
            if already_satisfied > 0 {
                // Every message is already in the target state: no command.
                return Ok(());
            }
            // Nothing moved and nothing was already satisfied: permanent, so
            // the engine does not show a state the server never had.
            return Err(ProviderError::InvalidOperation(format!(
                "cannot {}: the thread has no message copy in a mailbox this move \
                 can act on (its messages live only in Sent, a user folder, or a \
                 label folder); refusing permanently",
                op.describe()
            )));
        }

        // One leased session for the whole move (the command slot).
        let mut session = self.open_session().await?;
        let result = self
            .apply_system_move_on(session.as_mut(), op, &dest_mailbox, by_source)
            .await;
        let _ = session.logout().await;
        result
    }

    /// The system move against an already-open session, factored out so the
    /// session is logged out exactly once. One source mailbox at a time; a
    /// source mailbox that finishes persists its local state before the next
    /// is attempted (partial-failure honesty).
    async fn apply_system_move_on(
        &self,
        session: &mut dyn ImapSession,
        op: SystemMoveOp,
        dest_mailbox: &str,
        by_source: std::collections::BTreeMap<String, (i64, Vec<PlannedMove>)>,
    ) -> ProviderResult<()> {
        for (source_mailbox, (uidvalidity, moves)) in by_source {
            let write_session = as_write_session(session)?;
            // Guard the SOURCE mailbox's UIDVALIDITY (stale -> transient,
            // nothing sent), as the flag path does.
            let ctx = crate::provider::imap::write::select_for_write(
                write_session,
                &self.store,
                &source_mailbox,
                uidvalidity as u32,
            )
            .await?;

            // Move each message individually so a half-applied
            // copy+delete+expunge records exactly that message's destination
            // location (idempotent retry). The whole source mailbox shares one
            // SELECT/context.
            for planned in &moves {
                self.move_one_message(write_session, &ctx, op, &source_mailbox, dest_mailbox, planned)
                    .await?;
            }
        }
        Ok(())
    }

    /// Move ONE message's source copy to `dest_mailbox`, recording local state
    /// and keeping the retry idempotent. Steps:
    ///
    /// 1. If the destination ALREADY has this message locally (recorded by an
    ///    earlier partial failure), do not copy again: finish the half-applied
    ///    move by expunging the still-present source copy, then clear the
    ///    source row. (copy+delete+expunge strategy only; MOVE is atomic.)
    /// 2. Otherwise relocate the single UID with the recording relocate. On a
    ///    partial failure that already copied, PERSIST the destination row
    ///    (from the returned COPYUID) BEFORE propagating the error, so the
    ///    retry takes path 1.
    /// 3. On a `TRYCREATE` destination: Archive is created and the move retried
    ///    once; Trash/Junk are never created (permanent error naming the role).
    /// 4. On success, delete the source location row and (when COPYUID is
    ///    known) insert the destination row with the same flags and the new
    ///    UID/uidvalidity, so the next sweep sees no change and no duplicate.
    async fn move_one_message(
        &self,
        session: &mut dyn crate::provider::imap::write::ImapWriteSession,
        ctx: &crate::provider::imap::write::WriteContext,
        op: SystemMoveOp,
        source_mailbox: &str,
        dest_mailbox: &str,
        planned: &PlannedMove,
    ) -> ProviderResult<()> {
        let uid = planned.uid as u32;

        // Explicit: is there a destination location row for THIS message
        // already (recorded by an earlier partial failure)?
        let dest_has_message = self
            .store
            .locations_for_message(&planned.message_id)
            .map_err(db_err)?
            .iter()
            .any(|l| l.mailbox == dest_mailbox);

        if dest_has_message
            && matches!(
                ctx.caps.relocation_strategy(),
                crate::provider::imap::write::RelocationStrategy::CopyDeleteExpunge
            )
        {
            // Finish the half-applied move: expunge the source copy only.
            crate::provider::imap::write::expunge_source_copies(session, ctx, &[uid]).await?;
            self.store
                .delete_location(source_mailbox, planned.uid_validity_of(ctx), uid as i64)
                .map_err(db_err)?;
            return Ok(());
        }

        // Path 2/3: relocate the single UID, recording a partial failure's
        // destination so a retry is idempotent.
        match self
            .relocate_one_recording(session, ctx, dest_mailbox, planned)
            .await
        {
            Ok(outcome) => {
                self.commit_moved_location(ctx, source_mailbox, dest_mailbox, planned, &outcome.copyuid)?;
                Ok(())
            }
            Err(failure) => {
                // Record whatever landed in the destination BEFORE propagating,
                // so the retry (path 1) does not copy again.
                if let Some(copyuid) = &failure.copyuid {
                    self.record_dest_only(dest_mailbox, planned, copyuid)?;
                }
                match failure.error {
                    crate::provider::imap::write::WriteError::TryCreate(text) => {
                        self.handle_trycreate_and_retry(
                            session,
                            ctx,
                            op,
                            source_mailbox,
                            dest_mailbox,
                            planned,
                            &text,
                        )
                        .await
                    }
                    crate::provider::imap::write::WriteError::Provider(error) => Err(error),
                }
            }
        }
    }

    /// Create a missing destination (Archive only) and retry the move ONCE;
    /// Trash/Junk are never created. The create path reuses the session's
    /// `create_mailbox`; a second failure propagates.
    #[allow(clippy::too_many_arguments)]
    async fn handle_trycreate_and_retry(
        &self,
        session: &mut dyn crate::provider::imap::write::ImapWriteSession,
        ctx: &crate::provider::imap::write::WriteContext,
        op: SystemMoveOp,
        source_mailbox: &str,
        dest_mailbox: &str,
        planned: &PlannedMove,
        trycreate_text: &str,
    ) -> ProviderResult<()> {
        // Only Archive is auto-created. Trash/Junk missing is permanent.
        if op.dest_role() != MoveRole::Archive {
            return Err(ProviderError::InvalidOperation(format!(
                "cannot {}: the destination mailbox {dest_mailbox:?} does not exist \
                 (TRYCREATE: {trycreate_text}) and the provider does not create it; \
                 refusing permanently",
                op.describe()
            )));
        }
        // Create the Archive mailbox, then retry the relocate once.
        session.create_mailbox(dest_mailbox).await?;
        match self
            .relocate_one_recording(session, ctx, dest_mailbox, planned)
            .await
        {
            Ok(outcome) => {
                self.commit_moved_location(ctx, source_mailbox, dest_mailbox, planned, &outcome.copyuid)
            }
            Err(failure) => {
                if let Some(copyuid) = &failure.copyuid {
                    self.record_dest_only(dest_mailbox, planned, copyuid)?;
                }
                Err(failure.error.into_provider_error())
            }
        }
    }

    /// After a VERIFIED move: delete the source location row, and when COPYUID
    /// is known insert the destination row (same flags, new UID, destination
    /// UIDVALIDITY). Without COPYUID (never on the UIDPLUS copy path; possible
    /// only on a MOVE server that returned none) just delete the source — the
    /// next sweep of the destination re-discovers the message by Message-ID,
    /// and the grace rule keeps the thread meanwhile.
    fn commit_moved_location(
        &self,
        ctx: &crate::provider::imap::write::WriteContext,
        source_mailbox: &str,
        dest_mailbox: &str,
        planned: &PlannedMove,
        copyuid: &Option<crate::provider::imap::write::CopyUid>,
    ) -> ProviderResult<()> {
        // Insert the destination row first (so a crash between the two leaves
        // the message FINDABLE in both, never lost), then delete the source.
        if let Some(copyuid) = copyuid {
            if let Some(dest_uid) = dest_uid_for(copyuid, planned.uid as u32) {
                self.store
                    .upsert_location(&ImapLocation {
                        mailbox: dest_mailbox.to_string(),
                        uidvalidity: copyuid.uidvalidity as i64,
                        uid: dest_uid as i64,
                        message_id: planned.message_id.clone(),
                        flags_json: planned.flags_json.clone(),
                        modseq: planned.modseq,
                    })
                    .map_err(db_err)?;
            }
        }
        self.store
            .delete_location(source_mailbox, ctx.uidvalidity as i64, planned.uid)
            .map_err(db_err)?;
        self.store
            .complete_copy(&planned.message_id, dest_mailbox)
            .map_err(db_err)?;
        Ok(())
    }

    /// Record ONLY the destination location row (used on a partial failure, so
    /// the retry sees the copy and does not re-copy). The source row is left
    /// in place; the retry's expunge removes the server copy and then clears
    /// it.
    fn record_dest_only(
        &self,
        dest_mailbox: &str,
        planned: &PlannedMove,
        copyuid: &crate::provider::imap::write::CopyUid,
    ) -> ProviderResult<()> {
        if let Some(dest_uid) = dest_uid_for(copyuid, planned.uid as u32) {
            self.store
                .upsert_location(&ImapLocation {
                    mailbox: dest_mailbox.to_string(),
                    uidvalidity: copyuid.uidvalidity as i64,
                    uid: dest_uid as i64,
                    message_id: planned.message_id.clone(),
                    flags_json: planned.flags_json.clone(),
                    modseq: planned.modseq,
                })
                .map_err(db_err)?;
            self.store
                .complete_copy(&planned.message_id, dest_mailbox)
                .map_err(db_err)?;
        }
        Ok(())
    }

    /// The COPY fallback records durable intent before the wire command. The
    /// native MOVE path is unchanged; it cannot create duplicate source copies.
    async fn relocate_one_recording(
        &self,
        session: &mut dyn crate::provider::imap::write::ImapWriteSession,
        ctx: &crate::provider::imap::write::WriteContext,
        dest: &str,
        planned: &PlannedMove,
    ) -> crate::provider::imap::write::RecordingRelocateResult {
        use crate::provider::imap::write::{RelocateFailure, RelocateOutcome, RelocationStrategy};
        if ctx.caps.relocation_strategy() != RelocationStrategy::CopyDeleteExpunge {
            return crate::provider::imap::write::relocate_uids_recording(
                session,
                ctx,
                &[planned.uid as u32],
                dest,
            )
            .await;
        }
        crate::provider::imap::write::require_targeted_delete(ctx).map_err(|error| RelocateFailure {
            copyuid: None,
            error: error.into(),
        })?;
        let outcome = self
            .copy_with_recovery(session, ctx, dest, planned)
            .await
            .map_err(|error| RelocateFailure {
                copyuid: None,
                error,
            })?;
        if let Err(error) =
            crate::provider::imap::write::expunge_source_copies(session, ctx, &[planned.uid as u32]).await
        {
            return Err(RelocateFailure {
                copyuid: outcome.copyuid,
                error: crate::provider::imap::write::classified_partial_relocate_error(
                    &ctx.mailbox,
                    dest,
                    &planned.uid.to_string(),
                    "COPY succeeded but verified source deletion failed",
                    &error,
                )
                .into(),
            });
        }
        Ok(RelocateOutcome {
            copyuid: outcome.copyuid,
        })
    }

    /// On retry after a lost reply or process exit, inspect the destination
    /// using normal sync identity before issuing another COPY. Only uncertain
    /// copies scan the destination, and FETCH is bounded and uses BODY.PEEK.
    pub(super) async fn copy_with_recovery(
        &self,
        session: &mut dyn crate::provider::imap::write::ImapWriteSession,
        ctx: &crate::provider::imap::write::WriteContext,
        dest: &str,
        planned: &PlannedMove,
    ) -> crate::provider::imap::write::WriteResult<crate::provider::imap::write::CopyOutcome> {
        use crate::provider::imap::write::WriteError;
        let arg = crate::provider::imap::write::build_wire_mailbox_arg(dest)?;
        let pending = self
            .store
            .pending_copy_identity(&planned.message_id, dest)
            .map_err(db_err)?;
        if let Some(identity) = &pending {
            if let Some(outcome) = self
                .reconcile_copy(session, ctx, dest, planned.uid as u32, identity)
                .await?
            {
                return Ok(outcome);
            }
        }
        let set = planned.uid.to_string();
        let source = fetch_identity(session, &set)
            .await?
            .into_iter()
            .find(|row| row.uid == planned.uid as u32)
            .ok_or_else(|| {
                ProviderError::TransientTransport(
                    "COPY source is no longer present; resync before retrying".into(),
                )
            })?;
        let identity = crate::provider::imap::identity::derive_message_id(self.store.account_id(), &source.inputs);
        if pending
            .as_ref()
            .is_some_and(|previous| previous != &identity)
        {
            return Err(ProviderError::InvalidOperation(
                "COPY source identity changed while recovering; refusing to copy another message"
                    .into(),
            )
            .into());
        }
        self.store
            .begin_copy(&planned.message_id, dest, &identity)
            .map_err(db_err)?;
        match session.uid_copy(&set, &arg).await {
            Ok(outcome) if outcome.copyuid.is_some() => Ok(outcome),
            Ok(_) => self.reconcile_copy(session, ctx, dest, planned.uid as u32, &identity).await?
                .ok_or_else(|| ProviderError::TransientTransport(
                    "COPY was acknowledged but its destination is not visible yet; retry reconciliation".into()
                ).into()),
            Err(WriteError::TryCreate(text)) => {
                // The server explicitly refused the COPY; no copy landed.
                self.store.complete_copy(&planned.message_id, dest).map_err(db_err)?;
                Err(WriteError::TryCreate(text))
            }
            Err(error) => Err(error), // Keep the intent: the outcome may be uncertain.
        }
    }

    async fn reconcile_copy(
        &self,
        session: &mut dyn crate::provider::imap::write::ImapWriteSession,
        ctx: &crate::provider::imap::write::WriteContext,
        dest: &str,
        source_uid: u32,
        identity: &str,
    ) -> ProviderResult<Option<crate::provider::imap::write::CopyOutcome>> {
        let status = session.examine(dest).await?;
        let validity = status.uid_validity.ok_or_else(|| {
            ProviderError::InvalidOperation(
                "COPY destination supplied no UIDVALIDITY; cannot reconcile safely".into(),
            )
        })?;
        let uids = session.uid_search("ALL").await?;
        let mut found = None;
        for batch in crate::provider::imap::write::build_uid_set_batches(&uids) {
            for row in fetch_identity(session, &batch).await? {
                if crate::provider::imap::identity::derive_message_id(self.store.account_id(), &row.inputs)
                    == identity
                {
                    found = Some(crate::provider::imap::write::CopyOutcome {
                        copyuid: Some(crate::provider::imap::write::CopyUid {
                            uidvalidity: validity,
                            source: vec![source_uid],
                            dest: vec![row.uid],
                        }),
                    });
                    break;
                }
            }
            if found.is_some() {
                break;
            }
        }
        // EXAMINE changed the selection. Re-establish and guard the source
        // before the caller can COPY, STORE or EXPUNGE using its old context.
        crate::provider::imap::write::select_for_write(session, &self.store, &ctx.mailbox, ctx.uidvalidity).await?;
        Ok(found)
    }
}

/// One message's planned move: the source UID (in the source mailbox the
/// `by_source` key names) and the row fields that carry over to the
/// destination (flags, modseq, stable id).
#[derive(Clone, Debug)]
pub(super) struct PlannedMove {
    pub(super) message_id: String,
    pub(super) uid: i64,
    pub(super) flags_json: String,
    pub(super) modseq: Option<i64>,
}

impl PlannedMove {
    /// The source mailbox's UIDVALIDITY, taken from the write context (the
    /// guard confirmed it matches what the location rows recorded).
    fn uid_validity_of(&self, ctx: &crate::provider::imap::write::WriteContext) -> i64 {
        ctx.uidvalidity as i64
    }
}

/// The destination UID a source UID mapped to, from a `COPYUID` pairing.
pub(super) fn dest_uid_for(copyuid: &crate::provider::imap::write::CopyUid, source_uid: u32) -> Option<u32> {
    copyuid
        .pairs()
        .find(|(src, _)| *src == source_uid)
        .map(|(_, dest)| dest)
}

/// A human name for a move role, for error wording.
fn role_name(role: MoveRole) -> &'static str {
    match role {
        MoveRole::Inbox => "Inbox",
        MoveRole::Archive => "Archive",
        MoveRole::Trash => "Trash",
        MoveRole::Junk => "Junk",
    }
}
