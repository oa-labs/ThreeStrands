use super::*;
use crate::db::test_support::{database, message};

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-11-01T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}
fn evidence(thread: &str, age: i64, emails: &[&str]) -> Evidence {
    Evidence {
        account: "you@example.com".into(),
        thread: thread.into(),
        date: now() - Duration::days(age),
        recipients: emails
            .iter()
            .map(|email| (email.to_string(), None))
            .collect(),
    }
}
fn seeds(emails: &[&str]) -> BTreeSet<String> {
    emails.iter().map(|s| s.to_string()).collect()
}
fn sent(db: &Database, id: &str, recipients: &[&str]) -> NormalizedMessage {
    let mut m = message(
        id,
        id,
        &(Utc::now() - Duration::days(1)).to_rfc3339(),
        "body",
    );
    m.labels = vec!["SENT".into()];
    m.from = "you@example.com".into();
    m.to = recipients.iter().map(|s| s.to_string()).collect();
    db.upsert_thread("you@example.com", &[m.clone()]).unwrap();
    m
}
fn relations(db: &Database, account: Option<&str>, raw: &[&str]) -> HashMap<String, Relationship> {
    db.with_connection(|c| {
        relationships(
            c,
            account,
            &raw.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            Utc::now(),
        )
    })
    .unwrap()
}

#[test]
fn score_requires_two_messages_and_prefers_shared_seeds_then_recent_small_groups() {
    let mut messages = vec![
        evidence("a", 1, &["alice", "bob"]),
        evidence("b", 2, &["alice", "bob"]),
    ];
    assert!(score(&messages[..1], &seeds(&["alice"]), now()).is_empty());
    assert_eq!(
        score(&messages, &seeds(&["alice"]), now())["bob"].seeds,
        vec!["alice"]
    );
    messages.extend([
        evidence("c", 30, &["alice", "carol", "dave"]),
        evidence("d", 31, &["alice", "carol", "dave"]),
    ]);
    let r = score(&messages, &seeds(&["alice"]), now());
    assert!(r["bob"].strength > r["carol"].strength);
    let r = score(&messages, &seeds(&["alice", "dave"]), now());
    assert_eq!(r["carol"].seeds, vec!["alice", "dave"]);
    assert!(compare(r.get("carol"), r.get("bob")).is_lt());
}

#[test]
fn score_window_size_support_and_future_boundaries() {
    for age in [WINDOW_DAYS - 1, WINDOW_DAYS, WINDOW_DAYS + 1] {
        let r = score(
            &[
                evidence("a", age, &["alice", "bob"]),
                evidence("b", age, &["alice", "bob"]),
            ],
            &seeds(&["alice"]),
            now(),
        );
        assert_eq!(r.contains_key("bob"), age <= WINDOW_DAYS);
    }
    for size in [MAX_RECIPIENTS - 1, MAX_RECIPIENTS, MAX_RECIPIENTS + 1] {
        let names: Vec<_> = (0..size).map(|n| format!("person{n}")).collect();
        let names: Vec<_> = names.iter().map(String::as_str).collect();
        let r = score(
            &[evidence("a", 0, &names), evidence("b", 0, &names)],
            &seeds(&["person0"]),
            now(),
        );
        assert_eq!(r.contains_key("person1"), size <= MAX_RECIPIENTS);
    }
    let future = score(
        &[
            evidence("a", -10, &["alice", "bob"]),
            evidence("b", -10, &["alice", "bob"]),
        ],
        &seeds(&["alice"]),
        now(),
    );
    assert_eq!(future["bob"].strength, 1.0);
    let r = score(
        &[
            evidence("a", 0, &["alice", "bob"]),
            evidence("b", 0, &["alice", "bob"]),
        ],
        &seeds(&["alice"]),
        now(),
    );
    assert_eq!(r["bob"].strength, 1.0);
}

#[test]
fn repeated_thread_day_is_capped_but_separate_days_and_accounts_count() {
    let r = score(
        &[
            evidence("same", 0, &["alice", "bob"]),
            evidence("same", 0, &["alice", "bob"]),
        ],
        &seeds(&["alice"]),
        now(),
    );
    assert_eq!(r["bob"].strength, 0.5);
    let mut other = evidence("same", 0, &["alice", "bob"]);
    other.account = "work@example.com".into();
    let r = score(
        &[evidence("same", 0, &["alice", "bob"]), other],
        &seeds(&["alice"]),
        now(),
    );
    assert_eq!(r["bob"].strength, 1.0);
}

