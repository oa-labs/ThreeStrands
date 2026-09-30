//! Checked-in golden vectors for the recovery key hierarchy and the X25519
//! sealed box.
//!
//! - `recovery_phrase_v1.txt` is a fixed 24-word phrase (encoding the
//!   synthetic seed `0x60..=0x7f`, not any real wallet's). It must always
//!   parse to [`RECOVERY_SEED`], the seed must always encode back to that
//!   exact phrase, and the seed must always derive the recovery X25519 and
//!   Ed25519 public keys frozen in `recovery_keys_v1.hex`
//!   (`x25519_public(32) || ed25519_public(32)`). A user's written-down
//!   phrase is their only offline way back into a sync space, so any change
//!   here strands every existing recovery phrase. These are deterministic;
//!   the `regenerate` helper below rewrites them, and should be run only
//!   for a deliberate, versioned change to the derivation (explained in the
//!   same commit).
//! - `sealed_box_v1.hex` is one `seal_to_x25519` output for
//!   [`SEALED_PLAINTEXT`] addressed to [`RECIPIENT_SECRET`]'s public key.
//!   Sealing is randomized (fresh ephemeral key and nonce per call), so this
//!   file was generated once and frozen; it has no regenerate helper. The
//!   test only opens the frozen bytes, which pins the wire layout
//!   (`ephemeral_public(32) || nonce(24) || ciphertext_and_tag`), the HKDF
//!   domain and key binding, and the AEAD construction: an epoch key sealed
//!   by an older build must stay openable by every later build.

use threestrands_sync_envelope::{
    check_recovery_phrase, recovery_ed25519_signing_key, recovery_phrase_from_seed, recovery_seed_from_phrase,
    recovery_x25519_secret, seal_to_x25519, try_open_sealed_box, X25519PublicKey, X25519StaticSecret,
    RECOVERY_PHRASE_WORDS, RECOVERY_SEED_LEN,
};

const GOLDEN_PHRASE: &str = include_str!("vectors/recovery_phrase_v1.txt");
const GOLDEN_RECOVERY_KEYS_HEX: &str = include_str!("vectors/recovery_keys_v1.hex");
const GOLDEN_SEALED_BOX_HEX: &str = include_str!("vectors/sealed_box_v1.hex");

/// Synthetic seed: the bytes `0x60..=0x7f`.
const RECOVERY_SEED: [u8; RECOVERY_SEED_LEN] = [
    0x60, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x6b, 0x6c, 0x6d, 0x6e, 0x6f,
    0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x7b, 0x7c, 0x7d, 0x7e, 0x7f,
];

/// Synthetic sealed-box recipient static secret: the bytes `0x80..=0x9f`.
const RECIPIENT_SECRET: [u8; 32] = [
    0x80, 0x81, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x8b, 0x8c, 0x8d, 0x8e, 0x8f,
    0x90, 0x91, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0x9b, 0x9c, 0x9d, 0x9e, 0x9f,
];

const SEALED_PLAINTEXT: &[u8] = b"golden sealed-box plaintext: a 32-byte epoch key goes here";

const SEALED_BOX_HEADER_LEN: usize = 32 + 24;

