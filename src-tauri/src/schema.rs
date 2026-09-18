//! Database schema and migration ownership.
//!
//! Connection lifecycle and locking belong to `db`; all changes to the shape
//! of persisted data belong here so the current schema and its upgrade path
//! are reviewed together.

use chrono::Utc;
use rusqlite::{params, Connection};

use crate::mime::GmailMessage;

/// Bumped alongside the last `if version < N` block in [`migrate`]. Read
/// before migrating so a pre-migration backup is only taken when a
/// migration is actually about to run.
pub(crate) const LATEST_VERSION: i64 = 19;

pub(crate) const INITIAL_SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
-- Durability/perf tradeoff: NORMAL is safe under WAL (never corrupts the
-- database) and only risks losing the most recent commit(s) on an OS crash
-- or power loss. Acceptable here since Gmail remains the source of truth
-- and the local cache is resyncable.
PRAGMA synchronous = NORMAL;
-- Unset defaults to 0 (immediate SQLITE_BUSY). Matters once more than one
-- connection can touch this file at a time (e.g. a backup connection).
PRAGMA busy_timeout = 5000;
-- Make the checkpoint threshold explicit rather than relying on whatever
-- SQLite's own default happens to be.
PRAGMA wal_autocheckpoint = 1000;
-- Bounds how large the WAL file can grow between checkpoints.
PRAGMA journal_size_limit = 67108864;

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

CREATE TABLE IF NOT EXISTS calendar_accounts (
    email TEXT PRIMARY KEY,
    connected_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS triage_events (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    sender_email TEXT NOT NULL,
    sender_domain TEXT NOT NULL,
    event_kind TEXT NOT NULL CHECK(event_kind IN ('open', 'close', 'disposition', 'restore', 'response')),
    context TEXT NOT NULL CHECK(context IN ('inbox', 'other')),
    action TEXT CHECK(action IS NULL OR action IN ('archive', 'trash')),
    opened INTEGER NOT NULL DEFAULT 0,
    dwell_ms INTEGER,
    scrolled INTEGER NOT NULL DEFAULT 0,
    batch INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS triage_events_account_sender
ON triage_events(account_id, sender_email, created_at);

CREATE INDEX IF NOT EXISTS triage_events_account_time
ON triage_events(account_id, created_at);

CREATE TABLE IF NOT EXISTS pinned_contacts (
    account_id TEXT NOT NULL,
    email TEXT NOT NULL,
    display_name TEXT,
    pinned_at TEXT NOT NULL,
    PRIMARY KEY (account_id, email)
);
"#;

fn error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn json<T: serde::Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value).map_err(error)
}

