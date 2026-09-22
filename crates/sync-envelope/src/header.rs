//! The 20-byte authenticated envelope header and its associated data.

use crate::error::EnvelopeError;

pub const MAGIC: u8 = 0xE5;
pub const FORMAT_VERSION: u8 = 1;

pub const HEADER_LEN: usize = 20;
pub const NONCE_LEN: usize = 24;
pub const MESSAGE_ID_LEN: usize = 8;
pub const TAG_LEN: usize = 16;

/// Fixed protocol domain mixed into every envelope's associated data,
/// alongside the stable sync-space id. Provider account ids, transport
/// object ids, and other transport metadata are deliberately excluded so
/// identical encrypted objects can be replicated byte-for-byte across
/// transports without re-encryption.
pub const AAD_DOMAIN: &[u8] = b"threestrands/sync-envelope/aad/v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ObjectKind {
    Operations = 1,
    Snapshot = 2,
    Enrollment = 3,
    KeyRotation = 4,
}

impl ObjectKind {
    fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Operations),
            2 => Some(Self::Snapshot),
            3 => Some(Self::Enrollment),
            4 => Some(Self::KeyRotation),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CipherSuite {
    XChaCha20Poly1305Hkdf = 1,
}

impl CipherSuite {
    fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::XChaCha20Poly1305Hkdf),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnvelopeHeader {
    pub object_kind: ObjectKind,
    pub cipher_suite: CipherSuite,
    pub key_epoch: u32,
    pub chunk_index: u16,
    pub chunk_count: u16,
    pub message_id: [u8; MESSAGE_ID_LEN],
}

impl EnvelopeHeader {
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut out = [0u8; HEADER_LEN];
        out[0] = MAGIC;
        out[1] = FORMAT_VERSION;
        out[2] = self.object_kind as u8;
        out[3] = self.cipher_suite as u8;
        out[4..8].copy_from_slice(&self.key_epoch.to_be_bytes());
        out[8..10].copy_from_slice(&self.chunk_index.to_be_bytes());
        out[10..12].copy_from_slice(&self.chunk_count.to_be_bytes());
        out[12..20].copy_from_slice(&self.message_id);
        out
    }

    pub fn decode(bytes: &[u8; HEADER_LEN]) -> Result<Self, EnvelopeError> {
        if bytes[0] != MAGIC {
            return Err(EnvelopeError::HeaderInvalid);
        }
        if bytes[1] != FORMAT_VERSION {
            return Err(EnvelopeError::UnsupportedFormatVersion);
        }
        let object_kind = ObjectKind::from_u8(bytes[2]).ok_or(EnvelopeError::HeaderInvalid)?;
        let cipher_suite = CipherSuite::from_u8(bytes[3]).ok_or(EnvelopeError::HeaderInvalid)?;
        let key_epoch = u32::from_be_bytes(bytes[4..8].try_into().unwrap());
        let chunk_index = u16::from_be_bytes(bytes[8..10].try_into().unwrap());
        let chunk_count = u16::from_be_bytes(bytes[10..12].try_into().unwrap());
        if chunk_count == 0 || chunk_index >= chunk_count {
            return Err(EnvelopeError::HeaderInvalid);
        }
        let mut message_id = [0u8; MESSAGE_ID_LEN];
        message_id.copy_from_slice(&bytes[12..20]);
        Ok(Self {
            object_kind,
            cipher_suite,
            key_epoch,
            chunk_index,
            chunk_count,
            message_id,
        })
    }
}

/// Builds the AEAD associated data for one chunk: the complete authenticated
/// header, followed by the fixed protocol domain, followed by the stable
/// sync-space id.
pub fn build_aad(header_bytes: &[u8; HEADER_LEN], sync_space_id: &[u8]) -> Vec<u8> {
    let mut aad = Vec::with_capacity(HEADER_LEN + AAD_DOMAIN.len() + sync_space_id.len());
    aad.extend_from_slice(header_bytes);
    aad.extend_from_slice(AAD_DOMAIN);
    aad.extend_from_slice(sync_space_id);
    aad
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> EnvelopeHeader {
        EnvelopeHeader {
            object_kind: ObjectKind::Operations,
            cipher_suite: CipherSuite::XChaCha20Poly1305Hkdf,
            key_epoch: 7,
            chunk_index: 1,
            chunk_count: 3,
            message_id: [1, 2, 3, 4, 5, 6, 7, 8],
        }
    }

    #[test]
    fn round_trips() {
        let header = sample();
        let encoded = header.encode();
        assert_eq!(encoded.len(), HEADER_LEN);
        let decoded = EnvelopeHeader::decode(&encoded).unwrap();
        assert_eq!(decoded, header);
    }

    #[test]
    fn rejects_wrong_magic() {
        let mut encoded = sample().encode();
        encoded[0] ^= 0xFF;
        assert_eq!(
            EnvelopeHeader::decode(&encoded),
            Err(EnvelopeError::HeaderInvalid)
        );
    }

    #[test]
    fn rejects_unsupported_format_version() {
        let mut encoded = sample().encode();
        encoded[1] = 99;
        assert_eq!(
            EnvelopeHeader::decode(&encoded),
            Err(EnvelopeError::UnsupportedFormatVersion)
        );
    }

    #[test]
    fn rejects_unknown_cipher_suite() {
        let mut encoded = sample().encode();
        encoded[3] = 99;
        assert_eq!(
            EnvelopeHeader::decode(&encoded),
            Err(EnvelopeError::HeaderInvalid)
        );
    }

    #[test]
    fn rejects_chunk_index_out_of_range() {
        let mut encoded = sample().encode();
        // chunk_count stays 3, set chunk_index to 3 (out of range).
        encoded[8..10].copy_from_slice(&3u16.to_be_bytes());
        assert_eq!(
            EnvelopeHeader::decode(&encoded),
            Err(EnvelopeError::HeaderInvalid)
        );
    }

    #[test]
    fn aad_changes_with_sync_space_id() {
        let header = sample().encode();
        let a = build_aad(&header, b"space-a");
        let b = build_aad(&header, b"space-b");
        assert_ne!(a, b);
    }
}
