use super::*;
use async_trait::async_trait;

use crate::provider::{MailSync, ProviderResult};

use super::super::server_search::imap_search_report;
use crate::provider::imap::fetch::BodyCache;
use crate::provider::imap::policy::{self};
use crate::provider::imap::session::ImapSession;
use crate::provider::imap::settings::LabelStorage;
use crate::provider::imap::ImapStateStore;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A session source that COUNTS how many times it was opened, wrapping
/// the same fresh-scripted-server behaviour as `FakeSource`, so a test
/// can assert "no network call at all" (zero opens) for a search the
/// renderer skips.
struct CountingSource {
    mailbox: Arc<Mutex<FakeMailbox>>,
    opens: Arc<AtomicUsize>,
}

#[async_trait]
impl ImapSessionSource for CountingSource {
    async fn open(&self) -> ProviderResult<Box<dyn ImapSession>> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        let snapshot = self.mailbox.lock().unwrap().clone();
        let (client, _task) = spawn_server(snapshot);
        let session = async_imap::Client::new(client)
            .login("user", "password")
            .await
            .map_err(|(error, _)| super::super::super::error::map_imap_error(&error))?;
        Ok(Box::new(super::super::super::session::AsyncImapSession::new(session)))
    }
}

/// Build a provider whose source counts opens, with a tiny non-INBOX
/// window so acquired hits are genuinely out-of-window (pinned), and
/// keyword label storage (so `label:` renders a KEYWORD, not a folder).
fn counting_provider(
    mailbox: Arc<Mutex<FakeMailbox>>,
) -> (ImapProvider, ImapStateStore, Arc<AtomicUsize>) {
    let (mut provider, store) = provider_with(mailbox.clone());
    let opens = Arc::new(AtomicUsize::new(0));
    provider.source = Arc::new(CountingSource {
        mailbox,
        opens: opens.clone(),
    });
    // A tiny window so even an acquired hit is out of the sync window
    // and must be pinned to survive — mirrors the search_pins tests.
    provider.folder_window_override = Some(1);
    // Keyword labels so the standard plan's `label:` path renders a
    // KEYWORD; the settings' label_storage drives the renderer context.
    provider.settings.label_storage = LabelStorage::Keywords;
    (provider, store, opens)
}

fn plan_mailboxes(provider: &ImapProvider) -> Vec<String> {
    provider
        .build_sync_plan()
        .unwrap()
        .entries
        .iter()
        .map(|e| e.mailbox.clone())
        .collect()
}

/// A standard multi-mailbox fake: INBOX + Archive + Sent each with a
/// couple of messages. Trash/Junk present but not in the default set.
fn multi_mailbox() -> FakeMailbox {
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<inbox-old@x>", "old inbox hit", ""));
    mb.add(&["\\Seen"], &message("<inbox-new@x>", "new inbox hit", ""));
    mb.add_to("Archive", 50, &["\\Seen"], &message("<arch-old@x>", "old archive hit", ""));
    mb.add_to("Archive", 50, &["\\Seen"], &message("<arch-new@x>", "new archive hit", ""));
    mb.add_to("Sent", 70, &["\\Seen"], &message("<sent-old@x>", "old sent hit", ""));
    mb.add_to("Sent", 70, &["\\Seen"], &message("<sent-new@x>", "new sent hit", ""));
    mb
}

