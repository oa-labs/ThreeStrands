use std::{
    path::Path,
    sync::{Mutex, MutexGuard},
};

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use uuid::Uuid;

use crate::mime::NormalizedMessage;
use crate::mime::UnsubscribeMetadata;
use crate::models::{
    Account, Message, SearchThreadsRequest, SyncStatus, Thread, ThreadDetail, ThreadMutation,
    UnsubscribeMethod, UnsubscribeTarget,
};

/// Assigned to newly connected accounts in rotation, so each has a distinct
/// color for switcher/thread-row indicators without asking the user to pick
/// one up front.
const ACCOUNT_COLORS: [&str; 8] = [
    "#4285F4", "#34A853", "#EA4335", "#FBBC05", "#9C27B0", "#00ACC1", "#FF7043", "#5C6BC0",
];

#[derive(Debug, Clone)]
pub struct PendingMutation {
    pub id: String,
    pub provider_thread_id: String,
    pub mutation: ThreadMutation,
}

/// `threads.id`, derived from the pair that's actually unique: Gmail thread
/// IDs are unique only within one account, not across two different
/// accounts, so the bare provider ID can't be used as the local primary key
/// once more than one account is connected.
fn local_thread_id(account_id: &str, provider_thread_id: &str) -> String {
    format!("{account_id}:{provider_thread_id}")
}

