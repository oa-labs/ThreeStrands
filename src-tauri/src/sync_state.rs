//! This device's replica state, stored in SQLite: the local side of
//! state-based replicated sync (see `threestrands_sync_core::state`).
//!
//! - `sync_values`: every surviving value, one row per field per write that
//!   still holds it (several rows while a field is in conflict).
//! - `sync_context`: the causal context, the highest write counter seen from
//!   each device.
//! - `sync_local_state`: whether the state changed since this device last
//!   sealed a snapshot of it, and that snapshot's sequence.
//!
//! Local writes go straight to SQL. Merging a peer's snapshot loads the
//! whole state, merges it with `ReplicaState::merge`, and writes back only
//! the fields that changed, all in one transaction so a concurrent local
//! write can't be lost. Synchronized data is small (tasks, snippets,
//! preferences, account metadata), so loading it whole is cheap.

use std::collections::{BTreeMap, BTreeSet};

use chrono::Utc;
use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::Value;
use threestrands_sync_core::{Dot, EntityType, FieldKey, ReplicaState, StateValue, ENTITY_EXISTENCE_FIELD};
use threestrands_sync_envelope::{
    DeviceId as EnvelopeDeviceId, ReplicaSnapshot, SequenceEntry, SnapshotField, SnapshotValue, PROTOCOL_VERSION,
};

use crate::db::{Database, DbResult};
use crate::error_text::display;
use crate::replicated_sync::{decode_id, encode_id, ensure_space_and_device, FrontierConflict, FrontierConflictCandidate};

/// Records that the replica changed, so the next push seals a new snapshot.
/// `local` is true for a write made on this device, which is also what the
/// device list shows as this device's latest change.
fn mark_changed(tx: &Transaction, local: bool) -> DbResult<()> {
    tx.execute("INSERT OR IGNORE INTO sync_local_state(id) VALUES (1)", [])?;
    tx.execute("UPDATE sync_local_state SET dirty=1, generation=generation+1 WHERE id=1", [])?;
    if local {
        tx.execute("UPDATE sync_local_state SET last_change_at=?1 WHERE id=1", params![Utc::now().to_rfc3339()])?;
    }
    Ok(())
}

/// Marks the replica clean only if it is still the generation captured by
/// the snapshot whose sealed objects are being stored.
pub(crate) fn clear_dirty_if_generation(tx: &Transaction, generation: i64) -> DbResult<()> {
    tx.execute(
        "UPDATE sync_local_state SET dirty=0 WHERE id=1 AND generation=?1",
        params![generation],
    )?;
    Ok(())
}

fn entity_has_values(tx: &Transaction, entity_type: EntityType, entity_id: &str) -> DbResult<bool> {
    Ok(tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_values WHERE entity_type=?1 AND entity_id=?2)",
        params![entity_type.as_str(), entity_id],
        |row| row.get(0),
    )?)
}

fn store_value(tx: &Transaction, key: &FieldKey, value: &StateValue) -> DbResult<()> {
    tx.execute(
        "INSERT INTO sync_values(entity_type, entity_id, field, device_id, counter, lamport, value) VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![
            key.entity_type.as_str(),
            key.entity_id,
            key.field,
            encode_id(&value.dot.device_id),
            value.dot.counter as i64,
            value.lamport as i64,
            value.value.as_ref().map(Value::to_string),
        ],
    )?;
    Ok(())
}

fn clear_field(tx: &Transaction, key: &FieldKey) -> DbResult<()> {
    tx.execute(
        "DELETE FROM sync_values WHERE entity_type=?1 AND entity_id=?2 AND field=?3",
        params![key.entity_type.as_str(), key.entity_id, key.field],
    )?;
    Ok(())
}

/// One `sync_values` row: entity type, entity id, field, device, counter,
/// lamport, and the JSON value.
type ValueRow = (String, String, String, String, i64, i64, Option<String>);

fn load_state(tx: &Transaction) -> DbResult<ReplicaState> {
    let context: BTreeMap<[u8; 16], u64> = {
        let mut statement = tx.prepare("SELECT device_id, counter FROM sync_context")?;
        let rows = statement
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(device, counter)| Ok((decode_id(&device)?, counter as u64)))
            .collect::<Result<_, String>>()?
    };
    let mut fields: BTreeMap<FieldKey, Vec<StateValue>> = BTreeMap::new();
    let rows: Vec<ValueRow> = {
        let mut statement =
            tx.prepare("SELECT entity_type, entity_id, field, device_id, counter, lamport, value FROM sync_values")?;
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    for (entity_type, entity_id, field, device, counter, lamport, value) in rows {
        let key = FieldKey { entity_type: entity_type.parse::<EntityType>()?, entity_id, field };
        fields.entry(key).or_default().push(StateValue {
            dot: Dot { device_id: decode_id(&device)?, counter: counter as u64 },
            lamport: lamport as u64,
            value: value.map(|json| serde_json::from_str(&json)).transpose().map_err(display)?,
        });
    }
    ReplicaState::from_parts(context, fields).map_err(|error| format!("The local sync state is inconsistent: {error:?}").into())
}

