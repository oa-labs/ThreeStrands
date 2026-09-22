//! Local persistence and projection scaffolding for the pluggable
//! replicated-sync engine (see `docs`/the sync rewrite plan, Phase 2).
//!
//! This module owns the local operation graph: one row per logical event
//! (`sync_events`), one row per field-level write (`sync_operations`), its
//! parent edges (`sync_operation_parents`), and the current frontier per
//! field (`sync_field_frontier`) — the same model implemented and property
//! tested in `crates/sync-core`, here reduced to its local-only case: every
//! operation recorded through this module originates on this device, so a
//! write's parents are always exactly the field's current frontier and can
//! never already be "consumed" by an unseen child. The general (possibly
//! out-of-order, possibly duplicate) case belongs to a future phase's
//! remote-apply path, which can reuse `threestrands_sync_core::OperationGraph`
//! directly instead of this module's simpler SQL.
//!
//! Entirely inert unless [`enabled`] returns `true` (gated by the
//! `THREESTRANDS_REPLICATED_SYNC` environment variable, unset by default):
//! no shipped behavior changes, and no application version bump is owed for
//! this phase. `Database::enqueue_cloud_entity` and
//! `Database::enqueue_cloud_deletion` call into this module at the end of
//! their existing bodies without changing their signatures, so no mutation
//! call site in `lib.rs` changes.

use std::collections::BTreeSet;
use std::sync::atomic::Ordering;

use chrono::Utc;
use rand::{rngs::OsRng, RngCore};
use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::Value;
use threestrands_sync_core::{EntityType, WinnerStamp, ENTITY_EXISTENCE_FIELD};

use crate::db::Database;

/// The single local sync space Phase 2 supports. Multiple concurrent spaces
/// are not a product concept yet; this is simply a stable primary key.
const SPACE_ID: &str = "default";

/// Whether the replicated-sync engine is active. Disabled by default so this
/// entire phase ships inert; set `THREESTRANDS_REPLICATED_SYNC=1` (or any
/// other non-empty value other than `0`/`false`) to exercise it.
pub fn enabled() -> bool {
    parse_flag(std::env::var("THREESTRANDS_REPLICATED_SYNC").ok().as_deref())
}

fn parse_flag(value: Option<&str>) -> bool {
    match value.map(str::trim) {
        None | Some("") => false,
        Some(value) => !value.eq_ignore_ascii_case("0") && !value.eq_ignore_ascii_case("false"),
    }
}

/// The outcome of checking whether a resolved, validated entity is ready to
/// materialize into application tables. Mirrors the plan's projection
/// contract: "Materialize parent entities before dependent entities... A
/// calendar selection that arrives before its calendar account remains
/// pending and is retried after the account materializes." Reuses the exact
/// same dependency check `cloud_sync::upsert_cloud_calendar_selection`
/// already performs for the legacy system.
// No production caller until a transport actually delivers remote
// operations to project (a later phase); exercised directly by this
// module's tests in the meantime.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectionReadiness {
    Ready,
    Pending { reason: String },
}

impl Database {
    /// Records a local field-level write (creation or update) into the
    /// replicated-sync operation graph, in one transaction: device-sequence
    /// increment, lamport increment, event insert, one operation per field,
    /// and frontier maintenance. A no-op while a remote projection is being
    /// applied (see [`Self::with_remote_projection`]), so applying an
    /// already-authenticated remote write can reuse the same materializer
    /// path this module will grow without creating an echo.
    ///
    /// `fields` and `payload` are exactly what `enqueue_cloud_entity`
    /// already computes for the legacy outbox, reused as-is.
    pub fn record_replicated_write(
        &self,
        entity_type: EntityType,
        entity_id: &str,
        fields: &BTreeSet<String>,
        payload: &Value,
    ) -> Result<(), String> {
        if self.is_projecting_remote_operation() {
            return Ok(());
        }
        let mut connection = self.connection()?;
        let tx = connection.transaction().map_err(display)?;
        let device_id = ensure_space_and_device(&tx)?;
        let creating = !entity_has_any_operation(&tx, entity_type, entity_id)?;
        let (event_id_hex, event_id, lamport) = begin_event(&tx, device_id)?;

        if creating {
            apply_field_operation(
                &tx,
                entity_type,
                entity_id,
                ENTITY_EXISTENCE_FIELD,
                Some(Value::Bool(true)),
                &event_id_hex,
                device_id,
                event_id,
                lamport,
            )?;
        }
        for field in fields {
            if field == "*" {
                continue;
            }
            let value = payload.get(field).cloned();
            apply_field_operation(
                &tx,
                entity_type,
                entity_id,
                field,
                value,
                &event_id_hex,
                device_id,
                event_id,
                lamport,
            )?;
        }

        tx.commit().map_err(display)
    }

