use crate::db::contacts;
use crate::db::test_support::{database, message};
use crate::models::SaveContactRequest;
use chrono::Utc;
use rusqlite::params;

#[test]
fn pinned_contacts_migrate_to_profiles_and_unpin_removes_favorite_rank() {
    let database = database();
    let mut sent = message(
        "sent-message",
        "sent-thread",
        "2026-01-01T00:00:00Z",
        "body",
    );
    sent.from = "you@example.com".into();
    sent.to = vec!["Frequent <frequent@example.com>".into()];
    database.upsert_thread("you@example.com", &[sent]).unwrap();

    database
        .pin_contact(
            "you@example.com",
            "Pinned@Example.com",
            Some("Pinned Person"),
        )
        .unwrap();

    let suggestions = database
        .list_contact_suggestions("you@example.com", "", 10)
        .unwrap();
    assert_eq!(suggestions[0].email, "pinned@example.com");
    assert!(suggestions[0].pinned);
    assert_eq!(suggestions[0].sent_count, 0);
    assert_eq!(suggestions[1].email, "frequent@example.com");
    assert!(!suggestions[1].pinned);

    database
        .unpin_contact("you@example.com", "pinned@example.com")
        .unwrap();
    let after_unpin = database
        .list_contact_suggestions("you@example.com", "", 10)
        .unwrap();
    assert_eq!(after_unpin.len(), 2);
    assert_eq!(after_unpin[0].email, "frequent@example.com");
    assert_eq!(after_unpin[1].email, "pinned@example.com");
    assert!(!after_unpin[1].pinned);
}

#[test]
fn saved_contact_search_links_addresses_and_enforces_unique_ownership() {
    let database = database();
    let saved = database
        .save_contact_profile(&SaveContactRequest {
            birthday: None,
            keep_in_touch: None,
            id: None,
            display_name: Some("Jane Rivera".into()),
            role: Some("Founder".into()),
            company: Some("Acme Labs".into()),
            location: Some("Boston".into()),
            bio: None,
            notes: Some("Met at launch".into()),
            links: vec!["https://social.invalid/jane".into()],
            photo_data: None,
            favorite: true,
            addresses: vec!["jane@example.com".into(), "j.rivera@example.com".into()],
        })
        .unwrap();
    assert_eq!(
        database.list_contact_profiles("Acme", 20).unwrap()[0].id,
        saved.id
    );
    assert_eq!(
        database
            .list_contact_profiles("social.invalid", 20)
            .unwrap()[0]
            .id,
        saved.id
    );
    assert_eq!(saved.addresses.len(), 2);
    let duplicate = database.save_contact_profile(&SaveContactRequest {
        birthday: None,
        keep_in_touch: None,
        id: Some("another".into()),
        display_name: Some("Other".into()),
        role: None,
        company: None,
        location: None,
        bio: None,
        notes: None,
        links: vec![],
        photo_data: None,
        favorite: false,
        addresses: vec!["jane@example.com".into()],
    });
    assert!(duplicate.is_err());
}

#[test]
fn contact_ids_for_addresses_maps_every_linked_address_to_its_saved_owner() {
    let database = database();
    let saved = database
        .save_contact_profile(&SaveContactRequest {
            birthday: None,
            keep_in_touch: None,
            id: None,
            display_name: Some("Jane Rivera".into()),
            role: None,
            company: None,
            location: None,
            bio: None,
            notes: None,
            links: vec![],
            photo_data: None,
            favorite: false,
            addresses: vec![
                "jane@example.com".into(),
                "j.rivera@work.example.com".into(),
            ],
        })
        .unwrap();
    let owners = database
        .contact_ids_for_addresses(&[
            "Jane@Example.com".into(),
            "j.rivera@work.example.com".into(),
            "stranger@example.com".into(),
            "".into(),
        ])
        .unwrap();
    assert_eq!(owners.len(), 2);
    assert_eq!(owners.get("jane@example.com"), Some(&saved.id));
    assert_eq!(owners.get("j.rivera@work.example.com"), Some(&saved.id));
    assert!(!owners.contains_key("stranger@example.com"));
}

#[test]
fn saved_contact_enumeration_is_not_limited_by_derived_contact_results() {
    let database = database();
    database.with_transaction(|tx|{
        for index in 0..5_005 {
            let id=format!("saved-contact-{index}");
            tx.execute("INSERT INTO contacts(id,display_name,links_json,favorite,updated_at) VALUES(?1,?2,'[]',0,'2026-01-01T00:00:00Z')",params![id,format!("Saved {index}")])?;
            tx.execute("INSERT INTO contact_addresses(contact_id,email) VALUES(?1,?2)",params![id,format!("saved{index}@example.com")])?;
        }
        Ok(())
    }).unwrap();
    let saved = database.list_saved_contact_profiles().unwrap();
    assert_eq!(saved.len(), 5_005);
    assert!(saved
        .iter()
        .all(|contact| !contact.id.starts_with("derived:")));
}

