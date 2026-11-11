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
fn the_catalogued_spelling_of_an_address_is_found_without_regard_to_case() {
    let database = database();
    database.adopt_account("Joel@Proton.me").unwrap();
    assert_eq!(
        database
            .catalogued_account_email("joel@proton.me")
            .unwrap()
            .as_deref(),
        Some("Joel@Proton.me")
    );
    assert_eq!(
        database
            .catalogued_account_email("JOEL@PROTON.ME")
            .unwrap()
            .as_deref(),
        Some("Joel@Proton.me")
    );
    assert_eq!(
        database
            .catalogued_account_email("someone@proton.me")
            .unwrap(),
        None
    );

    // Two rows differing only in case: the exact spelling wins.
    database.adopt_account("joel@proton.me").unwrap();
    assert_eq!(
        database
            .catalogued_account_email("joel@proton.me")
            .unwrap()
            .as_deref(),
        Some("joel@proton.me")
    );
    assert_eq!(
        database
            .catalogued_account_email("Joel@Proton.me")
            .unwrap()
            .as_deref(),
        Some("Joel@Proton.me")
    );
}

#[test]
fn account_label_model_reflects_provider_and_imap_label_storage() {
    use crate::models::MailProviderKind;
    use crate::provider::LabelModel;

    let database = std::sync::Arc::new(database());

    // Gmail accounts carry the many-to-many model, with no IMAP settings row.
    database.adopt_account("gmail@example.com").unwrap();
    assert_eq!(
        database
            .get_account("gmail@example.com")
            .unwrap()
            .unwrap()
            .label_model,
        LabelModel::GmailLabels
    );

    // An IMAP account's model comes from the label_storage it saved at setup.
    for (account, storage, expected) in [
        (
            "folders@example.com",
            "folders",
            LabelModel::ImapLabelFolders,
        ),
        ("keywords@example.com", "keywords", LabelModel::ImapKeywords),
        ("none@example.com", "none", LabelModel::ImapNoUserLabels),
    ] {
        database
            .adopt_mail_account(account, MailProviderKind::Imap)
            .unwrap();
        database
            .with_connection(|c| {
                c.execute(
                    "INSERT INTO imap_account_settings(account_id, imap_host, imap_port, imap_security, imap_username, smtp_host, smtp_port, smtp_security, smtp_username, label_storage)
                     VALUES (?1, 'imap.example.com', 993, 'implicit_tls', ?1, 'smtp.example.com', 465, 'implicit_tls', ?1, ?2)",
                    rusqlite::params![account, storage],
                )?;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            database.get_account(account).unwrap().unwrap().label_model,
            expected,
            "label_model for storage {storage}"
        );
    }

    // An IMAP account with no settings row yet (set up on another device)
    // falls back to the default rather than erroring.
    database
        .adopt_mail_account("bare-imap@example.com", MailProviderKind::Imap)
        .unwrap();
    assert_eq!(
        database
            .get_account("bare-imap@example.com")
            .unwrap()
            .unwrap()
            .label_model,
        LabelModel::GmailLabels
    );

    // list_accounts enriches the model the same way get_account does.
    let folders = database
        .list_accounts()
        .unwrap()
        .into_iter()
        .find(|account| account.email == "folders@example.com")
        .unwrap();
    assert_eq!(folders.label_model, LabelModel::ImapLabelFolders);

    let native =
        crate::provider::imap::ImapStateStore::new(database.clone(), "folders@example.com");
    native.set_native_labels(true).unwrap();
    assert_eq!(
        database
            .get_account("folders@example.com")
            .unwrap()
            .unwrap()
            .label_model,
        LabelModel::ImapNativeLabels
    );
    assert!(database
        .list_accounts()
        .unwrap()
        .iter()
        .any(|account| account.email == "folders@example.com"
            && account.label_model == LabelModel::ImapNativeLabels));
    assert_eq!(
        database
            .get_account("keywords@example.com")
            .unwrap()
            .unwrap()
            .label_model,
        LabelModel::ImapKeywords
    );
    native.set_native_labels(false).unwrap();
    assert_eq!(
        database
            .get_account("folders@example.com")
            .unwrap()
            .unwrap()
            .label_model,
        LabelModel::ImapNoUserLabels
    );
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

/// One received conversation and one outgoing message for `account`, each
/// with the rows that refer to it by message, thread, or JSON payload
/// rather than an account column. Every value carries `key`, so a test can
/// count what is left of each account. `payload_account` is how drafts and
/// other JSON records spell the address.
fn seed_account_mail(
    connection: &rusqlite::Connection,
    account: &str,
    payload_account: &str,
    key: &str,
) {
    let thread = format!("{key}-thread");
    let message = format!("{key}-message");
    let subject = format!("{key} subject");
    connection
        .execute(
            "INSERT INTO threads(id, provider_thread_id, subject, snippet, participants_json,
                 last_message_at, unread, starred, archived, labels_json, trashed, account_id,
                 has_attachments, last_received_at)
             VALUES (?1, ?1, ?2, 'snippet', '[]', '2026-03-05T14:15:00Z', 0, 0, 0, '[]', 0, ?3,
                 0, '2026-03-05T14:15:00Z')",
            rusqlite::params![thread, subject, account],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO messages(id, thread_id, sender, recipients_json, sent_at, body_html, body_text)
             VALUES (?1, ?2, 'a@example.com', '[]', '2026-03-05T14:15:00Z', '', 'body')",
            [&message, &thread],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO message_metadata(id, payload) VALUES (?1, ?2)",
            [&message, &format!("{{\"raw\":\"{key}\"}}")],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO unsubscribe_requests(id, message_id, thread_id, method, state, created_at)
             VALUES (?1, ?2, ?3, 'http', 'pending', '2026-03-05T14:15:00Z')",
            [&format!("{key}-unsubscribe"), &message, &thread],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO ai_activity(at, feature, provider, model, subject, message_count,
                 input_tokens, output_tokens)
             VALUES ('2026-03-05T14:15:00Z', 'summary', 'test', 'model', ?1, 1, 10, 5)",
            [&subject],
        )
        .unwrap();

    let draft = format!("{key}-draft");
    let draft_payload = serde_json::json!({"id": draft, "account": payload_account}).to_string();
    connection
        .execute(
            "INSERT INTO drafts(id, revision, payload) VALUES (?1, 1, ?2)",
            [&draft, &draft_payload],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO discarded_drafts(id, revision, payload, discarded_at) VALUES (?1, 1, ?2, 0)",
            [&format!("{key}-discarded"), &draft_payload],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO draft_server_links(draft_id, account, state) VALUES (?1, ?2, 'saved')",
            [&draft, payload_account],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO draft_server_deletions(account, message_id, due_at) VALUES (?1, ?2, 0)",
            [payload_account, &format!("{key}-server-draft")],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO outbox_messages(id, draft_id, revision, account, state, deadline, payload, raw)
             VALUES (?1, ?2, 1, ?3, 'scheduled', 0, ?4, ?5)",
            rusqlite::params![
                format!("{key}-outbox"),
                draft,
                payload_account,
                draft_payload,
                format!("Subject: {key}").into_bytes()
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO signature_defaults(id, payload) VALUES (?1, ?2)",
            [
                &format!("{key}-signature-defaults"),
                &serde_json::json!({"account": payload_account, "address": ""}).to_string(),
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO scheduled_send_summaries(id, report) VALUES (?1, ?2)",
            [
                &format!("{key}-summary"),
                &serde_json::json!({"account": payload_account, "subject": subject}).to_string(),
            ],
        )
        .unwrap();
}

/// Rows left in each table that mention `key`, in a fixed table order.
fn rows_mentioning(connection: &rusqlite::Connection, key: &str) -> Vec<(&'static str, i64)> {
    let like = format!("%{key}%");
    [
        ("threads", "SELECT COUNT(*) FROM threads WHERE id LIKE ?1"),
        ("messages", "SELECT COUNT(*) FROM messages WHERE id LIKE ?1"),
        (
            "message_metadata",
            "SELECT COUNT(*) FROM message_metadata WHERE id LIKE ?1",
        ),
        (
            "unsubscribe_requests",
            "SELECT COUNT(*) FROM unsubscribe_requests WHERE id LIKE ?1",
        ),
        (
            "ai_activity",
            "SELECT COUNT(*) FROM ai_activity WHERE subject LIKE ?1",
        ),
        ("drafts", "SELECT COUNT(*) FROM drafts WHERE id LIKE ?1"),
        (
            "discarded_drafts",
            "SELECT COUNT(*) FROM discarded_drafts WHERE id LIKE ?1",
        ),
        (
            "draft_server_links",
            "SELECT COUNT(*) FROM draft_server_links WHERE draft_id LIKE ?1",
        ),
        (
            "draft_server_deletions",
            "SELECT COUNT(*) FROM draft_server_deletions WHERE message_id LIKE ?1",
        ),
        (
            "outbox_messages",
            "SELECT COUNT(*) FROM outbox_messages WHERE id LIKE ?1",
        ),
        (
            "signature_defaults",
            "SELECT COUNT(*) FROM signature_defaults WHERE id LIKE ?1",
        ),
        (
            "scheduled_send_summaries",
            "SELECT COUNT(*) FROM scheduled_send_summaries WHERE id LIKE ?1",
        ),
    ]
    .into_iter()
    .map(|(table, sql)| {
        (
            table,
            connection
                .query_row(sql, [&like], |row| row.get(0))
                .unwrap(),
        )
    })
    .collect()
}

#[test]
fn removing_an_account_purges_its_outgoing_mail_and_rows_linked_by_message_or_payload() {
    let database = database();
    database.adopt_account("you@gmail.com").unwrap();
    database.adopt_account("other@gmail.com").unwrap();
    let connection = database.connection().unwrap();
    // Payloads spell the removed address in another case: an address
    // matches without regard to case, as the account catalog does.
    seed_account_mail(&connection, "you@gmail.com", "You@Gmail.com", "removed");
    seed_account_mail(&connection, "other@gmail.com", "other@gmail.com", "kept");
    drop(connection);

    database.remove_account("you@gmail.com").unwrap();

    let connection = database.connection().unwrap();
    let removed = rows_mentioning(&connection, "removed");
    assert!(
        removed.iter().all(|(_, count)| *count == 0),
        "left behind: {removed:?}"
    );
    let kept = rows_mentioning(&connection, "kept");
    assert!(
        kept.iter().all(|(_, count)| *count == 1),
        "another account lost: {kept:?}"
    );
    // The usage record stays, without saying what the request was about.
    let activity: i64 = connection
        .query_row("SELECT COUNT(*) FROM ai_activity", [], |row| row.get(0))
        .unwrap();
    assert_eq!(activity, 2);
}

#[test]
fn disconnecting_an_account_locally_drops_mail_derived_rows_but_keeps_its_drafts() {
    let database = database();
    database.adopt_account("you@gmail.com").unwrap();
    let connection = database.connection().unwrap();
    seed_account_mail(&connection, "you@gmail.com", "you@gmail.com", "local");
    drop(connection);

    database
        .disconnect_account_locally("you@gmail.com")
        .unwrap();

    let connection = database.connection().unwrap();
    let left: std::collections::HashMap<_, _> =
        rows_mentioning(&connection, "local").into_iter().collect();
    for table in [
        "threads",
        "messages",
        "message_metadata",
        "unsubscribe_requests",
        "ai_activity",
    ] {
        assert_eq!(
            left[table], 0,
            "{table} kept mail from a disconnected account"
        );
    }
    // Drafts and scheduled sends are the user's own work, kept for when the
    // account is set up on this device again.
    for table in ["drafts", "outbox_messages", "signature_defaults"] {
        assert_eq!(left[table], 1, "{table} lost the user's work");
    }
}

/// Every table with an account column must be named here, so a table added
/// later is checked against account removal rather than silently keeping a
/// removed account's data.
#[test]
fn every_table_keyed_by_account_is_purged_on_removal_or_deliberately_kept() {
    // Deleted by `remove_account`, directly or by a cascading foreign key.
    const PURGED: &[&str] = &[
        "account_labels",
        "agent_tasks",
        "co_recipient_addresses",
        "contact_interactions",
        "conversation_snoozes",
        "draft_server_deletions",
        "draft_server_links",
        "gmail_sending_identities",
        "goals",
        "imap_account_settings",
        "imap_bodies",
        "imap_change_journal",
        "imap_copy_intents",
        "imap_hot_threads",
        "imap_label_snapshots",
        "imap_locations",
        "imap_mailbox_sync_state",
        "imap_mailboxes",
        "imap_message_identities",
        "imap_message_tokens",
        "imap_search_pins",
        "imap_sent_copy_detection",
        "imap_server_capabilities",
        "imap_sync_state",
        "imap_thread_aliases",
        "imap_threads",
        "mutations",
        "outbox_messages",
        "pinned_contacts",
        "quarantined_messages",
        "sender_delivery_patterns",
        "sender_rule_actions",
        "sender_rules",
        "split_inboxes",
        "sync_recovery",
        "sync_recovery_threads",
        "sync_state",
        "tasks",
        "threads",
        "triage_events",
    ];
    // Not removed with a mail account, and why.
    const KEPT: &[(&str, &str)] = &[
        (
            "calendar_selections",
            "a calendar connection, removed when that calendar is disconnected",
        ),
        (
            "note_event_cache",
            "a calendar connection's events, removed with that connection",
        ),
        (
            "sync_transports",
            "the cross-device sync storage account, not a mail account",
        ),
    ];
    let removal_source = include_str!("../accounts.rs");

    let database = database();
    let connection = database.connection().unwrap();
    let mut statement = connection
        .prepare(
            "SELECT DISTINCT m.name FROM sqlite_master m, pragma_table_info(m.name) c
              WHERE m.type = 'table' AND c.name IN ('account_id', 'account', 'account_email')
              ORDER BY m.name",
        )
        .unwrap();
    let keyed: Vec<String> = statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let unclassified: Vec<&String> = keyed
        .iter()
        .filter(|table| {
            !PURGED.contains(&table.as_str()) && !KEPT.iter().any(|(kept, _)| kept == table)
        })
        .collect();
    assert!(
        unclassified.is_empty(),
        "decide whether removing an account deletes these tables' rows: {unclassified:?}"
    );
    // A table listed as purged must actually be: either its rows cascade
    // from a deleted thread, message, or account, or the removal code
    // names it.
    for table in PURGED {
        let cascades: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_list(?1)
                  WHERE \"table\" IN ('threads', 'messages', 'accounts')
                    AND on_delete = 'CASCADE')",
                [table],
                |row| row.get(0),
            )
            .unwrap();
        let named = removal_source.contains(&format!("\"{table}\""))
            || removal_source.contains(&format!("DELETE FROM {table} "));
        assert!(
            cascades || named,
            "{table} is listed as purged but nothing deletes it"
        );
    }
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
fn account_nicknames_are_optional_validated_and_independent_of_sender_names() {
    let database = database();
    assert!(database
        .adopt_account("you@gmail.com")
        .unwrap()
        .nickname
        .is_none());
    database
        .set_account_display_name("you@gmail.com", Some("Joel Reed"))
        .unwrap();
    database
        .set_account_nickname("you@gmail.com", Some("  Work  "))
        .unwrap();
    let account = database.get_account("you@gmail.com").unwrap().unwrap();
    assert_eq!(account.nickname.as_deref(), Some("Work"));
    assert_eq!(account.display_name.as_deref(), Some("Joel Reed"));
    // Reconnecting preserves the private label.
    assert_eq!(
        database
            .adopt_account("you@gmail.com")
            .unwrap()
            .nickname
            .as_deref(),
        Some("Work")
    );
    for length in [199, 200] {
        database
            .set_account_nickname("you@gmail.com", Some(&"é".repeat(length)))
            .unwrap();
    }
    for invalid in [
        "é".repeat(201),
        "Work\nPersonal".into(),
        "Work\u{0085}Personal".into(),
    ] {
        assert!(database
            .set_account_nickname("you@gmail.com", Some(&invalid))
            .is_err());
    }
    assert_eq!(
        database.list_accounts().unwrap()[0].nickname.as_deref(),
        Some("é".repeat(200).as_str())
    );
    for empty in [Some("   "), None] {
        database
            .set_account_nickname("you@gmail.com", empty)
            .unwrap();
        assert!(database
            .get_account("you@gmail.com")
            .unwrap()
            .unwrap()
            .nickname
            .is_none());
    }
    assert!(database
        .set_account_nickname("missing@gmail.com", Some("Work"))
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

fn seed_imap_cache(database: &Database, account: &str) {
    database
        .with_connection(|c| {
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
            c.execute("INSERT INTO imap_copy_intents VALUES (?1, 'm', 'Labels/A', 'identity')", [account])?;
            c.execute("INSERT INTO imap_mailboxes(account_id, name, uidvalidity, uidnext, permanent_flags_json, permanent_keywords)
                VALUES (?1, 'INBOX', 1, 2, '[]', 0)", [account])?;
            // The local label-catalog cache (run 2b-1) is purged by the
            // same per-account path; seed a row so this test proves it.
            c.execute("INSERT INTO account_labels(account_id, label_id, name, kind)
                VALUES (?1, 'Label_18', 'Work', 'user')", [account])?;
            Ok(())
        })
        .unwrap();
}

#[test]
fn connection_changes_invalidate_only_the_changed_mailbox_namespace() {
    use crate::provider::imap::{Identity, ImapStateStore, SecurityMode};
    use std::sync::Arc;
    for change in [
        "host",
        "port",
        "username",
        "smtp_host",
        "smtp_port",
        "smtp_username",
        "tls",
        "identity",
    ] {
        let db = Arc::new(database());
        let account = "you@example.com";
        db.adopt_mail_account(account, crate::models::MailProviderKind::Imap)
            .unwrap();
        db.adopt_mail_account("keep@example.com", crate::models::MailProviderKind::Imap)
            .unwrap();
        for email in [account, "keep@example.com"] {
            seed_imap_cache(&db, email);
            let store = ImapStateStore::new(db.clone(), email);
            store.upsert_pin("INBOX", 1, 1, 100).unwrap();
            store.set_sent_copy_detection(true, 100).unwrap();
        }
        db.with_connection(|c| {
            c.execute("UPDATE sync_state SET cursor='old-cursor' WHERE account_id=?1", [account])?;
            c.execute("INSERT INTO mutations(id,account_id,thread_id,kind,payload_json,state,created_at)
                VALUES ('pending',?1,'roadmap','archive','{}','pending','2026-10-12T00:00:00Z'),
                       ('running',?1,'roadmap','star','{}','running','2026-10-12T00:00:00Z')", [account])?;
            c.execute("INSERT INTO drafts VALUES ('draft',1,'{}')", [])?;
            c.execute("INSERT INTO pinned_contacts(account_id,email,pinned_at) VALUES (?1,'friend@example.com','2026-10-12')", [account])?;
            Ok(())
        }).unwrap();
        let mut settings = db.imap_account_settings(account).unwrap().unwrap();
        match change {
            "host" => settings.imap_host = "new.example.com".into(),
            "port" => settings.imap_port = 1143,
            "username" => settings.imap_username = "another-login".into(),
            "smtp_host" => settings.smtp_host = "relay.example.com".into(),
            "smtp_port" => settings.smtp_port = 587,
            "smtp_username" => settings.smtp_username = "another-smtp-login".into(),
            "tls" => settings.imap_security = SecurityMode::StartTls,
            "identity" => settings.identities.push(Identity {
                address: "alias@example.com".into(),
                display_name: None,
            }),
            _ => unreachable!(),
        }
        db.save_imap_connection_settings(account, &settings)
            .unwrap();
        assert_eq!(
            db.imap_account_settings(account).unwrap().unwrap(),
            settings
        );
        let reset = matches!(change, "host" | "port" | "username");
        let store = ImapStateStore::new(db.clone(), account);
        assert_eq!(
            store.locations_in_mailbox("INBOX").unwrap().is_empty(),
            reset,
            "{change}"
        );
        assert_eq!(store.search_pin_count().unwrap(), usize::from(!reset));
        assert_eq!(
            store
                .pending_copy_identity("m", "Labels/A")
                .unwrap()
                .is_none(),
            reset
        );
        assert_eq!(store.mailboxes().unwrap().is_empty(), reset);
        assert_eq!(store.generation().unwrap(), if reset { 0 } else { 1 });
        let c = db.connection().unwrap();
        for (table, expected) in [
            ("threads", 2 * i64::from(!reset)),
            ("thread_search", 2 * i64::from(!reset)),
            ("drafts", 1),
            ("pinned_contacts", 1),
        ] {
            let count: i64 = c
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, expected, "{change}: {table}");
        }
        let cursor: Option<String> = c
            .query_row(
                "SELECT cursor FROM sync_state WHERE account_id=?1",
                [account],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            cursor.as_deref(),
            if reset { None } else { Some("old-cursor") }
        );
        let failed: i64 = c
            .query_row(
                "SELECT count(*) FROM mutations WHERE state='failed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(failed, if reset { 2 } else { 0 });
        drop(c);
        assert_eq!(
            store.sent_copy_detection().unwrap().is_none(),
            reset || change.starts_with("smtp_")
        );
        let kept = ImapStateStore::new(db.clone(), "keep@example.com");
        assert_eq!(kept.locations_in_mailbox("INBOX").unwrap().len(), 1);
        assert_eq!(kept.search_pin_count().unwrap(), 1);
        assert_eq!(kept.generation().unwrap(), 1);
        assert_eq!(kept.sent_copy_detection().unwrap(), Some(true));
    }
}

#[test]
fn a_failed_connection_settings_write_rolls_back_cache_invalidation() {
    use crate::provider::imap::ImapStateStore;
    use std::sync::Arc;
    let db = Arc::new(database());
    let account = "you@example.com";
    db.adopt_mail_account(account, crate::models::MailProviderKind::Imap)
        .unwrap();
    seed_imap_cache(&db, account);
    let original = db.imap_account_settings(account).unwrap().unwrap();
    let mut changed = original.clone();
    changed.imap_host = "new.example.com".into();
    db.connection()
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER reject_connection BEFORE UPDATE ON imap_account_settings
        BEGIN SELECT RAISE(ABORT,'settings write failed'); END;",
        )
        .unwrap();
    assert!(db.save_imap_connection_settings(account, &changed).is_err());
    assert_eq!(
        db.imap_account_settings(account).unwrap().unwrap(),
        original
    );
    assert_eq!(
        ImapStateStore::new(db.clone(), account)
            .locations_in_mailbox("INBOX")
            .unwrap()
            .len(),
        1
    );
    assert_eq!(db.list_threads(Some(account)).unwrap().len(), 2);
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
            seed_imap_cache(&database, account);
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
                    "imap_copy_intents",
                    "imap_account_settings",
                    "account_labels",
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
