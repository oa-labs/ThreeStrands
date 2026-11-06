use super::*;
use crate::db::{test_support::TempDbPath, Database};
use crate::provider::imap::ImapLocation;
use std::sync::Arc;

fn inputs(gmail_id: Option<u64>) -> IdentityInputs {
    IdentityInputs {
        message_id: Some("<shared@example.test>".into()),
        gmail_message_id: gmail_id,
        ..Default::default()
    }
}

fn locate(store: &ImapStateStore, mailbox: &str, validity: i64, uid: i64, id: &str) {
    store
        .upsert_location(&ImapLocation {
            mailbox: mailbox.into(),
            uidvalidity: validity,
            uid,
            message_id: id.into(),
            flags_json: "[]".into(),
            modseq: None,
        })
        .unwrap();
}

#[test]
fn identity_reservations_survive_reopen_without_a_location_or_cursor_commit() {
    let path = TempDbPath::new();
    let id;
    {
        let db = Arc::new(Database::open(&path.path).unwrap());
        let store = ImapStateStore::new(db, "me@example.com");
        id = store
            .resolve_message_identity("INBOX", 1, 7, &inputs(Some(u64::MAX)))
            .unwrap();
        assert!(store.locations_in_mailbox("INBOX").unwrap().is_empty());
        assert_eq!(
            store.generation().unwrap(),
            0,
            "reservations cannot advance sync"
        );
    }
    let db = Arc::new(Database::open(&path.path).unwrap());
    let store = ImapStateStore::new(db.clone(), "me@example.com");
    let mut moved = inputs(Some(u64::MAX));
    moved.message_id = Some("<different-spelling@example.test>".into());
    assert_eq!(
        store
            .resolve_message_identity("Archive", 900, 80, &moved)
            .unwrap(),
        id
    );
    let token: String = db.test_with_connection(|c| Ok(c.query_row(
        "SELECT external_id FROM imap_message_identities WHERE namespace='gmail-message-id'", [], |r| r.get(0),
    )?)).unwrap();
    assert_eq!(
        token, "ffffffffffffffff",
        "wire decimal IDs canonicalize to Gmail API hex without precision loss"
    );
    let other = ImapStateStore::new(db, "other@example.com");
    assert_ne!(
        other
            .resolve_message_identity("INBOX", 1, 7, &inputs(Some(u64::MAX)))
            .unwrap(),
        id
    );
}

#[test]
fn exact_location_learns_new_namespaces_then_resolves_copies_and_uid_resets() {
    let db = Arc::new(Database::open_memory());
    let store = ImapStateStore::new(db, "me@example.com");
    let legacy = inputs(None);
    let id = store
        .resolve_message_identity("INBOX", 1, 7, &legacy)
        .unwrap();
    locate(&store, "INBOX", 1, 7, &id);
    let mut enriched = inputs(Some(123));
    enriched.email_id = Some("OpaqueServerId".into());
    assert_eq!(
        store
            .resolve_message_identity("INBOX", 1, 7, &enriched)
            .unwrap(),
        id
    );
    // Either namespace now suffices, including a fresh mailbox incarnation.
    let object_only = IdentityInputs {
        email_id: enriched.email_id,
        ..Default::default()
    };
    assert_eq!(
        store
            .resolve_message_identity("Sent", 90, 88, &object_only)
            .unwrap(),
        id
    );
    store.drop_mailbox_locations("INBOX").unwrap();
    assert_eq!(
        store
            .resolve_message_identity("INBOX", 200, 1, &inputs(Some(123)))
            .unwrap(),
        id
    );
}

