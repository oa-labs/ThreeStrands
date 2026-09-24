//! Per-device sequence vectors: a compact "how far along each device's
//! append-only feed" summary. A device head carries one as its `ack` (its
//! causally closed progress), and every event carries one as its
//! `causal_vector` (how much of every other device's feed its author had
//! applied when sealing it), so readers can tell which events an event may
//! depend on without walking operation parents.

use serde::{Deserialize, Serialize};

use crate::error::EnvelopeError;
use crate::ids::DeviceId;
use crate::limits::MAX_SEQUENCE_VECTOR_ENTRIES;

/// One device's position in a sequence vector: every event `1..=sequence`
/// from `device_id`. A device absent from a vector is at sequence 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SequenceEntry {
    pub device_id: DeviceId,
    pub sequence: u64,
}

/// A vector must list distinct devices in ascending id order, each at a
/// sequence of at least 1 (zero is expressed by omission), within
/// [`MAX_SEQUENCE_VECTOR_ENTRIES`], and never name `excluded` (an event's
/// own author, whose position its `device_sequence` already states). One
/// canonical form per vector keeps signatures and comparisons unambiguous.
pub fn validate_sequence_vector(entries: &[SequenceEntry], excluded: Option<&DeviceId>) -> Result<(), EnvelopeError> {
    if entries.len() > MAX_SEQUENCE_VECTOR_ENTRIES {
        return Err(EnvelopeError::LimitExceeded("sequence vector size"));
    }
    let mut previous: Option<&DeviceId> = None;
    for entry in entries {
        if entry.sequence == 0
            || previous.is_some_and(|previous| entry.device_id <= *previous)
            || excluded.is_some_and(|excluded| entry.device_id == *excluded)
        {
            return Err(EnvelopeError::LimitExceeded("sequence vector order"));
        }
        previous = Some(&entry.device_id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(byte: u8, sequence: u64) -> SequenceEntry {
        SequenceEntry { device_id: DeviceId::from_bytes([byte; 16]), sequence }
    }

    #[test]
    fn accepts_the_canonical_form() {
        assert!(validate_sequence_vector(&[], None).is_ok());
        assert!(validate_sequence_vector(&[entry(1, 3), entry(2, 1)], None).is_ok());
    }

    #[test]
    fn rejects_disorder_duplicates_zero_and_the_excluded_device() {
        assert!(validate_sequence_vector(&[entry(2, 1), entry(1, 1)], None).is_err());
        assert!(validate_sequence_vector(&[entry(1, 1), entry(1, 2)], None).is_err());
        assert!(validate_sequence_vector(&[entry(1, 0)], None).is_err());
        let author = DeviceId::from_bytes([1; 16]);
        assert!(validate_sequence_vector(&[entry(1, 1)], Some(&author)).is_err());
        assert!(validate_sequence_vector(&[entry(2, 1)], Some(&author)).is_ok());
    }

    #[test]
    fn is_limited_in_size() {
        let vector = |count: usize| -> Vec<SequenceEntry> {
            (0..count)
                .map(|index| {
                    let mut bytes = [0u8; 16];
                    bytes[..8].copy_from_slice(&(index as u64).to_be_bytes());
                    SequenceEntry { device_id: DeviceId::from_bytes(bytes), sequence: 1 }
                })
                .collect()
        };
        assert!(validate_sequence_vector(&vector(MAX_SEQUENCE_VECTOR_ENTRIES - 1), None).is_ok());
        assert!(validate_sequence_vector(&vector(MAX_SEQUENCE_VECTOR_ENTRIES), None).is_ok());
        assert!(matches!(
            validate_sequence_vector(&vector(MAX_SEQUENCE_VECTOR_ENTRIES + 1), None),
            Err(EnvelopeError::LimitExceeded("sequence vector size"))
        ));
    }
}