#[test]
fn reusing_a_removed_address_cannot_overwrite_the_previous_profile_id() {
    let database = database();
    let original = database
        .save_contact_profile(&SaveContactRequest {
            birthday: None,
            keep_in_touch: None,
            id: None,
            display_name: Some("Original person".into()),
            role: None,
            company: None,
            location: None,
            bio: None,
            notes: Some("Keep this profile".into()),
            links: vec![],
            photo_data: None,
            favorite: false,
            addresses: vec!["x@x.example".into()],
        })
        .unwrap();
    let moved = database
        .save_contact_profile(&SaveContactRequest {
            birthday: None,
            keep_in_touch: None,
            id: Some(original.id.clone()),
            display_name: Some("Original person".into()),
            role: None,
            company: None,
            location: None,
            bio: None,
            notes: Some("Keep this profile".into()),
            links: vec![],
            photo_data: None,
            favorite: false,
            addresses: vec!["y@y.example".into()],
        })
        .unwrap();
    assert_eq!(moved.id, original.id);

    let reused = database
        .save_contact_profile(&SaveContactRequest {
            birthday: None,
            keep_in_touch: None,
            id: None,
            display_name: Some("New person".into()),
            role: None,
            company: None,
            location: None,
            bio: None,
            notes: None,
            links: vec![],
            photo_data: None,
            favorite: false,
            addresses: vec!["x@x.example".into()],
        })
        .unwrap();
    assert_ne!(reused.id, original.id);
    let preserved = database.get_contact_profile(&original.id).unwrap().unwrap();
    assert_eq!(preserved.display_name.as_deref(), Some("Original person"));
    assert_eq!(preserved.notes.as_deref(), Some("Keep this profile"));
    assert_eq!(preserved.addresses, vec!["y@y.example"]);

    let pin_email = "pinned@x.example";
    let pinned = database
        .save_contact_profile(&SaveContactRequest {
            birthday: None,
            keep_in_touch: None,
            id: None,
            display_name: Some("Moved from pin".into()),
            role: None,
            company: None,
            location: None,
            bio: None,
            notes: Some("Preserve me too".into()),
            links: vec![],
            photo_data: None,
            favorite: false,
            addresses: vec![pin_email.into()],
        })
        .unwrap();
    database
        .save_contact_profile(&SaveContactRequest {
            birthday: None,
            keep_in_touch: None,
            id: Some(pinned.id.clone()),
            display_name: Some("Moved from pin".into()),
            role: None,
            company: None,
            location: None,
            bio: None,
            notes: Some("Preserve me too".into()),
            links: vec![],
            photo_data: None,
            favorite: false,
            addresses: vec!["moved@x.example".into()],
        })
        .unwrap();
    database
        .pin_contact("you@example.com", pin_email, None)
        .unwrap();
    let preserved_pin = database.get_contact_profile(&pinned.id).unwrap().unwrap();
    assert_eq!(preserved_pin.notes.as_deref(), Some("Preserve me too"));
    assert_eq!(preserved_pin.addresses, vec!["moved@x.example"]);
    let newly_pinned = database.list_contact_profiles(pin_email, 10).unwrap();
    assert_eq!(newly_pinned.len(), 1);
    assert_ne!(newly_pinned[0].id, pinned.id);
    assert!(newly_pinned[0].favorite);
}

#[test]
fn keep_in_touch_saves_a_derived_contact_and_counts_mail_from_every_account() {
    let database = database();
    database.adopt_account("you@example.com").unwrap();
    database.adopt_account("other@example.com").unwrap();
    let mut sent = message("kit-one", "kit-thread-one", "2026-09-01T12:00:00Z", "hello");
    sent.from = "you@example.com".into();
    sent.to = vec!["Jane <jane@example.com>".into()];
    database.upsert_thread("you@example.com", &[sent]).unwrap();
    let mut received = message("kit-two", "kit-thread-two", "2026-09-10T12:00:00Z", "reply");
    received.from = "Jane <jane@example.com>".into();
    received.to = vec!["other@example.com".into()];
    database
        .upsert_thread("other@example.com", &[received])
        .unwrap();

    let profiles = database
        .set_keep_in_touch(&["derived:jane@example.com".into()], Some(14))
        .unwrap();
    assert_eq!(profiles.len(), 1);
    let jane = &profiles[0];
    assert!(!jane.id.starts_with("derived:"));
    assert_eq!(jane.keep_in_touch.interval_days, Some(14));
    assert!(jane.keep_in_touch.started_at.is_some());
    // Received mail counts as a touch, from whichever account it reached.
    assert_eq!(
        jane.keep_in_touch_due_at.as_deref(),
        Some("2026-09-24T12:00:00+00:00")
    );
    // A view scoped to the account without the latest mail still reports
    // the same due date.
    let scoped = database
        .list_contact_profiles_for_account("jane", 20, Some("you@example.com"))
        .unwrap();
    assert_eq!(scoped[0].keep_in_touch_due_at, jane.keep_in_touch_due_at);
    assert_eq!(
        database
            .list_keep_in_touch()
            .unwrap()
            .iter()
            .map(|item| item.id.clone())
            .collect::<Vec<_>>(),
        vec![jane.id.clone()]
    );
}

