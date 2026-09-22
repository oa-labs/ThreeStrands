//! Signed device heads: the portable discovery primitive transports use to
//! find a device's latest event without enumerating a transport's full
//! object store. Each device publishes only its own head.

use serde::{Deserialize, Serialize};

use crate::crypto;
use crate::error::EnvelopeError;
use crate::ids::{DeviceId, Signature};
use crate::{canonical_cbor, decode_canonical_cbor, SigningKey, VerifyingKey};

const HEAD_SIGNATURE_DOMAIN: &[u8] = b"threestrands/sync-envelope/device-head-signature/v1";

/// The fields of a device head that get signed. `sync_space_id` is opaque
/// bytes (not necessarily UTF-8) so it can be a random identifier rather
/// than anything that could double as user content.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceHead {
    #[serde(with = "serde_bytes")]
    pub sync_space_id: Vec<u8>,
    pub device_id: DeviceId,
    pub epoch: u32,
    /// The greatest device-sequence number contiguously known for this
    /// device: every event 1..=this has been accepted, with no gaps.
    pub contiguous_sequence: u64,
    /// CIDv1 of this device's latest published event (its head chunk; see
    /// `SealedMessage::head_cid`). `None` for a brand new device with no
    /// events yet.
    pub latest_event_cid: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SignedDeviceHead {
    pub head: DeviceHead,
    pub signature: Signature,
}

pub fn sign_device_head(
    signing_key: &SigningKey,
    head: DeviceHead,
) -> Result<SignedDeviceHead, EnvelopeError> {
    let canonical = canonical_cbor(&head)?;
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
    let canonical = canonical_cbor(&signed.head)?;
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
/// content-addressed or signed" posture. Canonical CBOR, the same
/// encoding used for every other on-wire structure in this crate.
pub fn encode_signed_head(signed: &SignedDeviceHead) -> Result<Vec<u8>, EnvelopeError> {
    canonical_cbor(signed)
}

pub fn decode_signed_head(bytes: &[u8]) -> Result<SignedDeviceHead, EnvelopeError> {
    decode_canonical_cbor(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    fn sample_head() -> DeviceHead {
        DeviceHead {
            sync_space_id: b"space".to_vec(),
            device_id: DeviceId::from_bytes([7u8; 16]),
            epoch: 2,
            contiguous_sequence: 9,
            latest_event_cid: Some("bafkreifzjut3te2nhyekklss27nh3k72ysco7y32koao5eei66wof36n5e".to_string()),
        }
    }

    #[test]
    fn round_trips_and_verifies() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let signed = sign_device_head(&signing_key, sample_head()).unwrap();
        verify_device_head(&signing_key.verifying_key(), &signed).unwrap();
    }

    #[test]
    fn wire_encoding_round_trips() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let signed = sign_device_head(&signing_key, sample_head()).unwrap();
        let bytes = encode_signed_head(&signed).unwrap();
        let decoded = decode_signed_head(&bytes).unwrap();
        assert_eq!(decoded, signed);
        verify_device_head(&signing_key.verifying_key(), &decoded).unwrap();
    }

    #[test]
    fn rejects_a_tampered_head() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let mut signed = sign_device_head(&signing_key, sample_head()).unwrap();
        signed.head.contiguous_sequence += 1;
        assert!(verify_device_head(&signing_key.verifying_key(), &signed).is_err());
    }

    #[test]
    fn rejects_the_wrong_key() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let other = SigningKey::generate(&mut OsRng);
        let signed = sign_device_head(&signing_key, sample_head()).unwrap();
        assert!(verify_device_head(&other.verifying_key(), &signed).is_err());
    }
}