#[test]
fn different_server_messages_with_identical_headers_remain_distinct() {
    let store = ImapStateStore::new(Arc::new(Database::open_memory()), "me@example.com");
    let a = store
        .resolve_message_identity("INBOX", 1, 1, &inputs(Some(10)))
        .unwrap();
    let b = store
        .resolve_message_identity("INBOX", 1, 2, &inputs(Some(11)))
        .unwrap();
    assert_ne!(a, b);
    assert_eq!(
        store
            .resolve_message_identity("Sent", 2, 90, &inputs(Some(10)))
            .unwrap(),
        a
    );
    assert_eq!(
        store
            .resolve_message_identity("Archive", 3, 60, &inputs(Some(11)))
            .unwrap(),
        b
    );
    assert!(
        store
            .resolve_message_identity("Trash", 4, 77, &inputs(None))
            .is_err(),
        "ambiguous header evidence cannot choose either message when a server ID is absent"
    );
    // Headerless mail is also separated by server identity, not envelope hash.
    let no_header = |id| IdentityInputs {
        gmail_message_id: Some(id),
        ..Default::default()
    };
    assert_ne!(
        store
            .resolve_message_identity("INBOX", 1, 3, &no_header(12))
            .unwrap(),
        store
            .resolve_message_identity("INBOX", 1, 4, &no_header(13))
            .unwrap()
    );
}

#[test]
fn header_evidence_cannot_bind_a_new_server_id_to_legacy_mail_without_a_location() {
    let store = ImapStateStore::new(Arc::new(Database::open_memory()), "me@example.com");
    let legacy = store
        .resolve_message_identity("INBOX", 1, 7, &inputs(None))
        .unwrap();
    let new = store
        .resolve_message_identity("Archive", 2, 80, &inputs(Some(123)))
        .unwrap();
    assert_ne!(
        legacy, new,
        "a new server message must not steal existing task links via matching headers"
    );
    assert_eq!(
        store
            .resolve_message_identity("Sent", 3, 4, &inputs(Some(123)))
            .unwrap(),
        new
    );
}

#[test]
fn contradictory_server_mapping_is_retained_and_never_overwrites_a_location() {
    let path = TempDbPath::new();
    let (a, b);
    {
        let db = Arc::new(Database::open(&path.path).unwrap());
        let store = ImapStateStore::new(db, "me@example.com");
        a = store
            .resolve_message_identity("INBOX", 1, 1, &inputs(Some(10)))
            .unwrap();
        b = store
            .resolve_message_identity("INBOX", 1, 2, &inputs(Some(11)))
            .unwrap();
        locate(&store, "INBOX", 1, 1, &a);
        locate(&store, "INBOX", 1, 2, &b);
        assert!(store
            .resolve_message_identity("INBOX", 1, 1, &inputs(Some(11)))
            .is_err());
        assert_eq!(
            store.location_message_id("INBOX", 1, 1).unwrap().as_deref(),
            Some(a.as_str())
        );
        assert_eq!(
            store.location_message_id("INBOX", 1, 2).unwrap().as_deref(),
            Some(b.as_str())
        );
    }
    let store = ImapStateStore::new(
        Arc::new(Database::open(&path.path).unwrap()),
        "me@example.com",
    );
    assert!(
        store
            .resolve_message_identity("Sent", 2, 3, &inputs(Some(11)))
            .is_err(),
        "a reconnect must not erase contradictory evidence and start guessing"
    );
}

#[test]
fn changing_a_server_id_in_the_same_namespace_is_a_conflict() {
    let store = ImapStateStore::new(Arc::new(Database::open_memory()), "me@example.com");
    let original = store
        .resolve_message_identity("INBOX", 1, 1, &inputs(Some(10)))
        .unwrap();
    locate(&store, "INBOX", 1, 1, &original);
    assert!(store
        .resolve_message_identity("INBOX", 1, 1, &inputs(Some(99)))
        .is_err());
    assert!(store
        .resolve_message_identity("Archive", 2, 9, &inputs(Some(99)))
        .is_err());
}

#[test]
fn server_identity_tokens_are_bounded_and_case_sensitive() {
    let store = ImapStateStore::new(Arc::new(Database::open_memory()), "me@example.com");
    for size in [
        policy::MAX_EXTERNAL_ID_BYTES - 1,
        policy::MAX_EXTERNAL_ID_BYTES,
        policy::MAX_EXTERNAL_ID_BYTES + 1,
    ] {
        let input = IdentityInputs {
            email_id: Some("a".repeat(size)),
            ..Default::default()
        };
        assert_eq!(
            store
                .resolve_message_identity("INBOX", 1, size as i64, &input)
                .is_ok(),
            size <= policy::MAX_EXTERNAL_ID_BYTES
        );
    }
    let object = |id: &str| IdentityInputs {
        email_id: Some(id.into()),
        ..Default::default()
    };
    assert_ne!(
        store
            .resolve_message_identity("INBOX", 1, 3, &object("Case"))
            .unwrap(),
        store
            .resolve_message_identity("INBOX", 1, 4, &object("case"))
            .unwrap()
    );
    assert!(store
        .resolve_message_identity("INBOX", 1, 5, &object("bad\r\nid"))
        .is_err());
}

