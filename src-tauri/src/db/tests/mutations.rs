use super::*;
use crate::db::test_support::{database, message};
use crate::models::{SearchThreadsRequest, ThreadMutation};

#[test]
fn batch_mutations_commit_together() {
    let database = database();
    database
        .mutate_threads(&[
            ThreadMutation::Star {
                thread_id: "welcome".into(),
                value: true,
            },
            ThreadMutation::Star {
                thread_id: "roadmap".into(),
                value: false,
            },
        ])
        .unwrap();
    let threads = database.list_threads(None).unwrap();
    assert!(
        threads
            .iter()
            .find(|thread| thread.id == "welcome")
            .unwrap()
            .starred
    );
    assert!(
        !threads
            .iter()
            .find(|thread| thread.id == "roadmap")
            .unwrap()
            .starred
    );
}

#[test]
fn a_failing_mutation_rolls_back_the_whole_batch() {
    let database = database();
    let starred = |database: &Database, id: &str| {
        database
            .list_threads(None)
            .unwrap()
            .into_iter()
            .find(|thread| thread.id == id)
            .unwrap()
            .starred
    };
    let queued = |database: &Database| -> i64 {
        database
            .connection()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM mutations", [], |row| row.get(0))
            .unwrap()
    };
    let welcome_before = starred(&database, "welcome");
    let roadmap_before = starred(&database, "roadmap");
    let queued_before = queued(&database);

    let error = database
        .mutate_threads(&[
            ThreadMutation::Star {
                thread_id: "welcome".into(),
                value: !welcome_before,
            },
            ThreadMutation::Star {
                thread_id: "roadmap".into(),
                value: !roadmap_before,
            },
            ThreadMutation::Spam {
                thread_id: "no-such-thread".into(),
                value: true,
            },
        ])
        .unwrap_err();

    assert!(matches!(error, DatabaseError::NotFound("Thread")), "{error}");
    assert_eq!(error.to_string(), "Thread not found");
    assert_eq!(
        starred(&database, "welcome"),
        welcome_before,
        "an earlier mutation leaked out of the failed batch"
    );
    assert_eq!(
        starred(&database, "roadmap"),
        roadmap_before,
        "an earlier mutation leaked out of the failed batch"
    );
    assert_eq!(
        queued(&database),
        queued_before,
        "a failed batch must not queue provider mutations"
    );
}

#[test]
fn trashing_a_thread_hides_it_from_the_inbox_and_search() {
    let database = database();
    database
        .mutate_thread(&ThreadMutation::Trash {
            thread_id: "welcome".into(),
            value: true,
        })
        .unwrap();
    assert!(!database
        .list_threads(None)
        .unwrap()
        .iter()
        .any(|thread| thread.id == "welcome"));

    let hidden = database
        .search_threads(
            &SearchThreadsRequest {
                query: "keyboard".into(),
                limit: None,
                offset: None,
                include_archived: None,
            },
            None,
        )
        .unwrap();
    assert!(hidden.is_empty());

    let shown = database
        .search_threads(
            &SearchThreadsRequest {
                query: "keyboard".into(),
                limit: None,
                offset: None,
                include_archived: Some(true),
            },
            None,
        )
        .unwrap();
    assert!(shown[0].trashed);
}

#[test]
fn mutation_is_optimistic_and_durable() {
    let database = database();
    let mutation = ThreadMutation::Archive {
        thread_id: "welcome".into(),
        value: true,
    };
    database.mutate_thread(&mutation).unwrap();
    database.mutate_thread(&mutation).unwrap();
    assert!(!database
        .list_threads(None)
        .unwrap()
        .iter()
        .any(|thread| thread.id == "welcome"));
    assert_eq!(
        database.sync_status("default").unwrap().pending_mutations,
        1
    );
}

#[test]
fn claim_mutations_only_claims_the_given_accounts_mutations() {
    let database = database();
    database
        .upsert_thread(
            "work@example.com",
            &[message("m1", "t1", "2026-01-01T00:00:00Z", "body")],
        )
        .unwrap();
    database
        .upsert_thread(
            "personal@example.com",
            &[message("m2", "t2", "2026-01-01T00:00:00Z", "body")],
        )
        .unwrap();
    database
        .mutate_thread(&ThreadMutation::Star {
            thread_id: "work@example.com:t1".into(),
            value: true,
        })
        .unwrap();
    database
        .mutate_thread(&ThreadMutation::Star {
            thread_id: "personal@example.com:t2".into(),
            value: true,
        })
        .unwrap();

    let claimed = database.claim_mutations("work@example.com", 10).unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].provider_thread_id, "t1");
}

#[test]
fn claim_mutations_fails_rather_than_orphans_a_mutation_whose_thread_is_gone() {
    let database = database();
    database
        .upsert_thread(
            "work@example.com",
            &[message("m1", "t1", "2026-01-01T00:00:00Z", "body")],
        )
        .unwrap();
    database
        .mutate_thread(&ThreadMutation::Star {
            thread_id: "work@example.com:t1".into(),
            value: true,
        })
        .unwrap();
    // Simulate the thread row disappearing out from under a still-pending
    // mutation (e.g. the thread left Gmail entirely, or — historically —
    // a full resync wiped it).
    database
        .connection()
        .unwrap()
        .execute("DELETE FROM threads WHERE id = 'work@example.com:t1'", [])
        .unwrap();

    let claimed = database.claim_mutations("work@example.com", 10).unwrap();
    assert!(claimed.is_empty(), "orphaned mutation must not be claimed");

    let (state, error): (String, Option<String>) = database
        .connection()
        .unwrap()
        .query_row(
            "SELECT state, last_error FROM mutations WHERE account_id = 'work@example.com'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(state, "failed");
    assert!(error.unwrap().contains("no longer exists"));
}
