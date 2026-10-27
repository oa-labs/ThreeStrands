//! Pure label construction — turn a message copy's RESOLVED location label ids
//! and IMAP flags into the system/user `label_ids` the rest of the app
//! understands. No I/O.
//!
//! `docs/imap-design.md` ("Labels in the `RawMessage` envelope"). Labels are
//! worked out from WHAT the mailbox a message is stored in contributes — a
//! system role (INBOX / SENT / SPAM / TRASH), a user folder
//! (`folder:<name>`), or a label folder (`lf:<name>`) — plus the copy's flags,
//! never from the mailbox NAME parsed ad hoc. The sync plan (`plan.rs`)
//! resolves each mailbox to the OWNED location label id string it contributes
//! (fact 4: a `Copy` enum of static ids cannot carry a dynamic
//! `folder:<name>`), and this module unions those per-copy ids with the
//! flag-derived state labels. The design table for the location labels:
//!
//! | Mailbox the copy is in         | location `label_ids` entry   |
//! | ------------------------------ | ---------------------------- |
//! | INBOX                          | `INBOX`                      |
//! | `\Sent` role mailbox           | `SENT`                       |
//! | `\Junk` role mailbox           | `SPAM`                       |
//! | `\Trash` role mailbox          | `TRASH`                      |
//! | user folder `Clients/Acme`     | `folder:Clients/Acme`        |
//! | label folder `<container>/Foo` | `lf:Foo`                     |
//! | No `\Seen` flag                | `UNREAD`                     |
//! | `\Flagged`                     | `STARRED`                    |
//!
//! Classifying by the plan's resolved location (user override > RFC 6154
//! attribute > name match for roles; container membership for `lf:`; otherwise
//! a user folder) rather than by an ad-hoc name parse is the AGENTS.md
//! provider-neutral invariant: nothing here branches on a sender, host or
//! brand. The `\Archive` role and the label container contribute NO location
//! label (an archived message is simply not in INBOX; the container itself is
//! not a label).
//!
//! The INBOX->Trash move the brief calls out falls out of this naturally: when
//! a hot message's INBOX location is deleted and a Trash location appears, the
//! union of its per-copy labels is `TRASH` WITHOUT `INBOX`, and the caller
//! journals its thread so the engine re-ingests it — it is never silently
//! "archived".

/// Whether a wire flag list marks the message `\Seen`.
fn is_seen(flags: &[String]) -> bool {
    flags.iter().any(|flag| flag.eq_ignore_ascii_case("\\Seen"))
}

/// Whether a wire flag list marks the message `\Flagged`.
fn is_flagged(flags: &[String]) -> bool {
    flags.iter().any(|flag| flag.eq_ignore_ascii_case("\\Flagged"))
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

/// Build the `label_ids` for one message copy from the OWNED location label
/// ids the plan resolved for the copy's mailbox (zero or more — system
/// `INBOX`/`SENT`/… or dynamic `folder:<name>`/`lf:<name>`) and that copy's
/// flags. This is the owned-string model (fact 4): a `folder:<name>` id can
/// live here because the input is strings, not a `Copy` enum.
///
/// Deterministic order: the location ids first (in the order given), then the
/// state labels (`UNREAD`, `STARRED`) in a fixed order, so two runs produce
/// byte-identical lists. Never produces duplicates. A `SENT` location
/// suppresses `UNREAD` (your own sent mail is not "unread"), exactly as the
/// role model did. A copy with no location id yields only its state labels.
pub fn labels_for_location_ids(location_ids: &[String], flags: &[String]) -> Vec<String> {
    let mut labels: Vec<String> = Vec::new();
    for id in location_ids {
        if !labels.contains(id) {
            labels.push(id.clone());
        }
    }
    // A Sent copy is never "unread".
    let suppress_unread = location_ids.iter().any(|id| id == "SENT");
    if !suppress_unread && !is_seen(flags) && !labels.iter().any(|l| l == "UNREAD") {
        labels.push("UNREAD".to_string());
    }
    if is_flagged(flags) && !labels.iter().any(|l| l == "STARRED") {
        labels.push("STARRED".to_string());
    }
    labels
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flags(list: &[&str]) -> Vec<String> {
        list.iter().map(|f| f.to_string()).collect()
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

    // ----- owned-string model: labels_for_location_ids (Slice 5b-2 fact 4) ---

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn owned_location_ids_prepend_before_state_labels() {
        // INBOX + unseen -> INBOX, UNREAD in fixed order.
        assert_eq!(
            labels_for_location_ids(&ids(&["INBOX"]), &flags(&[])),
            vec!["INBOX", "UNREAD"]
        );
        // \Seen -> no UNREAD; \Flagged -> STARRED last.
        assert_eq!(
            labels_for_location_ids(&ids(&["INBOX"]), &flags(&["\\Seen", "\\Flagged"])),
            vec!["INBOX", "STARRED"]
        );
    }

    #[test]
    fn a_sent_location_id_suppresses_unread_but_not_starred() {
        // SENT, unseen -> no UNREAD (your own sent mail is not unread).
        assert_eq!(labels_for_location_ids(&ids(&["SENT"]), &flags(&[])), vec!["SENT"]);
        // \Flagged still STARRED.
        assert_eq!(
            labels_for_location_ids(&ids(&["SENT"]), &flags(&["\\Flagged"])),
            vec!["SENT", "STARRED"]
        );
    }

    #[test]
    fn dynamic_folder_and_lf_ids_round_trip_verbatim() {
        // A `folder:` id with spaces/colon/non-ASCII and an `lf:` id carry
        // through unchanged, with state labels appended (test (i)).
        assert_eq!(
            labels_for_location_ids(&ids(&["folder:Projektübersicht: Q3"]), &flags(&[])),
            vec!["folder:Projektübersicht: Q3", "UNREAD"]
        );
        assert_eq!(
            labels_for_location_ids(&ids(&["lf:Zoë's: tag"]), &flags(&["\\Seen"])),
            vec!["lf:Zoë's: tag"]
        );
    }

    #[test]
    fn empty_location_ids_yield_only_state_labels() {
        assert_eq!(labels_for_location_ids(&[], &flags(&[])), vec!["UNREAD"]);
        assert_eq!(
            labels_for_location_ids(&[], &flags(&["\\Seen", "\\Flagged"])),
            vec!["STARRED"]
        );
    }

    #[test]
    fn owned_location_ids_never_duplicate() {
        // Duplicate location ids and a pre-present state label do not duplicate.
        assert_eq!(
            labels_for_location_ids(&ids(&["INBOX", "INBOX", "UNREAD"]), &flags(&[])),
            vec!["INBOX", "UNREAD"]
        );
    }

    #[test]
    fn flag_matching_is_case_insensitive_and_ignores_unrelated_flags() {
        assert_eq!(
            labels_for_location_ids(&ids(&["INBOX"]), &flags(&["\\seen", "$Forwarded", "Project"])),
            vec!["INBOX"]
        );
    }

    #[test]
    fn a_trash_copy_without_an_inbox_copy_yields_trash_not_inbox() {
        // The INBOX->Trash move: once the INBOX location is gone the only copy
        // is in Trash, so the union is TRASH, never INBOX.
        let trash_only =
            merge_label_sets(&[labels_for_location_ids(&ids(&["TRASH"]), &flags(&["\\Seen"]))]);
        assert_eq!(trash_only, vec!["TRASH"]);
    }
}
