//! `sync-envelope`: a pure byte-format crate for ThreeStrands replicated
//! sync. It has no network or provider concepts. Given key material and a
//! [`SyncEvent`], it produces exact, content-addressable, chunked,
//! compressed, padded, authenticated-encrypted bytes; given those bytes and
//! key material, it reverses the process and verifies every authenticator
//! along the way.
//!
//! Wire layout of one chunk:
//!
//! ```text
//! header(20) || nonce(24) || ciphertext_and_tag
//! ```
//!
//! See [`header`] for the header layout and associated data, and [`chunk`]
//! for the compressed/padded/hashed plaintext layout inside the ciphertext.

mod chunk;
mod cid;
mod crypto;
mod device_head;
mod enrollment;
mod error;
pub mod header;
mod ids;
mod join_code;
pub mod limits;
mod recovery;

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use cid::compute_cid;
pub use crypto::{seal_to_x25519, try_open_sealed_box};
pub use device_head::{
    decode_signed_head, encode_signed_head, sign_device_head, verify_device_head, DeviceHead,
    SignedDeviceHead,
};
pub use ed25519_dalek::{SigningKey, VerifyingKey};
pub use enrollment::{
    decode_signed_enrollment_grant, decode_signed_enrollment_rejection, decode_signed_enrollment_request, decode_signed_key_rotation,
    encode_signed_enrollment_grant, encode_signed_enrollment_rejection, encode_signed_enrollment_request, encode_signed_key_rotation,
    enrollment_fingerprint, sign_enrollment_grant, sign_enrollment_rejection, sign_enrollment_request, sign_key_rotation,
    verify_enrollment_grant, verify_enrollment_rejection, verify_enrollment_request, verify_key_rotation, EnrollmentGrant,
    EnrollmentRejection, EnrollmentRequest, KeyRotation, RosterEntry, SignedEnrollmentGrant, SignedEnrollmentRejection,
    SignedEnrollmentRequest, SignedKeyRotation,
};
pub use error::EnvelopeError;
pub use join_code::{
    decode_join_code, decode_signed_invitation, decode_signed_invitation_redemption, encode_join_code,
    encode_signed_invitation, encode_signed_invitation_redemption, generate_invite_secret, invite_ed25519_signing_key,
    invite_x25519_secret, sign_invitation, sign_invitation_redemption, verify_invitation, verify_invitation_redemption,
    Invitation, InvitationRedemption, JoinCode, JoinCodeError, JoinConnector, SignedInvitation,
    SignedInvitationRedemption, INVITE_SECRET_LEN, JOIN_CODE_PREFIX, JOIN_CODE_VERSION,
};
pub use header::{CipherSuite, ObjectKind};
pub use ids::{DeviceId, EventId, OperationId, RequestId, Signature};
pub use recovery::{
    check_recovery_phrase, generate_recovery_seed, recovery_ed25519_signing_key, recovery_phrase_from_seed,
    recovery_seed_from_phrase, recovery_x25519_secret, RecoveryPhraseCheck, RECOVERY_PHRASE_WORDS, RECOVERY_SEED_LEN,
};
pub use threestrands_sync_protocol::EntityType;
pub use x25519_dalek::{PublicKey as X25519PublicKey, StaticSecret as X25519StaticSecret};

use header::{build_aad, EnvelopeHeader, HEADER_LEN, MESSAGE_ID_LEN, NONCE_LEN, TAG_LEN};

/// One field-level operation inside a [`SyncEvent`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FieldOperation {
    pub operation_id: OperationId,
    pub entity_type: EntityType,
    pub entity_id: String,
    pub field: String,
    pub value: Option<Value>,
    pub parents: Vec<OperationId>,
}

/// A [`SyncEvent`] before it has been signed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UnsignedSyncEvent {
    pub event_id: EventId,
    pub protocol_version: u16,
    pub key_epoch: u32,
    pub device_id: DeviceId,
    pub device_sequence: u64,
    /// CIDv1 of the previous event on this device's append-only feed.
    pub previous_device_event: Option<String>,
    pub lamport: u64,
    /// Display only; never used for causality or ordering.
    pub created_at_ms: i64,
    pub operations: Vec<FieldOperation>,
}

/// One logical, signed sync event. This is exactly what gets canonically
/// DAG-CBOR-encoded to build the compressed, chunked, encrypted envelope.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SyncEvent {
    pub event_id: EventId,
    pub protocol_version: u16,
    pub key_epoch: u32,
    pub device_id: DeviceId,
    pub device_sequence: u64,
    pub previous_device_event: Option<String>,
    pub lamport: u64,
    pub created_at_ms: i64,
    pub operations: Vec<FieldOperation>,
    pub device_signature: Signature,
}

