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
/// a `UID SEARCH ALL` + per-UID `FLAGS` fetch over the acquisition window
/// and any retained locations outside it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerMailboxView {
    /// The server's current `UIDVALIDITY` (from EXAMINE).
    pub uidvalidity: i64,
    /// Every UID the mailbox currently holds (`UID SEARCH ALL`), unordered.
    pub all_uids: Vec<u32>,
    /// `uid -> flags` for the UIDs whose flags were fetched this round (the
    /// acquisition window and retained UIDs). An absent UID is not compared.
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
///
/// `pinned` is the set of SEARCH-PINNED UIDs for this `(mailbox, uidvalidity)`
/// (schema v70, Phase 5 run 3). It is passed IN rather than queried so this
/// stays pure and table-testable. A pinned UID is a search-acquired location
/// that is usually OLD — outside the newest-N window of an evicting mailbox —
/// and must NOT be aged out: it is treated as retained exactly like a
/// non-evicting mailbox's aged-out UID, so it is a DELETION only when the
/// SERVER no longer has it (expunged elsewhere), never merely for falling
/// outside the window. Pinned UIDs still on the server also have their flags
/// refreshed (the caller extends `flags_by_uid` to cover them, the same way it
/// extends coverage for a non-evicting mailbox's retained UIDs). On a
/// UIDVALIDITY reset the pins are meaningless (the caller drops them with the
/// locations), so an empty set is passed and the reset path ignores it.
pub fn compute_delta(
    local: &LocalMailboxView,
    server: &ServerMailboxView,
    limits: &SyncLimits,
    pinned: &BTreeSet<u32>,
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
    //
    // SEARCH PINS (v70): a pinned UID is retained the same way — kept while the
    // server still has it, deleted only when the server does not. So for an
    // evicting mailbox the retained set is the windowed set PLUS any pinned UID
    // the server still holds; a pinned UID the server expunged is NOT in that
    // union and so IS reported deleted (the caller then drops its pin too).
    let live_server: BTreeSet<u32> = server.all_uids.iter().copied().collect();
    let retained_set: BTreeSet<u32> = if limits.evicts {
        let mut retained = server_set.clone();
        retained.extend(pinned.intersection(&live_server).copied());
        retained
    } else {
        live_server.clone()
    };
    let deleted_uids: Vec<u32> = local_set.difference(&retained_set).copied().collect();

    // Flag changes: present both sides, flags differ. Compared only for UIDs
    // whose flags the server actually fetched this round.
    let mut flag_changed_uids = Vec::new();
    for (&uid, server_flags) in &server.flags_by_uid {
        if !retained_set.contains(&uid) {
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

    /// No search pins — the common case for the pre-v70 delta cases.
    fn no_pins() -> BTreeSet<u32> {
        BTreeSet::new()
    }

    /// A pinned-UID set from a slice, for the v70 pin cases.
    fn pins(uids: &[u32]) -> BTreeSet<u32> {
        uids.iter().copied().collect()
    }

    #[test]
    fn first_sync_treats_everything_as_new() {
        let local = local(None, &[]);
        let server = server(5, &[1, 2, 3], &[(1, &["\\Seen"]), (2, &[]), (3, &[])]);
        let delta = compute_delta(&local, &server, &wide(), &no_pins());
        assert!(!delta.uidvalidity_reset);
        assert_eq!(delta.new_uids, vec![1, 2, 3]);
        assert!(delta.deleted_uids.is_empty());
        assert!(delta.flag_changed_uids.is_empty());
    }

    #[test]
    fn new_mail_is_detected() {
        let local = local(Some(5), &[(1, &["\\Seen"]), (2, &["\\Seen"])]);
        let server = server(5, &[1, 2, 3, 4], &[]);
        let delta = compute_delta(&local, &server, &wide(), &no_pins());
        assert_eq!(delta.new_uids, vec![3, 4]);
        assert!(delta.deleted_uids.is_empty());
    }

    #[test]
    fn a_flag_change_by_another_client_is_detected() {
        let local = local(Some(5), &[(1, &[]), (2, &["\\Seen"])]);
        // Server marked UID 1 \Seen; UID 2 unchanged (set-equal, reordered).
        let server = server(5, &[1, 2], &[(1, &["\\Seen"]), (2, &["\\Seen"])]);
        let delta = compute_delta(&local, &server, &wide(), &no_pins());
        assert_eq!(delta.flag_changed_uids, vec![1]);
        assert!(delta.new_uids.is_empty());
        assert!(delta.deleted_uids.is_empty());
    }

    #[test]
    fn flag_order_and_duplicates_do_not_count_as_a_change() {
        let local = local(Some(5), &[(1, &["\\Seen", "\\Flagged"])]);
        let server = server(5, &[1], &[(1, &["\\Flagged", "\\Seen"])]);
        let delta = compute_delta(&local, &server, &wide(), &no_pins());
        assert!(delta.flag_changed_uids.is_empty(), "set-equal flags are not a change");
    }

    #[test]
    fn an_expunge_by_another_client_is_detected() {
        let local = local(Some(5), &[(1, &["\\Seen"]), (2, &["\\Seen"]), (3, &[])]);
        let server = server(5, &[1, 3], &[]); // UID 2 expunged
        let delta = compute_delta(&local, &server, &wide(), &no_pins());
        assert_eq!(delta.deleted_uids, vec![2]);
    }

    #[test]
    fn a_uidvalidity_change_signals_a_reset_and_treats_the_server_view_as_new() {
        let local = local(Some(5), &[(1, &["\\Seen"]), (2, &[])]);
        let server = server(9, &[10, 11], &[]); // new UIDVALIDITY
        let delta = compute_delta(&local, &server, &wide(), &no_pins());
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
            &no_pins(),
        );
        assert_eq!(below.new_uids, vec![1, 2]);
        assert_eq!(below.beyond_window, 0);

        // Exactly at: 3 new, all kept including the oldest.
        let exact = compute_delta(
            &local(Some(1), &[]),
            &server(1, &[1, 2, 3], &[]),
            &limits,
            &no_pins(),
        );
        assert_eq!(exact.new_uids, vec![1, 2, 3]);
        assert_eq!(exact.beyond_window, 0);

        // Above: 4 new, the OLDEST (UID 1) falls outside, reported.
        let above = compute_delta(
            &local(Some(1), &[]),
            &server(1, &[1, 2, 3, 4], &[]),
            &limits,
            &no_pins(),
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
        let delta = compute_delta(&local(Some(1), &[]), &server(1, &all, &[]), &limits, &no_pins());
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
            compute_delta(&local_view, &server_view, &SyncLimits { mailbox_window: 3, evicts: true }, &no_pins());
        assert_eq!(evicting.deleted_uids, vec![1], "evicting class deletes the aged-out UID");

        // Non-evicting (Sent): UID 1 is still on the server, so it is KEPT even
        // though it aged out of the acquisition window.
        let sent =
            compute_delta(&local_view, &server_view, &SyncLimits { mailbox_window: 3, evicts: false }, &no_pins());
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
            &no_pins(),
        );
        assert_eq!(
            expunged.deleted_uids,
            vec![99],
            "Sent still deletes a server-expunged UID"
        );
    }

    #[test]
    fn retained_flag_coverage_respects_eviction_expunges_and_uidvalidity() {
        let local_view = local(Some(5), &[(1, &["\\Seen"]), (2, &["\\Seen"])]);
        // UID 1 aged out, UID 2 was expunged, UID 3 was never acquired. Flags
        // returned for a vanished UID must not resurrect it.
        let server_view = server(
            5,
            &[1, 3, 4, 5],
            &[
                (1, &["\\Flagged"]),
                (2, &["\\Flagged"]),
                (3, &["\\Flagged"]),
            ],
        );
        for evicts in [false, true] {
            let limits = SyncLimits {
                mailbox_window: 2,
                evicts,
            };
            let delta = compute_delta(&local_view, &server_view, &limits, &no_pins());
            assert_eq!(delta.new_uids, vec![4, 5], "acquisition stays windowed");
            assert_eq!(
                delta.flag_changed_uids,
                if evicts { vec![] } else { vec![1] }
            );
            assert_eq!(
                delta.deleted_uids,
                if evicts { vec![1, 2] } else { vec![2] }
            );
            let reset = compute_delta(
                &LocalMailboxView {
                    uidvalidity: Some(9),
                    ..local_view.clone()
                },
                &server_view,
                &limits,
            &no_pins(),
            );
            assert!(reset.uidvalidity_reset);
            assert!(
                reset.flag_changed_uids.is_empty(),
                "old UIDs do not survive a reset"
            );
            assert_eq!(reset.new_uids, vec![4, 5]);
        }
    }

    #[test]
    fn new_flag_change_and_deletion_can_all_happen_in_one_round() {
        let local = local(Some(5), &[(1, &[]), (2, &["\\Seen"]), (3, &["\\Seen"])]);
        // UID 3 expunged, UID 1 now \Seen, UID 4 is new.
        let server = server(5, &[1, 2, 4], &[(1, &["\\Seen"]), (2, &["\\Seen"])]);
        let delta = compute_delta(&local, &server, &wide(), &no_pins());
        assert_eq!(delta.new_uids, vec![4]);
        assert_eq!(delta.flag_changed_uids, vec![1]);
        assert_eq!(delta.deleted_uids, vec![3]);
    }

    // --- Phase 5 run 3: search-pin delta cases -------------------------

    /// A pinned UID that is IN the window behaves like any windowed UID: a pin
    /// does not make it "new" or "deleted"; it is simply retained, and its
    /// flags are compared like any other windowed UID.
    #[test]
    fn a_pinned_uid_in_the_window_is_retained_and_its_flags_are_compared() {
        let limits = SyncLimits { mailbox_window: 3, evicts: true };
        let local = local(Some(5), &[(2, &[]), (3, &["\\Seen"]), (4, &["\\Seen"])]);
        // UID 2 is pinned AND in-window; its flag changed on the server.
        let server = server(
            5,
            &[2, 3, 4],
            &[(2, &["\\Seen"]), (3, &["\\Seen"]), (4, &["\\Seen"])],
        );
        let delta = compute_delta(&local, &server, &limits, &pins(&[2]));
        assert!(delta.deleted_uids.is_empty(), "an in-window pinned UID is not deleted");
        assert_eq!(delta.flag_changed_uids, vec![2], "its flags are still refreshed");
        assert!(delta.new_uids.is_empty());
    }

    /// A pinned UID OUTSIDE the window of an EVICTING mailbox is NOT deleted
    /// merely for being out of window — the pin keeps it alive — whereas an
    /// identical UNpinned out-of-window local UID in the same mailbox IS
    /// deleted (the sliding-window eviction).
    #[test]
    fn a_pinned_out_of_window_uid_survives_but_an_unpinned_one_is_evicted() {
        // Window 2; local holds UID 1 (old) and 2 (old), server has 1..=4, so
        // 1 and 2 are both outside the newest-2 window (3,4).
        let limits = SyncLimits { mailbox_window: 2, evicts: true };
        let local = local(Some(5), &[(1, &["\\Seen"]), (2, &["\\Seen"])]);
        let server = server(5, &[1, 2, 3, 4], &[]);

        // UID 1 is pinned, UID 2 is not. UID 1 survives; UID 2 is evicted.
        let delta = compute_delta(&local, &server, &limits, &pins(&[1]));
        assert_eq!(
            delta.deleted_uids,
            vec![2],
            "the pinned out-of-window UID survives; the unpinned one is evicted"
        );
        assert_eq!(delta.new_uids, vec![3, 4], "the newest two are acquired");
    }

    /// A pinned UID is DELETED when the server no longer has it (expunged
    /// elsewhere), exactly like a non-evicting mailbox's retained UID. The pin
    /// does not keep a UID the server itself dropped.
    #[test]
    fn a_pinned_uid_the_server_expunged_is_deleted() {
        let limits = SyncLimits { mailbox_window: 2, evicts: true };
        // Local holds pinned UID 1 (out of window). The server no longer has
        // UID 1 at all (expunged elsewhere).
        let local = local(Some(5), &[(1, &["\\Seen"])]);
        let server = server(5, &[2, 3, 4], &[]);
        let delta = compute_delta(&local, &server, &limits, &pins(&[1]));
        assert_eq!(
            delta.deleted_uids,
            vec![1],
            "a pinned UID the server expunged is still deleted"
        );
    }

    /// On a UIDVALIDITY reset the pins are meaningless (the caller drops them
    /// with the mailbox's locations). The reset path ignores the pinned set:
    /// the whole windowed server view is new, nothing is reported deleted.
    #[test]
    fn a_uidvalidity_reset_ignores_pins() {
        let limits = SyncLimits { mailbox_window: 3, evicts: true };
        let local = local(Some(5), &[(1, &["\\Seen"]), (2, &[])]);
        let server = server(9, &[10, 11], &[]); // new UIDVALIDITY
        // Even if UID 1 were "pinned" under the old incarnation, the reset
        // path drops it; the delta reports only the fresh server view as new.
        let delta = compute_delta(&local, &server, &limits, &pins(&[1]));
        assert!(delta.uidvalidity_reset);
        assert_eq!(delta.new_uids, vec![10, 11]);
        assert!(delta.deleted_uids.is_empty());
        assert!(delta.flag_changed_uids.is_empty());
    }
}
