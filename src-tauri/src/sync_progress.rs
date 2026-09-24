//! Per-device progress through every device's append-only event feed.
//!
//! Two numbers per device, both in `sync_device_progress`:
//!
//! - **applied**: the contiguous prefix `1..=applied` of that device's feed
//!   that has been applied here. A chain walk stops once it reaches it, and
//!   an arriving event at or below it is never applied again — so an
//!   already-covered ancestor can't rejoin a frontier as a false conflict,
//!   even after its operation rows are gone.
//! - **progress**: the largest prefix, no longer than applied, whose events'
//!   dependencies are all covered too (by the other devices' progress). This
//!   is causally closed, and it's what a device publishes as its head's
//!   `ack`.
//!
//! An event's dependencies are summarized by its `causal_vector`: its
//! author's applied vector when sealing it. Each device's applied vector
//! only grows, so along one feed causal vectors only grow, and a prefix
//! `1..=s` is closed exactly when event `s`'s causal vector is covered.
//! Progress is the greatest fixpoint of that rule, found by lowering each
//! device's prefix until it holds everywhere.

use std::collections::HashMap;

use rusqlite::{params, OptionalExtension, Transaction};
use threestrands_sync_envelope::{DeviceHead, DeviceId as EnvelopeDeviceId, SequenceEntry};

use crate::db::{Database, DbResult};
use crate::replicated_sync::{decode_id, encode_id};

/// A sequence vector as stored in SQLite: `[[device_id_hex, sequence], ...]`.
pub(crate) fn encode_vector(entries: &[SequenceEntry]) -> String {
    let pairs: Vec<(String, u64)> =
        entries.iter().map(|entry| (encode_id(entry.device_id.as_bytes()), entry.sequence)).collect();
    serde_json::to_string(&pairs).expect("a list of strings and numbers always serializes")
}

fn decode_vector(json: &str) -> DbResult<Vec<(String, u64)>> {
    serde_json::from_str(json).map_err(|error| format!("Stored sequence vector is invalid: {error}").into())
}

/// Builds a canonical wire vector (ascending device id, zero entries
/// omitted) from `(device_id_hex, sequence)` pairs.
fn to_wire_vector(mut pairs: Vec<(String, u64)>) -> DbResult<Vec<SequenceEntry>> {
    pairs.retain(|(_, sequence)| *sequence > 0);
    // Lowercase hex of equal-length ids sorts exactly like the raw bytes.
    pairs.sort();
    pairs
        .into_iter()
        .map(|(device_id, sequence)| {
            Ok(SequenceEntry { device_id: EnvelopeDeviceId::from_bytes(decode_id(&device_id)?), sequence })
        })
        .collect()
}

fn ensure_row(tx: &Transaction, device_id_hex: &str) -> DbResult<()> {
    tx.execute("INSERT OR IGNORE INTO sync_device_progress(device_id) VALUES (?1)", params![device_id_hex])?;
    Ok(())
}

/// How much of `device_id_hex`'s feed is applied here, inside `tx`.
pub(crate) fn applied_sequence_in(tx: &Transaction, device_id_hex: &str) -> DbResult<u64> {
    let applied: Option<i64> = tx
        .query_row(
            "SELECT applied_sequence FROM sync_device_progress WHERE device_id=?1",
            params![device_id_hex],
            |row| row.get(0),
        )
        .optional()?;
    Ok(applied.unwrap_or(0) as u64)
}

/// Records that event `device_sequence` of `device_id_hex` was just applied
/// (a remote event) or sealed (one of this device's own). Refuses to skip
/// ahead: the applied prefix only ever grows by exactly one.
pub(crate) fn note_event_applied(tx: &Transaction, device_id_hex: &str, device_sequence: u64, at: &str) -> DbResult<()> {
    ensure_row(tx, device_id_hex)?;
    let applied = applied_sequence_in(tx, device_id_hex)?;
    if device_sequence != applied + 1 {
        return Err(format!(
            "Event {device_sequence} from device {device_id_hex} doesn't follow the {applied} already applied"
        )
        .into());
    }
    tx.execute(
        "UPDATE sync_device_progress SET applied_sequence=?2, last_event_at=?3 WHERE device_id=?1",
        params![device_id_hex, device_sequence as i64, at],
    )?;
    Ok(())
}

/// The causal vector for an event this device is sealing now: how much of
/// every other device's feed it has applied.
pub(crate) fn causal_vector_for_seal(tx: &Transaction, self_device_id_hex: &str) -> DbResult<Vec<SequenceEntry>> {
    let pairs: Vec<(String, u64)> = {
        let mut statement = tx.prepare(
            "SELECT device_id, applied_sequence FROM sync_device_progress WHERE device_id != ?1 AND applied_sequence > 0",
        )?;
        let rows = statement
            .query_map(params![self_device_id_hex], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64)))?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    to_wire_vector(pairs)
}