impl SyncEvent {
    /// The fields that are signed: everything except the signature itself.
    pub fn unsigned(&self) -> UnsignedSyncEvent {
        UnsignedSyncEvent {
            event_id: self.event_id,
            protocol_version: self.protocol_version,
            key_epoch: self.key_epoch,
            device_id: self.device_id,
            device_sequence: self.device_sequence,
            previous_device_event: self.previous_device_event.clone(),
            lamport: self.lamport,
            created_at_ms: self.created_at_ms,
            operations: self.operations.clone(),
        }
    }
}

/// Validates every hard limit in [`limits`] against a candidate event's
/// operations. Called both before sealing and after opening, so a
/// corrupted-but-decryptable message still cannot smuggle an
/// out-of-contract event past a caller.
pub fn validate_event_operations(operations: &[FieldOperation]) -> Result<(), EnvelopeError> {
    if operations.is_empty() || operations.len() > limits::MAX_OPERATIONS_PER_EVENT {
        return Err(EnvelopeError::LimitExceeded("operation count"));
    }

    let mut entities: HashSet<&str> = HashSet::new();
    let mut fields_per_entity: HashMap<&str, HashSet<&str>> = HashMap::new();

    for operation in operations {
        if operation.entity_id.is_empty() || operation.entity_id.len() > limits::MAX_ENTITY_ID_BYTES
        {
            return Err(EnvelopeError::LimitExceeded("entity id length"));
        }
        if operation.field.is_empty() || operation.field.len() > limits::MAX_FIELD_NAME_BYTES {
            return Err(EnvelopeError::LimitExceeded("field name length"));
        }
        if operation.parents.len() > limits::MAX_PARENTS_PER_OPERATION {
            return Err(EnvelopeError::LimitExceeded("parent count"));
        }
        if let Some(value) = &operation.value {
            let size = serde_json::to_vec(value)
                .map(|bytes| bytes.len())
                .unwrap_or(usize::MAX);
            if size > limits::MAX_VALUE_BYTES {
                return Err(EnvelopeError::LimitExceeded("value size"));
            }
        }

        entities.insert(operation.entity_id.as_str());
        let fields = fields_per_entity
            .entry(operation.entity_id.as_str())
            .or_default();
        fields.insert(operation.field.as_str());
        if fields.len() > limits::MAX_FIELDS_PER_ENTITY_PER_EVENT {
            return Err(EnvelopeError::LimitExceeded("fields per entity"));
        }
    }

    if entities.len() > limits::MAX_ENTITIES_PER_EVENT {
        return Err(EnvelopeError::LimitExceeded("entity count"));
    }
    Ok(())
}

/// Key material and framing needed to seal one event. Deliberately does not
/// know how `k_epoch` or `signing_key` are stored, sealed to other devices,
/// or rotated; that is the key hierarchy's responsibility.
pub struct SealParams<'a> {
    pub sync_space_id: &'a [u8],
    pub k_epoch: &'a [u8; 32],
    pub key_epoch: u32,
    pub object_kind: ObjectKind,
    pub signing_key: &'a SigningKey,
}

/// Key material and framing needed to open a previously sealed message.
pub struct OpenParams<'a> {
    pub sync_space_id: &'a [u8],
    pub k_epoch: &'a [u8; 32],
    pub key_epoch: u32,
    pub verifying_key: &'a VerifyingKey,
}

/// The result of sealing one event: its stable message id and the exact
/// wire bytes of every chunk, in order.
pub struct SealedMessage {
    pub message_id: [u8; MESSAGE_ID_LEN],
    pub chunks: Vec<Vec<u8>>,
}

impl SealedMessage {
    /// The CIDv1 of every chunk's exact wire bytes, in chunk order.
    pub fn chunk_cids(&self) -> Vec<String> {
        self.chunks.iter().map(|bytes| compute_cid(bytes)).collect()
    }

    /// The CID that `previous_device_event` should reference when a later
    /// event on this device names this one as its predecessor: the first
    /// chunk's CID, which is stable regardless of how many chunks follow.
    pub fn head_cid(&self) -> Option<String> {
        self.chunks.first().map(|bytes| compute_cid(bytes))
    }
}