#[test]
fn visible_headers_include_cc_without_changing_counts_or_learning_bcc_incoming_and_drafts() {
    let db = database();
    for id in ["visible1", "visible2"] {
        let mut m = sent(
            &db,
            id,
            &[
                "Alice <alice@example.com>",
                "ALICE@example.com",
                "you@example.com",
            ],
        );
        m.cc = vec!["Bob <bob@example.com>".into()];
        m.cc_bcc = vec!["Bob <bob@example.com>".into(), "secret@example.com".into()];
        db.upsert_thread("you@example.com", &[m.clone()]).unwrap();
        db.upsert_thread("you@example.com", &[m]).unwrap();
    }
    for id in ["incoming1", "incoming2"] {
        let mut m = message(id, id, &Utc::now().to_rfc3339(), "body");
        m.to = vec!["alice@example.com".into(), "outsider@example.com".into()];
        db.upsert_thread("you@example.com", &[m]).unwrap();
    }
    for id in ["draft1", "draft2"] {
        let mut m = sent(&db, id, &["alice@example.com", "draft-person@example.com"]);
        m.labels = vec!["DRAFT".into()];
        db.upsert_thread("you@example.com", &[m]).unwrap();
    }
    let r = relations(&db, Some("you@example.com"), &["alice@example.com"]);
    assert_eq!(r.len(), 1);
    assert_eq!(r["bob@example.com"].seeds, vec!["alice@example.com"]);
    assert!(relations(&db, Some("other@example.com"), &["alice@example.com"]).is_empty());
    let before = db
        .list_contact_suggestions("you@example.com", "bob", 8)
        .unwrap();
    assert!(
        before.is_empty(),
        "Cc evidence must not change existing contact counts"
    );
    let result = db
        .contextual_contact_suggestions(
            "you@example.com",
            "bob",
            1,
            &["alice@example.com".into()],
            &["alice@example.com".into()],
        )
        .unwrap();
    assert_eq!(result[0].email, "bob@example.com");
    assert_eq!(result[0].sent_count, 0);
    assert_eq!(result[0].display_name.as_deref(), Some("Bob"));
    assert!(db
        .contextual_contact_suggestions(
            "you@example.com",
            "unrelated",
            8,
            &["alice@example.com".into()],
            &[]
        )
        .unwrap()
        .is_empty());
}

