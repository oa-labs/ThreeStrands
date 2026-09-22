//! Content addressing for exact encrypted envelope bytes.
//!
//! Uses CIDv1 with the "raw" multicodec (0x55) and a sha2-256 multihash, the
//! same convention `ipfs add --cid-version=1 --raw-leaves` produces, so the
//! folder corpus is directly importable into IPFS without re-encoding.

use cid::Cid;
use multihash_codetable::{Code, MultihashDigest};

const RAW_MULTICODEC: u64 = 0x55;

/// Computes the CIDv1 string for the exact bytes given. The same bytes
/// always produce the same CID; different bytes (even by one bit) never
/// collide in practice.
pub fn compute_cid(bytes: &[u8]) -> String {
    let hash = Code::Sha2_256.digest(bytes);
    Cid::new_v1(RAW_MULTICODEC, hash).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_deterministic_and_content_addressed() {
        let a = compute_cid(b"hello world");
        let b = compute_cid(b"hello world");
        assert_eq!(a, b);
        assert_eq!(
            a,
            "bafkreifzjut3te2nhyekklss27nh3k72ysco7y32koao5eei66wof36n5e"
        );

        let different = compute_cid(b"hello world!");
        assert_ne!(a, different);
    }
}
