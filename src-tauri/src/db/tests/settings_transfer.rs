use super::*;
use crate::db::test_support::database;
use crate::transfer::{TransferAccount, TransferSnippet, TransferSplitInbox};

#[test]
fn import_transfer_preserves_local_credentials_and_marks_new_accounts_for_connection() {
    let database = database();
    database.adopt_account("connected@example.com").unwrap();
    database
        .create_split_inbox("Old rule", "domain", "old.example", "connected@example.com")
        .unwrap();

    database
        .import_transfer_data(
            &[
                TransferAccount {
                    email: "new@example.com".to_string(),
                    display_name: Some("New account".to_string()),
                    color: "#123456".to_string(),
                    provider: "gmail".to_string(),
                    sort_order: 0,
                },
                TransferAccount {
                    email: "connected@example.com".to_string(),
                    display_name: Some("Connected account".to_string()),
                    color: "#654321".to_string(),
                    provider: "gmail".to_string(),
                    sort_order: 1,
                },
            ],
            &[TransferSplitInbox {
                id: "imported-split".to_string(),
                name: "Imported rule".to_string(),
                match_kind: "pattern".to_string(),
                match_value: "newsletter".to_string(),
                sort_order: 0,
                created_at: "2026-03-06T00:00:00Z".to_string(),
                account_id: "new@example.com".to_string(),
            }],
            &[TransferSnippet {
                id: "imported-snippet".to_string(),
                name: "Imported snippet".to_string(),
                body: "Body".to_string(),
                created_at: "2026-03-06T00:00:00Z".to_string(),
            }],
            &[],
            None,
            Some(90),
        )
        .unwrap();

    assert_eq!(
        database
            .get_account("connected@example.com")
            .unwrap()
            .unwrap()
            .status,
        "connected"
    );
    assert_eq!(
        database
            .get_account("new@example.com")
            .unwrap()
            .unwrap()
            .status,
        "needs_reauth"
    );
    let splits = database.list_split_inboxes().unwrap();
    assert_eq!(splits.len(), 1);
    assert_eq!(splits[0].name, "Imported rule");
    let snippets = database.list_snippets().unwrap();
    assert_eq!(snippets.len(), 1);
    assert_eq!(snippets[0].name, "Imported snippet");
    assert_eq!(database.retention_days().unwrap(), Some(90));
}

#[test]
fn a_late_settings_failure_rolls_back_every_imported_feature() {
    use crate::db::test_support::table_rows;
    use crate::transfer::{TransferContact, TransferContactGroup};

    // Both retention paths run after every other feature has been imported.
    for retention in [Some(90), None] {
        let database = database();
        database.adopt_account("connected@example.com").unwrap();
        database
            .create_split_inbox(
                "Original rule",
                "domain",
                "old.example",
                "connected@example.com",
            )
            .unwrap();
        database
            .create_snippet("Original snippet", "Original body")
            .unwrap();
        database
            .create_contact_group("Original group", &[], &["old@example.com".into()])
            .unwrap();
        database.set_retention_days(Some(30)).unwrap();

        let source = Database::open_memory();
        source.adopt_account("new@example.com").unwrap();
        source.adopt_account("connected@example.com").unwrap();
        source
            .create_split_inbox("Imported rule", "domain", "new.example", "new@example.com")
            .unwrap();
        source
            .create_snippet("Imported snippet", "Imported body")
            .unwrap();
        source
            .create_contact_group("Imported group", &[], &["new-contact@example.com".into()])
            .unwrap();
        let accounts: Vec<TransferAccount> = source
            .list_accounts()
            .unwrap()
            .into_iter()
            .map(Into::into)
            .collect();
        let splits: Vec<TransferSplitInbox> = source
            .list_split_inboxes()
            .unwrap()
            .into_iter()
            .map(Into::into)
            .collect();
        let snippets: Vec<TransferSnippet> = source
            .list_snippets()
            .unwrap()
            .into_iter()
            .map(Into::into)
            .collect();
        let contacts: Vec<TransferContact> = source
            .list_contact_profiles("", 100)
            .unwrap()
            .into_iter()
            .map(Into::into)
            .collect();
        let groups: Vec<TransferContactGroup> = source
            .list_contact_groups()
            .unwrap()
            .into_iter()
            .map(Into::into)
            .collect();
        assert_eq!(contacts.len(), 1);
        assert_eq!(groups.len(), 1);

        let tables = [
            "accounts",
            "split_inboxes",
            "snippets",
            "contacts",
            "contact_addresses",
            "contact_groups",
            "contact_group_members",
            "compose_settings",
        ];
        let before: Vec<_> = tables
            .iter()
            .map(|table| table_rows(&database, table))
            .collect();
        let trigger = if retention.is_some() {
            "CREATE TEMP TRIGGER fail_import BEFORE INSERT ON compose_settings
             WHEN NEW.key = 'retention_days'
             BEGIN SELECT RAISE(ABORT, 'late settings failure'); END;"
        } else {
            "CREATE TEMP TRIGGER fail_import BEFORE DELETE ON compose_settings
             WHEN OLD.key = 'retention_days'
             BEGIN SELECT RAISE(ABORT, 'late settings failure'); END;"
        };
        database
            .connection()
            .unwrap()
            .execute_batch(trigger)
            .unwrap();
        let import = || {
            database.import_transfer_data(
                &accounts,
                &splits,
                &snippets,
                &contacts,
                Some(&groups),
                retention,
            )
        };
        let error = import().unwrap_err();
        assert!(matches!(error, crate::db::DatabaseError::Sqlite(_)));
        assert!(error.to_string().contains("late settings failure"));
        let after: Vec<_> = tables
            .iter()
            .map(|table| table_rows(&database, table))
            .collect();
        assert_eq!(
            after, before,
            "a failed import must preserve all destination records"
        );

        database
            .connection()
            .unwrap()
            .execute_batch("DROP TRIGGER fail_import")
            .unwrap();
        import().unwrap();
        assert_eq!(database.retention_days().unwrap(), retention);
        assert_eq!(
            database.list_snippets().unwrap()[0].name,
            "Imported snippet"
        );
        assert_eq!(
            database.list_contact_groups().unwrap()[0].name,
            "Imported group"
        );
        assert_eq!(
            database
                .get_account("connected@example.com")
                .unwrap()
                .unwrap()
                .status,
            "connected"
        );
        assert_eq!(
            database
                .get_account("new@example.com")
                .unwrap()
                .unwrap()
                .status,
            "needs_reauth"
        );
    }
}
