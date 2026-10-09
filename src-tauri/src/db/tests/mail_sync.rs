use crate::db::test_support::{database, message};

#[test]
fn clear_cursor_forgets_a_previously_finished_sync_cursor() {
    let database = database();
    database.finish_sync("default", "old-cursor").unwrap();
    assert_eq!(
        database.cursor("default").unwrap().as_deref(),
        Some("old-cursor")
    );
    database.clear_cursor("default").unwrap();
    assert_eq!(database.cursor("default").unwrap(), None);
}

#[test]
fn clear_cursor_never_deletes_any_accounts_threads() {
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
    database.clear_cursor("work@example.com").unwrap();
    let threads = database.list_threads(None).unwrap();
    assert!(threads.iter().any(|t| t.id == "work@example.com:t1"));
    assert!(threads.iter().any(|t| t.id == "personal@example.com:t2"));
}

#[test]
fn sync_state_is_isolated_per_account() {
    let database = database();
    database.adopt_account("work@example.com").unwrap();
    database.adopt_account("personal@example.com").unwrap();
    database
        .finish_sync("work@example.com", "work-cursor")
        .unwrap();
    assert_eq!(
        database.cursor("work@example.com").unwrap().as_deref(),
        Some("work-cursor")
    );
    assert_eq!(database.cursor("personal@example.com").unwrap(), None);
}

#[test]
fn finish_sync_stamps_the_account_last_synced_at() {
    let database = database();
    database.adopt_account("work@example.com").unwrap();
    database.adopt_account("personal@example.com").unwrap();
    database
        .finish_sync("work@example.com", "work-cursor")
        .unwrap();
    let accounts = database.list_accounts().unwrap();
    let work = accounts
        .iter()
        .find(|account| account.email == "work@example.com")
        .unwrap();
    assert!(work.last_synced_at.is_some());
    let personal = accounts
        .iter()
        .find(|account| account.email == "personal@example.com")
        .unwrap();
    assert_eq!(personal.last_synced_at, None);
}