/// Derives this event's stable message id from its event id alone, so
/// resealing the same event (for example after a crash between the local
/// commit and the encrypted upload) reproduces the same header and AAD
/// bytes rather than a fresh random value.
pub fn message_id_for_event(event_id: &EventId) -> [u8; MESSAGE_ID_LEN] {
    use sha2::{Digest, Sha256};
    const DOMAIN: &[u8] = b"threestrands/sync-envelope/message-id/v1";
    let mut hasher = Sha256::new();
    hasher.update(DOMAIN);
    hasher.update(event_id.as_bytes());
    let digest = hasher.finalize();
    let mut id = [0u8; MESSAGE_ID_LEN];
    id.copy_from_slice(&digest[..MESSAGE_ID_LEN]);
    id
}

/// Seals `unsigned` into a signed [`SyncEvent`] and its encrypted, chunked
/// wire bytes, drawing chunk nonces from `next_nonce` in order.
///
/// Use this variant (instead of [`seal_event`]) when a caller must persist
/// the exact nonces used before committing, and retry with the same nonces
/// after a crash, rather than re-encrypting with fresh random bytes for an
/// event id that was already committed locally.
pub fn seal_event_with_nonces<F>(
    unsigned: UnsignedSyncEvent,
    params: &SealParams,
    mut next_nonce: F,
) -> Result<(SyncEvent, SealedMessage), EnvelopeError>
where
    F: FnMut() -> [u8; NONCE_LEN],
{
    validate_event_operations(&unsigned.operations)?;
    validate_event_cid_references(&unsigned)?;

    let message_id = message_id_for_event(&unsigned.event_id);
    let unsigned_body = canonical_dag_cbor(&unsigned)?;
    let signature = Signature(crypto::sign_event(
        params.signing_key,
        &message_id,
        &unsigned_body,
    ));

    let event = SyncEvent {
        event_id: unsigned.event_id,
        protocol_version: unsigned.protocol_version,
        key_epoch: unsigned.key_epoch,
        device_id: unsigned.device_id,
        device_sequence: unsigned.device_sequence,
        previous_device_event: unsigned.previous_device_event,
        lamport: unsigned.lamport,
        created_at_ms: unsigned.created_at_ms,
        operations: unsigned.operations,
        device_signature: signature,
    };

    let canonical_body = canonical_dag_cbor(&event)?;
    let full_plaintext = chunk::build_full_plaintext(&canonical_body)?;
    let chunk_plaintexts = chunk::split_into_chunks(&full_plaintext)?;
    let chunk_count = chunk_plaintexts.len();
    if chunk_count > u16::MAX as usize {
        return Err(EnvelopeError::TooManyChunks);
    }

    let aead_key =
        crypto::derive_epoch_aead_key(params.k_epoch, params.sync_space_id, params.key_epoch);

    let mut chunks = Vec::with_capacity(chunk_count);
    for (index, plaintext) in chunk_plaintexts.into_iter().enumerate() {
        let header = EnvelopeHeader {
            object_kind: params.object_kind,
            cipher_suite: CipherSuite::XChaCha20Poly1305Hkdf,
            key_epoch: params.key_epoch,
            chunk_index: index as u16,
            chunk_count: chunk_count as u16,
            message_id,
        };
        let header_bytes = header.encode();
        let aad = build_aad(&header_bytes, params.sync_space_id);
        let nonce = next_nonce();
        let ciphertext = crypto::aead_encrypt(&aead_key, &nonce, &aad, &plaintext);

        let mut wire = Vec::with_capacity(HEADER_LEN + NONCE_LEN + ciphertext.len());
        wire.extend_from_slice(&header_bytes);
        wire.extend_from_slice(&nonce);
        wire.extend_from_slice(&ciphertext);
        chunks.push(wire);
    }

    Ok((event, SealedMessage { message_id, chunks }))
}

/// Seals `unsigned` using fresh random nonces. See
/// [`seal_event_with_nonces`] for the crash-safe, explicit-nonce variant.
pub fn seal_event(
    unsigned: UnsignedSyncEvent,
    params: &SealParams,
) -> Result<(SyncEvent, SealedMessage), EnvelopeError> {
    seal_event_with_nonces(unsigned, params, crypto::random_nonce)
}

