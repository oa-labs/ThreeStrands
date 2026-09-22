//! Compression, chunk splitting, and chunk reassembly.
//!
//! A logical message's plaintext is built once as:
//!
//! ```text
//! compressed_length u32be
//! uncompressed_length u32be
//! zstd(canonical_cbor_body)
//! ```
//!
//! That "full plaintext" is hashed, then split into one or more chunk
//! payloads. Every chunk's own plaintext (the bytes that get padded and
//! AEAD-encrypted) is:
//!
//! ```text
//! message_hash (32 bytes, sha256 of the complete unchunked full plaintext)
//! chunk_payload_length u32be
//! chunk_payload
//! zero padding to a bucket boundary
//! ```
//!
//! Chunking always runs, even when a message fits in one chunk, so there is
//! a single code path to test and no separate "unchunked" wire format.

use std::io::Read;

use sha2::{Digest, Sha256};

use crate::error::EnvelopeError;
use crate::limits::*;

pub const MESSAGE_HASH_LEN: usize = 32;
const CHUNK_LEN_PREFIX: usize = 4;

/// Compresses `canonical_body` and prefixes it with its compressed and
/// uncompressed lengths. This is the plaintext that gets hashed and then
/// split into chunks.
pub fn build_full_plaintext(canonical_body: &[u8]) -> Result<Vec<u8>, EnvelopeError> {
    if canonical_body.len() > MAX_DECOMPRESSED_BYTES {
        return Err(EnvelopeError::DecompressedTooLarge);
    }
    let compressed = zstd::stream::encode_all(canonical_body, 0)
        .map_err(|_| EnvelopeError::CompressionFailed)?;
    if compressed.len() > MAX_COMPRESSED_BYTES {
        return Err(EnvelopeError::CompressedTooLarge);
    }
    let mut out = Vec::with_capacity(8 + compressed.len());
    out.extend_from_slice(&(compressed.len() as u32).to_be_bytes());
    out.extend_from_slice(&(canonical_body.len() as u32).to_be_bytes());
    out.extend_from_slice(&compressed);
    Ok(out)
}

/// Reverses [`build_full_plaintext`]: validates the declared lengths against
/// the hard limits, decompresses with a hard cap so a lying or malicious
/// length cannot trigger a decompression bomb, and confirms the actual
/// decompressed length matches what was declared.
pub fn parse_full_plaintext(bytes: &[u8]) -> Result<Vec<u8>, EnvelopeError> {
    if bytes.len() < 8 {
        return Err(EnvelopeError::Malformed);
    }
    let compressed_length = u32::from_be_bytes(bytes[0..4].try_into().unwrap()) as usize;
    let uncompressed_length = u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize;
    if compressed_length > MAX_COMPRESSED_BYTES {
        return Err(EnvelopeError::CompressedTooLarge);
    }
    if uncompressed_length > MAX_DECOMPRESSED_BYTES {
        return Err(EnvelopeError::DecompressedTooLarge);
    }
    let compressed = bytes
        .get(8..8 + compressed_length)
        .ok_or(EnvelopeError::Malformed)?;
    if bytes.len() != 8 + compressed_length {
        return Err(EnvelopeError::Malformed);
    }

    let decoder =
        zstd::stream::Decoder::new(compressed).map_err(|_| EnvelopeError::DecompressionFailed)?;
    // Cap the reader so a length that lies small cannot still cause a
    // runaway decompression before the mismatch is detected below.
    let mut limited = decoder.take(MAX_DECOMPRESSED_BYTES as u64 + 1);
    let mut decompressed = Vec::new();
    limited
        .read_to_end(&mut decompressed)
        .map_err(|_| EnvelopeError::DecompressionFailed)?;
    if decompressed.len() > MAX_DECOMPRESSED_BYTES || decompressed.len() != uncompressed_length {
        return Err(EnvelopeError::DecompressedTooLarge);
    }
    Ok(decompressed)
}

/// Splits a full plaintext into one or more padded chunk plaintexts.
pub fn split_into_chunks(full_plaintext: &[u8]) -> Result<Vec<Vec<u8>>, EnvelopeError> {
    let message_hash: [u8; MESSAGE_HASH_LEN] = Sha256::digest(full_plaintext).into();
    let payload_capacity = MAX_CHUNK_PLAINTEXT_BYTES - MESSAGE_HASH_LEN - CHUNK_LEN_PREFIX;
    let chunk_count = full_plaintext.len().div_ceil(payload_capacity).max(1);
    if chunk_count > MAX_CHUNKS_PER_MESSAGE {
        return Err(EnvelopeError::TooManyChunks);
    }

    let mut chunks = Vec::with_capacity(chunk_count);
    for index in 0..chunk_count {
        let start = index * payload_capacity;
        let end = (start + payload_capacity).min(full_plaintext.len());
        let payload = &full_plaintext[start..end];

        let mut plaintext = Vec::with_capacity(MESSAGE_HASH_LEN + CHUNK_LEN_PREFIX + payload.len());
        plaintext.extend_from_slice(&message_hash);
        plaintext.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        plaintext.extend_from_slice(payload);
        pad_to_bucket(&mut plaintext)?;
        chunks.push(plaintext);
    }
    Ok(chunks)
}

fn pad_to_bucket(buf: &mut Vec<u8>) -> Result<(), EnvelopeError> {
    let bucket = PADDING_BUCKETS
        .iter()
        .copied()
        .find(|bucket| *bucket >= buf.len())
        .ok_or(EnvelopeError::ChunkTooLarge)?;
    buf.resize(bucket, 0);
    Ok(())
}