    /// Records a local deletion: an ordinary write of `_entity = false`, the
    /// reserved existence field. See the module doc for why deletion never
    /// touches any other field's frontier.
    pub fn record_replicated_deletion(&self, entity_type: EntityType, entity_id: &str) -> Result<(), String> {
        if self.is_projecting_remote_operation() {
            return Ok(());
        }
        let mut connection = self.connection()?;
        let tx = connection.transaction().map_err(display)?;
        let device_id = ensure_space_and_device(&tx)?;
        let (event_id_hex, event_id, lamport) = begin_event(&tx, device_id)?;
        apply_field_operation(
            &tx,
            entity_type,
            entity_id,
            ENTITY_EXISTENCE_FIELD,
            Some(Value::Bool(false)),
            &event_id_hex,
            device_id,
            event_id,
            lamport,
        )?;
        tx.commit().map_err(display)
    }

    /// True while an already-authenticated remote (or conflict-resolution)
    /// operation is being applied. A shared materializer checks this before
    /// calling [`Self::record_replicated_write`] / [`Self::record_replicated_deletion`]
    /// so projecting a remote write never re-enqueues it as a new local
    /// event — the echo-prevention the plan calls for. Nothing sets this yet
    /// outside tests; a future transport-aware phase wraps its projection
    /// application in [`Self::with_remote_projection`].
    pub(crate) fn is_projecting_remote_operation(&self) -> bool {
        self.replicated_sync_projecting.load(Ordering::SeqCst)
    }

    /// Runs `work` with remote-projection suppression engaged. Always
    /// restores the flag afterward, including when `work` returns an error.
    /// No production caller until a transport actually delivers remote
    /// operations to project; exercised directly by this module's tests.
    #[allow(dead_code)]
    pub(crate) fn with_remote_projection<R>(&self, work: impl FnOnce() -> Result<R, String>) -> Result<R, String> {
        self.replicated_sync_projecting.store(true, Ordering::SeqCst);
        let result = work();
        self.replicated_sync_projecting.store(false, Ordering::SeqCst);
        result
    }

    /// Validates a fully resolved entity payload and checks any known
    /// materialization dependency, without writing anything. A future
    /// projection path calls this before invoking the real per-entity
    /// upsert, and retries later on [`ProjectionReadiness::Pending`] rather
    /// than treating a not-yet-materialized dependency as an error. No
    /// production caller until a transport exists; exercised directly by
    /// this module's tests.
    #[allow(dead_code)]
    pub(crate) fn check_projection_readiness(
        &self,
        entity_type: EntityType,
        payload: &Value,
    ) -> Result<ProjectionReadiness, String> {
        entity_type.validate_payload(payload)?;
        if entity_type == EntityType::CalendarSelection {
            let account_id = payload.get("accountId").and_then(Value::as_str).unwrap_or_default();
            let known = self
                .list_calendar_accounts()?
                .iter()
                .any(|account| account.email == account_id);
            if !known {
                return Ok(ProjectionReadiness::Pending {
                    reason: format!("calendar account {account_id} has not materialized yet"),
                });
            }
        }
        Ok(ProjectionReadiness::Ready)
    }
}

fn ensure_space_and_device(tx: &Transaction) -> Result<[u8; 16], String> {
    tx.execute(
        "INSERT OR IGNORE INTO sync_spaces(id, active_epoch, lamport, enabled) VALUES (?1, 0, 0, 1)",
        params![SPACE_ID],
    )
    .map_err(display)?;
    let existing: Option<String> = tx
        .query_row("SELECT device_id FROM sync_devices LIMIT 1", [], |row| row.get(0))
        .optional()
        .map_err(display)?;
    if let Some(hex) = existing {
        return decode_id(&hex);
    }
    let device_id = random_id();
    tx.execute(
        "INSERT INTO sync_devices(device_id, status) VALUES (?1, 'active')",
        params![encode_id(&device_id)],
    )
    .map_err(display)?;
    Ok(device_id)
}

fn entity_has_any_operation(tx: &Transaction, entity_type: EntityType, entity_id: &str) -> Result<bool, String> {
    tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_operations WHERE entity_type=?1 AND entity_id=?2)",
        params![entity_type.as_str(), entity_id],
        |row| row.get(0),
    )
    .map_err(display)
}

