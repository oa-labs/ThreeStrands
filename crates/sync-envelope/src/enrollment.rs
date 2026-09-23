//! Device-to-device enrollment and key rotation objects. Like a
//! [`crate::device_head::SignedDeviceHead`], these are signed but otherwise
//! public — canonical DAG-CBOR, content-addressed, published and fetched
//! through the same [`crate`]-external object store as everything else. No
//! sync secret rides in cleartext: an epoch key only ever crosses this
//! layer as an anonymous [`crate::crypto::seal_to_x25519`] stanza that only
//! the intended recipient's static X25519 secret can open.

use serde::{Deserialize, Serialize};
use serde_bytes::ByteBuf;

use crate::crypto;
use crate::error::EnvelopeError;
use crate::ids::{DeviceId, RequestId, Signature};
use crate::{canonical_dag_cbor, decode_canonical_dag_cbor, SigningKey, VerifyingKey};

const REQUEST_SIGNATURE_DOMAIN: &[u8] = b"threestrands/sync-envelope/enrollment-request-signature/v1";
const GRANT_SIGNATURE_DOMAIN: &[u8] = b"threestrands/sync-envelope/enrollment-grant-signature/v1";
const REJECTION_SIGNATURE_DOMAIN: &[u8] = b"threestrands/sync-envelope/enrollment-rejection-signature/v1";
const ROTATION_SIGNATURE_DOMAIN: &[u8] = b"threestrands/sync-envelope/key-rotation-signature/v1";
const FINGERPRINT_DOMAIN: &[u8] = b"threestrands/sync-envelope/enrollment-fingerprint/v1";

/// A snapshot of one device's roster entry, carried inside a grant or
/// rotation object so a newly enrolling or recovering device can bootstrap
/// trust in every other active device at once, not only the one it directly
/// exchanged messages with.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RosterEntry {
    pub device_id: DeviceId,
    pub ed25519_public: ByteBuf,
    pub x25519_public: ByteBuf,
    pub status: String,
}

/// A new device's request to join, containing its full public keys and no
/// sync secret. Self-signed: the signature proves possession of the
/// embedded private key, not trust — trust comes from a human comparing
/// [`enrollment_fingerprint`] out of band.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EnrollmentRequest {
    pub request_id: RequestId,
    pub device_id: DeviceId,
    pub ed25519_public: ByteBuf,
    pub x25519_public: ByteBuf,
    pub created_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SignedEnrollmentRequest {
    pub request: EnrollmentRequest,
    pub signature: Signature,
}

pub fn sign_enrollment_request(
    signing_key: &SigningKey,
    request: EnrollmentRequest,
) -> Result<SignedEnrollmentRequest, EnvelopeError> {
    let canonical = canonical_dag_cbor(&request)?;
    let signature = Signature(crypto::sign_bytes(signing_key, REQUEST_SIGNATURE_DOMAIN, &canonical));
    Ok(SignedEnrollmentRequest { request, signature })
}

/// Verifies self-consistency only: that whoever holds the private key
/// matching `signed.request.ed25519_public` produced this request. Callers
/// must pass `verifying_key` derived from that same embedded field — this
/// function does not and cannot establish that the embedded key is
/// trustworthy.
pub fn verify_enrollment_request(verifying_key: &VerifyingKey, signed: &SignedEnrollmentRequest) -> Result<(), EnvelopeError> {
    let canonical = canonical_dag_cbor(&signed.request)?;
    crypto::verify_bytes(verifying_key, REQUEST_SIGNATURE_DOMAIN, &canonical, signed.signature.as_bytes())
}

pub fn encode_signed_enrollment_request(signed: &SignedEnrollmentRequest) -> Result<Vec<u8>, EnvelopeError> {
    canonical_dag_cbor(signed)
}

pub fn decode_signed_enrollment_request(bytes: &[u8]) -> Result<SignedEnrollmentRequest, EnvelopeError> {
    decode_canonical_dag_cbor(bytes)
}

