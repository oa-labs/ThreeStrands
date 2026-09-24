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
//! Entirely inert unless [`Database::replicated_sync_active`] is true (the
//! Settings beta toggle, or the `THREESTRANDS_REPLICATED_SYNC` environment
//! override). Mutation commands in `lib.rs` reach this module through
//! `Database::record_local_entity_write` and
//! `Database::record_local_entity_deletion` in `sync_projection.rs`.
//!
//! The rest of this module (from "Key material" on) is the Phase 3/4
//! replicator: sealing local events, delivering them to configured
//! transports, pulling and applying remote events, anti-entropy repair, and
//! health aggregation, plus the folder transport (`sync_folder.rs`). Unlike
//! the section above, `apply_remote_operation` implements the *general*
//! operation-graph algorithm (out-of-order and duplicate tolerant), because
//! a remote origin genuinely can deliver a child before its parent.

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use chrono::Utc;
use keyring::Entry;
use rand::{rngs::OsRng, RngCore};
use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use threestrands_sync_core::{EntityType, WinnerStamp, ENTITY_EXISTENCE_FIELD};
use threestrands_sync_envelope::{
    compute_cid, message_key_epoch, open_message, seal_event, sign_device_head, verify_device_head, DeviceHead,
    DeviceId as EnvelopeDeviceId, EventId as EnvelopeEventId, FieldOperation, ObjectKind,
    OpenParams, OperationId as EnvelopeOperationId, SealParams, SignedDeviceHead, SigningKey,
    SyncEvent, UnsignedSyncEvent, VerifyingKey, PROTOCOL_MARKER, PROTOCOL_VERSION,
};
use threestrands_sync_envelope::protocol_marker_cid;
use threestrands_sync_transport::{
    Cid as TransportCid, HeadLocator, SyncTransport, TransportError, TransportHealth,
    TransportInstanceId,
};

use crate::{
    backoff::retry_at,
    db::{Database, DatabaseError, DbResult},
    error_text::display,
    s3_transport::{S3Config, S3Credentials, S3ProbeReport, S3Transport},
    sync_connectors::{
        is_known_kind, Connector, ConnectorCredentials, ConnectorProbe, FolderConfig, IpfsRpcConfig, TransportConfig,
        TransportSecrets,
    },
};

/// The single local sync space Phase 2 supports. Multiple concurrent spaces
/// are not a product concept yet; this is simply a stable primary key.
pub(crate) const SPACE_ID: &str = "default";

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

impl Database {
    /// Whether replicated sync is active for this device: the hard
    /// `THREESTRANDS_REPLICATED_SYNC` env-var override (dev/CI), or the
    /// persisted "beta features" Settings toggle a user turned on
    /// themselves. Reuses `sync_spaces.enabled`, which every earlier phase
    /// reserved for exactly this without ever wiring it up.
    pub fn replicated_sync_active(&self) -> Result<bool, String> {
        Ok(enabled() || self.beta_features_enabled()?)
    }

    /// The persisted state of the Settings "enable beta features" toggle.
    /// `false` (not an error) when no `sync_spaces` row exists yet — nothing
    /// has ever been turned on.
    pub fn beta_features_enabled(&self) -> Result<bool, String> {
        let enabled: Option<bool> = self.with_connection(|connection| {
            Ok(connection
                .query_row("SELECT enabled FROM sync_spaces WHERE id=?1", params![SPACE_ID], |row| row.get(0))
                .optional()?)
        })?;
        Ok(enabled.unwrap_or(false))
    }

    /// Turns the Settings "enable beta features" toggle on or off. Turning
    /// it off stops replication (the periodic loop and every push/pull
    /// call check this) without deleting local keys, roster, or graph
    /// state — matching "disabling the beta and returning to local-only
    /// operation" rather than an irreversible reset.
    pub fn set_beta_features_enabled(&self, on: bool) -> Result<(), String> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO sync_spaces(id, active_epoch, lamport, enabled) VALUES (?1, 0, 0, ?2)
                 ON CONFLICT(id) DO UPDATE SET enabled=excluded.enabled",
                params![SPACE_ID, on],
            )?;
            Ok(())
        })?;
        Ok(())
    }
}

/// The outcome of checking whether a resolved, validated entity is ready to
/// materialize into application tables. Mirrors the plan's projection
/// contract: "Materialize parent entities before dependent entities... A
/// calendar selection that arrives before its calendar account remains
/// pending and is retried after the account materializes." Reuses the exact
/// same dependency check `sync_projection::upsert_synced_calendar_selection`
/// performs. Used by `materialize_one_entity`
/// while projecting a pulled remote event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectionReadiness {
    Ready,
    Pending { reason: String },
}

/// One candidate value still in a field's frontier — a write from some
/// device that has not (yet) been superseded.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontierConflictCandidate {
    pub operation_id: String,
    pub device_id: String,
    pub value: Option<Value>,
}

/// One field whose frontier currently has more than one member: a genuine
/// concurrent write, not arrival-order noise. The conflict review UI works
/// over exactly these — see [`Database::list_frontier_conflicts`] and
/// [`Database::resolve_frontier_conflict`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontierConflict {
    pub entity_type: String,
    pub entity_id: String,
    pub field: String,
    pub candidates: Vec<FrontierConflictCandidate>,
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
    /// `fields` names the changed fields; each one's value is read from the
    /// complete `payload` (see `Database::record_local_entity_write`).
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
        self.with_transaction(|tx| {
            let device_id = ensure_space_and_device(tx)?;
            let creating = !entity_has_any_operation(tx, entity_type, entity_id)?;
            let (event_id_hex, event_id, lamport) = begin_event(tx, device_id)?;

            if creating {
                apply_field_operation(
                    tx,
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
                    tx,
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
            Ok(())
        })
        .map_err(String::from)
    }

    /// True once at least one operation has been recorded for this entity
    /// anywhere in the graph (locally or via a pulled remote event).
    fn entity_recorded_in_graph(&self, entity_type: EntityType, entity_id: &str) -> DbResult<bool> {
        self.with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_operations WHERE entity_type=?1 AND entity_id=?2)",
                params![entity_type.as_str(), entity_id],
                |row| row.get(0),
            )?)
        })
    }

    /// Enqueues `entity_id` as a creation if the graph has no operation for
    /// it yet; a no-op otherwise. `record_replicated_write` already treats
    /// "no prior operation" as creation and writes `_entity=true` plus
    /// every field in `payload`, so this is just that call guarded by the
    /// existence check.
    fn reconcile_one_entity(&self, entity_type: EntityType, entity_id: &str, payload: Value) -> Result<bool, String> {
        if self.entity_recorded_in_graph(entity_type, entity_id)? {
            return Ok(false);
        }
        let fields: BTreeSet<String> = payload
            .as_object()
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default();
        self.record_replicated_write(entity_type, entity_id, &fields, &payload)?;
        Ok(true)
    }

    /// Repairs the one durability gap `record_replicated_write` leaves open:
    /// a mutation command's app-table write (`create_task`, `create_snippet`,
    /// ...) and its enqueue call remain two separate statements — so every
    /// existing mutation call site can keep calling them exactly as before —
    /// which means a crash in the narrow window between them would silently
    /// leave that one entity permanently un-enqueued, since nothing else
    /// ever retries a "local row exists, graph never heard about it" gap.
    ///
    /// This sweep closes it: enumerate every local entity and enqueue
    /// any one the graph has never seen. A no-op for anything already
    /// recorded, so it is safe and cheap to run on every sync cycle, not
    /// just at startup — `ReplicatedSync::sync_once` does exactly that.
    /// Portable preferences are intentionally not covered here: unlike
    /// every other entity type, their only local write *is* the enqueue
    /// call itself (see `update_synced_preferences`), so there is no
    /// separate app-table mutation for a crash to land between.
    ///
    /// Does not check [`enabled`] itself — like `record_replicated_write`,
    /// that is the caller's job (`ReplicatedSync::sync_once` already gates
    /// on it), so this stays directly callable from a test without an
    /// environment variable to fiddle with.
    pub fn reconcile_replicated_sync_backlog(&self) -> Result<usize, String> {
        let mut repaired = 0usize;
        for task in self.list_tasks(None, None)? {
            if self.reconcile_one_entity(EntityType::Task, &task.id, serde_json::to_value(&task).map_err(display)?)? {
                repaired += 1;
            }
        }
        for snippet in self.list_snippets()? {
            if self.reconcile_one_entity(EntityType::Snippet, &snippet.id, serde_json::to_value(&snippet).map_err(display)?)? {
                repaired += 1;
            }
        }
        for split in self.list_split_inboxes()? {
            if self.reconcile_one_entity(EntityType::SplitInbox, &split.id, serde_json::to_value(&split).map_err(display)?)? {
                repaired += 1;
            }
        }
        for account in self.list_accounts()? {
            let payload = json!({
                "email": account.email,
                "displayName": account.display_name,
                "color": account.color,
                "provider": account.provider,
                "sortOrder": account.sort_order,
            });
            if self.reconcile_one_entity(EntityType::MailAccount, &account.email.to_ascii_lowercase(), payload)? {
                repaired += 1;
            }
        }
        for account in self.list_calendar_accounts()? {
            let email = account.email.to_ascii_lowercase();
            if self.reconcile_one_entity(EntityType::CalendarAccount, &email, json!({ "email": account.email }))? {
                repaired += 1;
            }
            if let Some(ids) = self.calendar_selection(&account.email)? {
                let payload = json!({ "accountId": account.email, "calendarIds": ids });
                if self.reconcile_one_entity(EntityType::CalendarSelection, &email, payload)? {
                    repaired += 1;
                }
            }
        }
        // Unset retention is the unlimited default, not a choice this device
        // made. Seeding it would assert "forever" as a concurrent write the
        // moment a fresh device joins, conflicting with whatever the space
        // already agreed on. An explicit choice — including switching back
        // to unlimited — is recorded by the `set_retention_days` command.
        if let Some(days) = self.retention_days()? {
            if self.reconcile_one_entity(EntityType::Retention, "mail", json!({ "days": days }))? {
                repaired += 1;
            }
        }
        Ok(repaired)
    }

    /// Records a local deletion: an ordinary write of `_entity = false`, the
    /// reserved existence field. See the module doc for why deletion never
    /// touches any other field's frontier.
    pub fn record_replicated_deletion(&self, entity_type: EntityType, entity_id: &str) -> Result<(), String> {
        if self.is_projecting_remote_operation() {
            return Ok(());
        }
        self.with_transaction(|tx| {
            let device_id = ensure_space_and_device(tx)?;
            let (event_id_hex, event_id, lamport) = begin_event(tx, device_id)?;
            apply_field_operation(
                tx,
                entity_type,
                entity_id,
                ENTITY_EXISTENCE_FIELD,
                Some(Value::Bool(false)),
                &event_id_hex,
                device_id,
                event_id,
                lamport,
            )
        })
        .map_err(String::from)
    }

    /// Every field currently in conflict: a frontier with more than one
    /// member whose values differ. This is the multi-value register the
    /// plan's conflict UI reviews and resolves — see
    /// [`Self::resolve_frontier_conflict`]. Concurrent writes that agree on
    /// the value (for example, two devices that already held the same data
    /// before joining one space) leave nothing to choose between, so they
    /// are not reported; the frontier still keeps both members and the next
    /// write to the field collapses it as usual.
    pub fn list_frontier_conflicts(&self) -> Result<Vec<FrontierConflict>, String> {
        self.with_connection(|connection| {
            let keys: Vec<(String, String, String)> = {
                let mut statement = connection.prepare(
                    "SELECT entity_type, entity_id, field FROM sync_field_frontier
                     GROUP BY entity_type, entity_id, field HAVING COUNT(*) > 1",
                )?;
                let rows = statement
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                    .collect::<Result<Vec<_>, _>>()?;
                rows
            };

            let mut conflicts = Vec::with_capacity(keys.len());
            for (entity_type, entity_id, field) in keys {
                let candidates: Vec<(String, Option<String>, Vec<u8>)> = {
                    let mut statement = connection.prepare(
                        "SELECT so.operation_id, so.value, so.winner_stamp FROM sync_field_frontier sf
                         JOIN sync_operations so ON so.operation_id = sf.operation_id
                         WHERE sf.entity_type=?1 AND sf.entity_id=?2 AND sf.field=?3",
                    )?;
                    let rows = statement
                        .query_map(params![entity_type, entity_id, field], |row| {
                            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                        })?
                        .collect::<Result<Vec<_>, _>>()?;
                    rows
                };
                let mut resolved_candidates = Vec::with_capacity(candidates.len());
                for (operation_id, value_json, stamp_bytes) in candidates {
                    // encode_winner_stamp lays out lamport(8) || device_id(16) ||
                    // event_id(16) || operation_id(16); device_id is the middle
                    // 16 bytes.
                    let device_id = stamp_bytes.get(8..24).map(hex_encode).unwrap_or_default();
                    let value: Option<Value> =
                        value_json.map(|json| serde_json::from_str(&json)).transpose().map_err(display)?;
                    resolved_candidates.push(FrontierConflictCandidate {
                        operation_id,
                        device_id,
                        value,
                    });
                }
                let first_value = &resolved_candidates[0].value;
                if resolved_candidates.iter().all(|candidate| &candidate.value == first_value) {
                    continue;
                }
                conflicts.push(FrontierConflict {
                    entity_type,
                    entity_id,
                    field,
                    candidates: resolved_candidates,
                });
            }
            Ok(conflicts)
        })
        .map_err(String::from)
    }

    /// Resolves a field conflict: an ordinary local write carrying the
    /// chosen candidate's value. `apply_field_operation` already names
    /// whatever is *currently* in the field's frontier as this write's
    /// parents, which — because every candidate in the conflict is, by
    /// definition, still in that frontier — is exactly "names the entire
    /// current frontier as parents," the plan's conflict-resolution
    /// contract, with no special-cased logic needed beyond picking the
    /// value.
    pub fn resolve_frontier_conflict(
        &self,
        entity_type: EntityType,
        entity_id: &str,
        field: &str,
        chosen_operation_id: &str,
    ) -> Result<(), String> {
        let stored_value: Option<String> = self.with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT value FROM sync_operations WHERE operation_id=?1",
                params![chosen_operation_id],
                |row| row.get(0),
            )?)
        })?;
        let value: Option<Value> = stored_value.map(|json| serde_json::from_str(&json)).transpose().map_err(display)?;

        self.with_transaction(|tx| {
            let device_id = ensure_space_and_device(tx)?;
            let (event_id_hex, event_id, lamport) = begin_event(tx, device_id)?;
            apply_field_operation(tx, entity_type, entity_id, field, value, &event_id_hex, device_id, event_id, lamport)
        })
        .map_err(String::from)
    }

    /// True while an already-authenticated remote (or conflict-resolution)
    /// operation is being applied. A shared materializer checks this before
    /// calling [`Self::record_replicated_write`] / [`Self::record_replicated_deletion`]
    /// so projecting a remote write never re-enqueues it as a new local
    /// event — the echo-prevention the plan calls for.
    pub(crate) fn is_projecting_remote_operation(&self) -> bool {
        self.replicated_sync_projecting.load(Ordering::SeqCst)
    }

    /// Runs `work` with remote-projection suppression engaged. Always
    /// restores the flag afterward, including when `work` returns an error.
    /// Wraps `materialize_touched_entities`'s projection of a pulled event.
    pub(crate) fn with_remote_projection<R>(&self, work: impl FnOnce() -> Result<R, String>) -> Result<R, String> {
        self.replicated_sync_projecting.store(true, Ordering::SeqCst);
        let result = work();
        self.replicated_sync_projecting.store(false, Ordering::SeqCst);
        result
    }

    /// Validates a fully resolved entity payload and checks any known
    /// materialization dependency, without writing anything. Called before
    /// invoking the real per-entity upsert, and retried later on
    /// [`ProjectionReadiness::Pending`] rather than treating a
    /// not-yet-materialized dependency as an error.
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

pub(crate) fn ensure_space_and_device(tx: &Transaction) -> DbResult<[u8; 16]> {
    tx.execute(
        "INSERT OR IGNORE INTO sync_spaces(id, active_epoch, lamport, enabled) VALUES (?1, 0, 0, 1)",
        params![SPACE_ID],
    )?;
    // `is_self` — not "the first row" — is what identifies this device's
    // own entry once enrollment means `sync_devices` also holds peers.
    let existing: Option<String> = tx
        .query_row("SELECT device_id FROM sync_devices WHERE is_self=1 LIMIT 1", [], |row| row.get(0))
        .optional()?;
    if let Some(hex) = existing {
        return Ok(decode_id(&hex)?);
    }
    let device_id = random_id();
    tx.execute(
        "INSERT INTO sync_devices(device_id, status, is_self) VALUES (?1, 'active', 1)",
        params![encode_id(&device_id)],
    )?;
    Ok(device_id)
}

fn entity_has_any_operation(tx: &Transaction, entity_type: EntityType, entity_id: &str) -> DbResult<bool> {
    Ok(tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_operations WHERE entity_type=?1 AND entity_id=?2)",
        params![entity_type.as_str(), entity_id],
        |row| row.get(0),
    )?)
}

/// Bumps this device's sequence and the space's lamport, inserts the event
/// row, and returns identifiers every operation in the event shares.
fn begin_event(tx: &Transaction, device_id: [u8; 16]) -> DbResult<(String, [u8; 16], u64)> {
    let device_id_hex = encode_id(&device_id);
    let device_sequence: i64 = tx.query_row(
        "SELECT COALESCE(MAX(device_sequence),0)+1 FROM sync_events WHERE device_id=?1",
        params![device_id_hex],
        |row| row.get(0),
    )?;
    let lamport: i64 = tx.query_row(
        "UPDATE sync_spaces SET lamport = lamport + 1 WHERE id=?1 RETURNING lamport",
        params![SPACE_ID],
        |row| row.get(0),
    )?;
    let event_id = random_id();
    let event_id_hex = encode_id(&event_id);
    tx.execute(
        "INSERT INTO sync_events(event_id, epoch, device_id, device_sequence, lamport, state, created_at)
         VALUES (?1,0,?2,?3,?4,'recorded',?5)",
        params![event_id_hex, device_id_hex, device_sequence, lamport, Utc::now().to_rfc3339()],
    )?;
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
) -> DbResult<()> {
    let parents: Vec<String> = {
        let mut statement = tx.prepare(
            "SELECT operation_id FROM sync_field_frontier
             WHERE entity_type=?1 AND entity_id=?2 AND field=?3",
        )?;
        let rows = statement
            .query_map(params![entity_type.as_str(), entity_id, field], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
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
    )?;

    for parent in &parents {
        tx.execute(
            "INSERT INTO sync_operation_parents(operation_id, parent_operation_id) VALUES (?1,?2)",
            params![operation_id_hex, parent],
        )?;
    }
    tx.execute(
        "DELETE FROM sync_field_frontier WHERE entity_type=?1 AND entity_id=?2 AND field=?3",
        params![entity_type.as_str(), entity_id, field],
    )?;
    tx.execute(
        "INSERT INTO sync_field_frontier(entity_type, entity_id, field, operation_id) VALUES (?1,?2,?3,?4)",
        params![entity_type.as_str(), entity_id, field, operation_id_hex],
    )?;
    Ok(())
}

pub(crate) fn random_id() -> [u8; 16] {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    bytes
}

pub(crate) fn encode_id(bytes: &[u8; 16]) -> String {
    hex_encode(bytes)
}

pub(crate) fn decode_id(hex: &str) -> Result<[u8; 16], String> {
    let bytes = hex_decode(hex)?;
    bytes
        .try_into()
        .map_err(|_| "Invalid replicated-sync identifier".to_string())
}

pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn hex_decode(hex: &str) -> Result<Vec<u8>, String> {
    if !hex.len().is_multiple_of(2) {
        return Err("Invalid replicated-sync hex value".to_string());
    }
    (0..hex.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&hex[index..index + 2], 16)
                .map_err(|_| "Invalid replicated-sync hex value".to_string())
        })
        .collect()
}

fn encode_winner_stamp(stamp: &WinnerStamp) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(8 + 16 + 16 + 16);
    bytes.extend_from_slice(&stamp.lamport.to_be_bytes());
    bytes.extend_from_slice(&stamp.device_id);
    bytes.extend_from_slice(&stamp.event_id);
    bytes.extend_from_slice(&stamp.operation_id);
    bytes
}