#[test]
fn keep_in_touch_interval_changes_keep_the_start_and_turning_off_clears_the_snooze() {
    let database = database();
    let saved = database
        .save_contact_profile(&contact_request("Sam", "sam@example.com"))
        .unwrap();
    assert!(database
        .set_keep_in_touch(std::slice::from_ref(&saved.id), Some(0))
        .is_err());
    assert!(database
        .set_keep_in_touch(
            std::slice::from_ref(&saved.id),
            Some(contacts::MAX_KEEP_IN_TOUCH_DAYS + 1)
        )
        .is_err());
    assert!(database.set_keep_in_touch(&[], Some(7)).is_err());
    let on = database
        .set_keep_in_touch(std::slice::from_ref(&saved.id), Some(7))
        .unwrap()
        .remove(0);
    let started = on.keep_in_touch.started_at.clone().unwrap();
    let changed = database
        .set_keep_in_touch(
            std::slice::from_ref(&saved.id),
            Some(contacts::MAX_KEEP_IN_TOUCH_DAYS),
        )
        .unwrap()
        .remove(0);
    assert_eq!(
        changed.keep_in_touch.interval_days,
        Some(contacts::MAX_KEEP_IN_TOUCH_DAYS)
    );
    assert_eq!(
        changed.keep_in_touch.started_at.as_deref(),
        Some(started.as_str())
    );

    let until = (Utc::now() + chrono::Duration::days(10)).to_rfc3339();
    let snoozed = database
        .snooze_keep_in_touch(&saved.id, Some(&until))
        .unwrap();
    assert!(snoozed.keep_in_touch.snoozed_until.is_some());
    assert_eq!(
        snoozed.keep_in_touch_due_at,
        snoozed.keep_in_touch.snoozed_until
    );

    let off = database
        .set_keep_in_touch(std::slice::from_ref(&saved.id), None)
        .unwrap()
        .remove(0);
    assert_eq!(off.keep_in_touch.interval_days, None);
    assert_eq!(off.keep_in_touch.started_at, None);
    assert_eq!(off.keep_in_touch.snoozed_until, None);
    assert_eq!(off.keep_in_touch_due_at, None);
    assert!(database.list_keep_in_touch().unwrap().is_empty());
}

#[test]
fn keep_in_touch_snooze_requires_reminders_and_a_date_within_two_years() {
    let database = database();
    let saved = database
        .save_contact_profile(&contact_request("Lee", "lee@example.com"))
        .unwrap();
    let soon = (Utc::now() + chrono::Duration::days(3)).to_rfc3339();
    assert!(database
        .snooze_keep_in_touch(&saved.id, Some(&soon))
        .is_err());
    database
        .set_keep_in_touch(std::slice::from_ref(&saved.id), Some(30))
        .unwrap();
    for invalid in [
        (Utc::now() - chrono::Duration::days(1)).to_rfc3339(),
        (Utc::now() + chrono::Duration::days(731)).to_rfc3339(),
        "next week".to_string(),
    ] {
        assert!(
            database
                .snooze_keep_in_touch(&saved.id, Some(&invalid))
                .is_err(),
            "{invalid}"
        );
    }
    let edge = (Utc::now() + chrono::Duration::days(729)).to_rfc3339();
    database
        .snooze_keep_in_touch(&saved.id, Some(&edge))
        .unwrap();
    let cleared = database.snooze_keep_in_touch(&saved.id, None).unwrap();
    assert_eq!(cleared.keep_in_touch.snoozed_until, None);
    assert_eq!(cleared.keep_in_touch.snoozed_at, None);
}

#[test]
fn marking_contacted_ends_a_snooze_and_outlives_pruned_mail() {
    let database = database();
    database.adopt_account("you@example.com").unwrap();
    let mut sent = message(
        "kit-prune",
        "kit-thread-prune",
        "2026-09-01T12:00:00Z",
        "hello",
    );
    sent.from = "you@example.com".into();
    sent.to = vec!["ana@example.com".into()];
    database.upsert_thread("you@example.com", &[sent]).unwrap();
    let ana = database
        .set_keep_in_touch(&["derived:ana@example.com".into()], Some(30))
        .unwrap()
        .remove(0);
    let until = (Utc::now() + chrono::Duration::days(60)).to_rfc3339();
    database
        .snooze_keep_in_touch(&ana.id, Some(&until))
        .unwrap();

    let touched = database.mark_contacted(&ana.id).unwrap();
    let touch = touched.keep_in_touch.last_touch_at.clone().unwrap();
    assert_eq!(touched.keep_in_touch.snoozed_until, None);
    let expected = (chrono::DateTime::parse_from_rfc3339(&touch).unwrap()
        + chrono::Duration::days(30))
    .to_rfc3339();
    assert_eq!(
        touched.keep_in_touch_due_at.as_deref(),
        Some(expected.as_str())
    );

    // Retention pruning deletes the mail and its interactions; the logged
    // touch still anchors the due date.
    database
        .with_connection(|connection| {
            connection.execute("DELETE FROM threads", [])?;
            Ok(())
        })
        .unwrap();
    let after = database.get_contact_profile(&ana.id).unwrap().unwrap();
    assert_eq!(after.last_interacted_at, None);
    assert_eq!(
        after.keep_in_touch_due_at.as_deref(),
        Some(expected.as_str())
    );
    assert!(database.mark_contacted("missing-contact").is_err());
}

#[test]
fn saving_the_profile_form_keeps_reminders_and_stores_the_birthday() {
    let database = database();
    let saved = database
        .save_contact_profile(&contact_request("Kim", "kim@example.com"))
        .unwrap();
    database
        .set_keep_in_touch(std::slice::from_ref(&saved.id), Some(90))
        .unwrap();
    let until = (Utc::now() + chrono::Duration::days(5)).to_rfc3339();
    let snoozed = database
        .snooze_keep_in_touch(&saved.id, Some(&until))
        .unwrap();
    let mut form = contact_request("Kim Lee", "kim@example.com");
    form.id = Some(saved.id.clone());
    form.birthday = Some("1990-04-02".into());
    let resaved = database.save_contact_profile(&form).unwrap();
    assert_eq!(resaved.display_name.as_deref(), Some("Kim Lee"));
    assert_eq!(resaved.birthday.as_deref(), Some("1990-04-02"));
    assert_eq!(resaved.keep_in_touch, snoozed.keep_in_touch);
    form.birthday = Some("04-31".into());
    assert!(database.save_contact_profile(&form).is_err());
    form.birthday = None;
    assert_eq!(database.save_contact_profile(&form).unwrap().birthday, None);
}

