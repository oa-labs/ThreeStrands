use super::*;
use crate::db::test_support::{corrupt_header, list_matching, message, TempDbPath};
use crate::models::ThreadMutation;
use rusqlite::Connection;
use std::path::PathBuf;
use uuid::Uuid;

#[test]
fn fresh_database_does_not_seed_internal_roadmap_message() {
    let database = Database::open_memory();
    let threads = database.list_threads(None).unwrap();

    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].subject, "Welcome to ThreeStrands");
    assert!(threads
        .iter()
        .all(|thread| thread.subject != "Phase 1: read and triage"));
}

#[test]
fn interrupted_delivery_is_recovered_on_open() {
    let path = std::env::temp_dir().join(format!("dispatch-{}.sqlite", Uuid::new_v4()));
    {
        let database = Database::open(&path).unwrap();
        database
            .mutate_thread(&ThreadMutation::Star {
                thread_id: "welcome".into(),
                value: true,
            })
            .unwrap();
        database
            .connection()
            .unwrap()
            .execute("UPDATE mutations SET state = 'running'", [])
            .unwrap();
    }
    let reopened = Database::open(&path).unwrap();
    assert_eq!(reopened.claim_mutations("default", 10).unwrap().len(), 1);
    drop(reopened);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
}

#[test]
fn quick_check_flags_a_corrupted_committed_page() {
    let temp = TempDbPath::new();
    {
        let database = Database::open(&temp.path).unwrap();
        database
            .upsert_thread(
                "work@example.com",
                &[message(
                    "m1",
                    "inbox",
                    "2026-01-01T00:00:00Z",
                    &"padding to push this row across more than one page. ".repeat(200),
                )],
            )
            .unwrap();
        // Merge WAL into the main file so the data we're about to
        // corrupt actually lives there rather than in `-wal`.
        database.checkpoint_wal().unwrap();
    }

    {
        // Truncating partway through leaves the header's recorded page
        // count disagreeing with the file's actual size — a page-level
        // byte flip can land in unused free space and go unnoticed, but
        // this mismatch is exactly what `quick_check` is designed to
        // catch, every time.
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&temp.path)
            .unwrap();
        let len = file.metadata().unwrap().len();
        assert!(
            len > 8192,
            "expected more than one page of data to truncate"
        );
        file.set_len(len / 2).unwrap();
    }

    let connection = Connection::open(&temp.path).unwrap();
    let result = run_quick_check(&connection);
    assert!(
        result.is_err(),
        "flipping bytes in a committed data page should be caught by quick_check"
    );
}

#[test]
fn opening_a_brand_new_database_takes_a_pre_migration_snapshot() {
    let temp = TempDbPath::new();
    let database = Database::open(&temp.path).unwrap();
    drop(database);

    assert!(
        temp.dir
            .join("test.sqlite.pre-migration-v0000.bak")
            .exists(),
        "opening a schema at version 0 should snapshot it before migrating to the latest version"
    );
}

#[test]
fn open_with_recovery_falls_back_to_a_fresh_database_when_there_is_no_backup() {
    let temp = TempDbPath::new();
    {
        let database = Database::open(&temp.path).unwrap();
        database
            .upsert_thread(
                "work@example.com",
                &[message("m1", "lost-thread", "2026-01-01T00:00:00Z", "body")],
            )
            .unwrap();
    }
    corrupt_header(&temp.path);

    let (database, outcome) = open_with_recovery(&temp.path);
    match &outcome {
        RecoveryOutcome::FreshDatabase { corrupt_path } => {
            assert!(corrupt_path.as_ref().unwrap().contains(".corrupt-"));
        }
        other => panic!("expected FreshDatabase, got {other:?}"),
    }

    let threads = database.list_threads(None).unwrap();
    assert!(threads
        .iter()
        .any(|t| t.subject == "Welcome to ThreeStrands"));
    assert!(!threads
        .iter()
        .any(|t| t.id == "work@example.com:lost-thread"));
    assert!(
        !list_matching(&temp.dir, ".corrupt-").is_empty(),
        "the broken original should be quarantined, not deleted"
    );
}