// ============================== Key material ==============================

pub(crate) const KEYCHAIN_SERVICE: &str = "app.threestrands.replicated-sync";
const SIGNING_KEY_ENTRY: &str = "device-signing-key";
const X25519_KEY_ENTRY: &str = "device-x25519-key";

fn epoch_key_entry(key_epoch: u32) -> String {
    format!("epoch-key-{key_epoch}")
}

/// The minimal single-device key material push/pull need to seal and open
/// messages for real: an Ed25519 device signing key, the active epoch's
/// symmetric key, and every earlier epoch's key this device holds, all kept
/// in the OS keychain. New events are sealed only under the active epoch;
/// earlier keys exist to open history sealed before a rotation. The X25519
/// device key stays on [`DeviceIdentity`] — only enrollment/rotation
/// sealed-box handling needs it, not ordinary push/pull.
pub struct LocalKeys {
    pub signing_key: SigningKey,
    pub k_epoch: [u8; 32],
    pub key_epoch: u32,
    pub earlier_epoch_keys: BTreeMap<u32, [u8; 32]>,
    pub device_id: EnvelopeDeviceId,
    pub sync_space_id: Vec<u8>,
}

impl LocalKeys {
    /// The key for `key_epoch`, whether active or earlier, or `None` if this
    /// device never received it.
    pub fn epoch_key(&self, key_epoch: u32) -> Option<&[u8; 32]> {
        if key_epoch == self.key_epoch {
            Some(&self.k_epoch)
        } else {
            self.earlier_epoch_keys.get(&key_epoch)
        }
    }
}

/// This device's identity — always available once replicated sync is turned
/// on, regardless of enrollment state. Enough to sign and publish an
/// enrollment request/grant/rotation object; not enough to seal or open an
/// ordinary event, which additionally needs an active epoch key (see
/// [`Database::local_replicated_keys`]).
pub struct DeviceIdentity {
    pub signing_key: SigningKey,
    pub verifying_key: VerifyingKey,
    pub x25519_secret: [u8; 32],
    pub device_id: EnvelopeDeviceId,
    pub sync_space_id: Vec<u8>,
}

impl Database {
    /// Loads (provisioning on first use) this device's signing and X25519
    /// keypairs and trusts them for itself. Touches the OS keychain — never
    /// call this from a test.
    pub fn local_device_identity(&self) -> Result<DeviceIdentity, String> {
        let device_id = self.with_transaction(ensure_space_and_device)?;
        let signing_key = load_or_create_signing_key()?;
        let x25519_secret = load_or_create_device_x25519_secret()?;
        let verifying_key = signing_key.verifying_key();
        let x25519_public = x25519_public_bytes(&x25519_secret);
        self.trust_device_keys(&device_id, &verifying_key, &x25519_public)?;
        self.ensure_self_device_name(&encode_id(&device_id))?;
        Ok(DeviceIdentity {
            verifying_key,
            signing_key,
            x25519_secret,
            device_id: EnvelopeDeviceId::from_bytes(device_id),
            sync_space_id: SPACE_ID.as_bytes().to_vec(),
        })
    }

    /// Loads this device's full replicated-sync key material, additionally
    /// requiring that enrollment has already supplied an epoch key for the
    /// sync space's current `active_epoch` — see `enrollment.rs`. Returns an
    /// error (not a panic or a silently generated fresh epoch) if this
    /// device has not completed enrollment yet; callers treat that as "skip
    /// push/pull this cycle," not a hard failure.
    pub fn local_replicated_keys(&self) -> Result<LocalKeys, String> {
        let identity = self.local_device_identity()?;
        let active_epoch: u32 = self.with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT active_epoch FROM sync_spaces WHERE id=?1",
                params![SPACE_ID],
                |row| row.get(0),
            )?)
        })?;
        let k_epoch = load_epoch_key(active_epoch)?
            .ok_or_else(|| "This device has not completed replicated-sync enrollment yet".to_string())?;
        let earlier_epochs: Vec<u32> = self.with_connection(|connection| {
            let mut statement =
                connection.prepare("SELECT key_epoch FROM sync_epoch_history WHERE key_epoch < ?1 ORDER BY key_epoch")?;
            let rows = statement
                .query_map(params![active_epoch], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;
        let mut earlier_epoch_keys = BTreeMap::new();
        for key_epoch in earlier_epochs {
            if let Some(key) = load_epoch_key(key_epoch)? {
                earlier_epoch_keys.insert(key_epoch, key);
            }
        }
        Ok(LocalKeys {
            signing_key: identity.signing_key,
            k_epoch,
            key_epoch: active_epoch,
            earlier_epoch_keys,
            device_id: identity.device_id,
            sync_space_id: identity.sync_space_id,
        })
    }

    /// Records a device's public key as trusted for signature verification,
    /// leaving any existing X25519 public key untouched. For our own
    /// device, [`Self::local_device_identity`] calls
    /// [`Self::trust_device_keys`] instead. Tests call this directly to
    /// simulate an already-trusted peer that only needs Ed25519 material.
    #[cfg(test)]
    pub fn trust_device_public_key(&self, device_id: &[u8; 16], verifying_key: &VerifyingKey) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO sync_devices(device_id, public_key, status) VALUES (?1,?2,'active')
                 ON CONFLICT(device_id) DO UPDATE SET public_key=excluded.public_key",
                params![encode_id(device_id), verifying_key.to_bytes().to_vec()],
            )?;
            Ok(())
        })
    }

    /// Records both of a device's public keys as trusted and active. Used
    /// for self-trust and by enrollment/rotation import to adopt a roster
    /// snapshot.
    pub(crate) fn trust_device_keys(
        &self,
        device_id: &[u8; 16],
        verifying_key: &VerifyingKey,
        x25519_public: &[u8; 32],
    ) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO sync_devices(device_id, public_key, x25519_public, status) VALUES (?1,?2,?3,'active')
                 ON CONFLICT(device_id) DO UPDATE SET public_key=excluded.public_key, x25519_public=excluded.x25519_public, status='active'",
                params![encode_id(device_id), verifying_key.to_bytes().to_vec(), x25519_public.to_vec()],
            )?;
            Ok(())
        })
    }

    /// Marks a device revoked: it stops being trusted for future signature
    /// verification (ordinary events, heads, and enrollment/rotation
    /// objects alike), though it cannot un-decrypt ciphertext it already
    /// received under a prior epoch.
    pub(crate) fn revoke_device(&self, device_id: &[u8; 16]) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE sync_devices SET status='revoked' WHERE device_id=?1",
                params![encode_id(device_id)],
            )?;
            Ok(())
        })
    }

    /// Every device this local database currently trusts a public key for
    /// and considers active (not revoked).
    pub(crate) fn known_device_roster(&self) -> DbResult<Vec<(EnvelopeDeviceId, VerifyingKey)>> {
        let rows: Vec<(String, Vec<u8>)> = self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT device_id, public_key FROM sync_devices WHERE public_key IS NOT NULL AND status='active'",
            )?;
            let rows = statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;
        rows.into_iter()
            .map(|(device_id_hex, key_bytes)| {
                let device_id = EnvelopeDeviceId::from_bytes(decode_id(&device_id_hex)?);
                let key_array: [u8; 32] = key_bytes
                    .try_into()
                    .map_err(|_| "Stored device public key is invalid".to_string())?;
                let verifying_key = VerifyingKey::from_bytes(&key_array)
                    .map_err(|_| "Stored device public key is invalid".to_string())?;
                Ok((device_id, verifying_key))
            })
            .collect::<Result<_, String>>()
            .map_err(DatabaseError::from)
    }
}

fn load_or_create_signing_key() -> Result<SigningKey, String> {
    let entry = Entry::new(KEYCHAIN_SERVICE, SIGNING_KEY_ENTRY).map_err(display)?;
    match entry.get_password() {
        Ok(hex) => {
            let bytes: [u8; 32] = hex_decode(&hex)?
                .try_into()
                .map_err(|_| "Stored device signing key is invalid".to_string())?;
            Ok(SigningKey::from_bytes(&bytes))
        }
        Err(keyring::Error::NoEntry) => {
            let mut seed = [0u8; 32];
            OsRng.fill_bytes(&mut seed);
            entry.set_password(&hex_encode(&seed)).map_err(display)?;
            Ok(SigningKey::from_bytes(&seed))
        }
        Err(error) => Err(display(error)),
    }
}

pub(crate) fn x25519_public_bytes(secret: &[u8; 32]) -> [u8; 32] {
    threestrands_sync_envelope::X25519PublicKey::from(&threestrands_sync_envelope::X25519StaticSecret::from(*secret)).to_bytes()
}

fn load_or_create_device_x25519_secret() -> Result<[u8; 32], String> {
    let entry = Entry::new(KEYCHAIN_SERVICE, X25519_KEY_ENTRY).map_err(display)?;
    match entry.get_password() {
        Ok(hex) => hex_decode(&hex)?
            .try_into()
            .map_err(|_| "Stored device X25519 key is invalid".to_string()),
        Err(keyring::Error::NoEntry) => {
            let mut secret = [0u8; 32];
            OsRng.fill_bytes(&mut secret);
            entry.set_password(&hex_encode(&secret)).map_err(display)?;
            Ok(secret)
        }
        Err(error) => Err(display(error)),
    }
}

/// Reads a previously stored epoch key from the keychain, or `None` if this
/// device has never received (or generated, at genesis) that epoch.
pub(crate) fn load_epoch_key(key_epoch: u32) -> Result<Option<[u8; 32]>, String> {
    let entry = Entry::new(KEYCHAIN_SERVICE, &epoch_key_entry(key_epoch)).map_err(display)?;
    match entry.get_password() {
        Ok(hex) => hex_decode(&hex)?
            .try_into()
            .map(Some)
            .map_err(|_| "Stored epoch key is invalid".to_string()),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(display(error)),
    }
}

fn delete_keychain_entry(name: &str) -> Result<(), String> {
    match Entry::new(KEYCHAIN_SERVICE, name).map_err(display)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(display(error)),
    }
}

/// Removes every epoch key up to `highest_epoch` and this device's own
/// signing and X25519 keys, so the next identity load provisions a fresh
/// device. Keeps going past a failure so one stuck entry does not leave
/// the rest behind, then reports the first failure.
pub(crate) fn forget_sync_space_keys(highest_epoch: u32) -> Result<(), String> {
    let mut first_error = None;
    let entries = (0..=highest_epoch)
        .map(epoch_key_entry)
        .chain([SIGNING_KEY_ENTRY.to_string(), X25519_KEY_ENTRY.to_string()]);
    for name in entries {
        if let Err(error) = delete_keychain_entry(&name) {
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// Persists an epoch key this device just generated (genesis) or received
/// and opened (enrollment grant, rotation).
pub(crate) fn store_epoch_key(key_epoch: u32, key: &[u8; 32]) -> Result<(), String> {
    let entry = Entry::new(KEYCHAIN_SERVICE, &epoch_key_entry(key_epoch)).map_err(display)?;
    entry.set_password(&hex_encode(key)).map_err(display)
}

// ================================ Sealing ==================================

/// The discovery hint an event's `previous_device_event`/a device head's
/// `latest_event_cid` actually point to: not a chunk's CID directly (a
/// multi-chunk message has several, and there is no way to derive the rest
/// from just one), but this small, unauthenticated index listing every
/// chunk CID in order. It needs no signature of its own: every chunk is
/// independently AEAD-authenticated, every chunk in one message shares an
/// authenticated hash of the complete reassembled plaintext, and the
/// reassembled event itself carries a device signature — a forged or
/// corrupted index can only ever make `open_message` fail, never make a
/// wrong message succeed.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct ChunkIndex {
    chunk_cids: Vec<String>,
}

impl Database {
    /// Seals every locally recorded event not yet sealed: builds its
    /// encrypted chunks and chunk index, stores them in `sync_objects`, and
    /// creates one pending delivery row per object per transport instance
    /// in `transports`. Pure local bookkeeping — no network I/O.
    pub fn seal_pending_events(&self, keys: &LocalKeys, transports: &[TransportInstanceId]) -> Result<usize, String> {
        let pending: Vec<(String, String, i64)> = self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT event_id, device_id, device_sequence FROM sync_events WHERE state='recorded' ORDER BY device_sequence",
            )?;
            let rows = statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;

        for (event_id_hex, device_id_hex, device_sequence) in &pending {
            self.seal_one_event(event_id_hex, device_id_hex, *device_sequence, keys, transports)?;
        }
        if !pending.is_empty() {
            self.recompute_progress()?;
        }
        Ok(pending.len())
    }

    fn seal_one_event(
        &self,
        event_id_hex: &str,
        device_id_hex: &str,
        device_sequence: i64,
        keys: &LocalKeys,
        transports: &[TransportInstanceId],
    ) -> DbResult<()> {
        self.with_transaction(|tx| {
            let operations = load_operations_for_event(tx, event_id_hex)?;
            let lamport: i64 = tx.query_row(
                "SELECT lamport FROM sync_events WHERE event_id=?1",
                params![event_id_hex],
                |row| row.get(0),
            )?;
            let previous_device_event: Option<String> = if device_sequence > 1 {
                tx.query_row(
                    "SELECT so.cid FROM sync_objects so JOIN sync_events se ON se.event_id = so.event_id
                     WHERE se.device_id=?1 AND se.device_sequence=?2 AND so.object_kind='chunk_index'",
                    params![device_id_hex, device_sequence - 1],
                    |row| row.get(0),
                )
                .optional()?
            } else {
                None
            };

            // Every parent this event's operations name was already in the
            // local graph, so it came from an event inside what this device
            // has applied: that applied vector is a safe causal vector.
            let causal_vector = crate::sync_progress::causal_vector_for_seal(tx, device_id_hex)?;
            let unsigned = UnsignedSyncEvent {
                event_id: EnvelopeEventId::from_bytes(decode_id(event_id_hex)?),
                protocol_version: PROTOCOL_VERSION,
                key_epoch: keys.key_epoch,
                device_id: keys.device_id,
                device_sequence: device_sequence as u64,
                previous_device_event,
                lamport: lamport as u64,
                created_at_ms: crate::sync_policy::now_ms(),
                causal_vector: causal_vector.clone(),
                operations,
            };

            let (_, sealed) = seal_event(
                unsigned,
                &SealParams {
                    sync_space_id: &keys.sync_space_id,
                    k_epoch: &keys.k_epoch,
                    key_epoch: keys.key_epoch,
                    object_kind: ObjectKind::Operations,
                    signing_key: &keys.signing_key,
                },
            )
            .map_err(display)?;

            let chunk_count = sealed.chunks.len() as i64;
            let mut chunk_cids = Vec::with_capacity(sealed.chunks.len());
            for (index, chunk) in sealed.chunks.iter().enumerate() {
                let cid = compute_cid(chunk);
                tx.execute(
                    "INSERT OR IGNORE INTO sync_objects(cid,event_id,object_kind,chunk_index,chunk_count,bytes) VALUES (?1,?2,'operations',?3,?4,?5)",
                    params![cid, event_id_hex, index as i64, chunk_count, chunk],
                )?;
                for transport_id in transports {
                    tx.execute(
                        "INSERT OR IGNORE INTO sync_deliveries(cid,transport_instance_id,state,attempts) VALUES (?1,?2,'pending',0)",
                        params![cid, transport_id.0],
                    )?;
                }
                chunk_cids.push(cid);
            }

            let index_bytes = serde_json::to_vec(&ChunkIndex { chunk_cids }).map_err(display)?;
            let index_cid = compute_cid(&index_bytes);
            tx.execute(
                "INSERT OR IGNORE INTO sync_objects(cid,event_id,object_kind,chunk_index,chunk_count,bytes) VALUES (?1,?2,'chunk_index',0,1,?3)",
                params![index_cid, event_id_hex, index_bytes],
            )?;
            for transport_id in transports {
                tx.execute(
                    "INSERT OR IGNORE INTO sync_deliveries(cid,transport_instance_id,state,attempts) VALUES (?1,?2,'pending',0)",
                    params![index_cid, transport_id.0],
                )?;
            }

            tx.execute(
                "UPDATE sync_events SET state='sealed', epoch=?2, causal_vector=?3 WHERE event_id=?1",
                params![event_id_hex, keys.key_epoch, crate::sync_progress::encode_vector(&causal_vector)],
            )?;
            crate::sync_progress::note_event_applied(tx, device_id_hex, device_sequence as u64, &Utc::now().to_rfc3339())?;
            Ok(())
        })
    }

    /// Like [`Self::contiguous_head`], but counting only events whose every
    /// object `transport_instance_id` has confirmed receiving: the head that
    /// transport can honestly publish.
    fn contiguous_delivered_head(&self, device_id_hex: &str, transport_instance_id: &str) -> DbResult<(u64, Option<String>)> {
        self.with_connection(|connection| {
            let events: Vec<(i64, i64)> = {
                let mut statement = connection.prepare(
                    "SELECT se.device_sequence, SUM(CASE WHEN sd.state='delivered' THEN 0 ELSE 1 END)
                     FROM sync_events se
                     JOIN sync_objects so ON so.event_id = se.event_id
                     LEFT JOIN sync_deliveries sd ON sd.cid = so.cid AND sd.transport_instance_id = ?2
                     WHERE se.device_id=?1 AND se.state='sealed'
                     GROUP BY se.device_sequence ORDER BY se.device_sequence",
                )?;
                let rows = statement
                    .query_map(params![device_id_hex, transport_instance_id], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<Result<Vec<_>, _>>()?;
                rows
            };
            let mut contiguous = 0i64;
            for (sequence, undelivered) in events {
                if sequence != contiguous + 1 || undelivered > 0 {
                    break;
                }
                contiguous = sequence;
            }
            if contiguous == 0 {
                return Ok((0, None));
            }
            let latest_event_cid: Option<String> = connection
                .query_row(
                    "SELECT so.cid FROM sync_objects so JOIN sync_events se ON se.event_id = so.event_id
                     WHERE se.device_id=?1 AND se.device_sequence=?2 AND so.object_kind='chunk_index'",
                    params![device_id_hex, contiguous],
                    |row| row.get(0),
                )
                .optional()?;
            Ok((contiguous as u64, latest_event_cid))
        })
    }

    /// Stores a remotely fetched, already-authenticated message's chunk
    /// index and chunks locally, so a later chain walk recognizes it
    /// without re-fetching, and so anti-entropy repair can deliver it to
    /// another transport without going back to the transport it came from.
    fn remember_remote_message(
        &self,
        index_cid: &str,
        chunk_cids: &[String],
        chunks: &[Vec<u8>],
        event_id_hex: &str,
    ) -> DbResult<()> {
        self.with_transaction(|tx| {
            let index_bytes = serde_json::to_vec(&ChunkIndex {
                chunk_cids: chunk_cids.to_vec(),
            })
            .map_err(display)?;
            tx.execute(
                "INSERT OR IGNORE INTO sync_objects(cid,event_id,object_kind,chunk_index,chunk_count,bytes) VALUES (?1,?2,'chunk_index',0,1,?3)",
                params![index_cid, event_id_hex, index_bytes],
            )?;
            let chunk_count = chunks.len() as i64;
            for (index, (cid, bytes)) in chunk_cids.iter().zip(chunks.iter()).enumerate() {
                tx.execute(
                    "INSERT OR IGNORE INTO sync_objects(cid,event_id,object_kind,chunk_index,chunk_count,bytes) VALUES (?1,?2,'operations',?3,?4,?5)",
                    params![cid, event_id_hex, index as i64, chunk_count, bytes],
                )?;
            }
            Ok(())
        })
    }
}

impl Database {
    /// Records the protocol marker as a local object, so anti-entropy
    /// repair stores it on every connector this device uses, including ones
    /// added later. Idempotent.
    pub(crate) fn ensure_protocol_marker_object(&self) -> Result<(), String> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT OR IGNORE INTO sync_objects(cid,event_id,object_kind,chunk_index,chunk_count,bytes) VALUES (?1,NULL,'protocol_marker',0,1,?2)",
                params![protocol_marker_cid(), PROTOCOL_MARKER],
            )?;
            Ok(())
        })
        .map_err(String::from)
    }

    /// Whether to tell the user that upgrading reset sync: this device left
    /// a group created by an earlier, incompatible build.
    pub fn protocol_reset_notice(&self) -> Result<bool, String> {
        self.with_connection(|connection| {
            Ok(connection.query_row("SELECT EXISTS(SELECT 1 FROM sync_notices WHERE kind='protocol_reset')", [], |row| row.get(0))?)
        })
        .map_err(String::from)
    }

    pub fn dismiss_protocol_reset_notice(&self) -> Result<(), String> {
        self.with_connection(|connection| {
            connection.execute("DELETE FROM sync_notices WHERE kind='protocol_reset'", [])?;
            Ok(())
        })
        .map_err(String::from)
    }

    /// The highest epoch whose keychain entries a schema migration left
    /// behind (it can't reach the keychain itself), if any.
    fn pending_keychain_cleanup(&self) -> Result<Option<u32>, String> {
        self.with_connection(|connection| {
            Ok(connection
                .query_row("SELECT highest_epoch FROM sync_pending_keychain_cleanup WHERE id=1", [], |row| row.get(0))
                .optional()?)
        })
        .map_err(String::from)
    }

    fn clear_pending_keychain_cleanup(&self) -> Result<(), String> {
        self.with_connection(|connection| {
            connection.execute("DELETE FROM sync_pending_keychain_cleanup", [])?;
            Ok(())
        })
        .map_err(String::from)
    }

    /// Drops locally stored remote objects whose event was never applied.
    /// Earlier builds stored each fetched event before a chain walk had
    /// finished, so a walk that failed partway left newer events stored but
    /// unapplied, and every later walk stopped at them. Removing those
    /// objects (and their pending repair deliveries) lets the next walk
    /// fetch and apply them. Objects of applied events, and of this
    /// device's own events, always have a `sync_events` row and are kept.
    pub(crate) fn forget_unapplied_remote_messages(&self) -> Result<usize, String> {
        self.with_transaction(|tx| {
            tx.execute(
                "DELETE FROM sync_deliveries WHERE cid IN (
                     SELECT cid FROM sync_objects so WHERE so.event_id IS NOT NULL
                       AND NOT EXISTS (SELECT 1 FROM sync_events se WHERE se.event_id = so.event_id))",
                [],
            )?;
            Ok(tx.execute(
                "DELETE FROM sync_objects WHERE event_id IS NOT NULL
                   AND NOT EXISTS (SELECT 1 FROM sync_events se WHERE se.event_id = sync_objects.event_id)",
                [],
            )?)
        })
        .map_err(String::from)
    }
}

