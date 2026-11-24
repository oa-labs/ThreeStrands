use crate::db::test_support::{database, message};
use crate::db::Database;
use crate::mime::NormalizedMessage;
use crate::models::{SenderRule, ThreadMutation};

const ACCOUNT: &str = "you@example.com";
/// Later than any rule a test creates.
const NEW: &str = "2099-01-01T09:00:00Z";
/// Earlier than any rule a test creates.
const OLD: &str = "2020-01-01T09:00:00Z";

fn local(thread: &str) -> String {
    format!("{ACCOUNT}:{thread}")
}

fn mail(id: &str, thread: &str, date: &str, sender: &str, labels: &[&str]) -> NormalizedMessage {
    let mut item = message(id, thread, date, "body");
    item.from = sender.into();
    item.to = vec![ACCOUNT.into()];
    item.labels = labels.iter().map(|label| (*label).into()).collect();
    item
}

fn setup() -> Database {
    let database = database();
    database.adopt_account(ACCOUNT).unwrap();
    database
}

/// (archived, unread) for a conversation.
fn state(database: &Database, thread: &str) -> (bool, bool) {
    database
        .connection()
        .unwrap()
        .query_row(
            "SELECT archived, unread FROM threads WHERE id = ?1",
            [local(thread)],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
}

/// Queued mutations for a conversation, oldest first.
fn queued(database: &Database, thread: &str) -> Vec<serde_json::Value> {
    let connection = database.connection().unwrap();
    let mut statement = connection
        .prepare(
            "SELECT payload_json FROM mutations WHERE thread_id = ?1 ORDER BY created_at, rowid",
        )
        .unwrap();
    statement
        .query_map([local(thread)], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|payload| serde_json::from_str(&payload.unwrap()).unwrap())
        .collect()
}

#[test]
fn new_unread_mail_from_a_muted_address_is_marked_read_and_archived_through_the_queue() {
    let database = setup();
    database
        .mute_sender(ACCOUNT, "address", " News@Launch.Example ", &[])
        .unwrap();
    database
        .upsert_thread(
            ACCOUNT,
            &[mail(
                "n1",
                "news",
                NEW,
                "Launch <news@launch.example>",
                &["INBOX", "UNREAD"],
            )],
        )
        .unwrap();

    assert_eq!(state(&database, "news"), (true, false));
    let expected = [
        ThreadMutation::Read {
            thread_id: local("news"),
            value: true,
        },
        ThreadMutation::Archive {
            thread_id: local("news"),
            value: true,
        },
    ]
    .map(|mutation| serde_json::to_value(mutation).unwrap());
    assert_eq!(queued(&database, "news"), expected);
}

#[test]
fn a_domain_rule_takes_every_address_at_that_exact_domain() {
    let database = setup();
    database
        .mute_sender(ACCOUNT, "domain", "@Launch.example", &[])
        .unwrap();
    database
        .upsert_thread(
            ACCOUNT,
            &[mail(
                "d1",
                "billing",
                NEW,
                "billing@launch.example",
                &["INBOX", "UNREAD"],
            )],
        )
        .unwrap();
    database
        .upsert_thread(
            ACCOUNT,
            &[mail(
                "d2",
                "sub",
                NEW,
                "news@mail.launch.example",
                &["INBOX", "UNREAD"],
            )],
        )
        .unwrap();
    database
        .upsert_thread(
            ACCOUNT,
            &[mail(
                "d3",
                "alike",
                NEW,
                "news@notlaunch.example",
                &["INBOX", "UNREAD"],
            )],
        )
        .unwrap();
    assert_eq!(state(&database, "billing"), (true, false));
    assert_eq!(state(&database, "sub"), (false, true));
    assert_eq!(state(&database, "alike"), (false, true));
}

#[test]
fn mail_the_rule_should_not_touch_stays_where_it_is() {
    let database = setup();
    database
        .mute_sender(ACCOUNT, "address", "news@launch.example", &[])
        .unwrap();
    let cases = [
        // Arrived before the mute.
        (
            "old",
            vec![mail(
                "o1",
                "old",
                OLD,
                "news@launch.example",
                &["INBOX", "UNREAD"],
            )],
            (false, true),
        ),
        // Already read, e.g. on another device or after the user moved it back.
        (
            "read",
            vec![mail("r1", "read", NEW, "news@launch.example", &["INBOX"])],
            (false, false),
        ),
        // Not in the Inbox.
        (
            "elsewhere",
            vec![mail(
                "e1",
                "elsewhere",
                NEW,
                "news@launch.example",
                &["UNREAD"],
            )],
            (true, true),
        ),
        // A different sender, even one whose address contains the muted one.
        (
            "other",
            vec![mail(
                "x1",
                "other",
                NEW,
                "breaking-news@launch.example",
                &["INBOX", "UNREAD"],
            )],
            (false, true),
        ),
        // The user's own reply is the newest message.
        (
            "replied",
            vec![
                mail(
                    "p1",
                    "replied",
                    "2099-01-01T08:00:00Z",
                    "news@launch.example",
                    &["INBOX"],
                ),
                mail("p2", "replied", NEW, ACCOUNT, &["INBOX", "SENT", "UNREAD"]),
            ],
            (false, true),
        ),
    ];
    for (thread, messages, expected) in cases {
        database.upsert_thread(ACCOUNT, &messages).unwrap();
        assert_eq!(state(&database, thread), expected, "{thread}");
        assert!(queued(&database, thread).is_empty(), "{thread}");
    }
    // The same sender in another account is not muted there.
    database.adopt_account("work@example.org").unwrap();
    database
        .upsert_thread(
            "work@example.org",
            &[mail(
                "w1",
                "work",
                NEW,
                "news@launch.example",
                &["INBOX", "UNREAD"],
            )],
        )
        .unwrap();
    let archived: bool = database
        .connection()
        .unwrap()
        .query_row(
            "SELECT archived FROM threads WHERE id = 'work@example.org:work'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!archived);
}

#[test]
fn a_conversation_moved_back_stays_until_newer_mail_from_the_sender_arrives() {
    let database = setup();
    database
        .mute_sender(ACCOUNT, "address", "news@launch.example", &[])
        .unwrap();
    let first = mail(
        "m1",
        "news",
        NEW,
        "news@launch.example",
        &["INBOX", "UNREAD"],
    );
    database
        .upsert_thread(ACCOUNT, std::slice::from_ref(&first))
        .unwrap();
    assert_eq!(state(&database, "news"), (true, false));

    // The user moves it back and marks it unread; the provider then reports
    // it unread in the Inbox (Gmail refetches on any label change).
    database
        .mutate_threads(&[
            ThreadMutation::Archive {
                thread_id: local("news"),
                value: false,
            },
            ThreadMutation::Read {
                thread_id: local("news"),
                value: false,
            },
        ])
        .unwrap();
    database
        .upsert_thread(ACCOUNT, std::slice::from_ref(&first))
        .unwrap();
    assert_eq!(
        state(&database, "news"),
        (false, true),
        "the rule acts on a message once"
    );

    // A newer message from the sender is new mail, so the rule acts again.
    database
        .upsert_thread(
            ACCOUNT,
            &[
                first,
                mail(
                    "m2",
                    "news",
                    "2099-01-02T09:00:00Z",
                    "news@launch.example",
                    &["INBOX", "UNREAD"],
                ),
            ],
        )
        .unwrap();
    assert_eq!(state(&database, "news"), (true, false));
}

#[test]
fn muting_can_archive_what_is_already_in_the_inbox_and_reports_it_for_undo() {
    let database = setup();
    database
        .upsert_thread(
            ACCOUNT,
            &[mail(
                "a1",
                "unread",
                OLD,
                "news@launch.example",
                &["INBOX", "UNREAD"],
            )],
        )
        .unwrap();
    database
        .upsert_thread(
            ACCOUNT,
            &[mail("a2", "read", OLD, "news@launch.example", &["INBOX"])],
        )
        .unwrap();
    database
        .upsert_thread(
            ACCOUNT,
            &[mail("a3", "done", OLD, "news@launch.example", &[])],
        )
        .unwrap();
    let ids = [
        local("unread"),
        local("read"),
        local("done"),
        local("unread"),
        "work@example.org:missing".to_string(),
    ];

    let result = database
        .mute_sender(ACCOUNT, "address", "news@launch.example", &ids)
        .unwrap();
    assert_eq!(
        result.archived_thread_ids,
        vec![local("unread"), local("read")]
    );
    assert_eq!(result.unread_thread_ids, vec![local("unread")]);
    assert_eq!(state(&database, "unread"), (true, false));
    assert_eq!(state(&database, "read"), (true, false));
    assert!(queued(&database, "done").is_empty());

    // Muting the same sender again reuses the rule.
    let again = database
        .mute_sender(ACCOUNT, "address", "NEWS@launch.example", &[])
        .unwrap();
    assert_eq!(again.rule.id, result.rule.id);
    assert_eq!(database.list_sender_rules().unwrap().len(), 1);
}

#[test]
fn muting_refuses_the_users_own_address_bad_matches_and_unknown_accounts() {
    let database = setup();
    for (kind, value) in [
        ("address", ACCOUNT),
        ("address", "no-at-sign"),
        ("address", "a@b"),
        ("address", "x <a@b.example>"),
        ("domain", "nodot"),
        ("domain", "a@b.example"),
        ("domain", ".example.com"),
        ("pattern", "news"),
    ] {
        assert!(
            database.mute_sender(ACCOUNT, kind, value, &[]).is_err(),
            "{kind} {value}"
        );
    }
    assert!(database
        .mute_sender(
            "stranger@example.net",
            "address",
            "news@launch.example",
            &[]
        )
        .is_err());
    assert!(database.list_sender_rules().unwrap().is_empty());
}

#[test]
fn unmuting_lets_new_mail_through_and_removing_the_account_removes_its_rules() {
    let database = setup();
    let rule = database
        .mute_sender(ACCOUNT, "address", "news@launch.example", &[])
        .unwrap()
        .rule;
    database
        .upsert_thread(
            ACCOUNT,
            &[mail(
                "u1",
                "first",
                NEW,
                "news@launch.example",
                &["INBOX", "UNREAD"],
            )],
        )
        .unwrap();
    database.delete_sender_rule(&rule.id).unwrap();
    let actions: i64 = database
        .connection()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM sender_rule_actions", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(actions, 0, "a rule's log goes with it");
    database
        .upsert_thread(
            ACCOUNT,
            &[mail(
                "u2",
                "second",
                NEW,
                "news@launch.example",
                &["INBOX", "UNREAD"],
            )],
        )
        .unwrap();
    assert_eq!(state(&database, "second"), (false, true));

    database
        .mute_sender(ACCOUNT, "domain", "launch.example", &[])
        .unwrap();
    database.remove_account(ACCOUNT).unwrap();
    assert!(database.list_sender_rules().unwrap().is_empty());
}

#[test]
fn a_rule_from_another_device_is_checked_before_it_is_stored() {
    let database = setup();
    let rule = SenderRule {
        id: "synced".into(),
        account_id: ACCOUNT.into(),
        match_kind: "domain".into(),
        match_value: "Launch.Example".into(),
        action: "mute".into(),
        created_at: "2026-10-14T00:00:00Z".into(),
        last_matched_at: None,
    };
    database.upsert_synced_sender_rule(&rule).unwrap();
    assert_eq!(
        database.list_sender_rules().unwrap()[0].match_value,
        "launch.example"
    );
    for bad in [
        SenderRule {
            match_kind: "pattern".into(),
            ..rule.clone()
        },
        SenderRule {
            action: "delete".into(),
            ..rule.clone()
        },
        SenderRule {
            created_at: "yesterday".into(),
            ..rule.clone()
        },
        SenderRule {
            match_value: "not a domain".into(),
            ..rule.clone()
        },
    ] {
        assert!(database.upsert_synced_sender_rule(&bad).is_err(), "{bad:?}");
    }
}

// Blocked senders.

use super::{BlockUndo, SenderBlockChanges, BLOCK_IDLE_DAYS, MAX_BLOCKED_SENDERS};
use chrono::{Duration, Utc};

fn spam(thread: &str, value: bool) -> ThreadMutation {
    ThreadMutation::Spam {
        thread_id: local(thread),
        value,
    }
}

/// Marks conversations spam the way the app does.
fn mark_spam(database: &Database, threads: &[&str]) -> SenderBlockChanges {
    let mutations = threads
        .iter()
        .map(|thread| spam(thread, true))
        .collect::<Vec<_>>();
    database.apply_user_mutations(&mutations, None).unwrap()
}

fn blocks(database: &Database) -> Vec<SenderRule> {
    database
        .list_sender_rules()
        .unwrap()
        .into_iter()
        .filter(|rule| rule.action == "block")
        .collect()
}

fn in_spam(database: &Database, thread: &str) -> bool {
    database
        .connection()
        .unwrap()
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM threads t, json_each(t.labels_json) l
             WHERE t.id = ?1 AND l.value = 'SPAM')",
            [local(thread)],
            |row| row.get(0),
        )
        .unwrap()
}

