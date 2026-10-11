//! Database schema and migration ownership.
//!
//! Connection lifecycle and locking belong to `db`; all changes to the shape
//! of persisted data belong here so the current schema and its upgrade path
//! are reviewed together.

use chrono::Utc;
use rusqlite::{params, Connection};

use crate::mime::RawMessage;

/// Bumped alongside the last `if version < N` block in [`migrate`]. Read
/// before migrating so a pre-migration backup is only taken when a
/// migration is actually about to run.
pub(crate) const LATEST_VERSION: i64 = 62;

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

CREATE TABLE IF NOT EXISTS accounts (
    email TEXT PRIMARY KEY,
    display_name TEXT,
    color TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('connected', 'needs_reauth')),
    sort_order INTEGER NOT NULL,
    connected_at TEXT NOT NULL,
    last_synced_at TEXT
);

-- `default` is only the pre-connect placeholder. Recreating it after a real
-- account has been adopted makes the next identity refresh collide with that
-- account's existing sync-state primary key.
INSERT OR IGNORE INTO sync_state(account_id)
SELECT 'default' WHERE NOT EXISTS (SELECT 1 FROM accounts);

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

fn has_column(transaction: &rusqlite::Transaction<'_>, table: &str, column: &str) -> Result<bool, String> {
    let mut statement = transaction.prepare(&format!("PRAGMA table_info({table})")).map_err(error)?;
    let names = statement.query_map([], |row| row.get::<_, String>(1)).map_err(error)?;
    for name in names {
        if name.map_err(error)? == column { return Ok(true); }
    }
    Ok(false)
}

fn json<T: serde::Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value).map_err(error)
}

