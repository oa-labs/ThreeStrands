use super::*;

// An in-memory database so the store's SQL actually runs (no sandbox FS).
fn store() -> ImapStateStore {
    let database = Arc::new(Database::open_memory());
    ImapStateStore::new(database, "me@example.com")
}

#[test]
fn copy_intents_survive_reopen_and_are_account_and_destination_scoped() {
    let temp = crate::db::test_support::TempDbPath::new();
    {
        let database = Arc::new(Database::open(&temp.path).unwrap());
        let store = ImapStateStore::new(database.clone(), "me@example.com");
        store.begin_copy("m", "Labels/A", "original").unwrap();
        store.begin_copy("m", "Labels/A", "replacement").unwrap();
        store.begin_copy("m", "Labels/B", "other-label").unwrap();
        ImapStateStore::new(database, "other@example.com")
            .begin_copy("m", "Labels/A", "other-account")
            .unwrap();
    }
    let database = Arc::new(Database::open(&temp.path).unwrap());
    let store = ImapStateStore::new(database.clone(), "me@example.com");
    assert_eq!(
        store
            .pending_copy_identity("m", "Labels/A")
            .unwrap()
            .as_deref(),
        Some("original")
    );
    store.complete_copy("m", "Labels/A").unwrap();
    assert_eq!(store.pending_copy_identity("m", "Labels/A").unwrap(), None);
    assert_eq!(
        store
            .pending_copy_identity("m", "Labels/B")
            .unwrap()
            .as_deref(),
        Some("other-label")
    );
    assert_eq!(
        ImapStateStore::new(database, "other@example.com")
            .pending_copy_identity("m", "Labels/A")
            .unwrap()
            .as_deref(),
        Some("other-account")
    );
}