/// An existing device's (or the recovery authority's) response to a
/// specific request: membership for the whole current roster, plus the
/// active epoch key sealed to the requester's X25519 public key alone.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EnrollmentGrant {
    pub request_id: RequestId,
    pub approver_device_id: DeviceId,
    /// Signed by the recovery Ed25519 key instead of an active device's —
    /// recovery-phrase self-import, with no peer device online.
    pub signed_by_recovery: bool,
    pub key_epoch: u32,
    pub sealed_epoch_key: ByteBuf,
    pub roster: Vec<RosterEntry>,
    /// The sync space's recovery public keys, so the newly enrolled device
    /// can keep sealing its own future rotations to them without needing a
    /// separate distribution step.
    pub recovery_ed25519_public: ByteBuf,
    pub recovery_x25519_public: ByteBuf,
    pub created_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SignedEnrollmentGrant {
    pub grant: EnrollmentGrant,
    pub signature: Signature,
}

pub fn sign_enrollment_grant(signing_key: &SigningKey, grant: EnrollmentGrant) -> Result<SignedEnrollmentGrant, EnvelopeError> {
    let canonical = canonical_dag_cbor(&grant)?;
    let signature = Signature(crypto::sign_bytes(signing_key, GRANT_SIGNATURE_DOMAIN, &canonical));
    Ok(SignedEnrollmentGrant { grant, signature })
}

/// Verifies the grant was signed by whoever holds `verifying_key`. Callers
/// resolve `verifying_key` themselves: the recovery Ed25519 public key when
/// `signed.grant.signed_by_recovery` is set, or the embedded roster entry
/// for `signed.grant.approver_device_id` otherwise — plus, for a brand new
/// device with no roster of its own yet, a human fingerprint confirmation
/// before trusting that embedded entry at all.
pub fn verify_enrollment_grant(verifying_key: &VerifyingKey, signed: &SignedEnrollmentGrant) -> Result<(), EnvelopeError> {
    let canonical = canonical_dag_cbor(&signed.grant)?;
    crypto::verify_bytes(verifying_key, GRANT_SIGNATURE_DOMAIN, &canonical, signed.signature.as_bytes())
}

pub fn encode_signed_enrollment_grant(signed: &SignedEnrollmentGrant) -> Result<Vec<u8>, EnvelopeError> {
    canonical_dag_cbor(signed)
}

pub fn decode_signed_enrollment_grant(bytes: &[u8]) -> Result<SignedEnrollmentGrant, EnvelopeError> {
    decode_canonical_dag_cbor(bytes)
}

/// A trusted member's decision to reject a specific enrollment request.
/// Rejections are signed control objects so every existing device, and the
/// requesting device, can resolve the same request after syncing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EnrollmentRejection {
    pub request_id: RequestId,
    pub rejector_device_id: DeviceId,
    pub rejector_ed25519_public: ByteBuf,
    pub rejected_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SignedEnrollmentRejection {
    pub rejection: EnrollmentRejection,
    pub signature: Signature,
}

pub fn sign_enrollment_rejection(signing_key: &SigningKey, rejection: EnrollmentRejection) -> Result<SignedEnrollmentRejection, EnvelopeError> {
    let canonical = canonical_dag_cbor(&rejection)?;
    let signature = Signature(crypto::sign_bytes(signing_key, REJECTION_SIGNATURE_DOMAIN, &canonical));
    Ok(SignedEnrollmentRejection { rejection, signature })
}

pub fn verify_enrollment_rejection(verifying_key: &VerifyingKey, signed: &SignedEnrollmentRejection) -> Result<(), EnvelopeError> {
    let canonical = canonical_dag_cbor(&signed.rejection)?;
    crypto::verify_bytes(verifying_key, REJECTION_SIGNATURE_DOMAIN, &canonical, signed.signature.as_bytes())
}

pub fn encode_signed_enrollment_rejection(signed: &SignedEnrollmentRejection) -> Result<Vec<u8>, EnvelopeError> {
    canonical_dag_cbor(signed)
}

pub fn decode_signed_enrollment_rejection(bytes: &[u8]) -> Result<SignedEnrollmentRejection, EnvelopeError> {
    decode_canonical_dag_cbor(bytes)
}

