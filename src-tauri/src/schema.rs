//! Database schema and migration ownership.
//!
//! Connection lifecycle and locking belong to `db`; all changes to the shape
//! of persisted data belong here so the current schema and its upgrade path
//! are reviewed together.

use chrono::Utc;
use rusqlite::Connection;

/// The version `INITIAL_SCHEMA` builds: the fully-migrated v74 baseline. A
/// database older than this is refused (see [`migrate`]); a fresh one starts
/// here and runs every migration above it.
pub(crate) const BASELINE_VERSION: i64 = 74;

/// The current schema version. A new schema change adds an
/// `if version < N { ... }` block in [`migrate`] and bumps this constant to N.
pub(crate) const LATEST_VERSION: i64 = 81;

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

-- This baseline is the fully-migrated v74 schema, generated from a real
-- 0->74 run of migrate() (see schema squash). A fresh database is born at
-- v74; migrate() only applies versions above the baseline. Keep this in
-- lockstep with LATEST_VERSION and the user_version stamp at the end.

-- Tables
CREATE TABLE IF NOT EXISTS account_labels (
                account_id TEXT NOT NULL,
                label_id   TEXT NOT NULL,
                name       TEXT NOT NULL,
                kind       TEXT NOT NULL,
                PRIMARY KEY (account_id, label_id)
            );

CREATE TABLE IF NOT EXISTS accounts (
    email TEXT PRIMARY KEY,
    display_name TEXT,
    color TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('connected', 'needs_reauth')),
    sort_order INTEGER NOT NULL,
    connected_at TEXT NOT NULL,
    last_synced_at TEXT
, provider TEXT NOT NULL DEFAULT 'gmail', nickname TEXT, on_this_device INTEGER NOT NULL DEFAULT 1);

