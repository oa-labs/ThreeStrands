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
        .query_row("SELECT archived, unread FROM threads WHERE id = ?1", [local(thread)], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
}

/// Queued mutations for a conversation, oldest first.
fn queued(database: &Database, thread: &str) -> Vec<serde_json::Value> {
    let connection = database.connection().unwrap();
    let mut statement = connection.prepare("SELECT payload_json FROM mutations WHERE thread_id = ?1 ORDER BY created_at, rowid").unwrap();
    statement
        .query_map([local(thread)], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|payload| serde_json::from_str(&payload.unwrap()).unwrap())
        .collect()
}

#[test]
fn new_unread_mail_from_a_muted_address_is_marked_read_and_archived_through_the_queue() {
    let database = setup();
    database.mute_sender(ACCOUNT, "address", " News@Launch.Example ", &[]).unwrap();
    database.upsert_thread(ACCOUNT, &[mail("n1", "news", NEW, "Launch <news@launch.example>", &["INBOX", "UNREAD"])]).unwrap();

    assert_eq!(state(&database, "news"), (true, false));
    let expected = [
        ThreadMutation::Read { thread_id: local("news"), value: true },
        ThreadMutation::Archive { thread_id: local("news"), value: true },
    ].map(|mutation| serde_json::to_value(mutation).unwrap());
    assert_eq!(queued(&database, "news"), expected);
}

#[test]
fn a_domain_rule_takes_every_address_at_that_exact_domain() {
    let database = setup();
    database.mute_sender(ACCOUNT, "domain", "@Launch.example", &[]).unwrap();
    database.upsert_thread(ACCOUNT, &[mail("d1", "billing", NEW, "billing@launch.example", &["INBOX", "UNREAD"])]).unwrap();
    database.upsert_thread(ACCOUNT, &[mail("d2", "sub", NEW, "news@mail.launch.example", &["INBOX", "UNREAD"])]).unwrap();
    database.upsert_thread(ACCOUNT, &[mail("d3", "alike", NEW, "news@notlaunch.example", &["INBOX", "UNREAD"])]).unwrap();
    assert_eq!(state(&database, "billing"), (true, false));
    assert_eq!(state(&database, "sub"), (false, true));
    assert_eq!(state(&database, "alike"), (false, true));
}

#[test]
fn mail_the_rule_should_not_touch_stays_where_it_is() {
    let database = setup();
    database.mute_sender(ACCOUNT, "address", "news@launch.example", &[]).unwrap();
    let cases = [
        // Arrived before the mute.
        ("old", vec![mail("o1", "old", OLD, "news@launch.example", &["INBOX", "UNREAD"])], (false, true)),
        // Already read, e.g. on another device or after the user moved it back.
        ("read", vec![mail("r1", "read", NEW, "news@launch.example", &["INBOX"])], (false, false)),
        // Not in the Inbox.
        ("elsewhere", vec![mail("e1", "elsewhere", NEW, "news@launch.example", &["UNREAD"])], (true, true)),
        // A different sender, even one whose address contains the muted one.
        ("other", vec![mail("x1", "other", NEW, "breaking-news@launch.example", &["INBOX", "UNREAD"])], (false, true)),
        // The user's own reply is the newest message.
        ("replied", vec![
            mail("p1", "replied", "2099-01-01T08:00:00Z", "news@launch.example", &["INBOX"]),
            mail("p2", "replied", NEW, ACCOUNT, &["INBOX", "SENT", "UNREAD"]),
        ], (false, true)),
    ];
    for (thread, messages, expected) in cases {
        database.upsert_thread(ACCOUNT, &messages).unwrap();
        assert_eq!(state(&database, thread), expected, "{thread}");
        assert!(queued(&database, thread).is_empty(), "{thread}");
    }
    // The same sender in another account is not muted there.
    database.adopt_account("work@example.org").unwrap();
    database.upsert_thread("work@example.org", &[mail("w1", "work", NEW, "news@launch.example", &["INBOX", "UNREAD"])]).unwrap();
    let archived: bool = database.connection().unwrap()
        .query_row("SELECT archived FROM threads WHERE id = 'work@example.org:work'", [], |row| row.get(0)).unwrap();
    assert!(!archived);
}

