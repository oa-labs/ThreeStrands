use super::*;
use crate::db::test_support::{clear_seed_threads, database, list_matching, message, TempDbPath};
use rusqlite::Connection;
use uuid::Uuid;

#[test]
fn reclaim_space_returns_every_free_page() {
    let path = std::env::temp_dir().join(format!("dispatch-{}.sqlite", Uuid::new_v4()));
    {
        let database = Database::open(&path).unwrap();
        database.vacuum_to_incremental().unwrap();
        let connection = database.connection().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE reclaim_probe (body BLOB);
                 WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 200)
                 INSERT INTO reclaim_probe SELECT zeroblob(16384) FROM n;
                 DROP TABLE reclaim_probe;",
            )
            .unwrap();
        let free_pages = |connection: &Connection| -> i64 {
            connection
                .query_row("PRAGMA freelist_count", [], |row| row.get(0))
                .unwrap()
        };
        assert!(free_pages(&connection) > 100);
        drop(connection);

        database.reclaim_space().unwrap();

        assert_eq!(free_pages(&database.connection().unwrap()), 0);
    }
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
}

#[test]
fn compress_next_body_batch_converts_legacy_rows_and_drains_to_zero() {
    let database = database();
    clear_seed_threads(&database);
    database
        .upsert_thread(
            "work@example.com",
            &[message("m1", "t1", "2026-01-01T00:00:00Z", "placeholder")],
        )
        .unwrap();
    database
        .connection()
        .unwrap()
        .execute(
            "UPDATE messages SET body_html = 'Legacy <b>html</b>', body_text = 'Legacy text',
                body_html_z = NULL, body_text_z = NULL
             WHERE id = 'm1'",
            [],
        )
        .unwrap();
    let converted = database.compress_next_body_batch(500).unwrap();
    assert_eq!(converted, 1);
    assert_eq!(database.compress_next_body_batch(500).unwrap(), 0);
    let detail = database.get_thread("work@example.com:t1").unwrap();
    assert_eq!(detail.messages[0].body_html, "Legacy <b>html</b>");
    assert_eq!(detail.messages[0].body_text, "Legacy text");
}

#[test]
fn checkpoint_on_exit_truncates_the_wal_so_the_next_open_skips_quick_check() {
    let temp = TempDbPath::new();
    let database = Database::open(&temp.path).unwrap();
    database
        .upsert_thread(
            "work@example.com",
            &[message("m1", "t1", "2026-01-01T00:00:00Z", "body")],
        )
        .unwrap();
    assert!(
        wal_sidecar_nonempty(&temp.path),
        "a write should leave frames in the WAL"
    );
    database.checkpoint_on_exit().unwrap();
    assert!(
        !wal_sidecar_nonempty(&temp.path),
        "an exit checkpoint must leave the WAL empty, or the next launch runs quick_check"
    );
    assert_eq!(
        database
            .get_thread("work@example.com:t1")
            .unwrap()
            .messages
            .len(),
        1
    );
}

#[test]
fn compress_next_metadata_batch_converts_legacy_rows_and_drains_to_zero() {
    let database = database();
    database
        .connection()
        .unwrap()
        .execute_batch(
            "INSERT INTO message_metadata(id, payload) VALUES ('a', '{\"id\":\"a\"}'), ('b', '{\"id\":\"b\"}');",
        )
        .unwrap();
    assert_eq!(database.compress_next_metadata_batch(1).unwrap(), 1);
    assert_eq!(database.compress_next_metadata_batch(10).unwrap(), 1);
    assert_eq!(database.compress_next_metadata_batch(10).unwrap(), 0);
    assert_eq!(
        database.message_metadata("a").unwrap().as_deref(),
        Some("{\"id\":\"a\"}")
    );
    assert_eq!(
        database.message_metadata("b").unwrap().as_deref(),
        Some("{\"id\":\"b\"}")
    );
}