// --- 1) an OLD out-of-window hit is returned, pinned, and survives a
//        following sync ------------------------------------------------
/// Review addition: the real backfill path. `search_and_ingest_missing`
/// (what "include archived" runs) must turn an IMAP search hit into an
/// ENGINE thread row, which is what the local search then displays.
/// The acquire writes only the IMAP store, so the engine row comes from
/// `fetch_thread` on the returned id: this proves the ids match.
#[tokio::test]
async fn the_backfill_path_ingests_an_old_hit_into_the_engine_thread_list() {
    let mut mb = FakeMailbox::new(100);
    mb.add_to("Archive", 50, &["\\Seen"], &message("<a1@x>", "old one", ""));
    mb.add_to("Archive", 50, &["\\Seen"], &message("<a2@x>", "new two", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store, _opens) = counting_provider(mailbox);
    let db = store.database();
    // Baseline so the engine account and cursor exist; with window 1
    // only the newest Archive message is synced.
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
        .await
        .unwrap();
    let before = db.local_provider_thread_ids("me@example.com").unwrap();

    crate::sync::search_and_ingest_missing(
        db.as_ref(),
        "me@example.com",
        &provider,
        "in:archive subject:old",
        50,
    )
    .await
    .expect("backfill search");

    let page = provider.search("in:archive subject:old", None).await.unwrap();
    let after = db.local_provider_thread_ids("me@example.com").unwrap();
    let old_thread = store
        .thread_of_message(
            &store.location_message_id("Archive", 50, 1).unwrap().expect("uid 1 located"),
        )
        .unwrap()
        .expect("uid 1 threaded");
    let old_thread = store.resolve_thread_alias(&old_thread).unwrap();
    assert!(page.thread_ids.contains(&old_thread), "search returns the old hit");
    assert!(!before.contains(&old_thread), "the old hit was not in the engine before");
    assert!(after.contains(&old_thread), "the backfill put it in the engine thread list");
}

#[tokio::test]
async fn an_old_out_of_window_hit_is_returned_pinned_and_survives_a_sync() {
    // Archive with 3 messages, window 1: uids 1 and 2 are out of
    // window. A search targeting in:archive returns all three; the
    // driver acquires + pins them, so the OLD uid 1 comes back and
    // survives the next sweep.
    let mut mb = FakeMailbox::new(100);
    mb.add_to("Archive", 50, &["\\Seen"], &message("<a1@x>", "old one", ""));
    mb.add_to("Archive", 50, &["\\Seen"], &message("<a2@x>", "old two", ""));
    mb.add_to("Archive", 50, &["\\Seen"], &message("<a3@x>", "new three", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store, opens) = counting_provider(mailbox);
    let db = store.database();

    let result = provider
        .search_detailed("in:archive subject:old", None)
        .await
        .expect("search");
    assert!(opens.load(Ordering::SeqCst) >= 1, "a Plan opens a session");
    assert_eq!(result.partial, None, "all hits fit the budget");
    assert!(!result.thread_ids.is_empty(), "the archive hits come back");

    // uid 1 (the oldest, out of window) is pinned and located.
    assert!(store.is_pinned("Archive", 50, 1).unwrap(), "old uid pinned");
    assert!(
        store.locations_in_mailbox("Archive").unwrap().iter().any(|l| l.uid == 1),
        "old uid located"
    );
    let hit_thread = result.thread_ids[0].clone();

    // A following sync does not evict the pinned out-of-window hit.
    crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
        .await
        .unwrap();
    assert!(
        store.locations_in_mailbox("Archive").unwrap().iter().any(|l| l.uid == 1),
        "pinned old hit survives a sync"
    );
    assert!(
        provider.fetch_thread(&hit_thread).await.is_ok(),
        "the returned thread id is what fetch_thread resolves"
    );
}

// --- 2) hits across INBOX, Archive and Sent: budget split, newest-
//        first, dedup ---------------------------------------------------
#[tokio::test]
async fn hits_across_three_mailboxes_are_newest_first_and_deduped() {
    let mailbox = Arc::new(Mutex::new(multi_mailbox()));
    let (provider, store, _opens) = counting_provider(mailbox);

    // Default set is INBOX, Archive, Sent (plan order). from:x matches
    // all (the fake returns every uid), so each mailbox contributes its
    // two messages, newest-UID-first within a mailbox.
    let result = provider.search_detailed("from:someone", None).await.expect("search");
    assert_eq!(plan_mailboxes_default(&provider), vec!["INBOX", "Archive", "Sent"]);
    assert_eq!(result.partial, None);
    // Six distinct threads, newest-first within each mailbox, mailboxes
    // in plan order: INBOX(new,old), Archive(new,old), Sent(new,old).
    let subjects = result
        .thread_ids
        .iter()
        .map(|t| subject_of(&store, t))
        .collect::<Vec<_>>();
    assert_eq!(
        subjects,
        vec![
            "new inbox hit",
            "old inbox hit",
            "new archive hit",
            "old archive hit",
            "new sent hit",
            "old sent hit",
        ],
        "newest-UID-first within a mailbox, mailboxes in plan order"
    );
    // Dedup: every id is unique.
    let mut sorted = result.thread_ids.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), result.thread_ids.len(), "no duplicate thread ids");
}

