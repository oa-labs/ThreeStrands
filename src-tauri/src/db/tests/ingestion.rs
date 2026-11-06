use super::*;
use crate::db::test_support::{database, message};
use crate::models::ThreadMutation;

#[test]
fn a_provider_copy_fetched_before_an_undelivered_mutation_does_not_undo_it() {
    let database = database();
    let account = "work@example.com";
    let thread_id = "work@example.com:raced";
    let mut server_copy = message("m1", "raced", "2026-01-01T00:00:00Z", "body");
    server_copy.labels = vec!["INBOX".into(), "UNREAD".into(), "Label_keep".into()];
    database
        .upsert_thread(account, &[server_copy.clone()])
        .unwrap();
    let flags = |database: &Database| {
        let thread = database
            .list_all_mail(Some(account))
            .unwrap()
            .into_iter()
            .find(|thread| thread.id == thread_id)
            .unwrap();
        (
            thread.archived,
            thread.unread,
            thread.starred,
            thread.labels.contains(&"Label_keep".to_string()),
        )
    };

    // The user archives, reads, stars and unlabels the thread while a
    // sync holds a copy fetched before any of that.
    for mutation in [
        ThreadMutation::Archive {
            thread_id: thread_id.into(),
            value: true,
        },
        ThreadMutation::Read {
            thread_id: thread_id.into(),
            value: true,
        },
        ThreadMutation::Star {
            thread_id: thread_id.into(),
            value: true,
        },
        ThreadMutation::Label {
            thread_id: thread_id.into(),
            label_id: "Label_keep".into(),
            value: false,
        },
    ] {
        database.mutate_thread(&mutation).unwrap();
    }
    database
        .apply_ingested_threads(
            account,
            &[("raced".into(), vec![server_copy.clone()], vec![])],
        )
        .unwrap();
    assert_eq!(flags(&database), (true, false, true, false));

    // In flight to the provider still counts as undelivered.
    database
        .connection()
        .unwrap()
        .execute("UPDATE mutations SET state = 'running'", [])
        .unwrap();
    database
        .upsert_thread(account, &[server_copy.clone()])
        .unwrap();
    assert_eq!(flags(&database), (true, false, true, false));

    // Once delivered (or abandoned), the provider's copy is authoritative.
    database
        .connection()
        .unwrap()
        .execute("UPDATE mutations SET state = 'done'", [])
        .unwrap();
    database.upsert_thread(account, &[server_copy]).unwrap();
    assert_eq!(flags(&database), (false, true, false, true));
}

#[test]
fn conversation_metadata_comes_from_root_and_unread_comes_from_latest_message() {
    let database = database();
    let mut root = message(
        "root-message",
        "metadata-thread",
        "2026-01-01T00:00:00Z",
        "root",
    );
    root.labels = vec![
        "INBOX".into(),
        "STARRED".into(),
        "Label_root".into(),
        "IMPORTANT".into(),
    ];
    let mut latest = message(
        "latest-message",
        "metadata-thread",
        "2026-01-02T00:00:00Z",
        "latest",
    );
    latest.labels = vec!["INBOX".into(), "UNREAD".into(), "Label_latest".into()];

    database
        .upsert_thread("work@example.com", &[latest.clone(), root.clone()])
        .unwrap();

    let thread = database
        .get_thread("work@example.com:metadata-thread")
        .unwrap()
        .thread;
    assert!(thread.starred);
    assert!(thread.unread);
    assert!(thread.labels.contains(&"Label_root".to_string()));
    assert!(!thread.labels.contains(&"Label_latest".to_string()));
    assert!(
        thread.labels.contains(&"IMPORTANT".to_string()),
        "non-metadata system labels remain conversation-wide"
    );

    root.labels.retain(|label| label != "STARRED");
    latest.labels.push("STARRED".into());
    database
        .upsert_thread("work@example.com", &[root, latest])
        .unwrap();
    assert!(
        !database
            .get_thread("work@example.com:metadata-thread")
            .unwrap()
            .thread
            .starred,
        "a star on a later message does not become the conversation star"
    );
}

