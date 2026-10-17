//! Read/unread and star/unstar flag mutations, and the [`MailMutate`] impl that
//! routes each request to the flag, move, or label-folder path.

use async_trait::async_trait;

use crate::models::Label;
use crate::provider::{MailMutate, ProviderError, ProviderResult};

use crate::provider::imap::session::ImapSession;
use crate::provider::imap::ImapLocation;

use super::moves::classify_thread_mutation;
use super::moves::ThreadMutationKind;
use super::{db_err, ImapProvider};

/// The desired end-state of one message flag after a mutation request is
/// reduced: the flag must end up PRESENT, or ABSENT, on every synced copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DesiredFlag {
    /// The flag must be set on the message (e.g. `STARRED` -> `\Flagged`
    /// present; `remove UNREAD` -> `\Seen` present).
    Present,
    /// The flag must be cleared from the message (e.g. `add UNREAD` ->
    /// `\Seen` absent; `unstar` -> `\Flagged` absent).
    Absent,
}

/// The reduced plan for a flag mutation: the net desired state of `\Seen`
/// and/or `\Flagged`. Only the flags the request actually names are `Some`;
/// a flag the request did not mention is left untouched everywhere.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct FlagPlan {
    /// Desired `\Seen` state, derived from `UNREAD` (add UNREAD = `\Seen`
    /// absent; remove UNREAD = `\Seen` present). `None` if UNREAD was not in
    /// the request.
    seen: Option<DesiredFlag>,
    /// Desired `\Flagged` state, derived from `STARRED` (add STARRED =
    /// `\Flagged` present; remove STARRED = `\Flagged` absent). `None` if
    /// STARRED was not in the request.
    flagged: Option<DesiredFlag>,
}

impl FlagPlan {
    /// Whether this plan asks for no change at all (neither flag named).
    fn is_empty(self) -> bool {
        self.seen.is_none() && self.flagged.is_none()
    }

    /// Whether a stored flag list ALREADY satisfies every part of this plan,
    /// so the location needs no write command (idempotency / no-op).
    fn already_satisfied_by(self, flags: &[String]) -> bool {
        let has = |flag: &str| flags.iter().any(|f| f.eq_ignore_ascii_case(flag));
        let ok = |desired: Option<DesiredFlag>, present: bool| match desired {
            None => true,
            Some(DesiredFlag::Present) => present,
            Some(DesiredFlag::Absent) => !present,
        };
        ok(self.seen, has("\\Seen")) && ok(self.flagged, has("\\Flagged"))
    }

    /// The stored flag list this plan would leave behind on a location that
    /// started from `flags`: the same list with `\Seen`/`\Flagged` forced to
    /// the desired presence (case-insensitively), every other flag kept. Used
    /// to update `flags_json` locally after the verified store, so the next
    /// sync sweep sees no difference and does not flap.
    fn apply_to(self, flags: &[String]) -> Vec<String> {
        let mut out: Vec<String> = flags.to_vec();
        let force = |out: &mut Vec<String>, flag: &str, desired: Option<DesiredFlag>| {
            match desired {
                None => {}
                Some(DesiredFlag::Present) => {
                    if !out.iter().any(|f| f.eq_ignore_ascii_case(flag)) {
                        out.push(flag.to_string());
                    }
                }
                Some(DesiredFlag::Absent) => {
                    out.retain(|f| !f.eq_ignore_ascii_case(flag));
                }
            }
        };
        force(&mut out, "\\Seen", self.seen);
        force(&mut out, "\\Flagged", self.flagged);
        out
    }
}

