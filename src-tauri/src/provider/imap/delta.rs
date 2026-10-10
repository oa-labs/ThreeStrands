//! Pure delta computation for one mailbox — given the local view of a mailbox
//! and the server's current view, work out what changed. No network I/O.
//!
//! `docs/imap-design.md` ("Incremental sync for one mailbox", the "neither"
//! tier). Without CONDSTORE/QRESYNC the client compares the server's state
//! against the local location table every poll:
//!
//! * **UIDVALIDITY reset** — the server's `UIDVALIDITY` differs from what we
//!   stored. Every local UID for the mailbox is now meaningless; the caller
//!   drops that mailbox's locations (keeping cached bodies, which are keyed by
//!   the stable message id) and resyncs the mailbox from scratch. Signalled by
//!   [`MailboxDelta::uidvalidity_reset`].
//! * **New mail** — UIDs the server has that we do not, from the window-
//!   selected `UID SEARCH ALL` result (UIDs `>= uidnext_local` are necessarily
//!   new, but a comparison against the full local set also catches a UID we
//!   never recorded).
//! * **Flag changes** — UIDs present both sides whose flags differ.
//! * **Deletions** — local UIDs expunged by another client or aged out of
//!   the newest-message window.
//!
//! The window ([`policy::select_window`]) is applied to the entire server
//! mailbox so an over-window mailbox syncs only its newest mail. Older UIDs are
//! reported through [`MailboxDelta::beyond_window`], never silently dropped.
//!
//! Everything here is a set comparison over `u32` UIDs and `Vec<String>`
//! flags, so it is exhaustively table-testable with small injected limits and
//! never touches a server.

use std::collections::{BTreeMap, BTreeSet};

use super::policy::{self, SyncLimits};

/// The local view of one mailbox the delta is computed against: the stored
/// `UIDVALIDITY` and, for each known UID, its stored flags.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalMailboxView {
    /// The `UIDVALIDITY` we last recorded for this mailbox. `None` when the
    /// mailbox has never been synced (first sync — everything is new).
    pub uidvalidity: Option<i64>,
    /// `uid -> flags` for every UID we hold locally.
    pub flags_by_uid: BTreeMap<u32, Vec<String>>,
}

/// The server's current view, as the sync routine gathered it from EXAMINE and
/// a `UID SEARCH ALL` + per-UID `FLAGS` fetch over the window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerMailboxView {
    /// The server's current `UIDVALIDITY` (from EXAMINE).
    pub uidvalidity: i64,
    /// Every UID the mailbox currently holds (`UID SEARCH ALL`), unordered.
    pub all_uids: Vec<u32>,
    /// `uid -> flags` for the UIDs whose flags were fetched this round (the
    /// window). A UID absent here is simply not compared for flag changes.
    pub flags_by_uid: BTreeMap<u32, Vec<String>>,
}

/// What one mailbox's delta round found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MailboxDelta {
    /// The server's `UIDVALIDITY` no longer matches ours: drop local
    /// locations for this mailbox and resync it. When true the other UID
    /// lists describe the fresh server state (every in-window UID is "new").
    pub uidvalidity_reset: bool,
    /// New UIDs to fetch and ingest, windowed, ascending.
    pub new_uids: Vec<u32>,
    /// UIDs present both sides whose flags changed, ascending.
    pub flag_changed_uids: Vec<u32>,
    /// Local UIDs expunged elsewhere or now outside the mailbox window,
    /// ascending.
    pub deleted_uids: Vec<u32>,
    /// How many server UIDs fell outside the window (recorded, never dropped
    /// silently).
    pub beyond_window: usize,
}

/// Compute the delta for one mailbox. `limits` carries the window ceiling to
/// apply to the mailbox, so tests pass a tiny limit instead of a 5000-
/// message fixture; production passes the policy constant for the mailbox's
/// class.
pub fn compute_delta(
    local: &LocalMailboxView,
    server: &ServerMailboxView,
    limits: &SyncLimits,
) -> MailboxDelta {
    // UIDVALIDITY reset (or first sync against a mailbox we have never seen
    // but the server reports differently) — treat the whole server view as
    // new, windowed, and drop nothing-else because the caller drops locations.
    let reset = matches!(local.uidvalidity, Some(stored) if stored != server.uidvalidity);
    if reset {
        let selection = policy::select_window(server.all_uids.clone(), limits.mailbox_window);
        return MailboxDelta {
            uidvalidity_reset: true,
            new_uids: selection.in_window,
            flag_changed_uids: Vec::new(),
            deleted_uids: Vec::new(),
            beyond_window: selection.beyond_window,
        };
    }

    let selection = policy::select_window(server.all_uids.clone(), limits.mailbox_window);
    let server_set: BTreeSet<u32> = selection.in_window.into_iter().collect();
    let local_set: BTreeSet<u32> = local.flags_by_uid.keys().copied().collect();

    // Select the mailbox window BEFORE diffing so repeated idle polls cannot
    // backfill older mail, and newly arriving mail ages older locations out.
    let new_uids: Vec<u32> = server_set.difference(&local_set).copied().collect();

    // Deleted: expunged (gone from the server entirely) and, for an EVICTING
    // class, also local UIDs that aged out of the window as newer mail arrived.
    // A NON-evicting class (Sent's acquisition bound) keeps an aged-out UID: it
    // is only deleted when the SERVER no longer has it, so we subtract from the
    // full server UID set, not the windowed one.
    let deleted_uids: Vec<u32> = if limits.evicts {
        local_set.difference(&server_set).copied().collect()
    } else {
        let server_all: BTreeSet<u32> = server.all_uids.iter().copied().collect();
        local_set.difference(&server_all).copied().collect()
    };

    // Flag changes: present both sides, flags differ. Compared only for UIDs
    // whose flags the server actually fetched this round.
    let mut flag_changed_uids = Vec::new();
    for (&uid, server_flags) in &server.flags_by_uid {
        if !server_set.contains(&uid) {
            continue;
        }
        if let Some(local_flags) = local.flags_by_uid.get(&uid) {
            if flags_differ(local_flags, server_flags) {
                flag_changed_uids.push(uid);
            }
        }
    }
    flag_changed_uids.sort_unstable();

    MailboxDelta {
        uidvalidity_reset: false,
        new_uids,
        flag_changed_uids,
        deleted_uids,
        beyond_window: selection.beyond_window,
    }
}