pub(crate) fn migrate(connection: &mut Connection) -> Result<(), String> {
    let tx = connection.transaction().map_err(error)?;
    let version: i64 = tx
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(error)?;
    if version < 1 {
        tx.execute_batch("CREATE TABLE IF NOT EXISTS message_metadata(id TEXT PRIMARY KEY, payload TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS compose_settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS drafts(id TEXT PRIMARY KEY, revision INTEGER NOT NULL, payload TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS outbox_messages(id TEXT PRIMARY KEY, draft_id TEXT NOT NULL, revision INTEGER NOT NULL, account TEXT NOT NULL, state TEXT NOT NULL, deadline INTEGER NOT NULL, payload TEXT NOT NULL, raw BLOB NOT NULL, error TEXT, provider_id TEXT, UNIQUE(draft_id, revision));
        PRAGMA user_version=1;").map_err(error)?;
    }
    if version < 2 {
        tx.execute_batch(
            "ALTER TABLE outbox_messages ADD COLUMN attempts INTEGER NOT NULL DEFAULT 0;
            ALTER TABLE outbox_messages ADD COLUMN last_attempt_at INTEGER;
            PRAGMA user_version=2;",
        )
        .map_err(error)?;
    }
    if version < 3 {
        tx.execute_batch(
            "ALTER TABLE threads ADD COLUMN trashed INTEGER NOT NULL DEFAULT 0;
            CREATE INDEX IF NOT EXISTS threads_trashed ON threads(trashed);
            PRAGMA user_version=3;",
        )
        .map_err(error)?;
    }
    if version < 4 {
        tx.execute_batch(
            "ALTER TABLE threads ADD COLUMN account_id TEXT NOT NULL DEFAULT 'default';
            CREATE UNIQUE INDEX IF NOT EXISTS threads_account_provider_unique
                ON threads(account_id, provider_thread_id);
            PRAGMA user_version=4;",
        )
        .map_err(error)?;
    }
    if version < 5 {
        tx.execute_batch(
            "ALTER TABLE messages ADD COLUMN unsubscribe_json TEXT;
            CREATE TABLE IF NOT EXISTS unsubscribe_requests(
                id TEXT PRIMARY KEY,
                message_id TEXT NOT NULL,
                thread_id TEXT NOT NULL,
                method TEXT NOT NULL,
                state TEXT NOT NULL CHECK(state IN ('pending', 'succeeded', 'opened', 'failed')),
                http_status INTEGER,
                created_at TEXT NOT NULL,
                completed_at TEXT,
                last_error TEXT
            );
            CREATE INDEX IF NOT EXISTS unsubscribe_requests_message
                ON unsubscribe_requests(message_id, created_at);
            PRAGMA user_version=5;",
        )
        .map_err(error)?;
    }
    if version < 6 {
        tx.execute_batch(
            "ALTER TABLE messages ADD COLUMN unread INTEGER NOT NULL DEFAULT 0;
            PRAGMA user_version=6;",
        )
        .map_err(error)?;
    }
    if version < 7 {
        tx.execute_batch(
            "ALTER TABLE threads ADD COLUMN summary TEXT;
            ALTER TABLE threads ADD COLUMN summary_generated_at TEXT;
            PRAGMA user_version=7;",
        )
        .map_err(error)?;
    }
    if version < 8 {
        tx.execute_batch(
            "ALTER TABLE threads ADD COLUMN has_attachments INTEGER NOT NULL DEFAULT 0;
            ALTER TABLE messages ADD COLUMN attachments_json TEXT NOT NULL DEFAULT '[]';
            PRAGMA user_version=8;",
        )
        .map_err(error)?;
    }
    if version < 9 {
        let cached_payloads = {
            let mut statement = tx
                .prepare("SELECT id, payload FROM message_metadata")
                .map_err(error)?;
            let rows = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(error)?;
            rows
        };
        for (id, payload) in cached_payloads {
            let Ok(message) = serde_json::from_str::<GmailMessage>(&payload) else {
                continue;
            };
            let Ok(normalized) = crate::mime::normalize(&message) else {
                continue;
            };
            tx.execute(
                "UPDATE messages SET attachments_json=?1 WHERE id=?2",
                params![json(&normalized.attachments)?, id],
            )
            .map_err(error)?;
        }
        tx.execute(
            "UPDATE threads SET has_attachments = EXISTS(
                SELECT 1 FROM messages m, json_each(m.attachments_json) attachment
                WHERE m.thread_id = threads.id
                  AND COALESCE(json_extract(attachment.value, '$.inline'), 0) = 0
            )",
            [],
        )
        .map_err(error)?;
        tx.pragma_update(None, "user_version", 9).map_err(error)?;
    }
    if version < 10 {
        tx.execute_batch(
            "ALTER TABLE threads ADD COLUMN last_received_at TEXT NOT NULL DEFAULT '';",
        )
        .map_err(error)?;
        tx.execute(
            "UPDATE threads SET last_received_at = COALESCE(
                (SELECT MAX(m.sent_at) FROM messages m
                 JOIN message_metadata mm ON mm.id = m.id
                 WHERE m.thread_id = threads.id
                   AND NOT EXISTS (
                       SELECT 1 FROM json_each(json_extract(mm.payload, '$.labelIds')) label
                       WHERE label.value = 'SENT'
                   )),
                last_message_at
            )",
            [],
        )
        .map_err(error)?;
        tx.execute_batch(
            "DROP INDEX IF EXISTS threads_inbox_order;
            CREATE INDEX IF NOT EXISTS threads_inbox_order ON threads(archived, last_received_at DESC);
            DROP INDEX IF EXISTS threads_account_mailbox_order;
            CREATE INDEX IF NOT EXISTS threads_account_mailbox_order
                ON threads(account_id, trashed, archived, last_received_at DESC);
            DROP INDEX IF EXISTS threads_mailbox_order;
            CREATE INDEX IF NOT EXISTS threads_mailbox_order
                ON threads(trashed, archived, last_received_at DESC);
            PRAGMA user_version=10;",
        )
        .map_err(error)?;
    }
    if version < 11 {
        tx.execute_batch(
            "ALTER TABLE messages ADD COLUMN body_html_z BLOB;
            ALTER TABLE messages ADD COLUMN body_text_z BLOB;
            PRAGMA user_version=11;",
        )
        .map_err(error)?;
    }
    if version < 12 {
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS split_inboxes (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                match_kind TEXT NOT NULL CHECK(match_kind IN ('domain', 'label', 'pattern')),
                match_value TEXT NOT NULL,
                sort_order INTEGER NOT NULL,
                created_at TEXT NOT NULL
            );
            PRAGMA user_version=12;",
        )
        .map_err(error)?;
    }
    if version < 13 {
        // A split inbox now belongs to one account rather than applying
        // across all of them. Existing rows (created before this column
        // existed) are assigned to whichever account sorts first, since
        // there's no recorded owner to recover; the user can delete and
        // recreate a rule under the right account if that guess is wrong.
        tx.execute_batch(
            "ALTER TABLE split_inboxes ADD COLUMN account_id TEXT NOT NULL DEFAULT '';",
        )
        .map_err(error)?;
        tx.execute(
            "UPDATE split_inboxes SET account_id = (SELECT email FROM accounts ORDER BY sort_order LIMIT 1)
             WHERE account_id = ''",
            [],
        )
        .map_err(error)?;
        tx.pragma_update(None, "user_version", 13).map_err(error)?;
    }
    if version < 14 {
        tx.execute_batch(
            "ALTER TABLE mutations ADD COLUMN target_message_id TEXT;
            UPDATE mutations
            SET target_message_id = CASE
                WHEN kind IN ('star', 'label') THEN (
                    SELECT id FROM messages
                    WHERE thread_id = mutations.thread_id
                    ORDER BY sent_at ASC, id ASC LIMIT 1
                )
                WHEN kind = 'read'
                     AND json_extract(payload_json, '$.value') = 0 THEN (
                    SELECT id FROM messages
                    WHERE thread_id = mutations.thread_id
                    ORDER BY sent_at DESC, id DESC LIMIT 1
                )
            END
            WHERE state IN ('pending', 'running');
            PRAGMA user_version=14;",
        )
        .map_err(error)?;
    }
    if version < 15 {
        tx.execute_batch(
            "ALTER TABLE sync_state ADD COLUMN last_reconciled_at TEXT;
            PRAGMA user_version=15;",
        )
        .map_err(error)?;
    }
    if version < 16 {
        tx.execute_batch(
            "CREATE TABLE sync_recovery (
                account_id TEXT PRIMARY KEY,
                history_id TEXT NOT NULL
            );
            CREATE TABLE sync_recovery_threads (
                account_id TEXT NOT NULL,
                provider_thread_id TEXT NOT NULL,
                PRIMARY KEY (account_id, provider_thread_id)
            );
            PRAGMA user_version=16;",
        )
        .map_err(error)?;
    }
    if version < 17 {
        tx.execute_batch(
            "ALTER TABLE mutations ADD COLUMN next_attempt_at TEXT;
            DROP INDEX IF EXISTS mutations_pending;
            CREATE INDEX mutations_pending
                ON mutations(state, next_attempt_at, created_at);
            PRAGMA user_version=17;",
        )
        .map_err(error)?;
    }
    if version < 18 {
        tx.execute_batch(
            "CREATE TABLE quarantined_messages (
                account_id TEXT NOT NULL,
                provider_thread_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                error TEXT NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY (account_id, message_id)
            );
            CREATE INDEX quarantined_messages_account_time
                ON quarantined_messages(account_id, created_at DESC);
            PRAGMA user_version=18;",
        )
        .map_err(error)?;
    }
    if version < 19 {
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS calendar_accounts (
                email TEXT PRIMARY KEY,
                connected_at TEXT NOT NULL
            );
            PRAGMA user_version=19;",
        )
        .map_err(error)?;
    }
    tx.commit().map_err(error)?;

    connection.execute("UPDATE outbox_messages SET state='uncertain', error='Application stopped during delivery. Check sent mail before sending again.' WHERE state='sending'", []).map_err(error)?;
    connection
        .execute(
            "UPDATE outbox_messages SET deadline=?1 WHERE state='undo_pending'",
            [Utc::now().timestamp_millis() + 10_000],
        )
        .map_err(error)?;
    Ok(())
}