/// A conversation with one read Inbox message from `sender`, dated `OLD`.
fn received(database: &Database, thread: &str, sender: &str) {
    database
        .upsert_thread(
            ACCOUNT,
            &[mail(
                &format!("{thread}-1"),
                thread,
                OLD,
                sender,
                &["INBOX"],
            )],
        )
        .unwrap();
}

#[test]
fn marking_spam_blocks_the_latest_inbound_sender_never_the_user_or_a_domain() {
    let database = setup();
    database
        .upsert_thread(
            ACCOUNT,
            &[
                mail(
                    "p1",
                    "prize",
                    "2019-12-31T09:00:00Z",
                    "Prize <Prize@Lottery.example>",
                    &["INBOX"],
                ),
                mail("p2", "prize", OLD, ACCOUNT, &["INBOX", "SENT"]),
            ],
        )
        .unwrap();
    database
        .upsert_thread(
            ACCOUNT,
            &[mail("m1", "mine", OLD, ACCOUNT, &["INBOX", "SENT"])],
        )
        .unwrap();

    let changes = mark_spam(&database, &["prize", "mine"]);
    let rules = blocks(&database);
    assert_eq!(
        rules.len(),
        1,
        "a conversation only the user wrote blocks nobody"
    );
    assert_eq!(
        (
            rules[0].match_kind.as_str(),
            rules[0].match_value.as_str(),
            rules[0].account_id.as_str()
        ),
        ("address", "prize@lottery.example", ACCOUNT)
    );
    assert_eq!(changes.created_ids, vec![rules[0].id.clone()]);
    assert_eq!(changes.written, rules);
    assert!(in_spam(&database, "prize"));
}

