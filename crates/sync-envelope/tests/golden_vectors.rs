//! Checked-in golden vectors: a fixed event, fixed keys, and fixed nonces
//! must always seal to these exact bytes, and these exact bytes must
//! always open back to the same event. If a future change to this crate
//! alters the wire bytes for the same inputs, this test fails — that is
//! the point. A deliberate format change updates the frozen hex in
//! `vectors/` and explains why in the same commit.
//!
//! The `_v1` vectors are protocol version 1, which this crate no longer
//! accepts (version 2 added causal vectors and per-kind signature
//! domains). They stay checked in so the refusal itself is pinned.

use ed25519_dalek::SigningKey;
use serde_json::json;
use threestrands_sync_envelope::{
    open_message, seal_event_with_nonces, DeviceHead, DeviceId, EntityType, EventId,
    FieldOperation, ObjectKind, OpenParams, OperationId, SealParams, SequenceEntry, UnsignedSyncEvent,
    decode_signed_head, encode_signed_head, sign_device_head, verify_device_head, PROTOCOL_VERSION,
};

const SYNC_SPACE_ID: &[u8] = b"golden-vector-sync-space";
const KEY_EPOCH: u32 = 1;
const K_EPOCH: [u8; 32] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
    0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f,
];
const SIGNING_SEED: [u8; 32] = [
    0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x2b, 0x2c, 0x2d, 0x2e, 0x2f,
    0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x3b, 0x3c, 0x3d, 0x3e, 0x3f,
];
const CHUNK_NONCE: [u8; 24] = [
    0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x4b, 0x4c, 0x4d, 0x4e, 0x4f,
    0x50, 0x51, 0x52, 0x53, 0x54, 0x55, 0x56, 0x57,
];

const GOLDEN_CHUNK_HEX: &str = include_str!("vectors/operations_v2_single_chunk.hex");
const GOLDEN_MULTI_KEY_HEX: &str = include_str!("vectors/operations_v2_multi_key.hex");
const GOLDEN_HEAD_HEX: &str = include_str!("vectors/device_head_v2.hex");
const V1_CHUNK_HEX: &str = include_str!("vectors/operations_v1_single_chunk.hex");
const V1_MULTI_KEY_HEX: &str = include_str!("vectors/operations_v1_multi_key.hex");
const V1_HEAD_HEX: &str = include_str!("vectors/device_head_v1.hex");

/// The head CID of the single-chunk golden event above. The golden device
/// head below references it, so the two vectors stay tied to the same
/// corpus.
const GOLDEN_EVENT_HEAD_CID: &str = include_str!("vectors/operations_v2_single_chunk.cid");

fn golden_event() -> UnsignedSyncEvent {
    UnsignedSyncEvent {
        event_id: EventId::from_bytes([0x11; 16]),
        protocol_version: PROTOCOL_VERSION,
        key_epoch: KEY_EPOCH,
        device_id: DeviceId::from_bytes([0x22; 16]),
        device_sequence: 5,
        previous_device_event: Some(
            "bafkreifzjut3te2nhyekklss27nh3k72ysco7y32koao5eei66wof36n5e".to_string(),
        ),
        lamport: 9,
        created_at_ms: 1_700_000_000_000,
        causal_vector: vec![
            SequenceEntry { device_id: DeviceId::from_bytes([0x10; 16]), sequence: 3 },
            SequenceEntry { device_id: DeviceId::from_bytes([0x30; 16]), sequence: 12 },
        ],
        operations: vec![FieldOperation {
            operation_id: OperationId::from_bytes([0x33; 16]),
            entity_type: EntityType::Task,
            entity_id: "task-golden".to_string(),
            field: "title".to_string(),
            value: Some(json!("Golden vector fixture")),
            parents: vec![],
        }],
    }
}

