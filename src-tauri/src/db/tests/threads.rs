use crate::db::test_support::{clear_seed_threads, database, message};
use crate::models::ThreadMutation;
use chrono::Utc;

#[test]
fn mailbox_pages_report_remaining_rows() {
    let database = database();
    let first = database.list_threads_page(None, 0, 1).unwrap();
    let second = database.list_threads_page(None, 1, 1).unwrap();
    assert_eq!(first.threads.len(), 1);
    assert!(first.has_more);
    assert_eq!(second.threads.len(), 1);
    assert!(!second.has_more);
}

#[test]
fn unread_counts_include_only_unread_inbox_threads_and_group_by_account() {
    let database = database();
    let connection = database.connection().unwrap();
    connection
        .execute(
            "UPDATE threads SET account_id = 'work@example.com' WHERE id = 'roadmap'",
            [],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE threads SET unread = 1 WHERE id IN ('welcome', 'roadmap')",
            [],
        )
        .unwrap();
    drop(connection);

    let counts = database.list_unread_counts().unwrap();
    assert_eq!(counts.get("default"), Some(&1));
    assert_eq!(counts.get("work@example.com"), Some(&1));

    let connection = database.connection().unwrap();
    connection
        .execute("UPDATE threads SET archived = 1 WHERE id = 'roadmap'", [])
        .unwrap();
    drop(connection);
    let counts = database.list_unread_counts().unwrap();
    assert_eq!(counts.get("default"), Some(&1));
    assert!(!counts.contains_key("work@example.com"));
}

#[test]
fn deleting_a_thread_also_removes_its_search_index_row() {
    let database = database();
    let indexed: i64 = database
        .connection()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM thread_search WHERE thread_id = 'welcome'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(indexed, 1);
    database.delete_thread("default", "demo-welcome").unwrap();
    let remaining: i64 = database
        .connection()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM thread_search WHERE thread_id = 'welcome'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(remaining, 0);
}

#[test]
fn list_threads_page_excludes_threads_claimed_by_a_split_inbox() {
    let database = database();
    let before = database.list_threads_page(None, 0, 10).unwrap();
    assert_eq!(
        before.threads.len(),
        2,
        "welcome and roadmap are both seeded, unclaimed by any split"
    );

    // "roadmap"'s only participant is "Product Team" (see `insert_demo`),
    // which the `pattern` rule matches on the sender's normalized address.
    // Both seeded threads are account "default" (see `insert_demo`).
    database
        .create_split_inbox("Product", "pattern", "product", "default")
        .unwrap();

    let after = database.list_threads_page(None, 0, 10).unwrap();
    assert_eq!(
        after
            .threads
            .iter()
            .map(|t| t.id.as_str())
            .collect::<Vec<_>>(),
        vec!["welcome"]
    );
}

#[test]
fn list_threads_page_does_not_exclude_a_thread_for_another_accounts_split_rule() {
    let database = database();
    // "roadmap" (account "default") matches this rule's pattern, but the
    // rule belongs to a different account, so it must stay in the Inbox.
    database
        .create_split_inbox("Product", "pattern", "product", "other@example.com")
        .unwrap();

    let after = database.list_threads_page(None, 0, 10).unwrap();
    assert_eq!(after.threads.len(), 2);
}

#[test]
fn mailbox_unread_counts_buckets_unread_threads_by_split_and_excludes_them_from_inbox() {
    let database = database();
    let mut product_message = message(
        "product-message",
        "product-thread",
        "2026-01-02T00:00:00Z",
        "body",
    );
    product_message.from = "Team <team@product.example>".into();
    product_message.labels = vec!["INBOX".into(), "UNREAD".into()];
    database
        .upsert_thread("work@example.com", &[product_message])
        .unwrap();

    // "welcome" (seeded, unread) and the new product thread both count
    // toward the Inbox until a split inbox claims the latter.
    let before = database.mailbox_unread_counts(None).unwrap();
    assert_eq!(before.inbox, 2);
    assert!(before.splits.is_empty());

    let split = database
        .create_split_inbox("Product", "domain", "product.example", "work@example.com")
        .unwrap();
    let after = database.mailbox_unread_counts(None).unwrap();
    assert_eq!(
        after.inbox, 1,
        "the product thread moved out of the Inbox bucket"
    );
    assert_eq!(after.splits.get(&split.id), Some(&1));
}