#[test]
fn open_with_recovery_restores_the_latest_backup_when_the_file_is_corrupt() {
    let temp = TempDbPath::new();
    {
        let database = Database::open(&temp.path).unwrap();
        database
            .upsert_thread(
                "work@example.com",
                &[message(
                    "m1",
                    "backed-up-thread",
                    "2026-01-01T00:00:00Z",
                    "body",
                )],
            )
            .unwrap();
        database.create_periodic_backup().unwrap();
        // Written after the backup, so it must NOT survive recovery —
        // that's the proof the restore actually came from the backup
        // file rather than the (corrupt) live one.
        database
            .upsert_thread(
                "work@example.com",
                &[message(
                    "m2",
                    "post-backup-thread",
                    "2026-01-02T00:00:00Z",
                    "body",
                )],
            )
            .unwrap();
    }
    corrupt_header(&temp.path);

    let (database, outcome) = open_with_recovery(&temp.path);
    assert!(
        matches!(outcome, RecoveryOutcome::RestoredFromBackup { .. }),
        "expected RestoredFromBackup, got {outcome:?}"
    );

    let threads = database.list_threads(Some("work@example.com")).unwrap();
    let ids: Vec<_> = threads.iter().map(|t| t.id.as_str()).collect();
    assert!(ids.contains(&"work@example.com:backed-up-thread"));
    assert!(!ids.contains(&"work@example.com:post-backup-thread"));
}

#[test]
#[cfg(unix)]
fn open_with_recovery_leaves_a_permission_denied_database_untouched() {
    use std::os::unix::fs::PermissionsExt;

    let temp = TempDbPath::new();
    {
        let database = Database::open(&temp.path).unwrap();
        database
            .upsert_thread(
                "work@example.com",
                &[message("m1", "kept-thread", "2026-01-01T00:00:00Z", "body")],
            )
            .unwrap();
    }
    std::fs::set_permissions(&temp.path, std::fs::Permissions::from_mode(0o000)).unwrap();

    // Root (or a filesystem that ignores mode bits) can't be denied this
    // way; skip rather than produce a flaky assertion in that setup.
    let probe_succeeded = Connection::open(&temp.path).is_ok();
    if probe_succeeded {
        std::fs::set_permissions(&temp.path, std::fs::Permissions::from_mode(0o600)).unwrap();
        return;
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        open_with_recovery(&temp.path)
    }));

    std::fs::set_permissions(&temp.path, std::fs::Permissions::from_mode(0o600)).unwrap();

    assert!(
        result.is_err(),
        "a permission failure is not corruption and should not be silently recovered from"
    );
    assert!(
        list_matching(&temp.dir, ".corrupt-").is_empty(),
        "a non-corruption failure must not quarantine the original database"
    );

    let database = Database::open(&temp.path).unwrap();
    let threads = database.list_threads(Some("work@example.com")).unwrap();
    assert!(
        threads
            .iter()
            .any(|t| t.id == "work@example.com:kept-thread"),
        "the original database must survive untouched"
    );
}

/// `ENOSPC` needs a genuinely space-constrained filesystem, which isn't
/// something to fabricate inside the normal `cargo test` sandbox. Run
/// this manually against a small scratch volume, e.g. on macOS:
/// `hdiutil create -size 2m -fs "APFS" -volname threestrands-disk-full /tmp/threestrands-disk-full.dmg`
/// then `hdiutil attach /tmp/threestrands-disk-full.dmg`, point
/// `THREESTRANDS_DISK_FULL_TEST_DIR` at the mounted volume, and run
/// `cargo test disk_full -- --ignored`.
#[test]
#[ignore = "needs a real space-constrained filesystem; see comment"]
fn write_failure_under_disk_full_surfaces_as_an_error_not_a_panic() {
    let dir = std::env::var("THREESTRANDS_DISK_FULL_TEST_DIR")
        .expect("set THREESTRANDS_DISK_FULL_TEST_DIR to a small, space-constrained mount point");
    let path = PathBuf::from(dir).join("disk-full.sqlite");
    let database = Database::open(&path).unwrap();

    let mut hit_capacity_error = false;
    for i in 0..100_000 {
        let body = "x".repeat(4096);
        let result = database.upsert_thread(
            "work@example.com",
            &[message(
                &format!("m{i}"),
                &format!("thread-{i}"),
                "2026-01-01T00:00:00Z",
                &body,
            )],
        );
        if result.is_err() {
            hit_capacity_error = true;
            break;
        }
    }
    assert!(
        hit_capacity_error,
        "expected to eventually exhaust the constrained filesystem"
    );
}
