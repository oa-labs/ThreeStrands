use std::{
    path::Path,
    sync::{Mutex, MutexGuard},
};

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use uuid::Uuid;

use crate::models::{
    Message, SearchThreadsRequest, SyncStatus, Thread, ThreadDetail, ThreadMutation,
};

const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS threads (
    id TEXT PRIMARY KEY,
    provider_thread_id TEXT NOT NULL UNIQUE,
    subject TEXT NOT NULL,
    snippet TEXT NOT NULL,
    participants_json TEXT NOT NULL,
    last_message_at TEXT NOT NULL,
    unread INTEGER NOT NULL DEFAULT 0,
    starred INTEGER NOT NULL DEFAULT 0,
    archived INTEGER NOT NULL DEFAULT 0,
    labels_json TEXT NOT NULL DEFAULT '[]'
);

CREATE INDEX IF NOT EXISTS threads_inbox_order
ON threads(archived, last_message_at DESC);

CREATE TABLE IF NOT EXISTS messages (
    id TEXT PRIMARY KEY,
    thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    sender TEXT NOT NULL,
    recipients_json TEXT NOT NULL,
    sent_at TEXT NOT NULL,
    body_html TEXT NOT NULL,
    body_text TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS messages_by_thread
ON messages(thread_id, sent_at);

CREATE VIRTUAL TABLE IF NOT EXISTS thread_search USING fts5(
    thread_id UNINDEXED,
    subject,
    snippet,
    participants,
    body,
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TABLE IF NOT EXISTS mutations (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('pending', 'running', 'done', 'failed')),
    attempts INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    last_error TEXT
);

CREATE INDEX IF NOT EXISTS mutations_pending
ON mutations(state, created_at);

CREATE TABLE IF NOT EXISTS sync_state (
    account_id TEXT PRIMARY KEY,
    cursor TEXT,
    last_successful_sync TEXT,
    last_error TEXT
);

INSERT OR IGNORE INTO sync_state(account_id) VALUES ('default');
"#;

pub struct Database(Mutex<Connection>);

impl Database {
    pub fn open(path: &Path) -> Result<Self, String> {
        let connection = Connection::open(path).map_err(display_error)?;
        connection.execute_batch(SCHEMA).map_err(display_error)?;
        seed_if_empty(&connection).map_err(display_error)?;
        Ok(Self(Mutex::new(connection)))
    }

    fn connection(&self) -> Result<MutexGuard<'_, Connection>, String> {
        self.0.lock().map_err(|_| "Local database lock was poisoned".to_string())
    }

    pub fn list_threads(&self) -> Result<Vec<Thread>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, provider_thread_id, subject, snippet, participants_json,
                        last_message_at, unread, starred, archived, labels_json
                 FROM threads
                 WHERE archived = 0
                 ORDER BY last_message_at DESC",
            )
            .map_err(display_error)?;
        let rows = statement
            .query_map([], thread_from_row)
            .map_err(display_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(display_error)
    }

    pub fn get_thread(&self, id: &str) -> Result<ThreadDetail, String> {
        let connection = self.connection()?;
        let thread = connection
            .query_row(
                "SELECT id, provider_thread_id, subject, snippet, participants_json,
                        last_message_at, unread, starred, archived, labels_json
                 FROM threads WHERE id = ?1",
                [id],
                thread_from_row,
            )
            .optional()
            .map_err(display_error)?
            .ok_or_else(|| "Thread not found".to_string())?;

        let mut statement = connection
            .prepare(
                "SELECT id, thread_id, sender, recipients_json, sent_at, body_html, body_text
                 FROM messages WHERE thread_id = ?1 ORDER BY sent_at",
            )
            .map_err(display_error)?;
        let rows = statement
            .query_map([id], |row| {
                Ok(Message {
                    id: row.get(0)?,
                    thread_id: row.get(1)?,
                    sender: row.get(2)?,
                    recipients: decode_json(row.get::<_, String>(3)?)?,
                    sent_at: row.get(4)?,
                    body_html: row.get(5)?,
                    body_text: row.get(6)?,
                })
            })
            .map_err(display_error)?;
        let messages = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(display_error)?;
        Ok(ThreadDetail { thread, messages })
    }

    pub fn search_threads(&self, request: &SearchThreadsRequest) -> Result<Vec<Thread>, String> {
        if request.query.trim().is_empty() {
            return self.list_threads();
        }
        let connection = self.connection()?;
        let limit = request.limit.unwrap_or(50).min(200) as i64;
        let query = fts_query(&request.query);
        let mut statement = connection
            .prepare(
                "SELECT t.id, t.provider_thread_id, t.subject, t.snippet,
                        t.participants_json, t.last_message_at, t.unread, t.starred,
                        t.archived, t.labels_json
                 FROM thread_search s
                 JOIN threads t ON t.id = s.thread_id
                 WHERE thread_search MATCH ?1 AND t.archived = 0
                 ORDER BY rank, t.last_message_at DESC
                 LIMIT ?2",
            )
            .map_err(display_error)?;
        let rows = statement
            .query_map(params![query, limit], thread_from_row)
            .map_err(display_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(display_error)
    }

    pub fn mutate_thread(&self, mutation: &ThreadMutation) -> Result<(), String> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(display_error)?;
        let (kind, value) = match mutation {
            ThreadMutation::Archive { value, .. } => ("archive", *value),
            ThreadMutation::Read { value, .. } => ("read", *value),
            ThreadMutation::Star { value, .. } => ("star", *value),
        };
        let column = match mutation {
            ThreadMutation::Archive { .. } => "archived",
            ThreadMutation::Read { .. } => "unread",
            ThreadMutation::Star { .. } => "starred",
        };
        let stored_value = match mutation {
            ThreadMutation::Read { .. } => !value,
            _ => value,
        };
        let sql = format!("UPDATE threads SET {column} = ?1 WHERE id = ?2");
        let changed = transaction
            .execute(&sql, params![stored_value, mutation.thread_id()])
            .map_err(display_error)?;
        if changed == 0 {
            return Err("Thread not found".to_string());
        }
        transaction
            .execute(
                "INSERT INTO mutations(
                    id, account_id, thread_id, kind, payload_json, state, created_at
                 ) VALUES (?1, 'default', ?2, ?3, ?4, 'pending', ?5)",
                params![
                    Uuid::new_v4().to_string(),
                    mutation.thread_id(),
                    kind,
                    serde_json::to_string(mutation).map_err(display_error)?,
                    Utc::now().to_rfc3339(),
                ],
            )
            .map_err(display_error)?;
        transaction.commit().map_err(display_error)
    }

    pub fn sync_status(&self) -> Result<SyncStatus, String> {
        let connection = self.connection()?;
        let (cursor, last_successful_sync, error) = connection
            .query_row(
                "SELECT cursor, last_successful_sync, last_error
                 FROM sync_state WHERE account_id = 'default'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(display_error)?;
        let pending_mutations = connection
            .query_row(
                "SELECT count(*) FROM mutations WHERE state IN ('pending', 'running')",
                [],
                |row| row.get(0),
            )
            .map_err(display_error)?;
        Ok(SyncStatus {
            state: if error.is_some() { "error" } else { "idle" },
            last_successful_sync,
            cursor,
            pending_mutations,
            error,
        })
    }

    pub fn record_local_sync(&self) -> Result<SyncStatus, String> {
        let connection = self.connection()?;
        connection
            .execute(
                "UPDATE sync_state
                 SET last_successful_sync = ?1, last_error = NULL
                 WHERE account_id = 'default'",
                [Utc::now().to_rfc3339()],
            )
            .map_err(display_error)?;
        drop(connection);
        self.sync_status()
    }
}