/// Bumps this device's sequence and the space's lamport, inserts the event
/// row, and returns identifiers every operation in the event shares.
fn begin_event(tx: &Transaction, device_id: [u8; 16]) -> Result<(String, [u8; 16], u64), String> {
    let device_id_hex = encode_id(&device_id);
    let device_sequence: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(device_sequence),0)+1 FROM sync_events WHERE device_id=?1",
            params![device_id_hex],
            |row| row.get(0),
        )
        .map_err(display)?;
    let lamport: i64 = tx
        .query_row(
            "UPDATE sync_spaces SET lamport = lamport + 1 WHERE id=?1 RETURNING lamport",
            params![SPACE_ID],
            |row| row.get(0),
        )
        .map_err(display)?;
    let event_id = random_id();
    let event_id_hex = encode_id(&event_id);
    tx.execute(
        "INSERT INTO sync_events(event_id, epoch, device_id, device_sequence, lamport, state, created_at)
         VALUES (?1,0,?2,?3,?4,'recorded',?5)",
        params![event_id_hex, device_id_hex, device_sequence, lamport, Utc::now().to_rfc3339()],
    )
    .map_err(display)?;
    Ok((event_id_hex, event_id, lamport as u64))
}

/// Names the current frontier as parents, inserts the new operation and its
/// parent edges, and replaces the field's frontier with just this operation
/// — correct because every caller is local-only (see the module doc).
#[allow(clippy::too_many_arguments)]
fn apply_field_operation(
    tx: &Transaction,
    entity_type: EntityType,
    entity_id: &str,
    field: &str,
    value: Option<Value>,
    event_id_hex: &str,
    device_id: [u8; 16],
    event_id: [u8; 16],
    lamport: u64,
) -> Result<(), String> {
    let parents: Vec<String> = {
        let mut statement = tx
            .prepare(
                "SELECT operation_id FROM sync_field_frontier
                 WHERE entity_type=?1 AND entity_id=?2 AND field=?3",
            )
            .map_err(display)?;
        let rows = statement
            .query_map(params![entity_type.as_str(), entity_id, field], |row| row.get(0))
            .map_err(display)?
            .collect::<Result<_, _>>()
            .map_err(display)?;
        rows
    };

    let operation_id = random_id();
    let operation_id_hex = encode_id(&operation_id);
    let stamp = WinnerStamp {
        lamport,
        device_id,
        event_id,
        operation_id,
    };
    tx.execute(
        "INSERT INTO sync_operations(operation_id, event_id, entity_type, entity_id, field, value, winner_stamp)
         VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![
            operation_id_hex,
            event_id_hex,
            entity_type.as_str(),
            entity_id,
            field,
            value.as_ref().map(Value::to_string),
            encode_winner_stamp(&stamp),
        ],
    )
    .map_err(display)?;

    for parent in &parents {
        tx.execute(
            "INSERT INTO sync_operation_parents(operation_id, parent_operation_id) VALUES (?1,?2)",
            params![operation_id_hex, parent],
        )
        .map_err(display)?;
    }
    tx.execute(
        "DELETE FROM sync_field_frontier WHERE entity_type=?1 AND entity_id=?2 AND field=?3",
        params![entity_type.as_str(), entity_id, field],
    )
    .map_err(display)?;
    tx.execute(
        "INSERT INTO sync_field_frontier(entity_type, entity_id, field, operation_id) VALUES (?1,?2,?3,?4)",
        params![entity_type.as_str(), entity_id, field, operation_id_hex],
    )
    .map_err(display)?;
    Ok(())
}

fn random_id() -> [u8; 16] {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    bytes
}

fn encode_id(bytes: &[u8; 16]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_id(hex: &str) -> Result<[u8; 16], String> {
    if hex.len() != 32 {
        return Err("Invalid replicated-sync identifier".to_string());
    }
    let mut bytes = [0u8; 16];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16)
            .map_err(|_| "Invalid replicated-sync identifier".to_string())?;
    }
    Ok(bytes)
}

fn encode_winner_stamp(stamp: &WinnerStamp) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(8 + 16 + 16 + 16);
    bytes.extend_from_slice(&stamp.lamport.to_be_bytes());
    bytes.extend_from_slice(&stamp.device_id);
    bytes.extend_from_slice(&stamp.event_id);
    bytes.extend_from_slice(&stamp.operation_id);
    bytes
}

