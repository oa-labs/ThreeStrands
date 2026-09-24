#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum EnvelopeError {
    #[error("envelope header is malformed")]
    HeaderInvalid,
    #[error("unsupported envelope format version")]
    UnsupportedFormatVersion,
    #[error("unsupported sync protocol version")]
    UnsupportedProtocolVersion,
    #[error("sealed object is not of the expected kind")]
    UnexpectedObjectKind,
    #[error("envelope is too short to contain a header, nonce, and authentication tag")]
    Truncated,
    #[error("envelope authentication failed")]
    DecryptionFailed,
    #[error("signature is invalid")]
    SignatureInvalid,
    #[error("compressed body exceeds the configured limit")]
    CompressedTooLarge,
    #[error("decompressed body exceeds the configured limit")]
    DecompressedTooLarge,
    #[error("reassembled message exceeds the configured limit")]
    ReassembledTooLarge,
    #[error("failed to compress the canonical body")]
    CompressionFailed,
    #[error("failed to decompress the chunk body")]
    DecompressionFailed,
    #[error("message exceeds the configured chunk limit")]
    TooManyChunks,
    #[error("a single chunk exceeds the maximum supported chunk size")]
    ChunkTooLarge,
    #[error("chunk belongs to a different message")]
    CrossMessageChunk,
    #[error("chunk set is incomplete or inconsistent")]
    IncompleteMessage,
    #[error("reassembled message does not match its authenticated hash")]
    MessageHashMismatch,
    #[error("envelope body is malformed")]
    Malformed,
    #[error("event exceeds a hard protocol limit: {0}")]
    LimitExceeded(&'static str),
    #[error("failed to encode the canonical body")]
    EncodingFailed,
    #[error("failed to decode the canonical body")]
    DecodingFailed,
    #[error("a CID-referencing field is not a valid CIDv1 string")]
    InvalidCidReference,
}
