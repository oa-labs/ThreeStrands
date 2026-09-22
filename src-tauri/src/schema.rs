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
pub(crate) const LATEST_VERSION: i64 = 32;

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
        // Device names are local labels, never replicated: control-plane
        // objects are public and signed over their exact fields (a new
        // field would fail verification on older clients), and an unknown
        // synchronized entity type would stall older clients' pulls.
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS sync_device_labels (
                device_id TEXT PRIMARY KEY,
                label TEXT NOT NULL
            );",
        )
        .map_err(error)?;
        tx.pragma_update(None, "user_version", 32).map_err(error)?;
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
    fn upgrading_past_v21_backfills_every_existing_account_as_gmail() {
        let mut connection = unmigrated_database_with_one_account();
        super::migrate(&mut connection).unwrap();
        assert_eq!(account_provider(&connection, "you@gmail.com"), "gmail");
    }

    #[test]
    fn migrating_twice_does_not_fail_on_a_column_already_added() {
        let mut connection = unmigrated_database_with_one_account();
        super::migrate(&mut connection).unwrap();
        super::migrate(&mut connection).unwrap();
        assert_eq!(account_provider(&connection, "you@gmail.com"), "gmail");
    }

    #[test]
    fn v27_creates_the_replicated_sync_tables_and_reaches_latest_version() {
        let mut connection = unmigrated_database_with_one_account();
        super::migrate(&mut connection).unwrap();
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, super::LATEST_VERSION);
        for table in [
            "sync_spaces",
            "sync_devices",
            "sync_events",
            "sync_objects",
            "sync_operations",
            "sync_operation_parents",
            "sync_field_frontier",
            "sync_transports",
            "sync_deliveries",
        ] {
            connection
                .execute(&format!("SELECT * FROM {table}"), [])
                .unwrap_or_else(|error| panic!("table {table} should exist and be queryable: {error}"));
        }
    }

    #[test]
    fn v28_adds_transport_config_storage() {
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
    fn v29_adds_the_key_hierarchy_and_enrollment_tables() {
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
}