/// Turns a peer's opened snapshot into a mergeable state, refusing one that
/// isn't well formed.
pub(crate) fn snapshot_to_state(snapshot: &ReplicaSnapshot) -> Result<ReplicaState, String> {
    let context = snapshot.context.iter().map(|entry| (*entry.device_id.as_bytes(), entry.sequence)).collect();
    let fields = snapshot.fields.iter().map(|field| {
        let key = FieldKey { entity_type: field.entity_type, entity_id: field.entity_id.clone(), field: field.field.clone() };
        let values = field
            .values
            .iter()
            .map(|value| StateValue {
                dot: Dot { device_id: *value.device_id.as_bytes(), counter: value.counter },
                lamport: value.lamport,
                value: value.value.clone(),
            })
            .collect();
        (key, values)
    });
    ReplicaState::from_parts(context, fields).map_err(|error| format!("A peer's snapshot is malformed: {error:?}"))
}

impl Database {
    /// Records a local write (creation or update) into the replica, in one
    /// transaction: one new write whose value replaces each named field's
    /// values. Creating an entity (no values yet) also writes `_entity =
    /// true`. A no-op while a remote projection is being applied (see
    /// [`Self::with_remote_projection`]), so materializing a merged value
    /// never echoes back as a new local write.
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
            let device_hex = encode_id(&device_id);
            let creating = !entity_has_values(tx, entity_type, entity_id)?;
            tx.execute("INSERT OR IGNORE INTO sync_context(device_id, counter) VALUES (?1, 0)", params![device_hex])?;
            let counter: i64 = tx.query_row(
                "UPDATE sync_context SET counter = counter + 1 WHERE device_id=?1 RETURNING counter",
                params![device_hex],
                |row| row.get(0),
            )?;
            let lamport: i64 = tx.query_row(
                "UPDATE sync_spaces SET lamport = lamport + 1 WHERE id=?1 RETURNING lamport",
                params![crate::replicated_sync::SPACE_ID],
                |row| row.get(0),
            )?;
            let dot = Dot { device_id, counter: counter as u64 };
            let mut values: Vec<(String, Option<Value>)> = Vec::new();
            if creating {
                values.push((ENTITY_EXISTENCE_FIELD.to_string(), Some(Value::Bool(true))));
            }
            for field in fields.iter().filter(|field| *field != "*") {
                values.push((field.clone(), payload.get(field).cloned()));
            }
            for (field, value) in values {
                let key = FieldKey { entity_type, entity_id: entity_id.to_string(), field };
                clear_field(tx, &key)?;
                store_value(tx, &key, &StateValue { dot, lamport: lamport as u64, value })?;
            }
            mark_changed(tx, true)
        })
        .map_err(String::from)
    }

    /// Records a local deletion: every value of the entity is removed. No
    /// tombstone is kept; the causal context is what stops a peer's older
    /// copy from bringing it back.
    pub fn record_replicated_deletion(&self, entity_type: EntityType, entity_id: &str) -> Result<(), String> {
        if self.is_projecting_remote_operation() {
            return Ok(());
        }
        self.with_transaction(|tx| {
            ensure_space_and_device(tx)?;
            tx.execute(
                "DELETE FROM sync_values WHERE entity_type=?1 AND entity_id=?2",
                params![entity_type.as_str(), entity_id],
            )?;
            mark_changed(tx, true)
        })
        .map_err(String::from)
    }

    /// True once the replica holds any value for this entity.
    pub(crate) fn entity_recorded(&self, entity_type: EntityType, entity_id: &str) -> DbResult<bool> {
        self.with_transaction(|tx| entity_has_values(tx, entity_type, entity_id))
    }

    /// The whole local replica state.
    #[cfg(test)]
    pub(crate) fn load_replica_state(&self) -> DbResult<ReplicaState> {
        self.with_transaction(load_state)
    }

    /// Merges a peer's state into this replica and returns every entity
    /// whose values changed, for materializing. Lamport time moves past
    /// every merged value, so this device's next write outranks what it has
    /// seen.
    pub(crate) fn merge_replica_state(&self, remote: &ReplicaState) -> DbResult<Vec<(EntityType, String)>> {
        self.with_transaction(|tx| {
            let mut local = load_state(tx)?;
            let context_before = local.context().clone();
            let changed = local.merge(remote);
            for key in &changed {
                clear_field(tx, key)?;
                for value in local.values(key) {
                    store_value(tx, key, value)?;
                }
            }
            if local.context() != &context_before {
                for (device, counter) in local.context() {
                    tx.execute(
                        "INSERT INTO sync_context(device_id, counter) VALUES (?1, ?2)
                         ON CONFLICT(device_id) DO UPDATE SET counter = MAX(counter, excluded.counter)",
                        params![encode_id(device), *counter as i64],
                    )?;
                }
            }
            let newest_lamport = remote.fields().values().flatten().map(|value| value.lamport).max().unwrap_or(0);
            tx.execute(
                "UPDATE sync_spaces SET lamport = MAX(lamport, ?2) WHERE id=?1",
                params![crate::replicated_sync::SPACE_ID, newest_lamport as i64],
            )?;
            if !changed.is_empty() || local.context() != &context_before {
                mark_changed(tx, false)?;
            }
            let mut touched: Vec<(EntityType, String)> =
                changed.into_iter().map(|key| (key.entity_type, key.entity_id)).collect();
            touched.dedup();
            Ok(touched)
        })
    }

    /// Takes a snapshot and records the replica generation it represents.
    /// Dirty is cleared only after the sealed objects are stored, and only
    /// when no write has advanced this generation in the meantime.
    pub(crate) fn take_local_snapshot(
        &self,
        device_id: EnvelopeDeviceId,
        state_sequence: u64,
        created_at_ms: i64,
    ) -> DbResult<(ReplicaSnapshot, i64)> {
        self.with_transaction(|tx| {
            tx.execute("INSERT OR IGNORE INTO sync_local_state(id) VALUES (1)", [])?;
            let state = load_state(tx)?;
            let generation = tx.query_row("SELECT generation FROM sync_local_state WHERE id=1", [], |row| row.get(0))?;
            Ok((snapshot_of(&state, device_id, state_sequence, created_at_ms), generation))
        })
    }

    pub(crate) fn mark_replica_changed(&self) -> DbResult<()> {
        self.with_transaction(|tx| mark_changed(tx, false))
    }
}

