//! Signed device heads: the portable discovery primitive transports use to
//! find a device's latest replica snapshot without enumerating a
//! transport's full object store. Each device publishes only its own head.

use serde::{Deserialize, Serialize};

use crate::crypto;
use crate::error::EnvelopeError;
use crate::ids::{DeviceId, Signature};
use crate::{canonical_dag_cbor, decode_canonical_dag_cbor, validate_cid_reference, SigningKey, VerifyingKey};

const HEAD_SIGNATURE_DOMAIN: &[u8] = b"threestrands/sync-envelope/device-head-signature/v3";

/// The fields of a device head that get signed. `sync_space_id` is opaque
/// bytes (not necessarily UTF-8) so it can be a random identifier rather
/// than anything that could double as user content.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceHead {
    #[serde(with = "serde_bytes")]
    pub sync_space_id: Vec<u8>,
    pub device_id: DeviceId,
    /// The key epoch this device is on.
    pub epoch: u32,
    /// The sequence of the snapshot `state_cid` names; 0 before this device
    /// has published one.
    pub state_sequence: u64,
    /// The chunk-index CID of this device's latest snapshot on this
    /// transport, or `None` before it has published one.
    pub state_cid: Option<String>,
    /// The signing device's wall-clock time when it published this head.
    /// Only for telling how recently a device synced; never used for
    /// ordering or causality.
    pub published_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SignedDeviceHead {
    pub head: DeviceHead,
    pub signature: Signature,
}

/// The snapshot link must be a CIDv1, and present exactly when a snapshot
/// sequence is, checked on signing, verifying, and decoding alike.
fn validate_head(head: &DeviceHead) -> Result<(), EnvelopeError> {
    if let Some(cid) = &head.state_cid {
        validate_cid_reference(cid)?;
    }
    if head.state_cid.is_some() != (head.state_sequence > 0) {
        return Err(EnvelopeError::Malformed);
    }
    Ok(())
}

pub fn sign_device_head(
    signing_key: &SigningKey,
    head: DeviceHead,
) -> Result<SignedDeviceHead, EnvelopeError> {
    validate_head(&head)?;
    let canonical = canonical_dag_cbor(&head)?;
    let signature = Signature(crypto::sign_bytes(signing_key, HEAD_SIGNATURE_DOMAIN, &canonical));
    Ok(SignedDeviceHead { head, signature })
}

/// Verifies the signature and returns the authenticated head. Callers still
/// need to check that `verifying_key` is actually the claimed device's
/// current, non-revoked key — this function only proves internal
/// consistency (the bytes were signed by whoever holds that key).
pub fn verify_device_head(
    verifying_key: &VerifyingKey,
    signed: &SignedDeviceHead,
) -> Result<(), EnvelopeError> {
    validate_head(&signed.head)?;
    let canonical = canonical_dag_cbor(&signed.head)?;
    crypto::verify_bytes(
        verifying_key,
        HEAD_SIGNATURE_DOMAIN,
        &canonical,
        signed.signature.as_bytes(),
    )
}

/// A stable byte encoding for a signed head crossing a transport boundary
/// (published to, and resolved from, a [`crate`]-external store). Not
/// encrypted — a head is signed but otherwise public, matching the
/// envelope's own "provider account IDs excluded, everything else
/// content-addressed or signed" posture. Canonical DAG-CBOR, the same
/// encoding used for every other on-wire structure in this crate.
pub fn encode_signed_head(signed: &SignedDeviceHead) -> Result<Vec<u8>, EnvelopeError> {
    canonical_dag_cbor(signed)
}