#[test]
fn mailbox_unread_counts_ignores_a_split_rule_from_a_different_account() {
    let database = database();
    let mut product_message = message(
        "product-message",
        "product-thread",
        "2026-01-02T00:00:00Z",
        "body",
    );
    product_message.from = "Team <team@product.example>".into();
    product_message.labels = vec!["INBOX".into(), "UNREAD".into()];
    // Unread thread lives on "work@example.com"; the rule below belongs
    // to a different account and must not claim it.
    database
        .upsert_thread("work@example.com", &[product_message])
        .unwrap();
    database
        .create_split_inbox("Product", "domain", "product.example", "other@example.com")
        .unwrap();

    let counts = database.mailbox_unread_counts(None).unwrap();
    assert_eq!(
        counts.inbox, 2,
        "the product thread stays in the Inbox bucket"
    );
    assert!(counts.splits.is_empty());
}

#[test]
fn thread_summary_round_trips_through_get_thread() {
    let database = database();
    let before = database.get_thread("welcome").unwrap().thread;
    assert_eq!(before.summary, None);
    assert_eq!(before.summary_generated_at, None);

    assert!(database
        .set_thread_summary(
            "welcome",
            "- Point one\n- Point two",
            "2026-03-05T16:30:00Z",
            &before.last_message_at,
        )
        .unwrap());

    let after = database.get_thread("welcome").unwrap().thread;
    assert_eq!(after.summary.as_deref(), Some("- Point one\n- Point two"));
    assert_eq!(
        after.summary_generated_at.as_deref(),
        Some("2026-03-05T16:30:00Z")
    );
    assert_eq!(
        after.summary_revision.as_deref(),
        Some(before.last_message_at.as_str())
    );
}

#[test]
fn a_summary_for_an_older_revision_never_replaces_a_newer_one() {
    let database = database();
    let save = |summary: &str, revision: &str| {
        database
            .set_thread_summary("welcome", summary, "2026-03-05T16:30:00Z", revision)
            .unwrap()
    };
    assert!(save("newer", "2026-03-02T00:00:00Z"));
    // A slower request that read the thread before the newer message.
    assert!(!save("older", "2026-03-01T00:00:00Z"));
    // Regenerating for the same revision still replaces it.
    assert!(save("regenerated", "2026-03-02T00:00:00Z"));
    let thread = database.get_thread("welcome").unwrap().thread;
    assert_eq!(thread.summary.as_deref(), Some("regenerated"));
    assert_eq!(
        thread.summary_revision.as_deref(),
        Some("2026-03-02T00:00:00Z")
    );
}

#[test]
fn prune_expired_threads_is_a_noop_when_retention_is_unset() {
    let database = database();
    database
        .upsert_thread(
            "work@example.com",
            &[message("m1", "t1", "2000-01-01T00:00:00Z", "body")],
        )
        .unwrap();
    let removed = database.prune_expired_threads().unwrap();
    assert_eq!(removed, 0);
    assert!(database
        .list_threads(None)
        .unwrap()
        .iter()
        .any(|t| t.id == "work@example.com:t1"));
}

#[test]
fn prune_expired_threads_removes_only_old_unstarred_threads() {
    let database = database();
    clear_seed_threads(&database);
    database
        .upsert_thread(
            "work@example.com",
            &[message("m1", "t1", "2000-01-01T00:00:00Z", "body")],
        )
        .unwrap();
    database
        .upsert_thread(
            "work@example.com",
            &[message("m2", "t2", &Utc::now().to_rfc3339(), "body")],
        )
        .unwrap();
    database.set_retention_days(Some(30)).unwrap();
    let removed = database.prune_expired_threads().unwrap();
    assert_eq!(removed, 1);
    let threads = database.list_threads(None).unwrap();
    assert!(!threads.iter().any(|t| t.id == "work@example.com:t1"));
    assert!(threads.iter().any(|t| t.id == "work@example.com:t2"));
}