/// Reduce a `(add, remove)` request to a [`FlagPlan`], rejecting anything the
/// flag path does not support. Only `UNREAD` and `STARRED` are supported here;
/// every other label id (INBOX, TRASH, SPAM, SENT, `lf:`, `folder:`, or an
/// unknown id) is a permanent `InvalidOperation` naming it, so the engine
/// rolls the change back rather than retrying forever. A label appearing in
/// BOTH `add` and `remove` is contradictory and rejected (never guessed).
fn plan_flag_mutation(add: &[String], remove: &[String]) -> ProviderResult<FlagPlan> {
    // Every id must be a supported system flag label.
    let check_supported = |ids: &[String]| -> ProviderResult<()> {
        for id in ids {
            if id != "UNREAD" && id != "STARRED" {
                return Err(ProviderError::InvalidOperation(format!(
                    "label {id:?} is not a flag the IMAP provider can set/clear directly \
                     (only UNREAD and STARRED are); refusing permanently"
                )));
            }
        }
        Ok(())
    };
    check_supported(add)?;
    check_supported(remove)?;

    // No label may appear in both lists (contradictory; reject, do not guess).
    for id in add {
        if remove.iter().any(|r| r == id) {
            return Err(ProviderError::InvalidOperation(format!(
                "label {id:?} appears in both the add and remove lists; \
                 the request is contradictory and is refused"
            )));
        }
    }

    let adds = |label: &str| add.iter().any(|l| l == label);
    let removes = |label: &str| remove.iter().any(|l| l == label);

    // UNREAD present means the message LACKS \Seen, so:
    //   add UNREAD    -> \Seen must be ABSENT
    //   remove UNREAD -> \Seen must be PRESENT
    let seen = if adds("UNREAD") {
        Some(DesiredFlag::Absent)
    } else if removes("UNREAD") {
        Some(DesiredFlag::Present)
    } else {
        None
    };
    // STARRED == \Flagged, so:
    //   add STARRED    -> \Flagged PRESENT
    //   remove STARRED -> \Flagged ABSENT
    let flagged = if adds("STARRED") {
        Some(DesiredFlag::Present)
    } else if removes("STARRED") {
        Some(DesiredFlag::Absent)
    } else {
        None
    };

    Ok(FlagPlan { seen, flagged })
}

impl ImapProvider {
    /// Apply an `UNREAD`/`STARRED` flag change to a set of messages, at EVERY
    /// synced location of each message, idempotently and with partial-failure
    /// honesty. The shared implementation behind both `modify_thread`
    /// (thread-level, resolved to its messages) and `modify_messages`
    /// (message-level).
    ///
    /// Steps (plan-then-apply; nothing is sent until the whole request
    /// validates):
    /// 1. Reduce `(add, remove)` to a net [`FlagPlan`] via
    ///    [`plan_flag_mutation`] — reject an unsupported label id or a label
    ///    in both lists with a permanent `InvalidOperation`, before any
    ///    command. A request that reduces to no flag at all is `Ok(())`.
    /// 2. For every message, read its stored locations. A message with NO
    ///    location at all (an emptied/grace thread) is a permanent
    ///    `InvalidOperation` naming it — never a silent success. Skip any
    ///    location whose stored flags already satisfy the plan (idempotency;
    ///    avoids a pointless command).
    /// 3. Group the remaining locations by `(mailbox, uidvalidity)`. For each
    ///    mailbox, `select_for_write` (guards UIDVALIDITY; a stale mailbox is a
    ///    named transient error and NOTHING is sent), then run one
    ///    `verified_store` per store op (`+/-\Seen`, `+/-\Flagged`) using
    ///    run 1's capability-gated verified store — so a server that cannot
    ///    keep a flag fails cleanly with the permanent error naming it.
    /// 4. After each mailbox's ops verify, update that mailbox's locations'
    ///    stored `flags_json` so the next sync sweep sees no difference and
    ///    does NOT journal a spurious change. One session is reused for the
    ///    whole mutation (respecting the 2-connection cap — the command slot).
    /// 5. Partial failure: mailboxes are processed one at a time, and a
    ///    mailbox whose ops succeeded has its local flags already persisted; a
    ///    later mailbox's error is returned (transient stays transient,
    ///    permanent stays permanent), so a retry touches only the failed one.
    ///
    /// Does NOT bump the sync generation or journal anything: a flag the next
    /// sweep would observe anyway is kept consistent locally (step 4), and the
    /// engine already applied the optimistic local change that drove this
    /// mutation — the provider only makes the server and the local location
    /// rows agree. Journaling here would re-report a change the engine made.
    async fn apply_flag_mutation(
        &self,
        message_ids: &[String],
        add: &[String],
        remove: &[String],
    ) -> ProviderResult<()> {
        let plan = plan_flag_mutation(add, remove)?;
        if plan.is_empty() {
            return Ok(()); // Nothing to do (no supported flag named).
        }

        // Gather every location that still needs a write, grouped by mailbox,
        // validating messages up front (plan-then-apply). A message with no
        // location at all fails the whole request permanently.
        //
        // Keyed by mailbox name; each value carries the mailbox's UIDVALIDITY
        // (taken from its location rows) and the locations whose stored flags
        // do NOT yet satisfy the plan.
        let mut by_mailbox: std::collections::BTreeMap<String, (i64, Vec<ImapLocation>)> =
            std::collections::BTreeMap::new();
        for message_id in message_ids {
            let locations = self
                .store
                .locations_for_message(message_id)
                .map_err(db_err)?;
            if locations.is_empty() {
                return Err(ProviderError::InvalidOperation(format!(
                    "message {message_id:?} has no synced location to modify \
                     (it may have been expunged or moved to an unsynced mailbox); \
                     refusing the flag change permanently"
                )));
            }
            for location in locations {
                let flags: Vec<String> =
                    serde_json::from_str(&location.flags_json).unwrap_or_default();
                if plan.already_satisfied_by(&flags) {
                    continue; // Idempotent: this copy already matches.
                }
                by_mailbox
                    .entry(location.mailbox.clone())
                    .or_insert_with(|| (location.uidvalidity, Vec::new()))
                    .1
                    .push(location);
            }
        }

        // Everything is already satisfied: send no command at all (idempotent
        // no-op), as the brief requires.
        if by_mailbox.is_empty() {
            return Ok(());
        }

        // Reuse ONE session for the whole mutation (the command slot; the
        // 2-connection cap is respected because we lease exactly one).
        let mut session = self.open_session().await?;
        let result = self.apply_flag_mutation_on(session.as_mut(), &plan, by_mailbox).await;
        let _ = session.logout().await;
        result
    }

