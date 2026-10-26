use super::*;
use crate::{
    correspondence::Draft,
    models::MailProviderKind,
    signatures::tests::{defaults, template},
};

fn gmail_database() -> Database {
    let db = Database::open_memory();
    db.adopt_mail_account("me@example.com", MailProviderKind::Gmail)
        .unwrap();
    db
}

#[test]
fn long_account_and_alias_pairs_have_bounded_stable_replica_ids() {
    let account = format!("{}@example.com", "a".repeat(308));
    let alias = format!("{}@example.com", "b".repeat(308));
    let value = defaults(&account, &alias, None, None);
    assert!(value.id.len() <= threestrands_sync_envelope::limits::MAX_ENTITY_ID_BYTES);
    let db = Database::open_memory();
    db.store_signature_defaults(&value).unwrap();
    assert_eq!(
        SignatureDefaults::key(&account.to_uppercase(), &alias.to_uppercase()),
        value.id
    );
    assert_ne!(SignatureDefaults::key(&alias, &account), value.id);
}

#[test]
fn templates_defaults_and_alias_inheritance_are_independent() {
    let db = Database::open_memory();
    db.store_signature(&template("full")).unwrap();
    db.store_signature(&template("short")).unwrap();
    db.store_signature_defaults(&defaults("me@example.com", "", Some("full"), Some("short")))
        .unwrap();
    assert_eq!(
        db.default_draft_signature("ME@example.com", "alias@example.com", "new")
            .unwrap()
            .unwrap()
            .name,
        "full"
    );
    for mode in ["reply", "replyAll", "forward"] {
        assert_eq!(
            db.default_draft_signature("me@example.com", "me@example.com", mode)
                .unwrap()
                .unwrap()
                .name,
            "short"
        );
    }
    db.store_signature_defaults(&defaults(
        "me@example.com",
        "alias@example.com",
        None,
        Some("full"),
    ))
    .unwrap();
    assert!(db
        .default_draft_signature("me@example.com", "ALIAS@example.com", "new")
        .unwrap()
        .is_none());
    db.delete_signature_defaults(&SignatureDefaults::key(
        "me@example.com",
        "alias@example.com",
    ))
    .unwrap();
    assert_eq!(
        db.default_draft_signature("me@example.com", "alias@example.com", "new")
            .unwrap()
            .unwrap()
            .name,
        "full"
    );
    db.delete_signature("full").unwrap();
    assert!(db
        .default_draft_signature("me@example.com", "me@example.com", "new")
        .unwrap()
        .is_none());
}

#[test]
fn drafts_snapshot_templates_and_respect_explicit_choices_and_edits() {
    let db = Database::open_memory();
    db.store_signature(&template("original")).unwrap();
    db.store_signature_defaults(&defaults("first@example.com", "", Some("original"), None))
        .unwrap();
    db.store_signature(&template("second")).unwrap();
    db.store_signature_defaults(&defaults("second@example.com", "", Some("second"), None))
        .unwrap();
    let d = db
        .create_draft("new", None, "first@example.com", &[])
        .unwrap();
    assert_eq!(d.signature.as_ref().unwrap().text, "original");
    let mut changed = template("original");
    changed.text = "Updated template".into();
    db.store_signature(&changed).unwrap();
    db.delete_signature("original").unwrap();
    assert_eq!(db.draft(&d.id).unwrap().signature.unwrap().text, "original");
    let moved = db.set_draft_account(&d.id, "second@example.com").unwrap();
    assert_eq!(moved.signature.as_ref().unwrap().text, "second");
    let mut edited = moved;
    edited.signature.as_mut().unwrap().text = "My personal sign-off".into();
    let saved = db.save_draft(edited).unwrap();
    assert!(!saved.signature.as_ref().unwrap().automatic);
    let back = db
        .set_draft_account(&saved.id, "first@example.com")
        .unwrap();
    assert_eq!(back.signature.unwrap().text, "My personal sign-off");
    let mut none = db.draft(&d.id).unwrap();
    none.signature = Some(DraftSignature {
        id: None,
        name: "No Signature".into(),
        html: String::new(),
        text: String::new(),
        automatic: false,
        html_offset: 0,
        text_offset: 0,
    });
    db.save_draft(none).unwrap();
    assert!(db
        .set_draft_account(&d.id, "second@example.com")
        .unwrap()
        .signature
        .unwrap()
        .html
        .is_empty());
    // Legacy draft payloads still decode without requiring a new field.
    let mut legacy = serde_json::to_value(d).unwrap();
    legacy.as_object_mut().unwrap().remove("signature");
    assert!(serde_json::from_value::<Draft>(legacy)
        .unwrap()
        .signature
        .is_none());
}