#[test]
fn keep_in_touch_list_orders_by_due_date_then_birthday_only_contacts() {
    let database = database();
    let later = database
        .save_contact_profile(&contact_request("Later", "later@example.com"))
        .unwrap();
    let sooner = database
        .save_contact_profile(&contact_request("Sooner", "sooner@example.com"))
        .unwrap();
    let mut birthday = contact_request("Birthday", "birthday@example.com");
    birthday.birthday = Some("07-04".into());
    let birthday = database.save_contact_profile(&birthday).unwrap();
    database
        .save_contact_profile(&contact_request("Neither", "neither@example.com"))
        .unwrap();
    database
        .set_keep_in_touch(std::slice::from_ref(&later.id), Some(60))
        .unwrap();
    database
        .set_keep_in_touch(std::slice::from_ref(&sooner.id), Some(7))
        .unwrap();
    let ids = database
        .list_keep_in_touch()
        .unwrap()
        .into_iter()
        .map(|item| item.id)
        .collect::<Vec<_>>();
    assert_eq!(ids, vec![sooner.id, later.id, birthday.id]);
}

#[test]
fn contact_list_uses_sent_to_history_and_timeline_combines_accounts() {
    let database = database();
    database.adopt_account("you@example.com").unwrap();
    database.adopt_account("other@example.com").unwrap();
    let mut sent = message("contact-one", "thread-one", "2026-09-20T12:00:00Z", "hello");
    sent.from = "you@example.com".into();
    sent.to = vec!["Jane <jane@example.com>".into()];
    database.upsert_thread("you@example.com", &[sent]).unwrap();
    let mut received = message("contact-two", "thread-two", "2026-09-21T12:00:00Z", "reply");
    received.from = "Jane <jane@example.com>".into();
    received.to = vec!["you@example.com".into()];
    database
        .upsert_thread("other@example.com", &[received])
        .unwrap();
    let derived = database
        .list_contact_profiles("jane", 20)
        .unwrap()
        .into_iter()
        .find(|item| item.id == "derived:jane@example.com")
        .unwrap();
    assert_eq!(derived.sent_count, 1);
    let saved = database
        .save_contact_profile(&SaveContactRequest {
            birthday: None,
            keep_in_touch: None,
            id: Some(derived.id),
            display_name: Some("Jane".into()),
            role: None,
            company: None,
            location: None,
            bio: None,
            notes: None,
            links: vec![],
            photo_data: None,
            favorite: false,
            addresses: vec!["jane@example.com".into()],
        })
        .unwrap();
    let timeline = database.contact_timeline(&saved.id, 0, 10).unwrap();
    assert_eq!(timeline.len(), 2);
    assert!(timeline
        .iter()
        .any(|item| item.account_id == "you@example.com"));
    assert!(timeline
        .iter()
        .any(|item| item.account_id == "other@example.com"));
    assert!(timeline
        .iter()
        .all(|item| item.contact_email == "jane@example.com"));
    let you = database
        .list_contact_profiles_for_account("jane", 20, Some("you@example.com"))
        .unwrap();
    let other = database
        .list_contact_profiles_for_account("jane", 20, Some("other@example.com"))
        .unwrap();
    assert_eq!(you.len(), 1);
    assert_eq!(other.len(), 1);
    assert_eq!(you[0].id, other[0].id);
    assert_eq!(you[0].sent_count, 1);
    assert_eq!(you[0].received_count, 0);
    assert_eq!(other[0].sent_count, 0);
    assert_eq!(other[0].received_count, 1);
    assert_eq!(
        database
            .contact_timeline_for_account(&saved.id, 0, 10, Some("you@example.com"))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        database
            .contact_timeline_for_account(&saved.id, 0, 10, Some("other@example.com"))
            .unwrap()
            .len(),
        1
    );

    let mut other_only = message(
        "contact-three",
        "thread-three",
        "2026-09-22T12:00:00Z",
        "other",
    );
    other_only.from = "Taylor <taylor@example.com>".into();
    other_only.to = vec!["other@example.com".into()];
    database
        .upsert_thread("other@example.com", &[other_only])
        .unwrap();
    database
        .save_contact_profile(&SaveContactRequest {
            birthday: None,
            keep_in_touch: None,
            id: None,
            display_name: Some("Taylor".into()),
            role: None,
            company: None,
            location: None,
            bio: None,
            notes: None,
            links: vec![],
            photo_data: None,
            favorite: false,
            addresses: vec!["taylor@example.com".into()],
        })
        .unwrap();
    assert!(database
        .list_contact_profiles_for_account("taylor", 20, Some("you@example.com"))
        .unwrap()
        .is_empty());
    assert_eq!(
        database
            .list_contact_profiles_for_account("taylor", 20, Some("other@example.com"))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        database
            .list_contact_profiles_for_account("", 1, Some("you@example.com"))
            .unwrap()[0]
            .id,
        saved.id
    );

    let no_history = database
        .save_contact_profile(&SaveContactRequest {
            birthday: None,
            keep_in_touch: None,
            id: None,
            display_name: Some("New friend".into()),
            role: None,
            company: None,
            location: None,
            bio: None,
            notes: None,
            links: vec![],
            photo_data: None,
            favorite: false,
            addresses: vec!["newfriend@example.com".into()],
        })
        .unwrap();
    assert_eq!(
        database
            .list_contact_profiles_for_account("New friend", 20, Some("you@example.com"))
            .unwrap()[0]
            .id,
        no_history.id
    );
    assert_eq!(
        database
            .list_contact_profiles_for_account("New friend", 20, Some("other@example.com"))
            .unwrap()[0]
            .id,
        no_history.id
    );
}

