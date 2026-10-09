//! Shared seeded/empty database fixtures and isolated filesystem paths.

use super::{recovery::insert_demo, Database};
use crate::mime::NormalizedMessage;
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub(super) fn database() -> Database {
    let database = Database::open_memory();
    let connection = database.connection().unwrap();
    let transaction = connection.unchecked_transaction().unwrap();
    insert_demo(
        &transaction,
        "roadmap",
        "Phase 1: read and triage",
        "The first vertical slice includes local search and optimistic actions.",
        "Product Team",
        "2026-03-05T14:15:00Z",
        false,
        true,
        "<p>The first read-and-triage vertical slice is now running from local SQLite.</p>",
    )
    .unwrap();
    transaction.commit().unwrap();
    drop(connection);
    database
}

/// Removes `database()`'s seeded demo thread so a test can assert exact
/// counts (e.g. retention pruning) without the seed data participating.
pub(super) fn clear_seed_threads(database: &Database) {
    let connection = database.connection().unwrap();
    connection.execute("DELETE FROM thread_search", []).unwrap();
    connection.execute("DELETE FROM threads", []).unwrap();
}

pub(super) fn message(id: &str, thread_id: &str, date: &str, body: &str) -> NormalizedMessage {
    NormalizedMessage {
        id: id.into(),
        thread_id: thread_id.into(),
        subject: "Subject".into(),
        from: "sender@example.com".into(),
        to: vec!["recipient@example.com".into()],
        date: date.into(),
        body_html: String::new(),
        body_text: body.into(),
        snippet: body.into(),
        labels: vec!["INBOX".into()],
        metadata_json: "{}".into(),
        unsubscribe: None,
        attachments: vec![],
    }
}

/// A database path in its own temporary directory. Dropping it removes
/// the directory, so SQLite's `-wal`/`-shm` side files go with it.
pub(crate) struct TempDbPath {
    pub(super) dir: PathBuf,
    pub(crate) path: PathBuf,
}

impl TempDbPath {
    pub(crate) fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("threestrands-db-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.sqlite");
        Self { dir, path }
    }
}

impl Drop for TempDbPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub(super) fn corrupt_header(path: &Path) {
    use std::io::{Seek, SeekFrom, Write};
    let mut file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    file.write_all(&[0u8; 16]).unwrap();
}

pub(super) fn list_matching(dir: &Path, needle: &str) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| name.contains(needle))
        .collect();
    names.sort();
    names
}

/// Read every stored value in a trusted test table to detect partial writes.
pub(super) fn table_rows(database: &Database, table: &str) -> Vec<Vec<rusqlite::types::Value>> {
    let connection = database.connection().unwrap();
    let mut statement = connection
        .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
        .unwrap();
    let columns = statement.column_count();
    statement
        .query_map([], |row| {
            (0..columns)
                .map(|column| row.get(column))
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}
