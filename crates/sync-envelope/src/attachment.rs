//! Note attachment parts: a file's bytes, sealed apart from any snapshot.
//!
//! A snapshot is resealed and uploaded whole on every change, so files never
//! ride in one. Each file is split into parts of at most
//! [`MAX_ATTACHMENT_PART_BYTES`], and each part is sealed as its own
//! [`ObjectKind::AttachmentPart`] message: chunked, padded, encrypted, and
//! signed by the device that sealed it, under the attachment-part signature
//! domain. The snapshot's attachment record names the parts, so a part is
//! only ever read through a record its author's snapshot vouches for.

use serde::{Deserialize, Serialize};

use crate::error::EnvelopeError;
use crate::header::MESSAGE_ID_LEN;
use crate::limits::{MAX_ATTACHMENT_PARTS, MAX_ATTACHMENT_PART_BYTES};
use crate::snapshot::PROTOCOL_VERSION;
use crate::{crypto, open_object, seal_object, ObjectKind, OpenParams, SealParams, SealedMessage};

/// One part of one attachment's file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentPart {
    pub protocol_version: u16,
    /// The attachment's 16-byte id.
    #[serde(with = "serde_bytes")]
    pub attachment_id: Vec<u8>,
    /// This part's position, from 0.
    pub part: u32,
    /// How many parts the file has.
    pub parts: u32,
    #[serde(with = "serde_bytes")]
    pub bytes: Vec<u8>,
}

fn validate(part: &AttachmentPart) -> Result<(), EnvelopeError> {
    if part.protocol_version != PROTOCOL_VERSION
        || part.attachment_id.len() != 16
        || part.parts == 0
        || part.parts > MAX_ATTACHMENT_PARTS
        || part.part >= part.parts
        || part.bytes.len() > MAX_ATTACHMENT_PART_BYTES
    {
        return Err(EnvelopeError::Malformed);
    }
    Ok(())
}

/// A part's message id, derived from its attachment and position, so a
/// part can't be passed off as another.
pub fn message_id_for_attachment_part(attachment_id: &[u8], part: u32) -> [u8; MESSAGE_ID_LEN] {
    use sha2::{Digest, Sha256};
    const DOMAIN: &[u8] = b"threestrands/sync-envelope/attachment-part-message-id/v3";
    let mut hasher = Sha256::new();
    hasher.update(DOMAIN);
    hasher.update(attachment_id);
    hasher.update(part.to_be_bytes());
    let digest = hasher.finalize();
    let mut id = [0u8; MESSAGE_ID_LEN];
    id.copy_from_slice(&digest[..MESSAGE_ID_LEN]);
    id
}

/// Validates, signs, and seals one attachment part with fresh nonces.
pub fn seal_attachment_part(
    part: &AttachmentPart,
    params: &SealParams,
) -> Result<SealedMessage, EnvelopeError> {
    if params.object_kind != ObjectKind::AttachmentPart {
        return Err(EnvelopeError::UnexpectedObjectKind);
    }
    validate(part)?;
    let message_id = message_id_for_attachment_part(&part.attachment_id, part.part);
    let (_, sealed) = seal_object(part, message_id, params, crypto::random_nonce)?;
    Ok(sealed)
}

/// Opens one part's complete chunk set: everything [`open_object`] checks,
/// plus the part's own contract and that its message id matches its
/// attachment and position.
pub fn open_attachment_part(
    chunks: &[Vec<u8>],
    params: &OpenParams,
) -> Result<AttachmentPart, EnvelopeError> {
    let (part, _, message_id) =
        open_object::<AttachmentPart>(chunks, params, ObjectKind::AttachmentPart)?;
    validate(&part)?;
    if message_id != message_id_for_attachment_part(&part.attachment_id, part.part) {
        return Err(EnvelopeError::Malformed);
    }
    Ok(part)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{open_snapshot, SigningKey};

    const SPACE: &[u8] = b"space";
    const KEY: [u8; 32] = [7; 32];

    fn signer() -> SigningKey {
        SigningKey::from_bytes(&[3; 32])
    }

    fn part(bytes: Vec<u8>) -> AttachmentPart {
        AttachmentPart {
            protocol_version: PROTOCOL_VERSION,
            attachment_id: vec![1; 16],
            part: 0,
            parts: 1,
            bytes,
        }
    }

    fn seal(part: &AttachmentPart, kind: ObjectKind) -> Result<SealedMessage, EnvelopeError> {
        let key = signer();
        seal_attachment_part(
            part,
            &SealParams {
                sync_space_id: SPACE,
                k_epoch: &KEY,
                key_epoch: 1,
                object_kind: kind,
                signing_key: &key,
            },
        )
    }

    fn open_params<'a>(key: &'a crate::VerifyingKey) -> OpenParams<'a> {
        OpenParams {
            sync_space_id: SPACE,
            k_epoch: &KEY,
            key_epoch: 1,
            verifying_key: key,
        }
    }

    #[test]
    fn a_part_round_trips_at_the_largest_size_and_refuses_one_byte_more() {
        // Random-looking bytes don't compress, as with most real files.
        let mut state = 0x9e3779b97f4a7c15u64;
        let noise: Vec<u8> = (0..MAX_ATTACHMENT_PART_BYTES + 1)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect();
        let at_limit = part(noise[..MAX_ATTACHMENT_PART_BYTES].to_vec());
        let sealed = seal(&at_limit, ObjectKind::AttachmentPart).unwrap();
        let verifying = signer().verifying_key();
        let opened = open_attachment_part(&sealed.chunks, &open_params(&verifying)).unwrap();
        assert_eq!(opened, at_limit);
        let below = part(noise[..MAX_ATTACHMENT_PART_BYTES - 1].to_vec());
        assert!(seal(&below, ObjectKind::AttachmentPart).is_ok());
        assert!(matches!(
            seal(&part(noise), ObjectKind::AttachmentPart),
            Err(EnvelopeError::Malformed)
        ));
    }

    #[test]
    fn a_part_opens_only_as_a_part_from_its_signer() {
        let sealed = seal(&part(b"hello".to_vec()), ObjectKind::AttachmentPart).unwrap();
        let verifying = signer().verifying_key();
        assert!(open_snapshot(&sealed.chunks, &open_params(&verifying)).is_err());
        let stranger = SigningKey::from_bytes(&[4; 32]).verifying_key();
        assert!(open_attachment_part(&sealed.chunks, &open_params(&stranger)).is_err());
        assert!(matches!(
            seal(&part(b"hello".to_vec()), ObjectKind::Snapshot),
            Err(EnvelopeError::UnexpectedObjectKind)
        ));
    }

    #[test]
    fn part_positions_and_counts_are_bounded() {
        for (index, parts, ok) in [
            (0, 1, true),
            (MAX_ATTACHMENT_PARTS - 1, MAX_ATTACHMENT_PARTS, true),
            (1, 1, false),
            (0, 0, false),
            (0, MAX_ATTACHMENT_PARTS + 1, false),
        ] {
            let mut candidate = part(b"x".to_vec());
            candidate.part = index;
            candidate.parts = parts;
            assert_eq!(
                seal(&candidate, ObjectKind::AttachmentPart).is_ok(),
                ok,
                "{index}/{parts}"
            );
        }
        let mut short_id = part(b"x".to_vec());
        short_id.attachment_id = vec![1; 15];
        assert!(seal(&short_id, ObjectKind::AttachmentPart).is_err());
    }
}