    /// The flag mutation against an already-open session, factored out so the
    /// session is logged out exactly once regardless of outcome. Processes one
    /// mailbox at a time; a mailbox that succeeds persists its locations' new
    /// flags before the next is attempted, so a partial failure leaves the
    /// succeeded mailboxes consistent and a retry re-derives only the rest.
    async fn apply_flag_mutation_on(
        &self,
        session: &mut dyn ImapSession,
        plan: &FlagPlan,
        by_mailbox: std::collections::BTreeMap<String, (i64, Vec<ImapLocation>)>,
    ) -> ProviderResult<()> {
        for (mailbox, (uidvalidity, locations)) in by_mailbox {
            // The write session must also expose the write trait; the live
            // LeasedSession and the test AsyncImapSession both implement it.
            // Guard UIDVALIDITY (a stale mailbox is transient; nothing sent).
            let write_session = as_write_session(session)?;
            let ctx = crate::provider::imap::write::select_for_write(write_session, &self.store, &mailbox, uidvalidity as u32)
                .await?;

            // The UID set for this mailbox (every location that needs a write;
            // one store op applies to all of them).
            let uids: Vec<u32> = locations.iter().map(|l| l.uid as u32).collect();

            // One verified store per flag op the plan names. `\Seen` and
            // `\Flagged` are independent ops; each is a capability-gated,
            // re-fetched verified store (run 1), so a server that silently
            // ignores or cannot keep a flag fails with the permanent error
            // naming the flag + mailbox.
            if let Some(desired) = plan.seen {
                run_store(write_session, &ctx, &uids, "\\Seen", desired).await?;
            }
            if let Some(desired) = plan.flagged {
                run_store(write_session, &ctx, &uids, "\\Flagged", desired).await?;
            }

            // This mailbox's ops all verified: update each location's stored
            // flags so the next sweep sees no difference (no flap, no spurious
            // journal). Done BEFORE the next mailbox, so a later failure leaves
            // this mailbox's local state already consistent (partial-failure
            // honesty).
            for location in &locations {
                let flags: Vec<String> =
                    serde_json::from_str(&location.flags_json).unwrap_or_default();
                let updated = plan.apply_to(&flags);
                let new_json =
                    serde_json::to_string(&updated).unwrap_or_else(|_| location.flags_json.clone());
                self.store
                    .upsert_location(&ImapLocation {
                        flags_json: new_json,
                        ..location.clone()
                    })
                    .map_err(db_err)?;
            }
        }
        Ok(())
    }
}

/// Run one verified store that forces `flag` to `desired` on a UID set. An
/// `Absent` desired is a `-FLAGS` remove; a `Present` is a `+FLAGS` add. The
/// flag is validated to its wire spelling first (it is a fixed system flag
/// here, so this never rejects), then handed to run 1's capability-gated
/// verified store.
async fn run_store(
    session: &mut dyn crate::provider::imap::write::ImapWriteSession,
    ctx: &crate::provider::imap::write::WriteContext,
    uids: &[u32],
    flag: &str,
    desired: DesiredFlag,
) -> ProviderResult<()> {
    let wire = crate::provider::imap::write::validate_flag(flag)?;
    let op = match desired {
        DesiredFlag::Present => crate::provider::imap::write::StoreOp::Add,
        DesiredFlag::Absent => crate::provider::imap::write::StoreOp::Remove,
    };
    crate::provider::imap::write::verified_store(session, ctx, uids, op, &[wire]).await
}

