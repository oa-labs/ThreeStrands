//! Every numeric limit the IMAP body path enforces, in ONE place.
//!
//! `docs/imap-design.md` ("What gets synced" / "Bodies") and `AGENTS.md`
//! ("Email rendering") both require that numeric limits live in a single
//! policy module with below/at/above tests, following the frontend's
//! `src/emailRenderingPolicy.ts`. This is the Rust-side analogue for the IMAP
//! provider: the body cache's size ceiling, and the MIME-tree shape limits the
//! `rfc822` parser enforces before it will build a [`RawMessage`].
//!
//! The rule from `AGENTS.md` is "never silently clamp". Each limit here is
//! enforced by a helper that returns a typed [`ProviderError`] when the input
//! is over the limit, rather than truncating it. An over-limit message is
//! rejected whole; it is never half-ingested.
//!
//! The MIME-shape limits are deliberately independent of `mime.rs`'s own
//! `validate_mime_structure` limits (which guard the shared normalize path for
//! every provider). These are the IMAP ingest gate, applied to raw RFC 5322
//! bytes BEFORE a `RawMessage` is built. They are set no looser than the
//! shared ones so a message this gate accepts always survives `normalize`:
//! `MAX_MIME_PART_COUNT` (4096) is wider than `mime::MAX_MIME_PARTS` (1000),
//! which means an IMAP tree between those two bounds is accepted here and then
//! rejected by `normalize` with its own typed error — rejection either way,
//! never a silent clamp. `MAX_MIME_DEPTH` matches the shared limit exactly.

use crate::provider::ProviderError;

/// Largest raw message, in bytes, accepted into the on-disk body cache.
///
/// A `BODY.PEEK[]` fetch of a message bigger than this is refused rather than
/// cached: 64 MiB comfortably exceeds any ordinary mail (RFC 822 size plus
/// attachments) while bounding how large a single cache row — and the memory
/// held while writing it — can grow. The shared normalize path has its own,
/// smaller decoded-body and attachment ceilings; this is the gate on the raw
/// bytes before either runs.
pub const MAX_RAW_MESSAGE_BYTES: usize = 64 * 1024 * 1024;

/// Deepest MIME nesting the `rfc822` parser will build a tree for.
///
/// Matches `mime::MAX_MIME_DEPTH` so a tree this gate accepts is never then
/// rejected for depth by `normalize`. A `message/rfc822` part counts as one
/// level, so a mail-bomb of deeply nested forwarded messages is refused here.
pub const MAX_MIME_DEPTH: usize = 32;

/// Most MIME parts the `rfc822` parser will accept in one message.
///
/// A hostile message can declare thousands of empty parts to exhaust memory.
/// 4096 is generous for legitimate mail (even a large multipart/mixed with
/// many attachments and alternatives stays well under it) while bounding the
/// part vector. It is intentionally wider than `mime::MAX_MIME_PARTS` (1000):
/// the shared normalize path applies the tighter bound afterwards, so a tree
/// between the two is still rejected — by `normalize`, with its own error —
/// never silently clamped.
pub const MAX_MIME_PART_COUNT: usize = 4096;

/// Reject a raw message that is too large to cache. Returns the byte count
/// unchanged when it is within the limit (including exactly at it), so callers
/// read "ok, this many bytes" from the success value.
pub fn check_raw_message_bytes(len: usize) -> Result<usize, ProviderError> {
    if len > MAX_RAW_MESSAGE_BYTES {
        return Err(ProviderError::PermanentClientRejection(format!(
            "message is {len} bytes, over the {MAX_RAW_MESSAGE_BYTES}-byte ({} MiB) body-cache limit",
            MAX_RAW_MESSAGE_BYTES / 1024 / 1024
        )));
    }
    Ok(len)
}

/// Reject a MIME tree nested deeper than [`MAX_MIME_DEPTH`]. `depth` is the
/// deepest level reached (root = 1).
pub fn check_mime_depth(depth: usize) -> Result<usize, ProviderError> {
    if depth > MAX_MIME_DEPTH {
        return Err(ProviderError::PermanentClientRejection(format!(
            "MIME nesting is {depth} levels deep, over the {MAX_MIME_DEPTH}-level limit"
        )));
    }
    Ok(depth)
}

/// Reject a MIME tree with more than [`MAX_MIME_PART_COUNT`] parts.
pub fn check_mime_part_count(count: usize) -> Result<usize, ProviderError> {
    if count > MAX_MIME_PART_COUNT {
        return Err(ProviderError::PermanentClientRejection(format!(
            "message declares {count} MIME parts, over the {MAX_MIME_PART_COUNT}-part limit"
        )));
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Each limit is tested just below, exactly at, and just above the
    // boundary, per AGENTS.md "Email rendering" and the emailRenderingPolicy.ts
    // pattern. The boundary itself is accepted; one past it is a typed error,
    // not a clamp.

    #[test]
    fn raw_message_bytes_accepts_up_to_the_limit_and_rejects_above_it() {
        assert_eq!(check_raw_message_bytes(0).unwrap(), 0);
        assert_eq!(
            check_raw_message_bytes(MAX_RAW_MESSAGE_BYTES - 1).unwrap(),
            MAX_RAW_MESSAGE_BYTES - 1
        );
        assert_eq!(
            check_raw_message_bytes(MAX_RAW_MESSAGE_BYTES).unwrap(),
            MAX_RAW_MESSAGE_BYTES
        );
        let err = check_raw_message_bytes(MAX_RAW_MESSAGE_BYTES + 1).unwrap_err();
        assert!(
            matches!(err, ProviderError::PermanentClientRejection(_)),
            "over-limit is a typed rejection, not a clamp: {err:?}"
        );
    }

    #[test]
    fn mime_depth_accepts_up_to_the_limit_and_rejects_above_it() {
        assert_eq!(check_mime_depth(MAX_MIME_DEPTH - 1).unwrap(), MAX_MIME_DEPTH - 1);
        assert_eq!(check_mime_depth(MAX_MIME_DEPTH).unwrap(), MAX_MIME_DEPTH);
        assert!(matches!(
            check_mime_depth(MAX_MIME_DEPTH + 1).unwrap_err(),
            ProviderError::PermanentClientRejection(_)
        ));
    }

    #[test]
    fn mime_part_count_accepts_up_to_the_limit_and_rejects_above_it() {
        assert_eq!(
            check_mime_part_count(MAX_MIME_PART_COUNT - 1).unwrap(),
            MAX_MIME_PART_COUNT - 1
        );
        assert_eq!(
            check_mime_part_count(MAX_MIME_PART_COUNT).unwrap(),
            MAX_MIME_PART_COUNT
        );
        assert!(matches!(
            check_mime_part_count(MAX_MIME_PART_COUNT + 1).unwrap_err(),
            ProviderError::PermanentClientRejection(_)
        ));
    }

    // The IMAP ingest limits are never looser than the shared normalize-path
    // limits in a way that would let a message pass ingest and then be rejected
    // for DEPTH downstream: depth matches exactly, and the part gate is only
    // ever wider (so normalize, not a silent clamp, is what rejects the gap).
    #[test]
    fn ingest_depth_limit_matches_the_shared_normalize_limit() {
        assert_eq!(MAX_MIME_DEPTH, crate::mime::MAX_MIME_DEPTH);
        assert!(MAX_MIME_PART_COUNT >= crate::mime::MAX_MIME_PARTS);
    }
}
