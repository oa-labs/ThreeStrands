//! Pure label construction — turn a message copy's RESOLVED mailbox role and
//! IMAP flags into the system `label_ids` the rest of the app understands. No
//! I/O.
//!
//! `docs/imap-design.md` ("Labels in the `RawMessage` envelope"). Labels are
//! worked out from WHAT ROLE the mailbox a message is stored in plays, and the
//! copy's flags — never from the mailbox NAME. Slice 5a covered INBOX only;
//! Slice 5b-1 adds Sent/Junk/Trash. The design table for the roles this slice
//! produces:
//!
//! | Resolved role / flag      | `label_ids` entry |
//! | ------------------------- | ----------------- |
//! | INBOX                     | `INBOX`           |
//! | Sent (`\Sent`)            | `SENT`            |
//! | Junk (`\Junk`)            | `SPAM`            |
//! | Trash (`\Trash`)          | `TRASH`           |
//! | No `\Seen` flag           | `UNREAD`          |
//! | `\Flagged`                | `STARRED`         |
//!
//! Classifying by ROLE (resolved once by the sync plan via user override >
//! RFC 6154 attribute > name match) rather than by name is the AGENTS.md
//! provider-neutral invariant: nothing here branches on a sender, host or
//! brand, and a mailbox literally named "Trash" that the plan did not resolve
//! to the Trash role contributes NO location label.
//!
//! The INBOX->Trash move the brief calls out falls out of this naturally: when
//! a hot message's INBOX location is deleted and a Trash location appears, the
//! union of its per-copy labels is `TRASH` WITHOUT `INBOX`, and the caller
//! journals its thread so the engine re-ingests it — it is never silently
//! "archived".

/// Which system label a mailbox contributes by virtue of a message being
/// stored in it, keyed by the mailbox's RESOLVED role. A mailbox that
/// contributes no location label (an aggregate `\All`/`\Flagged`, Drafts, or a
/// `\Noselect` container) is simply never one of these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MailboxLabel {
    /// INBOX -> `INBOX`.
    Inbox,
    /// `\Sent` -> `SENT`.
    Sent,
    /// `\Junk` -> `SPAM`.
    Junk,
    /// `\Trash` -> `TRASH`.
    Trash,
    // Slice 5b-2: user folders -> folder:<name>, label folders -> lf:<name>.
}

impl MailboxLabel {
    /// The canonical system label id a mailbox role contributes.
    pub fn label_id(self) -> &'static str {
        match self {
            Self::Inbox => "INBOX",
            Self::Sent => "SENT",
            Self::Junk => "SPAM",
            Self::Trash => "TRASH",
        }
    }

    /// Whether this is the Sent role. A Sent-role copy never contributes
    /// `UNREAD` (your own sent mail is not "unread").
    fn is_sent(self) -> bool {
        matches!(self, Self::Sent)
    }

    /// The location label a resolved [`MailboxRole`](super::MailboxRole)
    /// contributes, if any. `\Archive`, `\All` (aggregate), `\Drafts` and the
    /// label container contribute none this slice.
    pub fn for_role(role: super::MailboxRole) -> Option<Self> {
        use super::MailboxRole;
        match role {
            MailboxRole::Sent => Some(Self::Sent),
            MailboxRole::Junk => Some(Self::Junk),
            MailboxRole::Trash => Some(Self::Trash),
            MailboxRole::Archive | MailboxRole::Drafts | MailboxRole::All => None,
        }
    }
}

/// Whether a wire flag list marks the message `\Seen`.
fn is_seen(flags: &[String]) -> bool {
    flags.iter().any(|flag| flag.eq_ignore_ascii_case("\\Seen"))
}

/// Whether a wire flag list marks the message `\Flagged`.
fn is_flagged(flags: &[String]) -> bool {
    flags.iter().any(|flag| flag.eq_ignore_ascii_case("\\Flagged"))
}

/// Build the system `label_ids` for one message copy from the mailbox's
/// RESOLVED label (if any) and that copy's flags. Deterministic order: the
/// location label first, then state labels (`UNREAD`, `STARRED`) in a fixed
/// order so two runs produce byte-identical lists. Never produces duplicates.
///
/// A Sent-role copy never contributes `UNREAD`. `STARRED` comes from `\Flagged`
/// on any copy. A copy whose role contributes no location label (Archive,
/// Drafts, an aggregate, or an unresolved mailbox) yields only its state
/// labels — the forward-compatible shape Slice 5b-2 extends.
pub fn labels_for(location: Option<MailboxLabel>, flags: &[String]) -> Vec<String> {
    let mut labels = Vec::new();
    if let Some(location) = location {
        labels.push(location.label_id().to_string());
    }
    // A Sent copy is never "unread".
    let suppress_unread = location.map(MailboxLabel::is_sent).unwrap_or(false);
    if !suppress_unread && !is_seen(flags) {
        labels.push("UNREAD".to_string());
    }
    if is_flagged(flags) {
        labels.push("STARRED".to_string());
    }
    labels
}