#[test]
fn a_mailbox_upserts_and_reads_back_with_its_counters() {
    let store = store();
    let inbox = ImapMailbox {
        name: "INBOX".into(),
        delimiter: Some("/".into()),
        special_use: None,
        uidvalidity: 95479608,
        uidnext: 979,
        highestmodseq: None,
        permanent_flags_json: Some(r#"["\\Seen","\\Flagged"]"#.into()),
        permanent_keywords: Some(false),
    };
    store.upsert_mailbox(&inbox).unwrap();
    // A second upsert updates in place rather than duplicating.
    let moved = ImapMailbox {
        uidnext: 1001,
        ..inbox.clone()
    };
    store.upsert_mailbox(&moved).unwrap();
    let mailboxes = store.mailboxes().unwrap();
    assert_eq!(mailboxes, vec![moved]);
}

#[test]
fn one_message_in_two_mailboxes_resolves_to_both_locations() {
    let store = store();
    for mailbox in ["INBOX", "Sent"] {
        store
            .upsert_location(&ImapLocation {
                mailbox: mailbox.into(),
                uidvalidity: 1,
                uid: 42,
                message_id: "imap:me@example.com:abc".into(),
                flags_json: r#"["\\Seen"]"#.into(),
                modseq: None,
            })
            .unwrap();
    }
    let locations = store.locations_for_message("imap:me@example.com:abc").unwrap();
    let mailboxes: Vec<_> = locations.iter().map(|location| location.mailbox.as_str()).collect();
    assert_eq!(mailboxes, vec!["INBOX", "Sent"]);
}

#[test]
fn dropping_a_mailboxs_locations_leaves_other_mailboxes_alone() {
    let store = store();
    for (mailbox, uid) in [("INBOX", 1), ("Archive", 2)] {
        store
            .upsert_location(&ImapLocation {
                mailbox: mailbox.into(),
                uidvalidity: 1,
                uid,
                message_id: format!("imap:me@example.com:{mailbox}"),
                flags_json: "[]".into(),
                modseq: None,
            })
            .unwrap();
    }
    store.drop_mailbox_locations("INBOX").unwrap();
    assert!(store
        .locations_for_message("imap:me@example.com:INBOX")
        .unwrap()
        .is_empty());
    assert_eq!(
        store.locations_for_message("imap:me@example.com:Archive").unwrap().len(),
        1
    );
}

#[test]
fn a_store_only_sees_its_own_accounts_rows() {
    let database = Arc::new(Database::open_memory());
    let mine = ImapStateStore::new(database.clone(), "me@example.com");
    let theirs = ImapStateStore::new(database, "other@example.com");
    theirs
        .upsert_location(&ImapLocation {
            mailbox: "INBOX".into(),
            uidvalidity: 1,
            uid: 1,
            message_id: "imap:other@example.com:x".into(),
            flags_json: "[]".into(),
            modseq: None,
        })
        .unwrap();
    // The other account's row is invisible here, and my own catalog is empty.
    assert!(mine
        .locations_for_message("imap:other@example.com:x")
        .unwrap()
        .is_empty());
    assert!(mine.mailboxes().unwrap().is_empty());
    mine.commit_sync_round(&SyncRoundWrite {
        hot_threads: vec!["mine".into()],
        ..Default::default()
    })
    .unwrap();
    theirs
        .commit_sync_round(&SyncRoundWrite {
            hot_threads: vec!["theirs".into()],
            ..Default::default()
        })
        .unwrap();
    assert_eq!(mine.hot_thread_ids().unwrap(), ["mine"]);
    assert_eq!(theirs.hot_thread_ids().unwrap(), ["theirs"]);
}

#[test]
fn an_empty_sync_round_is_a_no_op_and_does_not_bump_the_generation() {
    // SLICE5A_FIXES item 5 at the store level: committing an empty round
    // leaves the generation and journal untouched.
    let store = store();
    assert_eq!(store.generation().unwrap(), 0);
    let unchanged = store.commit_sync_round(&SyncRoundWrite::default()).unwrap();
    assert_eq!(unchanged, 0, "empty round returns the current generation");
    assert_eq!(store.generation().unwrap(), 0, "no bump");
    assert!(store.journal_since(0).unwrap().is_empty(), "no journal write");
}

#[test]
fn a_sync_round_applies_locations_deletions_and_journal_atomically() {
    // SLICE5A_FIXES item 1 at the store level: a round's location writes,
    // deletions, thread rows and journal all land together and bump the
    // generation exactly once.
    let store = store();
    // Pre-existing location to be deleted by the round.
    store
        .upsert_location(&ImapLocation {
            mailbox: "INBOX".into(),
            uidvalidity: 1,
            uid: 1,
            message_id: "imap:me@example.com:old".into(),
            flags_json: "[]".into(),
            modseq: None,
        })
        .unwrap();
    let round = SyncRoundWrite {
        locations: vec![ImapLocation {
            mailbox: "INBOX".into(),
            uidvalidity: 1,
            uid: 2,
            message_id: "imap:me@example.com:new".into(),
            flags_json: r#"["\\Seen"]"#.into(),
            modseq: None,
        }],
        deletions: vec![("INBOX".into(), 1, 1)],
        thread_assignments: vec![(
            "imap:me@example.com:new".into(),
            "imap:t:new".into(),
        )],
        changed_threads: vec!["imap:t:new".into()],
        ..Default::default()
    };
    let generation = store.commit_sync_round(&round).unwrap();
    assert_eq!(generation, 1);
    // The new location exists, the old one is gone.
    assert_eq!(
        store.locations_for_message("imap:me@example.com:new").unwrap().len(),
        1
    );
    assert!(store
        .locations_for_message("imap:me@example.com:old")
        .unwrap()
        .is_empty());
    // The thread was journaled under the new generation.
    assert_eq!(store.journal_since(0).unwrap(), vec!["imap:t:new".to_string()]);
}

#[test]
fn a_thread_keeps_its_earliest_creation_generation() {
    // SLICE5A_FIXES item 4 at the store level: a thread's creation
    // generation is the minimum across its messages and never increases
    // when a later message joins.
    let store = store();
    store
        .commit_sync_round(&SyncRoundWrite {
            thread_assignments: vec![("imap:me@example.com:m1".into(), "imap:t:x".into())],
            changed_threads: vec!["imap:t:x".into()],
            ..Default::default()
        })
        .unwrap();
    assert_eq!(store.thread_created_generation("imap:t:x").unwrap(), Some(1));
    // A later round adds a second message to the same thread.
    store
        .commit_sync_round(&SyncRoundWrite {
            thread_assignments: vec![("imap:me@example.com:m2".into(), "imap:t:x".into())],
            changed_threads: vec!["imap:t:x".into()],
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        store.thread_created_generation("imap:t:x").unwrap(),
        Some(1),
        "the thread keeps its earliest creation generation"
    );
}

#[test]
fn hotness_and_journaling_follow_chained_aliases_atomically() {
    let store = store();
    let initial = store
        .commit_sync_round(&SyncRoundWrite {
            thread_assignments: vec![("hot-message".into(), "hot".into())],
            hot_threads: vec!["hot".into()],
            ..Default::default()
        })
        .unwrap();
    // Fail after alias writes, while materializing inherited hotness.
    store
        .database()
        .with_connection(|c| {
            c.execute_batch(
                "CREATE TRIGGER reject_hot_survivor BEFORE INSERT ON imap_hot_threads
            WHEN NEW.thread_id = 'survivor'
            BEGIN SELECT RAISE(ABORT, 'injected hotness failure'); END;",
            )?;
            Ok(())
        })
        .unwrap();
    let round = SyncRoundWrite {
        aliases: vec![
            ("hot".into(), "middle".into()),
            ("middle".into(), "survivor".into()),
        ],
        ..Default::default()
    };
    assert!(store.commit_sync_round(&round).is_err());
    assert_eq!(store.generation().unwrap(), initial);
    assert_eq!(
        store.thread_of_message("hot-message").unwrap().as_deref(),
        Some("hot")
    );
    assert_eq!(store.resolve_thread_alias("hot").unwrap(), "hot");
    assert!(!store.is_thread_hot("survivor").unwrap());
    assert!(store.journal_since(initial).unwrap().is_empty());
    store
        .database()
        .with_connection(|c| {
            c.execute_batch("DROP TRIGGER reject_hot_survivor;")?;
            Ok(())
        })
        .unwrap();
    assert_eq!(store.commit_sync_round(&round).unwrap(), initial + 1);
    assert_eq!(store.resolve_thread_alias("hot").unwrap(), "survivor");
    assert!(store.is_thread_hot("survivor").unwrap());
    assert_eq!(store.hot_thread_ids().unwrap(), ["hot", "survivor"]);
    assert_eq!(
        store.journal_since(initial).unwrap(),
        ["hot", "middle", "survivor"]
    );
    let canonical_markers: i64 = store
        .database()
        .with_connection(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM imap_hot_threads
            WHERE account_id = ?1 AND thread_id = 'survivor'",
                [&store.account_id],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(canonical_markers, 1);
}

#[test]
fn a_cold_merge_stays_index_only_and_does_not_inherit_another_accounts_hotness() {
    let store = store();
    let other = ImapStateStore::new(store.database().clone(), "other@example.com");
    other
        .commit_sync_round(&SyncRoundWrite {
            hot_threads: vec!["old".into()],
            ..Default::default()
        })
        .unwrap();
    let generation = store
        .commit_sync_round(&SyncRoundWrite {
            aliases: vec![("old".into(), "survivor".into())],
            ..Default::default()
        })
        .unwrap();
    assert!(!store.is_thread_hot("survivor").unwrap());
    assert!(store.journal_since(0).unwrap().is_empty());
    assert_eq!(generation, 1);
}

#[test]
fn legacy_alias_families_keep_their_tokens_creation_order_and_hotness() {
    let store = store();
    // Earlier versions could leave the only hot marker on a retired id.
    // Include members still stored under several raw ids, a tokenless
    // member, and same-named aliases in another account.
    store
        .database()
        .with_connection(|c| {
            c.execute_batch(
                "INSERT INTO imap_threads VALUES
            ('me@example.com','root-message','root',1),
            ('me@example.com','old-message','old',3),
            ('me@example.com','bare-message','middle',2),
            ('other@example.com','foreign-message','root',0);
            INSERT INTO imap_message_tokens VALUES
            ('me@example.com','root-message','root-token'),
            ('me@example.com','old-message','old-token'),
            ('other@example.com','foreign-message','foreign-token');
            INSERT INTO imap_thread_aliases VALUES
            ('me@example.com','old','middle'),
            ('me@example.com','middle','root'),
            ('other@example.com','root','foreign-root');
            INSERT INTO imap_hot_threads(account_id, thread_id) VALUES ('me@example.com','old');",
            )?;
            Ok(())
        })
        .unwrap();
    assert!(store.is_thread_hot("root").unwrap());
    assert!(store.is_thread_hot("middle").unwrap());
    assert!(
        !ImapStateStore::new(store.database().clone(), "other@example.com")
            .is_thread_hot("root")
            .unwrap()
    );
    for token in ["root-token", "old-token"] {
        let seeded = store.seed_state_for_tokens(&[token.into()]).unwrap();
        assert_eq!(seeded.len(), 1);
        assert_eq!(seeded["root"].0, 1);
        assert_eq!(
            seeded["root"].1,
            [
                "bare-message",
                "old-message",
                "old-token",
                "root-message",
                "root-token"
            ]
        );
    }
    // Corrupt cycles must still terminate in both reverse traversals.
    store
        .database()
        .with_connection(|c| {
            c.execute(
                "INSERT INTO imap_thread_aliases VALUES ('me@example.com','root','old')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    assert!(store.is_thread_hot("root").unwrap());
    assert_eq!(
        store
            .seed_state_for_tokens(&["root-token".into()])
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn token_seeding_work_does_not_grow_with_unrelated_messages_and_aliases() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let store = store();
    store
        .commit_sync_round(&SyncRoundWrite {
            thread_assignments: vec![("reply-parent".into(), "target".into())],
            message_tokens: vec![("reply-parent".into(), vec!["parent-token".into()])],
            ..Default::default()
        })
        .unwrap();
    let measured_seed = || {
        let operations = Arc::new(AtomicUsize::new(0));
        let counter = operations.clone();
        store
            .database()
            .with_connection(|c| {
                c.progress_handler(
                    1,
                    Some(move || {
                        counter.fetch_add(1, Ordering::Relaxed);
                        false
                    }),
                )?;
                Ok(())
            })
            .unwrap();
        let seeded = store
            .seed_state_for_tokens(&["parent-token".into()])
            .unwrap();
        store
            .database()
            .with_connection(|c| {
                c.progress_handler(0, None::<fn() -> bool>)?;
                Ok(())
            })
            .unwrap();
        (seeded, operations.load(Ordering::Relaxed))
    };
    let (expected, small_work) = measured_seed();
    // Populate history directly so the measurement covers just seeding,
    // and not the cost of creating the unrelated fixture.
    store
        .database()
        .with_transaction(|tx| {
            for n in 0..2_000 {
                let message = format!("unrelated-message-{n}");
                let thread = format!("unrelated-thread-{n}");
                tx.execute(
                    "INSERT INTO imap_threads VALUES (?1, ?2, ?3, 1)",
                    rusqlite::params![store.account_id, message, thread],
                )?;
                tx.execute(
                    "INSERT INTO imap_message_tokens VALUES (?1, ?2, ?3)",
                    rusqlite::params![
                        store.account_id,
                        message,
                        format!("unrelated-token-{n}")
                    ],
                )?;
                tx.execute(
                    "INSERT INTO imap_thread_aliases VALUES (?1, ?2, ?3)",
                    rusqlite::params![store.account_id, format!("unrelated-old-{n}"), thread],
                )?;
            }
            Ok(())
        })
        .unwrap();
    let (actual, large_work) = measured_seed();
    assert_eq!(actual, expected);
    assert!(large_work <= small_work * 2 + 100,
        "seeding one thread must not scan unrelated history: {small_work} -> {large_work} VM operations");
}

/// An initial sync threads thousands of messages in ONE round, so the
/// incoming token set can far exceed SQLite's bound-variable limit. The
/// seeder must cope (chunked queries), not fail the round.
#[test]
fn seeding_copes_with_a_token_set_larger_than_sqlites_variable_limit() {
    let store = store();
    store
        .commit_sync_round(&SyncRoundWrite {
            thread_assignments: vec![("imap:me@example.com:m1".into(), "imap:t:x".into())],
            message_tokens: vec![(
                "imap:me@example.com:m1".into(),
                vec!["imap:me@example.com:m1".into(), "tok-0".into()],
            )],
            changed_threads: vec!["imap:t:x".into()],
            ..Default::default()
        })
        .unwrap();
    let tokens: Vec<String> = (0..100_000).map(|n| format!("tok-{n}")).collect();
    let seeded = store.seed_state_for_tokens(&tokens).unwrap();
    assert_eq!(seeded.len(), 1, "the one thread sharing tok-0 is seeded");
}

#[test]
fn attempts_are_account_scoped_and_do_not_claim_successful_coverage() {
    let store = store();
    store.record_mailbox_attempt("A").unwrap();
    store.record_mailbox_attempt("B").unwrap();
    let restarted = ImapStateStore::new(store.database().clone(), store.account_id());
    restarted.record_mailbox_attempt("A").unwrap();
    assert_eq!(restarted.mailbox_last_attempt("A").unwrap(), 3);
    assert_eq!(restarted.mailbox_last_attempt("B").unwrap(), 2);
    assert_eq!(restarted.mailbox_visited_in_epoch("A").unwrap(), -1);
    assert_eq!(restarted.mailbox_last_visited_at("A").unwrap(), 0);
    let (_, outstanding) = restarted
        .advance_coverage_epoch_if_complete(&["A".into(), "B".into()])
        .unwrap();
    assert_eq!(outstanding, ["A", "B"]);
    assert_eq!(store.generation().unwrap(), 0);
    assert_eq!(store.complete_walks().unwrap(), 0);
    let other = ImapStateStore::new(store.database().clone(), "other@example.com");
    assert_eq!(other.mailbox_last_attempt("A").unwrap(), 0);
    other.record_mailbox_attempt("A").unwrap();
    assert_eq!(other.mailbox_last_attempt("A").unwrap(), 1);
    assert_eq!(store.mailbox_last_attempt("A").unwrap(), 3);
}

#[test]
fn expiry_acknowledgment_is_atomic_durable_and_rearms_for_a_new_empty_period() {
    let store = store();
    let initial = store
        .commit_sync_round(&SyncRoundWrite {
            hot_threads: vec!["hot".into()],
            ..Default::default()
        })
        .unwrap();
    store.mark_hot_thread_emptied("hot", 0).unwrap();
    let (generation, _) = store.advance_coverage_epoch_if_complete(&[]).unwrap();
    assert_eq!(generation, initial, "one epoch is below the grace boundary");
    store.database().with_connection(|c| {
        c.execute_batch(
            "CREATE TRIGGER reject_expiry_ack BEFORE UPDATE OF expiry_journaled ON imap_hot_threads
             WHEN NEW.expiry_journaled = 1
             BEGIN SELECT RAISE(ABORT, 'injected expiry failure'); END;",
        )?;
        Ok(())
    }).unwrap();
    assert!(store.advance_coverage_epoch_if_complete(&[]).is_err());
    assert_eq!(store.complete_walks().unwrap(), 1);
    assert_eq!(store.generation().unwrap(), initial);
    assert!(store.journal_since(initial).unwrap().is_empty());
    store
        .database()
        .with_connection(|c| {
            c.execute_batch("DROP TRIGGER reject_expiry_ack;")?;
            Ok(())
        })
        .unwrap();
    let (expired, _) = store.advance_coverage_epoch_if_complete(&[]).unwrap();
    assert_eq!(expired, initial + 1);
    assert_eq!(store.journal_since(initial).unwrap(), ["hot"]);
    let restarted = ImapStateStore::new(store.database().clone(), store.account_id());
    let (idle, _) = restarted.advance_coverage_epoch_if_complete(&[]).unwrap();
    assert_eq!(idle, expired, "acknowledgment survives reconstruction");
    assert_eq!(
        restarted.hot_thread_emptied_at_walk("hot").unwrap(),
        Some(0)
    );
    restarted.clear_hot_thread_emptied("hot").unwrap(); // regained a location
    restarted.mark_hot_thread_emptied("hot", 3).unwrap(); // lost it again
    let (below, _) = restarted.advance_coverage_epoch_if_complete(&[]).unwrap();
    assert_eq!(below, expired);
    let (second_expiry, _) = restarted.advance_coverage_epoch_if_complete(&[]).unwrap();
    assert_eq!(
        second_expiry,
        expired + 1,
        "a new empty period gets its own notification"
    );
    assert_eq!(restarted.journal_since(expired).unwrap(), ["hot"]);
    let (above, _) = restarted.advance_coverage_epoch_if_complete(&[]).unwrap();
    assert_eq!(above, second_expiry);
}

// ---- Slice 5b-2 run B: vanished-mailbox retirement safety rules --------

fn catalog_mb(store: &ImapStateStore, name: &str) {
    store
        .upsert_mailbox(&ImapMailbox {
            name: name.into(),
            delimiter: Some("/".into()),
            special_use: None,
            uidvalidity: 1,
            uidnext: 1,
            highestmodseq: None,
            permanent_flags_json: None,
            permanent_keywords: None,
        })
        .unwrap();
}

#[test]
fn retirement_never_touches_inbox_and_an_empty_list_retires_nothing() {
    let store = store();
    catalog_mb(&store, "INBOX");
    catalog_mb(&store, "Clients/Acme");

    // An empty present-set against a non-empty catalog is a FAILURE signal:
    // retire nothing (safety rule).
    let retired = store.retire_vanished_mailboxes(&[]).unwrap();
    assert!(retired.is_empty(), "an empty LIST retires nothing");
    assert_eq!(store.mailboxes().unwrap().len(), 2, "catalog untouched");

    // A successful LIST missing Clients/Acme retires exactly it; INBOX is
    // never retired even though it, too, is "present" only implicitly.
    let retired = store
        .retire_vanished_mailboxes(&["INBOX".to_string()])
        .unwrap();
    assert_eq!(retired, vec!["Clients/Acme".to_string()]);
    let names: Vec<String> = store.mailboxes().unwrap().into_iter().map(|m| m.name).collect();
    assert_eq!(names, vec!["INBOX".to_string()], "only INBOX remains");

    // A LIST that still omits INBOX never retires it.
    let retired = store.retire_vanished_mailboxes(&["Something/Else".to_string()]).unwrap();
    assert!(retired.is_empty(), "INBOX is never retired");
    assert!(store.mailboxes().unwrap().iter().any(|m| m.name == "INBOX"));
}

#[test]
fn retirement_deletes_locations_and_sync_state_and_journals_hot_threads() {
    let store = store();
    catalog_mb(&store, "INBOX");
    catalog_mb(&store, "Clients/Acme");
    // A hot thread with a location in INBOX and in the user folder.
    store
        .commit_sync_round(&SyncRoundWrite {
            locations: vec![
                ImapLocation {
                    mailbox: "INBOX".into(),
                    uidvalidity: 1,
                    uid: 1,
                    message_id: "imap:me@example.com:m1".into(),
                    flags_json: "[]".into(),
                    modseq: None,
                },
                ImapLocation {
                    mailbox: "Clients/Acme".into(),
                    uidvalidity: 1,
                    uid: 2,
                    message_id: "imap:me@example.com:m1".into(),
                    flags_json: "[]".into(),
                    modseq: None,
                },
            ],
            thread_assignments: vec![("imap:me@example.com:m1".into(), "imap:t:1".into())],
            hot_threads: vec!["imap:t:1".into()],
            changed_threads: vec!["imap:t:1".into()],
            ..Default::default()
        })
        .unwrap();
    // Give the user folder a sync-state row too.
    store.record_catalog_refresh(0).unwrap();
    let gen_before = store.generation().unwrap();

    let retired = store
        .retire_vanished_mailboxes(&["INBOX".to_string()])
        .unwrap();
    assert_eq!(retired, vec!["Clients/Acme".to_string()]);
    // Locations, sync-state and catalog row for the folder are gone.
    assert!(store.locations_in_mailbox("Clients/Acme").unwrap().is_empty());
    assert!(store.mailbox_sync_state("Clients/Acme").unwrap().is_none());
    assert!(!store.mailboxes().unwrap().iter().any(|m| m.name == "Clients/Acme"));
    // The INBOX copy survives, so the thread is still hot and present.
    assert!(store.is_thread_hot("imap:t:1").unwrap());
    assert!(!store.locations_in_mailbox("INBOX").unwrap().is_empty());
    // The affected hot thread was journaled under a bumped generation.
    let gen_after = store.generation().unwrap();
    assert!(gen_after > gen_before, "a hot-thread retirement bumps the generation");
    assert!(store.journal_since(gen_before).unwrap().contains(&"imap:t:1".to_string()));
}