#[test]
fn a_conversation_moved_back_stays_until_newer_mail_from_the_sender_arrives() {
    let database = setup();
    database.mute_sender(ACCOUNT, "address", "news@launch.example", &[]).unwrap();
    let first = mail("m1", "news", NEW, "news@launch.example", &["INBOX", "UNREAD"]);
    database.upsert_thread(ACCOUNT, std::slice::from_ref(&first)).unwrap();
    assert_eq!(state(&database, "news"), (true, false));

    // The user moves it back and marks it unread; the provider then reports
    // it unread in the Inbox (Gmail refetches on any label change).
    database.mutate_threads(&[
        ThreadMutation::Archive { thread_id: local("news"), value: false },
        ThreadMutation::Read { thread_id: local("news"), value: false },
    ]).unwrap();
    database.upsert_thread(ACCOUNT, std::slice::from_ref(&first)).unwrap();
    assert_eq!(state(&database, "news"), (false, true), "the rule acts on a message once");

    // A newer message from the sender is new mail, so the rule acts again.
    database.upsert_thread(ACCOUNT, &[first, mail("m2", "news", "2099-01-02T09:00:00Z", "news@launch.example", &["INBOX", "UNREAD"])]).unwrap();
    assert_eq!(state(&database, "news"), (true, false));
}

#[test]
fn muting_can_archive_what_is_already_in_the_inbox_and_reports_it_for_undo() {
    let database = setup();
    database.upsert_thread(ACCOUNT, &[mail("a1", "unread", OLD, "news@launch.example", &["INBOX", "UNREAD"])]).unwrap();
    database.upsert_thread(ACCOUNT, &[mail("a2", "read", OLD, "news@launch.example", &["INBOX"])]).unwrap();
    database.upsert_thread(ACCOUNT, &[mail("a3", "done", OLD, "news@launch.example", &[])]).unwrap();
    let ids = [local("unread"), local("read"), local("done"), local("unread"), "work@example.org:missing".to_string()];

    let result = database.mute_sender(ACCOUNT, "address", "news@launch.example", &ids).unwrap();
    assert_eq!(result.archived_thread_ids, vec![local("unread"), local("read")]);
    assert_eq!(result.unread_thread_ids, vec![local("unread")]);
    assert_eq!(state(&database, "unread"), (true, false));
    assert_eq!(state(&database, "read"), (true, false));
    assert!(queued(&database, "done").is_empty());

    // Muting the same sender again reuses the rule.
    let again = database.mute_sender(ACCOUNT, "address", "NEWS@launch.example", &[]).unwrap();
    assert_eq!(again.rule.id, result.rule.id);
    assert_eq!(database.list_sender_rules().unwrap().len(), 1);
}

#[test]
fn muting_refuses_the_users_own_address_bad_matches_and_unknown_accounts() {
    let database = setup();
    for (kind, value) in [("address", ACCOUNT), ("address", "no-at-sign"), ("address", "a@b"), ("address", "x <a@b.example>"), ("domain", "nodot"), ("domain", "a@b.example"), ("domain", ".example.com"), ("pattern", "news")] {
        assert!(database.mute_sender(ACCOUNT, kind, value, &[]).is_err(), "{kind} {value}");
    }
    assert!(database.mute_sender("stranger@example.net", "address", "news@launch.example", &[]).is_err());
    assert!(database.list_sender_rules().unwrap().is_empty());
}

#[test]
fn unmuting_lets_new_mail_through_and_removing_the_account_removes_its_rules() {
    let database = setup();
    let rule = database.mute_sender(ACCOUNT, "address", "news@launch.example", &[]).unwrap().rule;
    database.upsert_thread(ACCOUNT, &[mail("u1", "first", NEW, "news@launch.example", &["INBOX", "UNREAD"])]).unwrap();
    database.delete_sender_rule(&rule.id).unwrap();
    let actions: i64 = database.connection().unwrap().query_row("SELECT COUNT(*) FROM sender_rule_actions", [], |row| row.get(0)).unwrap();
    assert_eq!(actions, 0, "a rule's log goes with it");
    database.upsert_thread(ACCOUNT, &[mail("u2", "second", NEW, "news@launch.example", &["INBOX", "UNREAD"])]).unwrap();
    assert_eq!(state(&database, "second"), (false, true));

    database.mute_sender(ACCOUNT, "domain", "launch.example", &[]).unwrap();
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
    };
    database.upsert_synced_sender_rule(&rule).unwrap();
    assert_eq!(database.list_sender_rules().unwrap()[0].match_value, "launch.example");
    for bad in [
        SenderRule { match_kind: "pattern".into(), ..rule.clone() },
        SenderRule { action: "delete".into(), ..rule.clone() },
        SenderRule { created_at: "yesterday".into(), ..rule.clone() },
        SenderRule { match_value: "not a domain".into(), ..rule.clone() },
    ] {
        assert!(database.upsert_synced_sender_rule(&bad).is_err(), "{bad:?}");
    }
}
