//! Database startup, initialization, and corruption-only file recovery.

use super::maintenance::{latest_periodic_backup, path_string, pre_migration_backup, sidecar_path};
use super::Database;
use chrono::Utc;
use rusqlite::{params, Connection, ErrorCode, Transaction};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Mutex,
};

/// What [`open_with_recovery`] had to do to get a usable database open.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum RecoveryOutcome {
    /// The database opened normally; no recovery was needed.
    Clean,
    /// The original file failed to open (or failed its post-open integrity
    /// check) and was replaced with the most recent periodic backup.
    RestoredFromBackup {
        corrupt_path: Option<String>,
        backup_path: String,
    },
    /// No usable backup existed, or the restored copy was itself broken, so
    /// a brand-new empty database was created. The caller is expected to
    /// trigger a full resync.
    FreshDatabase { corrupt_path: Option<String> },
}

/// Error from opening or initializing the database. Kept structured (rather
/// than collapsed to a `String`) so [`open_with_recovery`] can tell an
/// actually-corrupt file apart from a transient/environmental failure —
/// see [`OpenError::is_corruption`].
#[derive(Debug)]
pub enum OpenError {
    Sqlite(rusqlite::Error),
    /// `PRAGMA quick_check` ran successfully but reported a problem; the
    /// payload is its diagnostic text.
    IntegrityCheckFailed(String),
    Other(String),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::Sqlite(error) => write!(f, "{error}"),
            OpenError::IntegrityCheckFailed(detail) => {
                write!(f, "Database integrity check failed: {detail}")
            }
            OpenError::Other(message) => write!(f, "{message}"),
        }
    }
}

impl From<rusqlite::Error> for OpenError {
    fn from(error: rusqlite::Error) -> Self {
        OpenError::Sqlite(error)
    }
}

impl From<String> for OpenError {
    fn from(message: String) -> Self {
        OpenError::Other(message)
    }
}

impl OpenError {
    /// Whether this failure means the database file itself is broken, as
    /// opposed to a transient or environmental problem (disk full,
    /// permissions, another process holding a lock, a migration bug, …)
    /// that replacing the file would not fix and could make worse.
    /// [`open_with_recovery`] only quarantines and replaces the file for
    /// this class of error.
    fn is_corruption(&self) -> bool {
        match self {
            OpenError::IntegrityCheckFailed(_) => true,
            OpenError::Sqlite(rusqlite::Error::SqliteFailure(sqlite_error, _)) => matches!(
                sqlite_error.code,
                ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase
            ),
            _ => false,
        }
    }
}

impl Database {
    pub fn open(path: &Path) -> Result<Self, super::OpenError> {
        // Checked before opening: a clean shutdown checkpoints WAL back to
        // (near) zero length, so a non-empty WAL here means the last run
        // didn't shut down cleanly.
        let had_pending_wal = wal_sidecar_nonempty(path);
        let mut connection = Connection::open(path)?;
        restrict_to_owner(path);
        connection.execute_batch(crate::schema::INITIAL_SCHEMA)?;
        let reset_running = connection.execute(
            "UPDATE mutations SET state = 'pending', last_error = 'Interrupted before acknowledgement'
             WHERE state = 'running'",
            [],
        )?;
        let version_before_migration: i64 =
            connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version_before_migration < crate::schema::LATEST_VERSION {
            // Best-effort: a failed snapshot (e.g. a full disk) must not
            // block the migration itself — the migration's own single
            // transaction is the real safety net either way.
            let _ = pre_migration_backup(&connection, path, version_before_migration);
        }
        crate::schema::migrate(&mut connection).map_err(OpenError::Other)?;
        ensure_query_indexes(&connection)?;
        if had_pending_wal || reset_running > 0 {
            run_quick_check(&connection)?;
        }
        seed_if_empty(&connection)?;
        let database = Self {
            connection: Mutex::new(connection),
            path: Some(path.to_path_buf()),
            replicated_sync_projecting: Mutex::new(HashMap::new()),
        };
        // v44 reindexes stored recipients that strict address parsing skipped.
        if version_before_migration < 44 {
            database
                .rebuild_contact_interactions()
                .map_err(|error| OpenError::Other(error.to_string()))?;
        }
        Ok(database)
    }

    #[cfg(test)]
    pub(crate) fn open_memory() -> Self {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(crate::schema::INITIAL_SCHEMA)
            .unwrap();
        crate::schema::migrate(&mut connection).unwrap();
        ensure_query_indexes(&connection).unwrap();
        seed_if_empty(&connection).unwrap();
        let database = Self {
            connection: Mutex::new(connection),
            path: None,
            replicated_sync_projecting: Mutex::new(HashMap::new()),
        };
        database.rebuild_contact_interactions().unwrap();
        database
    }
}

