use ed25519_dalek::SigningKey;
use threestrands_sync_envelope::os_rng;
use serde_json::json;
use threestrands_sync_envelope::limits::{MAX_ENTITY_ID_BYTES, MAX_SNAPSHOT_FIELDS, MAX_VALUES_PER_FIELD, MAX_VALUE_BYTES};
use threestrands_sync_envelope::{
    message_id_for_snapshot, open_object, open_snapshot, seal_object, seal_snapshot, seal_snapshot_with_nonces,
    validate_snapshot, DeviceId, EntityType, EnvelopeError, ObjectKind, OpenParams, ReplicaSnapshot, SealParams,
    SequenceEntry, SnapshotField, SnapshotValue, PROTOCOL_VERSION,
};

#[path = "support/mod.rs"]
mod support;

const AUTHOR: [u8; 16] = [2u8; 16];
const PEER: [u8; 16] = [9u8; 16];

fn value(device: [u8; 16], counter: u64, json: serde_json::Value) -> SnapshotValue {
    SnapshotValue { device_id: DeviceId::from_bytes(device), counter, lamport: counter, value: Some(json) }
}

fn field(entity_id: &str, name: &str, values: Vec<SnapshotValue>) -> SnapshotField {
    SnapshotField { entity_type: EntityType::Task, entity_id: entity_id.to_string(), field: name.to_string(), values }
}

fn sample_snapshot() -> ReplicaSnapshot {
    ReplicaSnapshot {
        protocol_version: PROTOCOL_VERSION,
        device_id: DeviceId::from_bytes(AUTHOR),
        state_sequence: 42,
        created_at_ms: 1_700_000_000_000,
        context: vec![
            SequenceEntry { device_id: DeviceId::from_bytes(AUTHOR), sequence: 3 },
            SequenceEntry { device_id: DeviceId::from_bytes(PEER), sequence: 5 },
        ],
        fields: vec![
            field("task-1", "notes", vec![value(AUTHOR, 2, json!("mine")), value(PEER, 5, json!("theirs"))]),
            field("task-1", "title", vec![value(AUTHOR, 3, json!("Follow up with the vendor"))]),
        ],
    }
}

fn keys_and_epoch() -> (SigningKey, [u8; 32], &'static [u8], u32) {
    let signing_key = SigningKey::generate(&mut os_rng());
    let k_epoch = [9u8; 32];
    let sync_space_id: &[u8] = b"space-under-test";
    (signing_key, k_epoch, sync_space_id, 3)
}

fn seal_params<'a>(signing_key: &'a SigningKey, k_epoch: &'a [u8; 32], sync_space_id: &'a [u8], key_epoch: u32, object_kind: ObjectKind) -> SealParams<'a> {
    SealParams { sync_space_id, k_epoch, key_epoch, object_kind, signing_key }
}

fn open_params<'a>(verifying_key: &'a ed25519_dalek::VerifyingKey, k_epoch: &'a [u8; 32], sync_space_id: &'a [u8], key_epoch: u32) -> OpenParams<'a> {
    OpenParams { sync_space_id, k_epoch, key_epoch, verifying_key }
}

#[test]
fn seals_and_opens_a_single_chunk_snapshot() {
    let (signing_key, k_epoch, sync_space_id, key_epoch) = keys_and_epoch();
    let verifying_key = signing_key.verifying_key();
    let sealed = seal_snapshot(sample_snapshot(), &seal_params(&signing_key, &k_epoch, sync_space_id, key_epoch, ObjectKind::Snapshot)).unwrap();
    assert_eq!(sealed.chunks.len(), 1);
    assert!(sealed.head_cid().is_some());
    assert_eq!(sealed.message_id, message_id_for_snapshot(&DeviceId::from_bytes(AUTHOR), 42));

    let opened = open_snapshot(&sealed.chunks, &open_params(&verifying_key, &k_epoch, sync_space_id, key_epoch)).unwrap();
    assert_eq!(opened, sample_snapshot());
}

#[test]
fn seals_and_opens_a_multi_chunk_snapshot_in_any_chunk_order() {
    let (signing_key, k_epoch, sync_space_id, key_epoch) = keys_and_epoch();
    let verifying_key = signing_key.verifying_key();
    let mut snapshot = sample_snapshot();
    support::add_bulk_fields(&mut snapshot, 20, 60_000);
    let expected_fields = snapshot.fields.len();

    let sealed = seal_snapshot(snapshot, &seal_params(&signing_key, &k_epoch, sync_space_id, key_epoch, ObjectKind::Snapshot)).unwrap();
    assert!(sealed.chunks.len() > 1, "expected the bulk fields to force chunking");

    let params = open_params(&verifying_key, &k_epoch, sync_space_id, key_epoch);
    let opened = open_snapshot(&sealed.chunks, &params).unwrap();
    assert_eq!(opened.fields.len(), expected_fields);
    let mut reordered = sealed.chunks.clone();
    reordered.reverse();
    assert_eq!(open_snapshot(&reordered, &params).unwrap(), opened);
}

