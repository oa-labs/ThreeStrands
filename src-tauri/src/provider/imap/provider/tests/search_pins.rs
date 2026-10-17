use super::*;

use crate::provider::{LabelModel, MailSync};

use crate::provider::imap::fetch::BodyCache;
use crate::provider::imap::plan::{self};
use crate::provider::imap::session::ImapSession;
use crate::provider::imap::ImapStateStore;

/// A provider whose non-INBOX window ceiling is `window`, so an
/// acquired UID older than the newest-`window` is out of window and
/// would be evicted on the next sweep without a pin.
fn provider_window(
    mailbox: Arc<Mutex<FakeMailbox>>,
    window: usize,
) -> (ImapProvider, ImapStateStore) {
    let (mut provider, store) = provider_with(mailbox);
    provider.folder_window_override = Some(window);
    (provider, store)
}

/// Open a bare scripted session over the CURRENT shared mailbox so a
/// test can call `acquire_uids` directly, the way run 4 will from
/// inside a sync round.
async fn session_for(mailbox: &Arc<Mutex<FakeMailbox>>) -> Box<dyn ImapSession> {
    let snapshot = mailbox.lock().unwrap().clone();
    open_scripted_session(snapshot).await
}

/// The plan entry for `mailbox` (role resolved from the catalog), or an
/// INBOX entry when asked for INBOX.
fn entry_for(provider: &ImapProvider, mailbox: &str) -> plan::PlanEntry {
    let plan = provider.build_sync_plan().expect("plan");
    plan.entries
        .iter()
        .find(|e| e.mailbox == mailbox)
        .cloned()
        .unwrap_or_else(|| panic!("no plan entry for {mailbox}"))
}

/// Thread ids currently listed from the local store for a mailbox (the
/// DB thread list, not the location table).
fn threads_in(store: &ImapStateStore, mailbox: &str) -> Vec<String> {
    store.thread_ids_in_mailbox(mailbox).unwrap()
}