/// Best-effort: narrow the database file to owner-only access. Not fatal if
/// it fails (e.g. an unsupported filesystem) since the containing directory
/// is already locked down by the caller.
#[cfg(unix)]
fn restrict_to_owner(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_to_owner(_path: &Path) {}

pub(super) fn wal_sidecar_nonempty(path: &Path) -> bool {
    std::fs::metadata(sidecar_path(path, "-wal"))
        .map(|metadata| metadata.len() > 0)
        .unwrap_or(false)
}

/// Runs `PRAGMA quick_check` (a cheap, non-exhaustive integrity check, unlike
/// the much slower `integrity_check`). Only called when startup looked
/// suspicious in the first place — see the callers in [`Database::open`].
fn run_quick_check(connection: &Connection) -> Result<(), OpenError> {
    let result: String = connection.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    if result == "ok" {
        Ok(())
    } else {
        Err(OpenError::IntegrityCheckFailed(result))
    }
}

/// Moves a broken database file (and its `-wal`/`-shm` sidecars, if present)
/// aside rather than deleting it, so it remains available for support/
/// diagnosis. Returns the quarantined main file's path.
fn quarantine_broken_database(path: &Path) -> Option<PathBuf> {
    if !path.exists() {
        return None;
    }
    let timestamp = Utc::now().format("%Y%m%dT%H%M%S%.3fZ");
    let suffix = format!(".corrupt-{timestamp}");
    let quarantined = sidecar_path(path, &suffix);
    for extra in ["-wal", "-shm"] {
        let source = sidecar_path(path, extra);
        if source.exists() {
            let _ = std::fs::rename(&source, sidecar_path(&quarantined, extra));
        }
    }
    std::fs::rename(path, &quarantined).ok()?;
    Some(quarantined)
}

/// Opens the database at `path`, recovering from a corrupt file rather than
/// failing to launch at all: retries against the most recent periodic
/// backup, and falls all the way back to a fresh empty database if no usable
/// backup exists. The original file is quarantined (renamed aside, not
/// deleted) at each step so it stays available for diagnosis.
///
/// Only a corruption-class failure (a structured SQLite corruption error, or
/// a failed `quick_check`) triggers this — see [`OpenError::is_corruption`].
/// Anything else (disk full, permissions, a lock held by another process, a
/// migration bug, …) leaves the file untouched and aborts startup instead,
/// since replacing it in that case could discard a perfectly good database.
pub fn open_with_recovery(path: &Path) -> (Database, RecoveryOutcome) {
    let error = match Database::open(path) {
        Ok(database) => return (database, RecoveryOutcome::Clean),
        Err(error) => error,
    };

    if !error.is_corruption() {
        panic!(
            "Unable to open local database at {}: {error}. This does not look like database \
             corruption, so the existing file was left in place rather than replaced — check \
             for a full disk, a permissions problem, or another process holding the database open.",
            path.display()
        );
    }

    let corrupt_path = quarantine_broken_database(path).map(|p| path_string(&p));

    if let Some(backup) = latest_periodic_backup(path) {
        if std::fs::copy(&backup, path).is_ok() {
            match Database::open(path) {
                Ok(database) => {
                    return (
                        database,
                        RecoveryOutcome::RestoredFromBackup {
                            corrupt_path,
                            backup_path: path_string(&backup),
                        },
                    );
                }
                Err(_) => {
                    // The restored copy is itself unusable; discard it and
                    // fall through to a fresh database below.
                    let _ = std::fs::remove_file(path);
                    let _ = std::fs::remove_file(sidecar_path(path, "-wal"));
                    let _ = std::fs::remove_file(sidecar_path(path, "-shm"));
                }
            }
        }
    }

    let database = Database::open(path).unwrap_or_else(|error| {
        panic!("Unable to open local database even starting fresh: {error}")
    });
    (database, RecoveryOutcome::FreshDatabase { corrupt_path })
}

fn ensure_query_indexes(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "CREATE INDEX IF NOT EXISTS threads_account_mailbox_order
         ON threads(account_id, trashed, archived, last_received_at DESC);
         CREATE INDEX IF NOT EXISTS threads_mailbox_order
         ON threads(trashed, archived, last_received_at DESC);
         CREATE INDEX IF NOT EXISTS mutations_account_pending
         ON mutations(account_id, state, created_at);",
    )
}

fn seed_if_empty(connection: &Connection) -> rusqlite::Result<()> {
    let count: i64 = connection.query_row("SELECT count(*) FROM threads", [], |row| row.get(0))?;
    if count != 0 {
        return Ok(());
    }
    let transaction = connection.unchecked_transaction()?;
    insert_demo(
        &transaction,
        "welcome",
        "Welcome to ThreeStrands",
        "A keyboard-first inbox that keeps your mail on this device.",
        "ThreeStrands",
        "2026-03-05T16:30:00Z",
        true,
        false,
        "<p>Welcome to <strong>ThreeStrands</strong>.</p><p>Use <kbd>j</kbd> and <kbd>k</kbd> to move, <kbd>e</kbd> to archive, <kbd>s</kbd> to star, and <kbd>⌘K</kbd> to open the command palette.</p>",
    )?;
    transaction.commit()
}

#[allow(clippy::too_many_arguments)]
pub(super) fn insert_demo(
    transaction: &Transaction<'_>,
    id: &str,
    subject: &str,
    snippet: &str,
    participant: &str,
    sent_at: &str,
    unread: bool,
    starred: bool,
    body: &str,
) -> rusqlite::Result<()> {
    let participants = serde_json::to_string(&[participant]).expect("static data serializes");
    let labels = serde_json::to_string(&["INBOX"]).expect("static data serializes");
    transaction.execute(
        "INSERT INTO threads(
            id, provider_thread_id, subject, snippet, participants_json, last_message_at,
            unread, starred, archived, labels_json, trashed, account_id, summary,
            summary_generated_at, has_attachments, last_received_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9, 0, 'default', NULL, NULL, 0, ?6)",
        params![
            id,
            format!("demo-{id}"),
            subject,
            snippet,
            participants,
            sent_at,
            unread,
            starred,
            labels
        ],
    )?;
    transaction.execute(
        "INSERT INTO messages(
            id, thread_id, sender, recipients_json, sent_at, body_html, body_text, unread
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            format!("{id}-message"),
            id,
            format!("{participant} <hello@threestrands.local>"),
            "[\"You <you@example.com>\"]",
            sent_at,
            body,
            snippet,
            unread,
        ],
    )?;
    transaction.execute(
        "INSERT INTO thread_search(thread_id, subject, snippet, participants, body)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, subject, snippet, participant, snippet],
    )?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/recovery.rs"]
mod tests;
