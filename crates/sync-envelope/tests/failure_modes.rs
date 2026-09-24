//! "Fails closed" tests: wrong sync space, epoch, suite, header, key, chunk
//! hash, or chunk membership must all be rejected, along with truncation,
//! extension, duplicate chunks, inconsistent counts, and cross-message
//! chunk substitution.

use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use serde_json::json;
use threestrands_sync_envelope::header::{HEADER_LEN, NONCE_LEN};
use threestrands_sync_envelope::{
    open_snapshot, seal_snapshot, DeviceId, EntityType, EnvelopeError, ObjectKind, OpenParams, ReplicaSnapshot,
    SealParams, SequenceEntry, SnapshotField, SnapshotValue, PROTOCOL_VERSION,
};

#[path = "support/mod.rs"]
mod support;

fn snapshot(value: &str) -> ReplicaSnapshot {
    let device_id = DeviceId::from_bytes([2u8; 16]);
    ReplicaSnapshot {
        protocol_version: PROTOCOL_VERSION,
        device_id,
        state_sequence: 1,
        created_at_ms: 0,
        context: vec![SequenceEntry { device_id, sequence: 1 }],
        fields: vec![SnapshotField {
            entity_type: EntityType::Task,
            entity_id: "task-1".to_string(),
            field: "title".to_string(),
            values: vec![SnapshotValue { device_id, counter: 1, lamport: 1, value: Some(json!(value)) }],
        }],
    }
}

struct Fixture {
    signing_key: SigningKey,
    k_epoch: [u8; 32],
    sync_space_id: &'static [u8],
    key_epoch: u32,
    chunks: Vec<Vec<u8>>,
}

fn sealed_fixture() -> Fixture {
    let signing_key = SigningKey::generate(&mut OsRng);
    let k_epoch = [4u8; 32];
    let sync_space_id: &[u8] = b"fixture-space";
    let key_epoch = 5;
    let sealed = seal_snapshot(
        snapshot("hello"),
        &SealParams {
            sync_space_id,
            k_epoch: &k_epoch,
            key_epoch,
            object_kind: ObjectKind::Snapshot,
            signing_key: &signing_key,
        },
    )
    .unwrap();
    Fixture {
        signing_key,
        k_epoch,
        sync_space_id,
        key_epoch,
        chunks: sealed.chunks,
    }
}

fn open(fixture: &Fixture, chunks: &[Vec<u8>]) -> Result<(), EnvelopeError> {
    let verifying_key = fixture.signing_key.verifying_key();
    open_snapshot(
        chunks,
        &OpenParams {
            sync_space_id: fixture.sync_space_id,
            k_epoch: &fixture.k_epoch,
            key_epoch: fixture.key_epoch,
            verifying_key: &verifying_key,
        },
    )
    .map(|_| ())
}

#[test]
fn wrong_sync_space_fails_closed() {
    let fixture = sealed_fixture();
    let verifying_key = fixture.signing_key.verifying_key();
    let result = open_snapshot(
        &fixture.chunks,
        &OpenParams {
            sync_space_id: b"a-different-space",
            k_epoch: &fixture.k_epoch,
            key_epoch: fixture.key_epoch,
            verifying_key: &verifying_key,
        },
    );
    assert_eq!(result, Err(EnvelopeError::DecryptionFailed));
}

#[test]
fn wrong_epoch_fails_closed() {
    let fixture = sealed_fixture();
    let verifying_key = fixture.signing_key.verifying_key();
    let result = open_snapshot(
        &fixture.chunks,
        &OpenParams {
            sync_space_id: fixture.sync_space_id,
            k_epoch: &fixture.k_epoch,
            key_epoch: fixture.key_epoch + 1,
            verifying_key: &verifying_key,
        },
    );
    assert_eq!(result, Err(EnvelopeError::DecryptionFailed));
}

#[test]
fn wrong_key_fails_closed() {
    let fixture = sealed_fixture();
    let verifying_key = fixture.signing_key.verifying_key();
    let mut wrong_key = fixture.k_epoch;
    wrong_key[0] ^= 0xFF;
    let result = open_snapshot(
        &fixture.chunks,
        &OpenParams {
            sync_space_id: fixture.sync_space_id,
            k_epoch: &wrong_key,
            key_epoch: fixture.key_epoch,
            verifying_key: &verifying_key,
        },
    );
    assert_eq!(result, Err(EnvelopeError::DecryptionFailed));
}