#[test]
fn contact_timeline_shows_the_address_used_on_the_latest_matching_message() {
    let database = database();
    database.adopt_account("you@example.com").unwrap();
    let mut first = message("first", "thread", "2026-09-20T12:00:00Z", "hello");
    first.from = "Jane <jane@example.com>".into();
    first.to = vec!["you@example.com".into()];
    let mut latest = message("latest", "thread", "2026-09-21T12:00:00Z", "hello again");
    latest.from = "Jane Work <jane@work.example.com>".into();
    latest.to = vec!["you@example.com".into()];
    database
        .upsert_thread("you@example.com", &[first, latest])
        .unwrap();
    let saved = database
        .save_contact_profile(&SaveContactRequest {
            birthday: None,
            keep_in_touch: None,
            id: None,
            display_name: Some("Jane".into()),
            role: None,
            company: None,
            location: None,
            bio: None,
            notes: None,
            links: vec![],
            photo_data: None,
            favorite: false,
            addresses: vec!["jane@example.com".into(), "jane@work.example.com".into()],
        })
        .unwrap();

    let timeline = database.contact_timeline(&saved.id, 0, 10).unwrap();
    assert_eq!(timeline.len(), 1);
    assert_eq!(timeline[0].contact_email, "jane@work.example.com");
}

#[test]
fn contact_index_reads_display_names_with_unquoted_commas() {
    let database = database();
    database.adopt_account("you@example.com").unwrap();
    let mut to_dan = message(
        "comma-one",
        "comma-thread",
        "2026-03-04T01:35:42Z",
        "thanks",
    );
    to_dan.from = "You <you@example.com>".into();
    to_dan.to = vec!["Daniel O'Connor, CFA® <doconnor@wealth.example>".into()];
    let mut from_pat = message(
        "comma-two",
        "comma-other",
        "2026-03-05T12:00:00Z",
        "results",
    );
    from_pat.from = "Smith, Pat, PhD <pat@lab.example>".into();
    from_pat.to = vec!["you@example.com".into()];
    database
        .upsert_thread("you@example.com", &[to_dan])
        .unwrap();
    database
        .upsert_thread("you@example.com", &[from_pat])
        .unwrap();

    let dan = database
        .contact_activity("derived:doconnor@wealth.example")
        .unwrap();
    assert_eq!(
        (dan.sent_count, dan.last_sent_at.as_deref()),
        (1, Some("2026-03-04T01:35:42Z"))
    );
    let pat = database
        .contact_activity("derived:pat@lab.example")
        .unwrap();
    assert_eq!(pat.received_count, 1);

    database.rebuild_contact_interactions().unwrap();
    assert_eq!(
        database
            .contact_activity("derived:doconnor@wealth.example")
            .unwrap()
            .sent_count,
        1
    );
}

#[test]
fn contact_activity_counts_every_address_and_lists_arrivals_newest_first() {
    let database = database();
    database.adopt_account("you@example.com").unwrap();
    let mut first = message(
        "activity-one",
        "activity-thread",
        "2026-07-01T12:00:00Z",
        "report",
    );
    first.from = "Jane <jane@example.com>".into();
    first.to = vec!["you@example.com".into()];
    let mut reply = message(
        "activity-two",
        "activity-thread",
        "2026-07-02T12:00:00Z",
        "thanks",
    );
    reply.from = "you@example.com".into();
    reply.to = vec!["Jane <jane@example.com>".into()];
    let mut work = message(
        "activity-three",
        "activity-other",
        "2026-08-01T12:00:00Z",
        "report",
    );
    work.from = "Jane <jane@work.example.com>".into();
    work.to = vec!["you@example.com".into()];
    let mut newsletter = message(
        "activity-four",
        "activity-news",
        "2026-09-01T12:00:00Z",
        "news",
    );
    newsletter.from = "Jane <jane@example.com>".into();
    newsletter.to = vec!["you@example.com".into()];
    newsletter.unsubscribe = Some(crate::mime::UnsubscribeMetadata {
        one_click_url: None,
        mailto_url: Some("mailto:leave@example.com".into()),
        web_url: None,
        list_id: None,
    });
    database
        .upsert_thread("you@example.com", &[first, reply])
        .unwrap();
    database.upsert_thread("you@example.com", &[work]).unwrap();
    database
        .upsert_thread("you@example.com", &[newsletter])
        .unwrap();
    let saved = database
        .save_contact_profile(&SaveContactRequest {
            birthday: None,
            keep_in_touch: None,
            id: None,
            display_name: Some("Jane".into()),
            role: None,
            company: None,
            location: None,
            bio: None,
            notes: None,
            links: vec![],
            photo_data: None,
            favorite: false,
            addresses: vec!["jane@example.com".into(), "jane@work.example.com".into()],
        })
        .unwrap();

    let activity = database.contact_activity(&saved.id).unwrap();
    assert_eq!(
        (
            activity.sent_count,
            activity.received_count,
            activity.thread_count
        ),
        (1, 2, 2)
    );
    assert_eq!(activity.first_at.as_deref(), Some("2026-07-01T12:00:00Z"));
    assert_eq!(
        activity.last_sent_at.as_deref(),
        Some("2026-07-02T12:00:00Z")
    );
    assert_eq!(
        activity.recent_received_at,
        vec!["2026-08-01T12:00:00Z", "2026-07-01T12:00:00Z"]
    );

    let derived = database
        .contact_activity("derived:jane@example.com")
        .unwrap();
    assert_eq!(
        (
            derived.sent_count,
            derived.received_count,
            derived.thread_count
        ),
        (1, 1, 1)
    );
    let stranger = database
        .contact_activity("derived:nobody@example.com")
        .unwrap();
    assert_eq!(
        (
            stranger.sent_count,
            stranger.received_count,
            stranger.thread_count
        ),
        (0, 0, 0)
    );
    assert!(stranger.first_at.is_none());
    assert!(database
        .contact_activity("contact:unknown")
        .unwrap()
        .recent_received_at
        .is_empty());
}

