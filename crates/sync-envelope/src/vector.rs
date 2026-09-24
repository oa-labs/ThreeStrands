//! Per-device sequence vectors: for each device, the highest write counter
//! seen from it. A replica snapshot carries one as its causal context.

use serde::{Deserialize, Serialize};

use crate::error::EnvelopeError;
use crate::ids::DeviceId;
use crate::limits::MAX_SEQUENCE_VECTOR_ENTRIES;

/// One device's position in a sequence vector: every write `1..=sequence`
/// from `device_id`. A device absent from a vector is at sequence 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SequenceEntry {
    pub device_id: DeviceId,
    pub sequence: u64,
}

/// A vector must list distinct devices in ascending id order, each at a
/// sequence of at least 1 (zero is expressed by omission), within
/// [`MAX_SEQUENCE_VECTOR_ENTRIES`]. One canonical form per vector keeps
/// signatures and comparisons unambiguous.
pub fn validate_sequence_vector(entries: &[SequenceEntry]) -> Result<(), EnvelopeError> {
    if entries.len() > MAX_SEQUENCE_VECTOR_ENTRIES {
        return Err(EnvelopeError::LimitExceeded("sequence vector size"));
    }
    let mut previous: Option<&DeviceId> = None;
    for entry in entries {
        if entry.sequence == 0 || previous.is_some_and(|previous| entry.device_id <= *previous) {
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
        assert!(validate_sequence_vector(&[]).is_ok());
        assert!(validate_sequence_vector(&[entry(1, 3), entry(2, 1)]).is_ok());
    }

    #[test]
    fn rejects_disorder_duplicates_and_zero() {
        assert!(validate_sequence_vector(&[entry(2, 1), entry(1, 1)]).is_err());
        assert!(validate_sequence_vector(&[entry(1, 1), entry(1, 2)]).is_err());
        assert!(validate_sequence_vector(&[entry(1, 0)]).is_err());
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
        assert!(validate_sequence_vector(&vector(MAX_SEQUENCE_VECTOR_ENTRIES - 1)).is_ok());
        assert!(validate_sequence_vector(&vector(MAX_SEQUENCE_VECTOR_ENTRIES)).is_ok());
        assert!(matches!(
            validate_sequence_vector(&vector(MAX_SEQUENCE_VECTOR_ENTRIES + 1)),
            Err(EnvelopeError::LimitExceeded("sequence vector size"))
        ));
    }
}