#[test]
fn wrong_signature_key_fails_closed() {
    let fixture = sealed_fixture();
    let other_verifying_key = SigningKey::generate(&mut OsRng).verifying_key();
    let result = open_snapshot(
        &fixture.chunks,
        &OpenParams {
            sync_space_id: fixture.sync_space_id,
            k_epoch: &fixture.k_epoch,
            key_epoch: fixture.key_epoch,
            verifying_key: &other_verifying_key,
        },
    );
    assert_eq!(result, Err(EnvelopeError::SignatureInvalid));
}

#[test]
fn wrong_header_byte_fails_closed() {
    let fixture = sealed_fixture();
    let mut chunks = fixture.chunks.clone();
    chunks[0][0] ^= 0xFF; // magic byte
    assert_eq!(open(&fixture, &chunks), Err(EnvelopeError::HeaderInvalid));

    let mut chunks = fixture.chunks.clone();
    chunks[0][3] = 99; // unknown cipher suite
    assert_eq!(open(&fixture, &chunks), Err(EnvelopeError::HeaderInvalid));

    let mut chunks = fixture.chunks.clone();
    chunks[0][4] ^= 0xFF; // key_epoch byte, still authenticated by AAD
    assert_eq!(
        open(&fixture, &chunks),
        Err(EnvelopeError::DecryptionFailed)
    );
}

#[test]
fn truncation_fails_closed() {
    let fixture = sealed_fixture();
    let mut chunks = fixture.chunks.clone();
    chunks[0].truncate(HEADER_LEN + NONCE_LEN); // no ciphertext/tag left
    assert_eq!(open(&fixture, &chunks), Err(EnvelopeError::Truncated));

    let mut chunks = fixture.chunks.clone();
    chunks[0].pop();
    assert_eq!(
        open(&fixture, &chunks),
        Err(EnvelopeError::DecryptionFailed)
    );
}

#[test]
fn extension_fails_closed() {
    let fixture = sealed_fixture();
    let mut chunks = fixture.chunks.clone();
    chunks[0].push(0);
    assert_eq!(
        open(&fixture, &chunks),
        Err(EnvelopeError::DecryptionFailed)
    );
}

#[test]
fn duplicate_chunk_indices_fail_closed() {
    let fixture = sealed_fixture();
    let mut chunks = fixture.chunks.clone();
    chunks.push(chunks[0].clone());
    assert_eq!(
        open(&fixture, &chunks),
        Err(EnvelopeError::IncompleteMessage)
    );
}

#[test]
fn cross_message_chunk_substitution_at_a_shared_index_fails_closed() {
    // Both messages happen to have the same author and sequence (so the
    // same message id) and both fit in one chunk, so the substituted chunk collides on
    // chunk_index too: the duplicate-index check fires first. See
    // `rejects_cross_message_chunk_substitution` in `chunk.rs` for the
    // narrower case where two chunks disagree only on their authenticated
    // message hash.
    let fixture = sealed_fixture();
    let signing_key = SigningKey::generate(&mut OsRng);
    let other_sealed = seal_snapshot(
        snapshot("a different message"),
        &SealParams {
            sync_space_id: fixture.sync_space_id,
            k_epoch: &fixture.k_epoch,
            key_epoch: fixture.key_epoch,
            object_kind: ObjectKind::Snapshot,
            signing_key: &signing_key,
        },
    )
    .unwrap();

    let mut chunks = fixture.chunks.clone();
    chunks.push(other_sealed.chunks[0].clone());
    assert_eq!(
        open(&fixture, &chunks),
        Err(EnvelopeError::IncompleteMessage)
    );
}