/// Narrow a `&mut dyn ImapSession` to the write trait. Every `ImapSession`
/// the provider opens (the live `LeasedSession` and the test
/// `AsyncImapSession`) also implements `ImapWriteSession` and overrides
/// `ImapSession::as_write_session` to return `Some(self)`; a read-only session
/// (the test stub) returns `None`, which here is an internal error — the flag
/// path is only ever driven over a write-capable session, so `None` means a
/// programming mistake, surfaced as a non-retryable `Other` rather than a
/// panic.
pub(super) fn as_write_session<'a>(
    session: &'a mut dyn ImapSession,
) -> ProviderResult<&'a mut dyn crate::provider::imap::write::ImapWriteSession> {
    session.as_write_session().ok_or_else(|| {
        ProviderError::Other(
            "internal error: flag mutation driven over a non-write-capable IMAP session".into(),
        )
    })
}

/// A classified `modify_messages` request in label-folder terms: either a flag
/// change (handled by run 2's flag path) or a single `lf:` add/remove.
enum MessageMutationKind {
    /// Only `UNREAD`/`STARRED` ids — the run-2 flag path.
    Flag,
    /// A single `lf:<name>` ADD (`add == true`) or REMOVE (`add == false`).
    Label { id: String, add: bool },
}

/// Classify a `modify_messages` `(add, remove)` request. A request naming no
/// `lf:` id is the flag path (run 2's `plan_flag_mutation` still rejects a
/// `folder:`/system/`kw:`/unknown id or an id in both lists). A request that
/// names exactly ONE `lf:` id — `([lf:X], [])` or `([], [lf:X])`, the two
/// shapes the engine delivers a user-label change in — is the label path.
/// Anything else (a label mixed with another id, several labels, a label in
/// both lists, a label alongside a flag) is a permanent `InvalidOperation`
/// BEFORE any command.
fn classify_message_mutation(
    add: &[String],
    remove: &[String],
) -> ProviderResult<MessageMutationKind> {
    let is_lf = |id: &String| id.starts_with("lf:");
    let any_lf = add.iter().chain(remove.iter()).any(is_lf);
    if !any_lf {
        return Ok(MessageMutationKind::Flag);
    }
    // Exactly one lf: id, alone, in exactly one list.
    match (add, remove) {
        ([only], []) if is_lf(only) => {
            Ok(MessageMutationKind::Label { id: only.clone(), add: true })
        }
        ([], [only]) if is_lf(only) => {
            Ok(MessageMutationKind::Label { id: only.clone(), add: false })
        }
        _ => Err(ProviderError::InvalidOperation(format!(
            "the IMAP provider accepts a user-label change only as a single lf: id added or \
             removed on its own ([lf:X],[]) or ([],[lf:X]); the request add={add:?} remove={remove:?} \
             mixes a label with other ids, names several, or repeats one; refusing permanently"
        ))),
    }
}

#[async_trait]
impl MailMutate for ImapProvider {
    async fn modify_thread(
        &self,
        id: &str,
        add: &[String],
        remove: &[String],
    ) -> ProviderResult<()> {
        // A thread id may be an alias: resolve it, then act on every message
        // of the surviving thread. An unknown/emptied thread resolves to no
        // messages and `apply_flag_mutation` rejects it as a permanent
        // InvalidOperation naming the thread.
        let thread_id = self.store.resolve_thread_alias(id).map_err(db_err)?;
        // `imap_threads` rows are permanent, so a thread still lists messages
        // that were expunged elsewhere. Only messages that still have a
        // location can be changed; skipping the rest keeps one dead sibling
        // from failing (and rolling back) the whole thread-level change.
        let mut message_ids = Vec::new();
        for message_id in self.store.messages_in_thread(&thread_id).map_err(db_err)? {
            if !self
                .store
                .locations_for_message(&message_id)
                .map_err(db_err)?
                .is_empty()
            {
                message_ids.push(message_id);
            }
        }
        if message_ids.is_empty() {
            return Err(ProviderError::InvalidOperation(format!(
                "thread {id:?} has no messages to modify (it may have been emptied); \
                 refusing a flag change permanently"
            )));
        }
        // A `modify_thread` request is EITHER a flag change (UNREAD/STARRED)
        // or a system-label MOVE (archive/trash/spam and their reverses). The
        // engine never mixes the two (`sync.rs::mutation_labels`), so classify
        // the request first and route it; a shape that is neither — a mix, a
        // system id combined with a flag or `lf:`/`folder:`/SENT, or an id in
        // both lists — is a permanent `InvalidOperation` BEFORE any command.
        match classify_thread_mutation(add, remove)? {
            ThreadMutationKind::Flag => self.apply_flag_mutation(&message_ids, add, remove).await,
            ThreadMutationKind::SystemMove(move_plan) => {
                self.apply_system_move(&message_ids, move_plan).await
            }
        }
    }

