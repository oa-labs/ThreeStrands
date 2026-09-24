//! Hard protocol limits for the sync envelope format.
//!
//! Every numeric limit that guards the wire format lives here so a single
//! file answers "what is the largest X we accept" for the whole crate. Do
//! not duplicate or silently re-derive these bounds elsewhere.

/// Largest zstd-compressed body accepted for one logical message, before
/// chunking. Bounds both the encoder's output and the decoder's expectation.
pub const MAX_COMPRESSED_BYTES: usize = 4 * 1024 * 1024;

/// Largest decompressed canonical body accepted for one logical message.
/// Enforced against both the declared length and the actual decompressed
/// byte count, so a lying header cannot be used to skip the check.
pub const MAX_DECOMPRESSED_BYTES: usize = 32 * 1024 * 1024;

/// Largest total byte count accepted after concatenating every chunk's
/// payload back together, independent of the compressed/decompressed
/// limits above. This is the guard against a message that stays under the
/// per-field limits but chunks itself into an unreasonable total.
pub const MAX_REASSEMBLED_BYTES: usize = MAX_COMPRESSED_BYTES + 4_096;

/// Most fields one replica snapshot may hold. A snapshot is a device's
/// whole synchronized state (tasks, snippets, preferences, and so on), so
/// this bounds how large that state can grow; the message size limits
/// above bound it in bytes too.
pub const MAX_SNAPSHOT_FIELDS: usize = 250_000;

/// Most values one field may hold at once: one, plus every concurrent value
/// still in conflict with it.
pub const MAX_VALUES_PER_FIELD: usize = 64;

/// Maximum byte length of an entity id.
pub const MAX_ENTITY_ID_BYTES: usize = 320;

/// Maximum byte length of a field name.
pub const MAX_FIELD_NAME_BYTES: usize = 200;

/// Maximum serialized byte length of a single field value.
pub const MAX_VALUE_BYTES: usize = 64 * 1024;

/// Maximum number of chunks a single logical message may be split into.
pub const MAX_CHUNKS_PER_MESSAGE: usize = 4_096;

/// Ceiling for a single chunk's padded plaintext size. Also the largest
/// entry in [`PADDING_BUCKETS`].
pub const MAX_CHUNK_PLAINTEXT_BYTES: usize = 256 * 1024;

/// Padding buckets a chunk's plaintext is rounded up into, ascending and
/// terminating at [`MAX_CHUNK_PLAINTEXT_BYTES`]. Padding hides the exact
/// content length from anyone observing object sizes in a transport.
pub const PADDING_BUCKETS: &[usize] = &[4_096, 16_384, 65_536, MAX_CHUNK_PLAINTEXT_BYTES];

/// Most devices one snapshot's causal context may name. Far above any real
/// sync group; bounds the work a hostile snapshot can cause.
pub const MAX_SEQUENCE_VECTOR_ENTRIES: usize = 1_024;

/// Most earlier-epoch keys one enrollment grant or invitation may carry.
/// Every join code rotates the epoch, so this bounds how many rotations a
/// group can accumulate before a new device can no longer be handed its
/// whole history; about 100 bytes per entry.
pub const MAX_EARLIER_EPOCH_KEYS: usize = 1_024;

/// Longest join code text accepted, after whitespace is removed. Far above
/// a real code (a few connectors with credentials is well under 4 KiB);
/// bounds the work a pasted blob can cause.
pub const MAX_JOIN_CODE_CHARS: usize = 64 * 1024;

/// Most connectors one join code may carry.
pub const MAX_JOIN_CONNECTORS: usize = 8;

/// Longest inviter or joining-device name carried by a join code or
/// redemption, in characters. Matches the app's device-name limit.
pub const MAX_JOIN_NAME_CHARS: usize = 60;

/// Longest connector kind tag in a join code, in bytes.
pub const MAX_JOIN_CONNECTOR_KIND_BYTES: usize = 32;

/// Longest non-secret connector config (JSON) in a join code, in bytes.
pub const MAX_JOIN_CONNECTOR_CONFIG_BYTES: usize = 4 * 1024;

/// Longest connector secret (JSON) in a join code, in bytes. Room for an
/// S3 session token.
pub const MAX_JOIN_CONNECTOR_SECRETS_BYTES: usize = 12 * 1024;