fn load_operations_for_event(tx: &Transaction, event_id_hex: &str) -> DbResult<Vec<FieldOperation>> {
    let rows: Vec<(String, String, String, String, Option<String>)> = {
        let mut statement = tx.prepare(
            "SELECT operation_id, entity_type, entity_id, field, value FROM sync_operations WHERE event_id=?1",
        )?;
        let rows = statement
            .query_map(params![event_id_hex], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };

    let mut operations = Vec::with_capacity(rows.len());
    for (operation_id_hex, entity_type_str, entity_id, field, value_json) in rows {
        let parent_hexes: Vec<String> = {
            let mut statement =
                tx.prepare("SELECT parent_operation_id FROM sync_operation_parents WHERE operation_id=?1")?;
            let rows = statement
                .query_map(params![operation_id_hex], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };
        let parents = parent_hexes
            .into_iter()
            .map(|hex| decode_id(&hex).map(EnvelopeOperationId::from_bytes))
            .collect::<Result<Vec<_>, _>>()?;

        operations.push(FieldOperation {
            operation_id: EnvelopeOperationId::from_bytes(decode_id(&operation_id_hex)?),
            entity_type: EntityType::from_str(&entity_type_str)?,
            entity_id,
            field,
            value: value_json.map(|json| serde_json::from_str(&json)).transpose().map_err(display)?,
            parents,
        });
    }
    Ok(operations)
}

// =============================== Delivery ===================================

/// A failed delivery is retried this many times (with exponential backoff)
/// before it is treated as settled-failed rather than pending.
const MAX_DELIVERY_ATTEMPTS: i64 = 20;

struct DeliveryItem {
    cid: String,
    bytes: Vec<u8>,
    attempts: i64,
}

impl Database {
    fn pending_delivery_items(&self, transport_instance_id: &str) -> DbResult<Vec<DeliveryItem>> {
        self.with_connection(|connection| {
            let now = Utc::now().to_rfc3339();
            let mut statement = connection.prepare(
                "SELECT sd.cid, so.bytes, sd.attempts
                 FROM sync_deliveries sd JOIN sync_objects so ON so.cid = sd.cid
                 WHERE sd.transport_instance_id=?1 AND sd.state='pending'
                   AND (sd.retry_at IS NULL OR sd.retry_at <= ?2)",
            )?;
            let rows = statement
                .query_map(params![transport_instance_id, now], |row| {
                    Ok(DeliveryItem {
                        cid: row.get(0)?,
                        bytes: row.get(1)?,
                        attempts: row.get(2)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    fn record_delivery_success(&self, cid: &str, transport_instance_id: &str, remote_id: Option<&str>) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE sync_deliveries SET state='delivered', remote_id=?3, last_error=NULL, retry_at=NULL
                 WHERE cid=?1 AND transport_instance_id=?2",
                params![cid, transport_instance_id, remote_id],
            )?;
            Ok(())
        })
    }

    fn record_delivery_failure(
        &self,
        cid: &str,
        transport_instance_id: &str,
        attempts: i64,
        error: &TransportError,
    ) -> DbResult<()> {
        let next_attempts = attempts + 1;
        let (state, retry_timestamp) = if error.is_retryable() && next_attempts < MAX_DELIVERY_ATTEMPTS {
            (
                "pending",
                Some(retry_at(
                    next_attempts.max(0).try_into().unwrap_or(u32::MAX),
                )),
            )
        } else {
            ("failed", None)
        };
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE sync_deliveries SET state=?3, attempts=?4, retry_at=?5, last_error=?6
                 WHERE cid=?1 AND transport_instance_id=?2",
                params![cid, transport_instance_id, state, next_attempts, retry_timestamp, error.to_string()],
            )?;
            Ok(())
        })
    }

    /// Ensures a pending delivery row exists for every locally known object
    /// on every transport in `transport_ids` that doesn't already have one
    /// (pending, delivered, or failed) — the anti-entropy behavior that
    /// turns transport union into replication. Cheap to call repeatedly:
    /// `INSERT OR IGNORE` only ever adds rows for a truly new pairing.
    pub fn enqueue_repair_deliveries(&self, transport_ids: &[TransportInstanceId]) -> Result<usize, String> {
        self.with_connection(|connection| {
            let cids: Vec<String> = {
                let mut statement = connection.prepare("SELECT cid FROM sync_objects")?;
                let rows = statement
                    .query_map([], |row| row.get(0))?
                    .collect::<Result<Vec<_>, _>>()?;
                rows
            };
            let mut created = 0;
            for cid in &cids {
                for transport_id in transport_ids {
                    created += connection.execute(
                        "INSERT OR IGNORE INTO sync_deliveries(cid,transport_instance_id,state,attempts) VALUES (?1,?2,'pending',0)",
                        params![cid, transport_id.0],
                    )?;
                }
            }
            Ok(created)
        })
        .map_err(String::from)
    }

    /// Pending/delivered/failed delivery counts for one transport instance,
    /// for the Settings UI.
    pub fn delivery_counts(&self, transport_instance_id: &str) -> Result<(i64, i64, i64), String> {
        self.with_connection(|connection| {
            let count = |state: &str| -> DbResult<i64> {
                Ok(connection.query_row(
                    "SELECT COUNT(*) FROM sync_deliveries WHERE transport_instance_id=?1 AND state=?2",
                    params![transport_instance_id, state],
                    |row| row.get(0),
                )?)
            };
            Ok((count("pending")?, count("delivered")?, count("failed")?))
        })
        .map_err(String::from)
    }
}

/// The result of one push cycle.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PushOutcome {
    pub sealed_events: usize,
    pub delivered: usize,
    pub failed: usize,
}

/// Seals every pending local event, then attempts delivery to every
/// transport concurrently. Never holds a SQLite transaction across a
/// `put_object` call: sealing commits first, and each delivery outcome is
/// recorded in its own short transaction after the network call returns.
pub async fn push_pending_events(
    database: &Database,
    keys: &LocalKeys,
    transports: &[Arc<dyn SyncTransport>],
) -> Result<PushOutcome, String> {
    let instance_ids: Vec<TransportInstanceId> = transports.iter().map(|transport| transport.instance_id()).collect();
    let sealed_events = database.seal_pending_events(keys, &instance_ids)?;

    let mut delivered = 0usize;
    let mut failed = 0usize;
    let mut tasks = tokio::task::JoinSet::new();
    for transport in transports {
        let transport = Arc::clone(transport);
        let items = database.pending_delivery_items(&transport.instance_id().0)?;
        tasks.spawn(async move {
            let mut outcomes = Vec::with_capacity(items.len());
            for item in items {
                let cid = TransportCid(item.cid.clone());
                let result = transport.put_object(&cid, &item.bytes).await;
                outcomes.push((item, transport.instance_id(), result));
            }
            outcomes
        });
    }
    while let Some(result) = tasks.join_next().await {
        let outcomes = result.map_err(display)?;
        for (item, instance_id, outcome) in outcomes {
            match outcome {
                Ok(locator) => {
                    database.record_delivery_success(&item.cid, &instance_id.0, locator.remote_id.as_deref())?;
                    delivered += 1;
                }
                Err(error) => {
                    database.record_delivery_failure(&item.cid, &instance_id.0, item.attempts, &error)?;
                    failed += 1;
                }
            }
        }
    }

    publish_local_head(database, keys, transports).await;
    Ok(PushOutcome {
        sealed_events,
        delivered,
        failed,
    })
}

/// Publishes this device's current signed head to every transport where
/// it's due, best-effort: one transport failing to accept the head never
/// blocks publishing to the others, and never fails the push cycle.
///
/// Each transport's head names the newest event whose objects — and every
/// earlier event's — that transport has confirmed receiving, so a reader
/// never walks into an event that isn't there yet (a walk that fails
/// applies nothing from this device). A head is published even before this
/// device has written anything, so peers always see its ack and how
/// recently it synced. It's due on a transport when its content
/// (everything except the publication time) changed since it was last
/// published there, or once the heartbeat interval has passed.
async fn publish_local_head(database: &Database, keys: &LocalKeys, transports: &[Arc<dyn SyncTransport>]) {
    let device_id_hex = encode_id(keys.device_id.as_bytes());
    let Ok(ack) = database.progress_vector() else {
        return;
    };
    let now = crate::sync_policy::now_ms();
    for transport in transports {
        let instance_id = transport.instance_id();
        let Ok((contiguous_sequence, latest_event_cid)) = database.contiguous_delivered_head(&device_id_hex, &instance_id.0) else {
            continue;
        };
        let head = DeviceHead {
            sync_space_id: keys.sync_space_id.clone(),
            device_id: keys.device_id,
            epoch: keys.key_epoch,
            contiguous_sequence,
            latest_event_cid,
            published_at_ms: now,
            ack: ack.clone(),
            snapshot_cid: None,
        };
        let content = json!({
            "epoch": head.epoch,
            "contiguousSequence": head.contiguous_sequence,
            "latestEventCid": head.latest_event_cid,
            "ack": crate::sync_progress::encode_vector(&head.ack),
            "snapshotCid": head.snapshot_cid,
        })
        .to_string();
        if !database.head_publication_due(&instance_id.0, &content, now).unwrap_or(true) {
            continue;
        }
        let Ok(signed) = sign_device_head(&keys.signing_key, head) else {
            continue;
        };
        match transport.publish_head(&signed).await {
            Ok(_) => {
                if let Err(error) = database.record_head_publication(&instance_id.0, &content, now) {
                    log::debug!(target: "replicated_sync", "recording the head publication to {} failed: {error}", instance_id.0);
                }
            }
            Err(error) => log::debug!(
                target: "replicated_sync",
                "publishing the device head to {} failed: {error}",
                instance_id.0
            ),
        }
    }
}

impl Database {
    /// Applies one already-authenticated remote field operation to the
    /// local graph, tolerating out-of-order and duplicate delivery —
    /// idempotent by `operation_id`. This is the general case; contrast
    /// with `apply_field_operation` above, which assumes local-only,
    /// always-in-order writes.
    #[allow(clippy::too_many_arguments)]
    fn apply_remote_operation(
        tx: &Transaction,
        entity_type: EntityType,
        entity_id: &str,
        field: &str,
        value: Option<&Value>,
        event_id_hex: &str,
        operation_id_hex: &str,
        parents: &[String],
        stamp: &WinnerStamp,
    ) -> DbResult<bool> {
        let already_known: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_operations WHERE operation_id=?1)",
            params![operation_id_hex],
            |row| row.get(0),
        )?;
        if already_known {
            return Ok(false);
        }

        tx.execute(
            "INSERT INTO sync_operations(operation_id,event_id,entity_type,entity_id,field,value,winner_stamp) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                operation_id_hex,
                event_id_hex,
                entity_type.as_str(),
                entity_id,
                field,
                value.map(Value::to_string),
                encode_winner_stamp(stamp),
            ],
        )?;

        for parent in parents {
            tx.execute(
                "INSERT OR IGNORE INTO sync_operation_parents(operation_id, parent_operation_id) VALUES (?1,?2)",
                params![operation_id_hex, parent],
            )?;
            tx.execute(
                "DELETE FROM sync_field_frontier WHERE entity_type=?1 AND entity_id=?2 AND field=?3 AND operation_id=?4",
                params![entity_type.as_str(), entity_id, field, parent],
            )?;
        }

        let consumed: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_operation_parents WHERE parent_operation_id=?1)",
            params![operation_id_hex],
            |row| row.get(0),
        )?;
        if !consumed {
            tx.execute(
                "INSERT OR IGNORE INTO sync_field_frontier(entity_type, entity_id, field, operation_id) VALUES (?1,?2,?3,?4)",
                params![entity_type.as_str(), entity_id, field, operation_id_hex],
            )?;
        }
        Ok(true)
    }

    /// Applies a complete, already-signature-verified event to the graph in
    /// one transaction, then materializes every entity it touched only
    /// after that transaction commits — matching the plan's projection
    /// ordering exactly.
    ///
    /// Events apply strictly in feed order: the next one must be exactly
    /// one past the author's applied prefix. One at or below it is already
    /// covered and is dropped without touching the graph (so an ancestor
    /// that arrives again can never re-enter a frontier). Returns whether
    /// the event was applied.
    fn apply_sealed_message_and_materialize(&self, event: SyncEvent) -> Result<bool, String> {
        let event_id_hex = encode_id(event.event_id.as_bytes());
        let device_id_hex = encode_id(event.device_id.as_bytes());

        let touched = self.with_transaction(|tx| {
            if event.device_sequence <= crate::sync_progress::applied_sequence_in(tx, &device_id_hex)? {
                return Ok(None);
            }
            let applied_at = Utc::now().to_rfc3339();
            crate::sync_progress::note_event_applied(tx, &device_id_hex, event.device_sequence, &applied_at)?;
            tx.execute(
                "INSERT INTO sync_events(event_id,epoch,device_id,device_sequence,lamport,state,created_at,causal_vector)
                 VALUES (?1,?2,?3,?4,?5,'sealed',?6,?7)",
                params![
                    event_id_hex,
                    event.key_epoch,
                    device_id_hex,
                    event.device_sequence as i64,
                    event.lamport as i64,
                    applied_at,
                    crate::sync_progress::encode_vector(&event.causal_vector),
                ],
            )?;

            let mut touched: Vec<(EntityType, String)> = Vec::new();
            for op in &event.operations {
                let operation_id_hex = hex_encode(op.operation_id.as_bytes());
                let parents: Vec<String> = op.parents.iter().map(|parent| hex_encode(parent.as_bytes())).collect();
                let stamp = WinnerStamp {
                    lamport: event.lamport,
                    device_id: *event.device_id.as_bytes(),
                    event_id: *event.event_id.as_bytes(),
                    operation_id: *op.operation_id.as_bytes(),
                };
                let applied = Self::apply_remote_operation(
                    tx,
                    op.entity_type,
                    &op.entity_id,
                    &op.field,
                    op.value.as_ref(),
                    &event_id_hex,
                    &operation_id_hex,
                    &parents,
                    &stamp,
                )?;
                if applied {
                    touched.push((op.entity_type, op.entity_id.clone()));
                }
            }
            Ok(Some(touched))
        })?;

        let Some(touched) = touched else { return Ok(false) };
        self.materialize_touched_entities(&touched)?;
        Ok(true)
    }

    fn materialize_touched_entities(&self, touched: &[(EntityType, String)]) -> Result<(), String> {
        let mut pending: Vec<(EntityType, String)> = touched.to_vec();
        pending.sort_by(|a, b| a.1.cmp(&b.1));
        pending.dedup();
        if pending.is_empty() {
            return Ok(());
        }
        self.with_remote_projection(|| {
            // Two passes: a dependency that materializes within this same
            // batch (e.g. a calendar account and its selection arriving
            // together) becomes ready on the second pass.
            for _ in 0..2 {
                let mut still_pending = Vec::new();
                for (entity_type, entity_id) in &pending {
                    match self.materialize_one_entity(*entity_type, entity_id)? {
                        ProjectionReadiness::Ready => {}
                        ProjectionReadiness::Pending { .. } => still_pending.push((*entity_type, entity_id.clone())),
                    }
                }
                let done = still_pending.len() == pending.len();
                pending = still_pending;
                if pending.is_empty() || done {
                    break;
                }
            }
            Ok(())
        })
    }

    fn materialize_one_entity(&self, entity_type: EntityType, entity_id: &str) -> Result<ProjectionReadiness, String> {
        let exists = self
            .resolve_field_winner(entity_type, entity_id, ENTITY_EXISTENCE_FIELD)?
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        if !exists {
            self.materialize_entity(entity_type, entity_id, None, true)?;
            return Ok(ProjectionReadiness::Ready);
        }

        let mut payload = serde_json::Map::new();
        for field in self.known_fields(entity_type, entity_id)? {
            if field == ENTITY_EXISTENCE_FIELD {
                continue;
            }
            if let Some(value) = self.resolve_field_winner(entity_type, entity_id, &field)? {
                payload.insert(field, value);
            }
        }
        let payload = Value::Object(payload);
        let readiness = self.check_projection_readiness(entity_type, &payload)?;
        if readiness == ProjectionReadiness::Ready {
            self.materialize_entity(entity_type, entity_id, Some(&payload), false)?;
        }
        Ok(readiness)
    }

    fn known_fields(&self, entity_type: EntityType, entity_id: &str) -> DbResult<Vec<String>> {
        self.with_connection(|connection| {
            let mut statement =
                connection.prepare("SELECT DISTINCT field FROM sync_operations WHERE entity_type=?1 AND entity_id=?2")?;
            let rows = statement
                .query_map(params![entity_type.as_str(), entity_id], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    /// The current winning value for one field: the frontier member with
    /// the greatest [`WinnerStamp`]. Comparing the stored stamp *bytes*
    /// lexicographically gives the same order as comparing
    /// `(lamport, device_id, event_id, operation_id)`, because
    /// `encode_winner_stamp` writes them in that priority order with a
    /// fixed-width big-endian lamport — no need to decode a candidate to
    /// rank it.
    fn resolve_field_winner(&self, entity_type: EntityType, entity_id: &str, field: &str) -> DbResult<Option<Value>> {
        let candidates: Vec<(Option<String>, Vec<u8>)> = self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT so.value, so.winner_stamp FROM sync_field_frontier sf
                 JOIN sync_operations so ON so.operation_id = sf.operation_id
                 WHERE sf.entity_type=?1 AND sf.entity_id=?2 AND sf.field=?3",
            )?;
            let candidates = statement
                .query_map(params![entity_type.as_str(), entity_id, field], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(candidates)
        })?;
        let Some(winner) = candidates.into_iter().max_by(|a, b| a.1.cmp(&b.1)) else {
            return Ok(None);
        };
        Ok(winner.0.map(|json| serde_json::from_str(&json)).transpose().map_err(display)?)
    }
}

/// One remote event fetched and opened during a chain walk, held until the
/// whole walk succeeds so it can be applied and remembered oldest first.
struct FetchedEvent {
    index_cid: String,
    chunk_cids: Vec<String>,
    chunks: Vec<Vec<u8>>,
    event: SyncEvent,
}

/// Resolves and walks one device's signed head back through
/// `previous_device_event` until reaching the part of its feed already
/// applied here, verifying every fetched object's bytes against its
/// requested CID before it is parsed or decrypted, then applies every
/// newly-seen event oldest first.
///
/// The walk stops by sequence number, not by whether an object happens to
/// be stored locally, and checks the feed is gapless and in order: each
/// event must be the author's and exactly one before the last. Each event
/// is opened with the key for the epoch its header names, so history sealed
/// before a rotation stays readable.
///
/// All or nothing per walk: the events walked so far are newer than any
/// that failed, so applying them would leave a gap in the author's applied
/// prefix. A failure applies nothing and the next pull walks again. A
/// fetched event is only remembered locally after it has been applied.
async fn pull_device_chain(
    database: &Database,
    transport: &dyn SyncTransport,
    verifying_key: &VerifyingKey,
    keys: &LocalKeys,
    signed_head: &SignedDeviceHead,
) -> Result<usize, String> {
    let author = signed_head.head.device_id;
    let author_hex = encode_id(author.as_bytes());
    let applied = database.applied_sequence(&author_hex)?;
    let mut expected = signed_head.head.contiguous_sequence;
    let mut cursor = signed_head.head.latest_event_cid.clone();
    let mut chain: Vec<FetchedEvent> = Vec::new();

    while expected > applied {
        let index_cid = cursor.ok_or_else(|| format!("Device {author_hex}'s feed ends before its event {expected}"))?;
        let index_bytes = transport.get_object(&TransportCid(index_cid.clone())).await.map_err(display)?;
        if compute_cid(&index_bytes) != index_cid {
            return Err("Fetched chunk index bytes do not match the requested CID".to_string());
        }
        let index: ChunkIndex = serde_json::from_slice(&index_bytes).map_err(display)?;

        let mut chunks = Vec::with_capacity(index.chunk_cids.len());
        for chunk_cid in &index.chunk_cids {
            let bytes = transport.get_object(&TransportCid(chunk_cid.clone())).await.map_err(display)?;
            if &compute_cid(&bytes) != chunk_cid {
                return Err("Fetched chunk bytes do not match the requested CID".to_string());
            }
            chunks.push(bytes);
        }

        let key_epoch = chunks
            .first()
            .ok_or_else(|| "A chunk index lists no chunks".to_string())
            .and_then(|chunk| message_key_epoch(chunk).map_err(display))?;
        let k_epoch = keys
            .epoch_key(key_epoch)
            .ok_or_else(|| format!("This device doesn't have the key for sync epoch {key_epoch}"))?;
        let event = open_message(
            &chunks,
            &OpenParams {
                sync_space_id: &keys.sync_space_id,
                k_epoch,
                key_epoch,
                verifying_key,
            },
        )
        .map_err(display)?;
        if event.device_id != author || event.device_sequence != expected {
            return Err(format!(
                "Device {author_hex}'s feed is out of order: expected its event {expected}, found {} from {}",
                event.device_sequence,
                encode_id(event.device_id.as_bytes())
            ));
        }

        cursor = event.previous_device_event.clone();
        expected -= 1;
        chain.push(FetchedEvent {
            index_cid,
            chunk_cids: index.chunk_cids,
            chunks,
            event,
        });
    }

    let mut count = 0;
    for fetched in chain.into_iter().rev() {
        let event_id_hex = encode_id(fetched.event.event_id.as_bytes());
        // Apply before remembering: a crash in between only means the next
        // walk fetches this event again, and it's then already covered.
        if database.apply_sealed_message_and_materialize(fetched.event)? {
            count += 1;
        }
        database.remember_remote_message(&fetched.index_cid, &fetched.chunk_cids, &fetched.chunks, &event_id_hex)?;
    }
    Ok(count)
}

/// The result of one pull cycle.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PullOutcome {
    pub applied_events: usize,
    pub failed_transports: usize,
}

