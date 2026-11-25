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
use crate::limits::MAX_EARLIER_EPOCH_KEYS;
use crate::{canonical_dag_cbor, decode_canonical_dag_cbor, SigningKey, VerifyingKey};

const REQUEST_SIGNATURE_DOMAIN: &[u8] =
    b"threestrands/sync-envelope/enrollment-request-signature/v1";
const GRANT_SIGNATURE_DOMAIN: &[u8] = b"threestrands/sync-envelope/enrollment-grant-signature/v1";
const REJECTION_SIGNATURE_DOMAIN: &[u8] =
    b"threestrands/sync-envelope/enrollment-rejection-signature/v1";
const ROTATION_SIGNATURE_DOMAIN: &[u8] = b"threestrands/sync-envelope/key-rotation-signature/v1";
const ROTATION_ANCHOR_SIGNATURE_DOMAIN: &[u8] =
    b"threestrands/sync-envelope/key-rotation-recovery-anchor/v1";
const ROTATION_LINK_SIGNATURE_DOMAIN: &[u8] =
    b"threestrands/sync-envelope/key-rotation-epoch-link/v1";
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

/// One earlier epoch's `K_epoch`, sealed to the same recipient as the
/// object's current-epoch key. A device joining after a rotation needs
/// these to open history sealed before it arrived.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SealedEpochKey {
    pub key_epoch: u32,
    pub sealed_key: ByteBuf,
}

/// Earlier-epoch keys must name distinct epochs in ascending order, all
/// before `key_epoch`, and fit [`MAX_EARLIER_EPOCH_KEYS`]. Checked when
/// signing and again when decoding, so an out-of-contract list can neither
/// be produced nor accepted.
pub(crate) fn validate_earlier_epoch_keys(
    entries: &[SealedEpochKey],
    key_epoch: u32,
) -> Result<(), EnvelopeError> {
    if entries.len() > MAX_EARLIER_EPOCH_KEYS {
        return Err(EnvelopeError::LimitExceeded("earlier epoch key count"));
    }
    let mut previous: Option<u32> = None;
    for entry in entries {
        if entry.key_epoch >= key_epoch
            || previous.is_some_and(|previous| entry.key_epoch <= previous)
        {
            return Err(EnvelopeError::LimitExceeded("earlier epoch key order"));
        }
        previous = Some(entry.key_epoch);
    }
    Ok(())
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
    let signature = Signature(crypto::sign_bytes(
        signing_key,
        REQUEST_SIGNATURE_DOMAIN,
        &canonical,
    ));
    Ok(SignedEnrollmentRequest { request, signature })
}

/// Verifies self-consistency only: that whoever holds the private key
/// matching `signed.request.ed25519_public` produced this request. Callers
/// must pass `verifying_key` derived from that same embedded field — this
/// function does not and cannot establish that the embedded key is
/// trustworthy.
pub fn verify_enrollment_request(
    verifying_key: &VerifyingKey,
    signed: &SignedEnrollmentRequest,
) -> Result<(), EnvelopeError> {
    let canonical = canonical_dag_cbor(&signed.request)?;
    crypto::verify_bytes(
        verifying_key,
        REQUEST_SIGNATURE_DOMAIN,
        &canonical,
        signed.signature.as_bytes(),
    )
}

pub fn encode_signed_enrollment_request(
    signed: &SignedEnrollmentRequest,
) -> Result<Vec<u8>, EnvelopeError> {
    canonical_dag_cbor(signed)
}

pub fn decode_signed_enrollment_request(
    bytes: &[u8],
) -> Result<SignedEnrollmentRequest, EnvelopeError> {
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
    /// Every earlier epoch the approver holds, sealed to the requester like
    /// `sealed_epoch_key`. Omitted from the encoding when empty, so a grant
    /// with none is byte-identical to one made before this field existed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub earlier_epoch_keys: Vec<SealedEpochKey>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SignedEnrollmentGrant {
    pub grant: EnrollmentGrant,
    pub signature: Signature,
}

pub fn sign_enrollment_grant(
    signing_key: &SigningKey,
    grant: EnrollmentGrant,
) -> Result<SignedEnrollmentGrant, EnvelopeError> {
    validate_earlier_epoch_keys(&grant.earlier_epoch_keys, grant.key_epoch)?;
    let canonical = canonical_dag_cbor(&grant)?;
    let signature = Signature(crypto::sign_bytes(
        signing_key,
        GRANT_SIGNATURE_DOMAIN,
        &canonical,
    ));
    Ok(SignedEnrollmentGrant { grant, signature })
}

