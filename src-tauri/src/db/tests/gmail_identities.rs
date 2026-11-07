use super::*;
use crate::provider::SendingIdentity;
const ACCOUNT: &str = "you@example.com";
fn snapshot() -> Vec<SendingIdentity> {
    vec![
        SendingIdentity {
            address: ACCOUNT.into(),
            ..Default::default()
        },
        SendingIdentity {
            address: "work@example.org".into(),
            display_name: Some("Work".into()),
            reply_to: Some("replies@example.org".into()),
            is_default: true,
        },
    ]
}
fn database() -> Database {
    let db = Database::open_memory();
    db.adopt_account(ACCOUNT).unwrap();
    db
}

#[test]
fn verified_cache_is_offline_readable_account_scoped_and_replaced_on_refresh() {
    let db = database();
    let identities = snapshot();
    assert!(db
        .store_gmail_sending_identities(ACCOUNT, &identities, 1000)
        .unwrap());
    assert_eq!(db.account_sending_identities(ACCOUNT).unwrap(), identities);
    assert_eq!(
        db.account_sending_identities("other@example.com")
            .unwrap()
            .len(),
        1
    );
    let own = db
        .with_connection(|connection| {
            Ok(crate::db::sending_addresses_for_account(
                connection, ACCOUNT,
            ))
        })
        .unwrap();
    assert!(own.contains("work@example.org"));
    assert!(!db
        .store_gmail_sending_identities(ACCOUNT, &identities, 2000)
        .unwrap());
    assert!(db
        .store_gmail_sending_identities(ACCOUNT, &identities[..1], 3000)
        .unwrap());
    assert_eq!(db.account_sending_identities(ACCOUNT).unwrap().len(), 1);
}

#[test]
fn cache_refresh_boundary_and_invalid_refresh_never_overwrite_good_addresses() {
    let db = database();
    db.store_gmail_sending_identities(ACCOUNT, &snapshot(), 1000)
        .unwrap();
    for delta in [
        REFRESH_INTERVAL_MS - 1,
        REFRESH_INTERVAL_MS,
        REFRESH_INTERVAL_MS + 1,
    ] {
        assert_eq!(
            db.gmail_identities_refresh_due(ACCOUNT, 1000 + delta)
                .unwrap(),
            delta >= REFRESH_INTERVAL_MS
        );
    }
    for address in ["bad\r\nBcc: victim@example.com", "other@example.com"] {
        let mut bad = snapshot();
        bad[0].address = address.into();
        assert!(db
            .store_gmail_sending_identities(ACCOUNT, &bad, 5000)
            .is_err());
        assert_eq!(db.account_sending_identities(ACCOUNT).unwrap(), snapshot());
    }
}

#[test]
fn cached_alias_selects_default_and_signatures_but_unknown_or_removed_from_is_rejected() {
    let db = database();
    let identities = snapshot();
    db.store_gmail_sending_identities(ACCOUNT, &identities, 1000)
        .unwrap();
    db.store_signature(&crate::signatures::tests::template("work"))
        .unwrap();
    db.store_signature_defaults(&crate::signatures::tests::defaults(
        ACCOUNT,
        "work@example.org",
        Some("work"),
        Some("work"),
    ))
    .unwrap();
    let draft = db.create_draft("new", None, ACCOUNT, &identities).unwrap();
    assert_eq!(draft.from.as_deref(), Some("work@example.org"));
    assert_eq!(draft.signature.unwrap().text, "work");
    assert!(db
        .set_draft_from(&draft.id, Some("stranger@example.com"))
        .is_err());
    db.store_gmail_sending_identities(ACCOUNT, &identities[..1], 2000)
        .unwrap();
    assert!(db
        .set_draft_from(&draft.id, Some("work@example.org"))
        .is_err());
    assert_eq!(db.set_draft_from(&draft.id, None).unwrap().from, None);
}