/// Resolves every known device's head through every enabled transport
/// independently, walks and applies whatever is new, then enqueues
/// anti-entropy repair so a newly pulled object also reaches every other
/// enabled transport. One failing transport is recorded and skipped —
/// never allowed to block pulling from the others.
pub async fn pull_from_transports(
    database: &Database,
    keys: &LocalKeys,
    transports: &[Arc<dyn SyncTransport>],
) -> Result<PullOutcome, String> {
    database.forget_unapplied_remote_messages()?;
    let roster = database.known_device_roster()?;
    let locators: Vec<HeadLocator> = roster
        .iter()
        .map(|(device_id, _)| HeadLocator {
            device_id: *device_id,
            remote_id: None,
        })
        .collect();

    let mut applied_events = 0usize;
    let mut failed_transports = 0usize;

    for transport in transports {
        let heads = match transport.resolve_heads(&locators).await {
            Ok(heads) => heads,
            Err(_) => {
                failed_transports += 1;
                continue;
            }
        };
        for signed_head in heads {
            // This device's own feed is applied as it is sealed.
            if signed_head.head.device_id == keys.device_id {
                continue;
            }
            let Some((_, verifying_key)) = roster.iter().find(|(device_id, _)| *device_id == signed_head.head.device_id) else {
                continue;
            };
            if verify_device_head(verifying_key, &signed_head).is_err() {
                continue;
            }
            database.record_head_observation(&signed_head.head, crate::sync_policy::now_ms())?;
            match pull_device_chain(database, transport.as_ref(), verifying_key, keys, &signed_head).await {
                Ok(count) => applied_events += count,
                Err(error) => {
                    log::warn!(
                        target: "replicated_sync",
                        "pulling device {} from transport {} failed: {error}",
                        encode_id(signed_head.head.device_id.as_bytes()),
                        transport.instance_id().0
                    );
                    failed_transports += 1;
                }
            }
        }
    }

    database.recompute_progress()?;
    let instance_ids: Vec<TransportInstanceId> = transports.iter().map(|transport| transport.instance_id()).collect();
    database.enqueue_repair_deliveries(&instance_ids)?;

    Ok(PullOutcome {
        applied_events,
        failed_transports,
    })
}

// ================================= Health ===================================

/// Each configured transport's current health, for the replicator's
/// aggregation and the Settings UI. A transport whose `health()` call
/// itself errors is reported as `Unavailable` rather than propagating the
/// error — health reporting must never be the thing that fails.
pub async fn transport_health(transports: &[Arc<dyn SyncTransport>]) -> Vec<(TransportInstanceId, TransportHealth)> {
    let mut results = Vec::with_capacity(transports.len());
    for transport in transports {
        let health = match transport.health().await {
            Ok(health) => health,
            Err(error) => TransportHealth::Unavailable(error.to_string()),
        };
        results.push((transport.instance_id(), health));
    }
    results
}

// ============================ Transport config ==============================

/// One configured transport instance's persisted row — a durable record of
/// "the user selected this folder," independent of whatever live
/// `Arc<dyn SyncTransport>` gets constructed from it at startup or on
/// demand.
pub struct ConfiguredTransport {
    pub instance_id: String,
    pub kind: String,
    pub config_json: String,
    pub enabled: bool,
    pub last_success_at: Option<String>,
    pub last_error: Option<String>,
}

impl Database {
    pub fn configured_transports(&self) -> Result<Vec<ConfiguredTransport>, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT instance_id, kind, config_json, enabled, last_success_at, last_error FROM sync_transports",
            )?;
            let rows = statement
                .query_map([], |row| {
                    Ok(ConfiguredTransport {
                        instance_id: row.get(0)?,
                        kind: row.get(1)?,
                        config_json: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                        enabled: row.get(3)?,
                        last_success_at: row.get(4)?,
                        last_error: row.get(5)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .map_err(String::from)
    }

    /// Persists (or reconfigures, if `instance_id` already exists) one
    /// connector: validates it, writes its non-secret config to
    /// `sync_transports`, then stores `secrets` in the OS keychain — or,
    /// with `None`, clears any secret a previous configuration left.
    pub fn add_transport(
        &self,
        instance_id: &str,
        config: &TransportConfig,
        secrets: Option<&TransportSecrets>,
    ) -> Result<(), String> {
        config.validate(instance_id, secrets)?;
        let config_json = config.to_config_json()?;
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO sync_transports(instance_id,kind,config_json,required,enabled) VALUES (?1,?2,?3,1,1)
                 ON CONFLICT(instance_id) DO UPDATE SET kind=excluded.kind, config_json=excluded.config_json, enabled=1",
                params![instance_id, config.kind(), config_json],
            )?;
            Ok(())
        })?;
        match secrets {
            Some(secrets) => secrets.store(instance_id),
            None => TransportSecrets::delete(config.kind(), instance_id),
        }
    }

    /// Persists (or reconfigures) a folder transport instance. Only the
    /// path is stored — no credential, no content.
    pub fn add_folder_transport(&self, instance_id: &str, path: &std::path::Path) -> Result<(), String> {
        let config = TransportConfig::Folder(FolderConfig {
            path: path.to_string_lossy().into_owned(),
            label: None,
        });
        self.add_transport(instance_id, &config, None)
    }

    /// Persists (or reconfigures) an IPFS RPC transport instance. Only the
    /// base URL is stored in `sync_transports`; the access token (if any)
    /// goes to the OS keychain under this instance's id, never into SQLite.
    pub fn add_ipfs_rpc_transport(&self, instance_id: &str, base_url: &str, token: Option<&str>) -> Result<(), String> {
        let config = TransportConfig::IpfsRpc(IpfsRpcConfig {
            base_url: base_url.to_string(),
            label: None,
        });
        let secrets = token.map(|token| TransportSecrets::IpfsRpcToken(token.to_string()));
        self.add_transport(instance_id, &config, secrets.as_ref())
    }

    /// Persists (or reconfigures) an S3-compatible storage transport
    /// instance, validated before anything is written.
    pub fn add_s3_transport(
        &self,
        instance_id: &str,
        config: &crate::s3_transport::S3Config,
        credentials: &crate::s3_transport::S3Credentials,
    ) -> Result<(), String> {
        self.add_transport(
            instance_id,
            &TransportConfig::S3(config.clone()),
            Some(&TransportSecrets::S3(credentials.clone())),
        )
    }

    /// Changes an existing connector's config (for example its name) and,
    /// when `secrets` is given, replaces its stored secret — keeping its
    /// delivery ledger, enabled state, and last-success time. The kind
    /// can't change; remove and re-add for that.
    pub fn update_transport_config(
        &self,
        instance_id: &str,
        config: &TransportConfig,
        secrets: Option<&TransportSecrets>,
    ) -> Result<(), String> {
        let existing = self
            .configured_transports()?
            .into_iter()
            .find(|row| row.instance_id == instance_id)
            .ok_or_else(|| "That connector no longer exists".to_string())?;
        if existing.kind != config.kind() {
            return Err("A connector's kind can't be changed; remove it and add a new one".to_string());
        }
        let stored;
        let effective_secrets = match secrets {
            Some(secrets) => Some(secrets),
            None => {
                stored = TransportSecrets::load(config.kind(), instance_id)?;
                stored.as_ref()
            }
        };
        config.validate(instance_id, effective_secrets)?;
        let config_json = config.to_config_json()?;
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE sync_transports SET config_json=?2 WHERE instance_id=?1",
                params![instance_id, config_json],
            )?;
            Ok(())
        })?;
        match secrets {
            Some(secrets) => secrets.store(instance_id),
            None => Ok(()),
        }
    }

    /// Forgets a configured transport instance, its delivery ledger rows,
    /// and its keychain secret, if its kind has one. Does not touch the
    /// remote corpus itself — the caller deletes that first (through the
    /// live connector) if the user asked for that.
    pub fn remove_transport(&self, instance_id: &str) -> Result<(), String> {
        let kind = self
            .configured_transports()?
            .into_iter()
            .find(|row| row.instance_id == instance_id)
            .map(|row| row.kind);
        self.with_connection(|connection| {
            connection.execute("DELETE FROM sync_transports WHERE instance_id=?1", params![instance_id])?;
            connection.execute("DELETE FROM sync_deliveries WHERE transport_instance_id=?1", params![instance_id])?;
            connection.execute("DELETE FROM sync_head_publications WHERE transport_instance_id=?1", params![instance_id])?;
            Ok(())
        })?;
        match kind {
            Some(kind) if is_known_kind(&kind) => TransportSecrets::delete(&kind, instance_id),
            // No row, or a kind this version doesn't know: clear every
            // secret this instance id could have.
            _ => TransportSecrets::delete_every_kind(instance_id),
        }
    }

    fn set_transport_success(&self, instance_id: &str) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE sync_transports SET last_success_at=?2, last_error=NULL WHERE instance_id=?1",
                params![instance_id, Utc::now().to_rfc3339()],
            )?;
            Ok(())
        })
    }

    fn set_transport_error(&self, instance_id: &str, error: &str) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE sync_transports SET last_error=?2 WHERE instance_id=?1",
                params![instance_id, error],
            )?;
            Ok(())
        })
    }
}

impl ConfiguredTransport {
    /// This row's typed config, or `None` for an unknown kind or malformed
    /// JSON.
    pub fn config(&self) -> Option<TransportConfig> {
        TransportConfig::from_row(&self.kind, &self.config_json)
    }

    /// Opens this row's live connector, loading its secret from the
    /// keychain. `None` if it's misconfigured, of an unknown kind, or its
    /// storage can't be opened.
    pub async fn open_connector(&self) -> Option<Connector> {
        Connector::open_persisted(&self.instance_id, &self.config()?).await.ok()
    }
}

/// Builds the live transport for one configured row, or `None` if it's
/// disabled, misconfigured, or of an unknown kind.
async fn build_transport_from_row(row: &ConfiguredTransport) -> Option<Arc<dyn SyncTransport>> {
    if !row.enabled {
        return None;
    }
    Some(row.open_connector().await?.into_transport())
}

/// Builds the live transport for every enabled configured row. A row whose
/// transport fails to open (folder missing, permission denied, endpoint
/// URL no longer valid, ...) is skipped rather than failing the whole set
/// — its own health will report `Unavailable` on the next status check.
pub async fn build_configured_transports(database: &Database) -> Vec<Arc<dyn SyncTransport>> {
    let mut transports: Vec<Arc<dyn SyncTransport>> = Vec::new();
    let Ok(rows) = database.configured_transports() else {
        return transports;
    };
    for row in rows {
        if let Some(transport) = build_transport_from_row(&row).await {
            transports.push(transport);
        }
    }
    transports
}

// ============================== Engine ======================================

const SYNC_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// One transport instance's status for the Settings UI: identity, live
/// health, delivery ledger counts, and a best-effort storage estimate.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplicatedSyncTransportStatus {
    pub instance_id: String,
    /// `"folder"`, `"ipfs_rpc"`, or `"s3"`.
    pub kind: String,
    /// The user-chosen name, if any.
    pub label: Option<String>,
    /// A folder's path, an RPC endpoint's base URL, or an S3 endpoint with
    /// its bucket and prefix — never a credential.
    pub location: String,
    /// Whether "delete files and disconnect" can remove this connector's
    /// synchronized data (folder and S3; not IPFS pins).
    pub supports_delete_data: bool,
    /// An S3 connector's non-secret settings, so Settings can re-test
    /// replacement credentials against the same bucket. Never credentials.
    pub s3_config: Option<S3Config>,
    pub health: String,
    /// Whether this instance can currently discover other devices'
    /// signed heads on its own (a folder's `heads/` directory; an RPC
    /// endpoint's dedicated bucket pin index) — `false` means "storage-only": still a
    /// valid write/read replica, but it cannot bootstrap a new device by
    /// itself.
    pub head_discovery: bool,
    pub pending: i64,
    pub delivered: i64,
    pub failed: i64,
    pub last_success_at: Option<String>,
    pub last_error: Option<String>,
    pub storage_bytes: Option<u64>,
}

/// What "Test connection" reports for a candidate S3 connector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct S3ConnectionTest {
    #[serde(flatten)]
    pub checks: S3ProbeReport,
    /// Whether this bucket and prefix already hold a sync group; `None`
    /// when the key couldn't list and read, so nothing could be checked.
    pub space_presence: Option<crate::enrollment::SyncSpacePresence>,
}

/// Owns the replicated-sync background cycle: a cheaply `Clone`-able handle
/// held directly in `AppState`, constructed once, cloned into commands and
/// the background task.
#[derive(Clone)]
pub struct ReplicatedSync {
    database: Arc<Database>,
    gate: Arc<tokio::sync::Mutex<()>>,
}

