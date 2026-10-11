use super::*;
use crate::db::test_support::database;
use crate::models::ThreadMutation;
use uuid::Uuid;

#[test]
fn adopting_an_account_creates_it_and_rewrites_legacy_default_state() {
    let database = database();
    database
        .mutate_thread(&ThreadMutation::Star {
            thread_id: "welcome".into(),
            value: true,
        })
        .unwrap();

    let account = database.adopt_account("you@gmail.com").unwrap();
    assert_eq!(account.email, "you@gmail.com");
    assert_eq!(account.status, "connected");
    assert_eq!(account.provider, "gmail");
    assert_eq!(account.sort_order, 0);

    let connection = database.connection().unwrap();
    let sync_account_id: String = connection
        .query_row(
            "SELECT account_id FROM sync_state WHERE account_id = 'you@gmail.com'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(sync_account_id, "you@gmail.com");
    let mutation_account_id: String = connection
        .query_row("SELECT account_id FROM mutations", [], |row| row.get(0))
        .unwrap();
    assert_eq!(mutation_account_id, "you@gmail.com");
}

#[test]
fn adopting_the_same_account_twice_does_not_duplicate_it() {
    let database = database();
    database.adopt_account("you@gmail.com").unwrap();
    database.adopt_account("you@gmail.com").unwrap();
    assert_eq!(database.list_accounts().unwrap().len(), 1);
}

#[test]
fn adopting_an_existing_account_discards_a_recreated_default_sync_row() {
    let database = database();
    database.adopt_account("you@gmail.com").unwrap();
    let connection = database.connection().unwrap();
    connection
        .execute(
            "UPDATE sync_state SET cursor = 'account-cursor' WHERE account_id = 'you@gmail.com'",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO sync_state(account_id, cursor) VALUES ('default', 'placeholder-cursor')",
            [],
        )
        .unwrap();
    drop(connection);

    database.adopt_account("you@gmail.com").unwrap();

    let connection = database.connection().unwrap();
    let rows: i64 = connection
        .query_row("SELECT COUNT(*) FROM sync_state", [], |row| row.get(0))
        .unwrap();
    let cursor: String = connection
        .query_row(
            "SELECT cursor FROM sync_state WHERE account_id = 'you@gmail.com'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(rows, 1);
    assert_eq!(cursor, "account-cursor");
}

#[test]
fn reopening_an_account_database_does_not_recreate_default_sync_state() {
    let path = std::env::temp_dir().join(format!("dispatch-{}.sqlite", Uuid::new_v4()));
    {
        let database = Database::open(&path).unwrap();
        database.adopt_account("you@gmail.com").unwrap();
    }

    let reopened = Database::open(&path).unwrap();
    let default_rows: i64 = reopened
        .connection()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM sync_state WHERE account_id = 'default'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(default_rows, 0);
    reopened.adopt_account("you@gmail.com").unwrap();
    drop(reopened);

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
}

#[test]
fn accounts_get_increasing_sort_order_and_rotating_colors() {
    let database = database();
    let first = database.adopt_account("first@gmail.com").unwrap();
    let second = database.adopt_account("second@gmail.com").unwrap();
    assert_eq!(first.sort_order, 0);
    assert_eq!(second.sort_order, 1);
    assert_ne!(first.color, second.color);
}

#[test]
fn removing_the_only_account_leaves_no_accounts_listed() {
    let database = database();
    database.adopt_account("you@gmail.com").unwrap();
    assert_eq!(database.list_accounts().unwrap().len(), 1);
    database.remove_account("you@gmail.com").unwrap();
    assert!(database.list_accounts().unwrap().is_empty());
}

#[test]
fn removing_an_account_deletes_its_split_inboxes_but_not_another_accounts() {
    let database = database();
    database.adopt_account("you@gmail.com").unwrap();
    database.adopt_account("other@gmail.com").unwrap();
    database
        .create_split_inbox("Mine", "domain", "acme.com", "you@gmail.com")
        .unwrap();
    let kept = database
        .create_split_inbox("Theirs", "domain", "acme.com", "other@gmail.com")
        .unwrap();

    database.remove_account("you@gmail.com").unwrap();

    let remaining = database.list_split_inboxes().unwrap();
    assert_eq!(
        remaining.iter().map(|s| &s.id).collect::<Vec<_>>(),
        vec![&kept.id]
    );
}

#[test]
fn removing_an_account_purges_all_its_local_data_but_not_another_accounts() {
    let database = database();
    // `database()` seeds one thread/message/thread_search row under the
    // pre-multi-account 'default' bucket; adopting folds it onto the
    // account under test the same way a real onboarding flow would.
    database.adopt_account("you@gmail.com").unwrap();
    database.adopt_account("other@gmail.com").unwrap();

    let connection = database.connection().unwrap();
    for account in ["you@gmail.com", "other@gmail.com"] {
        connection
            .execute(
                "INSERT INTO mutations(id, account_id, thread_id, kind, payload_json, state, created_at)
                 VALUES (?1, ?1, 'roadmap', 'archive', '{}', 'pending', '2026-03-05T14:15:00Z')",
                [account],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO pinned_contacts(account_id, email, pinned_at)
                 VALUES (?1, 'friend@example.com', '2026-03-05T14:15:00Z')",
                [account],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO sync_recovery(account_id, history_id) VALUES (?1, '123')",
                [account],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO sync_recovery_threads(account_id, provider_thread_id)
                 VALUES (?1, 'thread-1')",
                [account],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO quarantined_messages(account_id, provider_thread_id, message_id, error, created_at)
                 VALUES (?1, 'thread-1', 'message-1', 'blocked', '2026-03-05T14:15:00Z')",
                [account],
            )
            .unwrap();
    }
    drop(connection);

    database.remove_account("you@gmail.com").unwrap();

    let connection = database.connection().unwrap();
    let count = |sql: &str| -> i64 { connection.query_row(sql, [], |row| row.get(0)).unwrap() };
    assert_eq!(
        count("SELECT COUNT(*) FROM threads WHERE account_id = 'you@gmail.com'"),
        0
    );
    assert_eq!(count("SELECT COUNT(*) FROM messages"), 0);
    assert_eq!(count("SELECT COUNT(*) FROM thread_search"), 0);
    assert_eq!(
        count("SELECT COUNT(*) FROM mutations WHERE account_id = 'you@gmail.com'"),
        0
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM sync_state WHERE account_id = 'you@gmail.com'"),
        0
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM pinned_contacts WHERE account_id = 'you@gmail.com'"),
        0
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM sync_recovery WHERE account_id = 'you@gmail.com'"),
        0
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM sync_recovery_threads WHERE account_id = 'you@gmail.com'"),
        0
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM quarantined_messages WHERE account_id = 'you@gmail.com'"),
        0
    );

    assert_eq!(
        count("SELECT COUNT(*) FROM mutations WHERE account_id = 'other@gmail.com'"),
        1
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM sync_state WHERE account_id = 'other@gmail.com'"),
        1
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM pinned_contacts WHERE account_id = 'other@gmail.com'"),
        1
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM sync_recovery WHERE account_id = 'other@gmail.com'"),
        1
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM sync_recovery_threads WHERE account_id = 'other@gmail.com'"),
        1
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM quarantined_messages WHERE account_id = 'other@gmail.com'"),
        1
    );
    drop(connection);

    assert!(database.get_account("you@gmail.com").unwrap().is_none());
    assert!(database.get_account("other@gmail.com").unwrap().is_some());
}

#[test]
fn set_account_color_updates_an_existing_account_and_rejects_an_unknown_one() {
    let database = database();
    database.adopt_account("you@gmail.com").unwrap();
    database
        .set_account_color("you@gmail.com", "#123456")
        .unwrap();
    assert_eq!(
        database
            .get_account("you@gmail.com")
            .unwrap()
            .unwrap()
            .color,
        "#123456"
    );
    assert!(database
        .set_account_color("missing@gmail.com", "#123456")
        .is_err());
}

#[test]
fn sender_display_name_can_be_saved_cleared_and_cannot_inject_headers() {
    let database = database();
    database.adopt_account("you@gmail.com").unwrap();
    database
        .set_account_display_name("you@gmail.com", Some("  Joel Reed  "))
        .unwrap();
    assert_eq!(
        database
            .get_account("you@gmail.com")
            .unwrap()
            .unwrap()
            .display_name
            .as_deref(),
        Some("Joel Reed")
    );

    assert!(database
        .set_account_display_name("you@gmail.com", Some("Joel\r\nBcc: attacker@example.com"))
        .is_err());
    database
        .set_account_display_name("you@gmail.com", Some("  "))
        .unwrap();
    assert_eq!(
        database
            .get_account("you@gmail.com")
            .unwrap()
            .unwrap()
            .display_name,
        None
    );
    assert!(database
        .set_account_display_name("missing@gmail.com", Some("Nobody"))
        .is_err());
}

#[test]
fn primary_account_id_tracks_the_first_account_and_falls_back_to_the_placeholder() {
    let database = database();
    // Before anything is connected the catalog is empty, so the primary
    // is the pre-connect placeholder key.
    assert_eq!(database.primary_account_id(), crate::auth::LEGACY_KEY);

    database.adopt_account("first@gmail.com").unwrap();
    database.adopt_account("second@gmail.com").unwrap();
    assert_eq!(database.primary_account_id(), "first@gmail.com");

    // Derived from sort order rather than insertion order, so reordering
    // accounts in the UI moves the compose default with them.
    database
        .reorder_accounts(&["second@gmail.com".into(), "first@gmail.com".into()])
        .unwrap();
    assert_eq!(database.primary_account_id(), "second@gmail.com");

    // Removing every account returns to the pre-connect placeholder.
    database.remove_account("second@gmail.com").unwrap();
    database.remove_account("first@gmail.com").unwrap();
    assert_eq!(database.primary_account_id(), crate::auth::LEGACY_KEY);
}

#[test]
fn reorder_accounts_updates_sort_order_by_position() {
    let database = database();
    database.adopt_account("first@gmail.com").unwrap();
    database.adopt_account("second@gmail.com").unwrap();
    database
        .reorder_accounts(&["second@gmail.com".into(), "first@gmail.com".into()])
        .unwrap();
    let accounts = database.list_accounts().unwrap();
    assert_eq!(accounts[0].email, "second@gmail.com");
    assert_eq!(accounts[1].email, "first@gmail.com");
}

#[test]
fn account_removal_and_local_disconnect_purge_only_that_accounts_imap_state() {
    use crate::models::MailProviderKind;
    for disconnect in [false, true] {
        let database = database();
        for account in ["remove@example.com", "keep@example.com"] {
            database
                .adopt_mail_account(account, MailProviderKind::Imap)
                .unwrap();
            database.with_connection(|c| {
                c.execute("INSERT INTO imap_account_settings(account_id, imap_host, imap_port, imap_security, imap_username, smtp_host, smtp_port, smtp_security, smtp_username, label_storage)
                    VALUES (?1, 'imap.example.com', 993, 'implicit_tls', ?1, 'smtp.example.com', 465, 'implicit_tls', ?1, 'none')", [account])?;
                c.execute("INSERT INTO imap_bodies VALUES (?1, 'm', X'736563726574', 6, 1)", [account])?;
                c.execute("INSERT INTO imap_locations VALUES (?1, 'INBOX', 1, 1, 'm', '[]', NULL)", [account])?;
                c.execute("INSERT INTO imap_threads VALUES (?1, 'm', 't', 1)", [account])?;
                c.execute("INSERT INTO imap_thread_aliases VALUES (?1, 'old', 't')", [account])?;
                c.execute("INSERT INTO imap_sync_state(account_id, generation, complete_walks) VALUES (?1, 1, 4)", [account])?;
                c.execute("INSERT INTO imap_change_journal VALUES (?1, 1, 't')", [account])?;
                c.execute("INSERT INTO imap_message_tokens VALUES (?1, 'm', '<m@x>')", [account])?;
                // Populate the Slice 5b-2 columns (last_visited_at, emptied_at_walk)
                // so this test proves the purge covers them too (item i).
                c.execute("INSERT INTO imap_mailbox_sync_state(account_id, mailbox, last_exists, last_uidnext, last_sweep_at, backfill_low_uid, last_visited_at)
                    VALUES (?1, 'Trash', 3, 9, 100, NULL, 200)", [account])?;
                c.execute("INSERT INTO imap_hot_threads(account_id, thread_id, emptied_at_walk) VALUES (?1, 't', 2)", [account])?;
                c.execute("INSERT INTO imap_mailboxes(account_id, name, uidvalidity, uidnext, permanent_flags_json, permanent_keywords)
                    VALUES (?1, 'INBOX', 1, 2, '[]', 0)", [account])?;
                Ok(())
            }).unwrap();
        }
        if disconnect {
            database
                .disconnect_account_locally("remove@example.com")
                .unwrap();
            assert_eq!(
                database
                    .get_account("remove@example.com")
                    .unwrap()
                    .unwrap()
                    .status,
                "needs_reauth"
            );
        } else {
            database.remove_account("remove@example.com").unwrap();
            assert!(database
                .get_account("remove@example.com")
                .unwrap()
                .is_none());
        }
        database
            .with_connection(|c| {
                for table in [
                    "imap_bodies",
                    "imap_locations",
                    "imap_mailboxes",
                    "imap_threads",
                    "imap_thread_aliases",
                    "imap_sync_state",
                    "imap_change_journal",
                    "imap_message_tokens",
                    "imap_mailbox_sync_state",
                    "imap_hot_threads",
                    "imap_account_settings",
                ] {
                    for account in ["remove@example.com", "keep@example.com"] {
                        let count: i64 = c.query_row(
                            &format!("SELECT COUNT(*) FROM {table} WHERE account_id = ?1"),
                            [account],
                            |row| row.get(0),
                        )?;
                        let expected = i64::from(
                            account == "keep@example.com"
                                || (disconnect && table == "imap_account_settings"),
                        );
                        assert_eq!(
                            count, expected,
                            "{table}, {account}, disconnect={disconnect}"
                        );
                    }
                }
                Ok(())
            })
            .unwrap();
    }
}
