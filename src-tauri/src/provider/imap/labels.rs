//! Pure label construction — turn a message's mailbox + IMAP flags into the
//! system `label_ids` the rest of the app understands. No I/O.
//!
//! `docs/imap-design.md` ("Labels in the `RawMessage` envelope"). Labels are
//! worked out from WHERE a message is stored and its flags. This slice (5a)
//! covers INBOX only; the full mapping (SENT / SPAM / TRASH / `folder:` /
//! `lf:` / `kw:`) is Slice 5b. The design table, for the states this slice
//! produces:
//!
//! | IMAP state                | `label_ids` entry |
//! | ------------------------- | ----------------- |
//! | Message is in `INBOX`     | `INBOX`           |
//! | No `\Seen` flag           | `UNREAD`          |
//! | `\Flagged`                | `STARRED`         |
//!
//! The function is written so Slice 5b adds the remaining mailbox roles and
//! the user-label kinds by extending the mailbox->label mapping and the flag
//! rules, WITHOUT restructuring: a mailbox maps to zero or more location
//! labels through [`MailboxLabel`], and flags add state labels on top. INBOX
//! is simply the one `MailboxLabel::Inbox` case wired this slice.

/// Which system label a mailbox contributes by virtue of a message being
/// stored in it. Only [`MailboxLabel::Inbox`] is produced this slice; the
/// other roles are reserved so Slice 5b fills them in without changing the
/// call shape. A mailbox that contributes no location label (an aggregate or
/// a `\Noselect` container) is simply never classified as one of these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MailboxLabel {
    /// INBOX -> `INBOX`.
    Inbox,
    // Slice 5b: Sent -> SENT, Junk -> SPAM, Trash -> TRASH, user folders ->
    // folder:<name>, label folders -> lf:<name>.
}

impl MailboxLabel {
    /// The canonical system label id a mailbox role contributes.
    fn label_id(self) -> &'static str {
        match self {
            Self::Inbox => "INBOX",
        }
    }

    /// Classify a mailbox by name/special-use into a location label, if it is
    /// one this slice knows. INBOX is matched case-insensitively (RFC 3501
    /// reserves the name). Everything else returns `None` this slice — Slice
    /// 5b extends this to special-use roles and user folders.
    pub fn classify(mailbox_name: &str, _special_use: Option<&str>) -> Option<Self> {
        if mailbox_name.eq_ignore_ascii_case("INBOX") {
            Some(Self::Inbox)
        } else {
            None
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

/// Build the system `label_ids` for one message copy from the mailbox it is in
/// and that copy's flags. Deterministic order: the location label first, then
/// state labels (`UNREAD`, `STARRED`) in a fixed order so two runs produce
/// byte-identical lists. Never produces duplicates.
///
/// This slice: an INBOX copy yields `INBOX`; the absence of `\Seen` adds
/// `UNREAD`; `\Flagged` adds `STARRED`. A copy in a mailbox this slice does
/// not classify (anything but INBOX) yields only its state labels — which is
/// exactly the forward-compatible shape Slice 5b extends.
pub fn labels_for(mailbox_name: &str, special_use: Option<&str>, flags: &[String]) -> Vec<String> {
    let mut labels = Vec::new();
    if let Some(location) = MailboxLabel::classify(mailbox_name, special_use) {
        labels.push(location.label_id().to_string());
    }
    // State labels come from flags, independent of mailbox.
    if !is_seen(flags) {
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

    fn flags(list: &[&str]) -> Vec<String> {
        list.iter().map(|f| f.to_string()).collect()
    }

    #[test]
    fn an_inbox_message_carries_the_inbox_label() {
        assert_eq!(labels_for("INBOX", None, &flags(&["\\Seen"])), vec!["INBOX"]);
        // Case-insensitive mailbox name match.
        assert_eq!(labels_for("inbox", None, &flags(&["\\Seen"])), vec!["INBOX"]);
    }

    #[test]
    fn an_unseen_message_is_unread() {
        // No \Seen flag -> UNREAD; order is INBOX then UNREAD.
        assert_eq!(labels_for("INBOX", None, &flags(&[])), vec!["INBOX", "UNREAD"]);
        // \Seen present -> no UNREAD.
        assert_eq!(labels_for("INBOX", None, &flags(&["\\Seen"])), vec!["INBOX"]);
    }

    #[test]
    fn a_flagged_message_is_starred() {
        assert_eq!(
            labels_for("INBOX", None, &flags(&["\\Seen", "\\Flagged"])),
            vec!["INBOX", "STARRED"]
        );
        // Unseen AND flagged: INBOX, UNREAD, STARRED, in that fixed order.
        assert_eq!(
            labels_for("INBOX", None, &flags(&["\\Flagged"])),
            vec!["INBOX", "UNREAD", "STARRED"]
        );
    }

    #[test]
    fn flag_matching_is_case_insensitive_and_ignores_unrelated_flags() {
        assert_eq!(
            labels_for("INBOX", None, &flags(&["\\seen", "$Forwarded", "Project"])),
            vec!["INBOX"]
        );
    }

    #[test]
    fn a_non_inbox_mailbox_contributes_no_location_label_this_slice() {
        // Slice 5a is INBOX-only: a Sent/Archive copy yields only state
        // labels, never a location label, and never panics on special-use.
        assert_eq!(labels_for("Sent", Some("\\Sent"), &flags(&[])), vec!["UNREAD"]);
        assert_eq!(
            labels_for("Archive", Some("\\Archive"), &flags(&["\\Seen", "\\Flagged"])),
            vec!["STARRED"]
        );
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

    #[test]
    fn classification_is_extensible_without_touching_callers() {
        // Only INBOX is a known location label this slice; anything else is
        // deliberately unclassified so Slice 5b can add roles here alone.
        assert_eq!(MailboxLabel::classify("INBOX", None), Some(MailboxLabel::Inbox));
        assert_eq!(MailboxLabel::classify("Sent", Some("\\Sent")), None);
        assert_eq!(MailboxLabel::classify("Clients/Acme", None), None);
    }
}