impl ReplicatedSync {
    pub fn new(database: Arc<Database>) -> Self {
        Self {
            database,
            gate: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// Every configured transport's current status, for the Settings UI.
    pub async fn status(&self) -> Result<Vec<ReplicatedSyncTransportStatus>, String> {
        let rows = self.database.configured_transports()?;
        let mut statuses = Vec::with_capacity(rows.len());
        for row in rows {
            let (pending, delivered, failed) = self.database.delivery_counts(&row.instance_id).unwrap_or((0, 0, 0));
            let config = row.config();

            let mut health = "unavailable: not configured".to_string();
            let mut head_discovery = false;
            let mut storage_bytes = None;
            if row.enabled {
                if let Some(connector) = row.open_connector().await {
                    let transport = connector.transport();
                    health = match transport.health().await {
                        Ok(TransportHealth::Healthy) => "healthy".to_string(),
                        Ok(TransportHealth::Degraded(message)) => format!("degraded: {message}"),
                        Ok(TransportHealth::Unavailable(message)) => format!("unavailable: {message}"),
                        Err(error) => format!("unavailable: {error}"),
                    };
                    head_discovery = transport.capabilities().head_discovery;
                    storage_bytes = connector.corpus_size_bytes().await;
                }
            }

            statuses.push(ReplicatedSyncTransportStatus {
                instance_id: row.instance_id,
                kind: row.kind,
                label: config.as_ref().and_then(|config| config.label()).map(str::to_string),
                location: config.as_ref().map(TransportConfig::location).unwrap_or_default(),
                supports_delete_data: config.as_ref().is_some_and(TransportConfig::supports_delete_data),
                s3_config: match &config {
                    Some(TransportConfig::S3(s3)) => Some(s3.clone()),
                    _ => None,
                },
                health,
                head_discovery,
                pending,
                delivered,
                failed,
                last_success_at: row.last_success_at,
                last_error: row.last_error,
                storage_bytes,
            });
        }
        Ok(statuses)
    }

    /// Validates a candidate IPFS RPC endpoint without persisting anything
    /// — the "test connection" step Settings runs before letting the user
    /// enable a replica, per the plan's "explain a missing required
    /// capability before the user enables the replica."
    pub async fn probe_ipfs_rpc_endpoint(
        &self,
        base_url: &str,
        token: Option<&str>,
    ) -> Result<crate::ipfs_transport::ProbeReport, String> {
        let config = TransportConfig::IpfsRpc(IpfsRpcConfig {
            base_url: base_url.to_string(),
            label: None,
        });
        let secrets = token.map(|token| TransportSecrets::IpfsRpcToken(token.to_string()));
        match Connector::open("probe", &config, secrets).await?.probe().await? {
            ConnectorProbe::IpfsRpc(report) => Ok(report),
            _ => unreachable!("an IPFS RPC connector reports an IPFS RPC probe"),
        }
    }

    /// "Test connection" for a candidate S3 connector, persisting nothing:
    /// the permission checklist, plus — when the key can list and read —
    /// whether the bucket and prefix already hold a sync group. `Err` only
    /// when the config or credentials are invalid before any request.
    pub async fn probe_s3(&self, config: &S3Config, credentials: &S3Credentials) -> Result<S3ConnectionTest, String> {
        let transport = S3Transport::new("probe", config, credentials).map_err(|error| error.to_string())?;
        let checks = transport.probe().await;
        let space_presence = if checks.can_list && checks.can_read {
            let transports: Vec<Arc<dyn SyncTransport>> = vec![Arc::new(transport)];
            Some(crate::enrollment::inspect_sync_space(&transports).await)
        } else {
            None
        };
        Ok(S3ConnectionTest { checks, space_presence })
    }

    /// Renames a connector (`label`: `None` keeps the name, a blank string
    /// clears it) and/or replaces its credentials, keeping its delivery
    /// ledger. Credentials of the wrong kind, or for a folder, are refused.
    pub fn update_connector(
        &self,
        instance_id: &str,
        label: Option<&str>,
        credentials: Option<ConnectorCredentials>,
    ) -> Result<(), String> {
        let row = self
            .database
            .configured_transports()?
            .into_iter()
            .find(|row| row.instance_id == instance_id)
            .ok_or_else(|| "That connector no longer exists".to_string())?;
        let mut config = row
            .config()
            .ok_or_else(|| "This connector's settings can't be read by this version of ThreeStrands".to_string())?;
        if let Some(label) = label {
            config.set_label(Some(label));
        }
        let secrets = credentials.map(ConnectorCredentials::into_secrets).transpose()?;
        self.database.update_transport_config(instance_id, &config, secrets.as_ref())
    }

    /// Runs one push-then-pull cycle against every configured transport,
    /// plus the enrollment/rotation control-object sweep. A no-op if the
    /// feature is disabled or nothing is configured yet. Serialized against
    /// concurrent calls (the periodic loop and a manual "sync now" click)
    /// by `gate`.
    pub async fn sync_once(&self) -> Result<(), String> {
        if !self.database.replicated_sync_active()? {
            return Ok(());
        }
        let _guard = self.gate.lock().await;
        // Finish leaving a group from an earlier, incompatible build: the
        // schema migration left that group but couldn't reach the keychain.
        // Forgetting the device keys too means this device joins its next
        // group as a fresh identity.
        if let Some(highest_epoch) = self.database.pending_keychain_cleanup()? {
            forget_sync_space_keys(highest_epoch)?;
            self.database.clear_pending_keychain_cleanup()?;
        }
        // Best-effort: catch up any entity a crash left un-enqueued before
        // doing anything else, so it is never more than one cycle behind
        // even with no transport configured yet.
        if let Err(error) = self.database.reconcile_replicated_sync_backlog() {
            log::warn!(target: "replicated_sync", "backlog reconciliation failed: {error}");
        }
        let transports = build_configured_transports(&self.database).await;
        if transports.is_empty() {
            return Ok(());
        }
        let identity = self.database.local_device_identity()?;
        if let Err(error) = crate::enrollment::run_enrollment_sweep(
            &self.database,
            &identity,
            &crate::enrollment::KeychainEpochKeyStore,
            &transports,
        )
        .await
        {
            log::warn!(target: "replicated_sync", "enrollment sweep failed: {error}");
        }

        // A device that has not finished enrollment yet (no epoch key)
        // still benefits from the sweep above; it just has nothing to
        // push/pull until a grant or genesis supplies one.
        let mut keys = match self.database.local_replicated_keys() {
            Ok(keys) => keys,
            Err(_) => return Ok(()),
        };
        // Admit or refuse join-code redemptions and expire old codes; a
        // closed invitation rotates the epoch, so reload the keys after.
        match crate::enrollment::process_join_codes(
            &self.database,
            &identity,
            &keys,
            &crate::enrollment::KeychainEpochKeyStore,
            &transports,
            Utc::now().timestamp_millis(),
        )
        .await
        {
            Ok(true) => keys = self.database.local_replicated_keys()?,
            Ok(false) => {}
            Err(error) => log::warn!(target: "replicated_sync", "join code processing failed: {error}"),
        }
        self.database.record_self_device_name_if_missing(&encode_id(identity.device_id.as_bytes()))?;
        self.database.ensure_protocol_marker_object()?;

        let push_result = push_pending_events(&self.database, &keys, &transports).await;
        let pull_result = pull_from_transports(&self.database, &keys, &transports).await;

        for (instance_id, health) in transport_health(&transports).await {
            let recorded = match health {
                TransportHealth::Healthy => self.database.set_transport_success(&instance_id.0),
                TransportHealth::Degraded(message) | TransportHealth::Unavailable(message) => {
                    self.database.set_transport_error(&instance_id.0, &message)
                }
            };
            if let Err(error) = recorded {
                log::warn!(
                    target: "replicated_sync",
                    "recording health for transport {} failed: {error}",
                    instance_id.0
                );
            }
        }

        push_result?;
        pull_result?;
        Ok(())
    }

    /// Starts a brand-new sync space on this device and returns the
    /// recovery phrase, shown to the user exactly once. Refused when a
    /// configured transport already holds a space, unless the user
    /// explicitly chose to start a separate one.
    pub async fn begin_genesis(&self, allow_existing_space: bool) -> Result<String, String> {
        let identity = self.database.local_device_identity()?;
        let transports = build_configured_transports(&self.database).await;
        crate::enrollment::begin_genesis(&self.database, &identity, &crate::enrollment::KeychainEpochKeyStore, &transports, allow_existing_space).await
    }

    /// Whether the configured transports already hold a sync space, so
    /// Settings can steer a new device toward joining it.
    pub async fn inspect_sync_space(&self) -> crate::enrollment::SyncSpacePresence {
        let transports = build_configured_transports(&self.database).await;
        crate::enrollment::inspect_sync_space(&transports).await
    }

    /// Publishes a signed enrollment request for this (new) device and
    /// returns its fingerprint for display.
    pub async fn request_enrollment(&self) -> Result<String, String> {
        let identity = self.database.local_device_identity()?;
        let transports = build_configured_transports(&self.database).await;
        crate::enrollment::publish_enrollment_request(&self.database, &identity, &transports).await
    }

    /// Approves a pending incoming request, publishing a grant.
    pub async fn approve_enrollment_request(&self, request_id_hex: &str) -> Result<(), String> {
        let identity = self.database.local_device_identity()?;
        let keys = self.database.local_replicated_keys()?;
        let transports = build_configured_transports(&self.database).await;
        crate::enrollment::approve_enrollment_request(&self.database, &identity, &keys, request_id_hex, &transports).await
    }

    /// Rejects an incoming request and publishes the signed group-wide
    /// decision to every configured connector.
    pub async fn reject_enrollment_request(&self, request_id_hex: &str) -> Result<(), String> {
        let identity = self.database.local_device_identity()?;
        let transports = build_configured_transports(&self.database).await;
        crate::enrollment::reject_enrollment_request(&self.database, &identity, request_id_hex, &transports).await
    }

    /// Imports a staged grant after the user confirms its fingerprint.
    pub async fn confirm_enrollment(&self, request_id_hex: &str) -> Result<(), String> {
        let identity = self.database.local_device_identity()?;
        crate::enrollment::confirm_and_import_grant(&self.database, &identity, &crate::enrollment::KeychainEpochKeyStore, request_id_hex).await
    }

    /// Rotates the active epoch, optionally revoking a device.
    pub async fn rotate_epoch(&self, revoke_device_id_hex: Option<&str>) -> Result<(), String> {
        let identity = self.database.local_device_identity()?;
        let keys = self.database.local_replicated_keys()?;
        let transports = build_configured_transports(&self.database).await;
        crate::enrollment::rotate_epoch(&self.database, &identity, &keys, &crate::enrollment::KeychainEpochKeyStore, &transports, revoke_device_id_hex).await
    }

    /// Leaves the sync space on this device only; see
    /// [`Database::leave_sync_space`]. Holds the sync gate so a cycle in
    /// flight finishes before the log it is using disappears.
    pub async fn leave_sync_space(&self) -> Result<(), String> {
        let _guard = self.gate.lock().await;
        let highest_epoch = self.database.leave_sync_space()?;
        forget_sync_space_keys(highest_epoch)
            .map_err(|error| format!("Left the sync group, but some keys could not be removed from the keychain: {error}"))
    }

    /// Creates a join code for the chosen connectors. Requires this device
    /// to be enrolled.
    pub async fn create_join_code(
        &self,
        connectors: &[crate::enrollment::JoinCodeConnectorChoice],
        expires_in_hours: u32,
    ) -> Result<String, String> {
        let identity = self.database.local_device_identity()?;
        let keys = self
            .database
            .local_replicated_keys()
            .map_err(|_| "This device must belong to a sync group before it can invite another device".to_string())?;
        let transports = build_configured_transports(&self.database).await;
        crate::enrollment::create_join_code(
            &self.database,
            &identity,
            &keys,
            &transports,
            connectors,
            expires_in_hours,
            Utc::now().timestamp_millis(),
        )
        .await
    }

    /// Cancels an open join code and rotates keys. Holds the sync gate so
    /// the rotation never races a cycle's own.
    pub async fn cancel_join_code(&self, invitation_cid: &str) -> Result<(), String> {
        let _guard = self.gate.lock().await;
        let identity = self.database.local_device_identity()?;
        let keys = self.database.local_replicated_keys()?;
        let transports = build_configured_transports(&self.database).await;
        crate::enrollment::cancel_join_code(
            &self.database,
            &identity,
            &keys,
            &crate::enrollment::KeychainEpochKeyStore,
            &transports,
            invitation_cid,
        )
        .await
    }

    /// Joins a sync group with a pasted join code, setting up its
    /// connectors on this device.
    pub async fn join_with_code(
        &self,
        code: &str,
        folders: &[crate::enrollment::JoinFolderChoice],
        credentials: Vec<crate::enrollment::JoinCredentialsChoice>,
    ) -> Result<(), String> {
        let _guard = self.gate.lock().await;
        let identity = self.database.local_device_identity()?;
        crate::enrollment::join_with_code(
            &self.database,
            &identity,
            &crate::enrollment::KeychainEpochKeyStore,
            code,
            folders,
            credentials,
            Utc::now().timestamp_millis(),
        )
        .await
    }

    /// Joins an existing sync space using only a recovery phrase.
    pub async fn join_with_recovery_phrase(&self, phrase: &str) -> Result<(), String> {
        let identity = self.database.local_device_identity()?;
        let transports = build_configured_transports(&self.database).await;
        crate::enrollment::join_with_recovery_phrase(&self.database, &identity, &crate::enrollment::KeychainEpochKeyStore, phrase, &transports).await
    }

    /// Spawns the periodic push/pull loop. Only ever does real work when
    /// [`enabled`] is true and at least one transport is configured;
    /// otherwise `sync_once` returns immediately, so this is cheap to
    /// spawn unconditionally at startup. `on_synced` runs after every
    /// successful cycle, so the app can react to entities a pull removed.
    pub fn spawn<F>(self, handle: tauri::AppHandle, on_synced: F)
    where
        F: Fn(&tauri::AppHandle) + Send + Sync + 'static,
    {
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(SYNC_INTERVAL).await;
                match self.sync_once().await {
                    Ok(()) => on_synced(&handle),
                    Err(error) => log::warn!(target: "replicated_sync", "periodic sync failed: {error}"),
                }
                use tauri::Emitter;
                let _ = handle.emit("replicated-sync-status", ());
            }
        });
    }
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
    fn field_winner_and_known_fields_read_the_recorded_graph() {
        let db = Database::open_memory();
        assert_eq!(db.resolve_field_winner(EntityType::Snippet, "one", "name").unwrap(), None);
        db.record_replicated_write(EntityType::Snippet, "one", &fields(&["name"]), &json!({"name": "first"}))
            .unwrap();
        db.record_replicated_write(EntityType::Snippet, "one", &fields(&["name"]), &json!({"name": "second"}))
            .unwrap();

        assert_eq!(
            db.resolve_field_winner(EntityType::Snippet, "one", "name").unwrap(),
            Some(json!("second"))
        );
        let mut known = db.known_fields(EntityType::Snippet, "one").unwrap();
        known.sort();
        assert_eq!(known, vec![ENTITY_EXISTENCE_FIELD.to_string(), "name".to_string()]);
        assert!(db.entity_recorded_in_graph(EntityType::Snippet, "one").unwrap());
        assert!(!db.entity_recorded_in_graph(EntityType::Snippet, "two").unwrap());
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

#[cfg(test)]
mod replicator_tests {
    use super::*;
    use threestrands_sync_transport::fake::FakeTransport;

    fn fields(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn snippet_payload(id: &str, name: &str) -> Value {
        serde_json::json!({"id": id, "name": name, "body": "body", "createdAt": "2026-01-01T00:00:00Z"})
    }

    /// Synthetic key material matching whatever device id
    /// `record_replicated_write` already provisioned for `database`. Never
    /// touches the OS keychain — that's what makes this different from
    /// `Database::local_replicated_keys`.
    fn test_keys(database: &Database) -> LocalKeys {
        let device_id = {
            let mut connection = database.connection().unwrap();
            let tx = connection.transaction().unwrap();
            let device_id = ensure_space_and_device(&tx).unwrap();
            tx.commit().unwrap();
            device_id
        };
        let signing_key = SigningKey::generate(&mut OsRng);
        database.trust_device_public_key(&device_id, &signing_key.verifying_key()).unwrap();
        LocalKeys {
            signing_key,
            k_epoch: [7u8; 32],
            key_epoch: 0,
            earlier_epoch_keys: BTreeMap::new(),
            device_id: EnvelopeDeviceId::from_bytes(device_id),
            sync_space_id: b"test-space".to_vec(),
        }
    }

    #[tokio::test]
    async fn sealing_creates_chunk_and_index_objects_and_pending_deliveries() {
        let database = Database::open_memory();
        database
            .record_replicated_write(EntityType::Snippet, "one", &fields(&["id", "name", "body", "createdAt"]), &snippet_payload("one", "n"))
            .unwrap();
        let keys = test_keys(&database);
        let sealed = database.seal_pending_events(&keys, &[TransportInstanceId("t".to_string())]).unwrap();
        assert_eq!(sealed, 1);

        let connection = database.connection().unwrap();
        let object_count: i64 = connection.query_row("SELECT COUNT(*) FROM sync_objects", [], |row| row.get(0)).unwrap();
        assert!(object_count >= 2, "expected at least one chunk plus its index");
        let index_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM sync_objects WHERE object_kind='chunk_index'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(index_count, 1);
        let delivery_count: i64 = connection.query_row("SELECT COUNT(*) FROM sync_deliveries WHERE state='pending'", [], |row| row.get(0)).unwrap();
        assert_eq!(delivery_count, object_count);
        let event_state: String = connection.query_row("SELECT state FROM sync_events", [], |row| row.get(0)).unwrap();
        assert_eq!(event_state, "sealed");
    }

    #[tokio::test]
    async fn a_transports_head_names_only_events_it_has_received() {
        let database = Database::open_memory();
        let keys = test_keys(&database);
        let device_id_hex = encode_id(keys.device_id.as_bytes());
        let transports = fake("shared");
        let transport_id = transports[0].instance_id().0;
        assert_eq!(database.contiguous_delivered_head(&device_id_hex, &transport_id).unwrap(), (0, None));

        for id in ["one", "two"] {
            database
                .record_replicated_write(EntityType::Snippet, id, &fields(&["id", "name", "body", "createdAt"]), &snippet_payload(id, "n"))
                .unwrap();
        }
        // Sealed, but not yet delivered: nothing to name.
        assert_eq!(database.seal_pending_events(&keys, &[transports[0].instance_id()]).unwrap(), 2);
        assert_eq!(database.contiguous_delivered_head(&device_id_hex, &transport_id).unwrap(), (0, None));
        // Sealing is what applies this device's own feed.
        assert_eq!(database.applied_sequence(&device_id_hex).unwrap(), 2);

        push_pending_events(&database, &keys, &transports).await.unwrap();
        let (sequence, latest) = database.contiguous_delivered_head(&device_id_hex, &transport_id).unwrap();
        assert_eq!(sequence, 2);
        let latest = latest.expect("a delivered head names its chunk index");
        let kind: String = database
            .connection()
            .unwrap()
            .query_row("SELECT object_kind FROM sync_objects WHERE cid=?1", params![latest], |row| row.get(0))
            .unwrap();
        assert_eq!(kind, "chunk_index");
        assert_eq!(published_head(&transports, &keys).await.unwrap().head.latest_event_cid.as_deref(), Some(latest.as_str()));
    }

    #[tokio::test]
    async fn a_head_waits_for_a_failed_delivery_before_naming_its_event() {
        let database = Database::open_memory();
        let keys = test_keys(&database);
        let fake_transport = Arc::new(FakeTransport::new("flaky"));
        let transports: Vec<Arc<dyn SyncTransport>> = vec![fake_transport.clone()];
        write_snippet(&database, "one");
        push_pending_events(&database, &keys, &transports).await.unwrap();
        write_snippet(&database, "two");
        // The second event's first object fails; the head keeps naming the
        // first event rather than one a reader couldn't fetch.
        fake_transport.inject_transient_outage(1);
        push_pending_events(&database, &keys, &transports).await.unwrap();
        let head = published_head(&transports, &keys).await.unwrap().head;
        assert_eq!(head.contiguous_sequence, 1);
        assert_eq!(head.latest_event_cid, Some(own_chunk_index_cid(&database, 1)));
    }

    #[test]
    fn roster_drops_a_device_once_it_is_revoked() {
        let database = Database::open_memory();
        let keys = test_keys(&database);
        let peer = random_id();
        let peer_key = SigningKey::generate(&mut OsRng).verifying_key();
        database.trust_device_keys(&peer, &peer_key, &[9u8; 32]).unwrap();

        let roster = database.known_device_roster().unwrap();
        assert_eq!(roster.len(), 2);
        assert!(roster.iter().any(|(device_id, key)| *device_id.as_bytes() == peer && *key == peer_key));

        database.revoke_device(&peer).unwrap();
        let roster = database.known_device_roster().unwrap();
        assert_eq!(roster.len(), 1);
        assert!(roster[0].0 == keys.device_id);
    }

    #[tokio::test]
    async fn push_delivers_to_a_fake_transport_and_records_success() {
        let database = Database::open_memory();
        database
            .record_replicated_write(EntityType::Snippet, "one", &fields(&["id", "name", "body", "createdAt"]), &snippet_payload("one", "n"))
            .unwrap();
        let keys = test_keys(&database);
        let transport: Arc<dyn SyncTransport> = Arc::new(FakeTransport::new("folder-a"));
        let transports = vec![transport];

        let outcome = push_pending_events(&database, &keys, &transports).await.unwrap();
        assert_eq!(outcome.sealed_events, 1);
        assert_eq!(outcome.failed, 0);
        assert!(outcome.delivered > 0);

        let (pending, delivered, failed) = database.delivery_counts("folder-a").unwrap();
        assert_eq!(pending, 0);
        assert_eq!(failed, 0);
        assert!(delivered > 0);
    }

    #[tokio::test]
    async fn push_keeps_a_transient_failure_pending_for_retry() {
        let database = Database::open_memory();
        database
            .record_replicated_write(EntityType::Snippet, "one", &fields(&["id", "name", "body", "createdAt"]), &snippet_payload("one", "n"))
            .unwrap();
        let keys = test_keys(&database);
        let fake = FakeTransport::new("folder-a");
        fake.inject_transient_outage(1000);
        let transport: Arc<dyn SyncTransport> = Arc::new(fake);
        let transports = vec![transport];

        let outcome = push_pending_events(&database, &keys, &transports).await.unwrap();
        assert_eq!(outcome.delivered, 0);
        assert!(outcome.failed > 0);

        let (pending, delivered, failed) = database.delivery_counts("folder-a").unwrap();
        assert!(pending > 0);
        assert_eq!(delivered, 0);
        // Transient failures stay retryable rather than settling as
        // permanently failed.
        assert_eq!(failed, 0);
    }

    #[tokio::test]
    async fn two_devices_converge_through_a_shared_transport() {
        let database_a = Database::open_memory();
        let database_b = Database::open_memory();

        database_b
            .record_replicated_write(EntityType::Snippet, "b-1", &fields(&["id", "name", "body", "createdAt"]), &snippet_payload("b-1", "From B"))
            .unwrap();
        let keys_b = test_keys(&database_b);

        let shared: Arc<dyn SyncTransport> = Arc::new(FakeTransport::new("shared"));
        let transports = vec![shared];
        push_pending_events(&database_b, &keys_b, &transports).await.unwrap();

        // Device A trusts device B's key (simulating completed enrollment,
        // a later phase's job) and pulls.
        let keys_a = test_keys(&database_a);
        database_a.trust_device_public_key(keys_b.device_id.as_bytes(), &keys_b.signing_key.verifying_key()).unwrap();

        let outcome = pull_from_transports(&database_a, &keys_a, &transports).await.unwrap();
        assert_eq!(outcome.failed_transports, 0);
        assert_eq!(outcome.applied_events, 1);

        let snippet_name: String = database_a
            .connection()
            .unwrap()
            .query_row("SELECT name FROM snippets WHERE id='b-1'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(snippet_name, "From B");

        // Pulling again recognizes the already-known chunk index and
        // applies nothing new.
        let second = pull_from_transports(&database_a, &keys_a, &transports).await.unwrap();
        assert_eq!(second.applied_events, 0);
    }

    /// The same device's keys, as they would be after its group rotated to
    /// `key_epoch`, holding `earlier` for older epochs.
    fn at_epoch(keys: &LocalKeys, key_epoch: u32, k_epoch: [u8; 32], earlier: &[(u32, [u8; 32])]) -> LocalKeys {
        LocalKeys {
            signing_key: keys.signing_key.clone(),
            k_epoch,
            key_epoch,
            earlier_epoch_keys: earlier.iter().copied().collect(),
            device_id: keys.device_id,
            sync_space_id: keys.sync_space_id.clone(),
        }
    }

    fn stored_snippet_name(database: &Database, id: &str) -> Option<String> {
        database
            .connection()
            .unwrap()
            .query_row("SELECT name FROM snippets WHERE id=?1", params![id], |row| row.get(0))
            .optional()
            .unwrap()
    }

    fn write_snippet(database: &Database, id: &str) {
        database
            .record_replicated_write(EntityType::Snippet, id, &fields(&["id", "name", "body", "createdAt"]), &snippet_payload(id, id))
            .unwrap();
    }

    fn stored_object_count(database: &Database) -> i64 {
        database.connection().unwrap().query_row("SELECT COUNT(*) FROM sync_objects", [], |row| row.get(0)).unwrap()
    }

    /// The chunk index CID of `database`'s own event at `device_sequence`.
    fn own_chunk_index_cid(database: &Database, device_sequence: i64) -> String {
        database
            .connection()
            .unwrap()
            .query_row(
                "SELECT so.cid FROM sync_objects so JOIN sync_events se ON se.event_id = so.event_id
                 WHERE so.object_kind='chunk_index' AND se.device_sequence=?1",
                params![device_sequence],
                |row| row.get(0),
            )
            .unwrap()
    }

    #[tokio::test]
    async fn each_remote_event_opens_with_the_key_for_its_own_epoch() {
        let database_a = Database::open_memory();
        let database_b = Database::open_memory();
        let transports: Vec<Arc<dyn SyncTransport>> = vec![Arc::new(FakeTransport::new("shared"))];

        // A writes once before its group rotates and once after.
        write_snippet(&database_a, "epoch-0");
        let keys_a = test_keys(&database_a);
        push_pending_events(&database_a, &keys_a, &transports).await.unwrap();
        write_snippet(&database_a, "epoch-1");
        let keys_a_rotated = at_epoch(&keys_a, 1, [9u8; 32], &[(0, keys_a.k_epoch)]);
        push_pending_events(&database_a, &keys_a_rotated, &transports).await.unwrap();

        let keys_b = test_keys(&database_b);
        database_b.trust_device_public_key(keys_a.device_id.as_bytes(), &keys_a.signing_key.verifying_key()).unwrap();

        // Without epoch 0's key, B applies nothing and keeps nothing, so a
        // later pull still walks the whole chain.
        let missing = at_epoch(&keys_b, 1, [9u8; 32], &[]);
        let outcome = pull_from_transports(&database_b, &missing, &transports).await.unwrap();
        assert_eq!(outcome.applied_events, 0);
        assert_eq!(outcome.failed_transports, 1);
        assert_eq!(stored_object_count(&database_b), 0);
        assert_eq!(stored_snippet_name(&database_b, "epoch-1"), None);

        let complete = at_epoch(&keys_b, 1, [9u8; 32], &[(0, [7u8; 32])]);
        let outcome = pull_from_transports(&database_b, &complete, &transports).await.unwrap();
        assert_eq!(outcome.failed_transports, 0);
        assert_eq!(outcome.applied_events, 2);
        assert_eq!(stored_snippet_name(&database_b, "epoch-0").as_deref(), Some("epoch-0"));
        assert_eq!(stored_snippet_name(&database_b, "epoch-1").as_deref(), Some("epoch-1"));
    }

    #[tokio::test]
    async fn a_walk_that_fails_partway_applies_everything_on_the_next_pull() {
        let database_a = Database::open_memory();
        let database_b = Database::open_memory();
        let fake = Arc::new(FakeTransport::new("shared"));
        let transports: Vec<Arc<dyn SyncTransport>> = vec![fake.clone()];

        let keys_a = test_keys(&database_a);
        for id in ["first", "second", "third"] {
            write_snippet(&database_a, id);
            push_pending_events(&database_a, &keys_a, &transports).await.unwrap();
        }
        let keys_b = test_keys(&database_b);
        database_b.trust_device_public_key(keys_a.device_id.as_bytes(), &keys_a.signing_key.verifying_key()).unwrap();

        // The oldest event isn't visible yet, so the walk from the newest
        // one fails after fetching the two newer events.
        fake.inject_delayed_visibility(&TransportCid(own_chunk_index_cid(&database_a, 1)), 1);
        let outcome = pull_from_transports(&database_b, &keys_b, &transports).await.unwrap();
        assert_eq!(outcome.failed_transports, 1);
        assert_eq!(outcome.applied_events, 0);
        assert_eq!(stored_snippet_name(&database_b, "third"), None);

        let outcome = pull_from_transports(&database_b, &keys_b, &transports).await.unwrap();
        assert_eq!(outcome.failed_transports, 0);
        assert_eq!(outcome.applied_events, 3);
        for id in ["first", "second", "third"] {
            assert_eq!(stored_snippet_name(&database_b, id).as_deref(), Some(id));
        }
    }

    #[tokio::test]
    async fn events_an_earlier_build_stored_without_applying_are_fetched_again_and_applied() {
        let database_a = Database::open_memory();
        let database_b = Database::open_memory();
        let transports: Vec<Arc<dyn SyncTransport>> = vec![Arc::new(FakeTransport::new("shared"))];

        let keys_a = test_keys(&database_a);
        for id in ["first", "second"] {
            write_snippet(&database_a, id);
            push_pending_events(&database_a, &keys_a, &transports).await.unwrap();
        }

        // Reproduce what a failed walk used to leave behind on B: A's newest
        // event stored locally, never applied.
        let newest_index_cid = own_chunk_index_cid(&database_a, 2);
        let (event_id_hex, index_bytes): (String, Vec<u8>) = database_a
            .connection()
            .unwrap()
            .query_row("SELECT event_id, bytes FROM sync_objects WHERE cid=?1", params![newest_index_cid], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        let index: ChunkIndex = serde_json::from_slice(&index_bytes).unwrap();
        let chunks: Vec<Vec<u8>> = index
            .chunk_cids
            .iter()
            .map(|cid| {
                database_a
                    .connection()
                    .unwrap()
                    .query_row("SELECT bytes FROM sync_objects WHERE cid=?1", params![cid], |row| row.get(0))
                    .unwrap()
            })
            .collect();
        let keys_b = test_keys(&database_b);
        database_b.remember_remote_message(&newest_index_cid, &index.chunk_cids, &chunks, &event_id_hex).unwrap();
        database_b.enqueue_repair_deliveries(&[TransportInstanceId("shared".to_string())]).unwrap();
        database_b.trust_device_public_key(keys_a.device_id.as_bytes(), &keys_a.signing_key.verifying_key()).unwrap();

        let outcome = pull_from_transports(&database_b, &keys_b, &transports).await.unwrap();
        assert_eq!(outcome.applied_events, 2);
        assert_eq!(stored_snippet_name(&database_b, "first").as_deref(), Some("first"));
        assert_eq!(stored_snippet_name(&database_b, "second").as_deref(), Some("second"));

        // Applied remote events and this device's own events are kept.
        write_snippet(&database_b, "own");
        push_pending_events(&database_b, &keys_b, &transports).await.unwrap();
        let before = stored_object_count(&database_b);
        assert_eq!(database_b.forget_unapplied_remote_messages().unwrap(), 0);
        assert_eq!(stored_object_count(&database_b), before);
    }

    fn trust(database: &Database, keys: &LocalKeys) {
        database.trust_device_public_key(keys.device_id.as_bytes(), &keys.signing_key.verifying_key()).unwrap();
    }

    fn fake(name: &str) -> Vec<Arc<dyn SyncTransport>> {
        vec![Arc::new(FakeTransport::new(name))]
    }

    fn progress_row(database: &Database, keys: &LocalKeys) -> (i64, i64) {
        database
            .connection()
            .unwrap()
            .query_row(
                "SELECT applied_sequence, progress_sequence FROM sync_device_progress WHERE device_id=?1",
                params![encode_id(keys.device_id.as_bytes())],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .unwrap()
            .unwrap_or((0, 0))
    }

    fn update_snippet(database: &Database, id: &str, name: &str) {
        database
            .record_replicated_write(EntityType::Snippet, id, &fields(&["name"]), &snippet_payload(id, name))
            .unwrap();
    }

    fn signed_head_for(keys: &LocalKeys, contiguous_sequence: u64, latest_event_cid: Option<String>) -> SignedDeviceHead {
        sign_device_head(
            &keys.signing_key,
            DeviceHead {
                sync_space_id: keys.sync_space_id.clone(),
                device_id: keys.device_id,
                epoch: keys.key_epoch,
                contiguous_sequence,
                latest_event_cid,
                published_at_ms: crate::sync_policy::now_ms(),
                ack: vec![],
                snapshot_cid: None,
            },
        )
        .unwrap()
    }

    #[tokio::test]
    async fn a_sealed_event_names_everything_its_author_had_applied() {
        let (database_a, database_c) = (Database::open_memory(), Database::open_memory());
        let shared = fake("shared");
        write_snippet(&database_c, "from-c");
        let keys_c = test_keys(&database_c);
        push_pending_events(&database_c, &keys_c, &shared).await.unwrap();

        let keys_a = test_keys(&database_a);
        trust(&database_a, &keys_c);
        pull_from_transports(&database_a, &keys_a, &shared).await.unwrap();
        update_snippet(&database_a, "from-c", "edited by A");
        push_pending_events(&database_a, &keys_a, &shared).await.unwrap();

        let vector: String = database_a
            .connection()
            .unwrap()
            .query_row(
                "SELECT causal_vector FROM sync_events WHERE device_id=?1 AND device_sequence=1",
                params![encode_id(keys_a.device_id.as_bytes())],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(vector, format!("[[\"{}\",1]]", encode_id(keys_c.device_id.as_bytes())));
    }

    #[tokio::test]
    async fn progress_waits_for_an_events_dependencies_and_advances_once_they_arrive() {
        let (database_a, database_b, database_c) = (Database::open_memory(), Database::open_memory(), Database::open_memory());
        let (only_a, only_c) = (fake("a-only"), fake("c-only"));

        // C writes; A reads it and edits the same snippet, so A's event
        // depends on C's.
        write_snippet(&database_c, "shared-note");
        let keys_c = test_keys(&database_c);
        push_pending_events(&database_c, &keys_c, &only_c).await.unwrap();
        let keys_a = test_keys(&database_a);
        trust(&database_a, &keys_c);
        pull_from_transports(&database_a, &keys_a, &only_c).await.unwrap();
        update_snippet(&database_a, "shared-note", "edited by A");
        push_pending_events(&database_a, &keys_a, &only_a).await.unwrap();

        // B sees only A's connector: it applies A's event but can't count
        // it as closed without C's.
        let keys_b = test_keys(&database_b);
        trust(&database_b, &keys_a);
        trust(&database_b, &keys_c);
        let outcome = pull_from_transports(&database_b, &keys_b, &only_a).await.unwrap();
        assert_eq!(outcome.applied_events, 1);
        assert_eq!(progress_row(&database_b, &keys_a), (1, 0));
        assert!(database_b.progress_vector().unwrap().iter().all(|entry| entry.device_id != keys_a.device_id));

        // Once C's event arrives, both are closed.
        let both: Vec<Arc<dyn SyncTransport>> = only_a.iter().chain(only_c.iter()).cloned().collect();
        let outcome = pull_from_transports(&database_b, &keys_b, &both).await.unwrap();
        assert_eq!(outcome.applied_events, 1);
        assert_eq!(progress_row(&database_b, &keys_a), (1, 1));
        assert_eq!(progress_row(&database_b, &keys_c), (1, 1));
        assert_eq!(stored_snippet_name(&database_b, "shared-note").as_deref(), Some("edited by A"));

        // B's own next event depends on both, and B's progress is closed.
        write_snippet(&database_b, "from-b");
        push_pending_events(&database_b, &keys_b, &only_a).await.unwrap();
        assert_eq!(progress_row(&database_b, &keys_b), (1, 1));
    }

    #[tokio::test]
    async fn the_same_feed_from_two_connectors_applies_once() {
        let (database_a, database_b) = (Database::open_memory(), Database::open_memory());
        let both: Vec<Arc<dyn SyncTransport>> = vec![Arc::new(FakeTransport::new("one")), Arc::new(FakeTransport::new("two"))];
        let keys_a = test_keys(&database_a);
        for id in ["first", "second"] {
            write_snippet(&database_a, id);
            push_pending_events(&database_a, &keys_a, &both).await.unwrap();
        }
        let keys_b = test_keys(&database_b);
        trust(&database_b, &keys_a);
        let outcome = pull_from_transports(&database_b, &keys_b, &both).await.unwrap();
        assert_eq!(outcome.applied_events, 2);
        assert_eq!(outcome.failed_transports, 0);
        assert_eq!(progress_row(&database_b, &keys_a), (2, 2));
        assert_eq!(pull_from_transports(&database_b, &keys_b, &both).await.unwrap().applied_events, 0);
    }

    #[tokio::test]
    async fn a_stale_head_naming_already_applied_events_changes_nothing() {
        let (database_a, database_b) = (Database::open_memory(), Database::open_memory());
        let shared = fake("shared");
        let keys_a = test_keys(&database_a);
        write_snippet(&database_a, "note");
        push_pending_events(&database_a, &keys_a, &shared).await.unwrap();
        update_snippet(&database_a, "note", "second name");
        push_pending_events(&database_a, &keys_a, &shared).await.unwrap();
        let keys_b = test_keys(&database_b);
        trust(&database_b, &keys_a);
        pull_from_transports(&database_b, &keys_b, &shared).await.unwrap();

        // A lagging connector still holds A's first event and a head that
        // names it. B already has it, so nothing is fetched or re-applied,
        // and no conflict appears.
        let lagging = fake("lagging");
        let first_index = own_chunk_index_cid(&database_a, 1);
        lagging[0].publish_head(&signed_head_for(&keys_a, 1, Some(first_index))).await.unwrap();
        let outcome = pull_from_transports(&database_b, &keys_b, &lagging).await.unwrap();
        assert_eq!(outcome.failed_transports, 0);
        assert_eq!(outcome.applied_events, 0);
        assert!(database_b.list_frontier_conflicts().unwrap().is_empty());
        assert_eq!(stored_snippet_name(&database_b, "note").as_deref(), Some("second name"));
    }

    #[tokio::test]
    async fn a_head_whose_feed_skips_or_reorders_events_applies_nothing() {
        let (database_a, database_b) = (Database::open_memory(), Database::open_memory());
        let shared = fake("shared");
        let keys_a = test_keys(&database_a);
        for id in ["first", "second"] {
            write_snippet(&database_a, id);
            push_pending_events(&database_a, &keys_a, &shared).await.unwrap();
        }
        // A head claiming a third event while naming the second.
        let forged = fake("forged");
        for cid in [own_chunk_index_cid(&database_a, 2)] {
            let bytes = shared[0].get_object(&TransportCid(cid.clone())).await.unwrap();
            forged[0].put_object(&TransportCid(cid.clone()), &bytes).await.unwrap();
            let index: ChunkIndex = serde_json::from_slice(&bytes).unwrap();
            for chunk in index.chunk_cids {
                let chunk_bytes = shared[0].get_object(&TransportCid(chunk.clone())).await.unwrap();
                forged[0].put_object(&TransportCid(chunk), &chunk_bytes).await.unwrap();
            }
        }
        forged[0].publish_head(&signed_head_for(&keys_a, 3, Some(own_chunk_index_cid(&database_a, 2)))).await.unwrap();

        let keys_b = test_keys(&database_b);
        trust(&database_b, &keys_a);
        let outcome = pull_from_transports(&database_b, &keys_b, &forged).await.unwrap();
        assert_eq!(outcome.failed_transports, 1);
        assert_eq!(outcome.applied_events, 0);
        assert_eq!(progress_row(&database_b, &keys_a), (0, 0));
        assert_eq!(stored_object_count(&database_b), 0);
    }

    async fn published_head(transports: &[Arc<dyn SyncTransport>], keys: &LocalKeys) -> Option<SignedDeviceHead> {
        transports[0]
            .resolve_heads(&[HeadLocator { device_id: keys.device_id, remote_id: None }])
            .await
            .unwrap()
            .into_iter()
            .next()
    }

    #[tokio::test]
    async fn an_unchanged_head_is_republished_only_when_the_heartbeat_is_due() {
        let database = Database::open_memory();
        let shared = fake("shared");
        let keys = test_keys(&database);
        let start = 1_800_000_000_000;
        crate::sync_policy::set_test_clock(Some(start));

        // Published even with nothing written yet.
        push_pending_events(&database, &keys, &shared).await.unwrap();
        let head = published_head(&shared, &keys).await.unwrap();
        assert_eq!(head.head.published_at_ms, start);
        assert_eq!((head.head.contiguous_sequence, head.head.latest_event_cid.clone()), (0, None));

        crate::sync_policy::set_test_clock(Some(start + crate::sync_policy::HEAD_HEARTBEAT_MS - 1));
        push_pending_events(&database, &keys, &shared).await.unwrap();
        assert_eq!(published_head(&shared, &keys).await.unwrap().head.published_at_ms, start);

        let due = start + crate::sync_policy::HEAD_HEARTBEAT_MS;
        crate::sync_policy::set_test_clock(Some(due));
        push_pending_events(&database, &keys, &shared).await.unwrap();
        assert_eq!(published_head(&shared, &keys).await.unwrap().head.published_at_ms, due);

        // A change is published at once.
        crate::sync_policy::set_test_clock(Some(due + 1));
        write_snippet(&database, "note");
        push_pending_events(&database, &keys, &shared).await.unwrap();
        let head = published_head(&shared, &keys).await.unwrap().head;
        assert_eq!(head.published_at_ms, due + 1);
        assert_eq!(head.contiguous_sequence, 1);
        assert_eq!(head.ack, vec![threestrands_sync_envelope::SequenceEntry { device_id: keys.device_id, sequence: 1 }]);
        crate::sync_policy::set_test_clock(None);
    }

    #[tokio::test]
    async fn a_pull_records_each_peers_ack_and_publication_time() {
        let (database_a, database_b) = (Database::open_memory(), Database::open_memory());
        let shared = fake("shared");
        crate::sync_policy::set_test_clock(Some(1_800_000_000_000));
        let keys_a = test_keys(&database_a);
        write_snippet(&database_a, "note");
        push_pending_events(&database_a, &keys_a, &shared).await.unwrap();

        crate::sync_policy::set_test_clock(Some(1_800_000_060_000));
        let keys_b = test_keys(&database_b);
        trust(&database_b, &keys_a);
        pull_from_transports(&database_b, &keys_b, &shared).await.unwrap();
        let (published, seen, ack): (i64, i64, String) = database_b
            .connection()
            .unwrap()
            .query_row(
                "SELECT last_head_published_at_ms, last_head_seen_at_ms, ack_json FROM sync_device_progress WHERE device_id=?1",
                params![encode_id(keys_a.device_id.as_bytes())],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!((published, seen), (1_800_000_000_000, 1_800_000_060_000));
        assert_eq!(ack, format!("[[\"{}\",1]]", encode_id(keys_a.device_id.as_bytes())));
        crate::sync_policy::set_test_clock(None);
    }

    #[tokio::test]
    async fn pull_skips_a_head_from_an_untrusted_device() {
        let database_a = Database::open_memory();
        let database_b = Database::open_memory();

        database_b
            .record_replicated_write(EntityType::Snippet, "b-1", &fields(&["id", "name", "body", "createdAt"]), &snippet_payload("b-1", "From B"))
            .unwrap();
        let keys_b = test_keys(&database_b);
        let shared: Arc<dyn SyncTransport> = Arc::new(FakeTransport::new("shared"));
        let transports = vec![shared];
        push_pending_events(&database_b, &keys_b, &transports).await.unwrap();

        // A never calls trust_device_public_key for B this time.
        let keys_a = test_keys(&database_a);
        let outcome = pull_from_transports(&database_a, &keys_a, &transports).await.unwrap();
        assert_eq!(outcome.applied_events, 0);

        let missing = database_a
            .connection()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM snippets WHERE id='b-1'", [], |row| row.get::<_, i64>(0))
            .unwrap();
        assert_eq!(missing, 0);
    }

    #[tokio::test]
    async fn repair_delivers_a_pulled_object_to_a_second_transport() {
        let database_a = Database::open_memory();
        let database_b = Database::open_memory();

        database_b
            .record_replicated_write(EntityType::Snippet, "b-1", &fields(&["id", "name", "body", "createdAt"]), &snippet_payload("b-1", "From B"))
            .unwrap();
        let keys_b = test_keys(&database_b);

        let transport_x: Arc<dyn SyncTransport> = Arc::new(FakeTransport::new("x"));
        let transport_y: Arc<dyn SyncTransport> = Arc::new(FakeTransport::new("y"));

        // B only pushes to transport X.
        push_pending_events(&database_b, &keys_b, std::slice::from_ref(&transport_x)).await.unwrap();

        let keys_a = test_keys(&database_a);
        database_a.trust_device_public_key(keys_b.device_id.as_bytes(), &keys_b.signing_key.verifying_key()).unwrap();

        // A knows about both transports and pulls from X.
        let outcome = pull_from_transports(&database_a, &keys_a, &[transport_x.clone(), transport_y.clone()]).await.unwrap();
        assert_eq!(outcome.applied_events, 1);

        // Repair queued delivery to Y even though A never pushed anything
        // of its own.
        let (pending_y, _, _) = database_a.delivery_counts("y").unwrap();
        assert!(pending_y > 0);

        push_pending_events(&database_a, &keys_a, &[transport_x, transport_y]).await.unwrap();
        let (pending_y_after, delivered_y, _) = database_a.delivery_counts("y").unwrap();
        assert_eq!(pending_y_after, 0);
        assert!(delivered_y > 0);
    }

    #[tokio::test]
    async fn one_failed_transport_never_blocks_pull_from_a_healthy_one() {
        let database_a = Database::open_memory();
        let database_b = Database::open_memory();

        database_b
            .record_replicated_write(EntityType::Snippet, "b-1", &fields(&["id", "name", "body", "createdAt"]), &snippet_payload("b-1", "From B"))
            .unwrap();
        let keys_b = test_keys(&database_b);

        let healthy: Arc<dyn SyncTransport> = Arc::new(FakeTransport::new("healthy"));
        let failing_fake = FakeTransport::new("failing");
        failing_fake.set_authentication_failure(true);
        let failing: Arc<dyn SyncTransport> = Arc::new(failing_fake);

        push_pending_events(&database_b, &keys_b, std::slice::from_ref(&healthy)).await.unwrap();

        let keys_a = test_keys(&database_a);
        database_a.trust_device_public_key(keys_b.device_id.as_bytes(), &keys_b.signing_key.verifying_key()).unwrap();

        let outcome = pull_from_transports(&database_a, &keys_a, &[failing, healthy]).await.unwrap();
        assert_eq!(outcome.failed_transports, 1);
        assert_eq!(outcome.applied_events, 1);
    }

    #[test]
    fn shared_retry_at_is_in_the_future_and_grows_with_attempts() {
        let now = Utc::now().to_rfc3339();
        let first = retry_at(1);
        let later = retry_at(6);
        assert!(first.as_str() > now.as_str());
        assert!(later.as_str() > first.as_str());
    }
}

#[cfg(test)]
mod config_tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;

    struct TempFolder {
        path: std::path::PathBuf,
    }

    impl TempFolder {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("threestrands-replicated-sync-config-test-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempFolder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn add_folder_transport_persists_and_lists_it() {
        let database = Database::open_memory();
        let folder = TempFolder::new();
        database.add_folder_transport("folder-1", &folder.path).unwrap();

        let rows = database.configured_transports().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].instance_id, "folder-1");
        assert_eq!(rows[0].kind, "folder");
        assert!(rows[0].enabled);
        assert_eq!(
            rows[0].config(),
            Some(TransportConfig::Folder(FolderConfig { path: folder.path.to_string_lossy().into_owned(), label: None }))
        );
    }

    #[test]
    fn remove_transport_clears_config_and_deliveries() {
        let database = Database::open_memory();
        let folder = TempFolder::new();
        database.add_folder_transport("folder-1", &folder.path).unwrap();
        database
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO sync_deliveries(cid,transport_instance_id,state,attempts) VALUES ('c1','folder-1','pending',0)",
                [],
            )
            .unwrap();

        database.remove_transport("folder-1").unwrap();

        assert!(database.configured_transports().unwrap().is_empty());
        let remaining: i64 = database
            .connection()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM sync_deliveries WHERE transport_instance_id='folder-1'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(remaining, 0);
    }

    #[tokio::test]
    async fn build_configured_transports_skips_a_disabled_or_unopenable_row() {
        let database = Database::open_memory();
        let folder = TempFolder::new();
        database.add_folder_transport("folder-1", &folder.path).unwrap();
        database
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO sync_transports(instance_id,kind,config_json,required,enabled) VALUES ('folder-2','folder','{\"path\":\"/nonexistent/definitely-not-real\"}',1,1)",
                [],
            )
            .unwrap();
        database
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO sync_transports(instance_id,kind,config_json,required,enabled) VALUES ('folder-3','folder',?1,1,0)",
                params![serde_json::json!({"path": folder.path.to_string_lossy()}).to_string()],
            )
            .unwrap();

        let transports = build_configured_transports(&database).await;
        assert_eq!(transports.len(), 1);
        assert_eq!(transports[0].instance_id().0, "folder-1");
    }

    #[test]
    fn add_ipfs_rpc_transport_persists_only_non_secret_config() {
        let database = Database::open_memory();
        // Secrets go to an in-memory stand-in for the keychain in tests
        // (see `sync_connectors::secret_store`).
        let id = format!("ipfs-{}", Uuid::new_v4());
        database.add_ipfs_rpc_transport(&id, "https://rpc.filebase.io", Some("ipfs-token-value")).unwrap();

        let rows = database.configured_transports().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "ipfs_rpc");
        assert_eq!(rows[0].config().map(|config| config.location()).as_deref(), Some("https://rpc.filebase.io"));
        // No credential ever appears in the stored config.
        assert!(!rows[0].config_json.to_ascii_lowercase().contains("token"));
        assert!(!rows[0].config_json.contains("ipfs-token-value"));
        assert_eq!(
            TransportSecrets::load("ipfs_rpc", &id).unwrap(),
            Some(TransportSecrets::IpfsRpcToken("ipfs-token-value".to_string()))
        );

        // Re-adding without a token clears the stored one.
        database.add_ipfs_rpc_transport(&id, "https://rpc.filebase.io", None).unwrap();
        assert_eq!(TransportSecrets::load("ipfs_rpc", &id).unwrap(), None);
    }

    #[tokio::test]
    async fn build_configured_transports_includes_an_ipfs_rpc_row() {
        let database = Database::open_memory();
        database.add_ipfs_rpc_transport("ipfs-1", "https://rpc.filebase.io", None).unwrap();
        let transports = build_configured_transports(&database).await;
        assert_eq!(transports.len(), 1);
        assert_eq!(transports[0].instance_id().0, "ipfs-1");
    }

    #[test]
    fn removing_an_ipfs_rpc_transport_clears_its_config_row() {
        let database = Database::open_memory();
        database.add_ipfs_rpc_transport("ipfs-1", "https://rpc.filebase.io", None).unwrap();
        database.remove_transport("ipfs-1").unwrap();
        assert!(database.configured_transports().unwrap().is_empty());
    }

    fn s3_test_config(endpoint: &str) -> crate::s3_transport::S3Config {
        crate::s3_transport::S3Config {
            endpoint: endpoint.to_string(),
            region: "us-east-1".to_string(),
            bucket: "sync-bucket".to_string(),
            prefix: "team".to_string(),
            path_style: false,
            label: Some("Team bucket".to_string()),
        }
    }

    fn s3_test_credentials() -> crate::s3_transport::S3Credentials {
        crate::s3_transport::S3Credentials {
            access_key_id: "AKIAEXAMPLE".to_string(),
            secret_access_key: "s3-secret-value".to_string(),
            session_token: Some("s3-session-token".to_string()),
        }
    }

    #[test]
    fn s3_transport_config_persists_without_any_secret() {
        let database = Database::open_memory();
        let id = format!("s3-{}", Uuid::new_v4());
        database
            .add_s3_transport(&id, &s3_test_config("https://s3.us-east-1.amazonaws.com"), &s3_test_credentials())
            .unwrap();

        let rows = database.configured_transports().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "s3");
        assert_eq!(rows[0].config(), Some(TransportConfig::S3(s3_test_config("https://s3.us-east-1.amazonaws.com"))));
        let lowered = rows[0].config_json.to_ascii_lowercase();
        for secret_field in ["secret", "accesskey", "access_key", "token", "credential", "akiaexample"] {
            assert!(!lowered.contains(secret_field), "{secret_field} in {lowered}");
        }
        assert_eq!(
            TransportSecrets::load("s3", &id).unwrap(),
            Some(TransportSecrets::S3(s3_test_credentials()))
        );
    }

    #[test]
    fn add_s3_transport_rejects_an_invalid_config_before_writing_anything() {
        let database = Database::open_memory();
        let id = format!("s3-{}", Uuid::new_v4());
        // Plaintext HTTP to a remote host fails validation, which runs
        // before both the SQLite and the keychain writes.
        let error = database
            .add_s3_transport(&id, &s3_test_config("http://s3.example.com"), &s3_test_credentials())
            .unwrap_err();
        assert!(error.contains("HTTPS"), "{error}");
        assert!(database.configured_transports().unwrap().is_empty());
        assert_eq!(TransportSecrets::load("s3", &id).unwrap(), None);
    }

    #[tokio::test]
    async fn an_s3_row_without_stored_credentials_is_skipped_and_reported_unavailable() {
        let database = Database::open_memory();
        let config_json = TransportConfig::S3(s3_test_config("https://s3.us-east-1.amazonaws.com")).to_config_json().unwrap();
        database
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO sync_transports(instance_id,kind,config_json,required,enabled) VALUES (?1,'s3',?2,1,1)",
                params![format!("s3-{}", Uuid::new_v4()), config_json],
            )
            .unwrap();
        assert!(build_configured_transports(&database).await.is_empty());

        let engine = ReplicatedSync::new(Arc::new(database));
        let statuses = engine.status().await.unwrap();
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].kind, "s3");
        assert_eq!(statuses[0].label.as_deref(), Some("Team bucket"));
        assert_eq!(statuses[0].location, "https://s3.us-east-1.amazonaws.com · sync-bucket/team");
        assert!(statuses[0].supports_delete_data);
        assert_eq!(statuses[0].s3_config, Some(s3_test_config("https://s3.us-east-1.amazonaws.com")));
        let json = serde_json::to_string(&statuses[0]).unwrap();
        assert!(json.contains("\"s3Config\":{\"endpoint\""), "{json}");
        assert!(!json.to_ascii_lowercase().contains("secret"), "{json}");
        assert!(statuses[0].health.starts_with("unavailable"));
    }

    #[test]
    fn removing_a_connector_clears_its_row_and_its_secret() {
        let database = Database::open_memory();
        let s3_id = format!("s3-{}", Uuid::new_v4());
        database
            .add_s3_transport(&s3_id, &s3_test_config("https://s3.us-east-1.amazonaws.com"), &s3_test_credentials())
            .unwrap();
        let ipfs_id = format!("ipfs-{}", Uuid::new_v4());
        database.add_ipfs_rpc_transport(&ipfs_id, "https://rpc.filebase.io", Some("token")).unwrap();

        database.remove_transport(&s3_id).unwrap();
        assert_eq!(TransportSecrets::load("s3", &s3_id).unwrap(), None);
        assert!(crate::sync_connectors::secret_store::values_for(&s3_id).is_empty());
        // The other connector and its secret are untouched.
        assert!(TransportSecrets::load("ipfs_rpc", &ipfs_id).unwrap().is_some());

        database.remove_transport(&ipfs_id).unwrap();
        assert!(database.configured_transports().unwrap().is_empty());
        assert!(crate::sync_connectors::secret_store::values_for(&ipfs_id).is_empty());
    }

    #[test]
    fn removing_a_row_of_an_unknown_kind_clears_every_possible_secret() {
        let database = Database::open_memory();
        let id = format!("future-{}", Uuid::new_v4());
        database
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO sync_transports(instance_id,kind,config_json,required,enabled) VALUES (?1,'carrier-pigeon','{}',1,1)",
                params![id],
            )
            .unwrap();
        TransportSecrets::IpfsRpcToken("stale".to_string()).store(&id).unwrap();
        database.remove_transport(&id).unwrap();
        assert!(crate::sync_connectors::secret_store::values_for(&id).is_empty());
    }

    #[tokio::test]
    async fn an_unknown_kind_is_skipped_and_reported_without_a_location() {
        let database = Database::open_memory();
        database
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO sync_transports(instance_id,kind,config_json,required,enabled) VALUES ('future-1','carrier-pigeon','{}',1,1)",
                [],
            )
            .unwrap();
        assert!(build_configured_transports(&database).await.is_empty());
        let statuses = ReplicatedSync::new(Arc::new(database)).status().await.unwrap();
        assert_eq!(statuses[0].location, "");
        assert_eq!(statuses[0].label, None);
        assert!(!statuses[0].supports_delete_data);
        assert_eq!(statuses[0].health, "unavailable: not configured");
    }

    #[test]
    fn update_renames_a_connector_without_losing_its_ledger_or_secret() {
        let database = Database::open_memory();
        let id = format!("s3-{}", Uuid::new_v4());
        database
            .add_s3_transport(&id, &s3_test_config("https://s3.us-east-1.amazonaws.com"), &s3_test_credentials())
            .unwrap();
        database
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO sync_deliveries(cid,transport_instance_id,state,attempts) VALUES ('c1',?1,'delivered',1)",
                params![id],
            )
            .unwrap();
        database.set_transport_success(&id).unwrap();

        let mut renamed = database.configured_transports().unwrap()[0].config().unwrap();
        renamed.set_label(Some("Renamed"));
        database.update_transport_config(&id, &renamed, None).unwrap();

        let row = &database.configured_transports().unwrap()[0];
        assert_eq!(row.config().unwrap().label(), Some("Renamed"));
        assert!(row.last_success_at.is_some());
        assert_eq!(database.delivery_counts(&id).unwrap(), (0, 1, 0));
        assert_eq!(TransportSecrets::load("s3", &id).unwrap(), Some(TransportSecrets::S3(s3_test_credentials())));
    }

    #[test]
    fn update_can_rotate_credentials_and_refuses_a_kind_change_or_invalid_config() {
        let database = Database::open_memory();
        let id = format!("s3-{}", Uuid::new_v4());
        let config = TransportConfig::S3(s3_test_config("https://s3.us-east-1.amazonaws.com"));
        database
            .add_transport(&id, &config, Some(&TransportSecrets::S3(s3_test_credentials())))
            .unwrap();

        let mut rotated = s3_test_credentials();
        rotated.secret_access_key = "rotated-secret".to_string();
        database.update_transport_config(&id, &config, Some(&TransportSecrets::S3(rotated.clone()))).unwrap();
        assert_eq!(TransportSecrets::load("s3", &id).unwrap(), Some(TransportSecrets::S3(rotated.clone())));

        let folder = TransportConfig::Folder(FolderConfig { path: "/tmp".to_string(), label: None });
        assert!(database.update_transport_config(&id, &folder, None).is_err());

        let TransportConfig::S3(mut insecure) = config.clone() else { unreachable!() };
        insecure.endpoint = "http://s3.example.com".to_string();
        assert!(database.update_transport_config(&id, &TransportConfig::S3(insecure), None).is_err());
        // Nothing changed after the refusals.
        assert_eq!(database.configured_transports().unwrap()[0].config(), Some(config));
        assert_eq!(TransportSecrets::load("s3", &id).unwrap(), Some(TransportSecrets::S3(rotated)));

        assert!(database.update_transport_config("missing", &folder, None).is_err());
    }

    #[tokio::test]
    async fn probe_s3_checks_permissions_and_an_empty_bucket_without_persisting_anything() {
        use crate::s3_transport::fake_server::FakeS3Server;
        let server = FakeS3Server::spawn().await;
        let database = Arc::new(Database::open_memory());
        let engine = ReplicatedSync::new(database.clone());

        let test = engine.probe_s3(&server.config("group"), &FakeS3Server::credentials()).await.unwrap();
        assert!(test.checks.can_list && test.checks.can_write && test.checks.can_read && test.checks.can_delete);
        assert_eq!(test.space_presence, Some(crate::enrollment::SyncSpacePresence::None));
        assert!(database.configured_transports().unwrap().is_empty());
        assert!(server.state().objects.is_empty());
    }

    #[tokio::test]
    async fn probe_s3_skips_the_group_check_when_the_key_cannot_list() {
        use crate::s3_transport::fake_server::FakeS3Server;
        let server = FakeS3Server::spawn().await;
        let mut credentials = FakeS3Server::credentials();
        credentials.secret_access_key = "wrong".to_string();
        let test = ReplicatedSync::new(Arc::new(Database::open_memory()))
            .probe_s3(&server.config(""), &credentials)
            .await
            .unwrap();
        assert!(test.checks.reachable && !test.checks.can_list);
        assert_eq!(test.space_presence, None);
    }

    #[tokio::test]
    async fn probe_s3_rejects_an_invalid_config_before_any_request() {
        use crate::s3_transport::fake_server::FakeS3Server;
        let server = FakeS3Server::spawn().await;
        let mut config = server.config("");
        config.path_style = false; // an IP-address endpoint needs path-style
        let error = ReplicatedSync::new(Arc::new(Database::open_memory()))
            .probe_s3(&config, &FakeS3Server::credentials())
            .await
            .unwrap_err();
        assert!(error.contains("path-style"), "{error}");
        assert_eq!(server.state().faults.requests, 0);
    }

    #[test]
    fn a_connection_test_serializes_flat_in_camel_case() {
        let test = S3ConnectionTest {
            checks: S3ProbeReport { reachable: true, can_list: true, versioning_enabled: Some(true), ..Default::default() },
            space_presence: Some(crate::enrollment::SyncSpacePresence::Existing),
        };
        assert_eq!(
            serde_json::to_value(&test).unwrap(),
            json!({
                "reachable": true, "canList": true, "canWrite": false, "canRead": false, "canDelete": false,
                "versioningEnabled": true, "error": null, "spacePresence": "existing"
            })
        );
    }

    #[tokio::test]
    async fn an_added_s3_connector_reports_healthy_with_a_storage_estimate() {
        use crate::s3_transport::fake_server::FakeS3Server;
        let server = FakeS3Server::spawn().await;
        let database = Arc::new(Database::open_memory());
        let id = format!("s3-{}", Uuid::new_v4());
        database.add_s3_transport(&id, &server.config("group"), &FakeS3Server::credentials()).unwrap();
        server.state().objects.insert("group/threestrands-sync/objects/ab/x.block".to_string(), vec![0; 10]);

        let statuses = ReplicatedSync::new(database).status().await.unwrap();
        assert_eq!(statuses[0].health, "healthy");
        assert_eq!(statuses[0].storage_bytes, Some(10));
        assert!(statuses[0].head_discovery);
    }

    #[test]
    fn update_connector_renames_clears_and_rotates_credentials() {
        let database = Arc::new(Database::open_memory());
        let engine = ReplicatedSync::new(database.clone());
        let id = format!("s3-{}", Uuid::new_v4());
        database
            .add_s3_transport(&id, &s3_test_config("https://s3.us-east-1.amazonaws.com"), &s3_test_credentials())
            .unwrap();
        let label = || database.configured_transports().unwrap()[0].config().unwrap().label().map(str::to_string);

        engine.update_connector(&id, Some("  Personal R2 "), None).unwrap();
        assert_eq!(label().as_deref(), Some("Personal R2"));

        // No label argument keeps the name.
        let rotated: ConnectorCredentials = serde_json::from_value(json!({
            "kind": "s3", "accessKeyId": "AKIAROTATED", "secretAccessKey": "rotated-secret"
        }))
        .unwrap();
        engine.update_connector(&id, None, Some(rotated)).unwrap();
        assert_eq!(label().as_deref(), Some("Personal R2"));
        match TransportSecrets::load("s3", &id).unwrap() {
            Some(TransportSecrets::S3(credentials)) => assert_eq!(credentials.access_key_id, "AKIAROTATED"),
            other => panic!("unexpected {other:?}"),
        }

        engine.update_connector(&id, Some(""), None).unwrap();
        assert_eq!(label(), None);
    }

    #[test]
    fn update_connector_refuses_mismatched_credentials_long_names_and_unknown_ids() {
        let database = Arc::new(Database::open_memory());
        let engine = ReplicatedSync::new(database.clone());
        let folder = TempFolder::new();
        database.add_folder_transport("folder-1", &folder.path).unwrap();

        let token: ConnectorCredentials = serde_json::from_value(json!({ "kind": "ipfs_rpc", "token": "t" })).unwrap();
        assert!(engine.update_connector("folder-1", None, Some(token)).is_err());
        assert!(engine.update_connector("folder-1", Some(&"x".repeat(61)), None).is_err());
        assert_eq!(database.configured_transports().unwrap()[0].config().unwrap().label(), None);
        assert!(engine.update_connector("missing", Some("name"), None).is_err());

        let ipfs_id = format!("ipfs-{}", Uuid::new_v4());
        database.add_ipfs_rpc_transport(&ipfs_id, "https://rpc.filebase.io", None).unwrap();
        let s3: ConnectorCredentials =
            serde_json::from_value(json!({ "kind": "s3", "accessKeyId": "a", "secretAccessKey": "b" })).unwrap();
        assert!(engine.update_connector(&ipfs_id, None, Some(s3)).is_err());
        let token: ConnectorCredentials = serde_json::from_value(json!({ "kind": "ipfs_rpc", "token": "new" })).unwrap();
        engine.update_connector(&ipfs_id, None, Some(token)).unwrap();
        assert_eq!(
            TransportSecrets::load("ipfs_rpc", &ipfs_id).unwrap(),
            Some(TransportSecrets::IpfsRpcToken("new".to_string()))
        );
    }

    #[tokio::test]
    async fn status_offers_data_deletion_for_folders_but_not_ipfs() {
        let database = Database::open_memory();
        let folder = TempFolder::new();
        database.add_folder_transport("folder-1", &folder.path).unwrap();
        database.add_ipfs_rpc_transport(&format!("ipfs-{}", Uuid::new_v4()), "https://rpc.filebase.io", None).unwrap();
        let statuses = ReplicatedSync::new(Arc::new(database)).status().await.unwrap();
        let by_kind = |kind: &str| statuses.iter().find(|status| status.kind == kind).unwrap();
        assert!(by_kind("folder").supports_delete_data);
        assert!(!by_kind("ipfs_rpc").supports_delete_data);
        assert_eq!(by_kind("folder").s3_config, None);
        assert_eq!(by_kind("folder").label, None);
    }

    #[tokio::test]
    async fn status_reports_health_and_a_storage_estimate_for_a_real_folder() {
        let database = Database::open_memory();
        let folder = TempFolder::new();
        database.add_folder_transport("folder-1", &folder.path).unwrap();

        let engine = ReplicatedSync::new(Arc::new(database));
        let statuses = engine.status().await.unwrap();
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].instance_id, "folder-1");
        assert_eq!(statuses[0].health, "healthy");
        assert_eq!(statuses[0].pending, 0);
        assert!(statuses[0].storage_bytes.is_some());
    }

    #[tokio::test]
    async fn sync_once_is_a_no_op_when_the_feature_is_disabled() {
        // `enabled()` reads THREESTRANDS_REPLICATED_SYNC, which is unset in
        // the test environment, so this never touches the OS keychain
        // (`local_replicated_keys` is only reached past that gate).
        assert!(!enabled());
        let database = Database::open_memory();
        let engine = ReplicatedSync::new(Arc::new(database));
        engine.sync_once().await.unwrap();
    }
}