#[test]
fn marking_spam_again_refreshes_the_block_instead_of_adding_one() {
    let database = setup();
    received(&database, "first", "prize@lottery.example");
    received(&database, "second", "PRIZE@lottery.example");
    let first = mark_spam(&database, &["first"]);
    database
        .connection()
        .unwrap()
        .execute(
            "UPDATE sender_rules SET created_at = '2026-01-01T00:00:00Z'",
            [],
        )
        .unwrap();

    let again = mark_spam(&database, &["second"]);
    let rules = blocks(&database);
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].id, first.created_ids[0]);
    assert!(
        rules[0].created_at.as_str() > "2026-01-01T00:00:00Z",
        "refreshed"
    );
    assert!(
        again.created_ids.is_empty(),
        "Undo leaves a block it didn't create"
    );
    assert_eq!(again.written, rules);
}

#[test]
fn undo_removes_only_the_blocks_mark_spam_created_and_not_spam_removes_the_senders_block() {
    let database = setup();
    received(&database, "known", "known@spam.example");
    received(&database, "earlier", "known@spam.example");
    received(&database, "new", "new@spam.example");
    let existing = mark_spam(&database, &["earlier"]).created_ids;

    let marked = mark_spam(&database, &["known", "new"]);
    assert_eq!(blocks(&database).len(), 2);
    let undo = database
        .apply_user_mutations(
            &[spam("known", false), spam("new", false)],
            Some(&marked.created_ids),
        )
        .unwrap();
    assert_eq!(undo.removed_ids, marked.created_ids);
    assert!(!in_spam(&database, "known") && !in_spam(&database, "new"));
    let left = blocks(&database);
    assert_eq!(
        left.iter().map(|rule| rule.id.clone()).collect::<Vec<_>>(),
        existing
    );

    // Not spam, outside Undo, unblocks the sender whatever made the block.
    let not_spam = database
        .apply_user_mutations(&[spam("earlier", false)], None)
        .unwrap();
    assert_eq!(not_spam.removed_ids, existing);
    assert_eq!(
        not_spam
            .unblocked
            .iter()
            .map(|rule| rule.match_value.as_str())
            .collect::<Vec<_>>(),
        ["known@spam.example"],
        "Not spam reports the addresses it unblocked"
    );
    assert!(undo.unblocked.is_empty(), "Undo of Mark Spam reports none");
    assert!(blocks(&database).is_empty());

    // Undo never removes a mute.
    let mute = database
        .mute_sender(ACCOUNT, "address", "news@launch.example", &[])
        .unwrap()
        .rule;
    database
        .apply_user_mutations(&[], Some(std::slice::from_ref(&mute.id)))
        .unwrap();
    assert_eq!(database.list_sender_rules().unwrap(), vec![mute]);
}

