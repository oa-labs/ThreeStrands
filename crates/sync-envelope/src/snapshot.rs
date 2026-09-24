//! Replica snapshots: the one object each device publishes to replicate.
//!
//! A snapshot is a device's whole replica state (see
//! `threestrands_sync_core::state`): its causal context and every field's
//! surviving values. Each device keeps replacing its own snapshot; readers
//! merge the latest snapshot of every device. It is sealed like any other
//! object, as [`ObjectKind::Snapshot`], so it is chunked, padded,
//! encrypted, and signed by its author under the snapshot signature domain.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::EnvelopeError;
use crate::header::{MESSAGE_ID_LEN, NONCE_LEN};
use crate::ids::DeviceId;
use crate::limits;
use crate::vector::{validate_sequence_vector, SequenceEntry};
use crate::{crypto, open_object, seal_object, EntityType, ObjectKind, OpenParams, SealParams, SealedMessage};

/// The only sync protocol version this crate seals or opens. Version 3
/// replaced the per-device event log with replica snapshots; earlier
/// versions' groups can't be read and must be recreated.
pub const PROTOCOL_VERSION: u16 = 3;

/// One device's replica state at one moment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReplicaSnapshot {
    pub protocol_version: u16,
    pub device_id: DeviceId,
    /// Increases with every snapshot this device publishes, so a reader can
    /// tell a newer snapshot from one it already merged.
    pub state_sequence: u64,
    /// Display only; never used for ordering.
    pub created_at_ms: i64,
    /// For each device, the highest write counter this replica has seen.
    pub context: Vec<SequenceEntry>,
    /// Every field holding at least one value, in ascending
    /// `(entity_type, entity_id, field)` order.
    pub fields: Vec<SnapshotField>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SnapshotField {
    pub entity_type: EntityType,
    pub entity_id: String,
    pub field: String,
    /// Surviving values in ascending `(device_id, counter)` order: one,
    /// or several while the field is in conflict.
    pub values: Vec<SnapshotValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SnapshotValue {
    pub device_id: DeviceId,
    pub counter: u64,
    pub lamport: u64,
    pub value: Option<Value>,
}

fn field_order(left: &SnapshotField, right: &SnapshotField) -> Ordering {
    (left.entity_type, &left.entity_id, &left.field).cmp(&(right.entity_type, &right.entity_id, &right.field))
}

fn value_order(left: &SnapshotValue, right: &SnapshotValue) -> Ordering {
    (left.device_id, left.counter).cmp(&(right.device_id, right.counter))
}

impl ReplicaSnapshot {
    /// Puts fields and values in canonical order, so one state always has
    /// exactly one encoding (and so one signature and one CID).
    pub fn canonicalize(&mut self) {
        self.context.sort_by_key(|entry| entry.device_id);
        for field in &mut self.fields {
            field.values.sort_by(value_order);
        }
        self.fields.sort_by(field_order);
    }
}

/// Checks every rule a snapshot must meet, at both seal and open time:
/// protocol version, canonical order with no duplicates, every value's
/// write inside the context, and every hard limit.
pub fn validate_snapshot(snapshot: &ReplicaSnapshot) -> Result<(), EnvelopeError> {
    if snapshot.protocol_version != PROTOCOL_VERSION {
        return Err(EnvelopeError::UnsupportedProtocolVersion);
    }
    validate_sequence_vector(&snapshot.context)?;
    if snapshot.fields.len() > limits::MAX_SNAPSHOT_FIELDS {
        return Err(EnvelopeError::LimitExceeded("snapshot field count"));
    }
    let seen = |device_id: &DeviceId| -> u64 {
        snapshot
            .context
            .binary_search_by_key(device_id, |entry| entry.device_id)
            .map(|index| snapshot.context[index].sequence)
            .unwrap_or(0)
    };
    for (index, field) in snapshot.fields.iter().enumerate() {
        if index > 0 && field_order(&snapshot.fields[index - 1], field) != Ordering::Less {
            return Err(EnvelopeError::LimitExceeded("snapshot field order"));
        }
        if field.entity_id.is_empty() || field.entity_id.len() > limits::MAX_ENTITY_ID_BYTES {
            return Err(EnvelopeError::LimitExceeded("entity id length"));
        }
        if field.field.is_empty() || field.field.len() > limits::MAX_FIELD_NAME_BYTES {
            return Err(EnvelopeError::LimitExceeded("field name length"));
        }
        if field.values.is_empty() || field.values.len() > limits::MAX_VALUES_PER_FIELD {
            return Err(EnvelopeError::LimitExceeded("values per field"));
        }
        for (position, value) in field.values.iter().enumerate() {
            if position > 0 && value_order(&field.values[position - 1], value) != Ordering::Less {
                return Err(EnvelopeError::LimitExceeded("snapshot value order"));
            }
            if value.counter == 0 || value.counter > seen(&value.device_id) {
                return Err(EnvelopeError::LimitExceeded("value outside context"));
            }
            if let Some(json) = &value.value {
                let size = serde_json::to_vec(json).map(|bytes| bytes.len()).unwrap_or(usize::MAX);
                if size > limits::MAX_VALUE_BYTES {
                    return Err(EnvelopeError::LimitExceeded("value size"));
                }
            }
        }
    }
    Ok(())
}

/// A snapshot's stable message id, from its author and sequence alone, so
/// resealing the same snapshot reproduces the same header.
pub fn message_id_for_snapshot(device_id: &DeviceId, state_sequence: u64) -> [u8; MESSAGE_ID_LEN] {
    use sha2::{Digest, Sha256};
    const DOMAIN: &[u8] = b"threestrands/sync-envelope/snapshot-message-id/v3";
    let mut hasher = Sha256::new();
    hasher.update(DOMAIN);
    hasher.update(device_id.as_bytes());
    hasher.update(state_sequence.to_be_bytes());
    let digest = hasher.finalize();
    let mut id = [0u8; MESSAGE_ID_LEN];
    id.copy_from_slice(&digest[..MESSAGE_ID_LEN]);
    id
}

/// Canonicalizes, validates, signs, and seals `snapshot`, drawing chunk
/// nonces from `next_nonce` in order.
pub fn seal_snapshot_with_nonces<F>(
    mut snapshot: ReplicaSnapshot,
    params: &SealParams,
    next_nonce: F,
) -> Result<SealedMessage, EnvelopeError>
where
    F: FnMut() -> [u8; NONCE_LEN],
{
    if params.object_kind != ObjectKind::Snapshot {
        return Err(EnvelopeError::UnexpectedObjectKind);
    }
    snapshot.canonicalize();
    validate_snapshot(&snapshot)?;
    let message_id = message_id_for_snapshot(&snapshot.device_id, snapshot.state_sequence);
    let (_, sealed) = seal_object(&snapshot, message_id, params, next_nonce)?;
    Ok(sealed)
}

/// Seals `snapshot` using fresh random nonces.
pub fn seal_snapshot(snapshot: ReplicaSnapshot, params: &SealParams) -> Result<SealedMessage, EnvelopeError> {
    seal_snapshot_with_nonces(snapshot, params, crypto::random_nonce)
}

/// Opens one snapshot's complete chunk set (in any order): everything
/// [`open_object`] checks, plus the snapshot's own contract and that its
/// message id is the one derived from its author and sequence.
pub fn open_snapshot(chunks: &[Vec<u8>], params: &OpenParams) -> Result<ReplicaSnapshot, EnvelopeError> {
    let (snapshot, _, message_id) = open_object::<ReplicaSnapshot>(chunks, params, ObjectKind::Snapshot)?;
    if message_id != message_id_for_snapshot(&snapshot.device_id, snapshot.state_sequence) {
        return Err(EnvelopeError::Malformed);
    }
    validate_snapshot(&snapshot)?;
    Ok(snapshot)
}