fn hex_decode(hex: &str) -> Vec<u8> {
    let hex = hex.trim();
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn seals_to_the_exact_frozen_bytes() {
    let signing_key = SigningKey::from_bytes(&SIGNING_SEED);
    let mut nonces = std::iter::once(CHUNK_NONCE);
    let (_, sealed) = seal_event_with_nonces(
        golden_event(),
        &SealParams {
            sync_space_id: SYNC_SPACE_ID,
            k_epoch: &K_EPOCH,
            key_epoch: KEY_EPOCH,
            object_kind: ObjectKind::Operations,
            signing_key: &signing_key,
        },
        || nonces.next().expect("golden vector uses exactly one chunk"),
    )
    .unwrap();

    assert_eq!(sealed.chunks.len(), 1);
    assert_eq!(hex_encode(&sealed.chunks[0]), GOLDEN_CHUNK_HEX.trim());
    assert_eq!(sealed.head_cid().unwrap(), GOLDEN_EVENT_HEAD_CID.trim());
}

#[test]
fn opens_the_frozen_bytes_back_to_the_golden_event() {
    let signing_key = SigningKey::from_bytes(&SIGNING_SEED);
    let verifying_key = signing_key.verifying_key();
    let chunk = hex_decode(GOLDEN_CHUNK_HEX);

    let event = open_message(
        &[chunk],
        &OpenParams {
            sync_space_id: SYNC_SPACE_ID,
            k_epoch: &K_EPOCH,
            key_epoch: KEY_EPOCH,
            verifying_key: &verifying_key,
        },
    )
    .unwrap();

    assert_eq!(event.event_id, EventId::from_bytes([0x11; 16]));
    assert_eq!(event.device_sequence, 5);
    assert_eq!(event.lamport, 9);
    assert_eq!(event.causal_vector, golden_event().causal_vector);
    assert_eq!(event.operations.len(), 1);
    assert_eq!(
        event.operations[0].value,
        Some(json!("Golden vector fixture"))
    );
}

/// A golden event whose field value is a JSON object with several keys.
///
/// The frozen bytes pin DAG-CBOR's specified map-key ordering: keys sort
/// by encoded length first and then bytewise, so `b` precedes `aa` even
/// though plain lexicographic order would reverse them. If a codec change
/// (or a transitive serde_json feature such as `preserve_order`) ever
/// alters key ordering, this vector fails.
fn multi_key_event() -> UnsignedSyncEvent {
    UnsignedSyncEvent {
        event_id: EventId::from_bytes([0x44; 16]),
        protocol_version: PROTOCOL_VERSION,
        key_epoch: KEY_EPOCH,
        device_id: DeviceId::from_bytes([0x22; 16]),
        device_sequence: 6,
        previous_device_event: None,
        lamport: 10,
        created_at_ms: 1_700_000_000_000,
        causal_vector: vec![],
        operations: vec![FieldOperation {
            operation_id: OperationId::from_bytes([0x55; 16]),
            entity_type: EntityType::Task,
            entity_id: "task-golden-map".to_string(),
            field: "settings".to_string(),
            value: Some(json!({"zebra": 3, "b": 2, "aa": 1})),
            parents: vec![],
        }],
    }
}

#[test]
fn multi_key_object_value_seals_to_the_frozen_bytes() {
    let signing_key = SigningKey::from_bytes(&SIGNING_SEED);
    let mut nonces = std::iter::once(CHUNK_NONCE);
    let (_, sealed) = seal_event_with_nonces(
        multi_key_event(),
        &SealParams {
            sync_space_id: SYNC_SPACE_ID,
            k_epoch: &K_EPOCH,
            key_epoch: KEY_EPOCH,
            object_kind: ObjectKind::Operations,
            signing_key: &signing_key,
        },
        || nonces.next().expect("multi-key vector uses exactly one chunk"),
    )
    .unwrap();

    assert_eq!(sealed.chunks.len(), 1);
    assert_eq!(hex_encode(&sealed.chunks[0]), GOLDEN_MULTI_KEY_HEX.trim());
}

#[test]
fn opens_the_frozen_multi_key_bytes_back_to_the_event() {
    let signing_key = SigningKey::from_bytes(&SIGNING_SEED);
    let verifying_key = signing_key.verifying_key();
    let chunk = hex_decode(GOLDEN_MULTI_KEY_HEX);

    let event = open_message(
        &[chunk],
        &OpenParams {
            sync_space_id: SYNC_SPACE_ID,
            k_epoch: &K_EPOCH,
            key_epoch: KEY_EPOCH,
            verifying_key: &verifying_key,
        },
    )
    .unwrap();

    assert_eq!(event.event_id, EventId::from_bytes([0x44; 16]));
    assert_eq!(
        event.operations[0].value,
        Some(json!({"aa": 1, "b": 2, "zebra": 3}))
    );
}

fn golden_head() -> DeviceHead {
    DeviceHead {
        sync_space_id: SYNC_SPACE_ID.to_vec(),
        device_id: DeviceId::from_bytes([0x22; 16]),
        epoch: KEY_EPOCH,
        contiguous_sequence: 5,
        latest_event_cid: Some(GOLDEN_EVENT_HEAD_CID.trim().to_string()),
        published_at_ms: 1_700_000_500_000,
        ack: vec![
            SequenceEntry { device_id: DeviceId::from_bytes([0x10; 16]), sequence: 3 },
            SequenceEntry { device_id: DeviceId::from_bytes([0x22; 16]), sequence: 5 },
        ],
        snapshot_cid: None,
    }
}

/// The golden signed device head: the public, unencrypted discovery
/// object every transport publishes and resolves. Its frozen bytes pin the
/// canonical DAG-CBOR encoding of the head structure itself, which is the
/// byte string a cross-implementation decoder must accept.
#[test]
fn signed_head_encodes_to_the_frozen_bytes() {
    let signing_key = SigningKey::from_bytes(&SIGNING_SEED);
    let signed = sign_device_head(&signing_key, golden_head()).unwrap();

    let bytes = encode_signed_head(&signed).unwrap();
    assert_eq!(hex_encode(&bytes), GOLDEN_HEAD_HEX.trim());

    let decoded = decode_signed_head(&bytes).unwrap();
    assert_eq!(decoded, signed);
    verify_device_head(&signing_key.verifying_key(), &decoded).unwrap();
}

#[test]
fn protocol_version_1_events_and_heads_are_refused() {
    let verifying_key = SigningKey::from_bytes(&SIGNING_SEED).verifying_key();
    for hex in [V1_CHUNK_HEX, V1_MULTI_KEY_HEX] {
        let result = open_message(
            &[hex_decode(hex)],
            &OpenParams {
                sync_space_id: SYNC_SPACE_ID,
                k_epoch: &K_EPOCH,
                key_epoch: KEY_EPOCH,
                verifying_key: &verifying_key,
            },
        );
        assert!(result.is_err(), "a v1 event must not open");
    }
    assert!(decode_signed_head(&hex_decode(V1_HEAD_HEX)).is_err(), "a v1 head must not decode");
}

#[test]
fn an_event_of_an_earlier_protocol_version_is_never_sealed() {
    let signing_key = SigningKey::from_bytes(&SIGNING_SEED);
    let mut event = golden_event();
    event.protocol_version = 1;
    let result = seal_event_with_nonces(
        event,
        &SealParams {
            sync_space_id: SYNC_SPACE_ID,
            k_epoch: &K_EPOCH,
            key_epoch: KEY_EPOCH,
            object_kind: ObjectKind::Operations,
            signing_key: &signing_key,
        },
        || CHUNK_NONCE,
    );
    assert!(matches!(result, Err(threestrands_sync_envelope::EnvelopeError::UnsupportedProtocolVersion)));
}

/// Regenerates the frozen v2 files. Run deliberately, only for an
/// intentional format change: `cargo test -p threestrands-sync-envelope
/// --test golden_vectors -- --ignored regenerate`.
#[test]
#[ignore]
fn regenerate() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors");
    let signing_key = SigningKey::from_bytes(&SIGNING_SEED);
    let params = SealParams {
        sync_space_id: SYNC_SPACE_ID,
        k_epoch: &K_EPOCH,
        key_epoch: KEY_EPOCH,
        object_kind: ObjectKind::Operations,
        signing_key: &signing_key,
    };
    let (_, single) = seal_event_with_nonces(golden_event(), &params, || CHUNK_NONCE).unwrap();
    std::fs::write(dir.join("operations_v2_single_chunk.hex"), format!("{}\n", hex_encode(&single.chunks[0]))).unwrap();
    std::fs::write(dir.join("operations_v2_single_chunk.cid"), format!("{}\n", single.head_cid().unwrap())).unwrap();
    let (_, multi) = seal_event_with_nonces(multi_key_event(), &params, || CHUNK_NONCE).unwrap();
    std::fs::write(dir.join("operations_v2_multi_key.hex"), format!("{}\n", hex_encode(&multi.chunks[0]))).unwrap();
    // The head names the event's CID, so write that first and derive the
    // head from the fresh value rather than the stale checked-in one.
    let mut head = golden_head();
    head.latest_event_cid = single.head_cid();
    let signed = sign_device_head(&signing_key, head).unwrap();
    std::fs::write(dir.join("device_head_v2.hex"), format!("{}\n", hex_encode(&encode_signed_head(&signed).unwrap()))).unwrap();
}