#[cfg(test)]
mod reconciliation_tests {
    use super::*;

    fn count(database: &Database, sql: &str) -> i64 {
        database.connection().unwrap().query_row(sql, [], |row| row.get(0)).unwrap()
    }

    /// Fills `database` with a group as a protocol-1 build left it (in the
    /// columns the upgrade reads), marks the schema as the version before
    /// the upgrade, and runs the migration again.
    fn upgrade_from_a_protocol_1_group(database: &Database, enrolled: bool) {
        database
            .connection()
            .unwrap()
            .execute_batch(
                "INSERT INTO sync_events(event_id,epoch,device_id,device_sequence,lamport,state,created_at)
                   VALUES ('e1',3,'d1',1,1,'sealed','2026-09-01T00:00:00Z');
                 INSERT INTO sync_operations(operation_id,event_id,entity_type,entity_id,field,value,winner_stamp)
                   VALUES ('o1','e1','snippet','s1','name','\"n\"',X'00');
                 INSERT INTO sync_field_frontier VALUES ('snippet','s1','name','o1');
                 INSERT INTO sync_objects(cid,event_id,object_kind,chunk_index,chunk_count,bytes) VALUES ('c1','e1','operations',0,1,X'00');
                 INSERT INTO sync_devices(device_id,status,is_self) VALUES ('d1','active',1);
                 INSERT INTO sync_device_labels VALUES ('d1','Laptop');",
            )
            .unwrap();
        if enrolled {
            database
                .connection()
                .unwrap()
                .execute_batch(
                    "INSERT INTO sync_epoch_history VALUES (3,'2026-09-01T00:00:00Z','cid');
                     UPDATE sync_spaces SET active_epoch=3;",
                )
                .unwrap();
        }
        let mut connection = database.connection().unwrap();
        connection.pragma_update(None, "user_version", 34).unwrap();
        crate::schema::migrate(&mut connection).unwrap();
    }