/// Whether two flag lists differ as SETS (order and duplicates do not matter;
/// a server may list flags in any order).
fn flags_differ(a: &[String], b: &[String]) -> bool {
    let sa: BTreeSet<&str> = a.iter().map(String::as_str).collect();
    let sb: BTreeSet<&str> = b.iter().map(String::as_str).collect();
    sa != sb
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::imap::policy::MAX_INBOX_SYNC_MESSAGES;

    fn flags(list: &[&str]) -> Vec<String> {
        list.iter().map(|f| f.to_string()).collect()
    }

    fn local(uidvalidity: Option<i64>, entries: &[(u32, &[&str])]) -> LocalMailboxView {
        LocalMailboxView {
            uidvalidity,
            flags_by_uid: entries.iter().map(|(uid, f)| (*uid, flags(f))).collect(),
        }
    }

    fn server(uidvalidity: i64, all: &[u32], entries: &[(u32, &[&str])]) -> ServerMailboxView {
        ServerMailboxView {
            uidvalidity,
            all_uids: all.to_vec(),
            flags_by_uid: entries.iter().map(|(uid, f)| (*uid, flags(f))).collect(),
        }
    }

    /// A generous window so these cases exercise the diff, not the cap.
    fn wide() -> SyncLimits {
        SyncLimits {
            mailbox_window: MAX_INBOX_SYNC_MESSAGES,
            evicts: true,
        }
    }

    #[test]
    fn first_sync_treats_everything_as_new() {
        let local = local(None, &[]);
        let server = server(5, &[1, 2, 3], &[(1, &["\\Seen"]), (2, &[]), (3, &[])]);
        let delta = compute_delta(&local, &server, &wide());
        assert!(!delta.uidvalidity_reset);
        assert_eq!(delta.new_uids, vec![1, 2, 3]);
        assert!(delta.deleted_uids.is_empty());
        assert!(delta.flag_changed_uids.is_empty());
    }

    #[test]
    fn new_mail_is_detected() {
        let local = local(Some(5), &[(1, &["\\Seen"]), (2, &["\\Seen"])]);
        let server = server(5, &[1, 2, 3, 4], &[]);
        let delta = compute_delta(&local, &server, &wide());
        assert_eq!(delta.new_uids, vec![3, 4]);
        assert!(delta.deleted_uids.is_empty());
    }

    #[test]
    fn a_flag_change_by_another_client_is_detected() {
        let local = local(Some(5), &[(1, &[]), (2, &["\\Seen"])]);
        // Server marked UID 1 \Seen; UID 2 unchanged (set-equal, reordered).
        let server = server(5, &[1, 2], &[(1, &["\\Seen"]), (2, &["\\Seen"])]);
        let delta = compute_delta(&local, &server, &wide());
        assert_eq!(delta.flag_changed_uids, vec![1]);
        assert!(delta.new_uids.is_empty());
        assert!(delta.deleted_uids.is_empty());
    }

    #[test]
    fn flag_order_and_duplicates_do_not_count_as_a_change() {
        let local = local(Some(5), &[(1, &["\\Seen", "\\Flagged"])]);
        let server = server(5, &[1], &[(1, &["\\Flagged", "\\Seen"])]);
        let delta = compute_delta(&local, &server, &wide());
        assert!(delta.flag_changed_uids.is_empty(), "set-equal flags are not a change");
    }

    #[test]
    fn an_expunge_by_another_client_is_detected() {
        let local = local(Some(5), &[(1, &["\\Seen"]), (2, &["\\Seen"]), (3, &[])]);
        let server = server(5, &[1, 3], &[]); // UID 2 expunged
        let delta = compute_delta(&local, &server, &wide());
        assert_eq!(delta.deleted_uids, vec![2]);
    }

    #[test]
    fn a_uidvalidity_change_signals_a_reset_and_treats_the_server_view_as_new() {
        let local = local(Some(5), &[(1, &["\\Seen"]), (2, &[])]);
        let server = server(9, &[10, 11], &[]); // new UIDVALIDITY
        let delta = compute_delta(&local, &server, &wide());
        assert!(delta.uidvalidity_reset);
        assert_eq!(delta.new_uids, vec![10, 11]);
        // On a reset the caller drops locations, so we do not also report
        // deletions/flag-changes.
        assert!(delta.deleted_uids.is_empty());
        assert!(delta.flag_changed_uids.is_empty());
    }

    #[test]
    fn the_window_is_applied_to_the_mailbox_below_at_and_above_the_limit() {
        let limits = SyncLimits { mailbox_window: 3, evicts: true };

        // Below: 2 new, all kept.
        let below = compute_delta(
            &local(Some(1), &[]),
            &server(1, &[1, 2], &[]),
            &limits,
        );
        assert_eq!(below.new_uids, vec![1, 2]);
        assert_eq!(below.beyond_window, 0);

        // Exactly at: 3 new, all kept including the oldest.
        let exact = compute_delta(
            &local(Some(1), &[]),
            &server(1, &[1, 2, 3], &[]),
            &limits,
        );
        assert_eq!(exact.new_uids, vec![1, 2, 3]);
        assert_eq!(exact.beyond_window, 0);

        // Above: 4 new, the OLDEST (UID 1) falls outside, reported.
        let above = compute_delta(
            &local(Some(1), &[]),
            &server(1, &[1, 2, 3, 4], &[]),
            &limits,
        );
        assert_eq!(above.new_uids, vec![2, 3, 4]);
        assert_eq!(above.beyond_window, 1);
    }

    #[test]
    fn exactly_at_the_inbox_ceiling_keeps_every_message() {
        // The exactly-at-ceiling INBOX case the brief calls out, with the real
        // ceiling via a tiny stand-in to avoid a 5000-UID fixture: equal to
        // the window, nothing is beyond it.
        let limits = SyncLimits { mailbox_window: 4, evicts: true };
        let all: Vec<u32> = (1..=4).collect();
        let delta = compute_delta(&local(Some(1), &[]), &server(1, &all, &[]), &limits);
        assert_eq!(delta.new_uids, all);
        assert_eq!(delta.beyond_window, 0);
    }

    /// A non-evicting class (Sent's acquisition bound) does NOT delete a UID
    /// that aged out of the window as newer mail arrived: it is kept until the
    /// SERVER no longer has it. An evicting class in the same situation DOES
    /// delete it.
    #[test]
    fn a_non_evicting_class_keeps_aged_out_uids_an_evicting_one_deletes_them() {
        // Local holds UID 1; the server now has 1,2,3,4 but the window is 3,
        // so UID 1 ages out of the window.
        let local_view = local(Some(5), &[(1, &["\\Seen"])]);
        let server_view = server(5, &[1, 2, 3, 4], &[]);

        // Evicting (INBOX/Folder): UID 1 aged out of the window -> deleted.
        let evicting =
            compute_delta(&local_view, &server_view, &SyncLimits { mailbox_window: 3, evicts: true });
        assert_eq!(evicting.deleted_uids, vec![1], "evicting class deletes the aged-out UID");

        // Non-evicting (Sent): UID 1 is still on the server, so it is KEPT even
        // though it aged out of the acquisition window.
        let sent =
            compute_delta(&local_view, &server_view, &SyncLimits { mailbox_window: 3, evicts: false });
        assert!(
            sent.deleted_uids.is_empty(),
            "Sent keeps an aged-out UID the server still has"
        );

        // But a Sent UID the server genuinely expunged IS deleted.
        let expunged_local = LocalMailboxView {
            uidvalidity: Some(5),
            flags_by_uid: [(99u32, flags(&["\\Seen"]))].into_iter().collect(),
        };
        let expunged = compute_delta(
            &expunged_local,
            &server(5, &[1, 2, 3, 4], &[]),
            &SyncLimits { mailbox_window: 3, evicts: false },
        );
        assert_eq!(
            expunged.deleted_uids,
            vec![99],
            "Sent still deletes a server-expunged UID"
        );
    }

    #[test]
    fn new_flag_change_and_deletion_can_all_happen_in_one_round() {
        let local = local(Some(5), &[(1, &[]), (2, &["\\Seen"]), (3, &["\\Seen"])]);
        // UID 3 expunged, UID 1 now \Seen, UID 4 is new.
        let server = server(5, &[1, 2, 4], &[(1, &["\\Seen"]), (2, &["\\Seen"])]);
        let delta = compute_delta(&local, &server, &wide());
        assert_eq!(delta.new_uids, vec![4]);
        assert_eq!(delta.flag_changed_uids, vec![1]);
        assert_eq!(delta.deleted_uids, vec![3]);
    }
}