#[test]
fn two_accounts_with_the_same_provider_thread_id_stay_fully_separate() {
    let database = database();
    database
        .upsert_thread(
            "work@example.com",
            &[message(
                "work-msg",
                "shared-id",
                "2026-01-01T00:00:00Z",
                "work body",
            )],
        )
        .unwrap();
    database
        .upsert_thread(
            "personal@example.com",
            &[message(
                "personal-msg",
                "shared-id",
                "2026-01-01T00:00:00Z",
                "personal body",
            )],
        )
        .unwrap();

    let threads = database.list_threads(None).unwrap();
    let work = threads
        .iter()
        .find(|t| t.id == "work@example.com:shared-id")
        .unwrap();
    let personal = threads
        .iter()
        .find(|t| t.id == "personal@example.com:shared-id")
        .unwrap();
    assert_eq!(
        database.get_thread(&work.id).unwrap().messages[0].id,
        "work-msg"
    );
    assert_eq!(
        database.get_thread(&personal.id).unwrap().messages[0].id,
        "personal-msg"
    );

    database
        .delete_thread("work@example.com", "shared-id")
        .unwrap();
    let remaining = database.list_threads(None).unwrap();
    assert!(!remaining.iter().any(|t| t.id == work.id));
    assert!(remaining.iter().any(|t| t.id == personal.id));
}

/// Capture persisted values, not just row counts: a failed replacement must
/// restore old bodies, previews, contact history, and optimistic local state.
fn ingestion_rows(database: &Database) -> Vec<Vec<Vec<rusqlite::types::Value>>> {
    [
        "threads",
        "messages",
        "message_metadata",
        "contact_interactions",
        "thread_search",
        "pending_search_reindex",
        "quarantined_messages",
        "mutations",
    ]
    .iter()
    .map(|table| crate::db::test_support::table_rows(database, table))
    .collect()
}

fn assert_ingestion_rolls_back(quarantine_failure: bool) {
    let database = database();
    let account = "work@example.com";
    database.adopt_account(account).unwrap();
    let original = message(
        "original",
        "existing",
        "2026-01-01T00:00:00Z",
        "original body",
    );
    database
        .apply_ingested_threads(
            account,
            &[(
                "existing".into(),
                vec![original.clone()],
                vec![("bad-original".into(), "original error".into())],
            )],
        )
        .unwrap();
    database
        .mutate_thread(&ThreadMutation::Archive {
            thread_id: "work@example.com:existing".into(),
            value: true,
        })
        .unwrap();
    database
        .connection()
        .unwrap()
        .execute(
            "INSERT INTO pending_search_reindex(thread_id) VALUES (?1)",
            ["work@example.com:existing"],
        )
        .unwrap();
    let before = ingestion_rows(&database);

    let mut replacement = original;
    replacement.body_text = "replacement body".into();
    replacement.from = "new-sender@example.com".into();
    replacement.metadata_json = r#"{"id":"original","changed":true}"#.into();
    let new_message = message(
        "new-message",
        "new-thread",
        "2026-01-02T00:00:00Z",
        "new body",
    );
    let trigger = if quarantine_failure {
        "CREATE TEMP TRIGGER fail_ingestion BEFORE INSERT ON quarantined_messages
         WHEN NEW.message_id = 'fail'
         BEGIN SELECT RAISE(ABORT, 'late ingestion failure'); END;"
    } else {
        "CREATE TEMP TRIGGER fail_ingestion BEFORE INSERT ON messages
         WHEN NEW.id = 'new-message'
         BEGIN SELECT RAISE(ABORT, 'late ingestion failure'); END;"
    };
    database
        .connection()
        .unwrap()
        .execute_batch(trigger)
        .unwrap();
    let ingest = || {
        if quarantine_failure {
            database.apply_ingested_threads(
                account,
                &[
                    ("existing".into(), vec![replacement.clone()], vec![]),
                    (
                        "new-thread".into(),
                        vec![new_message.clone()],
                        vec![("fail".into(), "new error".into())],
                    ),
                ],
            )
        } else {
            database.upsert_threads(
                account,
                &[vec![replacement.clone()], vec![new_message.clone()]],
            )
        }
    };
    let error = ingest().unwrap_err();
    assert!(matches!(error, crate::db::DatabaseError::Sqlite(_)));
    assert!(error.to_string().contains("late ingestion failure"));
    assert_eq!(
        ingestion_rows(&database),
        before,
        "all ingestion tables must roll back"
    );

    database
        .connection()
        .unwrap()
        .execute_batch("DROP TRIGGER fail_ingestion")
        .unwrap();
    ingest().unwrap();
    let updated = database.get_thread("work@example.com:existing").unwrap();
    assert!(
        updated.thread.archived,
        "successful ingestion must replay the queued archive"
    );
    assert_eq!(updated.messages[0].body_text, "replacement body");
    assert!(database.get_thread("work@example.com:new-thread").is_ok());
}