#[test]
fn cross_message_chunk_substitution_at_a_distinct_index_fails_closed() {
    // Two different sealed messages that share an author and sequence (hence
    // message id) and chunk count, but not content: splicing one message's chunk
    // into another's set at a non-colliding index must still be rejected,
    // via the authenticated message hash rather than index collision.
    let signing_key = SigningKey::generate(&mut OsRng);
    let k_epoch = [4u8; 32];
    let sync_space_id: &[u8] = b"fixture-space";
    let key_epoch = 5;
    let params = SealParams {
        sync_space_id,
        k_epoch: &k_epoch,
        key_epoch,
        object_kind: ObjectKind::Snapshot,
        signing_key: &signing_key,
    };

    let mut snapshot_a = snapshot("a");
    support::add_bulk_fields_seeded(&mut snapshot_a, 20, 60_000, 1);
    let sealed_a = seal_snapshot(snapshot_a, &params).unwrap();

    let mut snapshot_b = snapshot("b");
    support::add_bulk_fields_seeded(&mut snapshot_b, 20, 60_000, 2);
    let sealed_b = seal_snapshot(snapshot_b, &params).unwrap();

    assert_eq!(sealed_a.chunks.len(), sealed_b.chunks.len());
    assert!(sealed_a.chunks.len() > 1);

    let mut spliced = sealed_a.chunks.clone();
    let last = spliced.len() - 1;
    spliced[last] = sealed_b.chunks[last].clone();

    let verifying_key = signing_key.verifying_key();
    let result = open_snapshot(
        &spliced,
        &OpenParams {
            sync_space_id,
            k_epoch: &k_epoch,
            key_epoch,
            verifying_key: &verifying_key,
        },
    );
    assert_eq!(result, Err(EnvelopeError::CrossMessageChunk));
}

#[test]
fn inconsistent_chunk_counts_fail_closed() {
    // Two distinct seals that happen to share an author and sequence (so they
    // share a message id) but disagree wildly on size, and therefore chunk count.
    // Splicing a chunk from one into the other must never be accepted.
    let signing_key = SigningKey::generate(&mut OsRng);
    let k_epoch = [4u8; 32];
    let sync_space_id: &[u8] = b"fixture-space";
    let key_epoch = 5;
    let params = SealParams {
        sync_space_id,
        k_epoch: &k_epoch,
        key_epoch,
        object_kind: ObjectKind::Snapshot,
        signing_key: &signing_key,
    };

    let small_sealed = seal_snapshot(snapshot("small"), &params).unwrap();

    let mut big_snapshot = snapshot("placeholder");
    support::add_bulk_fields(&mut big_snapshot, 20, 60_000);
    let big_sealed = seal_snapshot(big_snapshot, &params).unwrap();

    assert_eq!(small_sealed.chunks.len(), 1);
    assert!(big_sealed.chunks.len() > 1);

    let mut spliced = small_sealed.chunks.clone();
    spliced.push(big_sealed.chunks[0].clone());

    let verifying_key = signing_key.verifying_key();
    let result = open_snapshot(
        &spliced,
        &OpenParams {
            sync_space_id,
            k_epoch: &k_epoch,
            key_epoch,
            verifying_key: &verifying_key,
        },
    );
    assert_eq!(result, Err(EnvelopeError::IncompleteMessage));
}

#[test]
fn incomplete_chunk_set_never_applies() {
    let signing_key = SigningKey::generate(&mut OsRng);
    let k_epoch = [4u8; 32];
    let sync_space_id: &[u8] = b"fixture-space";
    let key_epoch = 5;

    let mut big_snapshot = snapshot("placeholder");
    support::add_bulk_fields(&mut big_snapshot, 20, 60_000);

    let sealed = seal_snapshot(
        big_snapshot,
        &SealParams {
            sync_space_id,
            k_epoch: &k_epoch,
            key_epoch,
            object_kind: ObjectKind::Snapshot,
            signing_key: &signing_key,
        },
    )
    .unwrap();
    assert!(sealed.chunks.len() > 1);

    let verifying_key = signing_key.verifying_key();
    let incomplete = &sealed.chunks[..sealed.chunks.len() - 1];
    let result = open_snapshot(
        incomplete,
        &OpenParams {
            sync_space_id,
            k_epoch: &k_epoch,
            key_epoch,
            verifying_key: &verifying_key,
        },
    );
    assert_eq!(result, Err(EnvelopeError::IncompleteMessage));
}
