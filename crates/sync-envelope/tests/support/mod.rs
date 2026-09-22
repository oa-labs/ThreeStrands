use serde_json::json;
use threestrands_sync_envelope::{EntityType, FieldOperation, OperationId, UnsignedSyncEvent};

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

/// Appends enough operations to `event` to force multi-chunk sealing, while
/// keeping every individual operation comfortably under the per-operation
/// hard size limit.
pub fn add_bulk_operations(
    event: &mut UnsignedSyncEvent,
    operation_count: u8,
    bytes_per_operation: usize,
) {
    add_bulk_operations_seeded(event, operation_count, bytes_per_operation, 0);
}

/// Like [`add_bulk_operations`], but with a caller-chosen seed so two
/// otherwise-identically-shaped events (same operation count and size, so
/// the same chunk count) can be given different content.
pub fn add_bulk_operations_seeded(
    event: &mut UnsignedSyncEvent,
    operation_count: u8,
    bytes_per_operation: usize,
    seed: u64,
) {
    for index in 0..operation_count {
        event.operations.push(FieldOperation {
            operation_id: OperationId::from_bytes([100 + index; 16]),
            entity_type: EntityType::Task,
            entity_id: format!("bulk-entity-{index}"),
            field: "notes".to_string(),
            value: Some(json!(incompressible_text(
                0x9e37_79b9_u64
                    .wrapping_add(index as u64)
                    .wrapping_add(seed.wrapping_mul(0x1000_0000)),
                bytes_per_operation
            ))),
            parents: vec![],
        });
    }
}