#[test]
fn undo_of_not_spam_restores_the_removed_block_exactly() {
    let database = setup();
    received(&database, "one", "prize@lottery.example");
    mark_spam(&database, &["one"]);
    database
        .connection()
        .unwrap()
        .execute(
            "UPDATE sender_rules SET created_at = '2026-03-01T00:00:00Z', last_matched_at = '2026-03-02T00:00:00Z'",
            [],
        )
        .unwrap();
    let before = blocks(&database);
    assert_eq!(before.len(), 1);

    let not_spam = database
        .apply_user_mutations(&[spam("one", false)], None)
        .unwrap();
    assert_eq!(not_spam.unblocked, before);
    assert!(blocks(&database).is_empty());
    assert!(!in_spam(&database, "one"));

    let undo = database
        .apply_user_mutations_with(
            &[spam("one", true)],
            BlockUndo::NotSpam(&not_spam.unblocked),
        )
        .unwrap();
    assert!(in_spam(&database, "one"));
    assert_eq!(blocks(&database), before, "same id, dates, and address");
    assert!(undo.created_ids.is_empty());
    assert_eq!(undo.written, before, "other devices get the block back");

    // Restoring twice, or over a newer block for the address, adds nothing.
    let again = database
        .apply_user_mutations_with(
            &[spam("one", true)],
            BlockUndo::NotSpam(&not_spam.unblocked),
        )
        .unwrap();
    assert!(again.written.is_empty());
    assert_eq!(blocks(&database), before);
}