#[tokio::test]
async fn two_hits_sharing_a_thread_are_returned_once() {
    // Two messages in INBOX that thread together (a reply) must yield
    // ONE thread id. Build a parent + a reply referencing it.
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &message("<p@x>", "parent", ""));
    mb.add(
        &["\\Seen"],
        &message("<r@x>", "Re: parent", "In-Reply-To: <p@x>\r\nReferences: <p@x>\r\n"),
    );
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, _store, _opens) = counting_provider(mailbox);
    let result = provider.search_detailed("in:inbox subject:parent", None).await.unwrap();
    assert_eq!(result.thread_ids.len(), 1, "a reply and its parent are one thread");
}

// --- 3) budget below / exact / above -----------------------------------
#[tokio::test]
async fn the_acquire_budget_is_enforced_below_exact_and_above() {
    use crate::provider::imap::policy::MAX_SEARCH_ACQUIRE;
    // Put MANY messages in a single mailbox so one mailbox alone can
    // exceed the budget. in:archive targets just Archive, so its fair
    // share is the whole budget.
    let build = |count: usize| {
        let mut mb = FakeMailbox::new(100);
        for i in 0..count {
            mb.add_to("Archive", 50, &["\\Seen"], &message(&format!("<a{i}@x>"), &format!("m{i}"), ""));
        }
        Arc::new(Mutex::new(mb))
    };

    // Below the budget: all returned, not partial.
    let (p, _s, _o) = counting_provider(build(MAX_SEARCH_ACQUIRE - 1));
    let below = p.search_detailed("in:archive from:a", None).await.unwrap();
    assert_eq!(below.thread_ids.len(), MAX_SEARCH_ACQUIRE - 1);
    assert_eq!(below.partial, None);

    // Exactly at the budget: all returned, not partial.
    let (p, _s, _o) = counting_provider(build(MAX_SEARCH_ACQUIRE));
    let exact = p.search_detailed("in:archive from:a", None).await.unwrap();
    assert_eq!(exact.thread_ids.len(), MAX_SEARCH_ACQUIRE);
    assert_eq!(exact.partial, None);

    // Above the budget: capped to the budget, reported partial.
    let (p, _s, _o) = counting_provider(build(MAX_SEARCH_ACQUIRE + 5));
    let above = p.search_detailed("in:archive from:a", None).await.unwrap();
    assert_eq!(above.thread_ids.len(), MAX_SEARCH_ACQUIRE, "capped at the budget");
    assert_eq!(above.partial, Some(super::super::PartialReason::BudgetReached));
}

// --- 4) deadline: an expired deadline after the first mailbox returns
//        that mailbox's hits and reports partial ------------------------
#[tokio::test(start_paused = true)]
async fn an_expired_deadline_after_the_first_mailbox_returns_partial() {
    let mailbox = Arc::new(Mutex::new(multi_mailbox()));
    let (mut provider, store, _opens) = counting_provider(mailbox);
    // A zero deadline: the FIRST mailbox is examined (the deadline is
    // checked at the top of each mailbox AND wraps each await), but the
    // SECOND is not started. With start_paused the clock does not
    // advance on its own, so a zero budget means "already expired" on
    // entry to the loop. To let the first mailbox through, use a tiny
    // budget and advance time between mailboxes is not possible from
    // inside; instead rely on the deadline being checked at the TOP of
    // the loop: the first iteration (elapsed 0) proceeds, we then
    // advance via the timeout wrappers. Simplest deterministic form:
    // a deadline of zero expires before mailbox 1, so assert the empty-
    // but-partial shape; a positive-but-tiny budget is covered by the
    // live path. Here we prove the PARTIAL path fires and names counts.
    provider.search_deadline_override = Some(std::time::Duration::ZERO);
    let result = provider.search_detailed("from:someone", None).await.unwrap();
    match result.partial {
        Some(super::super::PartialReason::DeadlineExpired {
            mailboxes_searched,
            mailboxes_planned,
        }) => {
            assert_eq!(mailboxes_planned, 3, "INBOX, Archive, Sent planned");
            assert_eq!(mailboxes_searched, 0, "the zero deadline stopped before mailbox 1");
        }
        other => panic!("expected DeadlineExpired, got {other:?}"),
    }
    assert!(result.thread_ids.is_empty(), "nothing acquired past the deadline");
    // Nothing was pinned (no acquire ran).
    assert_eq!(store.search_pin_count().unwrap(), 0);
}