/// (required) An acquired OLD UID in an EVICTING mailbox (Archive)
/// survives: a full sweep, a window shift from new mail, a cadence-gated
/// no-op poll, and a flag-only change (whose new flags are picked up).
/// Its thread stays listed via the DB thread list after each sync.
#[tokio::test]
async fn an_acquired_old_uid_survives_sweeps_window_shifts_and_flag_changes() {
    use std::sync::atomic::{AtomicI64, Ordering};
    // A mutable clock so step 4 can advance past the folder-sweep
    // interval and let the periodic sweep refresh the pinned message's
    // flags — a flag-only change moves no counter, so (as for ANY
    // folder message) it is the periodic sweep, not an immediate poll,
    // that catches it.
    static CLOCK: AtomicI64 = AtomicI64::new(1_700_000_000);
    fn clock() -> i64 {
        CLOCK.load(Ordering::Relaxed)
    }

    // Archive with 3 messages; window 1, so only the newest (uid 3) is
    // in-window. We acquire the OLD uid 1 and pin it.
    let mut mb = FakeMailbox::new(100);
    mb.add_to("Archive", 50, &["\\Seen"], &message("<a1@x>", "Old one", ""));
    mb.add_to("Archive", 50, &["\\Seen"], &message("<a2@x>", "Old two", ""));
    mb.add_to("Archive", 50, &["\\Seen"], &message("<a3@x>", "New three", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (mut provider, store) = provider_window(mailbox.clone(), 1);
    provider.now = clock;
    CLOCK.store(1_700_000_000, Ordering::Relaxed);
    let db = store.database();

    // Acquire the OLD uid 1 (out of the window-1).
    let archive = entry_for(&provider, "Archive");
    let mut session = session_for(&mailbox).await;
    let outcome = provider
        .acquire_uids(session.as_mut(), &archive, &[1], 50)
        .await
        .expect("acquire");
    let _ = session.logout().await;
    assert_eq!(outcome.skipped_over_cap, 0);
    assert_eq!(outcome.affected_threads.len(), 1, "one thread acquired");
    let acquired_thread = outcome.affected_threads[0].clone();

    // The acquired location + pin exist.
    assert_eq!(store.locations_in_mailbox("Archive").unwrap().len(), 1);
    assert_eq!(store.pinned_uids("Archive", 50).unwrap(), vec![1]);
    assert!(threads_in(&store, "Archive").contains(&acquired_thread));

    // 1) A full sweep (normal sync) — uid 1 is out of window but pinned,
    // so it is NOT evicted. (The window-1 sweep also acquires the newest
    // in-window UID, so the count grows; assert uid 1 specifically.)
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
        .await
        .unwrap();
    assert!(
        store
            .locations_in_mailbox("Archive")
            .unwrap()
            .iter()
            .any(|l| l.uid == 1),
        "pinned old UID survives a full sweep"
    );
    assert!(threads_in(&store, "Archive").contains(&acquired_thread));

    // 2) New mail arrives (window shifts): add uid 4. The window-1 now
    // covers only uid 4; uid 1 is still pinned and survives.
    mailbox
        .lock()
        .unwrap()
        .add_to("Archive", 50, &["\\Seen"], &message("<a4@x>", "Newest", ""));
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
        .await
        .unwrap();
    assert!(
        store
            .locations_in_mailbox("Archive")
            .unwrap()
            .iter()
            .any(|l| l.uid == 1),
        "pinned old UID survives a window shift"
    );

    // 3) A cadence-gated no-op poll (nothing changed): uid 1 still there.
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
        .await
        .unwrap();
    assert!(
        store
            .locations_in_mailbox("Archive")
            .unwrap()
            .iter()
            .any(|l| l.uid == 1),
        "pinned old UID survives an idle poll"
    );

    // 4) A flag-only change on the pinned message is picked up by the
    // periodic sweep. Advance the clock past the sweep interval so the
    // sweep runs (a flag-only change bumps no counter, exactly like any
    // other folder message), and the pinned UID's flags are refreshed
    // because the sweep's flag coverage includes live pinned UIDs.
    {
        let mut mb = mailbox.lock().unwrap();
        let folder = mb.folder("Archive", 50);
        let m = folder.messages.iter_mut().find(|m| m.uid == 1).unwrap();
        m.flags = vec!["\\Seen".into(), "\\Flagged".into()];
    }
    CLOCK.store(
        1_700_000_000 + crate::provider::imap::policy::FOLDER_SWEEP_INTERVAL_SECS + 1,
        Ordering::Relaxed,
    );
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
        .await
        .unwrap();
    let flags: Vec<String> = {
        let loc = store.locations_in_mailbox("Archive").unwrap();
        let row = loc.iter().find(|l| l.uid == 1).unwrap();
        serde_json::from_str(&row.flags_json).unwrap()
    };
    assert!(
        flags.iter().any(|f| f == "\\Flagged"),
        "a flag-only change on the pinned message is picked up: {flags:?}"
    );
    // Still listed after all those syncs.
    assert!(threads_in(&store, "Archive").contains(&acquired_thread));
    assert!(
        provider.fetch_thread(&acquired_thread).await.is_ok(),
        "the acquired thread is still fetchable, not NotFound"
    );
}

/// (required) An acquired pinned UID is removed when the server expunges
/// it (the pin never keeps a UID the server dropped), and its pin is
/// released.
#[tokio::test]
async fn a_pinned_uid_is_removed_when_the_server_expunges_it() {
    let mut mb = FakeMailbox::new(100);
    mb.add_to("Archive", 50, &["\\Seen"], &message("<a1@x>", "Old", ""));
    mb.add_to("Archive", 50, &["\\Seen"], &message("<a2@x>", "New", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_window(mailbox.clone(), 1);
    let db = store.database();

    let archive = entry_for(&provider, "Archive");
    let mut session = session_for(&mailbox).await;
    provider
        .acquire_uids(session.as_mut(), &archive, &[1], 50)
        .await
        .expect("acquire");
    let _ = session.logout().await;
    assert_eq!(store.pinned_uids("Archive", 50).unwrap(), vec![1]);

    // The server expunges uid 1.
    mailbox
        .lock()
        .unwrap()
        .folder("Archive", 50)
        .messages
        .retain(|m| m.uid != 1);
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
        .await
        .unwrap();

    assert!(
        !store
            .locations_in_mailbox("Archive")
            .unwrap()
            .iter()
            .any(|l| l.uid == 1),
        "an expunged pinned UID is deleted"
    );
    assert!(
        store.pinned_uids("Archive", 50).unwrap().is_empty(),
        "its pin is released"
    );
}

/// (required) Pins drop on a UIDVALIDITY reset (with the locations).
#[tokio::test]
async fn pins_drop_on_a_uidvalidity_reset() {
    let mut mb = FakeMailbox::new(100);
    mb.add_to("Archive", 50, &["\\Seen"], &message("<a1@x>", "Old", ""));
    mb.add_to("Archive", 50, &["\\Seen"], &message("<a2@x>", "New", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_window(mailbox.clone(), 1);
    let db = store.database();

    let archive = entry_for(&provider, "Archive");
    let mut session = session_for(&mailbox).await;
    provider
        .acquire_uids(session.as_mut(), &archive, &[1], 50)
        .await
        .expect("acquire");
    let _ = session.logout().await;
    assert_eq!(store.pinned_uids("Archive", 50).unwrap(), vec![1]);

    // The server rebuilds Archive with a NEW UIDVALIDITY.
    {
        let mut mb = mailbox.lock().unwrap();
        let folder = mb.folder("Archive", 50);
        folder.uidvalidity = 77;
        folder.uidnext = 1;
        folder.messages.clear();
        mb.add_to("Archive", 77, &["\\Seen"], &message("<b1@x>", "Fresh", ""));
    }
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
        .await
        .unwrap();

    assert!(
        store.pinned_uids("Archive", 50).unwrap().is_empty(),
        "the old incarnation's pins are dropped"
    );
    assert!(
        store.pinned_uids("Archive", 77).unwrap().is_empty(),
        "no pins carried into the new incarnation"
    );
}

/// (required) Acquiring an already-local UID creates no duplicate
/// message/location and pins it; acquiring twice is idempotent.
#[tokio::test]
async fn acquiring_an_already_local_uid_is_idempotent_and_pins_it() {
    // uid 1 is IN the window, so the normal sync already acquired it.
    let mut mb = FakeMailbox::new(100);
    mb.add_to("Archive", 50, &["\\Seen"], &message("<a1@x>", "Known", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_window(mailbox.clone(), 5);
    let db = store.database();

    // Normal sync ingests uid 1 (in window).
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
        .await
        .unwrap();
    assert_eq!(store.locations_in_mailbox("Archive").unwrap().len(), 1);
    let before_locations = store.locations_in_mailbox("Archive").unwrap().len();

    // Acquire the SAME uid 1 twice.
    let archive = entry_for(&provider, "Archive");
    for _ in 0..2 {
        let mut session = session_for(&mailbox).await;
        let outcome = provider
            .acquire_uids(session.as_mut(), &archive, &[1], 50)
            .await
            .expect("acquire");
        let _ = session.logout().await;
        assert_eq!(outcome.skipped_over_cap, 0);
    }
    // No duplicate location/message, and it is pinned exactly once.
    assert_eq!(
        store.locations_in_mailbox("Archive").unwrap().len(),
        before_locations,
        "no duplicate location from acquiring an already-local UID"
    );
    assert_eq!(
        store.pinned_uids("Archive", 50).unwrap(),
        vec![1],
        "the already-local UID is pinned exactly once"
    );
}

/// (required) A failure injected mid-acquire (after the body fetch,
/// before commit) leaves ZERO locations, threads or pins. The injection
/// is asserted to have fired (the server dropped the connection).
#[tokio::test]
async fn a_failure_mid_acquire_leaves_no_locations_threads_or_pins() {
    let mut mb = FakeMailbox::new(100);
    mb.add_to("Archive", 50, &["\\Seen"], &message("<a1@x>", "Old", ""));
    mb.add_to("Archive", 50, &["\\Seen"], &message("<a2@x>", "New", ""));
    // The server DROPS the connection after the FIRST whole-body fetch,
    // so acquiring two UIDs fails mid-way (after a fetch, before the
    // round commits).
    mb.folder("Archive", 50).fail_after_body_fetches = Some(1);
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_window(mailbox.clone(), 1);

    let archive = entry_for(&provider, "Archive");
    let mut session = session_for(&mailbox).await;
    let result = provider
        .acquire_uids(session.as_mut(), &archive, &[1, 2], 50)
        .await;
    let _ = session.logout().await;

    // The injection fired: the acquire returned an error (the dropped
    // connection), proving we reached the mid-fetch failure point.
    assert!(
        result.is_err(),
        "the injected mid-acquire failure must surface as an error"
    );
    // Nothing durable was written: no locations, no threads, no pins.
    assert!(
        store.locations_in_mailbox("Archive").unwrap().is_empty(),
        "a failed acquire writes no locations"
    );
    assert!(
        threads_in(&store, "Archive").is_empty(),
        "a failed acquire writes no threads"
    );
    assert!(
        store.pinned_uids("Archive", 50).unwrap().is_empty(),
        "a failed acquire writes no pins"
    );
}

/// (required) A UIDVALIDITY mismatch at acquire time is refused with the
/// typed error and ingests nothing.
#[tokio::test]
async fn acquire_refuses_a_uidvalidity_mismatch() {
    let mut mb = FakeMailbox::new(100);
    mb.add_to("Archive", 50, &["\\Seen"], &message("<a1@x>", "Old", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_window(mailbox.clone(), 1);

    let archive = entry_for(&provider, "Archive");
    let mut session = session_for(&mailbox).await;
    // We expect incarnation 999, but the server reports 50.
    let result = provider
        .acquire_uids(session.as_mut(), &archive, &[1], 999)
        .await;
    let _ = session.logout().await;
    match result {
        Err(AcquireError::UidValidityMismatch { expected, found, .. }) => {
            assert_eq!((expected, found), (999, 50));
        }
        other => panic!("expected a UIDVALIDITY mismatch, got {other:?}"),
    }
    assert!(
        store.locations_in_mailbox("Archive").unwrap().is_empty(),
        "a refused acquire ingests nothing"
    );
    assert!(store.pinned_uids("Archive", 50).unwrap().is_empty());
}

/// Review fix: a mailbox outside the sync set would never refresh or
/// release a pin, so acquiring into it is refused with no writes.
#[tokio::test]
async fn acquire_refuses_a_mailbox_outside_the_sync_set() {
    let mut mb = FakeMailbox::new(100);
    mb.add_to("Archive", 50, &["\\Seen"], &message("<a1@x>", "Old", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_window(mailbox.clone(), 1);

    let mut archive = entry_for(&provider, "Archive");
    archive.synced_now = false;
    let mut session = session_for(&mailbox).await;
    let result = provider
        .acquire_uids(session.as_mut(), &archive, &[1], 50)
        .await;
    let _ = session.logout().await;
    assert!(
        matches!(result, Err(AcquireError::MailboxNotSynced { .. })),
        "expected MailboxNotSynced, got {result:?}"
    );
    assert!(store.locations_in_mailbox("Archive").unwrap().is_empty());
    assert_eq!(store.search_pin_count().unwrap(), 0);
}

/// Review fix: a requested UID the server no longer has (expunged
/// between the caller's SEARCH and this FETCH) gets no location, so it
/// must get no pin either. The sibling UID that does exist is pinned.
#[tokio::test]
async fn acquire_pins_only_uids_that_ended_up_located() {
    let mut mb = FakeMailbox::new(100);
    mb.add_to("Archive", 50, &["\\Seen"], &message("<a1@x>", "Old", ""));
    mb.add_to("Archive", 50, &["\\Seen"], &message("<a2@x>", "New", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_window(mailbox.clone(), 1);

    let archive = entry_for(&provider, "Archive");
    let mut session = session_for(&mailbox).await;
    provider
        .acquire_uids(session.as_mut(), &archive, &[1, 99], 50)
        .await
        .expect("acquire");
    let _ = session.logout().await;
    let located: Vec<i64> = store
        .locations_in_mailbox("Archive")
        .unwrap()
        .iter()
        .map(|l| l.uid)
        .collect();
    assert_eq!(located, vec![1], "the vanished UID 99 has no location");
    assert_eq!(store.pinned_uids("Archive", 50).unwrap(), vec![1]);
    assert_eq!(store.search_pin_count().unwrap(), 1, "no orphan pin for 99");
}

/// Review fix: every path that removes a location removes its pin, so
/// a local move/delete (`delete_location`) or a dropped mailbox
/// (`drop_mailbox_locations`, also used on retirement-style resets)
/// cannot leave a pin counting against the cap.
#[tokio::test]
async fn removing_a_pinned_location_by_any_path_drops_its_pin() {
    let mut mb = FakeMailbox::new(100);
    for uid in 1..=3u32 {
        mb.add_to("Archive", 50, &["\\Seen"], &message(&format!("<a{uid}@x>"), "m", ""));
    }
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_window(mailbox.clone(), 1);
    let archive = entry_for(&provider, "Archive");

    let mut session = session_for(&mailbox).await;
    provider
        .acquire_uids(session.as_mut(), &archive, &[1, 2, 3], 50)
        .await
        .expect("acquire");
    let _ = session.logout().await;
    assert_eq!(store.pinned_uids("Archive", 50).unwrap(), vec![1, 2, 3]);

    // A local write path removes one location.
    store.delete_location("Archive", 50, 1).unwrap();
    assert_eq!(store.pinned_uids("Archive", 50).unwrap(), vec![2, 3]);

    // Dropping the whole mailbox's locations drops the rest.
    store.drop_mailbox_locations("Archive").unwrap();
    assert!(store.pinned_uids("Archive", 50).unwrap().is_empty());
    assert_eq!(store.search_pin_count().unwrap(), 0);
}

/// (required) Cap: below / exact / above, with over-cap UIDs reported
/// and existing pins untouched. Uses a tiny injected cap via seeding
/// the pin table directly, so no 2,000-message fixture is needed.
#[tokio::test]
async fn acquire_honours_the_pin_cap_and_reports_over_cap_uids() {
    // Seed the account near the cap by pre-pinning UIDs in a DIFFERENT
    // mailbox so the per-account count is close to MAX_SEARCH_PINS,
    // then acquire into Archive and watch the cap bite.
    let mut mb = FakeMailbox::new(100);
    for uid in 1..=5u32 {
        mb.add_to("Archive", 50, &["\\Seen"], &message(&format!("<a{uid}@x>"), "m", ""));
    }
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_window(mailbox.clone(), 1);

    // Pre-pin (MAX_SEARCH_PINS - 2) pins in a sentinel mailbox so only
    // room for 2 new pins remains.
    let base = crate::provider::imap::policy::MAX_SEARCH_PINS - 2;
    for uid in 0..base as u32 {
        store
            .upsert_pin("OtherFolder", 1, uid as i64 + 1, 1_700_000_000)
            .unwrap();
    }
    assert_eq!(store.search_pin_count().unwrap(), base);

    // Acquire 5 Archive UIDs; only room for 2 => 3 refused, and the
    // ACCEPTED ones are the NEWEST (uids 4 and 5).
    let archive = entry_for(&provider, "Archive");
    let mut session = session_for(&mailbox).await;
    let outcome = provider
        .acquire_uids(session.as_mut(), &archive, &[1, 2, 3, 4, 5], 50)
        .await
        .expect("acquire");
    let _ = session.logout().await;
    assert_eq!(outcome.skipped_over_cap, 3, "3 over-cap UIDs reported");
    // The accepted new pins are the newest two.
    assert_eq!(
        store.pinned_uids("Archive", 50).unwrap(),
        vec![4, 5],
        "newest-first acceptance under the cap"
    );
    // Existing pins in the sentinel mailbox are untouched.
    assert_eq!(store.search_pin_count().unwrap(), base + 2);
}

/// Exactly-at-cap acquires everything; below the cap acquires
/// everything; above refuses the overflow (the boundary cases).
#[tokio::test]
async fn acquire_cap_below_exact_and_above() {
    let mut mb = FakeMailbox::new(100);
    for uid in 1..=3u32 {
        mb.add_to("Archive", 50, &["\\Seen"], &message(&format!("<a{uid}@x>"), "m", ""));
    }
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store) = provider_window(mailbox.clone(), 1);
    let archive = entry_for(&provider, "Archive");

    // BELOW: room for many, acquire 2 => none refused.
    let mut session = session_for(&mailbox).await;
    let below = provider
        .acquire_uids(session.as_mut(), &archive, &[1, 2], 50)
        .await
        .expect("acquire");
    let _ = session.logout().await;
    assert_eq!(below.skipped_over_cap, 0);

    // Fill to EXACTLY one slot below the cap via sentinel pins, then
    // acquire 1 new => exact, none refused.
    let room_used = store.search_pin_count().unwrap();
    let fill = crate::provider::imap::policy::MAX_SEARCH_PINS - room_used - 1;
    for i in 0..fill as u32 {
        store.upsert_pin("Sentinel", 1, i as i64 + 1, 1_700_000_000).unwrap();
    }
    let mut session = session_for(&mailbox).await;
    let exact = provider
        .acquire_uids(session.as_mut(), &archive, &[3], 50)
        .await
        .expect("acquire");
    let _ = session.logout().await;
    assert_eq!(exact.skipped_over_cap, 0, "exactly at the cap accepts the last pin");
    assert_eq!(store.search_pin_count().unwrap(), crate::provider::imap::policy::MAX_SEARCH_PINS);

    // ABOVE: now full; a fresh new UID is refused.
    mailbox
        .lock()
        .unwrap()
        .add_to("Archive", 50, &["\\Seen"], &message("<a9@x>", "overflow", ""));
    let mut session = session_for(&mailbox).await;
    let above = provider
        .acquire_uids(session.as_mut(), &archive, &[4], 50)
        .await
        .expect("acquire");
    let _ = session.logout().await;
    assert_eq!(above.skipped_over_cap, 1, "over the cap refuses the overflow");
    assert!(
        !store.pinned_uids("Archive", 50).unwrap().contains(&4),
        "the over-cap UID is not pinned"
    );
}

/// (required) TTL: a pin just-before the TTL survives a sync; at/after
/// the TTL it is released and (if out of window) its location follows
/// normal window rules; re-acquiring refreshes `pinned_at`.
#[tokio::test]
async fn pin_ttl_releases_at_the_boundary_and_reacquire_refreshes_it() {
    use std::sync::atomic::{AtomicI64, Ordering};
    // A mutable clock so the test can advance `now` across the TTL.
    static CLOCK: AtomicI64 = AtomicI64::new(1_700_000_000);
    fn clock() -> i64 {
        CLOCK.load(Ordering::Relaxed)
    }

    let mut mb = FakeMailbox::new(100);
    mb.add_to("Archive", 50, &["\\Seen"], &message("<a1@x>", "Old", ""));
    mb.add_to("Archive", 50, &["\\Seen"], &message("<a2@x>", "New", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (mut provider, store) = provider_window(mailbox.clone(), 1);
    provider.now = clock;
    let db = store.database();

    CLOCK.store(1_700_000_000, Ordering::Relaxed);
    // Acquire the OLD uid 1 at t0.
    let archive = entry_for(&provider, "Archive");
    let mut session = session_for(&mailbox).await;
    provider
        .acquire_uids(session.as_mut(), &archive, &[1], 50)
        .await
        .expect("acquire");
    let _ = session.logout().await;
    assert_eq!(store.pinned_uids("Archive", 50).unwrap(), vec![1]);

    // JUST BEFORE the TTL: sync keeps the pin and the location.
    CLOCK.store(1_700_000_000 + crate::provider::imap::policy::SEARCH_PIN_TTL - 1, Ordering::Relaxed);
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
        .await
        .unwrap();
    assert_eq!(
        store.pinned_uids("Archive", 50).unwrap(),
        vec![1],
        "a pin one second short of the TTL survives"
    );
    assert!(store.locations_in_mailbox("Archive").unwrap().iter().any(|l| l.uid == 1));

    // AT the TTL: the pin is released and the (out-of-window) location
    // ages out under normal window rules.
    CLOCK.store(1_700_000_000 + crate::provider::imap::policy::SEARCH_PIN_TTL, Ordering::Relaxed);
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
        .await
        .unwrap();
    assert!(
        store.pinned_uids("Archive", 50).unwrap().is_empty(),
        "a pin at the TTL boundary is released"
    );
    assert!(
        !store.locations_in_mailbox("Archive").unwrap().iter().any(|l| l.uid == 1),
        "after release the out-of-window location ages out"
    );

    // RE-ACQUIRE refreshes pinned_at: acquire again at a later time, and
    // a sync just-before the NEW TTL keeps it.
    let t2 = 1_700_000_000 + crate::provider::imap::policy::SEARCH_PIN_TTL + 100;
    CLOCK.store(t2, Ordering::Relaxed);
    let archive = entry_for(&provider, "Archive");
    let mut session = session_for(&mailbox).await;
    provider
        .acquire_uids(session.as_mut(), &archive, &[1], 50)
        .await
        .expect("re-acquire");
    let _ = session.logout().await;
    assert_eq!(store.pinned_uids("Archive", 50).unwrap(), vec![1], "re-acquired");
    // Just before the refreshed TTL: still pinned.
    CLOCK.store(t2 + crate::provider::imap::policy::SEARCH_PIN_TTL - 1, Ordering::Relaxed);
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
        .await
        .unwrap();
    assert_eq!(
        store.pinned_uids("Archive", 50).unwrap(),
        vec![1],
        "re-acquire refreshed pinned_at, so it survives relative to the new time"
    );
}

// ---- live Dovecot integration (opt-in; does not run without the
// container). Marked exactly like the other live tests in this module:
// gated on THREESTRANDS_IMAP_IT=1 + DOVECOT_TEST_FP, returns early
// otherwise. It acquires an out-of-window UID and runs two syncs,
// asserting the acquired hit survives. NOTE (reported): this did NOT
// run here — Docker is unavailable in this environment, so it was
// written and gated but not executed.
mod live {
    use super::*;
    use crate::provider::imap::connection::{
        ConnectionConfig, ImapConnectionManager, TlsMode,
    };
    use crate::provider::imap::tls::parse_sha256_fingerprint;

    const HOST: &str = "127.0.0.1";
    const PORT: u16 = 11143;
    const USER: &str = "test@threestrands.test";
    const PASSWORD: &str = "testpassword";

    fn gated() -> Option<ConnectionConfig> {
        if std::env::var("THREESTRANDS_IMAP_IT").ok().as_deref() != Some("1") {
            eprintln!("skipping: THREESTRANDS_IMAP_IT != 1");
            return None;
        }
        let Ok(fp_str) = std::env::var("DOVECOT_TEST_FP") else {
            eprintln!("skipping: DOVECOT_TEST_FP not set (Dovecot container absent)");
            return None;
        };
        let fp = parse_sha256_fingerprint(&fp_str)
            .expect("DOVECOT_TEST_FP must be a 64-hex-digit SHA-256 fingerprint");
        Some(ConnectionConfig {
            host: HOST.into(),
            port: PORT,
            tls_mode: TlsMode::StartTls,
            pinned_fingerprint: Some(fp),
        })
    }

    fn live_provider(config: ConnectionConfig) -> (ImapProvider, ImapStateStore) {
        let database = Arc::new(Database::open_memory());
        database
            .adopt_mail_account(USER, crate::models::MailProviderKind::Imap)
            .unwrap();
        let store = ImapStateStore::new(database.clone(), USER);
        let cache = BodyCache::new(store.clone());
        let provider = ImapProvider::new(ImapProviderConfig {
            account_id: USER.into(),
            username: USER.into(),
            password: PASSWORD.into(),
            smtp_password: None,
            manager: ImapConnectionManager::new(config),
            store: store.clone(),
            cache,
            label_model: LabelModel::ImapLabelFolders,
            settings: super::super::test_settings(),
        });
        (provider, store)
    }

    /// Acquire an out-of-window UID against the live Dovecot harness and
    /// run two syncs; the pinned hit must survive both. Gated — returns
    /// immediately without the container.
    #[tokio::test]
    async fn live_acquire_out_of_window_uid_survives_two_syncs() {
        let Some(config) = gated() else { return };
        let (mut provider, store) = live_provider(config);
        // Window of ONE in every mailbox, so the oldest UID of any
        // mailbox with two or more messages is genuinely out of window
        // and would be evicted by the next sweep without its pin.
        provider.limits.mailbox_window = 1;
        provider.folder_window_override = Some(1);
        let db = store.database();

        // Baseline so the catalog + INBOX are known.
        let _ = provider.baseline_cursor().await.expect("baseline");

        // INBOX: the harness seeds several messages there and it is an
        // EVICTING mailbox, so with a window of one its oldest UID is
        // out of window. (Archive holds a single seeded message.)
        let plan = provider.build_sync_plan().expect("plan");
        let target = plan
            .entries
            .iter()
            .find(|e| e.is_inbox)
            .cloned()
            .expect("INBOX is always planned");

        // EXAMINE to learn the UIDVALIDITY and an existing UID.
        let mut session = provider.open_session().await.expect("session");
        let status = session.examine(&target.mailbox).await.expect("examine");
        let uidvalidity = status.uid_validity.unwrap_or(0) as i64;
        let all = session.uid_search("ALL").await.expect("search");
        assert!(
            all.len() >= 2,
            "{} needs two or more seeded messages for an out-of-window UID",
            target.mailbox
        );
        let uid = *all.iter().min().unwrap();
        let outcome = provider
            .acquire_uids(session.as_mut(), &target, &[uid], uidvalidity)
            .await
            .expect("acquire");
        let _ = session.logout().await;
        // Drop the session NOW: it owns one of the account's two
        // connection leases, and holding it makes the sync below wait
        // forever for a slot.
        drop(session);
        assert_eq!(outcome.skipped_over_cap, 0);
        assert!(store.is_pinned(&target.mailbox, uidvalidity, uid as i64).unwrap());

        // Two syncs: the pinned out-of-window hit must survive both,
        // as a LOCATION (not just a pin row).
        for _ in 0..2 {
            crate::sync::sync_with(db.as_ref(), USER, &provider)
                .await
                .unwrap();
            assert!(
                store
                    .locations_in_mailbox(&target.mailbox)
                    .unwrap()
                    .iter()
                    .any(|l| l.uid == uid as i64),
                "the acquired out-of-window location survives sync"
            );
            assert!(
                store.is_pinned(&target.mailbox, uidvalidity, uid as i64).unwrap(),
                "the acquired pin survives sync"
            );
        }
    }

    /// Search a subject word seeded ONLY into Archive (an old,
    /// out-of-window message) with the windows shrunk to one, assert
    /// the old hit's thread comes back from `MailSync::search`, then
    /// drop the provider's session use and run `sync::sync_with` TWICE
    /// and assert the acquired location survives (the pin keeps it).
    /// Gated — returns immediately without the container. (This worker's
    /// sandbox has NO docker, so this did NOT run here; it is written to
    /// compile and skip cleanly, and the reviewer runs it against the
    /// container.)
    #[tokio::test]
    async fn live_search_returns_an_old_archive_hit_that_survives_two_syncs() {
        let Some(config) = gated() else { return };
        let (mut provider, store) = live_provider(config);
        // Window of one everywhere so the Archive hit (if Archive has
        // more than one message) is out of window; even with a single
        // Archive message the pin is what the sync must not evict.
        provider.limits.mailbox_window = 1;
        provider.folder_window_override = Some(1);
        // Keyword labels so `label:` would render a KEYWORD; not used
        // here but keeps the context consistent with production.
        provider.settings.label_storage =
            crate::provider::imap::LabelStorage::Keywords;
        let db = store.database();

        // Baseline so the catalog + plan know Archive.
        let _ = provider.baseline_cursor().await.expect("baseline");

        // Search for the seeded Archive subject word. in:archive scopes
        // the server search to Archive; "archived" matches the seed's
        // "An archived message" subject (docker/dovecot-test/seed.sh).
        let page = provider
            .search("in:archive subject:archived", None)
            .await
            .expect("search");
        assert!(
            !page.thread_ids.is_empty(),
            "the old Archive hit comes back from server search"
        );
        let hit = page.thread_ids[0].clone();
        assert!(page.next.is_none(), "one bounded page, no server paging token");

        // The hit is pinned + located in Archive.
        assert!(
            !store.locations_in_mailbox("Archive").unwrap().is_empty(),
            "the Archive hit is acquired as a location"
        );

        // `search` already logged out + dropped its session, so the two
        // syncs below cannot deadlock on the account's two-lease cap.
        for _ in 0..2 {
            crate::sync::sync_with(db.as_ref(), USER, &provider)
                .await
                .unwrap();
        }
        assert!(
            provider.fetch_thread(&hit).await.is_ok(),
            "the searched Archive thread survives two syncs (pin kept it)"
        );
    }
}