#[test]
fn one_state_always_seals_to_the_same_bytes_whatever_order_it_was_built_in() {
    let (signing_key, k_epoch, sync_space_id, key_epoch) = keys_and_epoch();
    let params = seal_params(&signing_key, &k_epoch, sync_space_id, key_epoch, ObjectKind::Snapshot);
    let mut shuffled = sample_snapshot();
    shuffled.fields.reverse();
    shuffled.fields[1].values.reverse();
    shuffled.context.reverse();
    let nonce = || [7u8; 24];
    let in_order = seal_snapshot_with_nonces(sample_snapshot(), &params, nonce).unwrap();
    let out_of_order = seal_snapshot_with_nonces(shuffled, &params, nonce).unwrap();
    assert_eq!(in_order.chunks, out_of_order.chunks);
}

#[test]
fn a_snapshot_that_isnt_in_canonical_order_is_refused_on_open() {
    // A peer that skipped canonicalizing: sealed as a raw object.
    let (signing_key, k_epoch, sync_space_id, key_epoch) = keys_and_epoch();
    let verifying_key = signing_key.verifying_key();
    let mut unordered = sample_snapshot();
    unordered.fields.reverse();
    let (_, sealed) = seal_object(
        &unordered,
        message_id_for_snapshot(&unordered.device_id, unordered.state_sequence),
        &seal_params(&signing_key, &k_epoch, sync_space_id, key_epoch, ObjectKind::Snapshot),
        || [1u8; 24],
    )
    .unwrap();
    assert_eq!(
        open_snapshot(&sealed.chunks, &open_params(&verifying_key, &k_epoch, sync_space_id, key_epoch)),
        Err(EnvelopeError::LimitExceeded("snapshot field order"))
    );
}

#[test]
fn a_snapshot_claiming_another_authors_message_id_is_refused() {
    let (signing_key, k_epoch, sync_space_id, key_epoch) = keys_and_epoch();
    let verifying_key = signing_key.verifying_key();
    let snapshot = sample_snapshot();
    let (_, sealed) = seal_object(
        &snapshot,
        message_id_for_snapshot(&snapshot.device_id, snapshot.state_sequence + 1),
        &seal_params(&signing_key, &k_epoch, sync_space_id, key_epoch, ObjectKind::Snapshot),
        || [1u8; 24],
    )
    .unwrap();
    assert_eq!(
        open_snapshot(&sealed.chunks, &open_params(&verifying_key, &k_epoch, sync_space_id, key_epoch)),
        Err(EnvelopeError::Malformed)
    );
}

#[test]
fn a_snapshot_is_refused_when_it_breaks_its_contract() {
    let refused = |change: &dyn Fn(&mut ReplicaSnapshot)| {
        let mut snapshot = sample_snapshot();
        change(&mut snapshot);
        snapshot.canonicalize();
        validate_snapshot(&snapshot).unwrap_err()
    };
    assert_eq!(refused(&|snapshot| snapshot.protocol_version = 2), EnvelopeError::UnsupportedProtocolVersion);
    assert_eq!(refused(&|snapshot| snapshot.context[1].sequence = 4), EnvelopeError::LimitExceeded("value outside context"));
    assert_eq!(refused(&|snapshot| snapshot.fields[0].values[0].counter = 0), EnvelopeError::LimitExceeded("value outside context"));
    assert_eq!(refused(&|snapshot| snapshot.fields[1].values.clear()), EnvelopeError::LimitExceeded("values per field"));
    assert_eq!(
        refused(&|snapshot| {
            let duplicate = snapshot.fields[1].clone();
            snapshot.fields.push(duplicate);
        }),
        EnvelopeError::LimitExceeded("snapshot field order")
    );
    assert_eq!(
        refused(&|snapshot| {
            let duplicate = snapshot.fields[1].values[0].clone();
            snapshot.fields[1].values.push(duplicate);
        }),
        EnvelopeError::LimitExceeded("snapshot value order")
    );
    assert_eq!(refused(&|snapshot| snapshot.fields[0].field.clear()), EnvelopeError::LimitExceeded("field name length"));
}