#[test]
fn new_unread_mail_from_a_blocked_address_moves_to_spam_through_the_queue() {
    let database = setup();
    received(&database, "first", "prize@lottery.example");
    mark_spam(&database, &["first"]);
    database
        .upsert_thread(
            ACCOUNT,
            &[mail(
                "n1",
                "later",
                NEW,
                "Prize <prize@lottery.example>",
                &["INBOX", "UNREAD"],
            )],
        )
        .unwrap();
    assert!(in_spam(&database, "later"));
    assert_eq!(state(&database, "later"), (true, true), "left unread");
    assert_eq!(
        queued(&database, "later"),
        vec![serde_json::to_value(spam("later", true)).unwrap()]
    );

    // Mail dated before the block, and mail already in Spam, stays put.
    database
        .upsert_thread(
            ACCOUNT,
            &[mail(
                "o1",
                "older",
                OLD,
                "prize@lottery.example",
                &["INBOX", "UNREAD"],
            )],
        )
        .unwrap();
    assert!(!in_spam(&database, "older"));
    assert!(queued(&database, "older").is_empty());
    database
        .upsert_thread(
            ACCOUNT,
            &[mail(
                "s1",
                "already",
                NEW,
                "prize@lottery.example",
                &["SPAM", "UNREAD"],
            )],
        )
        .unwrap();
    assert!(queued(&database, "already").is_empty());
}

#[test]
fn a_block_outranks_a_mute_of_the_same_sender() {
    let database = setup();
    database
        .mute_sender(ACCOUNT, "domain", "lottery.example", &[])
        .unwrap();
    received(&database, "first", "prize@lottery.example");
    mark_spam(&database, &["first"]);
    database
        .upsert_thread(
            ACCOUNT,
            &[mail(
                "n1",
                "later",
                NEW,
                "prize@lottery.example",
                &["INBOX", "UNREAD"],
            )],
        )
        .unwrap();
    assert!(in_spam(&database, "later"));
    assert_eq!(
        queued(&database, "later"),
        vec![serde_json::to_value(spam("later", true)).unwrap()],
        "not marked read and archived as the mute would"
    );
    // Anyone else at the domain is still only muted.
    database
        .upsert_thread(
            ACCOUNT,
            &[mail(
                "n2",
                "other",
                NEW,
                "winner@lottery.example",
                &["INBOX", "UNREAD"],
            )],
        )
        .unwrap();
    assert!(!in_spam(&database, "other"));
    assert_eq!(state(&database, "other"), (true, false));
}