    #[test]
    fn upgrading_leaves_a_protocol_1_group_but_keeps_local_data_connectors_and_the_beta() {
        let database = Database::open_memory();
        database.set_beta_features_enabled(true).unwrap();
        let snippet = database.create_snippet("Signature", "Best, Alex").unwrap();
        database.add_folder_transport("folder-1", &std::env::temp_dir()).unwrap();
        upgrade_from_a_protocol_1_group(&database, true);

        for table in ["sync_events", "sync_operations", "sync_field_frontier", "sync_objects", "sync_devices", "sync_device_labels", "sync_epoch_history"] {
            assert_eq!(count(&database, &format!("SELECT COUNT(*) FROM {table}")), 0, "{table} should be empty");
        }
        assert_eq!(count(&database, "SELECT active_epoch FROM sync_spaces"), 0);
        assert!(database.beta_features_enabled().unwrap());
        assert_eq!(database.configured_transports().unwrap().len(), 1);
        assert_eq!(count(&database, "SELECT COUNT(*) FROM snippets"), 1);
        assert_eq!(database.pending_keychain_cleanup().unwrap(), Some(3));

        // The user is told once, and can dismiss it.
        assert!(database.protocol_reset_notice().unwrap());
        database.dismiss_protocol_reset_notice().unwrap();
        assert!(!database.protocol_reset_notice().unwrap());

        // The next sync re-records local data for the next group.
        assert!(database.reconcile_replicated_sync_backlog().unwrap() >= 1);
        let recorded: i64 = database
            .connection()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM sync_operations WHERE entity_id=?1 AND field='_entity'", [&snippet.id], |row| row.get(0))
            .unwrap();
        assert_eq!(recorded, 1);
    }