fn thread_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Thread> {
    Ok(Thread {
        id: row.get(0)?,
        provider_thread_id: row.get(1)?,
        subject: row.get(2)?,
        snippet: row.get(3)?,
        participants: decode_json(row.get::<_, String>(4)?)?,
        last_message_at: row.get(5)?,
        unread: row.get(6)?,
        starred: row.get(7)?,
        archived: row.get(8)?,
        labels: decode_json(row.get::<_, String>(9)?)?,
    })
}

fn decode_json(value: String) -> rusqlite::Result<Vec<String>> {
    serde_json::from_str(&value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            value.len(),
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn fts_query(input: &str) -> String {
    input
        .split_whitespace()
        .map(|term| format!("\"{}\"*", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ")
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
        "Welcome to Dispatch",
        "A keyboard-first inbox that keeps your mail on this device.",
        "Dispatch",
        "2026-03-05T16:30:00Z",
        true,
        false,
        "<p>Welcome to <strong>Dispatch</strong>.</p><p>Use <kbd>j</kbd> and <kbd>k</kbd> to move, <kbd>e</kbd> to archive, <kbd>s</kbd> to star, and <kbd>⌘K</kbd> to open the command palette.</p>",
    )?;
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
    )?;
    transaction.commit()
}

#[allow(clippy::too_many_arguments)]
fn insert_demo(
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
        "INSERT INTO threads VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9)",
        params![id, format!("demo-{id}"), subject, snippet, participants, sent_at, unread, starred, labels],
    )?;
    transaction.execute(
        "INSERT INTO messages VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            format!("{id}-message"),
            id,
            format!("{participant} <hello@dispatch.local>"),
            "[\"You <you@example.com>\"]",
            sent_at,
            body,
            snippet,
        ],
    )?;
    transaction.execute(
        "INSERT INTO thread_search(thread_id, subject, snippet, participants, body)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, subject, snippet, participant, snippet],
    )?;
    Ok(())
}

fn display_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> Database {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(SCHEMA).unwrap();
        seed_if_empty(&connection).unwrap();
        Database(Mutex::new(connection))
    }

    #[test]
    fn searches_local_fts_index() {
        let result = database()
            .search_threads(&SearchThreadsRequest {
                query: "keyboard".into(),
                limit: None,
            })
            .unwrap();
        assert_eq!(result[0].id, "welcome");
    }

    #[test]
    fn mutation_is_optimistic_and_durable() {
        let database = database();
        database
            .mutate_thread(&ThreadMutation::Archive {
                thread_id: "welcome".into(),
                value: true,
            })
            .unwrap();
        assert!(!database.list_threads().unwrap().iter().any(|thread| thread.id == "welcome"));
        assert_eq!(database.sync_status().unwrap().pending_mutations, 1);
    }
}