#[test]
fn a_matching_message_records_when_the_sender_last_wrote() {
    let database = setup();
    received(&database, "first", "prize@lottery.example");
    mark_spam(&database, &["first"]);
    let mute = database
        .mute_sender(ACCOUNT, "address", "news@launch.example", &[])
        .unwrap()
        .rule;
    let last = |database: &Database, value: &str| {
        database
            .list_sender_rules()
            .unwrap()
            .into_iter()
            .find(|rule| rule.match_value == value)
            .unwrap()
            .last_matched_at
    };
    assert_eq!(last(&database, "prize@lottery.example"), None);

    // Stored in UTC, read or unread, wherever the message is.
    database
        .upsert_thread(
            ACCOUNT,
            &[mail(
                "a",
                "a",
                "2099-03-01T09:00:00+02:00",
                "prize@lottery.example",
                &[],
            )],
        )
        .unwrap();
    assert_eq!(
        last(&database, "prize@lottery.example").as_deref(),
        Some("2099-03-01T07:00:00Z")
    );
    // An older message never moves it back.
    database
        .upsert_thread(
            ACCOUNT,
            &[mail(
                "b",
                "b",
                "2099-02-01T09:00:00Z",
                "prize@lottery.example",
                &["INBOX"],
            )],
        )
        .unwrap();
    assert_eq!(
        last(&database, "prize@lottery.example").as_deref(),
        Some("2099-03-01T07:00:00Z")
    );
    // Mail from before the rule doesn't count as the sender still writing.
    database
        .upsert_thread(
            ACCOUNT,
            &[mail("c", "c", OLD, "news@launch.example", &["INBOX"])],
        )
        .unwrap();
    assert_eq!(last(&database, "news@launch.example"), None);
    // Mutes record it too.
    database
        .upsert_thread(
            ACCOUNT,
            &[mail(
                "d",
                "d",
                NEW,
                "news@launch.example",
                &["INBOX", "UNREAD"],
            )],
        )
        .unwrap();
    assert_eq!(
        last(&database, &mute.match_value).as_deref(),
        Some("2099-01-01T09:00:00Z")
    );
}

/// Adds a block directly, its clock `idle` before `now`.
fn insert_block(
    database: &Database,
    id: &str,
    account: &str,
    idle: Duration,
    now: chrono::DateTime<Utc>,
) {
    let at = (now - idle).to_rfc3339();
    database
        .connection()
        .unwrap()
        .execute(
            "INSERT INTO sender_rules(id, account_id, match_kind, match_value, action, created_at, updated_at)
             VALUES (?1, ?2, 'address', ?1 || '@spam.example', 'block', ?3, ?3)",
            rusqlite::params![id, account, at],
        )
        .unwrap();
}

fn block_ids(database: &Database) -> Vec<String> {
    let mut ids = blocks(database)
        .into_iter()
        .map(|rule| rule.id)
        .collect::<Vec<_>>();
    ids.sort();
    ids
}

#[test]
fn a_block_idle_past_the_limit_is_pruned_and_one_whose_sender_still_writes_stays() {
    let database = setup();
    let now = Utc::now();
    insert_block(
        &database,
        "below",
        ACCOUNT,
        Duration::days(BLOCK_IDLE_DAYS - 1),
        now,
    );
    insert_block(
        &database,
        "exact",
        ACCOUNT,
        Duration::days(BLOCK_IDLE_DAYS),
        now,
    );
    insert_block(
        &database,
        "above",
        ACCOUNT,
        Duration::days(BLOCK_IDLE_DAYS + 1),
        now,
    );
    insert_block(
        &database,
        "writing",
        ACCOUNT,
        Duration::days(BLOCK_IDLE_DAYS + 30),
        now,
    );
    insert_block(
        &database,
        "arrived",
        ACCOUNT,
        Duration::days(BLOCK_IDLE_DAYS + 30),
        now,
    );
    let connection = database.connection().unwrap();
    connection
        .execute(
            "UPDATE sender_rules SET last_matched_at = ?1 WHERE id = 'writing'",
            [(now - Duration::days(10)).to_rfc3339()],
        )
        .unwrap();
    // Made long ago elsewhere, but only just received here.
    connection
        .execute(
            "UPDATE sender_rules SET updated_at = ?1 WHERE id = 'arrived'",
            [now.to_rfc3339()],
        )
        .unwrap();
    drop(connection);
    let mute = database
        .mute_sender(ACCOUNT, "address", "news@launch.example", &[])
        .unwrap()
        .rule;
    database
        .connection()
        .unwrap()
        .execute(
            "UPDATE sender_rules SET created_at = '2001-01-01T00:00:00Z', updated_at = NULL WHERE id = ?1",
            [&mute.id],
        )
        .unwrap();

    assert_eq!(database.prune_blocked_senders_at(now).unwrap(), 1);
    assert_eq!(
        block_ids(&database),
        vec!["arrived", "below", "exact", "writing"]
    );
    assert!(
        database
            .list_sender_rules()
            .unwrap()
            .iter()
            .any(|rule| rule.id == mute.id),
        "mutes are never pruned"
    );
}