fn table_exists_in(tx: &rusqlite::Transaction, table: &str) -> Result<bool, String> {
    tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [table],
        |row| row.get(0),
    )
    .map_err(error)
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
            let Ok(message) = serde_json::from_str::<RawMessage>(&payload) else {
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
    if version < 20 {
        tx.execute_batch(
            "ALTER TABLE calendar_accounts
                ADD COLUMN selection_initialized INTEGER NOT NULL DEFAULT 0;
            CREATE TABLE calendar_selections (
                account_id TEXT NOT NULL REFERENCES calendar_accounts(email) ON DELETE CASCADE,
                calendar_id TEXT NOT NULL,
                PRIMARY KEY (account_id, calendar_id)
            );
            PRAGMA user_version=20;",
        )
        .map_err(error)?;
    }
    if version < 21 {
        // Every account created so far authenticated through Gmail, so the
        // default backfills correctly with no data to reconcile.
        tx.execute_batch(
            "ALTER TABLE accounts ADD COLUMN provider TEXT NOT NULL DEFAULT 'gmail';
            PRAGMA user_version=21;",
        )
        .map_err(error)?;
    }
    if version < 22 {
        tx.execute_batch(
            "CREATE TABLE tasks (
                id TEXT PRIMARY KEY,
                account_id TEXT NOT NULL,
                thread_id TEXT NOT NULL,
                source_message_id TEXT,
                subject_snapshot TEXT NOT NULL,
                title TEXT NOT NULL,
                notes TEXT,
                kind TEXT NOT NULL CHECK(kind IN ('action', 'follow_up', 'waiting_for')),
                due_kind TEXT NOT NULL CHECK(due_kind IN ('none', 'date', 'datetime')),
                due_value TEXT,
                time_zone TEXT,
                repeat_interval_days INTEGER,
                status TEXT NOT NULL CHECK(status IN ('open', 'completed', 'cancelled')),
                completion_source TEXT CHECK(completion_source IS NULL OR completion_source IN ('user', 'reply', 'external')),
                evidence_text TEXT,
                wait_after TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                completed_at TEXT
            );
            CREATE INDEX tasks_status_due ON tasks(status, due_value, updated_at);
            CREATE INDEX tasks_account_status ON tasks(account_id, status, updated_at);
            CREATE INDEX tasks_thread ON tasks(thread_id, status);
            PRAGMA user_version=22;",
        )
        .map_err(error)?;
    }
    if version < 23 {
        tx.execute_batch(
            "ALTER TABLE outbox_messages ADD COLUMN archive_on_send INTEGER NOT NULL DEFAULT 0;
            PRAGMA user_version=23;",
        )
        .map_err(error)?;
    }
    if version < 24 {
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS snippets (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                body TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            PRAGMA user_version=24;",
        )
        .map_err(error)?;
    }
    if version < 25 {
        // Tasks originally required an email thread. Rebuild the table so a
        // task created directly from the Tasks workspace can remain entirely
        // local, without a synthetic thread or misleading subject snapshot.
        tx.execute_batch(
            "DROP INDEX tasks_status_due;
            DROP INDEX tasks_account_status;
            DROP INDEX tasks_thread;
            ALTER TABLE tasks RENAME TO tasks_v24;
            CREATE TABLE tasks (
                id TEXT PRIMARY KEY,
                account_id TEXT NOT NULL,
                thread_id TEXT,
                source_message_id TEXT,
                subject_snapshot TEXT,
                title TEXT NOT NULL,
                notes TEXT,
                kind TEXT NOT NULL CHECK(kind IN ('action', 'follow_up', 'waiting_for')),
                due_kind TEXT NOT NULL CHECK(due_kind IN ('none', 'date', 'datetime')),
                due_value TEXT,
                time_zone TEXT,
                repeat_interval_days INTEGER,
                status TEXT NOT NULL CHECK(status IN ('open', 'completed', 'cancelled')),
                completion_source TEXT CHECK(completion_source IS NULL OR completion_source IN ('user', 'reply', 'external')),
                evidence_text TEXT,
                wait_after TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                completed_at TEXT
            );
            INSERT INTO tasks SELECT * FROM tasks_v24;
            DROP TABLE tasks_v24;
            CREATE INDEX tasks_status_due ON tasks(status, due_value, updated_at);
            CREATE INDEX tasks_account_status ON tasks(account_id, status, updated_at);
            CREATE INDEX tasks_thread ON tasks(thread_id, status);
            PRAGMA user_version=25;",
        )
        .map_err(error)?;
    }
    if version < 26 {
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS cloud_account_state (
                singleton INTEGER PRIMARY KEY CHECK(singleton=1),
                user_id TEXT,
                email TEXT,
                display_name TEXT,
                avatar_url TEXT,
                device_id TEXT NOT NULL,
                cursor INTEGER NOT NULL DEFAULT 0,
                enrollment_confirmed INTEGER NOT NULL DEFAULT 0,
                sync_entitled INTEGER NOT NULL DEFAULT 0,
                last_successful_sync TEXT,
                last_error TEXT
            );
            CREATE TABLE IF NOT EXISTS cloud_sync_metadata (
                entity_type TEXT NOT NULL,
                entity_id TEXT NOT NULL,
                server_version INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(entity_type, entity_id)
            );
            CREATE TABLE IF NOT EXISTS cloud_sync_outbox (
                operation_id TEXT PRIMARY KEY,
                entity_type TEXT NOT NULL,
                entity_id TEXT NOT NULL,
                base_version INTEGER NOT NULL,
                changed_fields TEXT NOT NULL,
                patch TEXT,
                deleted INTEGER NOT NULL DEFAULT 0,
                local_sequence INTEGER NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS cloud_sync_outbox_sequence ON cloud_sync_outbox(local_sequence);
            CREATE TABLE IF NOT EXISTS cloud_sync_conflicts (
                id TEXT PRIMARY KEY,
                entity_type TEXT NOT NULL,
                entity_id TEXT NOT NULL,
                current_version INTEGER NOT NULL,
                overlapping_fields TEXT NOT NULL,
                cloud_payload TEXT,
                device_patch TEXT,
                device_deleted INTEGER NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS cloud_preferences (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            INSERT OR IGNORE INTO cloud_account_state(singleton,device_id)
              VALUES(1, lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' || substr(lower(hex(randomblob(2))),2) || '-a' || substr(lower(hex(randomblob(2))),2) || '-' || lower(hex(randomblob(6))));
            ",
        )
        .map_err(error)?;
        if !has_column(&tx, "snippets", "updated_at")? {
            tx.execute("ALTER TABLE snippets ADD COLUMN updated_at TEXT", []).map_err(error)?;
        }
        tx.execute("UPDATE snippets SET updated_at=created_at WHERE updated_at IS NULL", []).map_err(error)?;
        if !has_column(&tx, "split_inboxes", "updated_at")? {
            tx.execute("ALTER TABLE split_inboxes ADD COLUMN updated_at TEXT", []).map_err(error)?;
        }
        tx.execute("UPDATE split_inboxes SET updated_at=created_at WHERE updated_at IS NULL", []).map_err(error)?;
        if !has_column(&tx, "calendar_accounts", "status")? {
            tx.execute("ALTER TABLE calendar_accounts ADD COLUMN status TEXT NOT NULL DEFAULT 'connected' CHECK(status IN ('connected','needs_reauth'))", []).map_err(error)?;
        }
        tx.pragma_update(None, "user_version", 26).map_err(error)?;
    }
    if version < 27 {
        // The pluggable replicated-sync engine's local operation graph,
        // logical events, transports, and delivery ledger, added in one
        // complete migration (see `replicated_sync.rs`). Entirely inert
        // until `THREESTRANDS_REPLICATED_SYNC` is set: no code writes to
        // these tables otherwise. Reserve a new schema number for every
        // later change to this graph rather than editing this block once
        // it has shipped.
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS sync_spaces (
                id TEXT PRIMARY KEY,
                active_epoch INTEGER NOT NULL DEFAULT 0,
                -- NULL until the Phase 5 key hierarchy generates a real
                -- recovery keypair; the plan's suggested schema treats this
                -- as required, but nothing can populate it yet.
                recovery_public_key BLOB,
                lamport INTEGER NOT NULL DEFAULT 0,
                enabled INTEGER NOT NULL DEFAULT 0,
                last_error TEXT
            );
            CREATE TABLE IF NOT EXISTS sync_devices (
                device_id TEXT PRIMARY KEY,
                public_key BLOB,
                status TEXT NOT NULL DEFAULT 'active' CHECK(status IN ('active','revoked')),
                added_by_operation TEXT,
                revoked_by_operation TEXT
            );
            CREATE TABLE IF NOT EXISTS sync_events (
                event_id TEXT PRIMARY KEY,
                epoch INTEGER NOT NULL,
                device_id TEXT NOT NULL,
                device_sequence INTEGER NOT NULL,
                lamport INTEGER NOT NULL,
                state TEXT NOT NULL DEFAULT 'recorded' CHECK(state IN ('recorded','sealed')),
                created_at TEXT NOT NULL,
                UNIQUE(device_id, device_sequence)
            );
            CREATE TABLE IF NOT EXISTS sync_objects (
                cid TEXT PRIMARY KEY,
                event_id TEXT,
                object_kind TEXT NOT NULL,
                chunk_index INTEGER NOT NULL,
                chunk_count INTEGER NOT NULL,
                bytes BLOB NOT NULL
            );
            CREATE TABLE IF NOT EXISTS sync_operations (
                operation_id TEXT PRIMARY KEY,
                event_id TEXT NOT NULL REFERENCES sync_events(event_id) ON DELETE CASCADE,
                entity_type TEXT NOT NULL,
                entity_id TEXT NOT NULL,
                field TEXT NOT NULL,
                value TEXT,
                winner_stamp BLOB NOT NULL
            );
            CREATE INDEX IF NOT EXISTS sync_operations_entity
                ON sync_operations(entity_type, entity_id, field);
            CREATE TABLE IF NOT EXISTS sync_operation_parents (
                operation_id TEXT NOT NULL,
                parent_operation_id TEXT NOT NULL,
                PRIMARY KEY(operation_id, parent_operation_id)
            );
            CREATE INDEX IF NOT EXISTS sync_operation_parents_by_parent
                ON sync_operation_parents(parent_operation_id);
            CREATE TABLE IF NOT EXISTS sync_field_frontier (
                entity_type TEXT NOT NULL,
                entity_id TEXT NOT NULL,
                field TEXT NOT NULL,
                operation_id TEXT NOT NULL,
                PRIMARY KEY(entity_type, entity_id, field, operation_id)
            );
            CREATE TABLE IF NOT EXISTS sync_transports (
                instance_id TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                account_id TEXT,
                required INTEGER NOT NULL DEFAULT 1,
                enabled INTEGER NOT NULL DEFAULT 0,
                cursor TEXT,
                last_success_at TEXT,
                last_error TEXT
            );
            CREATE TABLE IF NOT EXISTS sync_deliveries (
                cid TEXT NOT NULL,
                transport_instance_id TEXT NOT NULL,
                state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','delivered','failed')),
                remote_id TEXT,
                attempts INTEGER NOT NULL DEFAULT 0,
                retry_at TEXT,
                last_error TEXT,
                PRIMARY KEY(cid, transport_instance_id)
            );
            PRAGMA user_version=27;",
        )
        .map_err(error)?;
    }
    if version < 28 {
        // Versioned, non-secret adapter config (a folder path, an RPC base
        // URL) for each configured replicated-sync transport instance. Any
        // credential lives in the OS keychain instead, never here.
        if !has_column(&tx, "sync_transports", "config_json")? {
            tx.execute("ALTER TABLE sync_transports ADD COLUMN config_json TEXT", [])
                .map_err(error)?;
        }
        tx.pragma_update(None, "user_version", 28).map_err(error)?;
    }
    if version < 29 {
        // Phase 5's key hierarchy and device enrollment (see
        // `enrollment.rs`). `sync_spaces.recovery_public_key` (added in v27)
        // becomes the recovery Ed25519 authorization public key; this
        // migration adds its X25519 counterpart plus every device's X25519
        // public key, an epoch-activation history (secret epoch key bytes
        // themselves live only in the OS keychain, never here), a table
        // tracking this device's outgoing/incoming enrollment requests, and
        // a dedup cache for the enrollment/rotation object scan sweep.
        if !has_column(&tx, "sync_spaces", "recovery_x25519_public")? {
            tx.execute("ALTER TABLE sync_spaces ADD COLUMN recovery_x25519_public BLOB", [])
                .map_err(error)?;
        }
        if !has_column(&tx, "sync_devices", "x25519_public")? {
            tx.execute("ALTER TABLE sync_devices ADD COLUMN x25519_public BLOB", [])
                .map_err(error)?;
        }
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS sync_epoch_history (
                key_epoch INTEGER PRIMARY KEY,
                activated_at TEXT NOT NULL,
                source_cid TEXT
            );
            CREATE TABLE IF NOT EXISTS replicated_sync_enrollment_requests (
                request_id TEXT PRIMARY KEY,
                direction TEXT NOT NULL CHECK(direction IN ('outgoing','incoming')),
                device_id TEXT,
                ed25519_public BLOB,
                x25519_public BLOB,
                fingerprint TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','approved','rejected','staged','completed')),
                pending_grant_cbor BLOB,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS sync_control_objects_seen (
                cid TEXT PRIMARY KEY,
                object_kind TEXT NOT NULL
            );
            PRAGMA user_version=29;",
        )
        .map_err(error)?;
    }
    if version < 30 {
        // `sync_devices` previously had no way to distinguish this
        // device's own row from a trusted peer's once enrollment added
        // more than one — `ensure_space_and_device`'s "the device_id" was
        // just "the first row," which is only correct with exactly one
        // row. Mark the row explicitly. A pre-existing single-device
        // database's lone row is unambiguously self.
        if !has_column(&tx, "sync_devices", "is_self")? {
            tx.execute("ALTER TABLE sync_devices ADD COLUMN is_self INTEGER NOT NULL DEFAULT 0", [])
                .map_err(error)?;
        }
        tx.execute(
            "UPDATE sync_devices SET is_self=1 WHERE is_self=0 AND (SELECT COUNT(*) FROM sync_devices) = 1",
            [],
        )
        .map_err(error)?;
        tx.pragma_update(None, "user_version", 30).map_err(error)?;
    }
    if version < 31 {
        // The retired Three Strands account service's session, outbox,
        // server-version, and conflict tables have no reader left. Only
        // materialized portable preferences are still in use; they move to
        // a name that no longer implies a cloud service.
        let has_legacy_preferences: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='cloud_preferences')",
                [],
                |row| row.get(0),
            )
            .map_err(error)?;
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS synced_preferences (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );",
        )
        .map_err(error)?;
        if has_legacy_preferences {
            tx.execute_batch(
                "INSERT OR IGNORE INTO synced_preferences(key,value,updated_at)
                   SELECT key,value,updated_at FROM cloud_preferences;
                 DROP TABLE cloud_preferences;",
            )
            .map_err(error)?;
        }
        tx.execute_batch(
            "DROP INDEX IF EXISTS cloud_sync_outbox_sequence;
             DROP TABLE IF EXISTS cloud_sync_outbox;
             DROP TABLE IF EXISTS cloud_sync_metadata;
             DROP TABLE IF EXISTS cloud_sync_conflicts;
             DROP TABLE IF EXISTS cloud_account_state;",
        )
        .map_err(error)?;
        tx.pragma_update(None, "user_version", 31).map_err(error)?;
    }
    if version < 32 {
        // The roster cache stores device names. Shared names travel as
        // reserved fields on the existing Preferences entity, preserving
        // compatibility with clients that predate this table.
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS sync_device_labels (
                device_id TEXT PRIMARY KEY,
                label TEXT NOT NULL
            );",
        )
        .map_err(error)?;
        tx.pragma_update(None, "user_version", 32).map_err(error)?;
    }
    if version < 33 {
        // INITIAL_SCHEMA used to recreate this pre-connect placeholder on
        // every launch. Once an account catalog exists it is not a mailbox,
        // and adopting the real account would collide with its sync_state
        // primary key.
        tx.execute(
            "DELETE FROM sync_state
             WHERE account_id = 'default'
               AND EXISTS (SELECT 1 FROM accounts)",
            [],
        )
        .map_err(error)?;
        tx.pragma_update(None, "user_version", 33).map_err(error)?;
    }
    if version < 34 {
        // Join codes (see `enrollment/join_codes.rs`). Invitations this
        // device created, joined with, or observed from another group
        // member — never the invite secret or the code text — and the
        // redemptions published against them.
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS replicated_sync_invitations (
                invitation_cid TEXT PRIMARY KEY,
                direction TEXT NOT NULL,
                status TEXT NOT NULL,
                inviter_device_id TEXT NOT NULL,
                inviter_name TEXT,
                invite_ed25519_public BLOB,
                created_at TEXT NOT NULL,
                expires_at_ms INTEGER NOT NULL,
                redeemed_by_device_id TEXT,
                redemption_cid TEXT,
                object_deleted INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS replicated_sync_invitation_redemptions (
                redemption_cid TEXT PRIMARY KEY,
                invitation_cid TEXT NOT NULL,
                inviter_device_id TEXT,
                device_id TEXT NOT NULL,
                ed25519_public BLOB NOT NULL,
                x25519_public BLOB NOT NULL,
                device_name TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL,
                state TEXT NOT NULL,
                notice_dismissed INTEGER NOT NULL DEFAULT 0,
                received_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS replicated_sync_invitation_redemptions_invitation
                ON replicated_sync_invitation_redemptions(invitation_cid);",
        )
        .map_err(error)?;
        tx.pragma_update(None, "user_version", 34).map_err(error)?;
    }
    if version < 35 {
        // Replicated-sync protocol version 2 (causal vectors, v2 heads,
        // per-kind signatures). Version 1 groups can't be joined or read by
        // this build, so a device that belonged to one leaves it here, the
        // way "Leave this sync group" does: local data, connectors, and the
        // beta toggle stay, and the next sync re-records local entities
        // for whichever new group this device creates or joins. The OS
        // keychain can't be reached from a migration, so the highest epoch
        // to forget is recorded for the sync loop to clean up.
        // Enrolled in a group, or at least recorded local changes for one.
        let was_enrolled: bool = tx
            .query_row("SELECT EXISTS(SELECT 1 FROM sync_epoch_history)", [], |row| row.get(0))
            .map_err(error)?;
        let had_graph: bool = table_exists_in(&tx, "sync_events")?
            && tx
                .query_row("SELECT EXISTS(SELECT 1 FROM sync_events)", [], |row| row.get(0))
                .map_err(error)?;
        let highest_epoch: i64 = tx
            .query_row(
                "SELECT MAX(COALESCE((SELECT MAX(key_epoch) FROM sync_epoch_history), 0),
                            COALESCE((SELECT MAX(active_epoch) FROM sync_spaces), 0))",
                [],
                |row| row.get(0),
            )
            .map_err(error)?;
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS sync_notices (
                kind TEXT PRIMARY KEY,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS sync_pending_keychain_cleanup (
                id INTEGER PRIMARY KEY CHECK(id = 1),
                highest_epoch INTEGER NOT NULL
            );",
        )
        .map_err(error)?;
        if was_enrolled {
            tx.execute(
                "INSERT OR REPLACE INTO sync_pending_keychain_cleanup(id, highest_epoch) VALUES (1, ?1)",
                [highest_epoch],
            )
            .map_err(error)?;
            tx.execute(
                "INSERT OR IGNORE INTO sync_notices(kind, created_at) VALUES ('protocol_reset', datetime('now'))",
                [],
            )
            .map_err(error)?;
        }
        if was_enrolled || had_graph {
            tx.execute_batch(
                "DELETE FROM sync_deliveries;
                 DELETE FROM sync_field_frontier;
                 DELETE FROM sync_operation_parents;
                 DELETE FROM sync_objects;
                 DELETE FROM sync_epoch_history;
                 DELETE FROM replicated_sync_enrollment_requests;
                 DELETE FROM replicated_sync_invitation_redemptions;
                 DELETE FROM replicated_sync_invitations;
                 DELETE FROM sync_control_objects_seen;
                 DELETE FROM sync_device_labels;
                 DELETE FROM sync_devices;
                 UPDATE sync_spaces SET active_epoch=0, lamport=0, recovery_public_key=NULL, recovery_x25519_public=NULL, last_error=NULL;",
            )
            .map_err(error)?;
        }
        // The graph tables are rebuilt either way: their shape changed.
        tx.execute_batch(
            "DROP INDEX IF EXISTS sync_operations_entity;
             DROP TABLE IF EXISTS sync_operations;
             DROP TABLE IF EXISTS sync_events;
             CREATE TABLE sync_events (
                event_id TEXT PRIMARY KEY,
                epoch INTEGER NOT NULL,
                device_id TEXT NOT NULL,
                device_sequence INTEGER NOT NULL,
                lamport INTEGER NOT NULL,
                state TEXT NOT NULL DEFAULT 'recorded' CHECK(state IN ('recorded','sealed')),
                created_at TEXT NOT NULL,
                -- JSON [[device_id_hex, sequence], ...]: the event's causal
                -- vector, set once it is sealed (local) or applied (remote).
                causal_vector TEXT,
                UNIQUE(device_id, device_sequence)
            );
            -- An operation comes from exactly one event or, once snapshots
            -- exist, one snapshot. Deleting an event row never deletes its
            -- operations: an operation that still holds a field's current
            -- value must outlive its event once history is compacted.
            CREATE TABLE sync_operations (
                operation_id TEXT PRIMARY KEY,
                event_id TEXT,
                snapshot_id TEXT,
                entity_type TEXT NOT NULL,
                entity_id TEXT NOT NULL,
                field TEXT NOT NULL,
                value TEXT,
                winner_stamp BLOB NOT NULL,
                CHECK((event_id IS NULL) != (snapshot_id IS NULL))
            );
            CREATE INDEX sync_operations_entity ON sync_operations(entity_type, entity_id, field);
            CREATE INDEX sync_operations_event ON sync_operations(event_id);
            -- Per device: how much of its feed is applied here (a
            -- contiguous prefix), how much of that is causally closed
            -- (this device's ack), and what its latest head said.
            CREATE TABLE IF NOT EXISTS sync_device_progress (
                device_id TEXT PRIMARY KEY,
                applied_sequence INTEGER NOT NULL DEFAULT 0,
                progress_sequence INTEGER NOT NULL DEFAULT 0,
                last_event_at TEXT,
                last_head_seen_at_ms INTEGER,
                last_head_published_at_ms INTEGER,
                ack_json TEXT
            );
            -- What this device last published as its head on each
            -- transport, so an unchanged head is republished only when the
            -- heartbeat is due.
            CREATE TABLE IF NOT EXISTS sync_head_publications (
                transport_instance_id TEXT PRIMARY KEY,
                head_content TEXT NOT NULL,
                published_at_ms INTEGER NOT NULL
            );",
        )
        .map_err(error)?;
        tx.pragma_update(None, "user_version", 35).map_err(error)?;
    }
    if version < 36 {
        // Replicated-sync protocol version 3: devices replicate whole replica
        // states instead of an event log. A device in a version 2 group
        // leaves it, exactly as version 35 left version 1 groups: local
        // data, connectors, and the beta toggle stay, the user is told once,
        // and the sync loop forgets the old keys.
        let was_enrolled: bool = tx
            .query_row("SELECT EXISTS(SELECT 1 FROM sync_epoch_history)", [], |row| row.get(0))
            .map_err(error)?;
        let had_graph: bool = table_exists_in(&tx, "sync_events")?
            && tx
                .query_row("SELECT EXISTS(SELECT 1 FROM sync_events)", [], |row| row.get(0))
                .map_err(error)?;
        let highest_epoch: i64 = tx
            .query_row(
                "SELECT MAX(COALESCE((SELECT MAX(key_epoch) FROM sync_epoch_history), 0),
                            COALESCE((SELECT MAX(active_epoch) FROM sync_spaces), 0))",
                [],
                |row| row.get(0),
            )
            .map_err(error)?;
        if was_enrolled {
            tx.execute(
                "INSERT OR REPLACE INTO sync_pending_keychain_cleanup(id, highest_epoch) VALUES (1, ?1)",
                [highest_epoch],
            )
            .map_err(error)?;
            tx.execute(
                "INSERT OR IGNORE INTO sync_notices(kind, created_at) VALUES ('protocol_reset', datetime('now'))",
                [],
            )
            .map_err(error)?;
        }
        if was_enrolled || had_graph {
            tx.execute_batch(
                "DELETE FROM sync_deliveries;
                 DELETE FROM sync_epoch_history;
                 DELETE FROM replicated_sync_enrollment_requests;
                 DELETE FROM replicated_sync_invitation_redemptions;
                 DELETE FROM replicated_sync_invitations;
                 DELETE FROM sync_control_objects_seen;
                 DELETE FROM sync_device_labels;
                 DELETE FROM sync_devices;
                 DELETE FROM sync_head_publications;
                 UPDATE sync_spaces SET active_epoch=0, lamport=0, recovery_public_key=NULL, recovery_x25519_public=NULL, last_error=NULL;",
            )
            .map_err(error)?;
        }
        tx.execute_batch(
            "DROP INDEX IF EXISTS sync_operations_entity;
             DROP INDEX IF EXISTS sync_operations_event;
             DROP INDEX IF EXISTS sync_operation_parents_by_parent;
             DROP TABLE IF EXISTS sync_operations;
             DROP TABLE IF EXISTS sync_operation_parents;
             DROP TABLE IF EXISTS sync_field_frontier;
             DROP TABLE IF EXISTS sync_events;
             DROP TABLE IF EXISTS sync_device_progress;",
        )
        .map_err(error)?;
        // `sync_objects` is rebuilt only while it still has the event-log
        // shape, so running this again never drops this device's current
        // snapshot objects. The replica tables below are likewise only ever
        // created, never replaced: losing `sync_context` would let this
        // device reuse write counters its peers have already seen.
        if has_column(&tx, "sync_objects", "event_id")? {
            tx.execute("DROP TABLE sync_objects", []).map_err(error)?;
        }
        tx.execute_batch(
            "-- Every surviving value of every field: one row per write that
             -- still holds the field (several while it's in conflict).
             CREATE TABLE IF NOT EXISTS sync_values (
                entity_type TEXT NOT NULL,
                entity_id TEXT NOT NULL,
                field TEXT NOT NULL,
                device_id TEXT NOT NULL,
                counter INTEGER NOT NULL,
                lamport INTEGER NOT NULL,
                value TEXT,
                PRIMARY KEY(entity_type, entity_id, field, device_id, counter)
            );
            -- The causal context: the highest write counter seen from each
            -- device, this one included.
            CREATE TABLE IF NOT EXISTS sync_context (
                device_id TEXT PRIMARY KEY,
                counter INTEGER NOT NULL
            );
            -- Whether the replica changed since the last sealed snapshot.
            CREATE TABLE IF NOT EXISTS sync_local_state (
                id INTEGER PRIMARY KEY CHECK(id = 1),
                dirty INTEGER NOT NULL DEFAULT 1,
                state_sequence INTEGER NOT NULL DEFAULT 0,
                sealed_epoch INTEGER,
                last_change_at TEXT
            );
            -- The latest snapshot merged from each peer, its latest head,
            -- and the newest epoch whose keys this device has shared with it
            -- (see `enrollment::share_keys_with_lagging_peers`).
            CREATE TABLE IF NOT EXISTS sync_remote_states (
                device_id TEXT PRIMARY KEY,
                state_sequence INTEGER NOT NULL DEFAULT 0,
                merged_at TEXT,
                last_head_published_at_ms INTEGER,
                last_head_seen_at_ms INTEGER,
                last_head_epoch INTEGER,
                keys_shared_epoch INTEGER
            );
            -- Objects this device keeps on its connectors: its current
            -- snapshot's chunks and index (`state_sequence` set), the
            -- protocol marker, and control objects.
            CREATE TABLE IF NOT EXISTS sync_objects (
                cid TEXT PRIMARY KEY,
                object_kind TEXT NOT NULL,
                state_sequence INTEGER,
                chunk_index INTEGER NOT NULL,
                chunk_count INTEGER NOT NULL,
                bytes BLOB NOT NULL
            );
            -- This device's superseded snapshot objects, deleted from each
            -- connector once that connector's head names a newer snapshot.
            CREATE TABLE IF NOT EXISTS sync_retired_objects (
                cid TEXT NOT NULL,
                transport_instance_id TEXT NOT NULL,
                state_sequence INTEGER NOT NULL,
                PRIMARY KEY(cid, transport_instance_id)
            );",
        )
        .map_err(error)?;
        tx.pragma_update(None, "user_version", 36).map_err(error)?;
    }
    if version < 37 {
        // Distinguishes the replica state captured by an in-progress seal
        // from writes committed before that seal is stored.
        if !has_column(&tx, "sync_local_state", "generation")? {
            tx.execute("ALTER TABLE sync_local_state ADD COLUMN generation INTEGER NOT NULL DEFAULT 0", [])
                .map_err(error)?;
        }
        tx.pragma_update(None, "user_version", 37).map_err(error)?;
    }
    if version < 38 {
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS contacts (
                id TEXT PRIMARY KEY,
                display_name TEXT,
                role TEXT,
                company TEXT,
                location TEXT,
                bio TEXT,
                notes TEXT,
                links_json TEXT NOT NULL DEFAULT '[]',
                photo_data TEXT,
                favorite INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS contact_addresses (
                contact_id TEXT NOT NULL REFERENCES contacts(id) ON DELETE CASCADE,
                email TEXT NOT NULL UNIQUE,
                PRIMARY KEY(contact_id, email)
            );
            CREATE INDEX IF NOT EXISTS contacts_favorite_updated ON contacts(favorite, updated_at DESC);
            CREATE TEMP TABLE legacy_contact_map(email TEXT PRIMARY KEY, contact_id TEXT NOT NULL);
            INSERT INTO legacy_contact_map(email,contact_id)
            SELECT lower(email), 'legacy:' || lower(hex(randomblob(16))) FROM pinned_contacts GROUP BY lower(email);
            INSERT OR IGNORE INTO contacts(id, display_name, role, company, location, bio, notes,
                links_json, photo_data, favorite, updated_at)
            SELECT map.contact_id, max(p.display_name), NULL, NULL, NULL, NULL, NULL,
                '[]', NULL, 1, max(p.pinned_at) FROM pinned_contacts p JOIN legacy_contact_map map ON map.email=lower(p.email) GROUP BY map.contact_id;
            INSERT OR IGNORE INTO contact_addresses(contact_id, email)
            SELECT map.contact_id, map.email FROM legacy_contact_map map;
            DROP TABLE legacy_contact_map;
            PRAGMA user_version=38;",
        ).map_err(error)?;
    }
    if version < 39 {
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS contact_interactions (
                message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
                thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
                account_id TEXT NOT NULL,
                email TEXT NOT NULL,
                display_name TEXT,
                direction TEXT NOT NULL CHECK(direction IN ('sent','received')),
                sent_at TEXT NOT NULL,
                PRIMARY KEY(message_id,email,direction)
            );
            CREATE INDEX IF NOT EXISTS contact_interactions_account_email_time ON contact_interactions(account_id,email,sent_at DESC);
            CREATE INDEX IF NOT EXISTS contact_interactions_email_time ON contact_interactions(email,sent_at DESC);
            CREATE INDEX IF NOT EXISTS contact_interactions_thread ON contact_interactions(thread_id,sent_at DESC);
            PRAGMA user_version=39;",
        ).map_err(error)?;
    }
    if version < 40 {
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS pending_entity_materializations (
                entity_type TEXT NOT NULL,
                entity_id TEXT NOT NULL,
                reason TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY(entity_type, entity_id)
            );
            PRAGMA user_version=40;",
        ).map_err(error)?;
    }
    if version < 41 {
        // SQLite cannot alter a CHECK constraint, so rebuild the table to admit
        // the board's in-progress column. Columns are copied by name, and a
        // later version's goal_id is kept when a partial upgrade re-runs this.
        let goal_column = has_column(&tx, "tasks", "goal_id")?;
        tx.execute_batch(&format!(
            "DROP INDEX tasks_status_due;
            DROP INDEX tasks_account_status;
            DROP INDEX tasks_thread;
            ALTER TABLE tasks RENAME TO tasks_v40;
            CREATE TABLE tasks (
                id TEXT PRIMARY KEY,
                account_id TEXT NOT NULL,
                thread_id TEXT,
                source_message_id TEXT,
                subject_snapshot TEXT,
                title TEXT NOT NULL,
                notes TEXT,
                kind TEXT NOT NULL CHECK(kind IN ('action', 'follow_up', 'waiting_for')),
                due_kind TEXT NOT NULL CHECK(due_kind IN ('none', 'date', 'datetime')),
                due_value TEXT,
                time_zone TEXT,
                repeat_interval_days INTEGER,
                status TEXT NOT NULL CHECK(status IN ('open', 'in_progress', 'completed', 'cancelled')),
                completion_source TEXT CHECK(completion_source IS NULL OR completion_source IN ('user', 'reply', 'external')),
                evidence_text TEXT,
                wait_after TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                completed_at TEXT{goal_definition}
            );
            INSERT INTO tasks({columns}) SELECT {columns} FROM tasks_v40;
            DROP TABLE tasks_v40;
            CREATE INDEX tasks_status_due ON tasks(status, due_value, updated_at);
            CREATE INDEX tasks_account_status ON tasks(account_id, status, updated_at);
            CREATE INDEX tasks_thread ON tasks(thread_id, status);
            PRAGMA user_version=41;",
            goal_definition = if goal_column { ",\n                goal_id TEXT" } else { "" },
            columns = format!(
                "id, account_id, thread_id, source_message_id, subject_snapshot, title, notes, kind, due_kind, \
                 due_value, time_zone, repeat_interval_days, status, completion_source, evidence_text, wait_after, \
                 created_at, updated_at, completed_at{}",
                if goal_column { ", goal_id" } else { "" },
            ),
        )).map_err(error)?;
    }
    if version < 42 {
        // Local-only AI bookkeeping: verified suggestions per thread revision,
        // so a restart does not pay for them again, and daily provider usage
        // for the cost display. Neither table is synced or exported.
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS ai_thread_analyses (
                thread_id TEXT PRIMARY KEY REFERENCES threads(id) ON DELETE CASCADE,
                last_message_at TEXT NOT NULL,
                analysis_json TEXT NOT NULL,
                generated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS ai_usage (
                day TEXT NOT NULL,
                provider TEXT NOT NULL,
                model TEXT NOT NULL,
                requests INTEGER NOT NULL,
                input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                reported_cost_requests INTEGER NOT NULL,
                reported_cost_usd REAL NOT NULL,
                PRIMARY KEY (day, provider, model)
            );
            PRAGMA user_version=42;",
        ).map_err(error)?;
    }
    if version < 43 {
        // Raw provider payloads are the largest thing stored locally, so new
        // rows go to the zstd-compressed `payload_z` column (legacy plaintext
        // rows are converted in the background by storage maintenance). Also
        // a one-time sweep of payloads whose message no longer exists:
        // nothing removed them when their thread was replaced or deleted.
        if !has_column(&tx, "message_metadata", "payload_z")? {
            tx.execute_batch("ALTER TABLE message_metadata ADD COLUMN payload_z BLOB;")
                .map_err(error)?;
        }
        tx.execute_batch(
            "DELETE FROM message_metadata
             WHERE NOT EXISTS (SELECT 1 FROM messages m WHERE m.id = message_metadata.id);
            PRAGMA user_version=43;",
        ).map_err(error)?;
    }
    if version < 44 {
        // No schema change. Opening a database from before this version
        // rebuilds `contact_interactions` (see `Database::open`) so stored
        // recipients with unquoted commas in display names are indexed.
        tx.execute_batch("PRAGMA user_version=44;").map_err(error)?;
    }
    if version < 45 {
        // Long-term goals, one account each like tasks. A goal names one
        // period of its horizon ('2026', '2026-H2', '2026-Q4') and may
        // support one goal of a longer horizon. Neither link is a foreign
        // key: synced rows can arrive in any order, and a link to a goal
        // that is not here reads as no link.
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS goals (
                id TEXT PRIMARY KEY,
                account_id TEXT NOT NULL,
                title TEXT NOT NULL,
                notes TEXT,
                horizon TEXT NOT NULL CHECK(horizon IN ('year', 'half', 'quarter')),
                period TEXT NOT NULL,
                status TEXT NOT NULL CHECK(status IN ('active', 'achieved', 'dropped')),
                parent_goal_id TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                closed_at TEXT
            );
            CREATE INDEX IF NOT EXISTS goals_account_period ON goals(account_id, period, status);",
        ).map_err(error)?;
        if !has_column(&tx, "tasks", "goal_id")? {
            tx.execute_batch("ALTER TABLE tasks ADD COLUMN goal_id TEXT;").map_err(error)?;
        }
        tx.execute_batch(
            "CREATE INDEX IF NOT EXISTS tasks_goal ON tasks(goal_id);
            PRAGMA user_version=45;",
        ).map_err(error)?;
    }
    if version < 46 {
        // Search rows now leave out quoted history repeated within a thread.
        // Existing rows are queued here and rewritten in the background
        // (`Database::reindex_next_search_batch`); `apply_thread` dequeues a
        // thread whenever it writes a fresh row. A queued row stays fully
        // searchable until it is rewritten.
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS pending_search_reindex (thread_id TEXT PRIMARY KEY);
            INSERT OR IGNORE INTO pending_search_reindex(thread_id) SELECT thread_id FROM thread_search;
            PRAGMA user_version=46;",
        ).map_err(error)?;
    }
    if version < 47 {
        // A plain-text body holding an HTML document is now read as HTML, so
        // previews written from that markup are rewritten by the same
        // background reindex.
        tx.execute_batch(
            "INSERT OR IGNORE INTO pending_search_reindex(thread_id) SELECT thread_id FROM thread_search;
            PRAGMA user_version=47;",
        ).map_err(error)?;
    }
    if version < 48 {
        // The thread revision (newest message time) a summary was written
        // from. Staleness compares against it rather than the time the
        // provider call finished, which can be later than mail that arrived
        // while it ran. Existing summaries keep a NULL revision and fall back
        // to `summary_generated_at`.
        if !has_column(&tx, "threads", "summary_revision")? {
            tx.execute("ALTER TABLE threads ADD COLUMN summary_revision TEXT", []).map_err(error)?;
        }
        tx.execute_batch("PRAGMA user_version=48;").map_err(error)?;
    }
    if version < 49 {
        // Birthdays and keep-in-touch reminders on saved contacts. Every
        // column is nullable: a NULL interval means reminders are off, and
        // existing profiles keep that default.
        for (column, kind) in [
            ("birthday", "TEXT"),
            ("kit_interval_days", "INTEGER"),
            ("kit_started_at", "TEXT"),
            ("kit_snoozed_until", "TEXT"),
            ("kit_snoozed_at", "TEXT"),
            ("kit_last_touch_at", "TEXT"),
        ] {
            if !has_column(&tx, "contacts", column)? {
                tx.execute(&format!("ALTER TABLE contacts ADD COLUMN {column} {kind}"), []).map_err(error)?;
            }
        }
        tx.execute_batch("PRAGMA user_version=49;").map_err(error)?;
    }
    if version < 50 {
        // Progress of the one-time sent-mail backfill that seeds the address
        // book from history older than the inbox-only initial sync. The page
        // token and offset let it resume across polls and restarts.
        for (column, kind) in [
            ("sent_backfill_page", "TEXT"),
            ("sent_backfill_offset", "INTEGER NOT NULL DEFAULT 0"),
            ("sent_backfill_scanned", "INTEGER NOT NULL DEFAULT 0"),
            ("sent_backfill_completed_at", "TEXT"),
        ] {
            if !has_column(&tx, "sync_state", column)? {
                tx.execute(&format!("ALTER TABLE sync_state ADD COLUMN {column} {kind}"), []).map_err(error)?;
            }
        }
        tx.execute_batch("PRAGMA user_version=50;").map_err(error)?;
    }
    if version < 51 {
        // Named contact groups, global like saved contacts. Membership is
        // not a foreign key to `contacts`: synced groups and contacts can
        // arrive in any order, and a member whose contact is not here reads
        // as absent. Deleting a contact removes its memberships explicitly.
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS contact_groups (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS contact_group_members (
                group_id TEXT NOT NULL REFERENCES contact_groups(id) ON DELETE CASCADE,
                contact_id TEXT NOT NULL,
                PRIMARY KEY(group_id, contact_id)
            );
            CREATE INDEX IF NOT EXISTS contact_group_members_contact ON contact_group_members(contact_id);
            PRAGMA user_version=51;",
        ).map_err(error)?;
    }
    if version < 52 {
        // A contact's addresses keep the order they were saved in; the
        // first is the primary address. Saves rewrite every row in request
        // order, so rowid order is the order the user last saved.
        if !has_column(&tx, "contact_addresses", "position")? {
            tx.execute_batch(
                "ALTER TABLE contact_addresses ADD COLUMN position INTEGER NOT NULL DEFAULT 0;
                 UPDATE contact_addresses SET position=(SELECT COUNT(*) FROM contact_addresses earlier
                     WHERE earlier.contact_id=contact_addresses.contact_id AND earlier.rowid<contact_addresses.rowid);",
            ).map_err(error)?;
        }
        tx.execute_batch("PRAGMA user_version=52;").map_err(error)?;
    }
    if version < 53 {
        tx.execute_batch("CREATE TABLE IF NOT EXISTS contact_suggestion_suppressions(
            email TEXT PRIMARY KEY NOT NULL
        ); PRAGMA user_version=53;").map_err(error)?;
    }
    if version < 54 {
        // The IMAP provider's own persistent sync state (see
        // `docs/imap-design.md`, "Data model" / "Message identity"). This is
        // the backing store behind the `ImapStateStore` seam in
        // `provider::imap`: the per-account mailbox catalog with each
        // mailbox's UID counters, and the UID-to-message-id location map that
        // the opaque `SyncCursor` deliberately does not carry. Entirely inert
        // until the IMAP provider lands (phase 2): no Gmail code path reads or
        // writes these tables, so Gmail sync state is untouched. Reserve a new
        // schema number for every later change to this shape rather than
        // editing this block once it has shipped.
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS imap_mailboxes (
                account_id TEXT NOT NULL,
                name TEXT NOT NULL,              -- decoded; delimiter kept separately
                delimiter TEXT,
                special_use TEXT,                -- \\Sent, \\Archive, … or NULL
                uidvalidity INTEGER NOT NULL,
                uidnext INTEGER NOT NULL,
                highestmodseq INTEGER,           -- NULL without CONDSTORE
                permanent_flags_json TEXT NOT NULL,  -- PERMANENTFLAGS as listed
                permanent_keywords INTEGER NOT NULL, -- PERMANENTFLAGS contains \\*
                PRIMARY KEY (account_id, name)
            );
            CREATE TABLE IF NOT EXISTS imap_locations (
                account_id TEXT NOT NULL,
                mailbox TEXT NOT NULL,
                uidvalidity INTEGER NOT NULL,
                uid INTEGER NOT NULL,
                message_id TEXT NOT NULL,        -- the stable id above
                flags_json TEXT NOT NULL,
                modseq INTEGER,
                PRIMARY KEY (account_id, mailbox, uidvalidity, uid)
            );
            CREATE INDEX IF NOT EXISTS imap_locations_by_message
                ON imap_locations(account_id, message_id);
            PRAGMA user_version=54;",
        )
        .map_err(error)?;
    }
    if version < 55 {
        // The IMAP provider's NON-SECRET account settings (see
        // `docs/imap-design.md`, "Account setup" / "Save"). Phase 2 Slice 2
        // (account setup) is the first writer. Everything here is safe to put
        // in the local database and to carry in a settings export: hosts,
        // ports, the security mode, the IMAP/SMTP usernames, mailbox-name
        // overrides, the Archive-folder choice, the user-label storage mode
        // and its container mailbox, the ordered identity list (JSON), the
        // pinned leaf-certificate SHA-256 per `host:port` (JSON), and the
        // `server_saves_sent` flag. The PASSWORD is deliberately absent — it
        // lives only in the OS keychain as `StoredCredential::ImapPassword`,
        // never in this table and never in the export. One row per account,
        // keyed by the account's email, matching `accounts.email`.
        //
        // Inert until an IMAP account is set up: no Gmail code path reads or
        // writes this table, so Gmail is untouched. Reserve a new schema
        // number for every later change to this shape rather than editing this
        // block once it has shipped.
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS imap_account_settings (
                account_id TEXT PRIMARY KEY,     -- the account's email (accounts.email)
                imap_host TEXT NOT NULL,
                imap_port INTEGER NOT NULL,
                imap_security TEXT NOT NULL,     -- 'implicit_tls' | 'starttls'
                imap_username TEXT NOT NULL,
                smtp_host TEXT NOT NULL,
                smtp_port INTEGER NOT NULL,
                smtp_security TEXT NOT NULL,     -- 'implicit_tls' | 'starttls'
                smtp_username TEXT NOT NULL,
                mailbox_overrides_json TEXT NOT NULL DEFAULT '{}',  -- system-mailbox name overrides
                archive_mailbox TEXT,            -- chosen Archive mailbox, or NULL to decide at Slice 3
                label_storage TEXT NOT NULL,     -- 'keywords' | 'folders' | 'none'
                label_container TEXT,            -- container mailbox for label-folder mode
                identities_json TEXT NOT NULL DEFAULT '[]',  -- ordered [{address, displayName?}]
                pinned_fingerprints_json TEXT NOT NULL DEFAULT '{}', -- {\"host:port\": \"SHA-256 hex\"}
                server_saves_sent INTEGER NOT NULL DEFAULT 0
            );
            PRAGMA user_version=55;",
        )
        .map_err(error)?;
    }
    if version < 56 {
        // Discovery previously persisted EXAMINE's read-only permissions as
        // write capabilities. Invalidate that unreliable metadata and allow
        // NULL until a writable SELECT supplies real PERMANENTFLAGS. This
        // provider-internal catalog is not part of settings transfer.
        tx.execute_batch(
            "CREATE TABLE imap_mailboxes_v56 (
                account_id TEXT NOT NULL,
                name TEXT NOT NULL,
                delimiter TEXT,
                special_use TEXT,
                uidvalidity INTEGER NOT NULL,
                uidnext INTEGER NOT NULL,
                highestmodseq INTEGER,
                permanent_flags_json TEXT,
                permanent_keywords INTEGER,
                PRIMARY KEY (account_id, name)
            );
            INSERT INTO imap_mailboxes_v56
                SELECT account_id, name, delimiter, special_use, uidvalidity,
                       uidnext, highestmodseq, NULL, NULL FROM imap_mailboxes;
            DROP TABLE imap_mailboxes;
            ALTER TABLE imap_mailboxes_v56 RENAME TO imap_mailboxes;
            PRAGMA user_version=56;",
        )
        .map_err(error)?;
    }
    if version < 57 {
        // The IMAP provider's on-disk BODY.PEEK[] cache (Phase 2 Slice 4; see
        // `docs/imap-design.md` "Bodies"). A message body never changes in
        // IMAP, so each is fetched once and served from here; a flag change or
        // a UIDVALIDITY reset never re-downloads it. Keyed by the STABLE
        // message id, NOT by (mailbox, uidvalidity, uid): that is why a
        // `UIDVALIDITY` reset — which drops a mailbox's `imap_locations` rows —
        // leaves cached bodies intact, and why the same message in two
        // mailboxes shares one cached body.
        //
        // This is a rebuildable, provider-internal cache. It is NOT part of
        // the settings-transfer export (`transfer.rs` is untouched and its
        // VERSION is unchanged): losing it only forces a re-fetch. Inert until
        // the IMAP provider's read path lands (Slice 5); no Gmail code path
        // reads or writes it, so Gmail is untouched. Reserve a new schema
        // number for every later change to this shape rather than editing this
        // block once it has shipped.
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS imap_bodies (
                account_id TEXT NOT NULL,
                message_id TEXT NOT NULL,        -- the stable cross-account id
                raw BLOB NOT NULL,               -- the whole RFC 5322 message
                size INTEGER NOT NULL,           -- raw.len(), for cache accounting
                fetched_at INTEGER NOT NULL,     -- unix seconds the body was cached
                PRIMARY KEY (account_id, message_id)
            );
            PRAGMA user_version=57;",
        )
        .map_err(error)?;
    }
    if version < 58 {
        // The IMAP provider's live-sync state (Phase 2 Slice 5a; see
        // `docs/imap-design.md` "Sync" / "Threading"). These tables turn the
        // Slice 0-4 connection/identity/body machinery into something the
        // provider-neutral `sync.rs` engine can poll: a monotonic sync
        // generation per account, a change journal of thread ids per
        // generation (the at-least-once cursor's backing store), and the
        // local thread grouping with its merge aliases.
        //
        //  * `imap_sync_state`  — one row per account carrying the current
        //    monotonic `generation`. Bumped once per sync round; the opaque
        //    `SyncCursor` is just this number as decimal text.
        //  * `imap_change_journal` — `(account_id, generation, thread_id)`.
        //    Every thread whose content or labels changed in a round is
        //    appended here under that round's generation, so `poll(cursor=g)`
        //    can return exactly the threads changed since generation `g`.
        //    Indexed by `(account_id, generation)` for the poll scan.
        //  * `imap_threads` — `(account_id, message_id, thread_id)`: which
        //    local thread each message belongs to, indexed by thread so a
        //    thread's messages resolve in one query. Thread ids are derived
        //    from message references (`docs/imap-design.md` "Threading"),
        //    never from subject.
        //  * `imap_thread_aliases` — `(account_id, old_id, new_id)`: when a
        //    late message merges two threads the older id survives and the
        //    other's id is recorded here so holders of the old id resolve to
        //    the survivor.
        //
        // All four are keyed by `account_id` and are provider-internal sync
        // state: NO Gmail code path reads or writes them, and they are
        // deliberately NOT part of the settings-transfer export (`transfer.rs`
        // is untouched and its VERSION is unchanged) — losing them only forces
        // a full resync. Reserve a new schema number for every later change to
        // this shape rather than editing this block once it has shipped.
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS imap_sync_state (
                account_id TEXT PRIMARY KEY,
                generation INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS imap_change_journal (
                account_id TEXT NOT NULL,
                generation INTEGER NOT NULL,
                thread_id TEXT NOT NULL,
                PRIMARY KEY (account_id, generation, thread_id)
            );
            CREATE INDEX IF NOT EXISTS imap_change_journal_by_generation
                ON imap_change_journal(account_id, generation);
            CREATE TABLE IF NOT EXISTS imap_threads (
                account_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                thread_id TEXT NOT NULL,
                created_generation INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (account_id, message_id)
            );
            CREATE INDEX IF NOT EXISTS imap_threads_by_thread
                ON imap_threads(account_id, thread_id);
            CREATE TABLE IF NOT EXISTS imap_thread_aliases (
                account_id TEXT NOT NULL,
                old_id TEXT NOT NULL,
                new_id TEXT NOT NULL,
                PRIMARY KEY (account_id, old_id)
            );
            PRAGMA user_version=58;",
        )
        .map_err(error)?;
    }
    if version < 59 {
        // The IMAP provider's PERSISTED threading tokens (Phase 2 Slice 5b-1;
        // see `docs/imap-design.md` "Threading"). Before this version the
        // provider rebuilt the threader's prior state by reading and parsing
        // EVERY cached body on each new-mail round — O(all mail) in bytes per
        // round, and impossible for an index-tier message that has no cached
        // body at all. This table makes the token set of each threaded message
        // a durable fact, so a round seeds only the threads the incoming batch
        // actually touches without reading `imap_bodies`.
        //
        //  * `imap_message_tokens` — `(account_id, message_id, token)`: the
        //    exact set of normalized tokens the threader uses for a message —
        //    its stable id, its normalized `Message-ID` header, and the capped
        //    `In-Reply-To`/`References` ancestry (`MAX_REFERENCES`). Covering
        //    index on `(account_id, token)` for the "which messages share a
        //    token" seed lookup, and on `(account_id, message_id)` for purge
        //    and per-message reads. Tokens are written in the SAME atomic
        //    transaction as the thread assignment (`commit_sync_round`).
        //
        //  * `imap_sync_state.tokens_backfilled` — a per-account flag for the
        //    one-time upgrade backfill. A database that already holds threaded
        //    messages from 0.94.x has NO token rows, so existing rows default
        //    to 0 ("backfill needed") and a bounded, crash-resumable backfill
        //    fills them from the cached bodies before the first threading that
        //    needs tokens. A brand-new account never has token-less threads
        //    (tokens are written at assignment), so its `imap_sync_state` row
        //    is born with `tokens_backfilled = 1` ("done") in
        //    `commit_sync_round`.
        //
        // Provider-internal sync state: NO Gmail code path reads or writes it,
        // and it is deliberately NOT part of the settings-transfer export
        // (`transfer.rs` is untouched and its VERSION is unchanged) — losing it
        // only forces a re-derive from the bodies or a full resync. Reserve a
        // new schema number for every later change to this shape rather than
        // editing this block once it has shipped.
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS imap_message_tokens (
                account_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                token TEXT NOT NULL,
                PRIMARY KEY (account_id, message_id, token)
            );
            CREATE INDEX IF NOT EXISTS imap_message_tokens_by_token
                ON imap_message_tokens(account_id, token);
            CREATE INDEX IF NOT EXISTS imap_message_tokens_by_message
                ON imap_message_tokens(account_id, message_id);",
        )
        .map_err(error)?;
        // Existing accounts default to "backfill needed" (0); the column add
        // itself sets every pre-existing row to 0.
        if !has_column(&tx, "imap_sync_state", "tokens_backfilled")? {
            tx.execute_batch(
                "ALTER TABLE imap_sync_state
                    ADD COLUMN tokens_backfilled INTEGER NOT NULL DEFAULT 0;",
            )
            .map_err(error)?;
        }
        tx.execute_batch("PRAGMA user_version=59;").map_err(error)?;
    }
    if version < 60 {
        // The IMAP provider's MULTI-MAILBOX sync machinery (Phase 2 Slice
        // 5b-1; see `docs/imap-design.md` "Sync" / "What gets synced"). Slice
        // 5a synced INBOX only; 5b-1 generalizes the round to a plan-driven
        // set of mailboxes (INBOX, Trash, Junk, and Sent — the last added in
        // run 3 on this same schema) and adds the two tables that machinery
        // needs.
        //
        //  * `imap_mailbox_sync_state` — one row per synced mailbox carrying
        //    the cheap-cadence inputs: `last_exists`/`last_uidnext` from the
        //    previous EXAMINE (so an unchanged mailbox skips SEARCH/FETCH),
        //    `last_sweep_at` (the periodic full-sweep clock), and
        //    `backfill_low_uid` — the Sent chunked-backfill watermark (lowest
        //    UID acquired while the backfill is incomplete, NULL when done),
        //    which run 3 fills WITHOUT a new migration exactly as reserved here.
        //
        //  * `imap_hot_threads` — `(account_id, thread_id)`: a thread is "hot"
        //    when it has a location in INBOX or Sent, or was already hot. Only
        //    hot threads are journaled (reported to the engine); a Trash/Junk-
        //    only thread that was never hot is index-only. Set in the same
        //    atomic round as the location that makes it hot.
        //
        // UPGRADE SEEDING: every existing account is INBOX-only and all of its
        // mail was already ingested (journaled) under 5a, so every thread that
        // has a message with an INBOX location becomes hot here, in SQL, so no
        // already-reported thread is silently demoted to index-only on upgrade.
        //
        // Provider-internal sync state: NO Gmail code path reads or writes it,
        // and it is deliberately NOT part of the settings-transfer export
        // (`transfer.rs` is untouched and its VERSION is unchanged) — losing it
        // only forces a re-derive or a full resync. Reserve a new schema number
        // for every later change to this shape rather than editing this block.
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS imap_mailbox_sync_state (
                account_id TEXT NOT NULL,
                mailbox TEXT NOT NULL,
                last_exists INTEGER NOT NULL DEFAULT 0,
                last_uidnext INTEGER NOT NULL DEFAULT 0,
                last_sweep_at INTEGER NOT NULL DEFAULT 0,
                backfill_low_uid INTEGER,
                PRIMARY KEY (account_id, mailbox)
            );
            CREATE TABLE IF NOT EXISTS imap_hot_threads (
                account_id TEXT NOT NULL,
                thread_id TEXT NOT NULL,
                PRIMARY KEY (account_id, thread_id)
            );",
        )
        .map_err(error)?;
        // Seed hotness for every pre-existing INBOX-located thread. Idempotent
        // (INSERT OR IGNORE on the PK), so a rerun of this migration is a
        // no-op; it reads imap_locations + imap_threads only.
        tx.execute_batch(
            "INSERT OR IGNORE INTO imap_hot_threads(account_id, thread_id)
             SELECT DISTINCT t.account_id, t.thread_id
             FROM imap_threads t
             JOIN imap_locations l
               ON l.account_id = t.account_id AND l.message_id = t.message_id
             WHERE l.mailbox = 'INBOX';",
        )
        .map_err(error)?;
        tx.execute_batch("PRAGMA user_version=60;").map_err(error)?;
    }
    if version < 61 {
        // Targeted thread-family lookups follow aliases backwards, both for
        // token seeding and inherited hotness. Avoid scanning every alias in
        // an account whenever a reply extends one existing thread.
        tx.execute_batch(
            "CREATE INDEX IF NOT EXISTS imap_thread_aliases_by_survivor
                 ON imap_thread_aliases(account_id, new_id);
             PRAGMA user_version=61;",
        )
        .map_err(error)?;
    }
    if version < 62 {
        // The IMAP provider's FAIR FOLDER SCHEDULING and EMPTIED-HOT-THREAD
        // GRACE (Phase 2 Slice 5b-2; see `docs/imap-design.md` "Sync"). Three
        // nullable/defaulted columns on the Slice 5b-1 tables, no new table:
        //
        //  * `imap_mailbox_sync_state.last_visited_at` — the per-mailbox VISIT
        //    clock, distinct from `last_sweep_at`. A mailbox skipped by the
        //    cadence gate advances this (it WAS examined) but NOT the sweep
        //    clock, so least-recently-VISITED ordering never starves a mailbox
        //    the cadence gate keeps skipping. Defaults to 0 (never visited).
        //
        //  * `imap_mailbox_sync_state.visited_in_epoch` — the COVERAGE-EPOCH
        //    number this mailbox was last SUCCESSFULLY visited in. Coverage is
        //    measured ACROSS polls, not per poll: run B syncs more user/label
        //    folders than one poll's FOLDER_ROUNDS_PER_POLL budget can visit,
        //    so no single poll covers them all. An epoch completes when every
        //    currently-synced mailbox (INBOX + all `synced_now` entries) has
        //    been visited in the current epoch; only then does `complete_walks`
        //    advance. Defaults to 0.
        //
        //  * `imap_sync_state.complete_walks` — the per-account count of
        //    completed COVERAGE EPOCHS (an epoch = every currently-synced
        //    mailbox visited across one or more polls). The emptied-thread
        //    grace is measured in completed epochs, so a folder that is never
        //    successfully visited holds the epoch open and the grace never
        //    advances (safe: never deletes data early). Defaults 0.
        //
        //  * `imap_hot_threads.emptied_at_walk` — nullable: the completed-epoch
        //    count at which a hot thread was first observed with ZERO
        //    locations. NULL means "not currently empty". A thread that
        //    regains a location clears it; once `EMPTIED_THREAD_GRACE_WALKS`
        //    completed coverage epochs have passed the thread is journaled and
        //    the engine deletes it. This is the data-integrity fix for a hot
        //    message moved out of INBOX into a mailbox not swept that poll.
        //
        // Provider-internal sync state: NO Gmail code path reads or writes it,
        // and it is deliberately NOT part of the settings-transfer export
        // (`transfer.rs` is untouched and its VERSION is unchanged) — losing it
        // only forces a re-derive or a full resync. The column adds set every
        // pre-existing row to the default (0 / NULL), which is exactly the
        // "never visited / no epochs yet / not empty" starting state. Reserve a
        // new schema number for every later change rather than editing this.
        if !has_column(&tx, "imap_mailbox_sync_state", "last_visited_at")? {
            tx.execute_batch(
                "ALTER TABLE imap_mailbox_sync_state
                    ADD COLUMN last_visited_at INTEGER NOT NULL DEFAULT 0;",
            )
            .map_err(error)?;
        }
        if !has_column(&tx, "imap_mailbox_sync_state", "visited_in_epoch")? {
            tx.execute_batch(
                "ALTER TABLE imap_mailbox_sync_state
                    ADD COLUMN visited_in_epoch INTEGER NOT NULL DEFAULT 0;",
            )
            .map_err(error)?;
        }
        if !has_column(&tx, "imap_sync_state", "complete_walks")? {
            tx.execute_batch(
                "ALTER TABLE imap_sync_state
                    ADD COLUMN complete_walks INTEGER NOT NULL DEFAULT 0;",
            )
            .map_err(error)?;
        }
        if !has_column(&tx, "imap_hot_threads", "emptied_at_walk")? {
            tx.execute_batch(
                "ALTER TABLE imap_hot_threads
                    ADD COLUMN emptied_at_walk INTEGER;",
            )
            .map_err(error)?;
        }
        tx.execute_batch("PRAGMA user_version=62;").map_err(error)?;
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

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    /// A pre-v21 database: `INITIAL_SCHEMA` alone, with no `migrate()` run
    /// yet, so `PRAGMA user_version` is still 0 and `accounts` lacks
    /// `provider` — that column is added exclusively by the `version < 21`
    /// migration rather than baked into the baseline table, matching every
    /// other post-baseline column (`calendar_accounts.selection_initialized`,
    /// `outbox_messages.attempts`, ...).
    fn unmigrated_database_with_one_account() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(crate::schema::INITIAL_SCHEMA)
            .unwrap();
        connection
            .execute(
                "INSERT INTO accounts(email, color, status, sort_order, connected_at)
                 VALUES ('you@gmail.com', '#4285F4', 'connected', 0, '2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
        connection
    }

    fn account_provider(connection: &Connection, email: &str) -> String {
        connection
            .query_row(
                "SELECT provider FROM accounts WHERE email = ?1",
                [email],
                |row| row.get(0),
            )
            .unwrap()
    }

    #[test]
    fn upgrading_v52_adds_suggestion_suppression_without_changing_contacts() {
        let mut connection = unmigrated_database_with_one_account();
        super::migrate(&mut connection).unwrap();
        connection.execute_batch("DROP TABLE contact_suggestion_suppressions; PRAGMA user_version=52;").unwrap();
        connection.execute("INSERT INTO contacts(id,display_name,links_json,favorite,updated_at) VALUES('person','Person','[]',0,'2026-10-01')", []).unwrap();
        super::migrate(&mut connection).unwrap();
        assert_eq!(connection.query_row("SELECT display_name FROM contacts WHERE id='person'", [], |row| row.get::<_, String>(0)).unwrap(), "Person");
        connection.execute("INSERT INTO contact_suggestion_suppressions(email) VALUES('person@example.com')", []).unwrap();
        super::migrate(&mut connection).unwrap();
        assert_eq!(connection.query_row("SELECT COUNT(*) FROM contact_suggestion_suppressions", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
    }

    #[test]
    fn v54_adds_the_imap_state_store_tables_and_reruns_cleanly() {
        // A fresh database reaches the latest version with the IMAP provider
        // state-store tables present and queryable.
        let mut fresh = unmigrated_database_with_one_account();
        super::migrate(&mut fresh).unwrap();
        for table in ["imap_mailboxes", "imap_locations"] {
            fresh
                .execute(&format!("SELECT * FROM {table}"), [])
                .unwrap_or_else(|error| panic!("table {table} should exist and be queryable: {error}"));
        }
        assert_eq!(
            fresh.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).unwrap(),
            super::LATEST_VERSION
        );

        // An upgraded database (one that stopped at v53) converges to the same
        // schema: rebuild the v53 shape, re-run, and confirm the tables appear
        // and accept rows with the design-doc columns.
        let mut upgraded = unmigrated_database_with_one_account();
        super::migrate(&mut upgraded).unwrap();
        upgraded
            .execute_batch(
                "DROP INDEX imap_locations_by_message;
                 DROP TABLE imap_locations;
                 DROP TABLE imap_mailboxes;
                 PRAGMA user_version=53;",
            )
            .unwrap();
        super::migrate(&mut upgraded).unwrap();
        upgraded
            .execute(
                "INSERT INTO imap_mailboxes(account_id,name,delimiter,special_use,uidvalidity,uidnext,highestmodseq,permanent_flags_json,permanent_keywords)
                 VALUES ('you@gmail.com','INBOX','/','\\Inbox',95479608,979,NULL,'[\"\\\\Seen\",\"\\\\Flagged\"]',0)",
                [],
            )
            .unwrap();
        upgraded
            .execute(
                "INSERT INTO imap_locations(account_id,mailbox,uidvalidity,uid,message_id,flags_json,modseq)
                 VALUES ('you@gmail.com','INBOX',95479608,42,'imap:you@gmail.com:abc','[\"\\\\Seen\"]',NULL)",
                [],
            )
            .unwrap();
        let message_id: String = upgraded
            .query_row(
                "SELECT message_id FROM imap_locations WHERE account_id='you@gmail.com' AND uid=42",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(message_id, "imap:you@gmail.com:abc");
        // Re-running once more over the already-created tables must not fail.
        upgraded.pragma_update(None, "user_version", 53).unwrap();
        super::migrate(&mut upgraded).unwrap();
        assert_eq!(
            upgraded.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).unwrap(),
            super::LATEST_VERSION
        );
    }

    #[test]
    fn v55_adds_the_imap_account_settings_table_and_reruns_cleanly() {
        // A fresh database reaches the latest version with the IMAP account-
        // settings table present and queryable.
        let mut fresh = unmigrated_database_with_one_account();
        super::migrate(&mut fresh).unwrap();
        fresh
            .execute("SELECT * FROM imap_account_settings", [])
            .unwrap_or_else(|error| panic!("imap_account_settings should exist and be queryable: {error}"));
        assert_eq!(
            fresh.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).unwrap(),
            super::LATEST_VERSION
        );

        // An upgraded database that stopped at v54 converges to the same
        // schema: rebuild the v54 shape (drop the table, step user_version
        // back), re-run, and confirm the table appears and accepts a row with
        // the design-doc columns — password absent, as the design requires.
        let mut upgraded = unmigrated_database_with_one_account();
        super::migrate(&mut upgraded).unwrap();
        upgraded
            .execute_batch("DROP TABLE imap_account_settings; PRAGMA user_version=54;")
            .unwrap();
        super::migrate(&mut upgraded).unwrap();
        upgraded
            .execute(
                "INSERT INTO imap_account_settings(
                     account_id, imap_host, imap_port, imap_security, imap_username,
                     smtp_host, smtp_port, smtp_security, smtp_username,
                     label_storage, identities_json, pinned_fingerprints_json, server_saves_sent)
                 VALUES ('you@gmail.com','127.0.0.1',1143,'starttls','you@proton.me',
                     '127.0.0.1',1025,'starttls','you@proton.me',
                     'folders','[{\"address\":\"you@proton.me\"}]','{\"127.0.0.1:1143\":\"AA\"}',0)",
                [],
            )
            .unwrap();
        let (host, security, label_storage): (String, String, String) = upgraded
            .query_row(
                "SELECT imap_host, imap_security, label_storage FROM imap_account_settings WHERE account_id='you@gmail.com'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!((host.as_str(), security.as_str(), label_storage.as_str()), ("127.0.0.1", "starttls", "folders"));
        // Re-running once more over the already-created table must not fail.
        upgraded.pragma_update(None, "user_version", 54).unwrap();
        super::migrate(&mut upgraded).unwrap();
        assert_eq!(
            upgraded.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).unwrap(),
            super::LATEST_VERSION
        );
    }

    #[test]
    fn v56_invalidates_examine_permissions_without_losing_catalog_or_locations() {
        let mut connection = unmigrated_database_with_one_account();
        super::migrate(&mut connection).unwrap();
        // Reconstruct the preceding schema, including its misleading stored
        // EXAMINE permissions, rather than starting from the nullable shape.
        connection
            .execute_batch(
                "DROP TABLE imap_mailboxes;
             CREATE TABLE imap_mailboxes (
                 account_id TEXT NOT NULL, name TEXT NOT NULL, delimiter TEXT,
                 special_use TEXT, uidvalidity INTEGER NOT NULL, uidnext INTEGER NOT NULL,
                 highestmodseq INTEGER, permanent_flags_json TEXT NOT NULL,
                 permanent_keywords INTEGER NOT NULL, PRIMARY KEY (account_id, name)
             );
             INSERT INTO imap_mailboxes VALUES ('imap@example.com',' Sent ','/','\\Sent',7,9,11,'[]',0);
             INSERT INTO imap_locations VALUES ('imap@example.com',' Sent ',7,1,'message','[]',NULL);
             PRAGMA user_version=55;",
            )
            .unwrap();
        super::migrate(&mut connection).unwrap();
        let row = connection
            .query_row(
                "SELECT name, delimiter, special_use, uidvalidity, uidnext, highestmodseq,
                    permanent_flags_json, permanent_keywords FROM imap_mailboxes",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, Option<bool>>(7)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            row,
            (
                " Sent ".into(),
                "/".into(),
                "\\Sent".into(),
                7,
                9,
                11,
                None,
                None
            )
        );
        assert_eq!(
            connection
                .query_row("SELECT message_id FROM imap_locations", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "message"
        );
        connection
            .execute(
                "UPDATE imap_mailboxes SET permanent_flags_json='[]', permanent_keywords=0",
                [],
            )
            .unwrap();
        super::migrate(&mut connection).unwrap();
        // A normal restart preserves capabilities subsequently learned by SELECT.
        assert_eq!(
            connection
                .query_row("SELECT permanent_keywords FROM imap_mailboxes", [], |row| {
                    row.get::<_, Option<bool>>(0)
                })
                .unwrap(),
            Some(false)
        );
    }

    #[test]
    fn v57_adds_the_imap_bodies_cache_and_reruns_cleanly() {
        // A fresh database reaches the latest version with the body-cache
        // table present and queryable.
        let mut fresh = unmigrated_database_with_one_account();
        super::migrate(&mut fresh).unwrap();
        fresh
            .execute("SELECT account_id, message_id, raw, size, fetched_at FROM imap_bodies", [])
            .unwrap_or_else(|error| panic!("imap_bodies should exist and be queryable: {error}"));
        assert_eq!(
            fresh.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).unwrap(),
            super::LATEST_VERSION
        );

        // An upgraded database that stopped at v56 converges to the same
        // schema: drop the table, step user_version back, re-run, and confirm
        // it appears and accepts a row keyed by the stable message id.
        let mut upgraded = unmigrated_database_with_one_account();
        super::migrate(&mut upgraded).unwrap();
        upgraded
            .execute_batch("DROP TABLE imap_bodies; PRAGMA user_version=56;")
            .unwrap();
        super::migrate(&mut upgraded).unwrap();
        upgraded
            .execute(
                "INSERT INTO imap_bodies(account_id, message_id, raw, size, fetched_at)
                 VALUES ('you@gmail.com','imap:you@gmail.com:abc', X'48656C6C6F', 5, 1700000000)",
                [],
            )
            .unwrap();
        let (size, fetched_at): (i64, i64) = upgraded
            .query_row(
                "SELECT size, fetched_at FROM imap_bodies WHERE message_id='imap:you@gmail.com:abc'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((size, fetched_at), (5, 1700000000));
        let raw: Vec<u8> = upgraded
            .query_row(
                "SELECT raw FROM imap_bodies WHERE message_id='imap:you@gmail.com:abc'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(raw, b"Hello");

        // A UIDVALIDITY reset drops a mailbox's locations but MUST NOT touch
        // cached bodies — they are keyed by the stable message id, not the UID.
        upgraded
            .execute(
                "INSERT INTO imap_locations(account_id,mailbox,uidvalidity,uid,message_id,flags_json,modseq)
                 VALUES ('you@gmail.com','INBOX',1,42,'imap:you@gmail.com:abc','[]',NULL)",
                [],
            )
            .unwrap();
        upgraded
            .execute("DELETE FROM imap_locations WHERE account_id='you@gmail.com' AND mailbox='INBOX'", [])
            .unwrap();
        assert_eq!(
            upgraded
                .query_row("SELECT COUNT(*) FROM imap_bodies", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1,
            "dropping a mailbox's locations must leave the body cache intact"
        );

        // Re-running once more over the already-created table must not fail.
        upgraded.pragma_update(None, "user_version", 56).unwrap();
        super::migrate(&mut upgraded).unwrap();
        assert_eq!(
            upgraded.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).unwrap(),
            super::LATEST_VERSION
        );
    }

    #[test]
    fn v58_adds_the_imap_sync_state_tables_and_reruns_cleanly() {
        // A fresh database reaches the latest version with all four live-sync
        // tables present and queryable.
        let mut fresh = unmigrated_database_with_one_account();
        super::migrate(&mut fresh).unwrap();
        for stmt in [
            "SELECT account_id, generation FROM imap_sync_state",
            "SELECT account_id, generation, thread_id FROM imap_change_journal",
            "SELECT account_id, message_id, thread_id FROM imap_threads",
            "SELECT account_id, old_id, new_id FROM imap_thread_aliases",
        ] {
            fresh
                .execute(stmt, [])
                .unwrap_or_else(|error| panic!("{stmt} should run: {error}"));
        }
        assert_eq!(
            fresh.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).unwrap(),
            super::LATEST_VERSION
        );

        // An upgraded database that stopped at v57 converges to the same
        // schema: drop the four tables, step user_version back, re-run, and
        // confirm they appear and accept rows with the design-doc columns.
        let mut upgraded = unmigrated_database_with_one_account();
        super::migrate(&mut upgraded).unwrap();
        upgraded
            .execute_batch(
                "DROP INDEX imap_change_journal_by_generation;
                 DROP INDEX imap_threads_by_thread;
                 DROP TABLE imap_sync_state;
                 DROP TABLE imap_change_journal;
                 DROP TABLE imap_threads;
                 DROP TABLE imap_thread_aliases;
                 PRAGMA user_version=57;",
            )
            .unwrap();
        super::migrate(&mut upgraded).unwrap();
        upgraded
            .execute(
                "INSERT INTO imap_sync_state(account_id, generation) VALUES ('you@gmail.com', 7)",
                [],
            )
            .unwrap();
        upgraded
            .execute(
                "INSERT INTO imap_change_journal(account_id, generation, thread_id)
                 VALUES ('you@gmail.com', 7, 'imap:you@gmail.com:t:abc')",
                [],
            )
            .unwrap();
        upgraded
            .execute(
                "INSERT INTO imap_threads(account_id, message_id, thread_id)
                 VALUES ('you@gmail.com', 'imap:you@gmail.com:m1', 'imap:you@gmail.com:t:abc')",
                [],
            )
            .unwrap();
        upgraded
            .execute(
                "INSERT INTO imap_thread_aliases(account_id, old_id, new_id)
                 VALUES ('you@gmail.com', 'imap:you@gmail.com:t:old', 'imap:you@gmail.com:t:abc')",
                [],
            )
            .unwrap();
        let (generation, thread_id, alias): (i64, String, String) = upgraded
            .query_row(
                "SELECT s.generation, j.thread_id, a.new_id
                 FROM imap_sync_state s, imap_change_journal j, imap_thread_aliases a
                 WHERE s.account_id='you@gmail.com' AND j.account_id='you@gmail.com'
                   AND a.account_id='you@gmail.com'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(generation, 7);
        assert_eq!(thread_id, "imap:you@gmail.com:t:abc");
        assert_eq!(alias, "imap:you@gmail.com:t:abc");

        // Re-running once more over the already-created tables must not fail.
        upgraded.pragma_update(None, "user_version", 57).unwrap();
        super::migrate(&mut upgraded).unwrap();
        assert_eq!(
            upgraded.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).unwrap(),
            super::LATEST_VERSION
        );
    }

    #[test]
    fn v59_adds_the_imap_message_tokens_table_and_reruns_cleanly() {
        // A fresh database reaches the latest version with the token table and
        // the backfill-tracking column present and queryable.
        let mut fresh = unmigrated_database_with_one_account();
        super::migrate(&mut fresh).unwrap();
        fresh
            .execute("SELECT account_id, message_id, token FROM imap_message_tokens", [])
            .unwrap_or_else(|error| panic!("imap_message_tokens should exist: {error}"));
        fresh
            .execute("SELECT tokens_backfilled FROM imap_sync_state", [])
            .unwrap_or_else(|error| panic!("tokens_backfilled column should exist: {error}"));
        assert_eq!(
            fresh.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).unwrap(),
            super::LATEST_VERSION
        );

        // An upgraded database that stopped at v58, holding a pre-existing
        // sync-state row, converges to the same schema: the new column
        // defaults EXISTING accounts to 0 ("backfill needed"), the token table
        // appears and accepts rows, and the covering indexes exist.
        let mut upgraded = unmigrated_database_with_one_account();
        super::migrate(&mut upgraded).unwrap();
        upgraded
            .execute_batch(
                "DROP INDEX imap_message_tokens_by_token;
                 DROP INDEX imap_message_tokens_by_message;
                 DROP TABLE imap_message_tokens;
                 ALTER TABLE imap_sync_state DROP COLUMN tokens_backfilled;
                 PRAGMA user_version=58;",
            )
            .unwrap();
        // A v58 account that already synced (so it has token-less threads).
        upgraded
            .execute(
                "INSERT INTO imap_sync_state(account_id, generation) VALUES ('you@gmail.com', 3)",
                [],
            )
            .unwrap();
        super::migrate(&mut upgraded).unwrap();
        let backfilled: i64 = upgraded
            .query_row(
                "SELECT tokens_backfilled FROM imap_sync_state WHERE account_id='you@gmail.com'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(backfilled, 0, "an existing account defaults to backfill-needed");
        upgraded
            .execute(
                "INSERT INTO imap_message_tokens(account_id, message_id, token)
                 VALUES ('you@gmail.com','imap:you@gmail.com:m1','<m1@x>')",
                [],
            )
            .unwrap();
        let token: String = upgraded
            .query_row(
                "SELECT token FROM imap_message_tokens WHERE account_id='you@gmail.com' AND message_id='imap:you@gmail.com:m1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(token, "<m1@x>");
        // The covering indexes exist.
        for index in ["imap_message_tokens_by_token", "imap_message_tokens_by_message"] {
            let present: i64 = upgraded
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?1",
                    [index],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(present, 1, "{index} should exist");
        }

        // Re-running once more over the already-created table must not fail.
        upgraded.pragma_update(None, "user_version", 58).unwrap();
        super::migrate(&mut upgraded).unwrap();
        assert_eq!(
            upgraded.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).unwrap(),
            super::LATEST_VERSION
        );
    }

    #[test]
    fn v60_adds_the_multi_mailbox_tables_seeds_hot_threads_and_reruns_cleanly() {
        // A fresh database reaches v60 with both new tables present and the
        // reserved backfill column queryable.
        let mut fresh = unmigrated_database_with_one_account();
        super::migrate(&mut fresh).unwrap();
        fresh
            .execute(
                "SELECT account_id, mailbox, last_exists, last_uidnext, last_sweep_at, backfill_low_uid
                 FROM imap_mailbox_sync_state",
                [],
            )
            .unwrap_or_else(|error| panic!("imap_mailbox_sync_state should exist: {error}"));
        fresh
            .execute("SELECT account_id, thread_id FROM imap_hot_threads", [])
            .unwrap_or_else(|error| panic!("imap_hot_threads should exist: {error}"));
        assert_eq!(
            fresh.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).unwrap(),
            super::LATEST_VERSION
        );

        // An upgraded database stopped at v59 that holds threads with INBOX
        // locations seeds exactly those threads as hot; a Trash-only thread is
        // NOT seeded hot.
        let mut upgraded = unmigrated_database_with_one_account();
        super::migrate(&mut upgraded).unwrap();
        upgraded
            .execute_batch(
                "DROP TABLE imap_mailbox_sync_state;
                 DROP TABLE imap_hot_threads;
                 PRAGMA user_version=59;",
            )
            .unwrap();
        // t_inbox has an INBOX location -> hot after upgrade.
        // t_trash has only a Trash location -> NOT hot.
        upgraded
            .execute_batch(
                "INSERT INTO imap_threads(account_id, message_id, thread_id, created_generation)
                   VALUES ('you@gmail.com','m_in','t_inbox',1),
                          ('you@gmail.com','m_tr','t_trash',1);
                 INSERT INTO imap_locations(account_id, mailbox, uidvalidity, uid, message_id, flags_json, modseq)
                   VALUES ('you@gmail.com','INBOX',1,1,'m_in','[]',NULL),
                          ('you@gmail.com','Trash',1,2,'m_tr','[]',NULL);",
            )
            .unwrap();
        super::migrate(&mut upgraded).unwrap();
        let hot: Vec<String> = {
            let mut stmt = upgraded
                .prepare("SELECT thread_id FROM imap_hot_threads WHERE account_id='you@gmail.com' ORDER BY thread_id")
                .unwrap();
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            rows
        };
        assert_eq!(hot, vec!["t_inbox".to_string()], "only the INBOX-located thread is seeded hot");

        // Re-running once more over the already-created tables must not fail
        // and must not duplicate the seeded hot row (INSERT OR IGNORE on PK).
        upgraded.pragma_update(None, "user_version", 59).unwrap();
        super::migrate(&mut upgraded).unwrap();
        let hot_count: i64 = upgraded
            .query_row(
                "SELECT COUNT(*) FROM imap_hot_threads WHERE account_id='you@gmail.com'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(hot_count, 1, "rerun does not duplicate the seeded hot thread");
        assert_eq!(
            upgraded.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).unwrap(),
            super::LATEST_VERSION
        );
    }

    #[test]
    fn v61_upgrades_v60_aliases_without_losing_state_and_reruns_cleanly() {
        let mut connection = unmigrated_database_with_one_account();
        super::migrate(&mut connection).unwrap();
        connection
            .execute_batch(
                "DROP INDEX imap_thread_aliases_by_survivor;
            PRAGMA user_version=60;
            INSERT INTO imap_thread_aliases VALUES ('you@gmail.com','old','survivor');
            INSERT INTO imap_hot_threads(account_id, thread_id) VALUES ('you@gmail.com','old');",
            )
            .unwrap();
        for _ in 0..2 {
            super::migrate(&mut connection).unwrap();
            let columns: Vec<String> = connection
                .prepare("PRAGMA index_info(imap_thread_aliases_by_survivor)")
                .unwrap()
                .query_map([], |row| row.get(2))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            assert_eq!(columns, ["account_id", "new_id"]);
            let alias: String = connection.query_row(
                "SELECT new_id FROM imap_thread_aliases WHERE account_id='you@gmail.com' AND old_id='old'",
                [], |row| row.get(0)).unwrap();
            assert_eq!(alias, "survivor");
            let markers: i64 = connection.query_row(
                "SELECT COUNT(*) FROM imap_hot_threads WHERE account_id='you@gmail.com' AND thread_id='old'",
                [], |row| row.get(0)).unwrap();
            assert_eq!(markers, 1);
            assert_eq!(
                connection
                    .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                    .unwrap(),
                super::LATEST_VERSION
            );
            connection.pragma_update(None, "user_version", 60).unwrap();
        }
    }

    #[test]
    fn v62_adds_the_scheduling_and_grace_columns_without_losing_state_and_reruns_cleanly() {
        // A fresh database reaches v62 with all four new columns present and
        // queryable at their defaults.
        let mut fresh = unmigrated_database_with_one_account();
        super::migrate(&mut fresh).unwrap();
        fresh
            .execute(
                "SELECT last_visited_at, visited_in_epoch FROM imap_mailbox_sync_state",
                [],
            )
            .unwrap_or_else(|error| panic!("last_visited_at/visited_in_epoch should exist: {error}"));
        fresh
            .execute("SELECT complete_walks FROM imap_sync_state", [])
            .unwrap_or_else(|error| panic!("complete_walks should exist: {error}"));
        fresh
            .execute("SELECT emptied_at_walk FROM imap_hot_threads", [])
            .unwrap_or_else(|error| panic!("emptied_at_walk should exist: {error}"));
        assert_eq!(
            fresh.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).unwrap(),
            super::LATEST_VERSION
        );

        // An upgraded database stopped at v61 keeps its pre-existing sync
        // state, and the new columns arrive at their defaults for existing
        // rows (0 / 0 / NULL — the "never visited / no walks / not empty"
        // starting state).
        let mut upgraded = unmigrated_database_with_one_account();
        super::migrate(&mut upgraded).unwrap();
        upgraded
            .execute_batch(
                "ALTER TABLE imap_mailbox_sync_state DROP COLUMN last_visited_at;
                 ALTER TABLE imap_mailbox_sync_state DROP COLUMN visited_in_epoch;
                 ALTER TABLE imap_sync_state DROP COLUMN complete_walks;
                 ALTER TABLE imap_hot_threads DROP COLUMN emptied_at_walk;
                 PRAGMA user_version=61;",
            )
            .unwrap();
        upgraded
            .execute_batch(
                "INSERT INTO imap_sync_state(account_id, generation) VALUES ('you@gmail.com', 7);
                 INSERT INTO imap_mailbox_sync_state(account_id, mailbox, last_exists, last_uidnext, last_sweep_at)
                   VALUES ('you@gmail.com', 'Archive', 3, 9, 100);
                 INSERT INTO imap_hot_threads(account_id, thread_id) VALUES ('you@gmail.com', 't');",
            )
            .unwrap();
        super::migrate(&mut upgraded).unwrap();
        // Pre-existing state survived.
        let generation: i64 = upgraded
            .query_row(
                "SELECT generation FROM imap_sync_state WHERE account_id='you@gmail.com'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(generation, 7, "the generation survived the column add");
        // New columns defaulted for the existing rows.
        let (visited, walks): (i64, i64) = (
            upgraded
                .query_row(
                    "SELECT last_visited_at FROM imap_mailbox_sync_state WHERE mailbox='Archive'",
                    [],
                    |row| row.get(0),
                )
                .unwrap(),
            upgraded
                .query_row(
                    "SELECT complete_walks FROM imap_sync_state WHERE account_id='you@gmail.com'",
                    [],
                    |row| row.get(0),
                )
                .unwrap(),
        );
        assert_eq!((visited, walks), (0, 0), "new counters default to zero");
        let visited_epoch: i64 = upgraded
            .query_row(
                "SELECT visited_in_epoch FROM imap_mailbox_sync_state WHERE mailbox='Archive'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(visited_epoch, 0, "visited_in_epoch defaults to zero");
        let emptied: Option<i64> = upgraded
            .query_row(
                "SELECT emptied_at_walk FROM imap_hot_threads WHERE thread_id='t'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(emptied, None, "emptied_at_walk defaults to NULL (not empty)");

        // Re-running once more over the already-added columns must not fail.
        upgraded.pragma_update(None, "user_version", 61).unwrap();
        super::migrate(&mut upgraded).unwrap();
        assert_eq!(
            upgraded.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).unwrap(),
            super::LATEST_VERSION
        );
    }

    #[test]
    fn upgrading_past_v21_backfills_every_existing_account_as_gmail() {
        let mut connection = unmigrated_database_with_one_account();
        super::migrate(&mut connection).unwrap();
        assert_eq!(account_provider(&connection, "you@gmail.com"), "gmail");
    }

    #[test]
    fn upgrading_removes_the_default_sync_placeholder_when_an_account_exists() {
        let mut connection = unmigrated_database_with_one_account();
        let before: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sync_state WHERE account_id = 'default'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(before, 1);

        super::migrate(&mut connection).unwrap();

        let after: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sync_state WHERE account_id = 'default'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(after, 0);
    }

    #[test]
    fn v46_queues_existing_search_rows_for_reindexing() {
        let mut connection = unmigrated_database_with_one_account();
        connection
            .execute(
                "INSERT INTO thread_search(thread_id, subject, snippet, participants, body)
                 VALUES ('you@gmail.com:t1', 's', 's', 'p', 'body'), ('you@gmail.com:t2', 's', 's', 'p', 'body')",
                [],
            )
            .unwrap();
        super::migrate(&mut connection).unwrap();
        let queued: i64 = connection
            .query_row("SELECT COUNT(*) FROM pending_search_reindex", [], |row| row.get(0))
            .unwrap();
        assert_eq!(queued, 2);
        super::migrate(&mut connection).unwrap();
    }

    #[test]
    fn v47_requeues_search_rows_already_reindexed_by_v46() {
        let mut connection = unmigrated_database_with_one_account();
        super::migrate(&mut connection).unwrap();
        connection
            .execute_batch(
                "DELETE FROM pending_search_reindex;
                INSERT INTO thread_search(thread_id, subject, snippet, participants, body)
                VALUES ('you@gmail.com:t1', 's', '&lt;html&gt;', 'p', 'body');
                PRAGMA user_version=46;",
            )
            .unwrap();
        super::migrate(&mut connection).unwrap();
        let queued: Vec<String> = connection
            .prepare("SELECT thread_id FROM pending_search_reindex")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(queued, vec!["you@gmail.com:t1".to_string()]);
    }

    #[test]
    fn v48_adds_a_summary_revision_and_keeps_existing_summaries() {
        let mut connection = unmigrated_database_with_one_account();
        super::migrate(&mut connection).unwrap();
        connection
            .execute_batch(
                "INSERT INTO threads(id, provider_thread_id, subject, snippet, participants_json, last_message_at,
                                     account_id, summary, summary_generated_at)
                 VALUES ('you@gmail.com:t1', 't1', 's', 'n', '[]', '2026-01-01T00:00:00Z',
                         'you@gmail.com', 'Existing summary', '2026-01-02T00:00:00Z');
                 PRAGMA user_version=47;",
            )
            .unwrap();
        // Re-running over a column that already exists must not fail.
        super::migrate(&mut connection).unwrap();
        let (summary, revision): (Option<String>, Option<String>) = connection
            .query_row("SELECT summary, summary_revision FROM threads WHERE id='you@gmail.com:t1'", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(summary.as_deref(), Some("Existing summary"));
        assert_eq!(revision, None);
    }

    #[test]
    fn migrating_twice_does_not_fail_on_a_column_already_added() {
        let mut connection = unmigrated_database_with_one_account();
        super::migrate(&mut connection).unwrap();
        super::migrate(&mut connection).unwrap();
        assert_eq!(account_provider(&connection, "you@gmail.com"), "gmail");
    }

    #[test]
    fn fully_migrated_schema_reaches_latest_version_with_the_replicated_sync_tables() {
        let mut connection = unmigrated_database_with_one_account();
        super::migrate(&mut connection).unwrap();
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, super::LATEST_VERSION);
        for table in [
            "sync_spaces",
            "sync_devices",
            "sync_objects",
            "sync_transports",
            "sync_deliveries",
            // v36 replaced v27's event-log tables with the replica's.
            "sync_values",
            "sync_context",
            "sync_local_state",
            "sync_remote_states",
            "sync_retired_objects",
            "pending_entity_materializations",
        ] {
            connection
                .execute(&format!("SELECT * FROM {table}"), [])
                .unwrap_or_else(|error| panic!("table {table} should exist and be queryable: {error}"));
        }
        connection
            .execute("INSERT OR IGNORE INTO sync_local_state(id) VALUES (1)", [])
            .unwrap();
        let generation: i64 = connection
            .query_row("SELECT generation FROM sync_local_state WHERE id=1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(generation, 0);
    }

    #[test]
    fn v49_adds_keep_in_touch_columns_that_default_to_off() {
        let mut connection=unmigrated_database_with_one_account();
        super::migrate(&mut connection).unwrap();
        connection.execute("INSERT INTO contacts(id,display_name,updated_at) VALUES('c1','Ada','2026-01-01T00:00:00Z')",[]).unwrap();
        // Rebuild the v48 shape so the upgrade path runs, not just the guard.
        for column in ["birthday","kit_interval_days","kit_started_at","kit_snoozed_until","kit_snoozed_at","kit_last_touch_at"] {
            connection.execute(&format!("ALTER TABLE contacts DROP COLUMN {column}"),[]).unwrap();
        }
        connection.pragma_update(None,"user_version",48).unwrap();
        super::migrate(&mut connection).unwrap();
        let (name,interval):(String,Option<i64>)=connection.query_row("SELECT display_name,kit_interval_days FROM contacts WHERE id='c1'",[],|row|Ok((row.get(0)?,row.get(1)?))).unwrap();
        assert_eq!((name.as_str(),interval),("Ada",None));
        let version:i64=connection.query_row("PRAGMA user_version",[],|row|row.get(0)).unwrap();
        assert_eq!(version,super::LATEST_VERSION);
    }

    #[test]
    fn v50_starts_the_sent_backfill_from_the_first_page() {
        let mut connection=unmigrated_database_with_one_account();
        super::migrate(&mut connection).unwrap();
        connection.execute("INSERT OR IGNORE INTO sync_state(account_id,cursor) VALUES('me@example.com','history-1')",[]).unwrap();
        // Rebuild the v49 shape so the upgrade path runs, not just the guard.
        for column in ["sent_backfill_page","sent_backfill_offset","sent_backfill_scanned","sent_backfill_completed_at"] {
            connection.execute(&format!("ALTER TABLE sync_state DROP COLUMN {column}"),[]).unwrap();
        }
        connection.pragma_update(None,"user_version",49).unwrap();
        super::migrate(&mut connection).unwrap();
        let progress:(String,Option<String>,i64,i64,Option<String>)=connection.query_row("SELECT cursor,sent_backfill_page,sent_backfill_offset,sent_backfill_scanned,sent_backfill_completed_at FROM sync_state WHERE account_id='me@example.com'",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).unwrap();
        assert_eq!(progress,("history-1".to_string(),None,0,0,None));
        let version:i64=connection.query_row("PRAGMA user_version",[],|row|row.get(0)).unwrap();
        assert_eq!(version,super::LATEST_VERSION);
    }

    #[test]
    fn v52_numbers_contact_addresses_in_saved_order() {
        let mut connection=unmigrated_database_with_one_account();
        super::migrate(&mut connection).unwrap();
        connection.execute("ALTER TABLE contact_addresses DROP COLUMN position",[]).unwrap();
        connection.execute("INSERT INTO contacts(id,display_name,updated_at) VALUES('c1','Ada','2026-01-01T00:00:00Z'),('c2','Bob','2026-01-01T00:00:00Z')",[]).unwrap();
        // Saved order, deliberately not alphabetical.
        for (contact,email) in [("c1","zed@example.com"),("c2","bob@example.com"),("c1","ada@example.com")] {
            connection.execute("INSERT INTO contact_addresses(contact_id,email) VALUES(?1,?2)",[contact,email]).unwrap();
        }
        connection.pragma_update(None,"user_version",51).unwrap();
        super::migrate(&mut connection).unwrap();
        let rows={
            let mut statement=connection.prepare("SELECT contact_id,email,position FROM contact_addresses ORDER BY contact_id,position").unwrap();
            statement.query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,i64>(2)?))).unwrap().collect::<Result<Vec<_>,_>>().unwrap()
        };
        assert_eq!(rows,vec![("c1".into(),"zed@example.com".into(),0),("c1".into(),"ada@example.com".into(),1),("c2".into(),"bob@example.com".into(),0)]);
        connection.pragma_update(None,"user_version",51).unwrap();
        super::migrate(&mut connection).unwrap();
        let version:i64=connection.query_row("PRAGMA user_version",[],|row|row.get(0)).unwrap();
        assert_eq!(version,super::LATEST_VERSION);
    }

    #[test]
    fn v51_adds_contact_groups_and_reruns_cleanly() {
        let mut connection=unmigrated_database_with_one_account();
        super::migrate(&mut connection).unwrap();
        connection.execute_batch("DROP TABLE contact_group_members; DROP TABLE contact_groups;").unwrap();
        connection.pragma_update(None,"user_version",50).unwrap();
        super::migrate(&mut connection).unwrap();
        connection.execute("INSERT INTO contact_groups(id,name,created_at,updated_at) VALUES('g1','Board','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",[]).unwrap();
        // A member's contact may not have synced yet, so no foreign key to contacts.
        connection.execute("INSERT INTO contact_group_members(group_id,contact_id) VALUES('g1','contact:not-here-yet')",[]).unwrap();
        connection.execute("DELETE FROM contact_groups WHERE id='g1'",[]).unwrap();
        let members:i64=connection.query_row("SELECT COUNT(*) FROM contact_group_members",[],|row|row.get(0)).unwrap();
        assert_eq!(members,0);
        connection.pragma_update(None,"user_version",50).unwrap();
        super::migrate(&mut connection).unwrap();
        let version:i64=connection.query_row("PRAGMA user_version",[],|row|row.get(0)).unwrap();
        assert_eq!(version,super::LATEST_VERSION);
    }

    #[test]
    fn v38_migrates_legacy_pinned_contacts_into_profiles() {
        let mut connection=unmigrated_database_with_one_account();
        super::migrate(&mut connection).unwrap();
        connection.execute("INSERT INTO pinned_contacts(account_id,email,display_name,pinned_at) VALUES('you@example.com','jane@example.com','Jane Doe','2026-01-01T00:00:00Z')",[]).unwrap();
        connection.pragma_update(None,"user_version",37).unwrap();
        super::migrate(&mut connection).unwrap();
        let name:String=connection.query_row("SELECT c.display_name FROM contacts c JOIN contact_addresses a ON a.contact_id=c.id WHERE a.email='jane@example.com'",[],|row|row.get(0)).unwrap();
        let email:String=connection.query_row("SELECT email FROM contact_addresses WHERE email='jane@example.com'",[],|row|row.get(0)).unwrap();
        assert_eq!(name,"Jane Doe");
        assert_eq!(email,"jane@example.com");
    }

    #[test]
    fn v43_drops_orphaned_message_metadata_and_keeps_live_payloads() {
        let mut connection = unmigrated_database_with_one_account();
        super::migrate(&mut connection).unwrap();
        connection.execute_batch(
            "INSERT INTO threads(id,provider_thread_id,subject,snippet,participants_json,last_message_at)
                VALUES('a:t1','t1','s','','[]','2026-01-01T00:00:00Z');
            INSERT INTO messages(id,thread_id,sender,recipients_json,sent_at,body_html,body_text)
                VALUES('live','a:t1','x','[]','2026-01-01T00:00:00Z','','');
            INSERT INTO message_metadata(id,payload) VALUES('live','{\"id\":\"live\"}'),('gone','{\"id\":\"gone\"}');",
        ).unwrap();
        connection.pragma_update(None, "user_version", 42).unwrap();
        super::migrate(&mut connection).unwrap();
        let ids: Vec<String> = connection
            .prepare("SELECT id FROM message_metadata ORDER BY id").unwrap()
            .query_map([], |row| row.get(0)).unwrap()
            .collect::<Result<_, _>>().unwrap();
        assert_eq!(ids, ["live"]);
        let legacy: String = connection
            .query_row("SELECT payload FROM message_metadata WHERE id='live'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(legacy, "{\"id\":\"live\"}", "legacy plaintext stays readable until backfilled");
    }

    #[test]
    fn v45_adds_goals_and_an_unlinked_goal_column_to_existing_tasks() {
        let mut connection = unmigrated_database_with_one_account();
        super::migrate(&mut connection).unwrap();
        connection.execute_batch(
            "DROP INDEX tasks_goal;
            ALTER TABLE tasks DROP COLUMN goal_id;
            DROP TABLE goals;
            INSERT INTO tasks(id,account_id,thread_id,subject_snapshot,title,kind,due_kind,status,created_at,updated_at)
                VALUES('t1','you@example.com',NULL,NULL,'Existing task','action','none','in_progress','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z');",
        ).unwrap();
        connection.pragma_update(None, "user_version", 44).unwrap();
        super::migrate(&mut connection).unwrap();
        let (title, goal): (String, Option<String>) = connection
            .query_row("SELECT title, goal_id FROM tasks WHERE id='t1'", [], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap();
        assert_eq!((title.as_str(), goal), ("Existing task", None));
        connection.execute(
            "INSERT INTO goals(id,account_id,title,horizon,period,status,created_at,updated_at)
                VALUES('g1','you@example.com','Grow','quarter','2026-Q4','active','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",
            [],
        ).unwrap();
        assert!(connection.execute(
            "INSERT INTO goals(id,account_id,title,horizon,period,status,created_at,updated_at)
                VALUES('g2','you@example.com','Grow','month','2026-10','active','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",
            [],
        ).is_err());
    }

    #[test]
    fn re_running_the_v41_task_rebuild_keeps_goal_links() {
        let mut connection = unmigrated_database_with_one_account();
        super::migrate(&mut connection).unwrap();
        connection.execute(
            "INSERT INTO tasks(id,account_id,thread_id,subject_snapshot,title,kind,due_kind,status,created_at,updated_at,goal_id)
                VALUES('t1','you@example.com',NULL,NULL,'Linked','action','none','open','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z','g1')",
            [],
        ).unwrap();
        connection.pragma_update(None, "user_version", 40).unwrap();
        super::migrate(&mut connection).unwrap();
        let goal: Option<String> = connection.query_row("SELECT goal_id FROM tasks WHERE id='t1'", [], |row| row.get(0)).unwrap();
        assert_eq!(goal.as_deref(), Some("g1"));
        assert!(connection.query_row("SELECT 1 FROM sqlite_master WHERE name='tasks_goal'", [], |_| Ok(())).is_ok());
    }

    #[test]
    fn fully_migrated_schema_stores_transport_config() {
        let mut connection = unmigrated_database_with_one_account();
        super::migrate(&mut connection).unwrap();
        connection
            .execute(
                "INSERT INTO sync_transports(instance_id,kind,config_json,required,enabled) VALUES ('a','folder','{\"path\":\"/tmp/x\"}',1,1)",
                [],
            )
            .unwrap();
        let config: String = connection
            .query_row("SELECT config_json FROM sync_transports WHERE instance_id='a'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(config, "{\"path\":\"/tmp/x\"}");
    }

    #[test]
    fn fully_migrated_schema_has_the_key_hierarchy_and_enrollment_tables() {
        let mut connection = unmigrated_database_with_one_account();
        super::migrate(&mut connection).unwrap();
        for table in ["sync_epoch_history", "replicated_sync_enrollment_requests", "sync_control_objects_seen"] {
            connection
                .execute(&format!("SELECT * FROM {table}"), [])
                .unwrap_or_else(|error| panic!("table {table} should exist and be queryable: {error}"));
        }
        connection
            .execute("UPDATE sync_spaces SET recovery_x25519_public=X'01'", [])
            .unwrap_or_else(|error| panic!("sync_spaces.recovery_x25519_public should exist: {error}"));
        connection
            .execute("UPDATE sync_devices SET x25519_public=X'02'", [])
            .unwrap_or_else(|error| panic!("sync_devices.x25519_public should exist: {error}"));
    }

    #[test]
    fn re_running_v29_after_a_partial_prior_run_does_not_fail_on_an_existing_column() {
        let mut connection = unmigrated_database_with_one_account();
        super::migrate(&mut connection).unwrap();
        // Simulate a database that already has the v29 columns/tables (for
        // example from an interrupted upgrade) but whose recorded version
        // still predates it.
        connection.pragma_update(None, "user_version", 28).unwrap();
        super::migrate(&mut connection).unwrap();
        let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0)).unwrap();
        assert_eq!(version, super::LATEST_VERSION);
    }

    fn table_exists(connection: &Connection, table: &str) -> bool {
        connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                [table],
                |row| row.get(0),
            )
            .unwrap()
    }

    #[test]
    fn v31_drops_the_retired_account_service_tables_and_keeps_synced_preferences() {
        let mut connection = unmigrated_database_with_one_account();
        super::migrate(&mut connection).unwrap();
        // Rebuild the v30 shape: the legacy tables, with a materialized
        // preference row that must survive the upgrade.
        connection
            .execute_batch(
                "DROP TABLE synced_preferences;
                 CREATE TABLE cloud_account_state (singleton INTEGER PRIMARY KEY, device_id TEXT NOT NULL);
                 CREATE TABLE cloud_sync_metadata (entity_type TEXT, entity_id TEXT);
                 CREATE TABLE cloud_sync_outbox (operation_id TEXT PRIMARY KEY, local_sequence INTEGER);
                 CREATE INDEX cloud_sync_outbox_sequence ON cloud_sync_outbox(local_sequence);
                 CREATE TABLE cloud_sync_conflicts (id TEXT PRIMARY KEY);
                 CREATE TABLE cloud_preferences (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at TEXT NOT NULL);
                 INSERT INTO cloud_preferences VALUES ('portable', '{\"theme\":\"dark\"}', '2026-09-01T00:00:00Z');
                 PRAGMA user_version=30;",
            )
            .unwrap();

        super::migrate(&mut connection).unwrap();

        for table in [
            "cloud_account_state",
            "cloud_sync_metadata",
            "cloud_sync_outbox",
            "cloud_sync_conflicts",
            "cloud_preferences",
        ] {
            assert!(!table_exists(&connection, table), "{table} should be dropped");
        }
        let value: String = connection
            .query_row("SELECT value FROM synced_preferences WHERE key='portable'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, "{\"theme\":\"dark\"}");
    }

    #[test]
    fn re_running_v31_after_it_already_applied_does_not_fail() {
        let mut connection = unmigrated_database_with_one_account();
        super::migrate(&mut connection).unwrap();
        connection.pragma_update(None, "user_version", 30).unwrap();
        super::migrate(&mut connection).unwrap();
        assert!(table_exists(&connection, "synced_preferences"));
        let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0)).unwrap();
        assert_eq!(version, super::LATEST_VERSION);
    }

    #[test]
    fn v42_adds_local_ai_tables_and_reruns_without_losing_usage() {
        let mut connection = unmigrated_database_with_one_account();
        super::migrate(&mut connection).unwrap();
        assert!(table_exists(&connection, "ai_thread_analyses"));
        assert!(table_exists(&connection, "ai_usage"));
        connection.execute(
            "INSERT INTO ai_usage(day,provider,model,requests,input_tokens,output_tokens,reported_cost_requests,reported_cost_usd) VALUES ('2026-09-29','openai','gpt-4o',3,10,5,0,0)",
            [],
        ).unwrap();

        connection.pragma_update(None, "user_version", 41).unwrap();
        super::migrate(&mut connection).unwrap();
        let requests: i64 = connection.query_row("SELECT requests FROM ai_usage", [], |row| row.get(0)).unwrap();
        assert_eq!(requests, 3);
        let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0)).unwrap();
        assert_eq!(version, super::LATEST_VERSION);
    }

    #[test]
    fn v32_adds_local_device_labels_and_reruns_cleanly() {
        let mut connection = unmigrated_database_with_one_account();
        super::migrate(&mut connection).unwrap();
        assert!(table_exists(&connection, "sync_device_labels"));
        connection.execute("INSERT INTO sync_device_labels(device_id,label) VALUES ('d1','Laptop')", []).unwrap();

        connection.pragma_update(None, "user_version", 31).unwrap();
        super::migrate(&mut connection).unwrap();
        let label: String = connection.query_row("SELECT label FROM sync_device_labels WHERE device_id='d1'", [], |row| row.get(0)).unwrap();
        assert_eq!(label, "Laptop");
        let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0)).unwrap();
        assert_eq!(version, super::LATEST_VERSION);
    }

    #[test]
    fn v34_adds_join_code_tables_and_reruns_cleanly() {
        let mut connection = unmigrated_database_with_one_account();
        super::migrate(&mut connection).unwrap();
        assert!(table_exists(&connection, "replicated_sync_invitations"));
        assert!(table_exists(&connection, "replicated_sync_invitation_redemptions"));
        connection
            .execute(
                "INSERT INTO replicated_sync_invitations(invitation_cid,direction,status,inviter_device_id,created_at,expires_at_ms)
                 VALUES ('c1','outgoing','open','d1','2026-09-23T00:00:00Z',1)",
                [],
            )
            .unwrap();
        connection.execute("INSERT INTO sync_device_labels(device_id,label) VALUES ('d1','Laptop')", []).unwrap();

        connection.pragma_update(None, "user_version", 33).unwrap();
        super::migrate(&mut connection).unwrap();
        let status: String = connection
            .query_row("SELECT status FROM replicated_sync_invitations WHERE invitation_cid='c1'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(status, "open");
        let label: String = connection.query_row("SELECT label FROM sync_device_labels WHERE device_id='d1'", [], |row| row.get(0)).unwrap();
        assert_eq!(label, "Laptop");
        let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0)).unwrap();
        assert_eq!(version, super::LATEST_VERSION);
    }

    #[test]
    fn v25_task_migration_preserves_linked_tasks_and_allows_standalone_tasks() {
        let mut connection = unmigrated_database_with_one_account();
        super::migrate(&mut connection).unwrap();
        connection
            .execute_batch(
                "DROP INDEX tasks_status_due;
             DROP INDEX tasks_account_status;
             DROP INDEX tasks_thread;
             DROP TABLE tasks;
             CREATE TABLE tasks (
                id TEXT PRIMARY KEY, account_id TEXT NOT NULL, thread_id TEXT NOT NULL,
                source_message_id TEXT, subject_snapshot TEXT NOT NULL, title TEXT NOT NULL,
                notes TEXT, kind TEXT NOT NULL, due_kind TEXT NOT NULL, due_value TEXT,
                time_zone TEXT, repeat_interval_days INTEGER, status TEXT NOT NULL,
                completion_source TEXT, evidence_text TEXT, wait_after TEXT,
                created_at TEXT NOT NULL, updated_at TEXT NOT NULL, completed_at TEXT
             );
             CREATE INDEX tasks_status_due ON tasks(status, due_value, updated_at);
             CREATE INDEX tasks_account_status ON tasks(account_id, status, updated_at);
             CREATE INDEX tasks_thread ON tasks(thread_id, status);
             INSERT INTO tasks(id, account_id, thread_id, subject_snapshot, title, kind,
                due_kind, status, created_at, updated_at)
             VALUES ('linked', 'you@gmail.com', 'thread-1', 'Subject', 'Existing task',
                'action', 'none', 'open', '2026-09-20T00:00:00Z', '2026-09-20T00:00:00Z');
             PRAGMA user_version=24;",
            )
            .unwrap();

        super::migrate(&mut connection).unwrap();

        let preserved: (String, String) = connection
            .query_row(
                "SELECT thread_id, subject_snapshot FROM tasks WHERE id='linked'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(preserved, ("thread-1".into(), "Subject".into()));
        connection
            .execute(
                "INSERT INTO tasks(id, account_id, thread_id, subject_snapshot, title, kind,
                due_kind, status, created_at, updated_at)
             VALUES ('standalone', 'you@gmail.com', NULL, NULL, 'Standalone task',
                'action', 'none', 'open', '2026-09-20T00:00:00Z', '2026-09-20T00:00:00Z')",
                [],
            )
            .unwrap();
    }

    #[test]
    fn v41_task_migration_preserves_tasks_and_admits_in_progress() {
        let mut connection = unmigrated_database_with_one_account();
        super::migrate(&mut connection).unwrap();
        connection
            .execute_batch(
                "DROP INDEX tasks_status_due;
             DROP INDEX tasks_account_status;
             DROP INDEX tasks_thread;
             DROP TABLE tasks;
             CREATE TABLE tasks (
                id TEXT PRIMARY KEY, account_id TEXT NOT NULL, thread_id TEXT,
                source_message_id TEXT, subject_snapshot TEXT, title TEXT NOT NULL,
                notes TEXT, kind TEXT NOT NULL, due_kind TEXT NOT NULL, due_value TEXT,
                time_zone TEXT, repeat_interval_days INTEGER,
                status TEXT NOT NULL CHECK(status IN ('open', 'completed', 'cancelled')),
                completion_source TEXT, evidence_text TEXT, wait_after TEXT,
                created_at TEXT NOT NULL, updated_at TEXT NOT NULL, completed_at TEXT
             );
             CREATE INDEX tasks_status_due ON tasks(status, due_value, updated_at);
             CREATE INDEX tasks_account_status ON tasks(account_id, status, updated_at);
             CREATE INDEX tasks_thread ON tasks(thread_id, status);
             INSERT INTO tasks(id, account_id, thread_id, subject_snapshot, title, kind,
                due_kind, status, created_at, updated_at, completed_at)
             VALUES ('done', 'you@gmail.com', NULL, NULL, 'Finished task',
                'action', 'none', 'completed', '2026-09-20T00:00:00Z', '2026-09-20T00:00:00Z',
                '2026-09-21T00:00:00Z');
             PRAGMA user_version=40;",
            )
            .unwrap();

        super::migrate(&mut connection).unwrap();

        let preserved: (String, String) = connection
            .query_row("SELECT status, completed_at FROM tasks WHERE id='done'", [], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap();
        assert_eq!(preserved, ("completed".into(), "2026-09-21T00:00:00Z".into()));
        connection
            .execute("UPDATE tasks SET status='in_progress', completed_at=NULL WHERE id='done'", [])
            .unwrap();
        assert!(connection
            .execute("UPDATE tasks SET status='blocked' WHERE id='done'", [])
            .is_err());
        let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0)).unwrap();
        assert_eq!(version, super::LATEST_VERSION);
    }
}