/// Verifies the grant was signed by whoever holds `verifying_key`. Callers
/// resolve `verifying_key` themselves: the recovery Ed25519 public key when
/// `signed.grant.signed_by_recovery` is set, or the embedded roster entry
/// for `signed.grant.approver_device_id` otherwise — plus, for a brand new
/// device with no roster of its own yet, a human fingerprint confirmation
/// before trusting that embedded entry at all.
pub fn verify_enrollment_grant(
    verifying_key: &VerifyingKey,
    signed: &SignedEnrollmentGrant,
) -> Result<(), EnvelopeError> {
    let canonical = canonical_dag_cbor(&signed.grant)?;
    crypto::verify_bytes(
        verifying_key,
        GRANT_SIGNATURE_DOMAIN,
        &canonical,
        signed.signature.as_bytes(),
    )
}

pub fn encode_signed_enrollment_grant(
    signed: &SignedEnrollmentGrant,
) -> Result<Vec<u8>, EnvelopeError> {
    canonical_dag_cbor(signed)
}

pub fn decode_signed_enrollment_grant(
    bytes: &[u8],
) -> Result<SignedEnrollmentGrant, EnvelopeError> {
    let signed: SignedEnrollmentGrant = decode_canonical_dag_cbor(bytes)?;
    validate_earlier_epoch_keys(&signed.grant.earlier_epoch_keys, signed.grant.key_epoch)?;
    Ok(signed)
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

pub fn sign_enrollment_rejection(
    signing_key: &SigningKey,
    rejection: EnrollmentRejection,
) -> Result<SignedEnrollmentRejection, EnvelopeError> {
    let canonical = canonical_dag_cbor(&rejection)?;
    let signature = Signature(crypto::sign_bytes(
        signing_key,
        REJECTION_SIGNATURE_DOMAIN,
        &canonical,
    ));
    Ok(SignedEnrollmentRejection {
        rejection,
        signature,
    })
}

pub fn verify_enrollment_rejection(
    verifying_key: &VerifyingKey,
    signed: &SignedEnrollmentRejection,
) -> Result<(), EnvelopeError> {
    let canonical = canonical_dag_cbor(&signed.rejection)?;
    crypto::verify_bytes(
        verifying_key,
        REJECTION_SIGNATURE_DOMAIN,
        &canonical,
        signed.signature.as_bytes(),
    )
}

pub fn encode_signed_enrollment_rejection(
    signed: &SignedEnrollmentRejection,
) -> Result<Vec<u8>, EnvelopeError> {
    canonical_dag_cbor(signed)
}

pub fn decode_signed_enrollment_rejection(
    bytes: &[u8],
) -> Result<SignedEnrollmentRejection, EnvelopeError> {
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

/// A signed [`KeyRotation`] and the proofs that place it in its group's
/// chain of epochs.
///
/// `signature` is the initiator's device signature, which a member checks
/// against its own roster. The two proofs let a device that holds only the
/// recovery phrase, and no roster yet, tell the group's real rotations from
/// ones planted by anyone who can merely write to the connector:
///
/// - `recovery_anchor`, on the genesis rotation (epoch 0) only: a signature
///   by the recovery Ed25519 key, which only a holder of the recovery
///   phrase can make.
/// - `epoch_link`, on every later rotation (epoch `n + 1`) only: a
///   signature by a key derived from the raw `K_n` of the epoch it replaces
///   (see [`crate::crypto::derive_epoch_link_signing_key`]), which only a
///   member holding that key can make.
///
/// Both sign the rotation's canonical encoding, each under its own domain.
/// Each is omitted from the encoding when absent, so a rotation published
/// before they existed (protocol 3) still decodes byte for byte, and is
/// then refused by [`verify_rotation_anchor`] and [`verify_rotation_link`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SignedKeyRotation {
    pub rotation: KeyRotation,
    pub signature: Signature,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_anchor: Option<Signature>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch_link: Option<Signature>,
}

/// The proof a new rotation carries besides its initiator's signature.
pub enum RotationProof<'a> {
    /// The genesis rotation (epoch 0): signed by the recovery Ed25519 key.
    Genesis {
        recovery_signing_key: &'a SigningKey,
    },
    /// A rotation to epoch `n + 1`: `previous_k_epoch` is the raw `K_n`
    /// of the epoch it replaces.
    Successor { previous_k_epoch: &'a [u8; 32] },
}

/// Epoch 0 carries an anchor and no link; every later epoch carries a link
/// and no anchor.
fn check_rotation_proof_shape(signed: &SignedKeyRotation) -> Result<(), EnvelopeError> {
    let genesis = signed.rotation.key_epoch == 0;
    match (
        genesis,
        signed.recovery_anchor.is_some(),
        signed.epoch_link.is_some(),
    ) {
        (true, true, false) | (false, false, true) => Ok(()),
        _ => Err(EnvelopeError::RotationProofMismatch),
    }
}

pub fn sign_key_rotation(
    signing_key: &SigningKey,
    rotation: KeyRotation,
    proof: RotationProof<'_>,
) -> Result<SignedKeyRotation, EnvelopeError> {
    let canonical = canonical_dag_cbor(&rotation)?;
    let signature = Signature(crypto::sign_bytes(
        signing_key,
        ROTATION_SIGNATURE_DOMAIN,
        &canonical,
    ));
    let (recovery_anchor, epoch_link) = match proof {
        RotationProof::Genesis {
            recovery_signing_key,
        } => (
            Some(Signature(crypto::sign_bytes(
                recovery_signing_key,
                ROTATION_ANCHOR_SIGNATURE_DOMAIN,
                &canonical,
            ))),
            None,
        ),
        RotationProof::Successor { previous_k_epoch } => {
            let previous_epoch = rotation
                .key_epoch
                .checked_sub(1)
                .ok_or(EnvelopeError::RotationProofMismatch)?;
            let link_key = crypto::derive_epoch_link_signing_key(previous_k_epoch, previous_epoch);
            (
                None,
                Some(Signature(crypto::sign_bytes(
                    &link_key,
                    ROTATION_LINK_SIGNATURE_DOMAIN,
                    &canonical,
                ))),
            )
        }
    };
    let signed = SignedKeyRotation {
        rotation,
        signature,
        recovery_anchor,
        epoch_link,
    };
    check_rotation_proof_shape(&signed)?;
    Ok(signed)
}

/// Verifies the rotation was signed by whoever holds `verifying_key`.
/// Callers resolve it from their own current (pre-rotation) roster by
/// `signed.rotation.initiator_device_id` before calling this. Checks the
/// initiator's signature only, never the anchor or link.
pub fn verify_key_rotation(
    verifying_key: &VerifyingKey,
    signed: &SignedKeyRotation,
) -> Result<(), EnvelopeError> {
    let canonical = canonical_dag_cbor(&signed.rotation)?;
    crypto::verify_bytes(
        verifying_key,
        ROTATION_SIGNATURE_DOMAIN,
        &canonical,
        signed.signature.as_bytes(),
    )
}

/// Verifies a genesis rotation's recovery anchor against the recovery
/// Ed25519 public key. Callers derive `recovery_verifying_key` from the
/// recovery phrase themselves, never from the rotation's own
/// `recovery_ed25519_public`. Fails unless the rotation is at epoch 0 and
/// carries an anchor and no link.
pub fn verify_rotation_anchor(
    recovery_verifying_key: &VerifyingKey,
    signed: &SignedKeyRotation,
) -> Result<(), EnvelopeError> {
    check_rotation_proof_shape(signed)?;
    let anchor = signed
        .recovery_anchor
        .as_ref()
        .ok_or(EnvelopeError::RotationProofMismatch)?;
    let canonical = canonical_dag_cbor(&signed.rotation)?;
    crypto::verify_bytes(
        recovery_verifying_key,
        ROTATION_ANCHOR_SIGNATURE_DOMAIN,
        &canonical,
        anchor.as_bytes(),
    )
}

/// Verifies that a rotation to epoch `n + 1` was made by a holder of
/// `previous_k_epoch`, the raw `K_n` the verifier already trusts. Fails
/// unless the rotation is past epoch 0 and carries a link and no anchor.
pub fn verify_rotation_link(
    previous_k_epoch: &[u8; 32],
    signed: &SignedKeyRotation,
) -> Result<(), EnvelopeError> {
    check_rotation_proof_shape(signed)?;
    let link = signed
        .epoch_link
        .as_ref()
        .ok_or(EnvelopeError::RotationProofMismatch)?;
    let previous_epoch = signed
        .rotation
        .key_epoch
        .checked_sub(1)
        .ok_or(EnvelopeError::RotationProofMismatch)?;
    let link_key = crypto::derive_epoch_link_signing_key(previous_k_epoch, previous_epoch);
    let canonical = canonical_dag_cbor(&signed.rotation)?;
    crypto::verify_bytes(
        &link_key.verifying_key(),
        ROTATION_LINK_SIGNATURE_DOMAIN,
        &canonical,
        link.as_bytes(),
    )
}

pub fn encode_signed_key_rotation(signed: &SignedKeyRotation) -> Result<Vec<u8>, EnvelopeError> {
    canonical_dag_cbor(signed)
}

/// Decodes a rotation of either shape, so a connector holding a protocol-3
/// group is still recognized as holding one. Whether it carries the proof
/// its epoch requires is checked by [`verify_rotation_anchor`] and
/// [`verify_rotation_link`].
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
    use crate::os_rng;

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
        let signing_key = SigningKey::generate(&mut os_rng());
        let signed =
            sign_enrollment_request(&signing_key, sample_request(&signing_key, [9u8; 32])).unwrap();
        verify_enrollment_request(&signing_key.verifying_key(), &signed).unwrap();

        let bytes = encode_signed_enrollment_request(&signed).unwrap();
        let decoded = decode_signed_enrollment_request(&bytes).unwrap();
        assert_eq!(decoded, signed);
    }

    #[test]
    fn request_rejects_a_tampered_field() {
        let signing_key = SigningKey::generate(&mut os_rng());
        let mut signed =
            sign_enrollment_request(&signing_key, sample_request(&signing_key, [9u8; 32])).unwrap();
        signed.request.created_at_ms += 1;
        assert!(verify_enrollment_request(&signing_key.verifying_key(), &signed).is_err());
    }

    #[test]
    fn rejection_round_trips_and_verifies() {
        let signing_key = SigningKey::generate(&mut os_rng());
        let signed = sign_enrollment_rejection(
            &signing_key,
            EnrollmentRejection {
                request_id: RequestId::from_bytes([1u8; 16]),
                rejector_device_id: DeviceId::from_bytes([3u8; 16]),
                rejector_ed25519_public: ByteBuf::from(
                    signing_key.verifying_key().to_bytes().to_vec(),
                ),
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
        let approver = SigningKey::generate(&mut os_rng());
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
            earlier_epoch_keys: vec![],
        };
        let signed = sign_enrollment_grant(&approver, grant).unwrap();
        verify_enrollment_grant(&approver.verifying_key(), &signed).unwrap();

        let other = SigningKey::generate(&mut os_rng()).verifying_key();
        assert!(verify_enrollment_grant(&other, &signed).is_err());

        let bytes = encode_signed_enrollment_grant(&signed).unwrap();
        assert_eq!(decode_signed_enrollment_grant(&bytes).unwrap(), signed);
    }

    fn sample_grant(
        approver: &SigningKey,
        key_epoch: u32,
        earlier_epoch_keys: Vec<SealedEpochKey>,
    ) -> EnrollmentGrant {
        EnrollmentGrant {
            request_id: RequestId::from_bytes([1u8; 16]),
            approver_device_id: DeviceId::from_bytes([3u8; 16]),
            signed_by_recovery: false,
            key_epoch,
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
            earlier_epoch_keys,
        }
    }

    fn earlier(epochs: impl IntoIterator<Item = u32>) -> Vec<SealedEpochKey> {
        epochs
            .into_iter()
            .map(|key_epoch| SealedEpochKey {
                key_epoch,
                sealed_key: ByteBuf::from(vec![key_epoch as u8; 72]),
            })
            .collect()
    }

    #[test]
    fn a_grant_carries_earlier_epoch_keys_under_its_signature() {
        let approver = SigningKey::generate(&mut os_rng());
        let signed =
            sign_enrollment_grant(&approver, sample_grant(&approver, 4, earlier([0, 1, 3])))
                .unwrap();
        let bytes = encode_signed_enrollment_grant(&signed).unwrap();
        let decoded = decode_signed_enrollment_grant(&bytes).unwrap();
        assert_eq!(decoded, signed);
        verify_enrollment_grant(&approver.verifying_key(), &decoded).unwrap();

        let mut tampered = decoded.clone();
        tampered.grant.earlier_epoch_keys[1].sealed_key = ByteBuf::from(vec![0u8; 72]);
        assert!(verify_enrollment_grant(&approver.verifying_key(), &tampered).is_err());
        let mut dropped = decoded;
        dropped.grant.earlier_epoch_keys.pop();
        assert!(verify_enrollment_grant(&approver.verifying_key(), &dropped).is_err());
    }

    /// The shape `EnrollmentGrant` had before `earlier_epoch_keys` existed.
    #[derive(Serialize)]
    struct PreKeyringGrant {
        request_id: RequestId,
        approver_device_id: DeviceId,
        signed_by_recovery: bool,
        key_epoch: u32,
        sealed_epoch_key: ByteBuf,
        roster: Vec<RosterEntry>,
        recovery_ed25519_public: ByteBuf,
        recovery_x25519_public: ByteBuf,
        created_at_ms: i64,
    }

    #[derive(Serialize)]
    struct PreKeyringSignedGrant {
        grant: PreKeyringGrant,
        signature: Signature,
    }

    #[test]
    fn a_grant_from_before_earlier_epoch_keys_still_decodes_and_verifies() {
        let approver = SigningKey::generate(&mut os_rng());
        let current = sample_grant(&approver, 4, vec![]);
        let legacy = PreKeyringGrant {
            request_id: current.request_id,
            approver_device_id: current.approver_device_id,
            signed_by_recovery: current.signed_by_recovery,
            key_epoch: current.key_epoch,
            sealed_epoch_key: current.sealed_epoch_key.clone(),
            roster: current.roster.clone(),
            recovery_ed25519_public: current.recovery_ed25519_public.clone(),
            recovery_x25519_public: current.recovery_x25519_public.clone(),
            created_at_ms: current.created_at_ms,
        };
        let legacy_canonical = canonical_dag_cbor(&legacy).unwrap();
        let signature = Signature(crypto::sign_bytes(
            &approver,
            GRANT_SIGNATURE_DOMAIN,
            &legacy_canonical,
        ));
        let legacy_bytes = canonical_dag_cbor(&PreKeyringSignedGrant {
            grant: legacy,
            signature,
        })
        .unwrap();

        let decoded = decode_signed_enrollment_grant(&legacy_bytes).unwrap();
        assert!(decoded.grant.earlier_epoch_keys.is_empty());
        verify_enrollment_grant(&approver.verifying_key(), &decoded).unwrap();
        // With no earlier keys, a grant encodes exactly as it did before.
        assert_eq!(
            encode_signed_enrollment_grant(&decoded).unwrap(),
            legacy_bytes
        );
    }

    #[test]
    fn earlier_epoch_keys_are_limited_in_count() {
        let approver = SigningKey::generate(&mut os_rng());
        let at_limit = MAX_EARLIER_EPOCH_KEYS as u32;
        let below = sign_enrollment_grant(
            &approver,
            sample_grant(&approver, at_limit, earlier(0..at_limit - 1)),
        )
        .unwrap();
        assert!(
            decode_signed_enrollment_grant(&encode_signed_enrollment_grant(&below).unwrap())
                .is_ok()
        );
        let exact = sign_enrollment_grant(
            &approver,
            sample_grant(&approver, at_limit, earlier(0..at_limit)),
        )
        .unwrap();
        assert!(
            decode_signed_enrollment_grant(&encode_signed_enrollment_grant(&exact).unwrap())
                .is_ok()
        );

        let above = sample_grant(&approver, at_limit + 1, earlier(0..at_limit + 1));
        assert!(matches!(
            sign_enrollment_grant(&approver, above.clone()),
            Err(EnvelopeError::LimitExceeded("earlier epoch key count"))
        ));
        // A peer that skipped the check when signing is still refused.
        let unchecked = SignedEnrollmentGrant {
            grant: above,
            signature: Signature([0u8; 64]),
        };
        let bytes = canonical_dag_cbor(&unchecked).unwrap();
        assert!(matches!(
            decode_signed_enrollment_grant(&bytes),
            Err(EnvelopeError::LimitExceeded("earlier epoch key count"))
        ));
    }

    #[test]
    fn earlier_epoch_keys_must_be_distinct_ascending_and_before_the_current_epoch() {
        let approver = SigningKey::generate(&mut os_rng());
        for bad in [
            earlier([2, 1]),
            earlier([1, 1]),
            earlier([1, 4]),
            earlier([5]),
        ] {
            let grant = sample_grant(&approver, 4, bad);
            assert!(matches!(
                sign_enrollment_grant(&approver, grant.clone()),
                Err(EnvelopeError::LimitExceeded("earlier epoch key order"))
            ));
            let bytes = canonical_dag_cbor(&SignedEnrollmentGrant {
                grant,
                signature: Signature([0u8; 64]),
            })
            .unwrap();
            assert!(decode_signed_enrollment_grant(&bytes).is_err());
        }
    }

    fn sample_rotation(key_epoch: u32) -> KeyRotation {
        KeyRotation {
            key_epoch,
            initiator_device_id: DeviceId::from_bytes([4u8; 16]),
            roster: vec![],
            sealed_stanzas: vec![ByteBuf::from(vec![1u8; 40]), ByteBuf::from(vec![2u8; 40])],
            recovery_ed25519_public: ByteBuf::from(vec![5u8; 32]),
            recovery_x25519_public: ByteBuf::from(vec![6u8; 32]),
            created_at_ms: 3000,
        }
    }

    #[test]
    fn rotation_round_trips_and_verifies() {
        let initiator = SigningKey::generate(&mut os_rng());
        let signed = sign_key_rotation(
            &initiator,
            sample_rotation(5),
            RotationProof::Successor {
                previous_k_epoch: &[7u8; 32],
            },
        )
        .unwrap();
        verify_key_rotation(&initiator.verifying_key(), &signed).unwrap();
        verify_rotation_link(&[7u8; 32], &signed).unwrap();

        let bytes = encode_signed_key_rotation(&signed).unwrap();
        let decoded = decode_signed_key_rotation(&bytes).unwrap();
        assert_eq!(decoded, signed);
        assert_eq!(encode_signed_key_rotation(&decoded).unwrap(), bytes);
    }

    #[test]
    fn a_genesis_anchor_verifies_only_under_the_recovery_key() {
        let initiator = SigningKey::generate(&mut os_rng());
        let recovery = SigningKey::generate(&mut os_rng());
        let signed = sign_key_rotation(
            &initiator,
            sample_rotation(0),
            RotationProof::Genesis {
                recovery_signing_key: &recovery,
            },
        )
        .unwrap();
        assert!(signed.epoch_link.is_none());
        verify_rotation_anchor(&recovery.verifying_key(), &signed).unwrap();
        verify_key_rotation(&initiator.verifying_key(), &signed).unwrap();
        assert_eq!(
            verify_rotation_anchor(&initiator.verifying_key(), &signed),
            Err(EnvelopeError::SignatureInvalid)
        );

        let mut tampered = signed.clone();
        tampered.rotation.created_at_ms += 1;
        assert!(verify_rotation_anchor(&recovery.verifying_key(), &tampered).is_err());

        let bytes = encode_signed_key_rotation(&signed).unwrap();
        let decoded = decode_signed_key_rotation(&bytes).unwrap();
        assert_eq!(decoded, signed);
        assert_eq!(encode_signed_key_rotation(&decoded).unwrap(), bytes);
    }

    #[test]
    fn an_epoch_link_verifies_only_under_the_previous_epochs_key() {
        let initiator = SigningKey::generate(&mut os_rng());
        let k_previous = [7u8; 32];
        let signed = sign_key_rotation(
            &initiator,
            sample_rotation(5),
            RotationProof::Successor {
                previous_k_epoch: &k_previous,
            },
        )
        .unwrap();
        assert!(signed.recovery_anchor.is_none());
        verify_rotation_link(&k_previous, &signed).unwrap();
        assert_eq!(
            verify_rotation_link(&[8u8; 32], &signed),
            Err(EnvelopeError::SignatureInvalid)
        );

        let mut tampered = signed.clone();
        tampered.rotation.roster.push(RosterEntry {
            device_id: DeviceId::from_bytes([9u8; 16]),
            ed25519_public: ByteBuf::from(vec![9u8; 32]),
            x25519_public: ByteBuf::from(vec![9u8; 32]),
            status: "active".to_string(),
        });
        assert_eq!(
            verify_rotation_link(&k_previous, &tampered),
            Err(EnvelopeError::SignatureInvalid)
        );
        let mut flipped = signed;
        flipped.epoch_link.as_mut().unwrap().0[0] ^= 1;
        assert_eq!(
            verify_rotation_link(&k_previous, &flipped),
            Err(EnvelopeError::SignatureInvalid)
        );
    }

    /// The link key is derived for the epoch it replaces, so a link made
    /// from `K_n` for epoch `n + 1` proves nothing about epoch `n + 2`,
    /// even when the rest of the rotation is copied unchanged.
    #[test]
    fn an_epoch_link_is_bound_to_the_epoch_it_follows() {
        let initiator = SigningKey::generate(&mut os_rng());
        let k_previous = [7u8; 32];
        let for_epoch_5 = sign_key_rotation(
            &initiator,
            sample_rotation(5),
            RotationProof::Successor {
                previous_k_epoch: &k_previous,
            },
        )
        .unwrap();
        let mut moved = for_epoch_5.clone();
        moved.rotation.key_epoch = 6;
        moved.signature = sign_key_rotation(
            &initiator,
            moved.rotation.clone(),
            RotationProof::Successor {
                previous_k_epoch: &k_previous,
            },
        )
        .unwrap()
        .signature;
        verify_key_rotation(&initiator.verifying_key(), &moved).unwrap();
        assert_eq!(
            verify_rotation_link(&k_previous, &moved),
            Err(EnvelopeError::SignatureInvalid)
        );

        // Signed with the link key for epoch 4 -> 5, but checked as if it
        // followed epoch 5: the derived keys differ.
        let link_key_4 = crypto::derive_epoch_link_signing_key(&k_previous, 4);
        let link_key_5 = crypto::derive_epoch_link_signing_key(&k_previous, 5);
        assert_ne!(link_key_4.verifying_key(), link_key_5.verifying_key());
    }

    /// The device signature, the anchor, and the link each sign the same
    /// canonical rotation under their own domain, so one never passes as
    /// another even when the same key makes both.
    #[test]
    fn rotation_signatures_are_domain_separated() {
        let key = SigningKey::generate(&mut os_rng());
        let genesis = sign_key_rotation(
            &key,
            sample_rotation(0),
            RotationProof::Genesis {
                recovery_signing_key: &key,
            },
        )
        .unwrap();
        assert_ne!(Some(genesis.signature), genesis.recovery_anchor);

        let mut anchor_as_device = genesis.clone();
        anchor_as_device.signature = genesis.recovery_anchor.unwrap();
        assert_eq!(
            verify_key_rotation(&key.verifying_key(), &anchor_as_device),
            Err(EnvelopeError::SignatureInvalid)
        );
        let mut device_as_anchor = genesis.clone();
        device_as_anchor.recovery_anchor = Some(genesis.signature);
        assert_eq!(
            verify_rotation_anchor(&key.verifying_key(), &device_as_anchor),
            Err(EnvelopeError::SignatureInvalid)
        );

        // A link made with a key that is also the initiator's device key.
        let k_previous = [3u8; 32];
        let link_key = crypto::derive_epoch_link_signing_key(&k_previous, 4);
        let successor = sign_key_rotation(
            &link_key,
            sample_rotation(5),
            RotationProof::Successor {
                previous_k_epoch: &k_previous,
            },
        )
        .unwrap();
        let mut link_as_device = successor.clone();
        link_as_device.signature = successor.epoch_link.unwrap();
        assert_eq!(
            verify_key_rotation(&link_key.verifying_key(), &link_as_device),
            Err(EnvelopeError::SignatureInvalid)
        );
        let mut device_as_link = successor.clone();
        device_as_link.epoch_link = Some(successor.signature);
        assert_eq!(
            verify_rotation_link(&k_previous, &device_as_link),
            Err(EnvelopeError::SignatureInvalid)
        );
    }

    #[test]
    fn each_epoch_requires_exactly_its_own_proof() {
        let initiator = SigningKey::generate(&mut os_rng());
        let recovery = SigningKey::generate(&mut os_rng());
        let k_previous = [7u8; 32];

        // Signing refuses the wrong proof for the epoch.
        assert_eq!(
            sign_key_rotation(
                &initiator,
                sample_rotation(3),
                RotationProof::Genesis {
                    recovery_signing_key: &recovery
                }
            )
            .map(|_| ()),
            Err(EnvelopeError::RotationProofMismatch)
        );
        assert_eq!(
            sign_key_rotation(
                &initiator,
                sample_rotation(0),
                RotationProof::Successor {
                    previous_k_epoch: &k_previous
                }
            )
            .map(|_| ()),
            Err(EnvelopeError::RotationProofMismatch)
        );

        let genesis = sign_key_rotation(
            &initiator,
            sample_rotation(0),
            RotationProof::Genesis {
                recovery_signing_key: &recovery,
            },
        )
        .unwrap();
        let successor = sign_key_rotation(
            &initiator,
            sample_rotation(1),
            RotationProof::Successor {
                previous_k_epoch: &k_previous,
            },
        )
        .unwrap();

        // Missing.
        let mut no_anchor = genesis.clone();
        no_anchor.recovery_anchor = None;
        let mut no_link = successor.clone();
        no_link.epoch_link = None;
        // Extra.
        let mut genesis_with_link = genesis.clone();
        genesis_with_link.epoch_link = successor.epoch_link;
        let mut successor_with_anchor = successor.clone();
        successor_with_anchor.recovery_anchor = genesis.recovery_anchor;
        // The wrong one for the epoch.
        let mut genesis_with_only_a_link = no_anchor.clone();
        genesis_with_only_a_link.epoch_link = successor.epoch_link;
        let mut successor_with_only_an_anchor = no_link.clone();
        successor_with_only_an_anchor.recovery_anchor = genesis.recovery_anchor;

        for bad in [&no_anchor, &genesis_with_link, &genesis_with_only_a_link] {
            assert_eq!(
                verify_rotation_anchor(&recovery.verifying_key(), bad),
                Err(EnvelopeError::RotationProofMismatch)
            );
            assert_eq!(
                verify_rotation_link(&k_previous, bad),
                Err(EnvelopeError::RotationProofMismatch)
            );
        }
        for bad in [
            &no_link,
            &successor_with_anchor,
            &successor_with_only_an_anchor,
        ] {
            assert_eq!(
                verify_rotation_link(&k_previous, bad),
                Err(EnvelopeError::RotationProofMismatch)
            );
            assert_eq!(
                verify_rotation_anchor(&recovery.verifying_key(), bad),
                Err(EnvelopeError::RotationProofMismatch)
            );
        }
        // Every shape still decodes and re-encodes byte for byte; only the
        // verifiers refuse it.
        for shape in [
            &no_anchor,
            &genesis_with_link,
            &no_link,
            &successor_with_anchor,
        ] {
            let bytes = encode_signed_key_rotation(shape).unwrap();
            let decoded = decode_signed_key_rotation(&bytes).unwrap();
            assert_eq!(&decoded, shape);
            assert_eq!(encode_signed_key_rotation(&decoded).unwrap(), bytes);
        }
    }

    /// The shape `SignedKeyRotation` had in protocol 3, before rotations
    /// carried an anchor or link.
    #[derive(Serialize)]
    struct ProtocolThreeSignedRotation {
        rotation: KeyRotation,
        signature: Signature,
    }

    #[test]
    fn a_protocol_3_rotation_decodes_byte_for_byte_but_proves_nothing() {
        let initiator = SigningKey::generate(&mut os_rng());
        for key_epoch in [0, 2] {
            let rotation = sample_rotation(key_epoch);
            let signature = Signature(crypto::sign_bytes(
                &initiator,
                ROTATION_SIGNATURE_DOMAIN,
                &canonical_dag_cbor(&rotation).unwrap(),
            ));
            let legacy_bytes = canonical_dag_cbor(&ProtocolThreeSignedRotation {
                rotation,
                signature,
            })
            .unwrap();
            let decoded = decode_signed_key_rotation(&legacy_bytes).unwrap();
            assert_eq!(decoded.recovery_anchor, None);
            assert_eq!(decoded.epoch_link, None);
            assert_eq!(encode_signed_key_rotation(&decoded).unwrap(), legacy_bytes);
            // Still self-consistent, so a connector holding it is seen to
            // hold a group...
            verify_key_rotation(&initiator.verifying_key(), &decoded).unwrap();
            // ...but neither proof accepts it.
            assert_eq!(
                verify_rotation_anchor(&initiator.verifying_key(), &decoded),
                Err(EnvelopeError::RotationProofMismatch)
            );
            assert_eq!(
                verify_rotation_link(&[7u8; 32], &decoded),
                Err(EnvelopeError::RotationProofMismatch)
            );
        }
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