/// Opens a complete set of chunks (in any order) for one message, verifying
/// every header field, the AEAD tag of every chunk, the reassembled
/// message's hash, every hard limit, and the device signature.
///
/// An incomplete, duplicated, or cross-message chunk set is rejected before
/// any bytes are exposed to the caller; there is no partial or "best
/// effort" result.
pub fn open_message(chunks: &[Vec<u8>], params: &OpenParams) -> Result<SyncEvent, EnvelopeError> {
    if chunks.is_empty() {
        return Err(EnvelopeError::IncompleteMessage);
    }

    let aead_key =
        crypto::derive_epoch_aead_key(params.k_epoch, params.sync_space_id, params.key_epoch);

    let mut seen_message_id: Option<[u8; MESSAGE_ID_LEN]> = None;
    let mut seen_chunk_count: Option<u16> = None;
    let mut plaintexts_by_index: BTreeMap<u16, Vec<u8>> = BTreeMap::new();

    for raw in chunks {
        if raw.len() < HEADER_LEN + NONCE_LEN + TAG_LEN {
            return Err(EnvelopeError::Truncated);
        }
        let header_bytes: [u8; HEADER_LEN] = raw[0..HEADER_LEN].try_into().unwrap();
        let header = EnvelopeHeader::decode(&header_bytes)?;

        if header.key_epoch != params.key_epoch {
            return Err(EnvelopeError::DecryptionFailed);
        }
        match seen_message_id {
            None => seen_message_id = Some(header.message_id),
            Some(id) if id == header.message_id => {}
            Some(_) => return Err(EnvelopeError::CrossMessageChunk),
        }
        match seen_chunk_count {
            None => seen_chunk_count = Some(header.chunk_count),
            Some(count) if count == header.chunk_count => {}
            Some(_) => return Err(EnvelopeError::IncompleteMessage),
        }
        if plaintexts_by_index.contains_key(&header.chunk_index) {
            return Err(EnvelopeError::IncompleteMessage);
        }

        let nonce: [u8; NONCE_LEN] = raw[HEADER_LEN..HEADER_LEN + NONCE_LEN].try_into().unwrap();
        let ciphertext = &raw[HEADER_LEN + NONCE_LEN..];
        let aad = build_aad(&header_bytes, params.sync_space_id);
        let plaintext = crypto::aead_decrypt(&aead_key, &nonce, &aad, ciphertext)?;
        plaintexts_by_index.insert(header.chunk_index, plaintext);
    }

    let chunk_count = seen_chunk_count.ok_or(EnvelopeError::IncompleteMessage)?;
    if plaintexts_by_index.len() != chunk_count as usize {
        return Err(EnvelopeError::IncompleteMessage);
    }
    let ordered: Vec<Vec<u8>> = (0..chunk_count)
        .map(|index| {
            plaintexts_by_index
                .remove(&index)
                .expect("length equality checked above guarantees every index is present")
        })
        .collect();

    let full_plaintext = chunk::reassemble_chunks(&ordered)?;
    let canonical_body = chunk::parse_full_plaintext(&full_plaintext)?;
    let event: SyncEvent = decode_canonical_dag_cbor(&canonical_body)?;

    validate_event_operations(&event.operations)?;
    validate_event_cid_references(&event.unsigned())?;

    let message_id = seen_message_id.expect("set for every non-empty chunk list");
    let unsigned_body = canonical_dag_cbor(&event.unsigned())?;
    crypto::verify_event(
        params.verifying_key,
        &message_id,
        &unsigned_body,
        event.device_signature.as_bytes(),
    )?;

    Ok(event)
}

fn canonical_dag_cbor<T: Serialize>(value: &T) -> Result<Vec<u8>, EnvelopeError> {
    // DAG-CBOR is IPLD's canonical CBOR profile: RFC 8949 core
    // deterministic encoding plus a specified map-key sort order, enforced
    // by both the encoder and the strict decoder. It is the basis for every
    // signature and golden vector in this crate, so canonicality is a
    // property of the codec rather than of input construction order.
    serde_ipld_dagcbor::to_vec(value).map_err(|_| EnvelopeError::EncodingFailed)
}

fn decode_canonical_dag_cbor<T: for<'de> Deserialize<'de>>(
    bytes: &[u8],
) -> Result<T, EnvelopeError> {
    serde_ipld_dagcbor::from_slice(bytes).map_err(|_| EnvelopeError::DecodingFailed)
}

/// Fails closed unless `cid` is a parseable CIDv1 string. CID-referencing
/// wire fields stay `String` in the Rust API because callers and storage
/// address objects by their textual CID, but the format itself never
/// accepts arbitrary text in a link position.
fn validate_cid_reference(cid: &str) -> Result<(), EnvelopeError> {
    // `::cid` is the external cid crate; a plain `cid::` path would resolve
    // to this crate's own `cid` module.
    let parsed = ::cid::Cid::try_from(cid).map_err(|_| EnvelopeError::InvalidCidReference)?;
    if parsed.version() != ::cid::Version::V1 {
        return Err(EnvelopeError::InvalidCidReference);
    }
    Ok(())
}

