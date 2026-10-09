use crate::db::test_support::{database, message};
use crate::mime::UnsubscribeMetadata;

#[test]
fn contact_suggestions_rank_sent_recipients_above_mere_senders_and_filter_by_prefix() {
    let database = database();
    let mut sent = message(
        "sent-message",
        "sent-thread",
        "2026-01-01T00:00:00Z",
        "body",
    );
    sent.from = "you@example.com".into();
    sent.to = vec!["Jane Doe <jane@example.com>".into()];
    database.upsert_thread("you@example.com", &[sent]).unwrap();

    let mut received = message(
        "received-message",
        "received-thread",
        "2026-01-02T00:00:00Z",
        "body",
    );
    received.from = "Newsletter <newsletter@example.com>".into();
    received.to = vec!["you@example.com".into()];
    database
        .upsert_thread("you@example.com", &[received])
        .unwrap();

    let suggestions = database
        .list_contact_suggestions("you@example.com", "", 10)
        .unwrap();
    assert_eq!(suggestions.len(), 2);
    assert_eq!(suggestions[0].email, "jane@example.com");
    assert_eq!(suggestions[0].display_name.as_deref(), Some("Jane Doe"));
    assert_eq!(suggestions[0].sent_count, 1);
    assert_eq!(suggestions[1].email, "newsletter@example.com");
    assert_eq!(suggestions[1].received_count, 1);

    let filtered = database
        .list_contact_suggestions("you@example.com", "jan", 10)
        .unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].email, "jane@example.com");

    let filtered_out = database
        .list_contact_suggestions("you@example.com", "zzz", 10)
        .unwrap();
    assert!(filtered_out.is_empty());
}

#[test]
fn never_suggest_hides_mail_derived_addresses_without_hiding_the_address_book_history() {
    let database = database();
    database.adopt_account("you@example.com").unwrap();
    let mut sent = message(
        "sent-hidden",
        "thread-hidden",
        "2026-01-01T00:00:00Z",
        "body",
    );
    sent.from = "you@example.com".into();
    sent.to = vec!["Person <person@example.com>".into()];
    database.upsert_thread("you@example.com", &[sent]).unwrap();
    database
        .set_contact_suppressed("PERSON@EXAMPLE.COM", true)
        .unwrap();
    assert!(database
        .list_contact_suggestions("you@example.com", "person", 10)
        .unwrap()
        .is_empty());
    let profiles = database.list_contact_profiles("person", 10).unwrap();
    assert_eq!(profiles.len(), 1);
    assert!(profiles[0].id.starts_with("derived:"));
    assert_eq!(
        database
            .contact_timeline_for_account(&profiles[0].id, 0, 20, None)
            .unwrap()
            .len(),
        1
    );
    database
        .set_contact_suppressed("person@example.com", false)
        .unwrap();
    assert_eq!(
        database
            .list_contact_suggestions("you@example.com", "person", 10)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn contact_suggestions_match_email_prefixes_display_names_and_domains() {
    let database = database();
    let mut sent = message(
        "sent-to-kristen",
        "sent-to-kristen-thread",
        "2026-01-01T00:00:00Z",
        "body",
    );
    sent.from = "you@example.com".into();
    sent.to = vec!["Kristen Hammett <khammett@carsonwealth.com>".into()];
    database.upsert_thread("you@example.com", &[sent]).unwrap();

    for query in ["kham", "Kristen", "hammett", "CARS", "wealth"] {
        let matches = database
            .list_contact_suggestions("you@example.com", query, 10)
            .unwrap();
        assert_eq!(matches.len(), 1, "query {query:?} should match");
        assert_eq!(matches[0].email, "khammett@carsonwealth.com");
    }
}

#[test]
fn compose_contact_name_uses_the_most_recent_nonempty_message_name() {
    let database = database();
    for (id, date, name) in [
        ("name-old", "2026-01-01T00:00:00Z", "Zoe Earlier"),
        ("name-new", "2026-02-01T00:00:00Z", "Amy Later"),
    ] {
        let mut sent = message(id, &format!("{id}-thread"), date, "body");
        sent.from = "you@example.com".into();
        sent.to = vec![format!("{name} <person@example.com>")];
        database.upsert_thread("you@example.com", &[sent]).unwrap();
    }
    let suggestions = database
        .list_contact_suggestions("you@example.com", "person", 10)
        .unwrap();
    assert_eq!(suggestions[0].display_name.as_deref(), Some("Amy Later"));
}

#[test]
fn contact_suggestions_exclude_automated_senders_unless_sent_to_or_pinned() {
    let database = database();
    let mut newsletter = message(
        "newsletter-message",
        "newsletter-thread",
        "2026-01-01T00:00:00Z",
        "body",
    );
    newsletter.from = "Newsletter <newsletter@example.com>".into();
    newsletter.to = vec!["you@example.com".into()];
    newsletter.unsubscribe = Some(UnsubscribeMetadata {
        one_click_url: Some("https://example.com/unsubscribe".into()),
        mailto_url: None,
        web_url: None,
        list_id: None,
    });
    database
        .upsert_thread("you@example.com", &[newsletter])
        .unwrap();

    assert!(database
        .list_contact_suggestions("you@example.com", "", 10)
        .unwrap()
        .is_empty());

    // Mailing that same address directly still earns it a suggestion —
    // the exclusion only blocks the "heard from" side.
    let mut sent = message(
        "sent-message",
        "sent-thread",
        "2026-01-02T00:00:00Z",
        "body",
    );
    sent.from = "you@example.com".into();
    sent.to = vec!["newsletter@example.com".into()];
    database.upsert_thread("you@example.com", &[sent]).unwrap();

    let suggestions = database
        .list_contact_suggestions("you@example.com", "", 10)
        .unwrap();
    assert_eq!(suggestions.len(), 1);
    assert_eq!(suggestions[0].email, "newsletter@example.com");
    assert_eq!(suggestions[0].sent_count, 1);
    assert_eq!(suggestions[0].received_count, 0);
}