CREATE TABLE IF NOT EXISTS ai_thread_analyses (
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

CREATE TABLE IF NOT EXISTS calendar_accounts (
    email TEXT PRIMARY KEY,
    connected_at TEXT NOT NULL
, selection_initialized INTEGER NOT NULL DEFAULT 0, status TEXT NOT NULL DEFAULT 'connected' CHECK(status IN ('connected','needs_reauth')));

CREATE TABLE IF NOT EXISTS calendar_selections (
                account_id TEXT NOT NULL REFERENCES calendar_accounts(email) ON DELETE CASCADE,
                calendar_id TEXT NOT NULL,
                PRIMARY KEY (account_id, calendar_id)
            );

CREATE TABLE IF NOT EXISTS compose_settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS contact_addresses (
                contact_id TEXT NOT NULL REFERENCES contacts(id) ON DELETE CASCADE,
                email TEXT NOT NULL UNIQUE, position INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(contact_id, email)
            );

CREATE TABLE IF NOT EXISTS contact_group_members (
                group_id TEXT NOT NULL REFERENCES contact_groups(id) ON DELETE CASCADE,
                contact_id TEXT NOT NULL,
                PRIMARY KEY(group_id, contact_id)
            );

CREATE TABLE IF NOT EXISTS contact_groups (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

CREATE TABLE IF NOT EXISTS contact_interactions (
                message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
                thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
                account_id TEXT NOT NULL,
                email TEXT NOT NULL,
                display_name TEXT,
                direction TEXT NOT NULL CHECK(direction IN ('sent','received')),
                sent_at TEXT NOT NULL,
                PRIMARY KEY(message_id,email,direction)
            );

CREATE TABLE IF NOT EXISTS contact_suggestion_suppressions(
            email TEXT PRIMARY KEY NOT NULL
        );

CREATE TABLE IF NOT EXISTS contacts (
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
            , birthday TEXT, kit_interval_days INTEGER, kit_started_at TEXT, kit_snoozed_until TEXT, kit_snoozed_at TEXT, kit_last_touch_at TEXT);

CREATE TABLE IF NOT EXISTS drafts(id TEXT PRIMARY KEY, revision INTEGER NOT NULL, payload TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS file_attachments (
                message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
                attachment_id TEXT NOT NULL,
                filename TEXT NOT NULL,
                mime_type TEXT NOT NULL,
                size INTEGER NOT NULL,
                PRIMARY KEY(message_id, attachment_id)
            );

CREATE TABLE IF NOT EXISTS file_stars (
                message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
                attachment_id TEXT NOT NULL,
                PRIMARY KEY(message_id, attachment_id)
            );

CREATE TABLE IF NOT EXISTS goals (
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

CREATE TABLE IF NOT EXISTS imap_account_settings (
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
                pinned_fingerprints_json TEXT NOT NULL DEFAULT '{}', -- {"host:port": "SHA-256 hex"}
                server_saves_sent INTEGER NOT NULL DEFAULT 0
            );

CREATE TABLE IF NOT EXISTS imap_bodies (
                account_id TEXT NOT NULL,
                message_id TEXT NOT NULL,        -- the stable cross-account id
                raw BLOB NOT NULL,               -- the whole RFC 5322 message
                size INTEGER NOT NULL,           -- raw.len(), for cache accounting
                fetched_at INTEGER NOT NULL,     -- unix seconds the body was cached
                PRIMARY KEY (account_id, message_id)
            );

CREATE TABLE IF NOT EXISTS imap_change_journal (
                account_id TEXT NOT NULL,
                generation INTEGER NOT NULL,
                thread_id TEXT NOT NULL,
                PRIMARY KEY (account_id, generation, thread_id)
            );

CREATE TABLE IF NOT EXISTS imap_copy_intents (
                account_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                destination TEXT NOT NULL,
                identity_id TEXT NOT NULL,
                PRIMARY KEY (account_id, message_id, destination)
            );

CREATE TABLE IF NOT EXISTS imap_hot_threads (
                account_id TEXT NOT NULL,
                thread_id TEXT NOT NULL, emptied_at_walk INTEGER, expiry_journaled INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (account_id, thread_id)
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

CREATE TABLE IF NOT EXISTS imap_mailbox_sync_state (
                account_id TEXT NOT NULL,
                mailbox TEXT NOT NULL,
                last_exists INTEGER NOT NULL DEFAULT 0,
                last_uidnext INTEGER NOT NULL DEFAULT 0,
                last_sweep_at INTEGER NOT NULL DEFAULT 0,
                backfill_low_uid INTEGER, last_visited_at INTEGER NOT NULL DEFAULT 0, visited_in_epoch INTEGER NOT NULL DEFAULT 0, last_attempt INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (account_id, mailbox)
            );

CREATE TABLE IF NOT EXISTS imap_mailboxes (
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

CREATE TABLE IF NOT EXISTS imap_message_tokens (
                account_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                token TEXT NOT NULL,
                PRIMARY KEY (account_id, message_id, token)
            );

CREATE TABLE IF NOT EXISTS imap_search_pins (
                account_id TEXT NOT NULL,
                mailbox TEXT NOT NULL,
                uidvalidity INTEGER NOT NULL,
                uid INTEGER NOT NULL,
                pinned_at INTEGER NOT NULL,      -- unix seconds the pin was (re)acquired
                PRIMARY KEY (account_id, mailbox, uidvalidity, uid)
            );

CREATE TABLE IF NOT EXISTS imap_sent_copy_detection (
                account_id TEXT PRIMARY KEY,
                server_saves_sent INTEGER NOT NULL,
                decided_at INTEGER NOT NULL
            );

CREATE TABLE IF NOT EXISTS imap_sync_state (
                account_id TEXT PRIMARY KEY,
                generation INTEGER NOT NULL DEFAULT 0
            , tokens_backfilled INTEGER NOT NULL DEFAULT 0, complete_walks INTEGER NOT NULL DEFAULT 0);

CREATE TABLE IF NOT EXISTS imap_thread_aliases (
                account_id TEXT NOT NULL,
                old_id TEXT NOT NULL,
                new_id TEXT NOT NULL,
                PRIMARY KEY (account_id, old_id)
            );

CREATE TABLE IF NOT EXISTS imap_threads (
                account_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                thread_id TEXT NOT NULL,
                created_generation INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (account_id, message_id)
            );

CREATE TABLE IF NOT EXISTS message_metadata(id TEXT PRIMARY KEY, payload TEXT NOT NULL, payload_z BLOB);

CREATE TABLE IF NOT EXISTS messages (
    id TEXT PRIMARY KEY,
    thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    sender TEXT NOT NULL,
    recipients_json TEXT NOT NULL,
    sent_at TEXT NOT NULL,
    body_html TEXT NOT NULL,
    body_text TEXT NOT NULL
, unsubscribe_json TEXT, unread INTEGER NOT NULL DEFAULT 0, attachments_json TEXT NOT NULL DEFAULT '[]', body_html_z BLOB, body_text_z BLOB);

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
, target_message_id TEXT, next_attempt_at TEXT);

CREATE TABLE IF NOT EXISTS outbox_messages(id TEXT PRIMARY KEY, draft_id TEXT NOT NULL, revision INTEGER NOT NULL, account TEXT NOT NULL, state TEXT NOT NULL, deadline INTEGER NOT NULL, payload TEXT NOT NULL, raw BLOB NOT NULL, error TEXT, provider_id TEXT, attempts INTEGER NOT NULL DEFAULT 0, last_attempt_at INTEGER, archive_on_send INTEGER NOT NULL DEFAULT 0, schedule_json TEXT, sent_copy_state TEXT, sent_copy_attempts INTEGER NOT NULL DEFAULT 0, sent_copy_next_at INTEGER, sent_copy_append_attempted INTEGER NOT NULL DEFAULT 0, sent_copy_error TEXT, UNIQUE(draft_id, revision));

CREATE TABLE IF NOT EXISTS pending_entity_materializations (
                entity_type TEXT NOT NULL,
                entity_id TEXT NOT NULL,
                reason TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY(entity_type, entity_id)
            );

CREATE TABLE IF NOT EXISTS pending_search_reindex (thread_id TEXT PRIMARY KEY);

CREATE TABLE IF NOT EXISTS pinned_contacts (
    account_id TEXT NOT NULL,
    email TEXT NOT NULL,
    display_name TEXT,
    pinned_at TEXT NOT NULL,
    PRIMARY KEY (account_id, email)
);

CREATE TABLE IF NOT EXISTS quarantined_messages (
                account_id TEXT NOT NULL,
                provider_thread_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                error TEXT NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY (account_id, message_id)
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

CREATE TABLE IF NOT EXISTS replicated_sync_invitations (
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

CREATE TABLE IF NOT EXISTS scheduled_send_summaries (id TEXT PRIMARY KEY, report TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS snippets (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                body TEXT NOT NULL,
                created_at TEXT NOT NULL
            , updated_at TEXT);

CREATE TABLE IF NOT EXISTS split_inboxes (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                match_kind TEXT NOT NULL CHECK(match_kind IN ('domain', 'label', 'pattern')),
                match_value TEXT NOT NULL,
                sort_order INTEGER NOT NULL,
                created_at TEXT NOT NULL
            , account_id TEXT NOT NULL DEFAULT '', updated_at TEXT);

CREATE TABLE IF NOT EXISTS sync_context (
                device_id TEXT PRIMARY KEY,
                counter INTEGER NOT NULL
            );

CREATE TABLE IF NOT EXISTS sync_control_objects_seen (
                cid TEXT PRIMARY KEY,
                object_kind TEXT NOT NULL
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

CREATE TABLE IF NOT EXISTS sync_device_labels (
                device_id TEXT PRIMARY KEY,
                label TEXT NOT NULL
            );

CREATE TABLE IF NOT EXISTS sync_devices (
                device_id TEXT PRIMARY KEY,
                public_key BLOB,
                status TEXT NOT NULL DEFAULT 'active' CHECK(status IN ('active','revoked')),
                added_by_operation TEXT,
                revoked_by_operation TEXT
            , x25519_public BLOB, is_self INTEGER NOT NULL DEFAULT 0);

CREATE TABLE IF NOT EXISTS sync_epoch_history (
                key_epoch INTEGER PRIMARY KEY,
                activated_at TEXT NOT NULL,
                source_cid TEXT
            );

CREATE TABLE IF NOT EXISTS sync_head_publications (
                transport_instance_id TEXT PRIMARY KEY,
                head_content TEXT NOT NULL,
                published_at_ms INTEGER NOT NULL
            );

CREATE TABLE IF NOT EXISTS sync_local_state (
                id INTEGER PRIMARY KEY CHECK(id = 1),
                dirty INTEGER NOT NULL DEFAULT 1,
                state_sequence INTEGER NOT NULL DEFAULT 0,
                sealed_epoch INTEGER,
                last_change_at TEXT
            , generation INTEGER NOT NULL DEFAULT 0);

CREATE TABLE IF NOT EXISTS sync_notices (
                kind TEXT PRIMARY KEY,
                created_at TEXT NOT NULL
            );

CREATE TABLE IF NOT EXISTS sync_objects (
                cid TEXT PRIMARY KEY,
                object_kind TEXT NOT NULL,
                state_sequence INTEGER,
                chunk_index INTEGER NOT NULL,
                chunk_count INTEGER NOT NULL,
                bytes BLOB NOT NULL
            );

CREATE TABLE IF NOT EXISTS sync_pending_keychain_cleanup (
                id INTEGER PRIMARY KEY CHECK(id = 1),
                highest_epoch INTEGER NOT NULL
            );

CREATE TABLE IF NOT EXISTS sync_recovery (
                account_id TEXT PRIMARY KEY,
                history_id TEXT NOT NULL
            );

CREATE TABLE IF NOT EXISTS sync_recovery_threads (
                account_id TEXT NOT NULL,
                provider_thread_id TEXT NOT NULL,
                PRIMARY KEY (account_id, provider_thread_id)
            );

CREATE TABLE IF NOT EXISTS sync_remote_states (
                device_id TEXT PRIMARY KEY,
                state_sequence INTEGER NOT NULL DEFAULT 0,
                merged_at TEXT,
                last_head_published_at_ms INTEGER,
                last_head_seen_at_ms INTEGER,
                last_head_epoch INTEGER,
                keys_shared_epoch INTEGER
            );

CREATE TABLE IF NOT EXISTS sync_retired_objects (
                cid TEXT NOT NULL,
                transport_instance_id TEXT NOT NULL,
                state_sequence INTEGER NOT NULL,
                PRIMARY KEY(cid, transport_instance_id)
            );

CREATE TABLE IF NOT EXISTS sync_spaces (
                id TEXT PRIMARY KEY,
                active_epoch INTEGER NOT NULL DEFAULT 0,
                -- NULL until the Phase 5 key hierarchy generates a real
                -- recovery keypair; the plan's suggested schema treats this
                -- as required, but nothing can populate it yet.
                recovery_public_key BLOB,
                lamport INTEGER NOT NULL DEFAULT 0,
                enabled INTEGER NOT NULL DEFAULT 0,
                last_error TEXT
            , recovery_x25519_public BLOB);

CREATE TABLE IF NOT EXISTS sync_state (
    account_id TEXT PRIMARY KEY,
    cursor TEXT,
    last_successful_sync TEXT,
    last_error TEXT
, last_reconciled_at TEXT, sent_backfill_page TEXT, sent_backfill_offset INTEGER NOT NULL DEFAULT 0, sent_backfill_scanned INTEGER NOT NULL DEFAULT 0, sent_backfill_completed_at TEXT);

CREATE TABLE IF NOT EXISTS sync_transports (
                instance_id TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                account_id TEXT,
                required INTEGER NOT NULL DEFAULT 1,
                enabled INTEGER NOT NULL DEFAULT 0,
                cursor TEXT,
                last_success_at TEXT,
                last_error TEXT
            , config_json TEXT);

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

CREATE TABLE IF NOT EXISTS synced_preferences (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

CREATE TABLE IF NOT EXISTS tasks (
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
                completed_at TEXT
            , goal_id TEXT);

CREATE VIRTUAL TABLE IF NOT EXISTS thread_search USING fts5(
    thread_id UNINDEXED,
    subject,
    snippet,
    participants,
    body,
    tokenize = 'unicode61 remove_diacritics 2'
);

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
, trashed INTEGER NOT NULL DEFAULT 0, account_id TEXT NOT NULL DEFAULT 'default', summary TEXT, summary_generated_at TEXT, has_attachments INTEGER NOT NULL DEFAULT 0, last_received_at TEXT NOT NULL DEFAULT '', summary_revision TEXT);

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

-- `default` is only the pre-connect placeholder. Recreating it after a real
-- account has been adopted makes the next identity refresh collide with that
-- account's existing sync-state primary key.
INSERT OR IGNORE INTO sync_state(account_id)
SELECT 'default' WHERE NOT EXISTS (SELECT 1 FROM accounts);

-- Indexes
CREATE INDEX IF NOT EXISTS account_labels_by_name
                ON account_labels(account_id, name COLLATE NOCASE);

CREATE INDEX IF NOT EXISTS contact_group_members_contact ON contact_group_members(contact_id);

CREATE INDEX IF NOT EXISTS contact_interactions_account_email_time ON contact_interactions(account_id,email,sent_at DESC);

CREATE INDEX IF NOT EXISTS contact_interactions_email_time ON contact_interactions(email,sent_at DESC);

CREATE INDEX IF NOT EXISTS contact_interactions_thread ON contact_interactions(thread_id,sent_at DESC);

CREATE INDEX IF NOT EXISTS contacts_favorite_updated ON contacts(favorite, updated_at DESC);

CREATE INDEX IF NOT EXISTS file_attachments_by_name ON file_attachments(filename COLLATE NOCASE);

CREATE INDEX IF NOT EXISTS goals_account_period ON goals(account_id, period, status);

CREATE INDEX IF NOT EXISTS imap_change_journal_by_generation
                ON imap_change_journal(account_id, generation);

CREATE INDEX IF NOT EXISTS imap_locations_by_message
                ON imap_locations(account_id, message_id);

CREATE INDEX IF NOT EXISTS imap_mailbox_sync_state_by_attempt
                 ON imap_mailbox_sync_state(account_id, last_attempt);

CREATE INDEX IF NOT EXISTS imap_message_tokens_by_message
                ON imap_message_tokens(account_id, message_id);

CREATE INDEX IF NOT EXISTS imap_message_tokens_by_token
                ON imap_message_tokens(account_id, token);

CREATE INDEX IF NOT EXISTS imap_search_pins_by_account
                ON imap_search_pins(account_id);

CREATE INDEX IF NOT EXISTS imap_thread_aliases_by_survivor
                 ON imap_thread_aliases(account_id, new_id);

CREATE INDEX IF NOT EXISTS imap_threads_by_thread
                ON imap_threads(account_id, thread_id);

CREATE INDEX IF NOT EXISTS messages_by_thread
ON messages(thread_id, sent_at);

CREATE INDEX IF NOT EXISTS messages_file_order ON messages(sent_at DESC,id);

CREATE INDEX IF NOT EXISTS mutations_pending
                ON mutations(state, next_attempt_at, created_at);

CREATE INDEX IF NOT EXISTS outbox_sent_copy_due
                 ON outbox_messages(sent_copy_state, sent_copy_next_at);

CREATE INDEX IF NOT EXISTS quarantined_messages_account_time
                ON quarantined_messages(account_id, created_at DESC);

CREATE INDEX IF NOT EXISTS replicated_sync_invitation_redemptions_invitation
                ON replicated_sync_invitation_redemptions(invitation_cid);

CREATE INDEX IF NOT EXISTS tasks_account_status ON tasks(account_id, status, updated_at);

CREATE INDEX IF NOT EXISTS tasks_goal ON tasks(goal_id);

CREATE INDEX IF NOT EXISTS tasks_status_due ON tasks(status, due_value, updated_at);

CREATE INDEX IF NOT EXISTS tasks_thread ON tasks(thread_id, status);

CREATE INDEX IF NOT EXISTS threads_account_mailbox_order
                ON threads(account_id, trashed, archived, last_received_at DESC);

CREATE UNIQUE INDEX IF NOT EXISTS threads_account_provider_unique
                ON threads(account_id, provider_thread_id);

CREATE INDEX IF NOT EXISTS threads_inbox_order ON threads(archived, last_received_at DESC);

CREATE INDEX IF NOT EXISTS threads_mailbox_order
                ON threads(trashed, archived, last_received_at DESC);

CREATE INDEX IF NOT EXISTS threads_trashed ON threads(trashed);

CREATE INDEX IF NOT EXISTS triage_events_account_sender
ON triage_events(account_id, sender_email, created_at);

CREATE INDEX IF NOT EXISTS triage_events_account_time
ON triage_events(account_id, created_at);

CREATE INDEX IF NOT EXISTS unsubscribe_requests_message
                ON unsubscribe_requests(message_id, created_at);

-- Triggers
CREATE TRIGGER IF NOT EXISTS imap_search_pins_follow_location
                AFTER DELETE ON imap_locations
                BEGIN
                    DELETE FROM imap_search_pins
                     WHERE account_id = OLD.account_id AND mailbox = OLD.mailbox
                       AND uidvalidity = OLD.uidvalidity AND uid = OLD.uid;
                END;

CREATE TRIGGER IF NOT EXISTS files_after_message_insert AFTER INSERT ON messages BEGIN
                INSERT OR REPLACE INTO file_attachments
                SELECT NEW.id, json_extract(a.value,'$.id'), json_extract(a.value,'$.filename'),
                       CASE WHEN json_type(a.value,'$.mimeType')='text' THEN json_extract(a.value,'$.mimeType') ELSE 'application/octet-stream' END, CASE WHEN json_type(a.value,'$.size')='integer' AND json_extract(a.value,'$.size')>=0 THEN json_extract(a.value,'$.size') ELSE 0 END
                  FROM json_each(CASE WHEN json_valid(NEW.attachments_json) THEN NEW.attachments_json ELSE '[]' END) a
                 WHERE a.type='object' AND COALESCE(json_extract(a.value,'$.inline'),0)=0
                   AND json_type(a.value,'$.id')='text' AND json_type(a.value,'$.filename')='text';
            END;

CREATE TRIGGER IF NOT EXISTS files_after_message_update AFTER UPDATE OF attachments_json ON messages BEGIN
                DELETE FROM file_attachments WHERE message_id=NEW.id;
                INSERT OR REPLACE INTO file_attachments
                SELECT NEW.id, json_extract(a.value,'$.id'), json_extract(a.value,'$.filename'),
                       CASE WHEN json_type(a.value,'$.mimeType')='text' THEN json_extract(a.value,'$.mimeType') ELSE 'application/octet-stream' END, CASE WHEN json_type(a.value,'$.size')='integer' AND json_extract(a.value,'$.size')>=0 THEN json_extract(a.value,'$.size') ELSE 0 END
                  FROM json_each(CASE WHEN json_valid(NEW.attachments_json) THEN NEW.attachments_json ELSE '[]' END) a
                 WHERE a.type='object' AND COALESCE(json_extract(a.value,'$.inline'),0)=0
                   AND json_type(a.value,'$.id')='text' AND json_type(a.value,'$.filename')='text';
                DELETE FROM file_stars WHERE message_id=NEW.id AND attachment_id NOT IN
                    (SELECT attachment_id FROM file_attachments WHERE message_id=NEW.id);
            END;

"#;

fn error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

pub(crate) fn migrate(connection: &mut Connection) -> Result<(), String> {
    let tx = connection.transaction().map_err(error)?;
    let stored: i64 = tx
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(error)?;
    // INITIAL_SCHEMA builds the fully-migrated v74 baseline but does NOT stamp
    // user_version itself (stamping on every open would clobber an existing
    // database's version). The historical 0->74 migration chain has been
    // retired: this build only creates databases at the baseline and applies
    // migrations ABOVE it. A database older than the baseline must be
    // converted out of band.
    //
    //   version == 0              -> a brand-new baseline database; its tables
    //                                match the baseline, so migrate from there.
    //   0 < version < BASELINE    -> created by a build predating the squash;
    //                                refuse, pointing at the converter.
    //   BASELINE <= version       -> apply each migration above it.
    //   version >  LATEST         -> created by a newer build; refuse rather
    //                                than silently run against unknown schema.
    let version = if stored == 0 { BASELINE_VERSION } else { stored };
    if version < BASELINE_VERSION {
        return Err(format!(
            "database schema v{version} predates the v{BASELINE_VERSION} baseline and can \
             no longer be upgraded in-app; convert it with threestrands-cli before opening"
        ));
    }
    if version > LATEST_VERSION {
        return Err(format!(
            "database schema v{version} is newer than this build's v{LATEST_VERSION} \
             schema; update the application"
        ));
    }
    if version < 75 {
        // Muted senders: a per-account rule whose new mail sync marks read
        // and archives, and a device-local log of the messages each rule has
        // acted on, so a conversation the user moves back is left alone.
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS sender_rules (
                 id TEXT PRIMARY KEY,
                 account_id TEXT NOT NULL,
                 match_kind TEXT NOT NULL CHECK(match_kind IN ('address', 'domain')),
                 match_value TEXT NOT NULL,
                 action TEXT NOT NULL CHECK(action IN ('mute')),
                 created_at TEXT NOT NULL,
                 updated_at TEXT
             );
             CREATE INDEX IF NOT EXISTS sender_rules_by_account ON sender_rules(account_id);
             CREATE TABLE IF NOT EXISTS sender_rule_actions (
                 account_id TEXT NOT NULL,
                 message_id TEXT NOT NULL,
                 rule_id TEXT NOT NULL REFERENCES sender_rules(id) ON DELETE CASCADE,
                 acted_at TEXT NOT NULL,
                 PRIMARY KEY(account_id, message_id)
             );",
        )
        .map_err(error)?;
    }
    if version < 76 {
        tx.execute_batch("CREATE TABLE signatures(id TEXT PRIMARY KEY, payload TEXT NOT NULL); CREATE TABLE signature_defaults(id TEXT PRIMARY KEY, payload TEXT NOT NULL);").map_err(error)?;
    }
    if version < 77 {
        // Cc and Bcc recipients, so search's `to:` matches them as Gmail's
        // does. Existing mail is filled in from each message's stored payload
        // by the bounded search-reindex queue (`reindex_next_search_batch`).
        tx.execute_batch(
            "ALTER TABLE messages ADD COLUMN cc_bcc_json TEXT NOT NULL DEFAULT '[]';
             INSERT OR IGNORE INTO pending_search_reindex(thread_id)
                 SELECT DISTINCT thread_id FROM messages;",
        )
        .map_err(error)?;
    }
    if version < 78 {
        // Inbox previews now decode entity references before collapsing and
        // capping, so padding like `&zwnj;&nbsp;` disappears instead of
        // leaving blank space and a cut-off reference. Queue every thread so
        // the bounded search-reindex batch rebuilds its preview.
        tx.execute_batch(
            "INSERT OR IGNORE INTO pending_search_reindex(thread_id)
                 SELECT id FROM threads;",
        )
        .map_err(error)?;
    }
    if version < 79 {
        // Server drafts: each local draft's link to its copy in the account's
        // server drafts, and server copies waiting to be deleted after the
        // draft was sent, discarded, or moved to another account. Device-local,
        // never synced or exported. See docs/server-drafts-design.md.
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS draft_server_links (
                 draft_id TEXT PRIMARY KEY,
                 account TEXT NOT NULL,
                 server_id TEXT,
                 pushed_revision INTEGER NOT NULL DEFAULT 0,
                 pending_since INTEGER,
                 due_at INTEGER,
                 state TEXT NOT NULL CHECK(state IN ('pending', 'retrying', 'failed', 'saved')),
                 error TEXT,
                 attempts INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE IF NOT EXISTS draft_server_deletions (
                 id INTEGER PRIMARY KEY,
                 account TEXT NOT NULL,
                 server_id TEXT,
                 message_id TEXT NOT NULL,
                 due_at INTEGER NOT NULL,
                 attempts INTEGER NOT NULL DEFAULT 0
             );",
        )
        .map_err(error)?;
    }
    if version < 80 {
        // Server drafts, pull: the version of the server copy this device last
        // wrote or read, to tell edits made elsewhere from its own, and the
        // server's version held back when both sides changed the draft.
        tx.execute_batch(
            "ALTER TABLE draft_server_links ADD COLUMN server_version TEXT;
             ALTER TABLE draft_server_links ADD COLUMN conflict TEXT;",
        )
        .map_err(error)?;
    }
    if version < 81 {
        // Discarded drafts, held for a while so Undo can bring them back with
        // their attachment files. Device-local, never synced or exported.
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS discarded_drafts (
                 id TEXT PRIMARY KEY,
                 revision INTEGER NOT NULL,
                 payload TEXT NOT NULL,
                 discarded_at INTEGER NOT NULL
             );",
        )
        .map_err(error)?;
    }
    if stored == 0 {
        // Only new installations get the 90-day default. Existing databases
        // without a retention setting still mean Forever. Keep the default
        // separate from a chosen value so joining sync cannot publish it as
        // a concurrent settings change.
        tx.execute(
            "INSERT OR IGNORE INTO compose_settings(key, value) VALUES ('retention_days_default', '90')",
            [],
        ).map_err(error)?;
    }
    if version < LATEST_VERSION || stored == 0 {
        tx.pragma_update(None, "user_version", LATEST_VERSION)
            .map_err(error)?;
    }
    tx.commit().map_err(error)?;

    post_open_outbox_fixups(connection)
}

/// Outbox reconciliation that runs on every open, independent of schema
/// version: a send interrupted mid-flight is marked uncertain, an undo window
/// is re-armed briefly, and an overdue scheduled send is surfaced.
fn post_open_outbox_fixups(connection: &Connection) -> Result<(), String> {
    connection.execute("UPDATE outbox_messages SET state='uncertain', error='Application stopped during delivery. Check sent mail before sending again.' WHERE state='sending'", []).map_err(error)?;
    connection
        .execute(
            "UPDATE outbox_messages SET deadline=?1 WHERE state='undo_pending'",
            [Utc::now().timestamp_millis() + 10_000],
        )
        .map_err(error)?;
    connection.execute("UPDATE outbox_messages SET state='overdue' WHERE state='scheduled' AND deadline<?1", [Utc::now().timestamp_millis()]).map_err(error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    /// The exact `sqlite_master` object set a fully-migrated v74 database must
    /// have, captured from a real 0->74 run of `migrate()`. The new
    /// `INITIAL_SCHEMA` baseline must reproduce this set exactly, including the
    /// FTS5 shadow tables that `CREATE VIRTUAL TABLE thread_search` creates.
    const EXPECTED_V74_OBJECTS: &[&str] = &[
        "index:account_labels_by_name",
        "index:contact_group_members_contact",
        "index:contact_interactions_account_email_time",
        "index:contact_interactions_email_time",
        "index:contact_interactions_thread",
        "index:contacts_favorite_updated",
        "index:file_attachments_by_name",
        "index:goals_account_period",
        "index:imap_change_journal_by_generation",
        "index:imap_locations_by_message",
        "index:imap_mailbox_sync_state_by_attempt",
        "index:imap_message_tokens_by_message",
        "index:imap_message_tokens_by_token",
        "index:imap_search_pins_by_account",
        "index:imap_thread_aliases_by_survivor",
        "index:imap_threads_by_thread",
        "index:messages_by_thread",
        "index:messages_file_order",
        "index:mutations_pending",
        "index:outbox_sent_copy_due",
        "index:quarantined_messages_account_time",
        "index:replicated_sync_invitation_redemptions_invitation",
        "index:tasks_account_status",
        "index:tasks_goal",
        "index:tasks_status_due",
        "index:tasks_thread",
        "index:threads_account_mailbox_order",
        "index:threads_account_provider_unique",
        "index:threads_inbox_order",
        "index:threads_mailbox_order",
        "index:threads_trashed",
        "index:triage_events_account_sender",
        "index:triage_events_account_time",
        "index:unsubscribe_requests_message",
        "table:account_labels",
        "table:accounts",
        "table:ai_thread_analyses",
        "table:ai_usage",
        "table:calendar_accounts",
        "table:calendar_selections",
        "table:compose_settings",
        "table:contact_addresses",
        "table:contact_group_members",
        "table:contact_groups",
        "table:contact_interactions",
        "table:contact_suggestion_suppressions",
        "table:contacts",
        "table:drafts",
        "table:file_attachments",
        "table:file_stars",
        "table:goals",
        "table:imap_account_settings",
        "table:imap_bodies",
        "table:imap_change_journal",
        "table:imap_copy_intents",
        "table:imap_hot_threads",
        "table:imap_locations",
        "table:imap_mailbox_sync_state",
        "table:imap_mailboxes",
        "table:imap_message_tokens",
        "table:imap_search_pins",
        "table:imap_sent_copy_detection",
        "table:imap_sync_state",
        "table:imap_thread_aliases",
        "table:imap_threads",
        "table:message_metadata",
        "table:messages",
        "table:mutations",
        "table:outbox_messages",
        "table:pending_entity_materializations",
        "table:pending_search_reindex",
        "table:pinned_contacts",
        "table:quarantined_messages",
        "table:replicated_sync_enrollment_requests",
        "table:replicated_sync_invitation_redemptions",
        "table:replicated_sync_invitations",
        "table:scheduled_send_summaries",
        "table:snippets",
        "table:split_inboxes",
        "table:sync_context",
        "table:sync_control_objects_seen",
        "table:sync_deliveries",
        "table:sync_device_labels",
        "table:sync_devices",
        "table:sync_epoch_history",
        "table:sync_head_publications",
        "table:sync_local_state",
        "table:sync_notices",
        "table:sync_objects",
        "table:sync_pending_keychain_cleanup",
        "table:sync_recovery",
        "table:sync_recovery_threads",
        "table:sync_remote_states",
        "table:sync_retired_objects",
        "table:sync_spaces",
        "table:sync_state",
        "table:sync_transports",
        "table:sync_values",
        "table:synced_preferences",
        "table:tasks",
        "table:thread_search",
        "table:thread_search_config",
        "table:thread_search_content",
        "table:thread_search_data",
        "table:thread_search_docsize",
        "table:thread_search_idx",
        "table:threads",
        "table:triage_events",
        "table:unsubscribe_requests",
        "trigger:files_after_message_insert",
        "trigger:files_after_message_update",
        "trigger:imap_search_pins_follow_location",
    ];

    /// Every explicit object (type:name) in `sqlite_master`, excluding
    /// SQLite's internal `sqlite_*` autoindex rows (which have NULL sql).
    fn schema_objects(connection: &Connection) -> Vec<String> {
        let mut statement = connection
            .prepare(
                "SELECT type || ':' || name FROM sqlite_master \
                 WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' \
                 ORDER BY type, name",
            )
            .unwrap();
        let mut rows: Vec<String> = statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        rows.sort();
        rows
    }

    #[test]
    fn fresh_initial_schema_is_born_at_the_v74_baseline() {
        // INITIAL_SCHEMA builds the fully-migrated v74 shape but intentionally
        // leaves user_version at 0 (stamping on every open would clobber an
        // older database). The baseline version is reached by migrate(), which
        // stamps a fresh (version-0) database to LATEST_VERSION without walking
        // the historical chain — exactly how production opens a new database.
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(crate::schema::INITIAL_SCHEMA)
            .unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0,
            "INITIAL_SCHEMA must not self-stamp a version"
        );
        super::migrate(&mut connection).unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            super::LATEST_VERSION,
            "migrate() must stamp a fresh database to the current baseline"
        );
    }

    #[test]
    fn retention_default_preserves_existing_installations_and_choices() {
        use rusqlite::OptionalExtension;

        for version in [super::BASELINE_VERSION, super::LATEST_VERSION] {
            for choice in [None, Some(30), Some(90), Some(365)] {
                let mut connection = Connection::open_in_memory().unwrap();
                connection.execute_batch(super::INITIAL_SCHEMA).unwrap();
                if version == super::LATEST_VERSION {
                    super::migrate(&mut connection).unwrap();
                    connection
                        .execute(
                            "DELETE FROM compose_settings WHERE key='retention_days_default'",
                            [],
                        )
                        .unwrap();
                } else {
                    connection
                        .pragma_update(None, "user_version", version)
                        .unwrap();
                }
                if let Some(days) = choice {
                    connection
                        .execute(
                            "INSERT INTO compose_settings(key, value) VALUES ('retention_days', ?1)",
                            [days.to_string()],
                        )
                        .unwrap();
                }

                // An upgrade and subsequent opens must preserve both finite
                // choices and the absence that historically means Forever.
                for _ in 0..2 {
                    super::migrate(&mut connection).unwrap();
                    let chosen: Option<String> = connection
                        .query_row(
                            "SELECT value FROM compose_settings WHERE key='retention_days'",
                            [],
                            |row| row.get(0),
                        )
                        .optional()
                        .unwrap();
                    assert_eq!(chosen, choice.map(|days| days.to_string()));
                    let defaults: i64 = connection
                        .query_row(
                            "SELECT COUNT(*) FROM compose_settings WHERE key='retention_days_default'",
                            [],
                            |row| row.get(0),
                        )
                        .unwrap();
                    assert_eq!(
                        defaults, 0,
                        "existing v{version} must not acquire a new default"
                    );
                }
            }
        }
    }

    #[test]
    fn fresh_initial_schema_matches_the_fully_migrated_object_set() {
        // The new baseline must reproduce EXACTLY the sqlite_master object set
        // a 0->74 migration produced (captured in EXPECTED_V74_OBJECTS from a
        // real dump). This is the regression guard for the schema squash: any
        // drift between INITIAL_SCHEMA and the historical chain fails here.
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(crate::schema::INITIAL_SCHEMA)
            .unwrap();
        let actual = schema_objects(&connection);
        let expected: Vec<String> =
            EXPECTED_V74_OBJECTS.iter().map(|s| s.to_string()).collect();
        assert_eq!(
            actual, expected,
            "fresh INITIAL_SCHEMA object set drifted from the fully-migrated v74 schema"
        );
    }

    /// Objects each migration above the v74 baseline adds, in `schema_objects` form.
    const ABOVE_BASELINE_OBJECTS: &[&str] = &[
        "index:sender_rules_by_account",
        "table:sender_rule_actions",
        "table:sender_rules",
        "table:signatures",
        "table:signature_defaults",
        "table:draft_server_links",
        "table:draft_server_deletions",
        "table:discarded_drafts",
    ];

    #[test]
    fn migrate_adds_only_the_versions_above_the_baseline_and_is_idempotent() {
        // A fresh database is born at the baseline and runs every migration
        // above it; opening it again changes nothing.
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(crate::schema::INITIAL_SCHEMA)
            .unwrap();
        let before = schema_objects(&connection);
        super::migrate(&mut connection).unwrap();
        let after = schema_objects(&connection);
        let mut expected = before.clone();
        expected.extend(ABOVE_BASELINE_OBJECTS.iter().map(|object| object.to_string()));
        expected.sort();
        assert_eq!(after, expected, "migrate() must add exactly the post-baseline objects");
        super::migrate(&mut connection).unwrap();
        assert_eq!(schema_objects(&connection), after, "a second migrate() must not alter the schema");
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            super::LATEST_VERSION
        );
    }

    #[test]
    fn a_v74_database_upgrades_to_v76_and_keeps_its_data() {
        // A database a 0.121 build left at v74 gains the sender rule tables
        // without losing what it already holds.
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(crate::schema::INITIAL_SCHEMA)
            .unwrap();
        connection.pragma_update(None, "user_version", 74).unwrap();
        connection
            .execute(
                "INSERT INTO split_inboxes(id, name, match_kind, match_value, sort_order, created_at, account_id)
                 VALUES ('split', 'News', 'domain', 'news.example', 0, '2026-10-01T00:00:00Z', 'you@example.com')",
                [],
            )
            .unwrap();
        super::migrate(&mut connection).unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            super::LATEST_VERSION
        );
        assert_eq!(
            connection
                .query_row("SELECT name FROM split_inboxes WHERE id = 'split'", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "News"
        );
        connection
            .execute(
                "INSERT INTO sender_rules(id, account_id, match_kind, match_value, action, created_at)
                 VALUES ('rule', 'you@example.com', 'address', 'news@launch.example', 'mute', '2026-10-14T00:00:00Z')",
                [],
            )
            .unwrap();
        assert!(connection
            .execute(
                "INSERT INTO sender_rules(id, account_id, match_kind, match_value, action, created_at)
                 VALUES ('bad', 'you@example.com', 'pattern', 'news', 'mute', '2026-10-14T00:00:00Z')",
                [],
            )
            .is_err(), "only address and domain rules are stored");
    }

    #[test]
    fn a_v78_database_gains_server_draft_tables_and_keeps_its_drafts_local_until_edited() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(crate::schema::INITIAL_SCHEMA).unwrap();
        super::migrate(&mut connection).unwrap();
        // Roll back to what a 0.147 database looks like.
        connection
            .execute_batch("DROP TABLE draft_server_links; DROP TABLE draft_server_deletions;")
            .unwrap();
        connection.pragma_update(None, "user_version", 78).unwrap();
        connection
            .execute("INSERT INTO drafts VALUES ('draft-1', 4, '{\"id\":\"draft-1\"}')", [])
            .unwrap();
        super::migrate(&mut connection).unwrap();
        let count = |sql: &str| connection.query_row(sql, [], |row| row.get::<_, i64>(0)).unwrap();
        assert_eq!(count("SELECT count(*) FROM drafts"), 1);
        assert_eq!(count("SELECT count(*) FROM draft_server_links"), 0, "existing drafts wait for an edit");
        assert_eq!(count("SELECT count(*) FROM draft_server_deletions"), 0);
        assert!(connection
            .execute("INSERT INTO draft_server_links(draft_id, account, state) VALUES ('d', 'a', 'pushing')", [])
            .is_err(), "only known push states are stored");
    }

    #[test]
    fn a_v77_database_queues_every_thread_to_rebuild_its_preview() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(crate::schema::INITIAL_SCHEMA)
            .unwrap();
        connection.execute_batch("ALTER TABLE messages ADD COLUMN cc_bcc_json TEXT NOT NULL DEFAULT '[]';").unwrap();
        connection.pragma_update(None, "user_version", 77).unwrap();
        connection
            .execute_batch(
                "INSERT INTO threads(id, provider_thread_id, subject, snippet, participants_json, last_message_at, labels_json, account_id)
                 VALUES ('thread-a', 'a', 'A', 'Shutterstock &amp;zwnj', '[]', '2026-01-01T00:00:00Z', '[]', 'you@example.com'),
                        ('thread-b', 'b', 'B', 'Hello', '[]', '2026-01-02T00:00:00Z', '[]', 'you@example.com');",
            )
            .unwrap();
        super::migrate(&mut connection).unwrap();
        let queued: Vec<String> = connection
            .prepare("SELECT thread_id FROM pending_search_reindex ORDER BY thread_id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(queued, vec!["thread-a", "thread-b"]);
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            super::LATEST_VERSION
        );
    }

    #[test]
    fn a_v76_database_gains_cc_bcc_and_queues_its_mail_for_backfill() {
        // Mail stored before v77 has no Cc/Bcc yet: it gets an empty list and
        // its thread is queued so the reindex batch can fill it in.
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(crate::schema::INITIAL_SCHEMA)
            .unwrap();
        connection.pragma_update(None, "user_version", 76).unwrap();
        connection.pragma_update(None, "foreign_keys", false).unwrap();
        connection
            .execute_batch(
                "INSERT INTO messages(id, thread_id, sender, recipients_json, sent_at, body_html, body_text)
                 VALUES ('m1', 'thread-a', 'a@example.com', '[\"b@example.com\"]', '2026-01-01T00:00:00Z', '', ''),
                        ('m2', 'thread-a', 'b@example.com', '[]', '2026-01-02T00:00:00Z', '', ''),
                        ('m3', 'thread-b', 'c@example.com', '[]', '2026-01-03T00:00:00Z', '', '');",
            )
            .unwrap();
        super::migrate(&mut connection).unwrap();
        let cc_bcc: Vec<String> = connection
            .prepare("SELECT cc_bcc_json FROM messages ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(cc_bcc, vec!["[]", "[]", "[]"]);
        let queued: Vec<String> = connection
            .prepare("SELECT thread_id FROM pending_search_reindex ORDER BY thread_id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(queued, vec!["thread-a", "thread-b"]);
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            super::LATEST_VERSION
        );
    }

    #[test]
    fn migrate_refuses_a_database_older_than_the_baseline() {
        // The historical chain is retired: a database stamped below the
        // baseline can no longer be upgraded in-app and must be refused with
        // guidance toward the converter, rather than silently mis-migrated.
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(crate::schema::INITIAL_SCHEMA)
            .unwrap();
        connection.pragma_update(None, "user_version", 60).unwrap();
        let error = super::migrate(&mut connection).unwrap_err();
        assert!(
            error.contains("v60") && error.contains("threestrands-cli"),
            "a pre-baseline database should be refused with converter guidance, got: {error}"
        );
        // The refusal must not have advanced the version.
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            60
        );
    }

    #[test]
    fn migrate_refuses_a_database_newer_than_the_baseline() {
        // A database from a newer build (version above the baseline) is also
        // refused rather than run against a schema this build does not know.
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(crate::schema::INITIAL_SCHEMA)
            .unwrap();
        connection
            .pragma_update(None, "user_version", super::LATEST_VERSION + 1)
            .unwrap();
        let error = super::migrate(&mut connection).unwrap_err();
        assert!(
            error.contains("newer"),
            "a future-version database should be refused, got: {error}"
        );
    }
}
