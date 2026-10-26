//! Shared validation before a content identifier becomes a transport path/key.
use threestrands_sync_transport::TransportError;

/// Preserve the transport contract: 8–256 ASCII alphanumeric bytes only.
/// Separators, escapes, Unicode, and control bytes must never reach path APIs.
pub(crate) fn sanitize_cid(cid: &str) -> Result<&str, TransportError> {
    if cid.len() < 8 || cid.len() > 256 || !cid.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(TransportError::Permanent(
            "invalid content identifier".to_string(),
        ));
    }
    Ok(cid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_length_boundaries_and_rejects_unsafe_paths() {
        for length in [8, 9, 255, 256] {
            assert!(sanitize_cid(&"a".repeat(length)).is_ok());
        }
        for length in [0, 7, 257] {
            assert!(matches!(
                sanitize_cid(&"a".repeat(length)),
                Err(TransportError::Permanent(_))
            ));
        }
        for cid in [
            "../abcdef",
            "/abcdefgh",
            "abcd/efgh",
            "abcd\\efgh",
            "abcd%2efgh",
            "abcdefgh\0",
            "abcdéefgh",
            "abcd efgh",
            "abcd\nefgh",
        ] {
            assert!(sanitize_cid(cid).is_err(), "{cid:?}");
        }
        assert_eq!(sanitize_cid("bABC0123456789").unwrap(), "bABC0123456789");
    }
}