fn snapshot_of(state: &ReplicaState, device_id: EnvelopeDeviceId, state_sequence: u64, created_at_ms: i64) -> ReplicaSnapshot {
        ReplicaSnapshot {
            protocol_version: PROTOCOL_VERSION,
            device_id,
            state_sequence,
            created_at_ms,
            context: state
                .context()
                .iter()
                .filter(|(_, counter)| **counter > 0)
                .map(|(device, counter)| SequenceEntry { device_id: EnvelopeDeviceId::from_bytes(*device), sequence: *counter })
                .collect(),
            fields: state
                .fields()
                .iter()
                .map(|(key, values)| SnapshotField {
                    entity_type: key.entity_type,
                    entity_id: key.entity_id.clone(),
                    field: key.field.clone(),
                    values: values
                        .iter()
                        .map(|value| SnapshotValue {
                            device_id: EnvelopeDeviceId::from_bytes(value.dot.device_id),
                            counter: value.dot.counter,
                            lamport: value.lamport,
                            value: value.value.clone(),
                        })
                        .collect(),
                })
                .collect(),
        }
}

impl Database {
    /// The fields that currently hold any value for an entity.
    pub(crate) fn known_fields(&self, entity_type: EntityType, entity_id: &str) -> DbResult<Vec<String>> {
        self.with_connection(|connection| {
            let mut statement =
                connection.prepare("SELECT DISTINCT field FROM sync_values WHERE entity_type=?1 AND entity_id=?2")?;
            let rows = statement
                .query_map(params![entity_type.as_str(), entity_id], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    /// A field's working value: the surviving value with the greatest
    /// `(lamport, device, counter)`. Lowercase hex of equal-length device
    /// ids orders exactly like the bytes, so SQL can rank them directly.
    pub(crate) fn resolve_field_winner(&self, entity_type: EntityType, entity_id: &str, field: &str) -> DbResult<Option<Value>> {
        let winner: Option<Option<String>> = self.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT value FROM sync_values WHERE entity_type=?1 AND entity_id=?2 AND field=?3
                     ORDER BY lamport DESC, device_id DESC, counter DESC LIMIT 1",
                    params![entity_type.as_str(), entity_id, field],
                    |row| row.get(0),
                )
                .optional()?)
        })?;
        Ok(winner.flatten().map(|json| serde_json::from_str(&json)).transpose().map_err(display)?)
    }