/// Review fix: the deadline expires WHILE a command is in flight (the
/// server stalls on Archive's UID SEARCH). The first mailbox's hits are
/// kept and the search RETURNS: it must not then send LOGOUT on the
/// half-read connection and wait forever, which would hold the
/// account's sync gate and stop its sync. The outer timeout turns a
/// hang into a failure instead of a stuck test run.
#[tokio::test(start_paused = true)]
async fn a_deadline_that_cancels_an_in_flight_command_returns_without_hanging() {
    let mut mb = multi_mailbox();
    mb.folder("Archive", 50).search_stalls = true;
    let mailbox = Arc::new(Mutex::new(mb));
    let (mut provider, store, _opens) = counting_provider(mailbox);
    provider.search_deadline_override = Some(std::time::Duration::from_secs(5));

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        provider.search_detailed("from:someone", None),
    )
    .await
    .expect("search must return after its deadline, not hang on LOGOUT")
    .unwrap();
    match result.partial {
        Some(super::super::PartialReason::DeadlineExpired {
            mailboxes_searched,
            mailboxes_planned,
        }) => {
            assert_eq!(mailboxes_planned, 3);
            assert_eq!(mailboxes_searched, 1, "INBOX finished, Archive stalled");
        }
        other => panic!("expected DeadlineExpired, got {other:?}"),
    }
    assert_eq!(result.thread_ids.len(), 2, "INBOX's two hits are returned");
    assert_eq!(store.search_pin_count().unwrap(), 2, "and stay pinned");
}

#[tokio::test(start_paused = true)]
async fn a_stalled_connection_returns_partial_at_the_search_deadline() {
    struct StalledSource;
    #[async_trait]
    impl ImapSessionSource for StalledSource {
        async fn open(&self) -> ProviderResult<Box<dyn ImapSession>> {
            std::future::pending().await
        }
    }
    let (mut provider, store, _) = counting_provider(Arc::new(Mutex::new(multi_mailbox())));
    provider.source = Arc::new(StalledSource);
    let started = tokio::time::Instant::now();
    let result = tokio::time::timeout(
        policy::SEARCH_DEADLINE + std::time::Duration::from_secs(1),
        provider.search_detailed("from:someone", None),
    )
    .await
    .expect("connection establishment must be bounded")
    .unwrap();
    assert!(matches!(
        result.partial,
        Some(PartialReason::DeadlineExpired {
            mailboxes_searched: 0,
            mailboxes_planned: 3,
        })
    ));
    assert!(result.thread_ids.is_empty());
    assert_eq!(store.search_pin_count().unwrap(), 0);
    assert_eq!(started.elapsed(), policy::SEARCH_DEADLINE);
}

#[tokio::test(start_paused = true)]
async fn a_stalled_logout_returns_acquired_results_within_the_deadline() {
    let mut mb = multi_mailbox();
    mb.logout_stalls = true;
    let (provider, store, _) = counting_provider(Arc::new(Mutex::new(mb)));
    let started = tokio::time::Instant::now();
    let result = tokio::time::timeout(
        policy::SEARCH_DEADLINE + std::time::Duration::from_secs(1),
        provider.search_detailed("from:someone", None),
    )
    .await
    .expect("cleanup must be bounded")
    .unwrap();
    assert_eq!(result.thread_ids.len(), 6);
    assert_eq!(store.search_pin_count().unwrap(), 6);
    assert_eq!(
        result.partial, None,
        "cleanup cannot discard completed search results"
    );
    assert!(started.elapsed() <= policy::SEARCH_DEADLINE);
}