#[test]
fn imported_templates_and_defaults_are_both_recorded_for_sync() {
    let db = gmail_database();
    db.set_beta_features_enabled(true).unwrap();
    db.import_gmail_signature("me@example.com", &template("gmail"), true)
        .unwrap();
    let id = SignatureDefaults::key("me@example.com", "");
    let connection = db.connection().unwrap();
    for (entity_type, entity_id) in [
        (EntityType::Signature.as_str(), "gmail"),
        (EntityType::SignatureDefaults.as_str(), id.as_str()),
    ] {
        let count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sync_values WHERE entity_type=?1 AND entity_id=?2",
                params![entity_type, entity_id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(count > 0, "{entity_type} must replicate");
    }
}

#[test]
fn importing_preserves_existing_reply_and_alias_defaults() {
    let db = gmail_database();
    db.store_signature(&template("old")).unwrap();
    db.store_signature_defaults(&defaults("me@example.com", "", Some("old"), Some("old")))
        .unwrap();
    db.store_signature_defaults(&defaults(
        "me@example.com",
        "alias@example.com",
        Some("old"),
        None,
    ))
    .unwrap();
    db.import_gmail_signature("me@example.com", &template("gmail"), true)
        .unwrap();
    let library = db.signature_library().unwrap();
    let base = library
        .defaults
        .iter()
        .find(|d| d.address.is_empty())
        .unwrap();
    assert_eq!(base.new_signature_id.as_deref(), Some("gmail"));
    assert_eq!(base.reply_signature_id.as_deref(), Some("old"));
    assert_eq!(
        library
            .defaults
            .iter()
            .find(|d| !d.address.is_empty())
            .unwrap()
            .new_signature_id
            .as_deref(),
        Some("old")
    );
    assert_eq!(library.signatures.len(), 2);
    // A retry uses the same identity rather than duplicating the template.
    db.import_gmail_signature("me@example.com", &template("gmail"), true)
        .unwrap();
    assert_eq!(db.signature_library().unwrap().signatures.len(), 2);
}

#[test]
fn importing_without_a_default_leaves_all_defaults_unchanged() {
    let db = gmail_database();
    db.store_signature_defaults(&defaults("me@example.com", "", None, Some("short")))
        .unwrap();
    db.import_gmail_signature("me@example.com", &template("gmail"), false)
        .unwrap();
    let library = db.signature_library().unwrap();
    assert_eq!(library.signatures.len(), 1);
    assert_eq!(library.defaults.len(), 1);
    assert_eq!(library.defaults[0].new_signature_id, None);
    assert_eq!(
        library.defaults[0].reply_signature_id.as_deref(),
        Some("short")
    );
}

#[test]
fn importing_creates_a_new_message_default_only() {
    let db = gmail_database();
    db.import_gmail_signature("me@example.com", &template("gmail"), true)
        .unwrap();
    let library = db.signature_library().unwrap();
    assert_eq!(library.defaults.len(), 1);
    assert_eq!(
        library.defaults[0].new_signature_id.as_deref(),
        Some("gmail")
    );
    assert_eq!(library.defaults[0].reply_signature_id, None);
}

#[test]
fn invalid_input_or_a_failed_default_write_cannot_leave_a_partial_import() {
    let db = gmail_database();
    let mut invalid = template("invalid");
    invalid.name = "bad\nname".into();
    assert!(db
        .import_gmail_signature("me@example.com", &invalid, true)
        .is_err());
    db.with_connection(|c| { c.execute_batch("CREATE TRIGGER reject_import_defaults BEFORE INSERT ON signature_defaults BEGIN SELECT RAISE(ABORT, 'storage failure'); END;")?; Ok(()) }).unwrap();
    assert!(db
        .import_gmail_signature("me@example.com", &template("gmail"), true)
        .is_err());
    let library = db.signature_library().unwrap();
    assert!(library.signatures.is_empty());
    assert!(library.defaults.is_empty());
}

#[test]
fn importing_rejects_unknown_imap_and_other_device_accounts_before_writes() {
    let db = gmail_database();
    db.adopt_mail_account("imap@example.com", MailProviderKind::Imap)
        .unwrap();
    db.with_connection(|c| {
        c.execute(
            "UPDATE accounts SET on_this_device=0 WHERE email='me@example.com'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    for account in ["unknown@example.com", "imap@example.com", "me@example.com"] {
        assert!(db.require_local_gmail_account(account).is_err());
        assert!(db
            .import_gmail_signature(account, &template("gmail"), true)
            .is_err());
    }
    assert!(db.signature_library().unwrap().signatures.is_empty());
}