#[test]
fn sent_alias_is_classified_as_own_only_in_its_account() {
    let db = database();
    db.store_gmail_sending_identities(ACCOUNT, &snapshot(), 1000)
        .unwrap();
    let mut message = crate::db::test_support::message(
        "alias-mail",
        "alias-thread",
        "2026-01-01T09:00:00Z",
        "hello",
    );
    message.from = "Work <work@example.org>".into();
    message.labels = vec!["SENT".into()];
    db.upsert_thread(ACCOUNT, &[message]).unwrap();
    let thread = db.get_thread(&format!("{ACCOUNT}:alias-thread")).unwrap();
    assert_eq!(thread.messages[0].sent_by_account.as_deref(), Some(ACCOUNT));
}

#[test]
fn aliases_are_not_a_settings_transfer_permission_and_are_cleared_with_account() {
    let db = database();
    db.store_gmail_sending_identities(ACCOUNT, &snapshot(), 1000)
        .unwrap();
    let imap = Database::open_memory();
    imap.adopt_mail_account("imap@example.org", crate::models::MailProviderKind::Imap)
        .unwrap();
    assert!(imap
        .store_gmail_sending_identities("imap@example.org", &snapshot(), 1000)
        .is_err());
    db.remove_account(ACCOUNT).unwrap();
    assert_eq!(db.account_sending_identities(ACCOUNT).unwrap().len(), 1);
}

#[test]
fn an_unreadable_metadata_cache_is_rebuildable_and_cannot_block_primary_sending() {
    let db = database();
    db.store_gmail_sending_identities(ACCOUNT, &snapshot(), 1000)
        .unwrap();
    db.with_connection(|connection| {
        connection.execute(
            "UPDATE gmail_sending_identities SET identities_json='invalid' WHERE account_id=?1",
            [ACCOUNT],
        )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        db.account_sending_identities(ACCOUNT).unwrap()[0].address,
        ACCOUNT
    );
    assert!(db.gmail_identities_refresh_due(ACCOUNT, 1001).unwrap());
    db.store_gmail_sending_identities(ACCOUNT, &snapshot(), 2000)
        .unwrap();
    assert_eq!(db.account_sending_identities(ACCOUNT).unwrap(), snapshot());
}

#[test]
fn a_failed_refresh_backs_off_instead_of_repeating_every_sync() {
    let db = database();
    let at = 1_000_000;
    assert!(
        db.gmail_identities_refresh_due(ACCOUNT, at).unwrap(),
        "nothing cached yet"
    );

    db.record_gmail_identities_refresh_failure(ACCOUNT, at)
        .unwrap();
    assert!(!db
        .gmail_identities_refresh_due(ACCOUNT, at + 2 * 60_000 - 1)
        .unwrap());
    assert!(db
        .gmail_identities_refresh_due(ACCOUNT, at + 2 * 60_000)
        .unwrap());

    // Each failure in a row doubles the wait, up to the refresh interval.
    let mut failed_at = at;
    for minutes in [4, 8, 15, 15] {
        failed_at += 60 * 60_000;
        db.record_gmail_identities_refresh_failure(ACCOUNT, failed_at)
            .unwrap();
        assert!(!db
            .gmail_identities_refresh_due(ACCOUNT, failed_at + minutes * 60_000 - 1)
            .unwrap());
        assert!(db
            .gmail_identities_refresh_due(ACCOUNT, failed_at + minutes * 60_000)
            .unwrap());
    }
    // A successful refresh ends the backoff and the ordinary interval resumes.
    db.store_gmail_sending_identities(ACCOUNT, &snapshot(), failed_at + 1)
        .unwrap();
    assert!(db
        .gmail_identity_refresh_failures
        .lock()
        .unwrap()
        .is_empty());
    assert!(!db
        .gmail_identities_refresh_due(ACCOUNT, failed_at + 2)
        .unwrap());
}

#[test]
fn a_failure_on_one_account_does_not_delay_another() {
    let db = database();
    db.adopt_account("other@example.com").unwrap();
    db.record_gmail_identities_refresh_failure(ACCOUNT, 1_000)
        .unwrap();
    assert!(db
        .gmail_identities_refresh_due("other@example.com", 1_001)
        .unwrap());
}