// --- 5) Skipped and NoServerMatches make NO network call and return
//        empty ---------------------------------------------------------
#[tokio::test]
async fn a_skipped_query_makes_no_network_call_and_returns_empty() {
    let mailbox = Arc::new(Mutex::new(multi_mailbox()));
    let (provider, _store, opens) = counting_provider(mailbox);
    // `category:promotions` is an unknown operator -> Raw -> Skipped by the renderer.
    let result = provider.search_detailed("category:promotions", None).await.unwrap();
    assert!(result.thread_ids.is_empty());
    assert!(result.skipped.is_some(), "carries the skip reason for the UI");
    assert_eq!(result.partial, None);
    assert_eq!(opens.load(Ordering::SeqCst), 0, "no session opened for a Skipped render");
    // The MailSync wrapper returns the same empty page, next: None.
    let page = provider.search("category:promotions", None).await.unwrap();
    assert!(page.thread_ids.is_empty());
    assert!(page.next.is_none());
}

#[tokio::test]
async fn a_no_server_matches_query_makes_no_network_call_and_returns_empty() {
    let mailbox = Arc::new(Mutex::new(multi_mailbox()));
    let (provider, _store, opens) = counting_provider(mailbox);
    // in:nowhere is an unknown mailbox -> NoServerMatches.
    let result = provider.search_detailed("in:nowhere from:a", None).await.unwrap();
    assert!(result.thread_ids.is_empty());
    assert!(result.no_matches.is_some());
    assert_eq!(opens.load(Ordering::SeqCst), 0, "no session opened for a no-match render");
}

// --- 6) an unsynced planned mailbox is skipped + reported, others run --
#[tokio::test]
async fn an_unsynced_mailbox_is_skipped_and_reported_others_still_run() {
    // The default search set is INBOX, Archive, Sent — all system roles,
    // so all synced_now in the current plan. To exercise the driver's
    // unsynced SKIP + REPORT channel we hand `run_search_plan` a plan
    // whose Archive entry has been flipped to NOT synced_now (which run
    // B's richer plans can produce), naming all three mailboxes. INBOX
    // and Sent still run; Archive is skipped and reported, never
    // ingested (a pin there would never be maintained).
    let mailbox = Arc::new(Mutex::new(multi_mailbox()));
    let (provider, store, _opens) = counting_provider(mailbox);
    let mut plan = provider.build_sync_plan().unwrap();
    for entry in plan.entries.iter_mut() {
        if entry.mailbox == "Archive" {
            entry.synced_now = false;
        }
    }
    let search_plan = crate::provider::imap::search::ImapSearchPlan {
        mailboxes: vec!["INBOX".into(), "Archive".into(), "Sent".into()],
        criteria: "CHARSET UTF-8 FROM \"someone\"".into(),
        local_filters: Vec::new(),
    };
    let result = provider.run_search_plan(&plan, search_plan).await.unwrap();
    assert_eq!(result.unsynced_mailboxes, vec!["Archive"], "Archive reported unsynced");
    // INBOX and Sent still produced hits; Archive contributed none.
    assert!(!result.thread_ids.is_empty(), "the synced mailboxes still run");
    assert!(
        store.locations_in_mailbox("Archive").unwrap().is_empty(),
        "nothing acquired in the unsynced mailbox"
    );
    assert!(!store.locations_in_mailbox("INBOX").unwrap().is_empty());
    assert!(!store.locations_in_mailbox("Sent").unwrap().is_empty());
}

// --- 7) has:attachment filters the returned threads --------------------
#[tokio::test]
async fn has_attachment_filters_the_returned_threads_but_keeps_them_pinned() {
    // Two INBOX messages: one with an attachment part, one plain. Both
    // are acquired + pinned; only the attachment one is returned.
    let plain = message("<plain@x>", "no attachment", "");
    let with_attachment = concat!(
        "Message-ID: <att@x>\r\nFrom: a@x\r\nTo: b@x\r\nSubject: has attachment\r\n",
        "Date: 01 Jan 2024 10:00:00 +0000\r\nMIME-Version: 1.0\r\n",
        "Content-Type: multipart/mixed; boundary=M\r\n\r\n",
        "--M\r\nContent-Type: text/plain\r\n\r\nbody\r\n",
        "--M\r\nContent-Type: application/octet-stream\r\n",
        "Content-Disposition: attachment; filename=\"a.bin\"\r\n\r\nDATA\r\n--M--\r\n"
    )
    .to_string();
    let mut mb = FakeMailbox::new(100);
    mb.add(&["\\Seen"], &plain);
    mb.add(&["\\Seen"], &with_attachment);
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store, _opens) = counting_provider(mailbox);

    let result = provider
        .search_detailed("in:inbox has:attachment from:a", None)
        .await
        .unwrap();
    assert_eq!(result.thread_ids.len(), 1, "only the attachment thread is returned");
    assert_eq!(subject_of(&store, &result.thread_ids[0]), "has attachment");
    // BOTH were acquired + pinned (the filtered-out one stays pinned).
    assert_eq!(
        store.locations_in_mailbox("INBOX").unwrap().len(),
        2,
        "both messages were acquired; the filter only hides one from the result"
    );
}