    /// Every field currently in conflict: more than one surviving value, and
    /// the values differ. Concurrent writes that agree on the value leave
    /// nothing to choose between, so they aren't reported; the next write to
    /// the field collapses them as usual. See
    /// [`Self::resolve_frontier_conflict`].
    pub fn list_frontier_conflicts(&self) -> Result<Vec<FrontierConflict>, String> {
        self.with_connection(|connection| {
            let keys: Vec<(String, String, String)> = {
                let mut statement = connection.prepare(
                    "SELECT entity_type, entity_id, field FROM sync_values
                     GROUP BY entity_type, entity_id, field HAVING COUNT(*) > 1",
                )?;
                let rows = statement
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                    .collect::<Result<Vec<_>, _>>()?;
                rows
            };
            let mut conflicts = Vec::with_capacity(keys.len());
            for (entity_type, entity_id, field) in keys {
                let candidates: Vec<(String, i64, Option<String>)> = {
                    let mut statement = connection.prepare(
                        "SELECT device_id, counter, value FROM sync_values WHERE entity_type=?1 AND entity_id=?2 AND field=?3
                         ORDER BY device_id, counter",
                    )?;
                    let rows = statement
                        .query_map(params![entity_type, entity_id, field], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                        .collect::<Result<Vec<_>, _>>()?;
                    rows
                };
                let mut resolved = Vec::with_capacity(candidates.len());
                for (device_id, counter, value) in candidates {
                    resolved.push(FrontierConflictCandidate {
                        operation_id: format!("{device_id}:{counter}"),
                        device_id,
                        value: value.map(|json| serde_json::from_str(&json)).transpose().map_err(display)?,
                    });
                }
                if resolved.iter().all(|candidate| candidate.value == resolved[0].value) {
                    continue;
                }
                conflicts.push(FrontierConflict { entity_type, entity_id, field, candidates: resolved });
            }
            Ok(conflicts)
        })
        .map_err(String::from)
    }

    /// Resolves a field conflict: an ordinary local write of the chosen
    /// candidate's value, which replaces every value the field holds.
    /// `chosen_operation_id` is a candidate's `operation_id`
    /// (`"<device>:<counter>"`) from [`Self::list_frontier_conflicts`].
    pub fn resolve_frontier_conflict(
        &self,
        entity_type: EntityType,
        entity_id: &str,
        field: &str,
        chosen_operation_id: &str,
    ) -> Result<(), String> {
        let (device_id, counter) = chosen_operation_id
            .split_once(':')
            .and_then(|(device, counter)| Some((device.to_string(), counter.parse::<i64>().ok()?)))
            .ok_or_else(|| "That conflict choice is invalid".to_string())?;
        let stored: Option<String> = self
            .with_connection(|connection| {
                Ok(connection.query_row(
                    "SELECT value FROM sync_values WHERE entity_type=?1 AND entity_id=?2 AND field=?3 AND device_id=?4 AND counter=?5",
                    params![entity_type.as_str(), entity_id, field, device_id, counter],
                    |row| row.get(0),
                )?)
            })
            .map_err(|_| "That conflict was already resolved elsewhere".to_string())?;
        let mut payload = serde_json::Map::new();
        if let Some(json) = stored {
            payload.insert(field.to_string(), serde_json::from_str(&json).map_err(display)?);
        }
        self.record_replicated_write(entity_type, entity_id, &BTreeSet::from([field.to_string()]), &Value::Object(payload))
    }

    /// Whether the replica changed since the last sealed snapshot, that
    /// snapshot's sequence, and the epoch it was sealed under.
    pub(crate) fn local_state_status(&self) -> DbResult<(bool, u64, Option<u32>)> {
        self.with_transaction(|tx| {
            tx.execute("INSERT OR IGNORE INTO sync_local_state(id) VALUES (1)", [])?;
            Ok(tx.query_row("SELECT dirty, state_sequence, sealed_epoch FROM sync_local_state WHERE id=1", [], |row| {
                Ok((row.get::<_, bool>(0)?, row.get::<_, i64>(1)? as u64, row.get(2)?))
            })?)
        })
    }
}