#[test]
fn prune_orphaned_message_metadata_removes_payloads_whose_message_is_gone() {
    let database = database();
    database
        .upsert_thread(
            "work@example.com",
            &[
                message("m1", "t1", "2026-01-01T00:00:00Z", "body"),
                message("m2", "t1", "2026-01-02T00:00:00Z", "body"),
            ],
        )
        .unwrap();
    // A resync that no longer includes m2 replaces the thread's messages.
    database
        .upsert_thread(
            "work@example.com",
            &[message("m1", "t1", "2026-01-01T00:00:00Z", "body")],
        )
        .unwrap();
    database.put_message_metadata("never-synced", "{}").unwrap();
    assert_eq!(database.prune_orphaned_message_metadata().unwrap(), 2);
    assert!(database.message_metadata("m1").unwrap().is_some());
    assert!(database.message_metadata("m2").unwrap().is_none());
    assert_eq!(database.prune_orphaned_message_metadata().unwrap(), 0);
}

#[test]
fn pre_migration_backup_creates_a_versioned_snapshot_and_prunes_old_ones() {
    let temp = TempDbPath::new();
    // Opening already takes its own v0000 snapshot (this schema starts
    // at version 0); use a version range well clear of that so this
    // test's own rotation assertion isn't affected by it.
    let database = Database::open(&temp.path).unwrap();
    let connection = database.connection().unwrap();

    for version in 100..105 {
        pre_migration_backup(&connection, &temp.path, version).unwrap();
    }
    drop(connection);

    assert_eq!(
        list_matching(&temp.dir, ".pre-migration-v").len(),
        PRE_MIGRATION_BACKUPS_KEPT,
        "only the most recent PRE_MIGRATION_BACKUPS_KEPT snapshots should survive"
    );
    assert!(
        !list_matching(&temp.dir, "pre-migration-v0104").is_empty(),
        "the highest injected version should be the one kept"
    );
}

#[test]
fn backup_and_checkpoint_do_not_wait_on_the_shared_connection() {
    let temp = TempDbPath::new();
    let database = std::sync::Arc::new(Database::open(&temp.path).unwrap());
    database
        .upsert_thread(
            "work@example.com",
            &[message("m1", "keep-me", "2026-01-01T00:00:00Z", "body")],
        )
        .unwrap();

    // Hold the shared connection the way a long sync batch or UI read
    // would, then run maintenance on another thread.
    let guard = database.connection().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let maintenance_db = database.clone();
    std::thread::spawn(move || {
        let result = maintenance_db
            .checkpoint_wal()
            .and_then(|_| maintenance_db.create_periodic_backup());
        let _ = sender.send(result.map_err(|error| error.to_string()));
    });
    let outcome = receiver.recv_timeout(std::time::Duration::from_secs(10));
    drop(guard);

    outcome
        .expect("maintenance blocked on the shared connection mutex")
        .unwrap();
    assert_eq!(list_matching(&temp.dir, ".backup-").len(), 1);
}

#[test]
fn periodic_backup_round_trips_data_and_prunes_old_snapshots() {
    let temp = TempDbPath::new();
    let database = Database::open(&temp.path).unwrap();
    database
        .upsert_thread(
            "work@example.com",
            &[message("m1", "keep-me", "2026-01-01T00:00:00Z", "body")],
        )
        .unwrap();

    for _ in 0..(PERIODIC_BACKUPS_KEPT + 2) {
        database.create_periodic_backup().unwrap();
        // Keep consecutive snapshot filenames (timestamp-based) from
        // colliding within the same millisecond.
        std::thread::sleep(std::time::Duration::from_millis(2));
    }

    assert_eq!(
        list_matching(&temp.dir, ".backup-").len(),
        PERIODIC_BACKUPS_KEPT
    );

    let latest = latest_periodic_backup(&temp.path).unwrap();
    let restored_path = temp.dir.join("restored.sqlite");
    std::fs::copy(&latest, &restored_path).unwrap();
    let restored = Database::open(&restored_path).unwrap();
    let threads = restored.list_threads(Some("work@example.com")).unwrap();
    assert!(threads.iter().any(|t| t.id == "work@example.com:keep-me"));
}

use crate::db::recovery::wal_sidecar_nonempty;