#[test]
fn an_account_keeps_at_most_the_limit_of_blocks_evicting_the_idlest() {
    let database = setup();
    database.adopt_account("work@example.org").unwrap();
    let now = Utc::now();
    // One short of the limit, each an hour idler than the last.
    for index in 0..MAX_BLOCKED_SENDERS - 1 {
        insert_block(
            &database,
            &format!("b{index:04}"),
            ACCOUNT,
            Duration::hours(1 + index as i64),
            now,
        );
    }
    insert_block(
        &database,
        "work",
        "work@example.org",
        Duration::days(170),
        now,
    );

    // Reaching the limit evicts nothing.
    received(&database, "at", "at@spam.example");
    let at = mark_spam(&database, &["at"]);
    assert!(at.removed_ids.is_empty());
    assert_eq!(blocks(&database).len(), MAX_BLOCKED_SENDERS + 1);

    // One past it evicts the block idle longest, in that account only.
    received(&database, "over", "over@spam.example");
    let over = mark_spam(&database, &["over"]);
    let idlest = format!("b{:04}", MAX_BLOCKED_SENDERS - 2);
    assert_eq!(over.removed_ids, vec![idlest.clone()]);
    assert_eq!(over.created_ids.len(), 1, "the new block stays");
    let ids = block_ids(&database);
    assert_eq!(ids.len(), MAX_BLOCKED_SENDERS + 1);
    assert!(!ids.contains(&idlest));
    assert!(ids.contains(&"work".to_string()));
}

#[test]
fn a_block_from_another_device_must_name_an_address_and_refreshes_in_place() {
    let database = setup();
    let rule = SenderRule {
        id: "synced-block".into(),
        account_id: ACCOUNT.into(),
        match_kind: "address".into(),
        match_value: "Prize@Lottery.example".into(),
        action: "block".into(),
        created_at: "2026-10-14T00:00:00Z".into(),
        last_matched_at: None,
    };
    database.upsert_synced_sender_rule(&rule).unwrap();
    database
        .upsert_synced_sender_rule(&SenderRule {
            created_at: "2026-11-01T00:00:00Z".into(),
            ..rule.clone()
        })
        .unwrap();
    let stored = blocks(&database);
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].match_value, "prize@lottery.example");
    assert_eq!(stored[0].created_at, "2026-11-01T00:00:00Z");
    assert!(database
        .upsert_synced_sender_rule(&SenderRule {
            id: "domain-block".into(),
            match_kind: "domain".into(),
            match_value: "lottery.example".into(),
            ..rule
        })
        .is_err());
}

#[test]
fn the_synchronized_record_leaves_out_when_the_sender_last_wrote() {
    let rule = SenderRule {
        id: "block".into(),
        account_id: ACCOUNT.into(),
        match_kind: "address".into(),
        match_value: "prize@lottery.example".into(),
        action: "block".into(),
        created_at: "2026-10-14T00:00:00Z".into(),
        last_matched_at: Some("2026-10-20T00:00:00Z".into()),
    };
    let payload = rule.sync_payload();
    assert!(payload.get("lastMatchedAt").is_none());
    assert_eq!(payload["action"], "block");
    assert!(serde_json::to_value(&rule)
        .unwrap()
        .get("lastMatchedAt")
        .is_some());
}