#[test]
fn contact_files_list_attachments_the_person_sent_without_inline_images() {
    let database = database();
    database.adopt_account("you@example.com").unwrap();
    let attachment = |id: &str, filename: &str, inline: bool| crate::models::MessageAttachment {
        id: id.into(),
        filename: filename.into(),
        mime_type: if inline {
            "image/png".into()
        } else {
            "application/pdf".into()
        },
        size: 1200,
        content_id: inline.then(|| format!("{id}@cid")),
        inline,
    };
    let mut older = message(
        "files-one",
        "files-thread",
        "2026-08-01T12:00:00Z",
        "report",
    );
    older.from = "Jane <jane@example.com>".into();
    older.to = vec!["you@example.com".into()];
    older.attachments = vec![
        attachment("logo-1", "image001.png", true),
        attachment("report-1", "August.pdf", false),
    ];
    let mut mine = message(
        "files-two",
        "files-thread",
        "2026-08-02T12:00:00Z",
        "my notes",
    );
    mine.from = "you@example.com".into();
    mine.to = vec!["Jane <jane@example.com>".into()];
    mine.attachments = vec![attachment("mine-1", "Notes.pdf", false)];
    let mut newer = message(
        "files-three",
        "files-other",
        "2026-09-01T12:00:00Z",
        "report",
    );
    newer.from = "Jane <jane@example.com>".into();
    newer.to = vec!["you@example.com".into()];
    newer.attachments = vec![
        attachment("report-2", "September.pdf", false),
        attachment("data-2", "September.csv", false),
    ];
    let mut bulk = message("files-four", "files-bulk", "2026-09-02T12:00:00Z", "promo");
    bulk.from = "Jane <jane@example.com>".into();
    bulk.to = vec!["you@example.com".into()];
    bulk.unsubscribe = Some(crate::mime::UnsubscribeMetadata {
        one_click_url: None,
        mailto_url: Some("mailto:leave@example.com".into()),
        web_url: None,
        list_id: None,
    });
    bulk.attachments = vec![attachment("promo-1", "Promo.pdf", false)];
    database
        .upsert_thread("you@example.com", &[older, mine])
        .unwrap();
    database.upsert_thread("you@example.com", &[newer]).unwrap();
    database.upsert_thread("you@example.com", &[bulk]).unwrap();
    // Calendar invitations, recognized by type or by extension, stay out of the list and its total.
    let mut invite = message(
        "files-five",
        "files-invite",
        "2026-09-03T12:00:00Z",
        "invitation",
    );
    invite.from = "Jane <jane@example.com>".into();
    invite.to = vec!["you@example.com".into()];
    invite.attachments = vec![
        crate::models::MessageAttachment {
            mime_type: "text/calendar".into(),
            ..attachment("invite-1", "invite.ics", false)
        },
        crate::models::MessageAttachment {
            mime_type: "application/octet-stream".into(),
            ..attachment("invite-2", "Meeting.ICS", false)
        },
        crate::models::MessageAttachment {
            mime_type: "Text/Calendar; method=REQUEST".into(),
            ..attachment("invite-3", "event", false)
        },
    ];
    database
        .upsert_thread("you@example.com", &[invite])
        .unwrap();

    let files = database
        .contact_files("derived:jane@example.com", 2)
        .unwrap();
    assert_eq!(files.total, 3);
    assert_eq!(
        files
            .files
            .iter()
            .map(|file| file.attachment.filename.as_str())
            .collect::<Vec<_>>(),
        vec!["September.pdf", "September.csv"]
    );
    assert!(files
        .files
        .iter()
        .all(|file| file.message_id == "files-three" && file.thread_id.ends_with("files-other")));
    let all = database
        .contact_files("derived:jane@example.com", 50)
        .unwrap();
    assert_eq!(
        all.files.last().map(|file| file.attachment.id.as_str()),
        Some("report-1")
    );
    assert!(all.files.iter().all(|file| !file.attachment.inline));
    assert!(all.files.iter().all(|file| file.message_id != "files-five"));
    assert_eq!(
        database
            .contact_files("derived:nobody@example.com", 5)
            .unwrap()
            .total,
        0
    );
}

