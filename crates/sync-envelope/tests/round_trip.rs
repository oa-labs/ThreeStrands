use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use serde_json::json;
use threestrands_sync_envelope::{
    seal_event, DeviceId, EntityType, EventId, FieldOperation, ObjectKind, OpenParams, OperationId,
    SealParams, UnsignedSyncEvent,
};

#[path = "support/mod.rs"]
mod support;

fn sample_event() -> UnsignedSyncEvent {
    UnsignedSyncEvent {
        event_id: EventId::from_bytes([1u8; 16]),
        protocol_version: 1,
        key_epoch: 3,
        device_id: DeviceId::from_bytes([2u8; 16]),
        device_sequence: 42,
        previous_device_event: Some(
            "bafkreifzjut3te2nhyekklss27nh3k72ysco7y32koao5eei66wof36n5e".to_string(),
        ),
        lamport: 7,
        created_at_ms: 1_700_000_000_000,
        operations: vec![FieldOperation {
            operation_id: OperationId::from_bytes([3u8; 16]),
            entity_type: EntityType::Task,
            entity_id: "task-1".to_string(),
            field: "title".to_string(),
            value: Some(json!("Follow up with the vendor")),
            parents: vec![],
        }],
    }
}

fn keys_and_epoch() -> (SigningKey, [u8; 32], &'static [u8], u32) {
    let signing_key = SigningKey::generate(&mut OsRng);
    let k_epoch = [9u8; 32];
    let sync_space_id: &[u8] = b"space-under-test";
    (signing_key, k_epoch, sync_space_id, 3)
}

#[test]
fn seals_and_opens_a_single_chunk_event() {
    let (signing_key, k_epoch, sync_space_id, key_epoch) = keys_and_epoch();
    let verifying_key = signing_key.verifying_key();

    let (signed_event, sealed) = seal_event(
        sample_event(),
        &SealParams {
            sync_space_id,
            k_epoch: &k_epoch,
            key_epoch,
            object_kind: ObjectKind::Operations,
            signing_key: &signing_key,
        },
    )
    .unwrap();

    assert_eq!(sealed.chunks.len(), 1);
    assert_eq!(sealed.chunk_cids().len(), 1);
    assert!(sealed.head_cid().is_some());

    let opened = threestrands_sync_envelope::open_message(
        &sealed.chunks,
        &OpenParams {
            sync_space_id,
            k_epoch: &k_epoch,
            key_epoch,
            verifying_key: &verifying_key,
        },
    )
    .unwrap();

    assert_eq!(opened.event_id, signed_event.event_id);
    assert_eq!(opened.device_sequence, signed_event.device_sequence);
    assert_eq!(opened.lamport, signed_event.lamport);
    assert_eq!(opened.operations.len(), 1);
    assert_eq!(
        opened.operations[0].value,
        Some(json!("Follow up with the vendor"))
    );
}

#[test]
fn seals_and_opens_a_multi_chunk_event() {
    let (signing_key, k_epoch, sync_space_id, key_epoch) = keys_and_epoch();
    let verifying_key = signing_key.verifying_key();

    let mut event = sample_event();
    // Many operations, each under the per-operation size limit, whose
    // combined incompressible bulk forces multiple chunks.
    support::add_bulk_operations(&mut event, 20, 60_000);
    let expected_operation_count = event.operations.len();

    let (_, sealed) = seal_event(
        event,
        &SealParams {
            sync_space_id,
            k_epoch: &k_epoch,
            key_epoch,
            object_kind: ObjectKind::Operations,
            signing_key: &signing_key,
        },
    )
    .unwrap();
    assert!(
        sealed.chunks.len() > 1,
        "expected the bulk operations to force chunking"
    );

    let opened = threestrands_sync_envelope::open_message(
        &sealed.chunks,
        &OpenParams {
            sync_space_id,
            k_epoch: &k_epoch,
            key_epoch,
            verifying_key: &verifying_key,
        },
    )
    .unwrap();
    assert_eq!(opened.operations.len(), expected_operation_count);

    // Chunk order in the input slice must not matter.
    let mut reordered = sealed.chunks.clone();
    reordered.reverse();
    let opened_reordered = threestrands_sync_envelope::open_message(
        &reordered,
        &OpenParams {
            sync_space_id,
            k_epoch: &k_epoch,
            key_epoch,
            verifying_key: &verifying_key,
        },
    )
    .unwrap();
    assert_eq!(opened_reordered.operations, opened.operations);
}

#[test]
fn rejects_an_event_with_no_operations() {
    let (signing_key, k_epoch, sync_space_id, key_epoch) = keys_and_epoch();
    let mut event = sample_event();
    event.operations.clear();

    let result = seal_event(
        event,
        &SealParams {
            sync_space_id,
            k_epoch: &k_epoch,
            key_epoch,
            object_kind: ObjectKind::Operations,
            signing_key: &signing_key,
        },
    );
    assert!(result.is_err());
}

#[test]
fn every_chunk_reports_the_epoch_it_was_sealed_under() {
    let (signing_key, k_epoch, sync_space_id, key_epoch) = keys_and_epoch();
    let (_, sealed) = seal_event(
        sample_event(),
        &SealParams {
            sync_space_id,
            k_epoch: &k_epoch,
            key_epoch,
            object_kind: ObjectKind::Operations,
            signing_key: &signing_key,
        },
    )
    .unwrap();
    for chunk in &sealed.chunks {
        assert_eq!(threestrands_sync_envelope::message_key_epoch(chunk).unwrap(), key_epoch);
    }
    assert!(threestrands_sync_envelope::message_key_epoch(&sealed.chunks[0][..19]).is_err());
    assert!(threestrands_sync_envelope::message_key_epoch(&[]).is_err());
}