#[test]
fn contextual_pool_is_ranked_before_truncation_and_honors_pins_suppression_exclusion() {
    let db = database();
    for id in ["pair1", "pair2"] {
        sent(&db, id, &["alice@example.com", "bob@example.com"]);
    }
    for n in 0..10 {
        for occurrence in 0..3 {
            sent(
                &db,
                &format!("frequent{n}-{occurrence}"),
                &[&format!("frequent{n}@example.com")],
            );
        }
    }
    let context = vec!["alice@example.com".into()];
    let r = db
        .contextual_contact_suggestions("you@example.com", "", 1, &context, &context)
        .unwrap();
    assert_eq!(r[0].email, "bob@example.com");
    db.pin_contact("you@example.com", "pinned@example.com", None)
        .unwrap();
    assert_eq!(
        db.contextual_contact_suggestions("you@example.com", "", 1, &context, &context)
            .unwrap()[0]
            .email,
        "pinned@example.com"
    );
    db.with_connection(|c| {
        c.execute(
            "INSERT INTO contact_suggestion_suppressions(email) VALUES('bob@example.com')",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    assert!(db
        .contextual_contact_suggestions("you@example.com", "bob", 8, &context, &context)
        .unwrap()
        .is_empty());
    assert!(db
        .contextual_contact_suggestions(
            "you@example.com",
            "pinned",
            8,
            &context,
            &["pinned@example.com".into()]
        )
        .unwrap()
        .is_empty());
    assert!(relations(
        &db,
        Some("you@example.com"),
        &vec!["alice@example.com"; MAX_CONTEXT + 1]
    )
    .is_empty());
}

#[test]
fn reindex_replaces_evidence_and_backfill_resumes_without_double_counting() {
    let db = database();
    for id in ["old1", "old2"] {
        sent(&db, id, &["alice@example.com", "bob@example.com"]);
    }
    db.with_connection(|c| {
        c.execute("DELETE FROM co_recipient_addresses", [])?;
        Ok(())
    })
    .unwrap();
    assert!(relations(&db, Some("you@example.com"), &["alice@example.com"]).is_empty());
    assert_eq!(db.backfill_co_recipients(1).unwrap(), 1);
    while db.backfill_co_recipients(1).unwrap() != 0 {}
    assert!(
        relations(&db, Some("you@example.com"), &["alice@example.com"])
            .contains_key("bob@example.com")
    );
    assert_eq!(db.backfill_co_recipients(20).unwrap(), 0);
    let updated = sent(&db, "old1", &["alice@example.com", "carol@example.com"]);
    db.upsert_thread("you@example.com", &[updated]).unwrap();
    assert!(relations(&db, Some("you@example.com"), &["alice@example.com"]).is_empty());
    db.with_connection(|c| {
        c.execute("DELETE FROM messages WHERE id='old2'", [])?;
        assert_eq!(
            c.query_row(
                "SELECT count(*) FROM co_recipient_addresses WHERE message_id='old2'",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            0
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn malformed_bulk_and_oversized_messages_do_not_establish_relationships() {
    let db = database();
    for id in ["bad1", "bad2"] {
        let mut m = sent(&db, id, &["alice@example.com", "bob@example.com"]);
        m.date = "broken date".into();
        db.upsert_thread("you@example.com", &[m]).unwrap();
    }
    assert!(relations(&db, Some("you@example.com"), &["alice@example.com"]).is_empty());
    for id in ["large1", "large2"] {
        let mut names = vec!["alice@example.com".to_string(), "bob@example.com".into()];
        names.extend((0..MAX_RECIPIENTS).map(|i| format!("bulk{i}@example.com")));
        sent(
            &db,
            id,
            &names.iter().map(String::as_str).collect::<Vec<_>>(),
        );
    }
    assert!(relations(&db, Some("you@example.com"), &["alice@example.com"]).is_empty());
}

#[test]
fn lookup_uses_seed_index_and_has_bounded_large_mailbox_work() {
    let db = database();
    sent(&db, "pair1", &["alice@example.com", "bob@example.com"]);
    sent(&db, "pair2", &["alice@example.com", "bob@example.com"]);
    db.with_transaction(|tx| {
        // Thousands of unrelated cached header rows must not enter the seed lookup.
        for i in 0..5000 { tx.execute("INSERT INTO co_recipient_addresses(message_id,account_id,email,sender,sent_at) VALUES('pair1','you@example.com',?1,'you@example.com','2026-11-01T00:00:00+00:00')",[format!("unrelated{i}@example.com")])?; }
        let plan:String=tx.query_row("EXPLAIN QUERY PLAN SELECT message_id FROM co_recipient_addresses WHERE account_id=?1 AND email=?2 AND sent_at>=?3",["you@example.com","alice@example.com","2026"],|r|r.get(3))?;
        assert!(plan.contains("co_recipient_seed"),"{plan}");
        Ok(())
    }).unwrap();
    let start = std::time::Instant::now();
    // One malformed oversized message is rejected, leaving insufficient support.
    assert!(relations(&db, Some("you@example.com"), &["alice@example.com"]).is_empty());
    eprintln!("5000 unrelated header rows: {:?}", start.elapsed());
}

#[test]
fn sending_identity_changes_apply_without_reindex_and_self_never_becomes_a_suggestion() {
    use crate::provider::SendingIdentity;
    let db = database();
    db.adopt_account("you@example.com").unwrap();
    for id in ["alias1", "alias2"] {
        let mut m = sent(
            &db,
            id,
            &["alice@example.com", "bob@example.com", "alias@example.com"],
        );
        m.from = "alias@example.com".into();
        db.upsert_thread("you@example.com", &[m]).unwrap();
    }
    assert!(relations(&db, Some("you@example.com"), &["alice@example.com"]).is_empty());
    let identities = vec![
        SendingIdentity {
            address: "you@example.com".into(),
            ..Default::default()
        },
        SendingIdentity {
            address: "alias@example.com".into(),
            ..Default::default()
        },
    ];
    db.store_gmail_sending_identities("you@example.com", &identities, 1)
        .unwrap();
    let r = relations(&db, Some("you@example.com"), &["alice@example.com"]);
    assert!(r.contains_key("bob@example.com"));
    assert!(!r.contains_key("alias@example.com"));
    db.store_gmail_sending_identities("you@example.com", &identities[..1], 2)
        .unwrap();
    assert!(relations(&db, Some("you@example.com"), &["alice@example.com"]).is_empty());
}

#[test]
fn group_profile_ranking_uses_secondary_addresses_and_shared_accounts_without_saving_typed_seeds() {
    let db = database();
    db.adopt_account("you@example.com").unwrap();
    for id in ["group1", "group2"] {
        sent(&db, id, &["alice@example.com", "bob-work@example.com"]);
    }
    let alice = db
        .create_contact_group("Team", &[], &["alice@example.com".into()])
        .unwrap()
        .group;
    let bob = db
        .create_contact_group("Other", &[], &["bob@example.com".into()])
        .unwrap()
        .group;
    db.with_connection(|c| {c.execute("INSERT INTO contact_addresses(contact_id,email,position) VALUES(?1,'bob-work@example.com',1)",[&bob.member_ids[0]])?;Ok(())}).unwrap();
    let ranked = db
        .rank_contact_group_candidates(&alice.id, &[], &[])
        .unwrap();
    assert_eq!(ranked[0].id, bob.member_ids[0]);
    assert_eq!(ranked[0].related_to, vec!["alice@example.com"]);
    let before = db.list_contact_profiles("", 5000).unwrap().len();
    let empty = db.create_contact_group("Empty", &[], &[]).unwrap().group;
    assert_eq!(
        db.rank_contact_group_candidates(&empty.id, &[], &["alice@example.com".into()])
            .unwrap()[0]
            .id,
        bob.member_ids[0]
    );
    assert_eq!(db.list_contact_profiles("", 5000).unwrap().len(), before);
    assert_eq!(
        db.get_contact_profile(&bob.member_ids[0])
            .unwrap()
            .unwrap()
            .addresses[0],
        "bob@example.com"
    );
    assert!(db
        .get_contact_group(&empty.id)
        .unwrap()
        .unwrap()
        .member_ids
        .is_empty());
    db.with_connection(|c| {
        c.execute(
            "INSERT INTO contact_suggestion_suppressions(email) VALUES('bob@example.com')",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    assert!(
        !db.rank_contact_group_candidates(&alice.id, &[], &[])
            .unwrap()
            .iter()
            .any(|item| item.id == bob.member_ids[0]),
        "Secondary-address evidence must not promote a hidden primary address"
    );
}

#[test]
fn retention_and_account_removal_cascade_local_evidence() {
    let db = database();
    db.adopt_account("you@example.com").unwrap();
    for id in ["expire1", "expire2"] {
        let mut m = sent(&db, id, &["alice@example.com", "bob@example.com"]);
        m.date = (Utc::now() - Duration::days(60)).to_rfc3339();
        db.upsert_thread("you@example.com", &[m]).unwrap();
    }
    assert!(
        relations(&db, Some("you@example.com"), &["alice@example.com"])
            .contains_key("bob@example.com")
    );
    db.set_retention_days(Some(30)).unwrap();
    db.prune_expired_threads().unwrap();
    assert!(relations(&db, Some("you@example.com"), &["alice@example.com"]).is_empty());
    for id in ["remove1", "remove2"] {
        sent(&db, id, &["alice@example.com", "bob@example.com"]);
    }
    db.remove_account("you@example.com").unwrap();
    db.with_connection(|c| {
        assert_eq!(
            c.query_row(
                "SELECT count(*) FROM co_recipient_addresses WHERE account_id='you@example.com'",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            0
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn preceding_schema_upgrade_creates_empty_local_index_then_backfills_cached_cc() {
    let path = crate::db::test_support::TempDbPath::new();
    {
        let db = Database::open(&path.path).unwrap();
        for id in ["upgrade1", "upgrade2"] {
            let mut m = sent(&db, id, &["alice@example.com"]);
            m.cc = vec!["bob@example.com".into()];
            db.upsert_thread("you@example.com", &[m]).unwrap();
        }
        db.with_connection(|c| {
            c.execute_batch("DROP TABLE co_recipient_addresses; PRAGMA user_version=93;")?;
            Ok(())
        })
        .unwrap();
    }
    {
        let db = Database::open(&path.path).unwrap();
        assert!(relations(&db, Some("you@example.com"), &["alice@example.com"]).is_empty());
        assert_eq!(db.backfill_co_recipients(1).unwrap(), 1);
    }
    // A restart continues after the committed cursor.
    let db = Database::open(&path.path).unwrap();
    while db.backfill_co_recipients(1).unwrap() != 0 {}
    assert!(
        relations(&db, Some("you@example.com"), &["alice@example.com"])
            .contains_key("bob@example.com")
    );
    assert!(db
        .list_contact_suggestions("you@example.com", "bob", 8)
        .unwrap()
        .is_empty());
}

#[test]
fn sent_folder_and_bulk_headers_are_required_in_addition_to_sender_identity() {
    let db = database();
    for id in ["spoof1", "spoof2"] {
        let mut m = sent(&db, id, &["alice@example.com", "bob@example.com"]);
        m.labels = vec!["INBOX".into()];
        db.upsert_thread("you@example.com", &[m]).unwrap();
    }
    assert!(relations(&db, Some("you@example.com"), &["alice@example.com"]).is_empty());
    for (header, value) in [("List-Id", "a list"), ("Precedence", "bulk")] {
        for id in ["bulk1", "bulk2"] {
            let mut m = sent(&db, id, &["alice@example.com", "bob@example.com"]);
            m.metadata_json=serde_json::json!({"id":id,"threadId":id,"payload":{"headers":[{"name":header,"value":value}]}}).to_string();
            db.upsert_thread("you@example.com", &[m]).unwrap();
        }
        assert!(relations(&db, Some("you@example.com"), &["alice@example.com"]).is_empty());
    }
}

#[test]
fn shared_group_evidence_excludes_all_connected_self_addresses() {
    let db = database();
    db.adopt_account("you@example.com").unwrap();
    db.adopt_account("work@example.com").unwrap();
    for id in ["self1", "self2"] {
        sent(
            &db,
            id,
            &["alice@example.com", "bob@example.com", "work@example.com"],
        );
    }
    let r = relations(&db, None, &["alice@example.com"]);
    assert!(r.contains_key("bob@example.com"));
    assert!(!r.contains_key("work@example.com"));
    // Compose remains scoped to its account's identity set.
    assert!(
        relations(&db, Some("you@example.com"), &["alice@example.com"])
            .contains_key("work@example.com")
    );
}

#[test]
fn indexed_lookup_ignores_thousands_of_unrelated_cached_messages() {
    let db = database();
    db.adopt_account("you@example.com").unwrap();
    for id in ["benchmark1", "benchmark2"] {
        sent(&db, id, &["alice@example.com", "bob@example.com"]);
    }
    db.with_transaction(|tx| {
        tx.execute_batch("WITH RECURSIVE n(v) AS (VALUES(1) UNION ALL SELECT v+1 FROM n WHERE v<5000)
            INSERT INTO threads(id,provider_thread_id,account_id,subject,snippet,participants_json,last_message_at)
            SELECT 'noise-'||v,'noise-'||v,'you@example.com','Noise','','[]','2026-10-31T00:00:00Z' FROM n;
            INSERT INTO messages(id,thread_id,sender,recipients_json,sent_at,body_html,body_text)
            SELECT id,id,'you@example.com','[\"unrelated@example.com\"]',last_message_at,'','' FROM threads WHERE id LIKE 'noise-%';
            INSERT INTO co_recipient_addresses(message_id,account_id,email,sender,sent_at)
            SELECT id,'you@example.com','unrelated@example.com',sender,sent_at FROM messages WHERE id LIKE 'noise-%';")?;
        let mut statement = tx.prepare("EXPLAIN QUERY PLAN SELECT s.message_id,s.account_id,m.thread_id,s.sender,s.sent_at,r.email,r.display_name
            FROM co_recipient_addresses s JOIN co_recipient_addresses r ON r.message_id=s.message_id
            JOIN messages m ON m.id=s.message_id WHERE s.account_id=?1 AND s.email=?2 AND s.sent_at>=?3 LIMIT 20001")?;
        let plan = statement.query_map(["you@example.com", "alice@example.com", "2025"], |r| r.get::<_, String>(3))?.collect::<rusqlite::Result<Vec<_>>>()?.join("\n");
        assert!(plan.contains("SEARCH s USING INDEX co_recipient_seed"), "{plan}");
        assert!(!plan.contains("SCAN "), "{plan}");
        Ok(())
    }).unwrap();
    let start = std::time::Instant::now();
    assert!(
        relations(&db, Some("you@example.com"), &["alice@example.com"])
            .contains_key("bob@example.com")
    );
    eprintln!(
        "Co-recipient lookup with 5000 unrelated cached messages: {:?}",
        start.elapsed()
    );
}
