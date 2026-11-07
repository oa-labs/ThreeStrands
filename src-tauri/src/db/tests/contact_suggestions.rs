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
fn autocomplete_candidate_filter_preserves_counts_name_precedence_pins_and_suppression() {
    let database = database();
    for (index, name) in ["Historical Match", "Current Name", ""]
        .into_iter()
        .enumerate()
    {
        let mut sent = message(
            &format!("candidate-{index}"),
            &format!("candidate-thread-{index}"),
            &format!("2026-0{}-01T00:00:00Z", index + 1),
            "body",
        );
        sent.from = "you@example.com".into();
        sent.to = vec![if name.is_empty() {
            "person@partner.test".into()
        } else {
            format!("{name} <person@partner.test>")
        }];
        database.upsert_thread("you@example.com", &[sent]).unwrap();
    }
    database
        .connection()
        .unwrap()
        .execute_batch(
            "INSERT INTO pinned_contacts(account_id,email,display_name,pinned_at) VALUES
         ('you@example.com','person@partner.test','Pinned Override','2026-01-01'),
         ('you@example.com','pinned@example.test','Pinned Only','2026-01-01');
         INSERT INTO contacts(id,display_name,favorite,updated_at) VALUES
         ('candidate-profile','Profile Override',1,'2026-01-01'),
         ('candidate-new','Profile Only',1,'2026-01-01');
         INSERT INTO contact_addresses(contact_id,email) VALUES
         ('candidate-profile','person@partner.test'), ('candidate-new','profile@new.test');",
        )
        .unwrap();
    // A saved contact is found by the name it was saved under, too, while
    // its mail name stays the one shown.
    for query in [
        "CURRENT",
        "person",
        "partner",
        "Pinned Only",
        "Profile Only",
        "Profile Override",
    ] {
        let suggestions = database
            .list_contact_suggestions("you@example.com", query, 10)
            .unwrap();
        assert_eq!(suggestions.len(), 1, "query {query:?}");
        let suggestion = &suggestions[0];
        assert!(suggestion.pinned);
        if suggestion.email == "person@partner.test" {
            assert_eq!(
                suggestion.sent_count, 3,
                "counts must include non-matching history"
            );
            assert_eq!(suggestion.display_name.as_deref(), Some("Current Name"));
        }
    }
    for query in ["Historical", "Pinned Override", "unmatched"] {
        assert!(
            database
                .list_contact_suggestions("you@example.com", query, 10)
                .unwrap()
                .is_empty(),
            "query {query:?}"
        );
    }
    database
        .set_contact_suppressed("person@partner.test", true)
        .unwrap();
    assert!(database
        .list_contact_suggestions("you@example.com", "current", 10)
        .unwrap()
        .is_empty());
    assert_eq!(
        database
            .contact_suggestions_including_suppressed("you@example.com", "current", 10)
            .unwrap()[0]
            .sent_count,
        3
    );
    assert!(database
        .list_contact_suggestions("other@example.com", "profile override", 10)
        .unwrap()
        .is_empty());
    database
        .set_contact_suppressed("person@partner.test", false)
        .unwrap();
    // Personal address-book entries remain available on another account; its
    // history and pins must not contribute to that account's counts.
    let other = database
        .list_contact_suggestions("other@example.com", "profile override", 10)
        .unwrap();
    assert_eq!(other.len(), 1);
    assert_eq!(other[0].sent_count, 0);
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

fn sent_from(database: &crate::db::Database, account: &str, id: &str, recipients: &[&str]) {
    let mut sent = message(
        id,
        id,
        &(chrono::Utc::now() - chrono::Duration::days(1)).to_rfc3339(),
        "body",
    );
    sent.labels = vec!["SENT".into()];
    sent.from = account.into();
    sent.to = recipients
        .iter()
        .map(|recipient| recipient.to_string())
        .collect();
    database.upsert_thread(account, &[sent]).unwrap();
}

fn save_contact(database: &crate::db::Database, id: &str, name: &str, email: &str) {
    database.connection().unwrap().execute(
        "INSERT INTO contacts(id,display_name,favorite,updated_at) VALUES(?1,?2,0,'2026-01-01')", rusqlite::params![id, name],
    ).unwrap();
    database
        .connection()
        .unwrap()
        .execute(
            "INSERT INTO contact_addresses(contact_id,email) VALUES(?1,?2)",
            rusqlite::params![id, email],
        )
        .unwrap();
}

#[test]
fn saved_contacts_outrank_busier_correspondents_while_typing() {
    let database = database();
    for n in 0..10 {
        for occurrence in 0..3 {
            sent_from(
                &database,
                "you@example.com",
                &format!("sam{n}-{occurrence}"),
                &[&format!("Sam Busy{n} <sam{n}@example.com>")],
            );
        }
    }
    save_contact(
        &database,
        "never-emailed",
        "Sam Saved",
        "sam.saved@elsewhere.test",
    );
    // Saved under a different name than the one their mail uses.
    sent_from(
        &database,
        "you@example.com",
        "robert",
        &["Bob <rsmith@example.com>"],
    );
    save_contact(&database, "robert", "Robert Smith", "rsmith@example.com");

    let typed = database
        .list_contact_suggestions("you@example.com", "sam", 8)
        .unwrap();
    assert_eq!(typed.len(), 8);
    assert_eq!(typed[0].email, "sam.saved@elsewhere.test");
    assert_eq!(typed[0].sent_count, 0);

    let by_saved_name = database
        .list_contact_suggestions("you@example.com", "robert", 8)
        .unwrap();
    assert_eq!(
        by_saved_name
            .iter()
            .map(|item| item.email.as_str())
            .collect::<Vec<_>>(),
        ["rsmith@example.com"]
    );
    assert_eq!(by_saved_name[0].display_name.as_deref(), Some("Bob"));

    // With nothing typed, mail history decides the order.
    let untyped = database
        .list_contact_suggestions("you@example.com", "", 8)
        .unwrap();
    assert!(
        untyped.iter().all(|item| item.sent_count > 0),
        "{untyped:?}"
    );
    assert!(!untyped
        .iter()
        .any(|item| item.email == "sam.saved@elsewhere.test"));
}

#[test]
fn people_often_emailed_with_the_current_recipients_still_lead_saved_contacts() {
    let database = database();
    for id in ["pair1", "pair2"] {
        sent_from(
            &database,
            "you@example.com",
            id,
            &["alice@example.com", "Sam Partner <sam.partner@example.com>"],
        );
    }
    save_contact(&database, "saved", "Sam Saved", "sam.saved@elsewhere.test");
    let context = vec!["alice@example.com".to_string()];
    let ranked = database
        .contextual_contact_suggestions("you@example.com", "sam", 8, &context, &context)
        .unwrap();
    assert_eq!(
        ranked
            .iter()
            .map(|item| item.email.as_str())
            .collect::<Vec<_>>(),
        ["sam.partner@example.com", "sam.saved@elsewhere.test"]
    );
    assert_eq!(ranked[0].related_to, ["alice@example.com"]);
}

#[test]
fn invitee_suggestions_use_the_calendar_accounts_mail_and_never_suggest_invitees_or_the_organizer()
{
    let database = database();
    database.adopt_account("primary@example.com").unwrap();
    database.adopt_account("work@example.com").unwrap();
    for id in ["work1", "work2"] {
        sent_from(
            &database,
            "work@example.com",
            id,
            &["alice@example.com", "bob@example.com"],
        );
    }
    sent_from(
        &database,
        "primary@example.com",
        "home",
        &["Bob Home <bobby@home.test>"],
    );
    save_contact(&database, "saved", "Bob Saved", "bob.saved@elsewhere.test");
    let invitees = vec!["alice@example.com".to_string()];

    // The calendar account is a mail account: its history, with the invitees as context.
    let work = database
        .invitee_suggestions(Some("WORK@example.com"), "", 8, &invitees)
        .unwrap();
    assert_eq!(work[0].email, "bob@example.com");
    assert_eq!(work[0].related_to, ["alice@example.com"]);
    assert!(
        !work.iter().any(
            |item| ["alice@example.com", "work@example.com", "bobby@home.test"]
                .contains(&item.email.as_str())
        ),
        "{work:?}"
    );

    // A calendar-only account falls back to the primary mail account; saved contacts are always there.
    let calendar_only = database
        .invitee_suggestions(Some("calendar-only@example.com"), "bob", 8, &[])
        .unwrap();
    let emails = calendar_only
        .iter()
        .map(|item| item.email.as_str())
        .collect::<Vec<_>>();
    assert_eq!(emails, ["bob.saved@elsewhere.test", "bobby@home.test"]);

    // The organizer is never offered as an invitee.
    save_contact(&database, "me", "Organizer", "organizer@example.com");
    assert!(database
        .invitee_suggestions(Some("organizer@example.com"), "organizer", 8, &[])
        .unwrap()
        .is_empty());
}