#[test]
fn a_late_quarantine_failure_rolls_back_the_entire_ingestion_batch() {
    assert_ingestion_rolls_back(true);
}

#[test]
fn a_late_message_failure_rolls_back_the_entire_upsert_batch() {
    assert_ingestion_rolls_back(false);
}

#[test]
fn ingesting_a_merge_reparents_tasks_and_rolls_back_on_failure() {
    let database = database();
    let account = "merge@example.com";
    let a = message("a", "survivor", "2026-01-01T00:00:00Z", "root");
    let b = message("b", "old", "2026-01-02T00:00:00Z", "other root");
    database
        .upsert_threads(account, &[vec![a.clone()], vec![b.clone()]])
        .unwrap();
    database.with_connection(|c| {
        c.execute("INSERT INTO tasks(id, account_id, thread_id, source_message_id, title, kind, due_kind, status, created_at, updated_at)
            VALUES ('task', ?1, ?2, 'b', 'Keep this task', 'action', 'none', 'open', 'now', 'now')",
            params![account, local_thread_id(account, "old")])?;
        Ok(())
    }).unwrap();
    let mut moved = b;
    moved.thread_id = "survivor".into();
    let aliases = vec![("old".into(), "survivor".into())];
    // A duplicate message forces the ingestion transaction to fail after it
    // attempted alias cleanup. Both roots and the task must remain intact.
    assert!(database
        .apply_ingested_threads_with_aliases(
            account,
            &[(
                "survivor".into(),
                vec![a.clone(), moved.clone(), moved.clone()],
                vec![]
            )],
            &aliases
        )
        .is_err());
    assert_eq!(database.list_all_mail(Some(account)).unwrap().len(), 2);
    let task_thread = || {
        database
            .with_connection(|c| {
                Ok(
                    c.query_row("SELECT thread_id FROM tasks WHERE id = 'task'", [], |row| {
                        row.get::<_, String>(0)
                    })?,
                )
            })
            .unwrap()
    };
    assert_eq!(task_thread(), local_thread_id(account, "old"));
    assert_eq!(database.get_thread_for_message("b").unwrap().thread.id, local_thread_id(account, "old"));
    database
        .apply_ingested_threads_with_aliases(
            account,
            &[("survivor".into(), vec![a, moved], vec![])],
            &aliases,
        )
        .unwrap();
    assert_eq!(database.list_all_mail(Some(account)).unwrap().len(), 1);
    assert_eq!(
        database
            .get_thread(&local_thread_id(account, "survivor"))
            .unwrap()
            .messages
            .len(),
        2
    );
    assert_eq!(task_thread(), local_thread_id(account, "survivor"));
    let source: String = database.with_connection(|c| {
        Ok(c.query_row("SELECT source_message_id FROM tasks WHERE id='task'", [], |row| row.get(0))?)
    }).unwrap();
    assert_eq!(source, "b", "thread aliases must not replace stable source message IDs");
    assert_eq!(database.get_thread_for_message(&source).unwrap().thread.id, local_thread_id(account, "survivor"));
}

