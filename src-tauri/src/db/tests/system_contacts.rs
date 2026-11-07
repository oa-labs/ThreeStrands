use super::*;
fn source() -> ContactConnection {
    ContactConnection {
        sources: vec![],
        selected_sources: vec!["icloud".into()],
        refreshed_at: None,
    }
}
fn contact() -> SystemContact {
    SystemContact {
        id: "native".into(),
        source_id: "icloud".into(),
        display_name: Some("Ada Lovelace".into()),
        company: Some("Analytical Engines".into()),
        role: Some("Programmer".into()),
        addresses: vec![
            "ADA@example.com".into(),
            "ada+work@example.org".into(),
            "bad\r\nBcc: evil@example.com".into(),
        ],
    }
}
#[test]
fn cache_populates_browsing_and_suggestions_but_not_exports_or_saved_profiles() {
    let db = Database::open_memory();
    db.replace_system_contacts(&source(), &[contact()]).unwrap();
    let profiles = db.list_contact_profiles("Ada", 100).unwrap();
    assert_eq!(profiles.len(), 1);
    let p = &profiles[0];
    assert!(p.id.starts_with("derived:mac:"));
    assert_eq!(p.addresses, vec!["ada@example.com", "ada+work@example.org"]);
    assert_eq!(
        db.get_contact_profile(&p.id)
            .unwrap()
            .unwrap()
            .company
            .as_deref(),
        Some("Analytical Engines")
    );
    assert_eq!(db.contact_address_list(&p.id).unwrap(), p.addresses);
    assert_eq!(
        db.resolve_contact_ids_with_system(&["ADA@example.com".into()])
            .unwrap()["ada@example.com"],
        p.id
    );
    // Internal group/import ownership remains about saved profiles only.
    assert!(db
        .contact_ids_for_addresses(&["ada@example.com".into()])
        .unwrap()
        .is_empty());
    assert!(db.list_saved_contact_profiles().unwrap().is_empty());
    assert_eq!(
        db.list_contact_suggestions("any@example.org", "Ada", 10)
            .unwrap()
            .len(),
        2
    );
    db.set_contact_suppressed("ada@example.com", true).unwrap();
    assert_eq!(
        db.list_contact_suggestions("any@example.org", "Ada", 10)
            .unwrap()
            .len(),
        1
    );
    // Suppression survives source refresh and does not hide contact browsing.
    db.replace_system_contacts(&source(), &[contact()]).unwrap();
    assert_eq!(
        db.list_contact_suggestions("any@example.org", "Ada", 10)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(db.list_contact_profiles("Ada", 100).unwrap().len(), 1);
}
#[test]
fn deliberate_local_copies_survive_refresh_disconnect_and_source_deletions() {
    let db = Database::open_memory();
    db.replace_system_contacts(&source(), &[contact()]).unwrap();
    let p = db.list_contact_profiles("Ada", 100).unwrap().remove(0);
    let local_id = db.ensure_saved_contact(&p.id).unwrap();
    assert!(!local_id.starts_with("derived:"));
    db.with_connection(|c| {
        c.execute(
            "UPDATE contacts SET notes='Local notes',favorite=1 WHERE id=?1",
            [&local_id],
        )?;
        Ok(())
    })
    .unwrap();
    let mut changed = contact();
    changed.display_name = Some("Changed on Mac".into());
    db.replace_system_contacts(&source(), &[changed]).unwrap();
    let profiles = db.list_contact_profiles("Ada", 100).unwrap();
    assert_eq!(profiles.len(), 1);
    assert_eq!(profiles[0].id, local_id);
    assert_eq!(profiles[0].display_name.as_deref(), Some("Ada Lovelace"));
    db.disconnect_system_contacts().unwrap();
    let local = db.get_contact_profile(&local_id).unwrap().unwrap();
    assert_eq!(local.notes.as_deref(), Some("Local notes"));
    assert!(local.favorite);
    assert_eq!(local.company.as_deref(), Some("Analytical Engines"));
    assert!(db.get_contact_profile(&p.id).unwrap().is_none());
}
#[test]
fn atomic_replacement_removes_deleted_contacts_and_rejects_unselected_sources() {
    let db = Database::open_memory();
    db.replace_system_contacts(&source(), &[contact()]).unwrap();
    let mut bad = contact();
    bad.source_id = "excluded-google".into();
    assert!(db.replace_system_contacts(&source(), &[bad]).is_err());
    assert_eq!(db.system_contact_count().unwrap(), 1);
    db.replace_system_contacts(&source(), &[]).unwrap();
    assert_eq!(db.system_contact_count().unwrap(), 0);
    assert!(db
        .list_contact_suggestions("any@example.org", "Ada", 10)
        .unwrap()
        .is_empty());
}

#[test]
fn scoped_views_preserve_correspondence_scope_for_source_contacts() {
    let db = Database::open_memory();
    db.replace_system_contacts(&source(), &[contact()]).unwrap();
    assert_eq!(
        db.list_contact_profiles_for_account("Ada", 100, Some("other@example.org"))
            .unwrap()
            .len(),
        1
    );
    let mut mail = crate::db::test_support::message(
        "source-sent",
        "source-thread",
        "2026-10-31T00:00:00Z",
        "Hello",
    );
    mail.from = "you@example.com".into();
    mail.to = vec!["ada@example.com".into()];
    db.upsert_thread("you@example.com", &[mail]).unwrap();
    let own = db
        .list_contact_profiles_for_account("Ada", 100, Some("you@example.com"))
        .unwrap();
    assert_eq!(own.len(), 1);
    assert_eq!(own[0].sent_count, 1);
    assert!(db
        .list_contact_profiles_for_account("Ada", 100, Some("other@example.org"))
        .unwrap()
        .is_empty());
}

#[test]
fn large_address_books_remain_searchable_beyond_the_interactive_list_limit() {
    let db = Database::open_memory();
    let contacts = (0..5_001)
        .map(|i| SystemContact {
            id: format!("native-{i}"),
            source_id: "icloud".into(),
            display_name: Some(format!("Person {i}")),
            addresses: vec![format!("person-{i}@example.org")],
            ..Default::default()
        })
        .collect::<Vec<_>>();
    db.replace_system_contacts(&source(), &contacts).unwrap();
    assert_eq!(db.system_contact_count().unwrap(), 5_001);
    let found = db.list_contact_profiles("person-5000@", 500).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].addresses[0], "person-5000@example.org");
    assert_eq!(
        db.list_contact_suggestions("you@example.org", "person-5000@", 8)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn source_cards_do_not_merge_people_and_saved_ownership_wins_consistently() {
    let db = Database::open_memory();
    let first = contact();
    let mut second = contact();
    second.id = "another-native-card".into();
    second.addresses = vec!["ada@example.com".into(), "another@example.org".into()];
    db.replace_system_contacts(&source(), &[first, second])
        .unwrap();
    assert_eq!(db.list_contact_profiles("Ada", 100).unwrap().len(), 2);
    let suggestions = db
        .list_contact_suggestions("you@example.org", "ada@example.com", 10)
        .unwrap();
    assert_eq!(suggestions.len(), 1);
    let id = db.list_contact_profiles("another@", 100).unwrap()[0]
        .id
        .clone();
    db.ensure_saved_contact(&id).unwrap();
    // A source card overlapping any saved address is not partially offered as
    // a second person, even through its other address.
    assert!(db
        .list_contact_suggestions("you@example.org", "ada+work", 10)
        .unwrap()
        .is_empty());
    assert_eq!(db.list_contact_profiles("Ada", 100).unwrap().len(), 1);
}