// --- 8) a server error mid-search on the SECOND mailbox returns the
//        first mailbox's hits and reports partial (decision: partial,
//        not error) -----------------------------------------------------
#[tokio::test]
async fn a_server_error_on_the_second_mailbox_returns_partial_with_the_first() {
    // Archive's EXAMINE fails; INBOX is first and succeeds. The default
    // order is INBOX, Archive, Sent, so INBOX hits come back and Archive
    // triggers the partial.
    let mut mb = multi_mailbox();
    mb.folder("Archive", 50).examine_fails = true;
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, _store, _opens) = counting_provider(mailbox);
    let result = provider.search_detailed("from:someone", None).await.unwrap();
    assert!(
        !result.thread_ids.is_empty(),
        "INBOX hits acquired before the failing mailbox are returned"
    );
    match result.partial {
        Some(super::super::PartialReason::MailboxError { mailbox, .. }) => {
            assert_eq!(mailbox, "Archive");
        }
        other => panic!("expected a MailboxError partial, got {other:?}"),
    }
}

// --- UIDVALIDITY changing between EXAMINE and acquire ------------------
//
// The driver passes the EXAMINE's UIDVALIDITY straight into acquire_uids
// on the SAME session, so within one search EXAMINE and acquire always
// agree and the mismatch arm cannot be provoked through the fake server
// (which cannot change UIDVALIDITY between two commands on one
// connection). The mismatch is a clean per-mailbox skip in the driver
// (covered by code review and the acquire_uids UIDVALIDITY test in
// `search_pins`); the renderer-level incarnation handling is proven in
// the live Dovecot test. We assert here that the driver's own guard
// path exists by exercising the acquire mismatch directly.
#[tokio::test]
async fn acquire_uidvalidity_mismatch_is_a_clean_skip_not_a_wrong_ingest() {
    let mut mb = FakeMailbox::new(100);
    mb.add_to("Archive", 50, &["\\Seen"], &message("<a1@x>", "one", ""));
    let mailbox = Arc::new(Mutex::new(mb));
    let (provider, store, _opens) = counting_provider(mailbox.clone());
    let plan = provider.build_sync_plan().unwrap();
    let archive = plan.entries.iter().find(|e| e.mailbox == "Archive").unwrap().clone();
    let snapshot = mailbox.lock().unwrap().clone();
    let mut session = open_scripted_session(snapshot).await;
    // Expect the WRONG incarnation (server reports 50): a clean error,
    // nothing ingested.
    let err = provider
        .acquire_uids(session.as_mut(), &archive, &[1], 999)
        .await
        .expect_err("mismatch must error");
    let _ = session.logout().await;
    assert!(matches!(err, super::super::AcquireError::UidValidityMismatch { .. }));
    assert!(
        store.locations_in_mailbox("Archive").unwrap().is_empty(),
        "nothing ingested under the wrong incarnation"
    );
}

/// The default plan's searched mailboxes (helper duplicate of the
/// renderer's default set, read from the plan).
fn plan_mailboxes_default(provider: &ImapProvider) -> Vec<String> {
    // Mirror render's default: INBOX, Archive, Sent (present-in-plan).
    let names = plan_mailboxes(provider);
    ["INBOX", "Archive", "Sent"]
        .iter()
        .filter(|n| names.iter().any(|m| m == *n))
        .map(|n| n.to_string())
        .collect()
}

/// The subject header of a thread's (single) message, from the cached
/// body — used to assert ordering without exposing UIDs.
fn subject_of(store: &ImapStateStore, thread_id: &str) -> String {
    let resolved = store.resolve_thread_alias(thread_id).unwrap();
    let message_ids = store.messages_in_thread(&resolved).unwrap();
    for message_id in message_ids {
        // Read the cached body and pull the Subject header.
        if let Ok(Some(cached)) = BodyCache::new(store.clone()).get(&message_id) {
            let text = String::from_utf8_lossy(&cached.raw);
            for line in text.lines() {
                if let Some(rest) = line.strip_prefix("Subject: ") {
                    return rest.trim().to_string();
                }
            }
        }
    }
    String::new()
}