#[test]
fn removing_an_account_clears_only_its_identity_reservations() {
    let db = Arc::new(Database::open_memory());
    let a = ImapStateStore::new(db.clone(), "me@example.com");
    let b = ImapStateStore::new(db.clone(), "other@example.com");
    a.resolve_message_identity("INBOX", 1, 1, &inputs(Some(10)))
        .unwrap();
    b.resolve_message_identity("INBOX", 1, 1, &inputs(Some(10)))
        .unwrap();
    db.remove_account("me@example.com").unwrap();
    let accounts: Vec<String> = db
        .test_with_connection(|c| {
            Ok(
                c.prepare("SELECT DISTINCT account_id FROM imap_message_identities")?
                    .query_map([], |row| row.get(0))?
                    .collect::<Result<_, _>>()?,
            )
        })
        .unwrap();
    assert_eq!(accounts, vec!["other@example.com"]);
}

#[test]
fn a_database_failure_rolls_back_all_identity_reservations() {
    let db = Arc::new(Database::open_memory());
    let store = ImapStateStore::new(db.clone(), "me@example.com");
    db.test_with_connection(|c| {
        c.execute_batch(
            "CREATE TEMP TRIGGER reject_header_identity
            BEFORE INSERT ON imap_message_identities WHEN NEW.namespace='header-hash'
            BEGIN SELECT RAISE(ABORT, 'injected write failure'); END;",
        )?;
        Ok(())
    })
    .unwrap();
    assert!(store
        .resolve_message_identity("INBOX", 1, 1, &inputs(Some(10)))
        .is_err());
    db.test_with_connection(|c| {
        let count: i64 = c.query_row("SELECT COUNT(*) FROM imap_message_identities", [], |r| {
            r.get(0)
        })?;
        assert_eq!(
            count, 0,
            "failure after the strong-key insert must roll it back too"
        );
        c.execute_batch("DROP TRIGGER reject_header_identity")?;
        Ok(())
    })
    .unwrap();
    store
        .resolve_message_identity("INBOX", 1, 1, &inputs(Some(10)))
        .unwrap();
}

#[test]
fn concurrent_reservations_of_the_same_server_id_choose_one_message() {
    let db = Arc::new(Database::open_memory());
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let workers: Vec<_> = ["INBOX", "Sent"]
        .into_iter()
        .map(|mailbox| {
            let store = ImapStateStore::new(db.clone(), "me@example.com");
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                store
                    .resolve_message_identity(mailbox, 1, 1, &inputs(Some(123)))
                    .unwrap()
            })
        })
        .collect();
    let ids: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert_eq!(ids[0], ids[1]);
}

#[test]
fn fresh_devices_assign_the_same_ids_even_when_duplicate_headers_arrive_in_reverse_order() {
    let a = ImapStateStore::new(Arc::new(Database::open_memory()), "me@example.com");
    let b = ImapStateStore::new(Arc::new(Database::open_memory()), "me@example.com");
    let a10 = a
        .resolve_message_identity("INBOX", 1, 1, &inputs(Some(10)))
        .unwrap();
    let a11 = a
        .resolve_message_identity("INBOX", 1, 2, &inputs(Some(11)))
        .unwrap();
    let b11 = b
        .resolve_message_identity("Sent", 2, 8, &inputs(Some(11)))
        .unwrap();
    let b10 = b
        .resolve_message_identity("Archive", 3, 9, &inputs(Some(10)))
        .unwrap();
    assert_eq!(a10, b10);
    assert_eq!(a11, b11);
    assert_ne!(a10, a11);
}