#[test]
fn prune_expired_threads_keeps_starred_threads_regardless_of_age() {
    let database = database();
    clear_seed_threads(&database);
    database
        .upsert_thread(
            "work@example.com",
            &[message("m1", "t1", "2000-01-01T00:00:00Z", "body")],
        )
        .unwrap();
    database
        .mutate_thread(&ThreadMutation::Star {
            thread_id: "work@example.com:t1".into(),
            value: true,
        })
        .unwrap();
    database.set_retention_days(Some(30)).unwrap();
    let removed = database.prune_expired_threads().unwrap();
    assert_eq!(removed, 0);
    assert!(database
        .list_threads(None)
        .unwrap()
        .iter()
        .any(|t| t.id == "work@example.com:t1"));
}

#[test]
fn pruning_a_thread_also_removes_its_search_index_row() {
    let database = database();
    database
        .upsert_thread(
            "work@example.com",
            &[message("m1", "t1", "2000-01-01T00:00:00Z", "body")],
        )
        .unwrap();
    let indexed: i64 = database
        .connection()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM thread_search WHERE thread_id = 'work@example.com:t1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(indexed, 1);
    database.set_retention_days(Some(30)).unwrap();
    database.prune_expired_threads().unwrap();
    let remaining: i64 = database
        .connection()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM thread_search WHERE thread_id = 'work@example.com:t1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(remaining, 0);
}

#[test]
fn list_threads_merges_by_default_and_filters_when_scoped() {
    let database = database();
    database
        .upsert_thread(
            "work@example.com",
            &[message("m1", "t1", "2026-01-01T00:00:00Z", "body")],
        )
        .unwrap();
    database
        .upsert_thread(
            "personal@example.com",
            &[message("m2", "t2", "2026-01-02T00:00:00Z", "body")],
        )
        .unwrap();

    let merged = database.list_threads(None).unwrap();
    assert!(merged.iter().any(|t| t.id == "work@example.com:t1"));
    assert!(merged.iter().any(|t| t.id == "personal@example.com:t2"));

    let scoped = database.list_threads(Some("work@example.com")).unwrap();
    assert_eq!(scoped.len(), 1);
    assert_eq!(scoped[0].id, "work@example.com:t1");
    assert_eq!(scoped[0].account_id, "work@example.com");
}

#[test]
fn list_all_mail_excludes_trash_but_keeps_archived() {
    let database = database();
    database
        .upsert_thread(
            "work@example.com",
            &[message("m1", "inbox", "2026-01-01T00:00:00Z", "body")],
        )
        .unwrap();
    database
        .upsert_thread(
            "work@example.com",
            &[message("m2", "archived", "2026-01-02T00:00:00Z", "body")],
        )
        .unwrap();
    database
        .upsert_thread(
            "work@example.com",
            &[message("m3", "trashed", "2026-01-03T00:00:00Z", "body")],
        )
        .unwrap();
    database
        .mutate_thread(&ThreadMutation::Archive {
            thread_id: "work@example.com:archived".into(),
            value: true,
        })
        .unwrap();
    database
        .mutate_thread(&ThreadMutation::Trash {
            thread_id: "work@example.com:trashed".into(),
            value: true,
        })
        .unwrap();

    let all_mail = database.list_all_mail(None).unwrap();
    let ids: Vec<_> = all_mail.iter().map(|t| t.id.as_str()).collect();
    assert!(ids.contains(&"work@example.com:inbox"));
    assert!(ids.contains(&"work@example.com:archived"));
    assert!(!ids.contains(&"work@example.com:trashed"));
}

#[test]
fn list_trash_only_returns_trashed_threads() {
    let database = database();
    database
        .upsert_thread(
            "work@example.com",
            &[message("m1", "inbox", "2026-01-01T00:00:00Z", "body")],
        )
        .unwrap();
    database
        .upsert_thread(
            "work@example.com",
            &[message("m2", "trashed", "2026-01-02T00:00:00Z", "body")],
        )
        .unwrap();
    database
        .mutate_thread(&ThreadMutation::Trash {
            thread_id: "work@example.com:trashed".into(),
            value: true,
        })
        .unwrap();

    let trash = database.list_trash(None).unwrap();
    assert_eq!(trash.len(), 1);
    assert_eq!(trash[0].id, "work@example.com:trashed");

    let scoped = database.list_trash(Some("personal@example.com")).unwrap();
    assert!(scoped.is_empty());
}
