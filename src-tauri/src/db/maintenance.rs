//! File snapshots, WAL checkpoints, space reclamation, and storage backfills.
//! Snapshot/checkpoint operations keep their dedicated maintenance connection.

use super::messages::{compress_body, store_message_metadata};
use super::{Database, DbResult};
use chrono::Utc;
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

impl Database {
    /// Checkpoints and truncates the WAL file back down. Called from the
    /// periodic maintenance loop so a long-running session doesn't leave an
    /// ever-growing `-wal` file between the automatic checkpoints SQLite
    /// already performs on its own.
    ///
    /// Runs on a dedicated maintenance connection when the database has a
    /// file, so copying WAL pages back never holds the shared connection
    /// mutex that UI reads wait on. Under WAL, readers keep working
    /// throughout; writers only wait for the checkpoint's final truncate.
    pub fn checkpoint_wal(&self) -> DbResult<()> {
        let checkpoint = |connection: &Connection| -> DbResult<()> {
            Ok(connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?)
        };
        match &self.path {
            Some(path) => checkpoint(&maintenance_connection(path)?),
            None => self.with_connection(checkpoint),
        }
    }

    /// Checkpoints and truncates the WAL as the app exits. The process ends
    /// without closing the shared connection, so SQLite never does this on
    /// its own, and a non-empty WAL at the next launch makes `open` pay for a
    /// full `quick_check`. Best effort and briefly bounded: a busy database
    /// only means the next launch runs that check.
    pub fn checkpoint_on_exit(&self) -> DbResult<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let connection = Connection::open(path)?;
        connection.busy_timeout(std::time::Duration::from_millis(500))?;
        Ok(connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?)
    }

    /// Snapshots the database to a rotating sibling file via `VACUUM INTO`,
    /// pruning older snapshots beyond the retention window. A no-op for the
    /// in-memory test database, which has no path to snapshot alongside.
    ///
    /// `VACUUM INTO` copies the whole file, so it reads from its own WAL
    /// snapshot on a dedicated connection instead of holding the shared
    /// connection mutex (and with it every UI read) for the full copy.
    pub fn create_periodic_backup(&self) -> DbResult<()> {
        let Some(path) = self.path.as_deref() else {
            return Ok(());
        };
        periodic_backup(&maintenance_connection(path)?, path)
    }

    /// Returns freed pages to the OS. Cheap as long as `auto_vacuum` is
    /// already `INCREMENTAL` (see `vacuum_to_incremental`); otherwise a
    /// harmless no-op.
    ///
    /// SQLite frees one page per step of `incremental_vacuum`, so the
    /// statement must be stepped to completion; `execute_batch` steps once.
    pub fn reclaim_space(&self) -> DbResult<()> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare("PRAGMA incremental_vacuum")?;
            let mut rows = statement.query([])?;
            while rows.next()?.is_some() {}
            Ok(())
        })
    }

    /// Whether the database still needs the one-time conversion to
    /// incremental auto-vacuum mode.
    pub fn needs_vacuum_upgrade(&self) -> DbResult<bool> {
        let mode: i64 = self.with_connection(|connection| {
            Ok(connection.query_row("PRAGMA auto_vacuum", [], |r| r.get(0))?)
        })?;
        Ok(mode != 2)
    }

    /// One-time conversion to incremental auto-vacuum. Rewrites the entire
    /// file (like `VACUUM`), so it can be slow on a large existing database
    /// — call this off the async runtime's blocking pool, not inline at
    /// startup. Must not run inside a transaction.
    pub fn vacuum_to_incremental(&self) -> DbResult<()> {
        self.with_connection(|connection| {
            Ok(connection.execute_batch("PRAGMA auto_vacuum = INCREMENTAL; VACUUM;")?)
        })
    }

    /// Compresses a bounded batch of message bodies still on the legacy
    /// plaintext columns (written before the body-compression migration)
    /// into `body_html_z`/`body_text_z`, clearing the plaintext columns as
    /// it goes so the freed space is reclaimable. Returns the number of rows
    /// converted; call repeatedly (e.g. from a background loop) until it
    /// returns 0.
    pub fn compress_next_body_batch(&self, batch_size: usize) -> DbResult<usize> {
        self.with_transaction(|transaction| {
            let rows: Vec<(String, String, String)> = {
                let mut statement = transaction.prepare(
                    "SELECT id, body_html, body_text FROM messages
                         WHERE body_html_z IS NULL LIMIT ?1",
                )?;
                let collected = statement
                    .query_map(params![batch_size as i64], |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                collected
            };
            let count = rows.len();
            for (id, body_html, body_text) in rows {
                transaction.execute(
                    "UPDATE messages SET body_html = '', body_text = '',
                            body_html_z = ?1, body_text_z = ?2
                         WHERE id = ?3",
                    params![compress_body(&body_html), compress_body(&body_text), id],
                )?;
            }
            Ok(count)
        })
    }

    /// Like [`Self::compress_next_body_batch`] for raw provider payloads
    /// stored before `message_metadata.payload_z` existed. Payloads can be
    /// large, so callers use a smaller batch than for bodies.
    pub fn compress_next_metadata_batch(&self, batch_size: usize) -> DbResult<usize> {
        self.with_transaction(|transaction| {
            let rows: Vec<(String, String)> = {
                let mut statement = transaction.prepare(
                    "SELECT id, payload FROM message_metadata WHERE payload_z IS NULL LIMIT ?1",
                )?;
                let collected = statement
                    .query_map(params![batch_size as i64], |row| {
                        Ok((row.get(0)?, row.get(1)?))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                collected
            };
            for (id, payload) in &rows {
                store_message_metadata(transaction, id, payload)?;
            }
            Ok(rows.len())
        })
    }

    /// Deletes raw payloads whose message is gone. Replacing or deleting a
    /// thread cascades to `messages` but not to `message_metadata`.
    pub fn prune_orphaned_message_metadata(&self) -> DbResult<usize> {
        self.with_connection(|connection| {
            Ok(connection.execute(
                "DELETE FROM message_metadata
                 WHERE NOT EXISTS (SELECT 1 FROM messages m WHERE m.id = message_metadata.id)",
                [],
            )?)
        })
    }
}

pub(super) fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

pub(super) fn path_string(path: &Path) -> String {
    path.display().to_string()
}

/// Opens a short-lived connection for maintenance that must not hold the
/// shared connection mutex (backups, WAL checkpoints). WAL mode is a
/// persistent property of the file, so this connection shares it without
/// re-running the schema; the busy timeout matches the primary connection.
fn maintenance_connection(path: &Path) -> DbResult<Connection> {
    let connection = Connection::open(path)?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(connection)
}

fn vacuum_into(connection: &Connection, dest: &Path) -> DbResult<()> {
    connection.execute("VACUUM INTO ?1", params![path_string(dest)])?;
    Ok(())
}

/// Deletes every file under `dir` whose name starts with `prefix` beyond the
/// `keep` most recent (names are chosen so lexicographic order is
/// chronological order — see [`pre_migration_backup`]/[`periodic_backup`]).
fn prune_backups(dir: &Path, prefix: &str, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with(prefix))
        .collect();
    names.sort();
    if names.len() > keep {
        for name in &names[..names.len() - keep] {
            let _ = std::fs::remove_file(dir.join(name));
        }
    }
}

/// Snapshots the database via `VACUUM INTO` before a migration actually runs,
/// so a bad migration (or the hardware issue that caused it) leaves a
/// pre-migration copy behind rather than only the mid-upgrade result. Best
/// effort: the caller does not treat a failure here as fatal, since the
/// migration transaction's own atomicity is the real safety net.
pub(super) fn pre_migration_backup(
    connection: &Connection,
    db_path: &Path,
    old_version: i64,
) -> DbResult<()> {
    let dir = db_path.parent().unwrap_or_else(|| Path::new("."));
    let stem = db_path.file_name().unwrap_or_default().to_string_lossy();
    let prefix = format!("{stem}.pre-migration-v");
    // Zero-padded so lexicographic sort (used by `prune_backups`) matches
    // numeric/chronological order across single- and multi-digit versions.
    let dest = dir.join(format!("{prefix}{old_version:04}.bak"));
    vacuum_into(connection, &dest)?;
    prune_backups(dir, &prefix, PRE_MIGRATION_BACKUPS_KEPT);
    Ok(())
}

fn backup_prefix(db_path: &Path) -> String {
    format!(
        "{}.backup-",
        db_path.file_name().unwrap_or_default().to_string_lossy()
    )
}

/// Snapshots the database via `VACUUM INTO` to a timestamped sibling file,
/// then prunes older snapshots. The timestamp format is fixed-width, so
/// lexicographic filename order is chronological order.
fn periodic_backup(connection: &Connection, db_path: &Path) -> DbResult<()> {
    let dir = db_path.parent().unwrap_or_else(|| Path::new("."));
    let prefix = backup_prefix(db_path);
    let dest = dir.join(format!(
        "{prefix}{}",
        Utc::now().format("%Y%m%dT%H%M%S%.3fZ")
    ));
    vacuum_into(connection, &dest)?;
    prune_backups(dir, &prefix, PERIODIC_BACKUPS_KEPT);
    Ok(())
}

pub(super) fn latest_periodic_backup(db_path: &Path) -> Option<PathBuf> {
    let dir = db_path.parent().unwrap_or_else(|| Path::new("."));
    let prefix = backup_prefix(db_path);
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with(&prefix))
        .collect();
    names.sort();
    names.pop().map(|name| dir.join(name))
}

#[cfg(test)]
#[path = "tests/maintenance.rs"]
mod tests;

// Full-file snapshots retain only a few recent copies.
const PRE_MIGRATION_BACKUPS_KEPT: usize = 1;
const PERIODIC_BACKUPS_KEPT: usize = 3;