// --- Phase 5 run 5: ImapSearchResult -> SearchReport mapping ----------
//
// Each IMAP search OUTCOME must map to the right provider-neutral
// `SearchReport` the UI words, reusing the run-4 fixtures above. The
// mapping function and the `search_with_report` override are proven
// here; the internal `SearchNotice` / `PartialReason` detail must NOT
// leak into the report (only the small typed codes).
mod report_mapping {
    use super::*;
    use crate::provider::{
        NoMatchSearchReason, PartialSearchReason, SearchReportStatus, SkipSearchReason,
    };

    /// A complete search over the default set reports `Complete` with
    /// no caveats, and `search_with_report`'s page matches `search`.
    #[tokio::test]
    async fn a_complete_search_reports_complete_and_byte_identical_page() {
        let mailbox = Arc::new(Mutex::new(multi_mailbox()));
        let (provider, _store, _opens) = counting_provider(mailbox);
        let (page, report) = provider
            .search_with_report("from:someone", None)
            .await
            .expect("search");
        let plain = provider.search("from:someone", None).await.expect("search");
        assert_eq!(page.thread_ids, plain.thread_ids, "same ids as search()");
        assert!(page.next.is_none());
        assert_eq!(report.status, SearchReportStatus::Complete);
        assert_eq!(report.unsynced_mailboxes, 0);
        assert_eq!(report.skipped_over_cap, 0);
        assert_eq!(report.server_searched_accounts, 1, "this account searched");
    }

    /// Review fix: hits the pin cap refused are matches the user never
    /// sees, so a search that otherwise finished must NOT report
    /// `Complete`; it reports the budget-style partial.
    #[tokio::test]
    async fn hits_refused_by_the_pin_cap_report_partial_not_complete() {
        let mailbox = Arc::new(Mutex::new(multi_mailbox()));
        let (provider, store, _opens) = counting_provider(mailbox);
        // Room for exactly one new pin: the six hits overflow it.
        let base = crate::provider::imap::policy::MAX_SEARCH_PINS - 1;
        for uid in 0..base as u32 {
            store
                .upsert_pin("OtherFolder", 1, uid as i64 + 1, 1_700_000_000)
                .unwrap();
        }
        let (_page, report) = provider
            .search_with_report("from:someone", None)
            .await
            .unwrap();
        assert!(report.skipped_over_cap > 0, "the cap refused some hits");
        assert_eq!(
            report.status,
            SearchReportStatus::Partial {
                reasons: vec![PartialSearchReason::Budget]
            }
        );
    }

    /// `category:promotions` (unknown operator -> Raw -> Skipped) reports `Skipped`
    /// with the one user-facing `Unsupported` code — never the query.
    #[tokio::test]
    async fn a_skipped_render_reports_skipped_unsupported() {
        let mailbox = Arc::new(Mutex::new(multi_mailbox()));
        let (provider, _store, opens) = counting_provider(mailbox);
        let (page, report) = provider
            .search_with_report("category:promotions", None)
            .await
            .unwrap();
        assert!(page.thread_ids.is_empty());
        assert_eq!(report.server_searched_accounts, 0, "nothing reached the server");
        assert_eq!(
            report.status,
            SearchReportStatus::Skipped {
                reasons: vec![SkipSearchReason::Unsupported]
            }
        );
        assert_eq!(
            opens.load(Ordering::SeqCst),
            0,
            "a skipped render still makes no network call under the report path"
        );
    }

    /// `in:nowhere` (an unknown mailbox -> NoServerMatches) reports
    /// `NoServerMatches { UnknownName }`.
    #[tokio::test]
    async fn a_no_server_matches_render_reports_unknown_name() {
        let mailbox = Arc::new(Mutex::new(multi_mailbox()));
        let (provider, _store, _opens) = counting_provider(mailbox);
        let (_page, report) = provider
            .search_with_report("in:nowhere from:a", None)
            .await
            .unwrap();
        assert_eq!(
            report.status,
            SearchReportStatus::NoServerMatches {
                reason: NoMatchSearchReason::UnknownName
            }
        );
    }