#[test]
fn contact_files_list_one_file_once_from_its_newest_message() {
    let database = database();
    database.adopt_account("you@example.com").unwrap();
    database.adopt_account("you@work.example").unwrap();
    let attachment = |id: &str, filename: &str, size: u64| crate::models::MessageAttachment {
        id: id.into(),
        filename: filename.into(),
        mime_type: "application/pdf".into(),
        size,
        content_id: None,
        inline: false,
    };
    let from_jane = |id: &str,
                     thread: &str,
                     at: &str,
                     to: &str,
                     files: Vec<crate::models::MessageAttachment>| {
        let mut item = message(id, thread, at, "resume");
        item.from = "Jane <jane@example.com>".into();
        item.to = vec![to.into()];
        item.attachments = files;
        item
    };
    // One email delivered to both accounts is stored once per account.
    database
        .upsert_thread(
            "you@example.com",
            &[from_jane(
                "copy-home",
                "resume-home",
                "2026-06-01T16:04:35Z",
                "you@example.com",
                vec![attachment("part-1", "Resume.pdf", 37620)],
            )],
        )
        .unwrap();
    database
        .upsert_thread(
            "you@work.example",
            &[from_jane(
                "copy-work",
                "resume-work",
                "2026-06-01T16:04:35Z",
                "you@work.example",
                vec![attachment("part-1", "Resume.pdf", 37620)],
            )],
        )
        .unwrap();
    // Re-sent later under different capitalization: the newest message wins.
    database
        .upsert_thread(
            "you@example.com",
            &[from_jane(
                "resent",
                "resume-again",
                "2026-06-03T09:00:00Z",
                "you@example.com",
                vec![attachment("part-1", "resume.PDF", 37620)],
            )],
        )
        .unwrap();
    // Same name, different size: a revised file, listed separately.
    database
        .upsert_thread(
            "you@example.com",
            &[from_jane(
                "revised",
                "resume-revised",
                "2026-06-02T09:00:00Z",
                "you@example.com",
                vec![attachment("part-1", "Resume.pdf", 41000)],
            )],
        )
        .unwrap();

    let files = database
        .contact_files("derived:jane@example.com", 50)
        .unwrap();
    assert_eq!(files.total, 2);
    assert_eq!(
        files
            .files
            .iter()
            .map(|file| (file.message_id.as_str(), file.attachment.size))
            .collect::<Vec<_>>(),
        vec![("resent", 37620), ("revised", 41000)]
    );
    let first = database
        .contact_files("derived:jane@example.com", 1)
        .unwrap();
    assert_eq!((first.total, first.files.len()), (2, 1));
}

#[test]
fn domain_context_lists_other_people_at_the_domain_and_their_conversations() {
    let database = database();
    database.adopt_account("you@example.com").unwrap();
    let mut jane = message(
        "domain-one",
        "domain-jane",
        "2026-09-01T12:00:00Z",
        "report",
    );
    jane.from = "Jane <jane@acme.example>".into();
    jane.to = vec!["you@example.com".into()];
    let mut sam = message("domain-two", "domain-sam", "2026-09-02T12:00:00Z", "lunch?");
    sam.from = "Sam Lee <sam@acme.example>".into();
    sam.to = vec!["you@example.com".into(), "Jane <jane@acme.example>".into()];
    let mut to_pat = message(
        "domain-three",
        "domain-pat",
        "2026-09-03T12:00:00Z",
        "agenda",
    );
    to_pat.from = "you@example.com".into();
    to_pat.to = vec!["Pat <pat@acme.example>".into()];
    let mut lookalike = message(
        "domain-four",
        "domain-lookalike",
        "2026-09-04T12:00:00Z",
        "hi",
    );
    lookalike.from = "Kim <kim@notacme.example>".into();
    lookalike.to = vec!["you@example.com".into()];
    let mut sub = message("domain-five", "domain-sub", "2026-09-05T12:00:00Z", "hi");
    sub.from = "Lee <lee@mail.acme.example>".into();
    sub.to = vec!["you@example.com".into()];
    for item in [jane, sam, to_pat, lookalike, sub] {
        database.upsert_thread("you@example.com", &[item]).unwrap();
    }

    let context = database
        .domain_context("ACME.example", &["Jane@acme.example".into()], 10)
        .unwrap();
    assert_eq!(
        context
            .people
            .iter()
            .map(|person| (person.email.as_str(), person.display_name.as_deref()))
            .collect::<Vec<_>>(),
        vec![
            ("pat@acme.example", Some("Pat")),
            ("sam@acme.example", Some("Sam Lee"))
        ]
    );
    assert_eq!(
        context
            .threads
            .iter()
            .map(|thread| thread.thread_id.rsplit(':').next().unwrap())
            .collect::<Vec<_>>(),
        vec!["domain-pat", "domain-sam"]
    );
    assert_eq!(
        database
            .domain_context("acme.example", &[], 1)
            .unwrap()
            .people
            .len(),
        1
    );
    assert!(database
        .domain_context("localhost", &[], 10)
        .unwrap()
        .people
        .is_empty());
    assert!(database
        .domain_context("", &[], 10)
        .unwrap()
        .threads
        .is_empty());
}

