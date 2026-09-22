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

/// Maximum number of [`crate::FieldOperation`] entries in a single event.
pub const MAX_OPERATIONS_PER_EVENT: usize = 2_000;

/// Maximum number of distinct entities named across the operations of one
/// event.
pub const MAX_ENTITIES_PER_EVENT: usize = 500;

/// Maximum number of distinct fields touched on a single entity within one
/// event.
pub const MAX_FIELDS_PER_ENTITY_PER_EVENT: usize = 200;

/// Maximum number of parent operation ids a single operation may declare.
pub const MAX_PARENTS_PER_OPERATION: usize = 64;

/// Maximum byte length of an entity id.
pub const MAX_ENTITY_ID_BYTES: usize = 320;

/// Maximum byte length of a field name.
pub const MAX_FIELD_NAME_BYTES: usize = 200;

/// Maximum serialized byte length of a single operation's value.
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