    async fn modify_messages(
        &self,
        ids: &[String],
        add: &[String],
        remove: &[String],
    ) -> ProviderResult<()> {
        if ids.is_empty() {
            return Err(ProviderError::InvalidOperation(
                "modify_messages called with no message ids".into(),
            ));
        }
        // A `modify_messages` request is EITHER a flag change (UNREAD/STARRED)
        // or a single user-label (`lf:`) add/remove — the two shapes the engine
        // delivers through this method. Classify first (rejecting a mixed /
        // multi-label / folder: / unknown shape permanently BEFORE any
        // command), then route.
        match classify_message_mutation(add, remove)? {
            MessageMutationKind::Flag => self.apply_flag_mutation(ids, add, remove).await,
            MessageMutationKind::Label { id, add } => {
                self.apply_label_mutation(ids, &id, add).await
            }
        }
    }

    async fn list_labels(&self) -> ProviderResult<Vec<Label>> {
        // The six system labels first, in a fixed order, then the account's
        // dynamic labels read from the LOCAL plan/catalog (no network): `lf:`
        // labels (kind `user`, name = path relative to the container) for every
        // label-folder child, then `folder:` labels (kind `folder`, name = full
        // mailbox name) for every user folder — whether or not any message is
        // currently in them (fact 2 / item 3). The frontend resolves label ids
        // through this catalog, so an id absent here renders as raw text.
        let mut labels = vec![
            Label {
                id: "INBOX".into(),
                name: "Inbox".into(),
                kind: "system".into(),
            },
            Label {
                id: "SENT".into(),
                name: "Sent".into(),
                kind: "system".into(),
            },
            Label {
                id: "SPAM".into(),
                name: "Spam".into(),
                kind: "system".into(),
            },
            Label {
                id: "TRASH".into(),
                name: "Trash".into(),
                kind: "system".into(),
            },
            Label {
                id: "UNREAD".into(),
                name: "Unread".into(),
                kind: "system".into(),
            },
            Label {
                id: "STARRED".into(),
                name: "Starred".into(),
                kind: "system".into(),
            },
        ];
        let plan = self.build_sync_plan()?;
        for (id, kind) in plan.dynamic_labels() {
            // Display name: the id with its `lf:` / `folder:` prefix stripped
            // (the relative label path, or the full user-folder name).
            let name = id
                .strip_prefix("lf:")
                .or_else(|| id.strip_prefix("folder:"))
                .unwrap_or(&id)
                .to_string();
            let name = crate::provider::imap::write::decode_modified_utf7(&name).unwrap_or(name);
            labels.push(Label {
                id,
                name,
                kind: kind.to_string(),
            });
        }
        Ok(labels)
    }

    async fn create_label(&self, name: &str) -> ProviderResult<Label> {
        self.create_label_folder(name).await
    }

    async fn update_label(&self, _id: &str, _name: &str) -> ProviderResult<Label> {
        // RENAME is deliberately NOT implemented. The `lf:` label id is derived
        // from the label NAME, so a rename changes the id; but the engine and
        // database have no way to migrate stored label ids: there is no label
        // alias table (unlike thread aliases), and `update_label`'s returned
        // `Label` is handed straight to the frontend, never used to re-point
        // the `threads.labels_json` rows that carry `lf:<old>`. Renaming would
        // therefore silently orphan every message/thread labelled with the old
        // id until each is independently re-ingested. Until a label-id
        // migration exists, refuse permanently with a plain-language reason.
        Err(ProviderError::InvalidOperation(
            "renaming a label is not supported on this account: a label-folder label's id is \
             derived from its name, and there is no safe migration of the label id already stored \
             on your messages and threads, so a rename would detach them from the label. Create a \
             new label and move the messages instead. (Refusing permanently.)"
                .into(),
        ))
    }

    async fn delete_label(&self, id: &str) -> ProviderResult<()> {
        self.delete_label_folder(id).await
    }
}