#[test]
fn every_snapshot_limit_holds_at_its_boundary() {
    let with_fields = |count: usize| ReplicaSnapshot {
        fields: (0..count).map(|index| field(&format!("t{index:07}"), "title", vec![value(AUTHOR, 1, json!(1))])).collect(),
        ..sample_snapshot()
    };
    assert!(validate_snapshot(&with_fields(MAX_SNAPSHOT_FIELDS - 1)).is_ok());
    assert!(validate_snapshot(&with_fields(MAX_SNAPSHOT_FIELDS)).is_ok());
    assert_eq!(validate_snapshot(&with_fields(MAX_SNAPSHOT_FIELDS + 1)), Err(EnvelopeError::LimitExceeded("snapshot field count")));

    let with_values = |count: usize| {
        let devices: Vec<[u8; 16]> = (0..count).map(|index| [index as u8 + 10; 16]).collect();
        let mut snapshot = ReplicaSnapshot {
            context: devices.iter().map(|device| SequenceEntry { device_id: DeviceId::from_bytes(*device), sequence: 1 }).collect(),
            fields: vec![field("task-1", "title", devices.iter().map(|device| value(*device, 1, json!(1))).collect())],
            ..sample_snapshot()
        };
        snapshot.canonicalize();
        snapshot
    };
    assert!(validate_snapshot(&with_values(MAX_VALUES_PER_FIELD - 1)).is_ok());
    assert!(validate_snapshot(&with_values(MAX_VALUES_PER_FIELD)).is_ok());
    assert_eq!(validate_snapshot(&with_values(MAX_VALUES_PER_FIELD + 1)), Err(EnvelopeError::LimitExceeded("values per field")));

    let with_entity_id = |length: usize| ReplicaSnapshot {
        fields: vec![field(&"e".repeat(length), "title", vec![value(AUTHOR, 1, json!(1))])],
        ..sample_snapshot()
    };
    assert!(validate_snapshot(&with_entity_id(MAX_ENTITY_ID_BYTES - 1)).is_ok());
    assert!(validate_snapshot(&with_entity_id(MAX_ENTITY_ID_BYTES)).is_ok());
    assert_eq!(validate_snapshot(&with_entity_id(MAX_ENTITY_ID_BYTES + 1)), Err(EnvelopeError::LimitExceeded("entity id length")));

    // A JSON string's encoding adds its two quotes.
    let with_value_bytes = |length: usize| ReplicaSnapshot {
        fields: vec![field("task-1", "title", vec![value(AUTHOR, 1, json!("v".repeat(length - 2)))])],
        ..sample_snapshot()
    };
    assert!(validate_snapshot(&with_value_bytes(MAX_VALUE_BYTES - 1)).is_ok());
    assert!(validate_snapshot(&with_value_bytes(MAX_VALUE_BYTES)).is_ok());
    assert_eq!(validate_snapshot(&with_value_bytes(MAX_VALUE_BYTES + 1)), Err(EnvelopeError::LimitExceeded("value size")));
}

#[test]
fn every_chunk_reports_the_epoch_it_was_sealed_under() {
    let (signing_key, k_epoch, sync_space_id, key_epoch) = keys_and_epoch();
    let sealed = seal_snapshot(sample_snapshot(), &seal_params(&signing_key, &k_epoch, sync_space_id, key_epoch, ObjectKind::Snapshot)).unwrap();
    for chunk in &sealed.chunks {
        assert_eq!(threestrands_sync_envelope::message_key_epoch(chunk).unwrap(), key_epoch);
    }
    assert!(threestrands_sync_envelope::message_key_epoch(&sealed.chunks[0][..19]).is_err());
    assert!(threestrands_sync_envelope::message_key_epoch(&[]).is_err());
}

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
struct SampleBody {
    label: String,
    entries: Vec<u32>,
}

#[test]
fn any_object_seals_and_opens_as_its_own_kind_only() {
    let (signing_key, k_epoch, sync_space_id, key_epoch) = keys_and_epoch();
    let verifying_key = signing_key.verifying_key();
    let params = open_params(&verifying_key, &k_epoch, sync_space_id, key_epoch);
    let body = SampleBody { label: "control".to_string(), entries: vec![1, 2, 3] };
    let (signature, sealed) = seal_object(
        &body,
        [7u8; 8],
        &seal_params(&signing_key, &k_epoch, sync_space_id, key_epoch, ObjectKind::KeyRotation),
        random_nonce,
    )
    .unwrap();

    let (opened, opened_signature, message_id): (SampleBody, _, _) = open_object(&sealed.chunks, &params, ObjectKind::KeyRotation).unwrap();
    assert_eq!(opened, body);
    assert_eq!(opened_signature, signature);
    assert_eq!(message_id, [7u8; 8]);

    // The same bytes never open as another kind, and in particular never
    // as a snapshot.
    assert!(matches!(open_object::<SampleBody>(&sealed.chunks, &params, ObjectKind::Snapshot), Err(EnvelopeError::UnexpectedObjectKind)));
    assert!(matches!(open_snapshot(&sealed.chunks, &params), Err(EnvelopeError::UnexpectedObjectKind)));
    let other = SigningKey::generate(&mut os_rng()).verifying_key();
    assert!(matches!(
        open_object::<SampleBody>(&sealed.chunks, &OpenParams { verifying_key: &other, ..params }, ObjectKind::KeyRotation),
        Err(EnvelopeError::SignatureInvalid)
    ));
}

#[test]
fn a_snapshot_is_sealed_only_as_a_snapshot_object() {
    let (signing_key, k_epoch, sync_space_id, key_epoch) = keys_and_epoch();
    let result = seal_snapshot(sample_snapshot(), &seal_params(&signing_key, &k_epoch, sync_space_id, key_epoch, ObjectKind::Operations));
    assert!(matches!(result, Err(EnvelopeError::UnexpectedObjectKind)));
}

fn random_nonce() -> [u8; 24] {
    let mut nonce = [0u8; 24];
    rand::Rng::fill_bytes(&mut os_rng(), &mut nonce);
    nonce
}