/// The union of labels across every copy of a message, so a message with
/// locations in several mailboxes carries all their labels (one message,
/// several labels). De-duplicated; order is the first-seen order across the
/// supplied per-copy lists, which is deterministic for a sorted location
/// input.
pub fn merge_label_sets(per_copy: &[Vec<String>]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for copy in per_copy {
        for label in copy {
            if !out.contains(label) {
                out.push(label.clone());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::imap::MailboxRole;

    fn flags(list: &[&str]) -> Vec<String> {
        list.iter().map(|f| f.to_string()).collect()
    }

    #[test]
    fn an_inbox_message_carries_the_inbox_label() {
        assert_eq!(
            labels_for(Some(MailboxLabel::Inbox), &flags(&["\\Seen"])),
            vec!["INBOX"]
        );
    }

    #[test]
    fn an_unseen_message_is_unread() {
        // No \Seen flag -> UNREAD; order is INBOX then UNREAD.
        assert_eq!(
            labels_for(Some(MailboxLabel::Inbox), &flags(&[])),
            vec!["INBOX", "UNREAD"]
        );
        // \Seen present -> no UNREAD.
        assert_eq!(
            labels_for(Some(MailboxLabel::Inbox), &flags(&["\\Seen"])),
            vec!["INBOX"]
        );
    }

    #[test]
    fn a_flagged_message_is_starred() {
        assert_eq!(
            labels_for(Some(MailboxLabel::Inbox), &flags(&["\\Seen", "\\Flagged"])),
            vec!["INBOX", "STARRED"]
        );
        // Unseen AND flagged: INBOX, UNREAD, STARRED, in that fixed order.
        assert_eq!(
            labels_for(Some(MailboxLabel::Inbox), &flags(&["\\Flagged"])),
            vec!["INBOX", "UNREAD", "STARRED"]
        );
    }

    #[test]
    fn sent_junk_and_trash_roles_map_to_their_system_labels() {
        assert_eq!(labels_for(Some(MailboxLabel::Sent), &flags(&["\\Seen"])), vec!["SENT"]);
        assert_eq!(labels_for(Some(MailboxLabel::Junk), &flags(&["\\Seen"])), vec!["SPAM"]);
        assert_eq!(labels_for(Some(MailboxLabel::Trash), &flags(&["\\Seen"])), vec!["TRASH"]);
    }

    #[test]
    fn a_sent_copy_is_never_unread_but_can_be_starred() {
        // No \Seen, but a Sent copy must NOT be UNREAD.
        assert_eq!(labels_for(Some(MailboxLabel::Sent), &flags(&[])), vec!["SENT"]);
        // \Flagged still produces STARRED on a Sent copy.
        assert_eq!(
            labels_for(Some(MailboxLabel::Sent), &flags(&["\\Flagged"])),
            vec!["SENT", "STARRED"]
        );
    }

    #[test]
    fn a_trash_copy_without_an_inbox_copy_yields_trash_not_inbox() {
        // The INBOX->Trash move: once the INBOX location is gone the only copy
        // is in Trash, so the union is TRASH, never INBOX.
        let per_copy = vec![labels_for(Some(MailboxLabel::Trash), &flags(&["\\Seen"]))];
        assert_eq!(merge_label_sets(&per_copy), vec!["TRASH"]);
    }

    #[test]
    fn flag_matching_is_case_insensitive_and_ignores_unrelated_flags() {
        assert_eq!(
            labels_for(Some(MailboxLabel::Inbox), &flags(&["\\seen", "$Forwarded", "Project"])),
            vec!["INBOX"]
        );
    }

    #[test]
    fn an_unclassified_mailbox_contributes_no_location_label() {
        // A copy whose role contributes no location label (Archive/Drafts/an
        // aggregate / an unresolved mailbox) yields only state labels.
        assert_eq!(labels_for(None, &flags(&[])), vec!["UNREAD"]);
        assert_eq!(labels_for(None, &flags(&["\\Seen", "\\Flagged"])), vec!["STARRED"]);
    }

    #[test]
    fn role_to_label_mapping_covers_the_synced_roles_and_skips_the_rest() {
        assert_eq!(MailboxLabel::for_role(MailboxRole::Sent), Some(MailboxLabel::Sent));
        assert_eq!(MailboxLabel::for_role(MailboxRole::Junk), Some(MailboxLabel::Junk));
        assert_eq!(MailboxLabel::for_role(MailboxRole::Trash), Some(MailboxLabel::Trash));
        assert_eq!(MailboxLabel::for_role(MailboxRole::Archive), None);
        assert_eq!(MailboxLabel::for_role(MailboxRole::Drafts), None);
        assert_eq!(MailboxLabel::for_role(MailboxRole::All), None);
    }

    #[test]
    fn label_sets_merge_without_duplicates_across_copies() {
        let per_copy = vec![
            vec!["INBOX".to_string(), "UNREAD".to_string()],
            vec!["STARRED".to_string(), "UNREAD".to_string()],
        ];
        assert_eq!(
            merge_label_sets(&per_copy),
            vec!["INBOX", "UNREAD", "STARRED"]
        );
    }
}