impl Database {
    /// How much of `device_id_hex`'s feed is applied here.
    pub(crate) fn applied_sequence(&self, device_id_hex: &str) -> DbResult<u64> {
        self.with_transaction(|tx| applied_sequence_in(tx, device_id_hex))
    }

    /// Recomputes every device's causally closed progress from the applied
    /// prefixes and stored causal vectors, and persists it.
    pub(crate) fn recompute_progress(&self) -> DbResult<()> {
        self.with_transaction(|tx| {
            let applied: Vec<(String, u64)> = {
                let mut statement = tx.prepare("SELECT device_id, applied_sequence FROM sync_device_progress")?;
                let rows = statement
                    .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64)))?
                    .collect::<Result<Vec<_>, _>>()?;
                rows
            };
            let mut progress: HashMap<String, u64> = applied.iter().cloned().collect();
            let mut vectors: HashMap<(String, u64), Vec<(String, u64)>> = HashMap::new();
            loop {
                let mut changed = false;
                for (device_id, _) in &applied {
                    loop {
                        let sequence = progress[device_id];
                        if sequence == 0 {
                            break;
                        }
                        let key = (device_id.clone(), sequence);
                        if !vectors.contains_key(&key) {
                            let json: Option<String> = tx
                                .query_row(
                                    "SELECT causal_vector FROM sync_events WHERE device_id=?1 AND device_sequence=?2",
                                    params![device_id, sequence as i64],
                                    |row| row.get(0),
                                )
                                .optional()?
                                .flatten();
                            // An applied event always has a stored vector;
                            // a missing one is treated as unsatisfiable so
                            // progress can never overstate what's closed.
                            let vector = match json {
                                Some(json) => decode_vector(&json)?,
                                None => vec![(String::new(), u64::MAX)],
                            };
                            vectors.insert(key.clone(), vector);
                        }
                        let covered = vectors[&key]
                            .iter()
                            .all(|(dependency, needed)| progress.get(dependency).copied().unwrap_or(0) >= *needed);
                        if covered {
                            break;
                        }
                        progress.insert(device_id.clone(), sequence - 1);
                        changed = true;
                    }
                }
                if !changed {
                    break;
                }
            }
            for (device_id, sequence) in progress {
                tx.execute(
                    "UPDATE sync_device_progress SET progress_sequence=?2 WHERE device_id=?1",
                    params![device_id, sequence as i64],
                )?;
            }
            Ok(())
        })
    }

    /// This device's ack: its causally closed progress through every feed,
    /// its own included.
    pub(crate) fn progress_vector(&self) -> DbResult<Vec<SequenceEntry>> {
        let pairs: Vec<(String, u64)> = self.with_connection(|connection| {
            let mut statement =
                connection.prepare("SELECT device_id, progress_sequence FROM sync_device_progress WHERE progress_sequence > 0")?;
            let rows = statement
                .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64)))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;
        to_wire_vector(pairs)
    }

    /// Remembers what a peer's latest verified head said and when this
    /// device first saw it. Only a head newer than the one on record (by
    /// its own publication time) replaces it, so a stale copy on a lagging
    /// transport never rolls the record back.
    pub(crate) fn record_head_observation(&self, head: &DeviceHead, seen_at_ms: i64) -> DbResult<()> {
        let device_id_hex = encode_id(head.device_id.as_bytes());
        self.with_transaction(|tx| {
            ensure_row(tx, &device_id_hex)?;
            tx.execute(
                "UPDATE sync_device_progress
                 SET last_head_published_at_ms=?2, last_head_seen_at_ms=?3, ack_json=?4
                 WHERE device_id=?1 AND (last_head_published_at_ms IS NULL OR last_head_published_at_ms < ?2)",
                params![device_id_hex, head.published_at_ms, seen_at_ms, encode_vector(&head.ack)],
            )?;
            Ok(())
        })
    }

    /// Whether `head_content` (a head's content minus its publication time)
    /// should be published to `transport_instance_id` now: it changed since
    /// the last publication there, or the heartbeat is due.
    pub(crate) fn head_publication_due(&self, transport_instance_id: &str, head_content: &str, now_ms: i64) -> DbResult<bool> {
        let last: Option<(String, i64)> = self.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT head_content, published_at_ms FROM sync_head_publications WHERE transport_instance_id=?1",
                    params![transport_instance_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?)
        })?;
        Ok(match last {
            None => true,
            Some((content, published_at_ms)) => {
                content != head_content || now_ms - published_at_ms >= crate::sync_policy::HEAD_HEARTBEAT_MS
            }
        })
    }

    pub(crate) fn record_head_publication(&self, transport_instance_id: &str, head_content: &str, now_ms: i64) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO sync_head_publications(transport_instance_id, head_content, published_at_ms) VALUES (?1,?2,?3)
                 ON CONFLICT(transport_instance_id) DO UPDATE SET head_content=excluded.head_content, published_at_ms=excluded.published_at_ms",
                params![transport_instance_id, head_content, now_ms],
            )?;
            Ok(())
        })
    }
}
