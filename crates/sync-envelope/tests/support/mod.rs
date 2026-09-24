use serde_json::json;
use threestrands_sync_envelope::{EntityType, ReplicaSnapshot, SnapshotField, SnapshotValue};

/// Deterministic pseudo-random (effectively incompressible) ASCII text of
/// exactly `len` bytes, seeded so repeated calls with the same seed produce
/// the same text.
pub fn incompressible_text(seed: u64, len: usize) -> String {
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            char::from(b'a' + (state % 26) as u8)
        })
        .collect()
}

/// Appends enough fields to `snapshot` to force multi-chunk sealing, while
/// keeping every individual value comfortably under the per-value hard size
/// limit. Every value is the snapshot author's first write, so the fixture's
/// context must already cover it.
pub fn add_bulk_fields(snapshot: &mut ReplicaSnapshot, field_count: u8, bytes_per_value: usize) {
    add_bulk_fields_seeded(snapshot, field_count, bytes_per_value, 0);
}

/// Like [`add_bulk_fields`], but with a caller-chosen seed so two
/// otherwise-identically-shaped snapshots (same field count and size, so
/// the same chunk count) can be given different content.
pub fn add_bulk_fields_seeded(snapshot: &mut ReplicaSnapshot, field_count: u8, bytes_per_value: usize, seed: u64) {
    for index in 0..field_count {
        snapshot.fields.push(SnapshotField {
            entity_type: EntityType::Task,
            entity_id: format!("bulk-entity-{index:03}"),
            field: "notes".to_string(),
            values: vec![SnapshotValue {
                device_id: snapshot.device_id,
                counter: 1,
                lamport: 1,
                value: Some(json!(incompressible_text(
                    0x9e37_79b9_u64
                        .wrapping_add(index as u64)
                        .wrapping_add(seed.wrapping_mul(0x1000_0000)),
                    bytes_per_value
                ))),
            }],
        });
    }
}
