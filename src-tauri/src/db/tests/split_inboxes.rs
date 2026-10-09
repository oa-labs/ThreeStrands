use super::*;
use crate::db::test_support::{database, message};
use crate::models::{SplitInbox, Thread};

#[test]
fn split_inbox_matches_covers_domain_label_and_pattern_rules() {
    let thread = test_thread(&["Jane Doe <jane@Acme.com>"], &["IMPORTANT"]);

    assert!(split_inbox_matches(
        &test_rule("domain", "acme.com"),
        &thread
    ));
    assert!(!split_inbox_matches(
        &test_rule("domain", "other.com"),
        &thread
    ));

    assert!(split_inbox_matches(
        &test_rule("label", "IMPORTANT"),
        &thread
    ));
    assert!(!split_inbox_matches(
        &test_rule("label", "STARRED"),
        &thread
    ));

    assert!(split_inbox_matches(&test_rule("pattern", "jane@"), &thread));
    assert!(!split_inbox_matches(
        &test_rule("pattern", "john@"),
        &thread
    ));
}

#[test]
fn create_update_delete_and_reorder_split_inboxes() {
    let database = database();
    let acme = database
        .create_split_inbox("Acme", "domain", "Acme.com", "default")
        .unwrap();
    assert_eq!(acme.match_value, "acme.com", "domain values are lowercased");
    assert_eq!(acme.account_id, "default");
    let widgets = database
        .create_split_inbox("Widgets Co", "pattern", "widgets", "default")
        .unwrap();

    let listed = database.list_split_inboxes().unwrap();
    assert_eq!(
        listed.iter().map(|s| &s.name).collect::<Vec<_>>(),
        vec!["Acme", "Widgets Co"]
    );

    let renamed = database.update_split_inbox(&acme.id, "Acme Corp").unwrap();
    assert_eq!(renamed.name, "Acme Corp");
    assert_eq!(
        renamed.match_value, "acme.com",
        "rename leaves the rule untouched"
    );

    database
        .reorder_split_inboxes(&[widgets.id.clone(), acme.id.clone()])
        .unwrap();
    let reordered = database.list_split_inboxes().unwrap();
    assert_eq!(reordered[0].id, widgets.id);
    assert_eq!(reordered[1].id, acme.id);

    database.delete_split_inbox(&widgets.id).unwrap();
    assert_eq!(database.list_split_inboxes().unwrap().len(), 1);

    assert!(database
        .create_split_inbox("", "domain", "acme.com", "default")
        .is_err());
    assert!(database
        .create_split_inbox("Acme", "domain", "", "default")
        .is_err());
    assert!(database
        .create_split_inbox("Acme", "bogus", "acme.com", "default")
        .is_err());
}

#[test]
fn list_split_inbox_page_filters_the_inbox_and_paginates() {
    let database = database();
    let split_inbox = database
        .create_split_inbox("Inbox label", "label", "INBOX", "default")
        .unwrap();

    let first_page = database
        .list_split_inbox_page(&split_inbox.id, 0, 1)
        .unwrap();
    assert_eq!(first_page.threads.len(), 1);
    assert!(first_page.has_more);

    let second_page = database
        .list_split_inbox_page(&split_inbox.id, 1, 1)
        .unwrap();
    assert_eq!(second_page.threads.len(), 1);
    assert!(!second_page.has_more);
    assert_ne!(first_page.threads[0].id, second_page.threads[0].id);

    assert!(database.list_split_inbox_page("missing", 0, 10).is_err());
}

#[test]
fn list_split_inbox_page_never_pulls_in_another_accounts_threads() {
    let database = database();
    let mut other_message = message(
        "other-message",
        "other-thread",
        "2026-01-02T00:00:00Z",
        "body",
    );
    other_message.labels = vec!["INBOX".into()];
    database
        .upsert_thread("other@example.com", &[other_message])
        .unwrap();

    // Both accounts have an "INBOX"-labeled thread, but the rule only
    // belongs to "default" — the other account's matching thread must
    // not leak into its page.
    let split_inbox = database
        .create_split_inbox("Inbox label", "label", "INBOX", "default")
        .unwrap();
    let page = database
        .list_split_inbox_page(&split_inbox.id, 0, 10)
        .unwrap();
    assert!(page
        .threads
        .iter()
        .all(|thread| thread.account_id == "default"));
}

fn test_thread(participants: &[&str], labels: &[&str]) -> Thread {
    Thread {
        id: "t1".into(),
        provider_thread_id: "p1".into(),
        subject: "Hi".into(),
        snippet: "".into(),
        participants: participants.iter().map(|value| value.to_string()).collect(),
        last_message_at: "".into(),
        last_received_at: "".into(),
        unread: false,
        starred: false,
        archived: false,
        trashed: false,
        labels: labels.iter().map(|value| value.to_string()).collect(),
        account_id: "default".into(),
        match_snippet: None,
        summary: None,
        summary_generated_at: None,
        summary_revision: None,
        has_attachments: false,
    }
}

fn test_rule(match_kind: &str, match_value: &str) -> SplitInbox {
    SplitInbox {
        id: "s1".into(),
        name: "Test".into(),
        match_kind: match_kind.into(),
        match_value: match_value.into(),
        sort_order: 0,
        created_at: "".into(),
        account_id: "default".into(),
    }
}