/// Reassembles a full plaintext from chunk plaintexts already ordered by
/// chunk index. Every chunk must authenticate the same message hash; the
/// reconstructed bytes must themselves hash to that same value.
pub fn reassemble_chunks(ordered_chunk_plaintexts: &[Vec<u8>]) -> Result<Vec<u8>, EnvelopeError> {
    let mut expected_hash: Option<[u8; MESSAGE_HASH_LEN]> = None;
    let mut out = Vec::new();

    for plaintext in ordered_chunk_plaintexts {
        if plaintext.len() < MESSAGE_HASH_LEN + CHUNK_LEN_PREFIX {
            return Err(EnvelopeError::Malformed);
        }
        let hash: [u8; MESSAGE_HASH_LEN] = plaintext[0..MESSAGE_HASH_LEN].try_into().unwrap();
        match expected_hash {
            None => expected_hash = Some(hash),
            Some(existing) if existing == hash => {}
            Some(_) => return Err(EnvelopeError::CrossMessageChunk),
        }

        let len_start = MESSAGE_HASH_LEN;
        let len_end = len_start + CHUNK_LEN_PREFIX;
        let payload_len =
            u32::from_be_bytes(plaintext[len_start..len_end].try_into().unwrap()) as usize;
        let payload = plaintext
            .get(len_end..len_end + payload_len)
            .ok_or(EnvelopeError::Malformed)?;

        if out.len() + payload.len() > MAX_REASSEMBLED_BYTES {
            return Err(EnvelopeError::ReassembledTooLarge);
        }
        out.extend_from_slice(payload);
    }

    let expected_hash = expected_hash.ok_or(EnvelopeError::IncompleteMessage)?;
    let actual_hash: [u8; MESSAGE_HASH_LEN] = Sha256::digest(&out).into();
    if actual_hash != expected_hash {
        return Err(EnvelopeError::MessageHashMismatch);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_plaintext_round_trips() {
        let body = b"hello canonical world".repeat(100);
        let full = build_full_plaintext(&body).unwrap();
        let recovered = parse_full_plaintext(&full).unwrap();
        assert_eq!(recovered, body);
    }

    #[test]
    fn rejects_a_declared_uncompressed_length_over_the_limit() {
        // Construct bytes that claim an over-limit uncompressed length
        // without needing to actually produce that much data.
        let compressed = zstd::stream::encode_all(&b"tiny"[..], 0).unwrap();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(compressed.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&((MAX_DECOMPRESSED_BYTES + 1) as u32).to_be_bytes());
        bytes.extend_from_slice(&compressed);
        assert_eq!(
            parse_full_plaintext(&bytes),
            Err(EnvelopeError::DecompressedTooLarge)
        );
    }

    #[test]
    fn rejects_a_decompression_bomb_that_lies_about_its_length() {
        // A highly compressible payload whose real decompressed size is far
        // larger than the (falsely small) declared uncompressed_length.
        let real_size = MAX_DECOMPRESSED_BYTES + 4096;
        let bomb = vec![0u8; real_size];
        let compressed = zstd::stream::encode_all(&bomb[..], 3).unwrap();
        assert!(compressed.len() < MAX_COMPRESSED_BYTES);

        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(compressed.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&16u32.to_be_bytes()); // lies: claims 16 bytes
        bytes.extend_from_slice(&compressed);
        assert_eq!(
            parse_full_plaintext(&bytes),
            Err(EnvelopeError::DecompressedTooLarge)
        );
    }

    #[test]
    fn splits_and_reassembles_a_multi_chunk_message() {
        // Pseudo-random, effectively incompressible so the compressed body
        // actually spans multiple chunks rather than collapsing into one.
        let mut state = 0x1234_5678_9abc_def0u64;
        let body: Vec<u8> = (0..MAX_CHUNK_PLAINTEXT_BYTES * 3)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state & 0xFF) as u8
            })
            .collect();
        let full = build_full_plaintext(&body).unwrap();
        let chunks = split_into_chunks(&full).unwrap();
        assert!(chunks.len() > 1);
        let reassembled = reassemble_chunks(&chunks).unwrap();
        assert_eq!(reassembled, full);
    }

    #[test]
    fn single_chunk_message_round_trips() {
        let full = build_full_plaintext(b"small event").unwrap();
        let chunks = split_into_chunks(&full).unwrap();
        assert_eq!(chunks.len(), 1);
        let reassembled = reassemble_chunks(&chunks).unwrap();
        assert_eq!(reassembled, full);
    }

    #[test]
    fn rejects_cross_message_chunk_substitution() {
        let full_a = build_full_plaintext(b"message a").unwrap();
        let full_b = build_full_plaintext(b"message b, a different message").unwrap();
        let mut chunks_a = split_into_chunks(&full_a).unwrap();
        let chunks_b = split_into_chunks(&full_b).unwrap();
        chunks_a.push(chunks_b[0].clone());
        assert_eq!(
            reassemble_chunks(&chunks_a),
            Err(EnvelopeError::CrossMessageChunk)
        );
    }

    #[test]
    fn rejects_a_tampered_chunk_hash() {
        let full = build_full_plaintext(b"hello canonical world").unwrap();
        let mut chunks = split_into_chunks(&full).unwrap();
        chunks[0][0] ^= 0xFF;
        let result = reassemble_chunks(&chunks);
        assert!(matches!(
            result,
            Err(EnvelopeError::CrossMessageChunk) | Err(EnvelopeError::MessageHashMismatch)
        ));
    }
}