fn display(value: impl std::fmt::Display) -> String {
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeSet;

    fn fields(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn parses_the_environment_flag_conservatively() {
        assert!(!parse_flag(None));
        assert!(!parse_flag(Some("")));
        assert!(!parse_flag(Some("0")));
        assert!(!parse_flag(Some("false")));
        assert!(!parse_flag(Some("FALSE")));
        assert!(parse_flag(Some("1")));
        assert!(parse_flag(Some("true")));
        assert!(parse_flag(Some("yes")));
    }

    #[test]
    fn recording_a_creation_writes_an_entity_existence_operation_first() {
        let db = Database::open_memory();
        db.record_replicated_write(
            EntityType::Snippet,
            "one",
            &fields(&["name", "body"]),
            &json!({"name": "n", "body": "b"}),
        )
        .unwrap();

        let connection = db.connection().unwrap();
        let operation_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sync_operations WHERE entity_id='one'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        // _entity=true, name, body.
        assert_eq!(operation_count, 3);
        let existence_value: String = connection
            .query_row(
                "SELECT value FROM sync_operations WHERE entity_id='one' AND field='_entity'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(existence_value, "true");
    }

    #[test]
    fn a_later_update_does_not_repeat_the_existence_operation() {
        let db = Database::open_memory();
        db.record_replicated_write(EntityType::Snippet, "one", &fields(&["name"]), &json!({"name": "n"}))
            .unwrap();
        db.record_replicated_write(
            EntityType::Snippet,
            "one",
            &fields(&["name"]),
            &json!({"name": "n2"}),
        )
        .unwrap();

        let connection = db.connection().unwrap();
        let existence_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sync_operations WHERE entity_id='one' AND field='_entity'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(existence_count, 1);

        // The field's frontier now has exactly the newest write as its sole
        // (conflict-free) member, and the old one is no longer in it.
        let frontier_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sync_field_frontier WHERE entity_id='one' AND field='name'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(frontier_count, 1);
        let winning_value: String = connection
            .query_row(
                "SELECT so.value FROM sync_field_frontier sf
                 JOIN sync_operations so ON so.operation_id = sf.operation_id
                 WHERE sf.entity_id='one' AND sf.field='name'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(winning_value, "\"n2\"");
    }

    #[test]
    fn the_new_write_names_the_old_frontier_as_its_parent() {
        let db = Database::open_memory();
        db.record_replicated_write(EntityType::Snippet, "one", &fields(&["name"]), &json!({"name": "n"}))
            .unwrap();
        let connection = db.connection().unwrap();
        let first_operation_id: String = connection
            .query_row(
                "SELECT operation_id FROM sync_operations WHERE entity_id='one' AND field='name'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        drop(connection);

        db.record_replicated_write(
            EntityType::Snippet,
            "one",
            &fields(&["name"]),
            &json!({"name": "n2"}),
        )
        .unwrap();

        let connection = db.connection().unwrap();
        let parent_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sync_operation_parents WHERE parent_operation_id=?1",
                [&first_operation_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(parent_count, 1);
    }

    #[test]
    fn deletion_writes_entity_false_without_touching_other_fields() {
        let db = Database::open_memory();
        db.record_replicated_write(EntityType::Snippet, "one", &fields(&["name"]), &json!({"name": "n"}))
            .unwrap();
        db.record_replicated_deletion(EntityType::Snippet, "one").unwrap();

        let connection = db.connection().unwrap();
        let existence_value: String = connection
            .query_row(
                "SELECT so.value FROM sync_field_frontier sf
                 JOIN sync_operations so ON so.operation_id = sf.operation_id
                 WHERE sf.entity_id='one' AND sf.field='_entity'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(existence_value, "false");
        // The name field's frontier is untouched by the deletion.
        let name_frontier: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sync_field_frontier WHERE entity_id='one' AND field='name'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(name_frontier, 1);
    }

    #[test]
    fn remote_projection_suppresses_local_recording() {
        let db = Database::open_memory();
        db.with_remote_projection(|| {
            db.record_replicated_write(EntityType::Snippet, "one", &fields(&["name"]), &json!({"name": "n"}))
        })
        .unwrap();

        let connection = db.connection().unwrap();
        let operation_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM sync_operations", [], |row| row.get(0))
            .unwrap();
        assert_eq!(operation_count, 0);
        assert!(!db.is_projecting_remote_operation());
    }

    #[test]
    fn remote_projection_flag_is_restored_even_after_an_error() {
        let db = Database::open_memory();
        let result = db.with_remote_projection(|| Err::<(), _>("boom".to_string()));
        assert!(result.is_err());
        assert!(!db.is_projecting_remote_operation());
    }

    #[test]
    fn projection_readiness_accepts_a_complete_valid_entity() {
        let db = Database::open_memory();
        let readiness = db
            .check_projection_readiness(
                EntityType::Snippet,
                &json!({"name": "n", "body": "b"}),
            )
            .unwrap();
        assert_eq!(readiness, ProjectionReadiness::Ready);
    }

    #[test]
    fn projection_readiness_rejects_an_incomplete_entity() {
        let db = Database::open_memory();
        let result = db.check_projection_readiness(EntityType::Snippet, &json!({"name": "n"}));
        assert!(result.is_err());
    }

    #[test]
    fn projection_readiness_defers_a_calendar_selection_missing_its_account() {
        let db = Database::open_memory();
        let readiness = db
            .check_projection_readiness(
                EntityType::CalendarSelection,
                &json!({"accountId": "missing@example.com", "calendarIds": ["primary"]}),
            )
            .unwrap();
        assert_eq!(
            readiness,
            ProjectionReadiness::Pending {
                reason: "calendar account missing@example.com has not materialized yet".to_string()
            }
        );
    }
}