/// A new epoch: the fresh `K_epoch` sealed independently to every active
/// device's X25519 key and the recovery X25519 key (never as a cleartext
/// value), plus the roster snapshot that becomes active once this object is
/// accepted. Omitting a device from both `roster` and the sealed
/// recipients is how revocation happens — see the key hierarchy's
/// "revocation rotates the epoch and omits the revoked device."
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeyRotation {
    pub key_epoch: u32,
    pub initiator_device_id: DeviceId,
    pub roster: Vec<RosterEntry>,
    pub sealed_stanzas: Vec<ByteBuf>,
    pub recovery_ed25519_public: ByteBuf,
    pub recovery_x25519_public: ByteBuf,
    pub created_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SignedKeyRotation {
    pub rotation: KeyRotation,
    pub signature: Signature,
}

pub fn sign_key_rotation(signing_key: &SigningKey, rotation: KeyRotation) -> Result<SignedKeyRotation, EnvelopeError> {
    let canonical = canonical_dag_cbor(&rotation)?;
    let signature = Signature(crypto::sign_bytes(signing_key, ROTATION_SIGNATURE_DOMAIN, &canonical));
    Ok(SignedKeyRotation { rotation, signature })
}

/// Verifies the rotation was signed by whoever holds `verifying_key`.
/// Callers resolve it from their own current (pre-rotation) roster by
/// `signed.rotation.initiator_device_id` before calling this.
pub fn verify_key_rotation(verifying_key: &VerifyingKey, signed: &SignedKeyRotation) -> Result<(), EnvelopeError> {
    let canonical = canonical_dag_cbor(&signed.rotation)?;
    crypto::verify_bytes(verifying_key, ROTATION_SIGNATURE_DOMAIN, &canonical, signed.signature.as_bytes())
}

pub fn encode_signed_key_rotation(signed: &SignedKeyRotation) -> Result<Vec<u8>, EnvelopeError> {
    canonical_dag_cbor(signed)
}

pub fn decode_signed_key_rotation(bytes: &[u8]) -> Result<SignedKeyRotation, EnvelopeError> {
    decode_canonical_dag_cbor(bytes)
}

/// A short, human-comparable fingerprint of a device's public key pair —
/// the "four-word code" the plan calls for, rendered here as four hex
/// groups instead of a word list. Never a substitute for the underlying
/// signature check; it is what a human compares between two screens before
/// either side trusts the other's embedded public key.
pub fn enrollment_fingerprint(ed25519_public: &[u8], x25519_public: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(FINGERPRINT_DOMAIN);
    hasher.update(ed25519_public);
    hasher.update(x25519_public);
    let digest = hasher.finalize();
    digest[..8]
        .chunks(2)
        .map(|pair| format!("{:02X}{:02X}", pair[0], pair[1]))
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    fn sample_request(signing_key: &SigningKey, x25519_public: [u8; 32]) -> EnrollmentRequest {
        EnrollmentRequest {
            request_id: RequestId::from_bytes([1u8; 16]),
            device_id: DeviceId::from_bytes([2u8; 16]),
            ed25519_public: ByteBuf::from(signing_key.verifying_key().to_bytes().to_vec()),
            x25519_public: ByteBuf::from(x25519_public.to_vec()),
            created_at_ms: 1000,
        }
    }

    #[test]
    fn request_round_trips_and_verifies_self_consistently() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let signed = sign_enrollment_request(&signing_key, sample_request(&signing_key, [9u8; 32])).unwrap();
        verify_enrollment_request(&signing_key.verifying_key(), &signed).unwrap();

        let bytes = encode_signed_enrollment_request(&signed).unwrap();
        let decoded = decode_signed_enrollment_request(&bytes).unwrap();
        assert_eq!(decoded, signed);
    }

    #[test]
    fn request_rejects_a_tampered_field() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let mut signed = sign_enrollment_request(&signing_key, sample_request(&signing_key, [9u8; 32])).unwrap();
        signed.request.created_at_ms += 1;
        assert!(verify_enrollment_request(&signing_key.verifying_key(), &signed).is_err());
    }

    #[test]
    fn rejection_round_trips_and_verifies() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let signed = sign_enrollment_rejection(
            &signing_key,
            EnrollmentRejection {
                request_id: RequestId::from_bytes([1u8; 16]),
                rejector_device_id: DeviceId::from_bytes([3u8; 16]),
                rejector_ed25519_public: ByteBuf::from(signing_key.verifying_key().to_bytes().to_vec()),
                rejected_at_ms: 1000,
            },
        )
        .unwrap();
        verify_enrollment_rejection(&signing_key.verifying_key(), &signed).unwrap();
        let bytes = encode_signed_enrollment_rejection(&signed).unwrap();
        assert_eq!(decode_signed_enrollment_rejection(&bytes).unwrap(), signed);

        let mut tampered = signed;
        tampered.rejection.rejected_at_ms += 1;
        assert!(verify_enrollment_rejection(&signing_key.verifying_key(), &tampered).is_err());
    }

    #[test]
    fn grant_round_trips_and_verifies() {
        let approver = SigningKey::generate(&mut OsRng);
        let grant = EnrollmentGrant {
            request_id: RequestId::from_bytes([1u8; 16]),
            approver_device_id: DeviceId::from_bytes([3u8; 16]),
            signed_by_recovery: false,
            key_epoch: 4,
            sealed_epoch_key: ByteBuf::from(vec![7u8; 40]),
            roster: vec![RosterEntry {
                device_id: DeviceId::from_bytes([3u8; 16]),
                ed25519_public: ByteBuf::from(approver.verifying_key().to_bytes().to_vec()),
                x25519_public: ByteBuf::from(vec![1u8; 32]),
                status: "active".to_string(),
            }],
            recovery_ed25519_public: ByteBuf::from(vec![5u8; 32]),
            recovery_x25519_public: ByteBuf::from(vec![6u8; 32]),
            created_at_ms: 2000,
        };
        let signed = sign_enrollment_grant(&approver, grant).unwrap();
        verify_enrollment_grant(&approver.verifying_key(), &signed).unwrap();

        let other = SigningKey::generate(&mut OsRng).verifying_key();
        assert!(verify_enrollment_grant(&other, &signed).is_err());

        let bytes = encode_signed_enrollment_grant(&signed).unwrap();
        assert_eq!(decode_signed_enrollment_grant(&bytes).unwrap(), signed);
    }

    #[test]
    fn rotation_round_trips_and_verifies() {
        let initiator = SigningKey::generate(&mut OsRng);
        let rotation = KeyRotation {
            key_epoch: 5,
            initiator_device_id: DeviceId::from_bytes([4u8; 16]),
            roster: vec![],
            sealed_stanzas: vec![ByteBuf::from(vec![1u8; 40]), ByteBuf::from(vec![2u8; 40])],
            recovery_ed25519_public: ByteBuf::from(vec![5u8; 32]),
            recovery_x25519_public: ByteBuf::from(vec![6u8; 32]),
            created_at_ms: 3000,
        };
        let signed = sign_key_rotation(&initiator, rotation).unwrap();
        verify_key_rotation(&initiator.verifying_key(), &signed).unwrap();

        let bytes = encode_signed_key_rotation(&signed).unwrap();
        assert_eq!(decode_signed_key_rotation(&bytes).unwrap(), signed);
    }

    #[test]
    fn fingerprint_is_deterministic_and_sensitive_to_either_key() {
        let a = enrollment_fingerprint(&[1u8; 32], &[2u8; 32]);
        let again = enrollment_fingerprint(&[1u8; 32], &[2u8; 32]);
        assert_eq!(a, again);

        let different_ed25519 = enrollment_fingerprint(&[9u8; 32], &[2u8; 32]);
        assert_ne!(a, different_ed25519);
        let different_x25519 = enrollment_fingerprint(&[1u8; 32], &[9u8; 32]);
        assert_ne!(a, different_x25519);
    }
}