mod quarantine_reports {
    use super::*;

    const ACCOUNT: &str = "long@example.com";
    const OVER: &str = "Conversation is larger than 16 MB";

    fn ingest(database: &Database, thread: &str, set_aside: &[(&str, &str)]) {
        let kept = message(&format!("{thread}-kept"), thread, "2026-01-05T00:00:00Z", "newest");
        let set_aside = set_aside
            .iter()
            .map(|(id, error)| (id.to_string(), error.to_string()))
            .collect();
        database
            .apply_ingested_threads(ACCOUNT, &[(thread.into(), vec![kept], set_aside)])
            .unwrap();
    }

    fn backdate(database: &Database) {
        database
            .connection()
            .unwrap()
            .execute("UPDATE quarantined_messages SET created_at = '2026-01-01T00:00:00+00:00'", [])
            .unwrap();
    }

    #[test]
    fn a_conversation_reports_its_set_aside_messages_as_one_entry() {
        let database = database();
        database.adopt_account(ACCOUNT).unwrap();
        ingest(&database, "long", &[("old-1", OVER), ("old-2", OVER), ("old-3", OVER)]);
        ingest(&database, "other", &[("broken", "Invalid Gmail base64url body")]);

        let mut entries = database.sync_status(ACCOUNT).unwrap().quarantined_messages;
        entries.sort_by(|a, b| a.thread_id.cmp(&b.thread_id));
        let summary: Vec<_> = entries
            .iter()
            .map(|entry| (entry.thread_id.as_str(), entry.message_count, entry.message_id.as_str()))
            .collect();
        assert_eq!(summary, [("long", 3, "old-1"), ("other", 1, "broken")]);
    }

    #[test]
    fn a_resync_with_the_same_messages_set_aside_keeps_when_they_were_first_seen() {
        let database = database();
        database.adopt_account(ACCOUNT).unwrap();
        ingest(&database, "long", &[("old-1", OVER), ("old-2", OVER)]);
        backdate(&database);

        ingest(&database, "long", &[("old-1", OVER), ("old-2", OVER)]);

        let entries = database.sync_status(ACCOUNT).unwrap().quarantined_messages;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].created_at, "2026-01-01T00:00:00+00:00");
        assert_eq!(entries[0].message_count, 2);
    }

    #[test]
    fn a_newly_set_aside_message_updates_the_entry_and_a_kept_one_leaves_it() {
        let database = database();
        database.adopt_account(ACCOUNT).unwrap();
        ingest(&database, "long", &[("old-1", OVER), ("old-2", OVER)]);
        backdate(&database);

        // The conversation grew: old-2 no longer fits, old-3 joins it, and
        // old-1 has since been deleted from the server.
        ingest(&database, "long", &[("old-2", OVER), ("old-3", OVER)]);

        let entries = database.sync_status(ACCOUNT).unwrap().quarantined_messages;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message_count, 2);
        assert_eq!(entries[0].message_id, "old-2");
        assert_ne!(entries[0].created_at, "2026-01-01T00:00:00+00:00");

        ingest(&database, "long", &[]);
        assert!(database.sync_status(ACCOUNT).unwrap().quarantined_messages.is_empty());
    }

    #[test]
    fn a_changed_reason_is_reported_as_new() {
        let database = database();
        database.adopt_account(ACCOUNT).unwrap();
        ingest(&database, "long", &[("old-1", "Invalid Gmail base64url body")]);
        backdate(&database);

        ingest(&database, "long", &[("old-1", OVER)]);

        let entries = database.sync_status(ACCOUNT).unwrap().quarantined_messages;
        assert_eq!(entries[0].error, OVER);
        assert_ne!(entries[0].created_at, "2026-01-01T00:00:00+00:00");
    }
}