/// Validates every CID-referencing field of an event, at both seal and
/// open time, so a malformed link can never be signed or accepted.
fn validate_event_cid_references(event: &UnsignedSyncEvent) -> Result<(), EnvelopeError> {
    if let Some(previous) = &event.previous_device_event {
        validate_cid_reference(previous)?;
    }
    Ok(())
}

#[cfg(test)]
mod canonicality_tests {
    //! Proofs that canonicality is a property of the DAG-CBOR codec itself,
    //! not of how inputs happen to be constructed. These guard the
    //! signature-stability contract: if the codec ever stops sorting map
    //! keys, or starts accepting unsorted input bytes, one of these fails
    //! before any golden vector or signed structure can drift silently.

    use super::*;

    use serde::ser::{Serialize, SerializeMap, Serializer};

    /// Serializes as a map emitting keys deliberately out of DAG-CBOR
    /// order (and out of lexicographic order).
    struct UnsortedEmit {
        keys: Vec<(&'static str, u64)>,
    }

    impl Serialize for UnsortedEmit {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            let mut map = serializer.serialize_map(Some(self.keys.len()))?;
            for (key, value) in &self.keys {
                map.serialize_entry(key, value)?;
            }
            map.end()
        }
    }

    /// A Deserialize that records the order keys actually appear in on the
    /// wire, by consuming the map entry by entry.
    struct KeyOrder(Vec<String>);

    impl<'de> serde::Deserialize<'de> for KeyOrder {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            struct Visitor;
            impl<'de> serde::de::Visitor<'de> for Visitor {
                type Value = KeyOrder;

                fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    write!(f, "a map")
                }

                fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
                where
                    A: serde::de::MapAccess<'de>,
                {
                    let mut keys = Vec::new();
                    while let Some((key, _value)) = access
                        .next_entry::<String, serde_json::Value>()?
                    {
                        keys.push(key);
                    }
                    Ok(KeyOrder(keys))
                }
            }
            deserializer.deserialize_map(Visitor)
        }
    }

    #[test]
    fn the_encoder_sorts_map_keys_regardless_of_emit_order() {
        // Emission order: zebra (5 bytes), a (1 byte), mmmm (4 bytes).
        // DAG-CBOR order is length-first then bytewise: a, mmmm, zebra.
        // Plain lexicographic order would instead be a, mmmm, zebra here,
        // so pick a pair where the two rules disagree to prove which one
        // the codec implements: b (1) sorts before aa (2) by length, but
        // after it lexicographically.
        let emitted = UnsortedEmit {
            keys: vec![("zebra", 1), ("b", 2), ("aa", 3), ("mmmm", 4)],
        };
        let bytes = canonical_dag_cbor(&emitted).unwrap();
        let order = decode_canonical_dag_cbor::<KeyOrder>(&bytes).unwrap();
        assert_eq!(order.0, vec!["b", "aa", "mmmm", "zebra"]);

        // Re-encoding the decoded value must be byte-identical: the
        // canonical form is a fixed point.
        let value: serde_json::Value = decode_canonical_dag_cbor(&bytes).unwrap();
        assert_eq!(canonical_dag_cbor(&value).unwrap(), bytes);
    }

    #[test]
    fn the_decoder_rejects_unsorted_map_bytes() {
        // Hand-built CBOR: map(2) { "zz": 1, "a": 2 }. Valid CBOR, but not
        // canonical DAG-CBOR because "a" must precede "zz". Bytes produced
        // by a non-canonical encoder (such as an older ciborium build of
        // this crate) fail closed instead of decoding.
        let unsorted = [0xa2, 0x62, b'z', b'z', 0x01, 0x61, b'a', 0x02];
        let result: Result<serde_json::Value, _> = decode_canonical_dag_cbor(&unsorted);
        assert!(matches!(result, Err(EnvelopeError::DecodingFailed)));
    }

    #[test]
    fn cid_reference_validation_rejects_non_cid_strings() {
        assert!(validate_cid_reference("bafkreihyp2mdkcvn2et4tbcjqsirtmpevqgemx5ab2ac5oioyzmfkwlhlu").is_ok());
        assert!(matches!(
            validate_cid_reference("not-a-cid"),
            Err(EnvelopeError::InvalidCidReference)
        ));
    }
}