#[test]
fn contact_tasks_span_every_conversation_with_the_person_and_only_open_work() {
    let database = database();
    database.adopt_account("you@example.com").unwrap();
    database.adopt_account("other@example.com").unwrap();
    let mut to_jane = message(
        "jane-one",
        "jane-thread-one",
        "2026-09-20T12:00:00Z",
        "hello",
    );
    to_jane.from = "you@example.com".into();
    to_jane.to = vec!["Jane <jane@example.com>".into()];
    database
        .upsert_thread("you@example.com", &[to_jane])
        .unwrap();
    let mut from_jane_alt = message(
        "jane-two",
        "jane-thread-two",
        "2026-09-21T12:00:00Z",
        "reply",
    );
    from_jane_alt.from = "Jane <jane@work.example.com>".into();
    from_jane_alt.to = vec!["other@example.com".into()];
    database
        .upsert_thread("other@example.com", &[from_jane_alt])
        .unwrap();
    let mut from_taylor = message(
        "taylor-one",
        "taylor-thread",
        "2026-09-22T12:00:00Z",
        "other",
    );
    from_taylor.from = "Taylor <taylor@example.com>".into();
    from_taylor.to = vec!["you@example.com".into()];
    database
        .upsert_thread("you@example.com", &[from_taylor])
        .unwrap();
    let thread_for = |contact: &str| {
        database
            .contact_timeline(contact, 0, 10)
            .unwrap()
            .into_iter()
            .map(|item| (item.thread_id, item.account_id, item.subject))
            .collect::<Vec<_>>()
    };
    let add_task = |(thread_id, account_id, subject): &(String, String, String), title: &str| {
        database
            .create_task(&crate::models::CreateTaskRequest {
                account_id: account_id.clone(),
                thread_id: Some(thread_id.clone()),
                source_message_id: None,
                subject_snapshot: Some(subject.clone()),
                title: title.into(),
                notes: None,
                kind: "action".into(),
                due_kind: "none".into(),
                due_value: None,
                time_zone: None,
                repeat_interval_days: None,
                evidence_text: None,
                goal_id: None,
            })
            .unwrap()
    };
    let jane_primary = thread_for("derived:jane@example.com");
    let jane_work = thread_for("derived:jane@work.example.com");
    let taylor = thread_for("derived:taylor@example.com");
    add_task(&jane_primary[0], "Send Jane the deck");
    let done = add_task(&jane_primary[0], "Already done");
    database
        .set_task_status(&done.id, "completed", "user")
        .unwrap();
    add_task(&jane_work[0], "Review Jane's contract");
    add_task(&taylor[0], "Call Taylor");
    let titles = |id: &str| {
        let mut titles = database
            .list_contact_tasks(id)
            .unwrap()
            .into_iter()
            .map(|task| task.title)
            .collect::<Vec<_>>();
        titles.sort();
        titles
    };

    assert_eq!(
        titles("derived:jane@example.com"),
        vec!["Send Jane the deck"]
    );
    let saved = database
        .save_contact_profile(&SaveContactRequest {
            birthday: None,
            keep_in_touch: None,
            id: None,
            display_name: Some("Jane".into()),
            role: None,
            company: None,
            location: None,
            bio: None,
            notes: None,
            links: vec![],
            photo_data: None,
            favorite: false,
            addresses: vec!["jane@example.com".into(), "jane@work.example.com".into()],
        })
        .unwrap();
    assert_eq!(
        titles(&saved.id),
        vec!["Review Jane's contract", "Send Jane the deck"]
    );
    assert!(titles("contact:unknown").is_empty());

    for index in 0..crate::db::tasks::MAX_CONTACT_TASKS {
        add_task(&taylor[0], &format!("Taylor task {index}"));
    }
    assert_eq!(
        database
            .list_contact_tasks("derived:taylor@example.com")
            .unwrap()
            .len(),
        crate::db::tasks::MAX_CONTACT_TASKS
    );
    assert_eq!(
        database
            .list_contact_tasks("derived:jane@example.com")
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn removing_an_account_clears_its_local_contact_interactions_but_keeps_saved_profile() {
    let database = database();
    let mut sent = message(
        "contact-account-remove",
        "contact-account-remove-thread",
        "2026-09-22T12:00:00Z",
        "hello",
    );
    sent.from = "you@example.com".into();
    sent.to = vec!["Sam <sam@example.com>".into()];
    database.upsert_thread("you@example.com", &[sent]).unwrap();
    let saved = database
        .save_contact_profile(&SaveContactRequest {
            birthday: None,
            keep_in_touch: None,
            id: None,
            display_name: Some("Sam".into()),
            role: None,
            company: None,
            location: None,
            bio: None,
            notes: Some("Keep this note".into()),
            links: vec![],
            photo_data: None,
            favorite: false,
            addresses: vec!["sam@example.com".into()],
        })
        .unwrap();
    assert_eq!(
        database.contact_timeline(&saved.id, 0, 10).unwrap().len(),
        1
    );
    database.remove_account("you@example.com").unwrap();
    assert!(database
        .contact_timeline(&saved.id, 0, 10)
        .unwrap()
        .is_empty());
    let remaining = database.get_contact_profile(&saved.id).unwrap().unwrap();
    assert_eq!(remaining.notes.as_deref(), Some("Keep this note"));
    assert_eq!(remaining.sent_count, 0);
}

fn contact_request(name: &str, email: &str) -> SaveContactRequest {
    SaveContactRequest {
        birthday: None,
        keep_in_touch: None,
        id: None,
        display_name: Some(name.into()),
        role: None,
        company: None,
        location: None,
        bio: None,
        notes: None,
        links: vec![],
        photo_data: None,
        favorite: false,
        addresses: vec![email.into()],
    }
}
