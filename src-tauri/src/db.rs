use std::{
    path::Path,
    sync::{Mutex, MutexGuard},
};

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use uuid::Uuid;

use crate::mime::NormalizedMessage;
use crate::models::{
    Message, SearchThreadsRequest, SyncStatus, Thread, ThreadDetail, ThreadMutation,
};

#[derive(Debug, Clone)]
pub struct PendingMutation {
    pub id: String,
    pub provider_thread_id: String,
    pub mutation: ThreadMutation,
}

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
        let mut connection = Connection::open(path).map_err(display_error)?;
        connection.execute_batch(SCHEMA).map_err(display_error)?;
        connection
            .execute(
                "UPDATE mutations SET state = 'pending', last_error = 'Interrupted before acknowledgement'
                 WHERE state = 'running'",
                [],
            )
            .map_err(display_error)?;
        crate::correspondence::migrate(&mut connection)?;
        seed_if_empty(&connection).map_err(display_error)?;
        Ok(Self(Mutex::new(connection)))
    }

    #[cfg(test)]
    pub(crate) fn open_memory() -> Self {
        let mut connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(SCHEMA).unwrap();
        crate::correspondence::migrate(&mut connection).unwrap();
        seed_if_empty(&connection).unwrap();
        Self(Mutex::new(connection))
    }

    pub(crate) fn connection(&self) -> Result<MutexGuard<'_, Connection>, String> {
        self.0
            .lock()
            .map_err(|_| "Local database lock was poisoned".to_string())
    }

    pub fn list_threads(&self) -> Result<Vec<Thread>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, provider_thread_id, subject, snippet, participants_json,
                        last_message_at, unread, starred, archived, labels_json, trashed
                 FROM threads
                 WHERE archived = 0 AND trashed = 0
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
                        last_message_at, unread, starred, archived, labels_json, trashed
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
        let messages = rows.collect::<Result<Vec<_>, _>>().map_err(display_error)?;
        Ok(ThreadDetail { thread, messages })
    }

    pub fn search_threads(&self, request: &SearchThreadsRequest) -> Result<Vec<Thread>, String> {
        if request.query.trim().is_empty() {
            return self.list_threads();
        }
        let connection = self.connection()?;
        let limit = request.limit.unwrap_or(50).min(200) as i64;
        let offset = request.offset.unwrap_or(0) as i64;
        let query = fts_query(&request.query);
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        // Trashed threads are hidden alongside archived ones by default; the
        // same "include archived" search toggle reveals both, since neither
        // belongs in the everyday inbox view.
        let archived_filter = if request.include_archived.unwrap_or(false) {
            ""
        } else {
            "AND t.archived = 0 AND t.trashed = 0"
        };
        // -1 asks FTS5 to excerpt whichever column has the most matches, so a
        // hit on the body or a recipient still produces a relevant snippet.
        // The match itself is wrapped in \u{1}/\u{2} rather than HTML markup
        // so the frontend can highlight it without ever parsing untrusted HTML.
        let sql = format!(
            "SELECT t.id, t.provider_thread_id, t.subject, t.snippet,
                    t.participants_json, t.last_message_at, t.unread, t.starred,
                    t.archived, t.labels_json, t.trashed,
                    snippet(thread_search, -1, '\u{1}', '\u{2}', '…', 12) AS match_snippet
             FROM thread_search s
             JOIN threads t ON t.id = s.thread_id
             WHERE thread_search MATCH ?1 {archived_filter}
             ORDER BY rank, t.last_message_at DESC
             LIMIT ?2 OFFSET ?3"
        );
        let mut statement = connection.prepare(&sql).map_err(display_error)?;
        let rows = statement
            .query_map(params![query, limit, offset], |row| {
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
                    trashed: row.get(10)?,
                    match_snippet: row.get(11)?,
                })
            })
            .map_err(display_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(display_error)
    }

    pub fn mutate_thread(&self, mutation: &ThreadMutation) -> Result<(), String> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(display_error)?;
        let (kind, value) = match mutation {
            ThreadMutation::Archive { value, .. } => ("archive", *value),
            ThreadMutation::Trash { value, .. } => ("trash", *value),
            ThreadMutation::Read { value, .. } => ("read", *value),
            ThreadMutation::Star { value, .. } => ("star", *value),
            ThreadMutation::Label { value, .. } => ("label", *value),
        };
        let changed = match mutation {
            ThreadMutation::Label {
                thread_id,
                label_id,
                value,
            } => {
                let labels: String = transaction
                    .query_row(
                        "SELECT labels_json FROM threads WHERE id = ?1",
                        [thread_id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(display_error)?
                    .ok_or_else(|| "Thread not found".to_string())?;
                let mut labels: Vec<String> =
                    serde_json::from_str(&labels).map_err(display_error)?;
                labels.retain(|item| item != label_id);
                if *value {
                    labels.push(label_id.clone());
                    labels.sort();
                    labels.dedup();
                }
                let unread = labels.iter().any(|label| label == "UNREAD");
                let starred = labels.iter().any(|label| label == "STARRED");
                let archived = !labels.iter().any(|label| label == "INBOX");
                transaction
                    .execute(
                        "UPDATE threads
                         SET labels_json = ?1, unread = ?2, starred = ?3, archived = ?4
                         WHERE id = ?5",
                        params![
                            serde_json::to_string(&labels).map_err(display_error)?,
                            unread,
                            starred,
                            archived,
                            thread_id
                        ],
                    )
                    .map_err(display_error)?
            }
            _ => {
                let column = match mutation {
                    ThreadMutation::Archive { .. } => "archived",
                    ThreadMutation::Trash { .. } => "trashed",
                    ThreadMutation::Read { .. } => "unread",
                    ThreadMutation::Star { .. } => "starred",
                    ThreadMutation::Label { .. } => unreachable!(),
                };
                let stored_value = match mutation {
                    ThreadMutation::Read { .. } => !value,
                    _ => value,
                };
                let sql = format!("UPDATE threads SET {column} = ?1 WHERE id = ?2");
                transaction
                    .execute(&sql, params![stored_value, mutation.thread_id()])
                    .map_err(display_error)?
            }
        };
        if changed == 0 {
            return Err("Thread not found".to_string());
        }
        let payload = serde_json::to_string(mutation).map_err(display_error)?;
        let duplicate: bool = transaction
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM mutations
                    WHERE thread_id = ?1 AND kind = ?2 AND payload_json = ?3
                      AND state IN ('pending', 'running')
                 )",
                params![mutation.thread_id(), kind, payload],
                |row| row.get(0),
            )
            .map_err(display_error)?;
        if !duplicate {
            transaction
                .execute(
                    "INSERT INTO mutations(
                    id, account_id, thread_id, kind, payload_json, state, created_at
                 ) VALUES (?1, 'default', ?2, ?3, ?4, 'pending', ?5)",
                    params![
                        Uuid::new_v4().to_string(),
                        mutation.thread_id(),
                        kind,
                        payload,
                        Utc::now().to_rfc3339(),
                    ],
                )
                .map_err(display_error)?;
        }
        transaction.commit().map_err(display_error)
    }

    pub fn sync_status(&self) -> Result<SyncStatus, String> {
        let connection = self.connection()?;
        let (cursor, last_successful_sync, mut error): (
            Option<String>,
            Option<String>,
            Option<String>,
        ) = connection
            .query_row(
                "SELECT cursor, last_successful_sync, last_error
                 FROM sync_state WHERE account_id = 'default'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(display_error)?;
        if error.is_none() {
            error = connection
                .query_row(
                    "SELECT last_error FROM mutations
                     WHERE state = 'failed' ORDER BY created_at DESC LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(display_error)?
                .flatten();
        }
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

    pub fn cursor(&self) -> Result<Option<String>, String> {
        self.connection()?
            .query_row(
                "SELECT cursor FROM sync_state WHERE account_id = 'default'",
                [],
                |row| row.get(0),
            )
            .map_err(display_error)
    }

    pub fn finish_sync(&self, cursor: &str) -> Result<(), String> {
        self.connection()?
            .execute(
                "UPDATE sync_state SET cursor = ?1, last_successful_sync = ?2, last_error = NULL
                 WHERE account_id = 'default'",
                params![cursor, Utc::now().to_rfc3339()],
            )
            .map(|_| ())
            .map_err(display_error)
    }

    pub fn fail_sync(&self, error: &str) -> Result<(), String> {
        self.connection()?
            .execute(
                "UPDATE sync_state SET last_error = ?1 WHERE account_id = 'default'",
                [error],
            )
            .map(|_| ())
            .map_err(display_error)
    }

    pub fn begin_full_sync(&self) -> Result<(), String> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(display_error)?;
        transaction
            .execute("DELETE FROM thread_search", [])
            .map_err(display_error)?;
        transaction
            .execute("DELETE FROM threads", [])
            .map_err(display_error)?;
        // An interrupted full import must restart in full. Keeping the old
        // cursor here would make the next startup perform an incremental sync
        // against an intentionally emptied cache.
        transaction
            .execute(
                "UPDATE sync_state SET cursor = NULL WHERE account_id = 'default'",
                [],
            )
            .map_err(display_error)?;
        transaction.commit().map_err(display_error)
    }

    pub fn delete_gmail_thread(&self, provider_thread_id: &str) -> Result<(), String> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(display_error)?;
        let thread_id: Option<String> = transaction
            .query_row(
                "SELECT id FROM threads WHERE provider_thread_id = ?1",
                [provider_thread_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(display_error)?;
        if let Some(thread_id) = thread_id {
            transaction
                .execute(
                    "DELETE FROM thread_search WHERE thread_id = ?1",
                    [&thread_id],
                )
                .map_err(display_error)?;
            transaction
                .execute("DELETE FROM threads WHERE id = ?1", [&thread_id])
                .map_err(display_error)?;
        }
        transaction.commit().map_err(display_error)
    }

    pub fn upsert_gmail_thread(&self, messages: &[NormalizedMessage]) -> Result<(), String> {
        let Some(latest) = messages.iter().max_by(|a, b| a.date.cmp(&b.date)) else {
            return Ok(());
        };
        let thread_id = &latest.thread_id;
        let mut participants: Vec<String> = messages
            .iter()
            .map(|message| message.from.clone())
            .collect();
        participants.sort();
        participants.dedup();
        // Indexed separately from `participants`: recipients should be
        // searchable even though they aren't shown in the thread list's
        // "From" line.
        let mut search_participants: Vec<String> = messages
            .iter()
            .flat_map(|message| {
                std::iter::once(message.from.clone()).chain(message.to.iter().cloned())
            })
            .collect();
        search_participants.sort();
        search_participants.dedup();
        let mut labels: Vec<String> = messages
            .iter()
            .flat_map(|message| message.labels.iter().cloned())
            .collect();
        labels.sort();
        labels.dedup();
        let unread = labels.iter().any(|label| label == "UNREAD");
        let starred = labels.iter().any(|label| label == "STARRED");
        let archived = !labels.iter().any(|label| label == "INBOX");
        let trashed = labels.iter().any(|label| label == "TRASH");
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(display_error)?;
        transaction
            .execute(
                "INSERT INTO threads(
                    id, provider_thread_id, subject, snippet, participants_json,
                    last_message_at, unread, starred, archived, labels_json, trashed
                 ) VALUES (?1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(id) DO UPDATE SET
                    subject=excluded.subject, snippet=excluded.snippet,
                    participants_json=excluded.participants_json,
                    last_message_at=excluded.last_message_at, unread=excluded.unread,
                    starred=excluded.starred, archived=excluded.archived,
                    labels_json=excluded.labels_json, trashed=excluded.trashed",
                params![
                    thread_id,
                    latest.subject,
                    latest.snippet,
                    serde_json::to_string(&participants).map_err(display_error)?,
                    latest.date,
                    unread,
                    starred,
                    archived,
                    serde_json::to_string(&labels).map_err(display_error)?,
                    trashed,
                ],
            )
            .map_err(display_error)?;
        transaction
            .execute("DELETE FROM messages WHERE thread_id = ?1", [thread_id])
            .map_err(display_error)?;
        transaction
            .execute(
                "DELETE FROM thread_search WHERE thread_id = ?1",
                [thread_id],
            )
            .map_err(display_error)?;
        let mut body = String::new();
        for message in messages {
            transaction.execute("INSERT INTO message_metadata(id, payload) VALUES (?1, ?2) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload", params![message.id, message.metadata_json]).map_err(display_error)?;
            body.push_str(&message.body_text);
            body.push(' ');
            transaction
                .execute(
                    "INSERT INTO messages(
                        id, thread_id, sender, recipients_json, sent_at, body_html, body_text
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        message.id,
                        thread_id,
                        message.from,
                        serde_json::to_string(&message.to).map_err(display_error)?,
                        message.date,
                        message.body_html,
                        message.body_text,
                    ],
                )
                .map_err(display_error)?;
        }
        transaction
            .execute(
                "INSERT INTO thread_search(thread_id, subject, snippet, participants, body)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    thread_id,
                    latest.subject,
                    latest.snippet,
                    search_participants.join(" "),
                    body
                ],
            )
            .map_err(display_error)?;
        transaction.commit().map_err(display_error)
    }

    pub fn claim_mutations(&self, limit: usize) -> Result<Vec<PendingMutation>, String> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(display_error)?;
        let result = {
            let mut statement = transaction
                .prepare(
                    "SELECT m.id, t.provider_thread_id, m.payload_json
                     FROM mutations m JOIN threads t ON t.id = m.thread_id
                     WHERE m.state = 'pending' ORDER BY m.created_at LIMIT ?1",
                )
                .map_err(display_error)?;
            let rows = statement
                .query_map([limit as i64], |row| {
                    let payload: String = row.get(2)?;
                    let mutation = serde_json::from_str(&payload).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            payload.len(),
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?;
                    Ok(PendingMutation {
                        id: row.get(0)?,
                        provider_thread_id: row.get(1)?,
                        mutation,
                    })
                })
                .map_err(display_error)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(display_error)?
        };
        for mutation in &result {
            transaction
                .execute(
                    "UPDATE mutations SET state = 'running', attempts = attempts + 1
                     WHERE id = ?1 AND state = 'pending'",
                    [&mutation.id],
                )
                .map_err(display_error)?;
        }
        transaction.commit().map_err(display_error)?;
        Ok(result)
    }

    pub fn complete_mutation(&self, id: &str) -> Result<(), String> {
        self.connection()?
            .execute(
                "UPDATE mutations SET state = 'done', last_error = NULL WHERE id = ?1",
                [id],
            )
            .map(|_| ())
            .map_err(display_error)
    }

    pub fn reject_mutation(&self, id: &str, error: &str, retry: bool) -> Result<(), String> {
        self.connection()?
            .execute(
                "UPDATE mutations SET state = ?1, last_error = ?2 WHERE id = ?3",
                params![if retry { "pending" } else { "failed" }, error, id],
            )
            .map(|_| ())
            .map_err(display_error)
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
        trashed: row.get(10)?,
        match_snippet: None,
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

/// Builds an FTS5 MATCH expression, ANDing together every unquoted word and
/// every "quoted phrase" as a prefix match. `input.split('"')` alternates
/// unquoted segments (even indices) with quoted ones (odd indices); an
/// unterminated trailing quote is simply treated as still-quoted.
fn fts_query(input: &str) -> String {
    let mut terms: Vec<String> = Vec::new();
    for (index, segment) in input.split('"').enumerate() {
        if index % 2 == 0 {
            for word in segment.split_whitespace() {
                terms.push(format!("\"{}\"*", word));
            }
        } else {
            let words: Vec<&str> = segment.split_whitespace().collect();
            if words.is_empty() {
                continue;
            }
            terms.push(format!("\"{}\"*", words.join(" ")));
        }
    }
    terms.join(" AND ")
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
        "INSERT INTO threads VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9, 0)",
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
        Database::open_memory()
    }

    #[test]
    fn searches_local_fts_index() {
        let result = database()
            .search_threads(&SearchThreadsRequest {
                query: "keyboard".into(),
                limit: None,
                offset: None,
                include_archived: None,
            })
            .unwrap();
        assert_eq!(result[0].id, "welcome");
        assert!(result[0].match_snippet.is_some());
    }

    #[test]
    fn phrase_search_requires_contiguous_words() {
        let database = database();
        let matches = database
            .search_threads(&SearchThreadsRequest {
                query: "\"keeps your mail\"".into(),
                limit: None,
                offset: None,
                include_archived: None,
            })
            .unwrap();
        assert_eq!(matches[0].id, "welcome");

        let no_matches = database
            .search_threads(&SearchThreadsRequest {
                query: "\"mail your keeps\"".into(),
                limit: None,
                offset: None,
                include_archived: None,
            })
            .unwrap();
        assert!(no_matches.is_empty());
    }

    #[test]
    fn search_excludes_archived_unless_requested() {
        let database = database();
        database
            .mutate_thread(&ThreadMutation::Archive {
                thread_id: "welcome".into(),
                value: true,
            })
            .unwrap();

        let hidden = database
            .search_threads(&SearchThreadsRequest {
                query: "keyboard".into(),
                limit: None,
                offset: None,
                include_archived: None,
            })
            .unwrap();
        assert!(hidden.is_empty());

        let shown = database
            .search_threads(&SearchThreadsRequest {
                query: "keyboard".into(),
                limit: None,
                offset: None,
                include_archived: Some(true),
            })
            .unwrap();
        assert_eq!(shown[0].id, "welcome");
    }

    #[test]
    fn search_supports_offset_pagination() {
        let database = database();
        for (id, date) in [
            ("alpha", "2026-01-01T00:00:00Z"),
            ("beta", "2026-01-02T00:00:00Z"),
        ] {
            database
                .upsert_gmail_thread(&[NormalizedMessage {
                    id: format!("{id}-message"),
                    thread_id: id.into(),
                    subject: "Pagination test".into(),
                    from: "sender@example.com".into(),
                    to: vec!["recipient@example.com".into()],
                    date: date.into(),
                    body_html: String::new(),
                    body_text: "unique-pagination-term".into(),
                    snippet: "unique-pagination-term".into(),
                    labels: vec!["INBOX".into()],
                    metadata_json: "{}".into(),
                }])
                .unwrap();
        }

        let first_page = database
            .search_threads(&SearchThreadsRequest {
                query: "unique-pagination-term".into(),
                limit: Some(1),
                offset: None,
                include_archived: None,
            })
            .unwrap();
        let second_page = database
            .search_threads(&SearchThreadsRequest {
                query: "unique-pagination-term".into(),
                limit: Some(1),
                offset: Some(1),
                include_archived: None,
            })
            .unwrap();
        assert_eq!(first_page.len(), 1);
        assert_eq!(second_page.len(), 1);
        assert_ne!(first_page[0].id, second_page[0].id);
    }

    #[test]
    fn deleting_a_thread_also_removes_its_search_index_row() {
        let database = database();
        database.delete_gmail_thread("demo-welcome").unwrap();
        let remaining: i64 = database
            .connection()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM thread_search WHERE thread_id = 'welcome'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);
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
            .list_threads()
            .unwrap()
            .iter()
            .any(|thread| thread.id == "welcome"));

        let hidden = database
            .search_threads(&SearchThreadsRequest {
                query: "keyboard".into(),
                limit: None,
                offset: None,
                include_archived: None,
            })
            .unwrap();
        assert!(hidden.is_empty());

        let shown = database
            .search_threads(&SearchThreadsRequest {
                query: "keyboard".into(),
                limit: None,
                offset: None,
                include_archived: Some(true),
            })
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
            .list_threads()
            .unwrap()
            .iter()
            .any(|thread| thread.id == "welcome"));
        assert_eq!(database.sync_status().unwrap().pending_mutations, 1);
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
        assert_eq!(reopened.claim_mutations(10).unwrap().len(), 1);
        drop(reopened);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
        let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    }

    #[test]
    fn interrupted_full_sync_cannot_reuse_the_previous_cursor() {
        let database = database();
        database.finish_sync("old-cursor").unwrap();
        database.begin_full_sync().unwrap();
        assert_eq!(database.cursor().unwrap(), None);
    }
}