pub fn decode_signed_head(bytes: &[u8]) -> Result<SignedDeviceHead, EnvelopeError> {
    let signed: SignedDeviceHead = decode_canonical_dag_cbor(bytes)?;
    validate_head(&signed.head)?;
    Ok(signed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::os_rng;

    fn sample_head() -> DeviceHead {
        DeviceHead {
            sync_space_id: b"space".to_vec(),
            device_id: DeviceId::from_bytes([7u8; 16]),
            epoch: 2,
            state_sequence: 9,
            state_cid: Some("bafkreifzjut3te2nhyekklss27nh3k72ysco7y32koao5eei66wof36n5e".to_string()),
            published_at_ms: 1_700_000_000_000,
        }
    }

    #[test]
    fn round_trips_and_verifies() {
        let signing_key = SigningKey::generate(&mut os_rng());
        let signed = sign_device_head(&signing_key, sample_head()).unwrap();
        verify_device_head(&signing_key.verifying_key(), &signed).unwrap();
    }

    #[test]
    fn wire_encoding_round_trips() {
        let signing_key = SigningKey::generate(&mut os_rng());
        let signed = sign_device_head(&signing_key, sample_head()).unwrap();
        let bytes = encode_signed_head(&signed).unwrap();
        let decoded = decode_signed_head(&bytes).unwrap();
        assert_eq!(decoded, signed);
        verify_device_head(&signing_key.verifying_key(), &decoded).unwrap();
    }

    #[test]
    fn rejects_a_tampered_head() {
        let signing_key = SigningKey::generate(&mut os_rng());
        let mut signed = sign_device_head(&signing_key, sample_head()).unwrap();
        signed.head.state_sequence += 1;
        assert_eq!(verify_device_head(&signing_key.verifying_key(), &signed), Err(EnvelopeError::SignatureInvalid));
    }

    #[test]
    fn the_signature_covers_the_snapshot_and_publication_time() {
        let signing_key = SigningKey::generate(&mut os_rng());
        let signed = sign_device_head(&signing_key, sample_head()).unwrap();
        let verifying_key = signing_key.verifying_key();

        let mut sequence = signed.clone();
        sequence.head.state_sequence += 1;
        assert_eq!(verify_device_head(&verifying_key, &sequence), Err(EnvelopeError::SignatureInvalid));
        let mut published = signed.clone();
        published.head.published_at_ms += 1;
        assert_eq!(verify_device_head(&verifying_key, &published), Err(EnvelopeError::SignatureInvalid));
        let mut snapshot = signed;
        snapshot.head.state_cid = Some("bafkreihyp2mdkcvn2et4tbcjqsirtmpevqgemx5ab2ac5oioyzmfkwlhlu".to_string());
        assert_eq!(verify_device_head(&verifying_key, &snapshot), Err(EnvelopeError::SignatureInvalid));
    }

    #[test]
    fn a_malformed_snapshot_link_is_refused() {
        let signing_key = SigningKey::generate(&mut os_rng());
        let mut bad_link = sample_head();
        bad_link.state_cid = Some("not-a-cid".to_string());
        assert_eq!(sign_device_head(&signing_key, bad_link), Err(EnvelopeError::InvalidCidReference));
        let mut link_without_sequence = sample_head();
        link_without_sequence.state_sequence = 0;
        assert_eq!(sign_device_head(&signing_key, link_without_sequence), Err(EnvelopeError::Malformed));
        let mut sequence_without_link = sample_head();
        sequence_without_link.state_cid = None;
        assert_eq!(sign_device_head(&signing_key, sequence_without_link), Err(EnvelopeError::Malformed));
        let mut fresh = sample_head();
        fresh.state_sequence = 0;
        fresh.state_cid = None;
        assert!(sign_device_head(&signing_key, fresh).is_ok());
    }

    #[test]
    fn rejects_the_wrong_key() {
        let signing_key = SigningKey::generate(&mut os_rng());
        let other = SigningKey::generate(&mut os_rng());
        let signed = sign_device_head(&signing_key, sample_head()).unwrap();
        assert_eq!(verify_device_head(&other.verifying_key(), &signed), Err(EnvelopeError::SignatureInvalid));
    }
}