    #[test]
    fn upgrading_a_device_that_never_joined_a_group_says_nothing() {
        let database = Database::open_memory();
        upgrade_from_a_protocol_1_group(&database, false);
        assert!(!database.protocol_reset_notice().unwrap());
        assert_eq!(database.pending_keychain_cleanup().unwrap(), None);
        assert_eq!(count(&database, "SELECT COUNT(*) FROM sync_events"), 0);
    }

    #[test]
    fn an_operation_belongs_to_exactly_one_event_or_snapshot() {
        let database = Database::open_memory();
        let insert = |event: Option<&str>, snapshot: Option<&str>| {
            database.connection().unwrap().execute(
                "INSERT INTO sync_operations(operation_id,event_id,snapshot_id,entity_type,entity_id,field,winner_stamp)
                 VALUES (lower(hex(randomblob(16))),?1,?2,'snippet','s1','name',X'00')",
                params![event, snapshot],
            )
        };
        assert!(insert(Some("e1"), None).is_ok());
        assert!(insert(None, Some("snap-1")).is_ok());
        assert!(insert(None, None).is_err());
        assert!(insert(Some("e1"), Some("snap-1")).is_err());
        // Removing an event row never removes its operations.
        database
            .connection()
            .unwrap()
            .execute_batch(
                "INSERT INTO sync_events(event_id,epoch,device_id,device_sequence,lamport,state,created_at) VALUES ('e1',0,'d1',1,1,'sealed','x');
                 DELETE FROM sync_events WHERE event_id='e1';",
            )
            .unwrap();
        assert_eq!(count(&database, "SELECT COUNT(*) FROM sync_operations WHERE event_id='e1'"), 1);
    }

    #[test]
    fn catches_up_an_entity_created_without_being_enqueued() {
        let database = Database::open_memory();
        // Simulates the crash window: the app-table write happened, but the
        // enqueue call that should follow it never ran.
        let snippet = database.create_snippet("Signature", "Best, Alex").unwrap();
        let connection = database.connection().unwrap();
        let before: i64 = connection
            .query_row("SELECT COUNT(*) FROM sync_operations WHERE entity_id=?1", [&snippet.id], |row| row.get(0))
            .unwrap();
        assert_eq!(before, 0);
        drop(connection);

        let repaired = database.reconcile_replicated_sync_backlog().unwrap();
        assert!(repaired >= 1);

        let connection = database.connection().unwrap();
        let existence: String = connection
            .query_row(
                "SELECT value FROM sync_operations WHERE entity_id=?1 AND field='_entity'",
                [&snippet.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(existence, "true");
        let name: String = connection
            .query_row(
                "SELECT value FROM sync_operations WHERE entity_id=?1 AND field='name'",
                [&snippet.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(name, "\"Signature\"");
    }

    #[test]
    fn is_a_no_op_for_an_entity_already_recorded() {
        let database = Database::open_memory();
        let snippet = database.create_snippet("Signature", "Best, Alex").unwrap();
        let payload = serde_json::to_value(&snippet).unwrap();
        let fields: std::collections::BTreeSet<String> =
            payload.as_object().unwrap().keys().cloned().collect();
        database
            .record_replicated_write(EntityType::Snippet, &snippet.id, &fields, &payload)
            .unwrap();

        let connection = database.connection().unwrap();
        let before: i64 = connection
            .query_row("SELECT COUNT(*) FROM sync_operations WHERE entity_id=?1", [&snippet.id], |row| row.get(0))
            .unwrap();
        drop(connection);

        // Only assert the *snippet* is untouched — not that the whole sweep
        // found nothing to do.
        database.reconcile_replicated_sync_backlog().unwrap();

        let connection = database.connection().unwrap();
        let after: i64 = connection
            .query_row("SELECT COUNT(*) FROM sync_operations WHERE entity_id=?1", [&snippet.id], |row| row.get(0))
            .unwrap();
        assert_eq!(before, after, "reconciling an already-recorded entity must not create duplicate operations");
    }

    #[test]
    fn covers_every_locally_enumerable_entity_type_in_one_sweep() {
        let database = Database::open_memory();
        database.create_snippet("Signature", "Best, Alex").unwrap();
        database
            .create_split_inbox("Newsletters", "domain", "news.example.com", "you@example.com")
            .unwrap();
        database.set_retention_days(Some(90)).unwrap();

        let repaired = database.reconcile_replicated_sync_backlog().unwrap();
        // Snippet, split inbox, and the explicitly chosen retention setting.
        assert!(repaired >= 3, "expected at least snippet + split inbox + retention, got {repaired}");

        let second_pass = database.reconcile_replicated_sync_backlog().unwrap();
        assert_eq!(second_pass, 0, "a second sweep over the same state must repair nothing");
    }

    fn retention_operation_count(database: &Database) -> i64 {
        database
            .connection()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM sync_operations WHERE entity_type=?1 AND entity_id='mail'",
                [EntityType::Retention.as_str()],
                |row| row.get(0),
            )
            .unwrap()
    }

    #[test]
    fn does_not_seed_the_unlimited_retention_default() {
        // A freshly joining device has never chosen a retention period.
        // Seeding its unset default would enter the graph as a concurrent
        // "forever" write and conflict with the space's agreed value.
        let database = Database::open_memory();
        assert_eq!(database.retention_days().unwrap(), None);
        database.reconcile_replicated_sync_backlog().unwrap();
        assert_eq!(retention_operation_count(&database), 0);
    }

    #[test]
    fn seeds_an_explicit_retention_choice() {
        let database = Database::open_memory();
        database.set_retention_days(Some(365)).unwrap();
        database.reconcile_replicated_sync_backlog().unwrap();
        assert!(retention_operation_count(&database) > 0);
        let days: String = database
            .connection()
            .unwrap()
            .query_row(
                "SELECT value FROM sync_operations WHERE entity_type=?1 AND entity_id='mail' AND field='days'",
                [EntityType::Retention.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(days, "365");
    }
}

#[cfg(test)]
mod frontier_conflict_tests {
    use super::*;
    use serde_json::json;

    fn fields(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    /// Forces a genuine two-way conflict on `field`: both operations name
    /// the same current frontier as their parent, exactly as two devices
    /// that each branched before seeing the other's write would. This must
    /// go through the general `apply_remote_operation` (explicit parents),
    /// not `apply_field_operation` (which always reads the frontier at
    /// call time, so two sequential calls can never actually collide).
    fn force_conflict(database: &Database, entity_id: &str, field: &str, value_a: Value, value_b: Value) {
        let parent: String = database
            .connection()
            .unwrap()
            .query_row(
                "SELECT operation_id FROM sync_field_frontier WHERE entity_type=?1 AND entity_id=?2 AND field=?3",
                params![EntityType::Snippet.as_str(), entity_id, field],
                |row| row.get(0),
            )
            .unwrap();

        let mut connection = database.connection().unwrap();
        let tx = connection.transaction().unwrap();
        for (sequence, value) in [(1i64, value_a), (2i64, value_b)] {
            let device_id = random_id();
            let event_id = random_id();
            let operation_id = random_id();
            let event_id_hex = encode_id(&event_id);
            // sync_operations.event_id has a foreign key into sync_events,
            // so a fake remote event needs a (fake but present) row there
            // too, exactly as a real pulled event would have inserted one.
            tx.execute(
                "INSERT INTO sync_events(event_id,epoch,device_id,device_sequence,lamport,state,created_at)
                 VALUES (?1,0,?2,?3,10,'sealed',?4)",
                params![event_id_hex, encode_id(&device_id), sequence, Utc::now().to_rfc3339()],
            )
            .unwrap();
            let stamp = WinnerStamp {
                lamport: 10,
                device_id,
                event_id,
                operation_id,
            };
            Database::apply_remote_operation(
                &tx,
                EntityType::Snippet,
                entity_id,
                field,
                Some(&value),
                &event_id_hex,
                &encode_id(&operation_id),
                std::slice::from_ref(&parent),
                &stamp,
            )
            .unwrap();
        }
        tx.commit().unwrap();
    }

    #[test]
    fn lists_a_genuine_concurrent_write_as_a_conflict() {
        let database = Database::open_memory();
        database
            .record_replicated_write(
                EntityType::Snippet,
                "s1",
                &fields(&["id", "name", "body", "createdAt"]),
                &json!({"id": "s1", "name": "original", "body": "b", "createdAt": "2026-01-01T00:00:00Z"}),
            )
            .unwrap();

        force_conflict(&database, "s1", "name", json!("From A"), json!("From B"));

        let conflicts = database.list_frontier_conflicts().unwrap();
        let conflict = conflicts
            .iter()
            .find(|conflict| conflict.entity_id == "s1" && conflict.field == "name")
            .expect("the forced conflict should be listed");
        assert_eq!(conflict.entity_type, "snippet");
        assert_eq!(conflict.candidates.len(), 2);
        let values: std::collections::HashSet<_> = conflict.candidates.iter().map(|c| c.value.clone()).collect();
        assert!(values.contains(&Some(json!("From A"))));
        assert!(values.contains(&Some(json!("From B"))));

        // Every other field (untouched by the forced conflict) must not be
        // reported.
        assert!(!conflicts.iter().any(|c| c.field == "body"));
    }

    #[test]
    fn does_not_list_concurrent_writes_that_agree_on_the_value() {
        let database = Database::open_memory();
        database
            .record_replicated_write(
                EntityType::Snippet,
                "s1",
                &fields(&["id", "name", "body", "createdAt"]),
                &json!({"id": "s1", "name": "original", "body": "b", "createdAt": "2026-01-01T00:00:00Z"}),
            )
            .unwrap();

        force_conflict(&database, "s1", "name", json!("Same"), json!("Same"));

        let frontier_size: i64 = database
            .connection()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM sync_field_frontier WHERE entity_type='snippet' AND entity_id='s1' AND field='name'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(frontier_size, 2, "both concurrent writes stay in the frontier");
        assert!(!database
            .list_frontier_conflicts()
            .unwrap()
            .iter()
            .any(|conflict| conflict.entity_id == "s1" && conflict.field == "name"));
    }

    #[test]
    fn resolving_names_the_whole_frontier_as_parents_and_collapses_it() {
        let database = Database::open_memory();
        database
            .record_replicated_write(
                EntityType::Snippet,
                "s1",
                &fields(&["id", "name", "body", "createdAt"]),
                &json!({"id": "s1", "name": "original", "body": "b", "createdAt": "2026-01-01T00:00:00Z"}),
            )
            .unwrap();
        force_conflict(&database, "s1", "name", json!("From A"), json!("From B"));

        let conflict = database
            .list_frontier_conflicts()
            .unwrap()
            .into_iter()
            .find(|conflict| conflict.entity_id == "s1" && conflict.field == "name")
            .unwrap();
        let chosen = conflict
            .candidates
            .iter()
            .find(|candidate| candidate.value == Some(json!("From B")))
            .unwrap();
        let losing_operation_id = conflict
            .candidates
            .iter()
            .find(|candidate| candidate.value == Some(json!("From A")))
            .unwrap()
            .operation_id
            .clone();

        database
            .resolve_frontier_conflict(EntityType::Snippet, "s1", "name", &chosen.operation_id)
            .unwrap();

        // The conflict is gone: exactly one frontier member remains.
        let remaining = database.list_frontier_conflicts().unwrap();
        assert!(!remaining.iter().any(|c| c.entity_id == "s1" && c.field == "name"));

        let resolution_operation_id: String = database
            .connection()
            .unwrap()
            .query_row(
                "SELECT operation_id FROM sync_field_frontier WHERE entity_type='snippet' AND entity_id='s1' AND field='name'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        // The new operation's parents are the entire prior frontier — both
        // the chosen and the losing candidate — not just the chosen one.
        let parent_count: i64 = database
            .connection()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM sync_operation_parents WHERE operation_id=?1",
                params![resolution_operation_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(parent_count, 2);
        let has_losing_parent: bool = database
            .connection()
            .unwrap()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_operation_parents WHERE operation_id=?1 AND parent_operation_id=?2)",
                params![resolution_operation_id, losing_operation_id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(has_losing_parent);
    }
}