fn hex_decode(hex: &str) -> Vec<u8> {
    let hex = hex.trim();
    (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect()
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn derived_public_keys_hex(seed: &[u8; RECOVERY_SEED_LEN]) -> String {
    let x25519 = X25519PublicKey::from(&recovery_x25519_secret(seed)).to_bytes();
    let ed25519 = recovery_ed25519_signing_key(seed).verifying_key().to_bytes();
    format!("{}{}", hex_encode(&x25519), hex_encode(&ed25519))
}

#[test]
fn the_frozen_phrase_is_a_valid_recovery_phrase() {
    let check = check_recovery_phrase(GOLDEN_PHRASE);
    assert_eq!(check.word_count, RECOVERY_PHRASE_WORDS);
    assert!(check.unknown_word_positions.is_empty());
    assert!(check.valid);
}

#[test]
fn the_frozen_phrase_parses_to_the_frozen_seed_and_back() {
    assert_eq!(recovery_seed_from_phrase(GOLDEN_PHRASE).unwrap(), RECOVERY_SEED);
    assert_eq!(recovery_phrase_from_seed(&RECOVERY_SEED), GOLDEN_PHRASE.trim());
    // The whitespace normalization users rely on when retyping a phrase.
    let retyped = format!("  {}  ", GOLDEN_PHRASE.trim().replace(' ', "   "));
    assert_eq!(recovery_seed_from_phrase(&retyped).unwrap(), RECOVERY_SEED);
}

#[test]
fn the_frozen_phrase_derives_the_frozen_recovery_public_keys() {
    let seed = recovery_seed_from_phrase(GOLDEN_PHRASE).unwrap();
    assert_eq!(derived_public_keys_hex(&seed), GOLDEN_RECOVERY_KEYS_HEX.trim());
}

#[test]
fn the_frozen_sealed_box_opens_to_the_frozen_plaintext() {
    let sealed = hex_decode(GOLDEN_SEALED_BOX_HEX);
    assert_eq!(sealed.len(), SEALED_BOX_HEADER_LEN + SEALED_PLAINTEXT.len() + 16, "layout: epk || nonce || ct || tag");
    assert_eq!(try_open_sealed_box(&RECIPIENT_SECRET, &sealed).as_deref(), Some(SEALED_PLAINTEXT));
}

/// `try_open_sealed_box` deliberately has no error variant — "tampered" and
/// "not addressed to me" are indistinguishable by design — so the specific
/// failure asserted here is `None`.
#[test]
fn the_frozen_sealed_box_refuses_tampering_and_the_wrong_recipient() {
    let sealed = hex_decode(GOLDEN_SEALED_BOX_HEX);
    // One byte in each region: ephemeral public key, nonce, ciphertext, tag.
    for index in [0, 32, SEALED_BOX_HEADER_LEN, sealed.len() - 1] {
        let mut tampered = sealed.clone();
        tampered[index] ^= 0x01;
        assert_eq!(try_open_sealed_box(&RECIPIENT_SECRET, &tampered), None, "byte {index} was tampered");
    }
    assert_eq!(try_open_sealed_box(&RECIPIENT_SECRET, &sealed[..sealed.len() - 1]), None);

    // Not byte 0: X25519 clamping clears its low three bits, so flipping bit
    // 0 there yields the same scalar.
    let mut wrong_secret = RECIPIENT_SECRET;
    wrong_secret[1] ^= 0x01;
    assert_eq!(try_open_sealed_box(&wrong_secret, &sealed), None);
}

/// A fresh seal to the same recipient still opens (so the frozen box is a
/// real `seal_to_x25519` output shape, not a one-off), and differs from the
/// frozen bytes because sealing is randomized.
#[test]
fn a_fresh_seal_opens_but_never_reproduces_the_frozen_bytes() {
    let recipient_public = X25519PublicKey::from(&X25519StaticSecret::from(RECIPIENT_SECRET)).to_bytes();
    let fresh = seal_to_x25519(&recipient_public, SEALED_PLAINTEXT);
    assert_eq!(fresh.len(), hex_decode(GOLDEN_SEALED_BOX_HEX).len());
    assert_ne!(hex_encode(&fresh), GOLDEN_SEALED_BOX_HEX.trim());
    assert_eq!(try_open_sealed_box(&RECIPIENT_SECRET, &fresh).as_deref(), Some(SEALED_PLAINTEXT));
}

/// Regenerates the deterministic recovery files only (never the sealed
/// box). Run deliberately, only for an intentional derivation change:
/// `cargo test -p threestrands-sync-envelope --test recovery_vectors --
/// --ignored regenerate`.
#[test]
#[ignore]
fn regenerate() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors");
    std::fs::write(dir.join("recovery_phrase_v1.txt"), format!("{}\n", recovery_phrase_from_seed(&RECOVERY_SEED))).unwrap();
    std::fs::write(dir.join("recovery_keys_v1.hex"), format!("{}\n", derived_public_keys_hex(&RECOVERY_SEED))).unwrap();
}