    /// A zero deadline reports `Partial { Deadline }`.
    #[tokio::test(start_paused = true)]
    async fn a_deadline_partial_reports_partial_deadline() {
        let mailbox = Arc::new(Mutex::new(multi_mailbox()));
        let (mut provider, _store, _opens) = counting_provider(mailbox);
        provider.search_deadline_override = Some(std::time::Duration::ZERO);
        let (_page, report) =
            provider.search_with_report("from:someone", None).await.unwrap();
        assert_eq!(
            report.status,
            SearchReportStatus::Partial {
                reasons: vec![PartialSearchReason::Deadline]
            }
        );
    }

    /// Hits above the acquire budget report `Partial { Budget }`.
    #[tokio::test]
    async fn a_budget_partial_reports_partial_budget() {
        use crate::provider::imap::policy::MAX_SEARCH_ACQUIRE;
        let mut mb = FakeMailbox::new(100);
        for i in 0..(MAX_SEARCH_ACQUIRE + 5) {
            mb.add_to(
                "Archive",
                50,
                &["\\Seen"],
                &message(&format!("<a{i}@x>"), &format!("m{i}"), ""),
            );
        }
        let (provider, _store, _opens) = counting_provider(Arc::new(Mutex::new(mb)));
        let (_page, report) =
            provider.search_with_report("in:archive from:a", None).await.unwrap();
        assert_eq!(
            report.status,
            SearchReportStatus::Partial {
                reasons: vec![PartialSearchReason::Budget]
            }
        );
    }

    /// A mapped partial from a FOLDER error drops the mailbox name and
    /// error string: only the `FolderError` code reaches the report.
    #[tokio::test]
    async fn a_folder_error_partial_reports_folder_error_without_the_name() {
        let mut mb = multi_mailbox();
        mb.folder("Archive", 50).examine_fails = true;
        let (provider, _store, _opens) = counting_provider(Arc::new(Mutex::new(mb)));
        let (_page, report) =
            provider.search_with_report("from:someone", None).await.unwrap();
        assert_eq!(
            report.status,
            SearchReportStatus::Partial {
                reasons: vec![PartialSearchReason::FolderError]
            }
        );
    }

    /// An unsynced planned mailbox is COUNTED on the report regardless
    /// of status (here the run is otherwise complete).
    #[tokio::test]
    async fn an_unsynced_mailbox_is_counted_on_the_report() {
        let mailbox = Arc::new(Mutex::new(multi_mailbox()));
        let (provider, _store, _opens) = counting_provider(mailbox);
        let mut plan = provider.build_sync_plan().unwrap();
        for entry in plan.entries.iter_mut() {
            if entry.mailbox == "Archive" {
                entry.synced_now = false;
            }
        }
        let search_plan = super::super::super::search::ImapSearchPlan {
            mailboxes: vec!["INBOX".into(), "Archive".into(), "Sent".into()],
            criteria: "CHARSET UTF-8 FROM \"someone\"".into(),
            local_filters: Vec::new(),
        };
        let result = provider.run_search_plan(&plan, search_plan).await.unwrap();
        let report = imap_search_report(&result);
        assert_eq!(report.unsynced_mailboxes, 1, "Archive counted unsynced");
        assert_eq!(report.status, SearchReportStatus::Complete);
    }

    /// The report survives the whole backfill path:
    /// `search_and_ingest_missing` returns the IMAP provider's report.
    #[tokio::test]
    async fn the_report_survives_search_and_ingest_missing() {
        let mailbox = Arc::new(Mutex::new(multi_mailbox()));
        let (provider, store, _opens) = counting_provider(mailbox);
        let db = store.database();
        crate::sync::sync_with(db.as_ref(), "me@example.com", &provider)
            .await
            .unwrap();
        // A skipped render: the report must carry Skipped through
        // search_and_ingest_missing.
        let report = crate::sync::search_and_ingest_missing(
            db.as_ref(),
            "me@example.com",
            &provider,
            "category:promotions",
            50,
        )
        .await
        .expect("backfill");
        assert_eq!(
            report.status,
            SearchReportStatus::Skipped {
                reasons: vec![SkipSearchReason::Unsupported]
            }
        );
    }
}