const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS threads (
    id TEXT PRIMARY KEY,
    provider_thread_id TEXT NOT NULL,
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

CREATE TABLE IF NOT EXISTS accounts (
    email TEXT PRIMARY KEY,
    display_name TEXT,
    color TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('connected', 'needs_reauth')),
    sort_order INTEGER NOT NULL,
    connected_at TEXT NOT NULL,
    last_synced_at TEXT
);
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

    /// `account_id` merges every account when `None` — the unified inbox —
    /// or scopes to just that account when set.
    pub fn list_threads(&self, account_id: Option<&str>) -> Result<Vec<Thread>, String> {
        self.list_threads_where(account_id, "archived = 0 AND trashed = 0")
    }

    /// Gmail's "All Mail": everything except Trash. (There's a `spam` column
    /// referenced elsewhere for an in-progress Spam feature, but no migration
    /// has added it to `threads` yet, so it isn't filterable here.)
    pub fn list_all_mail(&self, account_id: Option<&str>) -> Result<Vec<Thread>, String> {
        self.list_threads_where(account_id, "trashed = 0")
    }

    pub fn list_trash(&self, account_id: Option<&str>) -> Result<Vec<Thread>, String> {
        self.list_threads_where(account_id, "trashed = 1")
    }

    fn list_threads_where(
        &self,
        account_id: Option<&str>,
        filter: &str,
    ) -> Result<Vec<Thread>, String> {
        let connection = self.connection()?;
        let sql = format!(
            "SELECT id, provider_thread_id, subject, snippet, participants_json,
                    last_message_at, unread, starred, archived, labels_json, trashed, account_id
             FROM threads
             WHERE {filter} {}
             ORDER BY last_message_at DESC",
            if account_id.is_some() {
                "AND account_id = ?1"
            } else {
                ""
            }
        );
        let mut statement = connection.prepare(&sql).map_err(display_error)?;
        let rows = match account_id {
            Some(id) => statement
                .query_map([id], thread_from_row)
                .map_err(display_error)?,
            None => statement
                .query_map([], thread_from_row)
                .map_err(display_error)?,
        };
        rows.collect::<Result<Vec<_>, _>>().map_err(display_error)
    }

    pub fn get_thread(&self, id: &str) -> Result<ThreadDetail, String> {
        let connection = self.connection()?;
        let thread = connection
            .query_row(
                "SELECT id, provider_thread_id, subject, snippet, participants_json,
                        last_message_at, unread, starred, archived, labels_json, trashed, account_id
                 FROM threads WHERE id = ?1",
                [id],
                thread_from_row,
            )
            .optional()
            .map_err(display_error)?
            .ok_or_else(|| "Thread not found".to_string())?;

        let mut statement = connection
            .prepare(
                "SELECT id, thread_id, sender, recipients_json, sent_at, body_html, body_text,
                        unsubscribe_json
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
                    unsubscribe: row
                        .get::<_, Option<String>>(7)?
                        .and_then(|value| serde_json::from_str::<UnsubscribeMetadata>(&value).ok())
                        .map(|value| value.info()),
                })
            })
            .map_err(display_error)?;
        let messages = rows.collect::<Result<Vec<_>, _>>().map_err(display_error)?;
        Ok(ThreadDetail { thread, messages })
    }

    /// Resolves the unsubscribe URL from locally cached message metadata and
    /// records the attempt before any external side effect occurs. The
    /// webview supplies only the stable message ID, never an arbitrary URL.
    pub fn begin_unsubscribe(&self, message_id: &str) -> Result<UnsubscribeTarget, String> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(display_error)?;
        let (thread_id, metadata_json): (String, Option<String>) = transaction
            .query_row(
                "SELECT thread_id, unsubscribe_json FROM messages WHERE id = ?1",
                [message_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(display_error)?
            .ok_or_else(|| "Message not found".to_string())?;
        let metadata = metadata_json
            .ok_or_else(|| "This message has no unsubscribe option".to_string())
            .and_then(|value| {
                serde_json::from_str::<UnsubscribeMetadata>(&value).map_err(display_error)
            })?;
        let (method, url) = if let Some(url) = metadata.one_click_url {
            (UnsubscribeMethod::OneClick, url)
        } else if let Some(url) = metadata.mailto_url {
            (UnsubscribeMethod::Mailto, url)
        } else if let Some(url) = metadata.web_url {
            (UnsubscribeMethod::Web, url)
        } else {
            return Err("This message has no usable unsubscribe option".to_string());
        };
        let request_id = Uuid::new_v4().to_string();
        transaction
            .execute(
                "INSERT INTO unsubscribe_requests(
                    id, message_id, thread_id, method, state, created_at
                 ) VALUES (?1, ?2, ?3, ?4, 'pending', ?5)",
                params![
                    request_id,
                    message_id,
                    thread_id,
                    unsubscribe_method_name(&method),
                    Utc::now().to_rfc3339(),
                ],
            )
            .map_err(display_error)?;
        transaction.commit().map_err(display_error)?;
        Ok(UnsubscribeTarget {
            request_id,
            method,
            url,
        })
    }

    pub fn finish_unsubscribe(
        &self,
        request_id: &str,
        state: &str,
        http_status: Option<u16>,
        error: Option<&str>,
    ) -> Result<(), String> {
        if !matches!(state, "succeeded" | "opened" | "failed") {
            return Err("Invalid unsubscribe request state".to_string());
        }
        let changed = self
            .connection()?
            .execute(
                "UPDATE unsubscribe_requests
                 SET state = ?1, http_status = ?2, completed_at = ?3, last_error = ?4
                 WHERE id = ?5 AND state = 'pending'",
                params![
                    state,
                    http_status,
                    Utc::now().to_rfc3339(),
                    error,
                    request_id
                ],
            )
            .map_err(display_error)?;
        if changed == 0 {
            return Err("Unsubscribe request was not pending".to_string());
        }
        Ok(())
    }

    pub fn message_ids_for_thread(&self, thread_id: &str) -> Result<Vec<String>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare("SELECT id FROM messages WHERE thread_id = ?1 ORDER BY sent_at")
            .map_err(display_error)?;
        let ids = statement
            .query_map([thread_id], |row| row.get(0))
            .map_err(display_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(display_error)?;
        Ok(ids)
    }

    /// `account_id` merges every account when `None` — the unified inbox —
    /// or scopes to just that account when set, same as [`Self::list_threads`].
    pub fn search_threads(
        &self,
        request: &SearchThreadsRequest,
        account_id: Option<&str>,
    ) -> Result<Vec<Thread>, String> {
        if request.query.trim().is_empty() {
            return self.list_threads(account_id);
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
        let account_filter = if account_id.is_some() {
            "AND t.account_id = ?4"
        } else {
            ""
        };
        // -1 asks FTS5 to excerpt whichever column has the most matches, so a
        // hit on the body or a recipient still produces a relevant snippet.
        // The match itself is wrapped in \u{1}/\u{2} rather than HTML markup
        // so the frontend can highlight it without ever parsing untrusted HTML.
        let sql = format!(
            "SELECT t.id, t.provider_thread_id, t.subject, t.snippet,
                    t.participants_json, t.last_message_at, t.unread, t.starred,
                    t.archived, t.labels_json, t.trashed, t.account_id,
                    snippet(thread_search, -1, '\u{1}', '\u{2}', '…', 12) AS match_snippet
             FROM thread_search s
             JOIN threads t ON t.id = s.thread_id
             WHERE thread_search MATCH ?1 {archived_filter} {account_filter}
             ORDER BY rank, t.last_message_at DESC
             LIMIT ?2 OFFSET ?3"
        );
        let mut statement = connection.prepare(&sql).map_err(display_error)?;
        let map_row = |row: &rusqlite::Row<'_>| {
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
                account_id: row.get(11)?,
                match_snippet: row.get(12)?,
            })
        };
        let rows = match account_id {
            Some(id) => statement
                .query_map(params![query, limit, offset, id], map_row)
                .map_err(display_error)?,
            None => statement
                .query_map(params![query, limit, offset], map_row)
                .map_err(display_error)?,
        };
        rows.collect::<Result<Vec<_>, _>>().map_err(display_error)
    }

    pub fn mutate_thread(&self, mutation: &ThreadMutation) -> Result<(), String> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(display_error)?;
        let (kind, value) = match mutation {
            ThreadMutation::Archive { value, .. } => ("archive", *value),
            ThreadMutation::Trash { value, .. } => ("trash", *value),
            ThreadMutation::Spam { value, .. } => ("spam", *value),
            ThreadMutation::Read { value, .. } => ("read", *value),
            ThreadMutation::Star { value, .. } => ("star", *value),
            ThreadMutation::Label { value, .. } => ("label", *value),
        };
        let changed = match mutation {
            ThreadMutation::Spam { thread_id, value } => {
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
                labels.retain(|item| item != "SPAM" && item != "INBOX");
                labels.push(if *value { "SPAM" } else { "INBOX" }.to_string());
                labels.sort();
                labels.dedup();
                transaction
                    .execute(
                        "UPDATE threads SET labels_json = ?1, archived = ?2 WHERE id = ?3",
                        params![
                            serde_json::to_string(&labels).map_err(display_error)?,
                            value,
                            thread_id,
                        ],
                    )
                    .map_err(display_error)?
            }
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
                    ThreadMutation::Spam { .. } => unreachable!(),
                    ThreadMutation::Read { .. } => "unread",
                    ThreadMutation::Star { .. } => "starred",
                    ThreadMutation::Label { .. } => unreachable!(),
                };
                let stored_value = match mutation {
                    ThreadMutation::Read { .. } => !value,
                    ThreadMutation::Spam { .. } => unreachable!(),
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
        let account_id: String = transaction
            .query_row(
                "SELECT account_id FROM threads WHERE id = ?1",
                [mutation.thread_id()],
                |row| row.get(0),
            )
            .map_err(display_error)?;
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
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6)",
                    params![
                        Uuid::new_v4().to_string(),
                        account_id,
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

    pub fn sync_status(&self, account_id: &str) -> Result<SyncStatus, String> {
        let connection = self.connection()?;
        let (cursor, last_successful_sync, mut error): (
            Option<String>,
            Option<String>,
            Option<String>,
        ) = connection
            .query_row(
                "SELECT cursor, last_successful_sync, last_error
                 FROM sync_state WHERE account_id = ?1",
                [account_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(display_error)?;
        if error.is_none() {
            error = connection
                .query_row(
                    "SELECT last_error FROM mutations
                     WHERE state = 'failed' AND account_id = ?1 ORDER BY created_at DESC LIMIT 1",
                    [account_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(display_error)?
                .flatten();
        }
        let pending_mutations = connection
            .query_row(
                "SELECT count(*) FROM mutations WHERE state IN ('pending', 'running') AND account_id = ?1",
                [account_id],
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

    pub fn cursor(&self, account_id: &str) -> Result<Option<String>, String> {
        self.connection()?
            .query_row(
                "SELECT cursor FROM sync_state WHERE account_id = ?1",
                [account_id],
                |row| row.get(0),
            )
            .map_err(display_error)
    }

    pub fn finish_sync(&self, account_id: &str, cursor: &str) -> Result<(), String> {
        self.connection()?
            .execute(
                "UPDATE sync_state SET cursor = ?1, last_successful_sync = ?2, last_error = NULL
                 WHERE account_id = ?3",
                params![cursor, Utc::now().to_rfc3339(), account_id],
            )
            .map(|_| ())
            .map_err(display_error)
    }

    pub fn fail_sync(&self, account_id: &str, error: &str) -> Result<(), String> {
        self.connection()?
            .execute(
                "UPDATE sync_state SET last_error = ?1 WHERE account_id = ?2",
                params![error, account_id],
            )
            .map(|_| ())
            .map_err(display_error)
    }

    /// Wipes only `account_id`'s cached threads before a full resync, never
    /// another connected account's mail.
    pub fn begin_full_sync(&self, account_id: &str) -> Result<(), String> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(display_error)?;
        transaction
            .execute(
                "DELETE FROM thread_search WHERE thread_id IN
                    (SELECT id FROM threads WHERE account_id = ?1)",
                [account_id],
            )
            .map_err(display_error)?;
        transaction
            .execute("DELETE FROM threads WHERE account_id = ?1", [account_id])
            .map_err(display_error)?;
        // An interrupted full import must restart in full. Keeping the old
        // cursor here would make the next startup perform an incremental sync
        // against an intentionally emptied cache.
        transaction
            .execute(
                "UPDATE sync_state SET cursor = NULL WHERE account_id = ?1",
                [account_id],
            )
            .map_err(display_error)?;
        transaction.commit().map_err(display_error)
    }

    pub fn delete_gmail_thread(
        &self,
        account_id: &str,
        provider_thread_id: &str,
    ) -> Result<(), String> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(display_error)?;
        let thread_id: Option<String> = transaction
            .query_row(
                "SELECT id FROM threads WHERE account_id = ?1 AND provider_thread_id = ?2",
                params![account_id, provider_thread_id],
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

    pub fn upsert_gmail_thread(
        &self,
        account_id: &str,
        messages: &[NormalizedMessage],
    ) -> Result<(), String> {
        let Some(latest) = messages.iter().max_by(|a, b| a.date.cmp(&b.date)) else {
            return Ok(());
        };
        let provider_thread_id = &latest.thread_id;
        let thread_id = local_thread_id(account_id, provider_thread_id);
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
                    id, account_id, provider_thread_id, subject, snippet, participants_json,
                    last_message_at, unread, starred, archived, labels_json, trashed
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 ON CONFLICT(id) DO UPDATE SET
                    subject=excluded.subject, snippet=excluded.snippet,
                    participants_json=excluded.participants_json,
                    last_message_at=excluded.last_message_at, unread=excluded.unread,
                    starred=excluded.starred, archived=excluded.archived,
                    labels_json=excluded.labels_json, trashed=excluded.trashed",
                params![
                    thread_id,
                    account_id,
                    provider_thread_id,
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
            .execute("DELETE FROM messages WHERE thread_id = ?1", [&thread_id])
            .map_err(display_error)?;
        transaction
            .execute(
                "DELETE FROM thread_search WHERE thread_id = ?1",
                [&thread_id],
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
                        id, thread_id, sender, recipients_json, sent_at, body_html, body_text,
                        unsubscribe_json
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        message.id,
                        thread_id,
                        message.from,
                        serde_json::to_string(&message.to).map_err(display_error)?,
                        message.date,
                        message.body_html,
                        message.body_text,
                        message
                            .unsubscribe
                            .as_ref()
                            .map(|value| serde_json::to_string(value).map_err(display_error))
                            .transpose()?,
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

    /// Claims only `account_id`'s pending mutations, so one account's poller
    /// never picks up and tries to deliver another account's mutation
    /// through the wrong Gmail session.
    pub fn claim_mutations(
        &self,
        account_id: &str,
        limit: usize,
    ) -> Result<Vec<PendingMutation>, String> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(display_error)?;
        let result = {
            let mut statement = transaction
                .prepare(
                    "SELECT m.id, t.provider_thread_id, m.payload_json
                     FROM mutations m JOIN threads t ON t.id = m.thread_id
                     WHERE m.state = 'pending' AND m.account_id = ?1
                     ORDER BY m.created_at LIMIT ?2",
                )
                .map_err(display_error)?;
            let rows = statement
                .query_map(params![account_id, limit as i64], |row| {
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

    pub fn list_accounts(&self) -> Result<Vec<Account>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT email, display_name, color, status, sort_order, connected_at, last_synced_at
                 FROM accounts ORDER BY sort_order",
            )
            .map_err(display_error)?;
        let rows = statement
            .query_map([], account_from_row)
            .map_err(display_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(display_error)
    }

    /// Ensures an `accounts` row exists for `email`, marking it connected
    /// either way, and folds any pre-multi-account local state (the
    /// `sync_state`/`mutations` rows still keyed by the literal `'default'`)
    /// onto it. Idempotent: safe to call on every successful identity
    /// refresh, not just the first one.
    pub fn adopt_account(&self, email: &str) -> Result<Account, String> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(display_error)?;
        let exists: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM accounts WHERE email = ?1)",
                [email],
                |row| row.get(0),
            )
            .map_err(display_error)?;
        if exists {
            transaction
                .execute(
                    "UPDATE accounts SET status = 'connected' WHERE email = ?1",
                    [email],
                )
                .map_err(display_error)?;
        } else {
            let sort_order: i64 = transaction
                .query_row(
                    "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM accounts",
                    [],
                    |row| row.get(0),
                )
                .map_err(display_error)?;
            let color = ACCOUNT_COLORS[(sort_order as usize) % ACCOUNT_COLORS.len()];
            transaction
                .execute(
                    "INSERT INTO accounts(email, color, status, sort_order, connected_at)
                     VALUES (?1, ?2, 'connected', ?3, ?4)",
                    params![email, color, sort_order, Utc::now().to_rfc3339()],
                )
                .map_err(display_error)?;
        }
        // Fold any pre-multi-account local state, still keyed by the literal
        // 'default', onto the real address. A no-op after the first time.
        transaction
            .execute(
                "UPDATE sync_state SET account_id = ?1 WHERE account_id = 'default'",
                [email],
            )
            .map_err(display_error)?;
        transaction
            .execute(
                "UPDATE mutations SET account_id = ?1 WHERE account_id = 'default'",
                [email],
            )
            .map_err(display_error)?;
        transaction
            .execute(
                "UPDATE threads SET account_id = ?1 WHERE account_id = 'default'",
                [email],
            )
            .map_err(display_error)?;
        // Accounts adopted directly (not migrated from a 'default' row)
        // still need their own cursor row.
        transaction
            .execute(
                "INSERT OR IGNORE INTO sync_state(account_id) VALUES (?1)",
                [email],
            )
            .map_err(display_error)?;
        transaction.commit().map_err(display_error)?;
        drop(connection);
        self.get_account(email)?
            .ok_or_else(|| "Account not found".to_string())
    }

    pub fn get_account(&self, email: &str) -> Result<Option<Account>, String> {
        self.connection()?
            .query_row(
                "SELECT email, display_name, color, status, sort_order, connected_at, last_synced_at
                 FROM accounts WHERE email = ?1",
                [email],
                account_from_row,
            )
            .optional()
            .map_err(display_error)
    }

    pub fn remove_account(&self, email: &str) -> Result<(), String> {
        self.connection()?
            .execute("DELETE FROM accounts WHERE email = ?1", [email])
            .map(|_| ())
            .map_err(display_error)
    }

    pub fn set_account_color(&self, email: &str, color: &str) -> Result<(), String> {
        let changed = self
            .connection()?
            .execute(
                "UPDATE accounts SET color = ?1 WHERE email = ?2",
                params![color, email],
            )
            .map_err(display_error)?;
        if changed == 0 {
            return Err("Account not found".to_string());
        }
        Ok(())
    }

    pub fn reorder_accounts(&self, ordered_emails: &[String]) -> Result<(), String> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(display_error)?;
        for (index, email) in ordered_emails.iter().enumerate() {
            transaction
                .execute(
                    "UPDATE accounts SET sort_order = ?1 WHERE email = ?2",
                    params![index as i64, email],
                )
                .map_err(display_error)?;
        }
        transaction.commit().map_err(display_error)
    }
}

fn account_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Account> {
    Ok(Account {
        email: row.get(0)?,
        display_name: row.get(1)?,
        color: row.get(2)?,
        status: row.get(3)?,
        sort_order: row.get(4)?,
        connected_at: row.get(5)?,
        last_synced_at: row.get(6)?,
    })
}

fn unsubscribe_method_name(method: &UnsubscribeMethod) -> &'static str {
    match method {
        UnsubscribeMethod::OneClick => "oneClick",
        UnsubscribeMethod::Mailto => "mailto",
        UnsubscribeMethod::Web => "web",
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
        account_id: row.get(11)?,
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
        "INSERT INTO threads VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9, 0, 'default')",
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
            id, thread_id, sender, recipients_json, sent_at, body_html, body_text
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
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
        assert_eq!(result[0].id, "welcome");
        assert!(result[0].match_snippet.is_some());
    }

    #[test]
    fn phrase_search_requires_contiguous_words() {
        let database = database();
        let matches = database
            .search_threads(
                &SearchThreadsRequest {
                    query: "\"keeps your mail\"".into(),
                    limit: None,
                    offset: None,
                    include_archived: None,
                },
                None,
            )
            .unwrap();
        assert_eq!(matches[0].id, "welcome");

        let no_matches = database
            .search_threads(
                &SearchThreadsRequest {
                    query: "\"mail your keeps\"".into(),
                    limit: None,
                    offset: None,
                    include_archived: None,
                },
                None,
            )
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
                .upsert_gmail_thread(
                    "default",
                    &[NormalizedMessage {
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
                        unsubscribe: None,
                    }],
                )
                .unwrap();
        }

        let first_page = database
            .search_threads(
                &SearchThreadsRequest {
                    query: "unique-pagination-term".into(),
                    limit: Some(1),
                    offset: None,
                    include_archived: None,
                },
                None,
            )
            .unwrap();
        let second_page = database
            .search_threads(
                &SearchThreadsRequest {
                    query: "unique-pagination-term".into(),
                    limit: Some(1),
                    offset: Some(1),
                    include_archived: None,
                },
                None,
            )
            .unwrap();
        assert_eq!(first_page.len(), 1);
        assert_eq!(second_page.len(), 1);
        assert_ne!(first_page[0].id, second_page[0].id);
    }

    #[test]
    fn deleting_a_thread_also_removes_its_search_index_row() {
        let database = database();
        database
            .delete_gmail_thread("default", "demo-welcome")
            .unwrap();
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
    fn interrupted_full_sync_cannot_reuse_the_previous_cursor() {
        let database = database();
        database.finish_sync("default", "old-cursor").unwrap();
        database.begin_full_sync("default").unwrap();
        assert_eq!(database.cursor("default").unwrap(), None);
    }

    #[test]
    fn adopting_an_account_creates_it_and_rewrites_legacy_default_state() {
        let database = database();
        database
            .mutate_thread(&ThreadMutation::Star {
                thread_id: "welcome".into(),
                value: true,
            })
            .unwrap();

        let account = database.adopt_account("you@gmail.com").unwrap();
        assert_eq!(account.email, "you@gmail.com");
        assert_eq!(account.status, "connected");
        assert_eq!(account.sort_order, 0);

        let connection = database.connection().unwrap();
        let sync_account_id: String = connection
            .query_row(
                "SELECT account_id FROM sync_state WHERE account_id = 'you@gmail.com'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(sync_account_id, "you@gmail.com");
        let mutation_account_id: String = connection
            .query_row("SELECT account_id FROM mutations", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mutation_account_id, "you@gmail.com");
    }

    #[test]
    fn adopting_the_same_account_twice_does_not_duplicate_it() {
        let database = database();
        database.adopt_account("you@gmail.com").unwrap();
        database.adopt_account("you@gmail.com").unwrap();
        assert_eq!(database.list_accounts().unwrap().len(), 1);
    }

    #[test]
    fn accounts_get_increasing_sort_order_and_rotating_colors() {
        let database = database();
        let first = database.adopt_account("first@gmail.com").unwrap();
        let second = database.adopt_account("second@gmail.com").unwrap();
        assert_eq!(first.sort_order, 0);
        assert_eq!(second.sort_order, 1);
        assert_ne!(first.color, second.color);
    }

    #[test]
    fn removing_an_account_deletes_its_row() {
        let database = database();
        database.adopt_account("you@gmail.com").unwrap();
        database.remove_account("you@gmail.com").unwrap();
        assert!(database.list_accounts().unwrap().is_empty());
    }

    #[test]
    fn set_account_color_updates_an_existing_account_and_rejects_an_unknown_one() {
        let database = database();
        database.adopt_account("you@gmail.com").unwrap();
        database
            .set_account_color("you@gmail.com", "#123456")
            .unwrap();
        assert_eq!(
            database
                .get_account("you@gmail.com")
                .unwrap()
                .unwrap()
                .color,
            "#123456"
        );
        assert!(database
            .set_account_color("missing@gmail.com", "#123456")
            .is_err());
    }

    #[test]
    fn reorder_accounts_updates_sort_order_by_position() {
        let database = database();
        database.adopt_account("first@gmail.com").unwrap();
        database.adopt_account("second@gmail.com").unwrap();
        database
            .reorder_accounts(&["second@gmail.com".into(), "first@gmail.com".into()])
            .unwrap();
        let accounts = database.list_accounts().unwrap();
        assert_eq!(accounts[0].email, "second@gmail.com");
        assert_eq!(accounts[1].email, "first@gmail.com");
    }

    fn message(id: &str, thread_id: &str, date: &str, body: &str) -> NormalizedMessage {
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
        }
    }

    #[test]
    fn unsubscribe_metadata_is_exposed_and_attempts_are_recorded() {
        let database = database();
        let mut normalized = message(
            "newsletter-message",
            "newsletter",
            "2026-01-01T00:00:00Z",
            "body",
        );
        normalized.unsubscribe = Some(UnsubscribeMetadata {
            one_click_url: Some("https://lists.example/one-click".into()),
            mailto_url: Some("mailto:list@example.com?subject=unsubscribe".into()),
            web_url: Some("https://lists.example/preferences".into()),
            list_id: Some("news.example".into()),
        });
        database
            .upsert_gmail_thread("work@example.com", &[normalized])
            .unwrap();

        let detail = database.get_thread("work@example.com:newsletter").unwrap();
        let info = detail.messages[0].unsubscribe.as_ref().unwrap();
        assert_eq!(info.methods.len(), 3);
        assert_eq!(info.list_id.as_deref(), Some("news.example"));

        let target = database.begin_unsubscribe("newsletter-message").unwrap();
        assert!(matches!(target.method, UnsubscribeMethod::OneClick));
        assert_eq!(target.url, "https://lists.example/one-click");
        let pending: String = database
            .connection()
            .unwrap()
            .query_row(
                "SELECT state FROM unsubscribe_requests WHERE id = ?1",
                [&target.request_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending, "pending");

        database
            .finish_unsubscribe(&target.request_id, "succeeded", Some(204), None)
            .unwrap();
        let completed: (String, Option<i64>) = database
            .connection()
            .unwrap()
            .query_row(
                "SELECT state, http_status FROM unsubscribe_requests WHERE id = ?1",
                [&target.request_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(completed, ("succeeded".into(), Some(204)));
    }

    #[test]
    fn two_accounts_with_the_same_provider_thread_id_stay_fully_separate() {
        let database = database();
        database
            .upsert_gmail_thread(
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
            .upsert_gmail_thread(
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
            .delete_gmail_thread("work@example.com", "shared-id")
            .unwrap();
        let remaining = database.list_threads(None).unwrap();
        assert!(!remaining.iter().any(|t| t.id == work.id));
        assert!(remaining.iter().any(|t| t.id == personal.id));
    }

    #[test]
    fn full_sync_wipes_only_the_given_accounts_threads() {
        let database = database();
        database
            .upsert_gmail_thread(
                "work@example.com",
                &[message("m1", "t1", "2026-01-01T00:00:00Z", "body")],
            )
            .unwrap();
        database
            .upsert_gmail_thread(
                "personal@example.com",
                &[message("m2", "t2", "2026-01-01T00:00:00Z", "body")],
            )
            .unwrap();
        database.begin_full_sync("work@example.com").unwrap();
        let threads = database.list_threads(None).unwrap();
        assert!(!threads.iter().any(|t| t.id == "work@example.com:t1"));
        assert!(threads.iter().any(|t| t.id == "personal@example.com:t2"));
    }

    #[test]
    fn claim_mutations_only_claims_the_given_accounts_mutations() {
        let database = database();
        database
            .upsert_gmail_thread(
                "work@example.com",
                &[message("m1", "t1", "2026-01-01T00:00:00Z", "body")],
            )
            .unwrap();
        database
            .upsert_gmail_thread(
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
    fn list_threads_merges_by_default_and_filters_when_scoped() {
        let database = database();
        database
            .upsert_gmail_thread(
                "work@example.com",
                &[message("m1", "t1", "2026-01-01T00:00:00Z", "body")],
            )
            .unwrap();
        database
            .upsert_gmail_thread(
                "personal@example.com",
                &[message("m2", "t2", "2026-01-02T00:00:00Z", "body")],
            )
            .unwrap();

        let merged = database.list_threads(None).unwrap();
        assert!(merged.iter().any(|t| t.id == "work@example.com:t1"));
        assert!(merged.iter().any(|t| t.id == "personal@example.com:t2"));

        let scoped = database.list_threads(Some("work@example.com")).unwrap();
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].id, "work@example.com:t1");
        assert_eq!(scoped[0].account_id, "work@example.com");
    }

    #[test]
    fn list_all_mail_excludes_trash_but_keeps_archived() {
        let database = database();
        database
            .upsert_gmail_thread(
                "work@example.com",
                &[message("m1", "inbox", "2026-01-01T00:00:00Z", "body")],
            )
            .unwrap();
        database
            .upsert_gmail_thread(
                "work@example.com",
                &[message("m2", "archived", "2026-01-02T00:00:00Z", "body")],
            )
            .unwrap();
        database
            .upsert_gmail_thread(
                "work@example.com",
                &[message("m3", "trashed", "2026-01-03T00:00:00Z", "body")],
            )
            .unwrap();
        database
            .mutate_thread(&ThreadMutation::Archive {
                thread_id: "work@example.com:archived".into(),
                value: true,
            })
            .unwrap();
        database
            .mutate_thread(&ThreadMutation::Trash {
                thread_id: "work@example.com:trashed".into(),
                value: true,
            })
            .unwrap();

        let all_mail = database.list_all_mail(None).unwrap();
        let ids: Vec<_> = all_mail.iter().map(|t| t.id.as_str()).collect();
        assert!(ids.contains(&"work@example.com:inbox"));
        assert!(ids.contains(&"work@example.com:archived"));
        assert!(!ids.contains(&"work@example.com:trashed"));
    }

    #[test]
    fn list_trash_only_returns_trashed_threads() {
        let database = database();
        database
            .upsert_gmail_thread(
                "work@example.com",
                &[message("m1", "inbox", "2026-01-01T00:00:00Z", "body")],
            )
            .unwrap();
        database
            .upsert_gmail_thread(
                "work@example.com",
                &[message("m2", "trashed", "2026-01-02T00:00:00Z", "body")],
            )
            .unwrap();
        database
            .mutate_thread(&ThreadMutation::Trash {
                thread_id: "work@example.com:trashed".into(),
                value: true,
            })
            .unwrap();

        let trash = database.list_trash(None).unwrap();
        assert_eq!(trash.len(), 1);
        assert_eq!(trash[0].id, "work@example.com:trashed");

        let scoped = database.list_trash(Some("personal@example.com")).unwrap();
        assert!(scoped.is_empty());
    }
}
