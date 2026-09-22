//! The recovery seed and the two keys deterministically derived from it: a
//! recovery X25519 keypair (an always-eligible recipient for sealed epoch
//! keys) and a recovery Ed25519 keypair (an authorization signer trusted for
//! enrollment grants, alongside ordinary active devices). The seed itself is
//! never persisted by normal device operation — see the key hierarchy's
//! "the seed exists in the recovery phrase... normal device operation does
//! not require or persist it."

use bip39::Mnemonic;
use ed25519_dalek::SigningKey;
use hkdf::Hkdf;
use rand::rngs::OsRng;
use rand::RngCore;
use sha2::Sha256;
use x25519_dalek::StaticSecret as X25519StaticSecret;

pub const RECOVERY_SEED_LEN: usize = 32;

const RECOVERY_X25519_DOMAIN: &[u8] = b"threestrands/sync-envelope/recovery-x25519/v1";
const RECOVERY_ED25519_DOMAIN: &[u8] = b"threestrands/sync-envelope/recovery-ed25519/v1";

/// A fresh, random 32-byte recovery seed — 256 bits of entropy, encoded as a
/// 24-word BIP-39 mnemonic.
pub fn generate_recovery_seed() -> [u8; RECOVERY_SEED_LEN] {
    let mut seed = [0u8; RECOVERY_SEED_LEN];
    OsRng.fill_bytes(&mut seed);
    seed
}

/// Encodes a recovery seed as a human-writable 24-word phrase.
pub fn recovery_phrase_from_seed(seed: &[u8; RECOVERY_SEED_LEN]) -> String {
    Mnemonic::from_entropy(seed)
        .expect("a 32-byte entropy array is always valid BIP-39 input")
        .to_string()
}

/// Decodes a 24-word phrase back into its 32-byte seed. Accepts the
/// standard BIP-39 whitespace/case normalization; rejects an invalid
/// checksum or word list membership.
pub fn recovery_seed_from_phrase(phrase: &str) -> Result<[u8; RECOVERY_SEED_LEN], String> {
    let mnemonic = Mnemonic::parse_normalized(phrase).map_err(|error| error.to_string())?;
    let entropy = mnemonic.to_entropy();
    entropy
        .as_slice()
        .try_into()
        .map_err(|_| "Recovery phrase did not encode a 32-byte seed".to_string())
}

/// Number of words in a recovery phrase.
pub const RECOVERY_PHRASE_WORDS: usize = 24;

/// Progress report for a partially typed recovery phrase, so a joining
/// device can point at a mistyped word before trying to join.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryPhraseCheck {
    pub word_count: usize,
    /// Zero-based positions of words not in the recovery word list. The
    /// word still being typed is included; callers decide when to flag it.
    pub unknown_word_positions: Vec<usize>,
    /// True only for exactly [`RECOVERY_PHRASE_WORDS`] known words whose
    /// checksum is valid — the phrase [`recovery_seed_from_phrase`] accepts.
    pub valid: bool,
}

pub fn check_recovery_phrase(phrase: &str) -> RecoveryPhraseCheck {
    let words: Vec<String> = phrase.split_whitespace().map(str::to_lowercase).collect();
    let unknown_word_positions: Vec<usize> = words
        .iter()
        .enumerate()
        .filter(|(_, word)| bip39::Language::English.find_word(word).is_none())
        .map(|(index, _)| index)
        .collect();
    let valid = words.len() == RECOVERY_PHRASE_WORDS
        && unknown_word_positions.is_empty()
        && recovery_seed_from_phrase(&words.join(" ")).is_ok();
    RecoveryPhraseCheck { word_count: words.len(), unknown_word_positions, valid }
}

fn hkdf_derive(seed: &[u8; RECOVERY_SEED_LEN], domain: &[u8]) -> [u8; 32] {
    let hkdf = Hkdf::<Sha256>::new(None, seed);
    let mut out = [0u8; 32];
    hkdf.expand(domain, &mut out)
        .expect("32-byte output is within HKDF-SHA256's expand limit");
    out
}

/// The recovery X25519 static secret: an always-eligible sealed-epoch-key
/// recipient, so a device that only has the recovery phrase (no online peer)
/// can still recover the current epoch key from a rotation/genesis object.
pub fn recovery_x25519_secret(seed: &[u8; RECOVERY_SEED_LEN]) -> X25519StaticSecret {
    X25519StaticSecret::from(hkdf_derive(seed, RECOVERY_X25519_DOMAIN))
}

/// The recovery Ed25519 signing key: authorizes enrollment grants and
/// rotations without needing an already-active device online, per "initial
/// device authorization is signed by the recovery authorization key."
pub fn recovery_ed25519_signing_key(seed: &[u8; RECOVERY_SEED_LEN]) -> SigningKey {
    SigningKey::from_bytes(&hkdf_derive(seed, RECOVERY_ED25519_DOMAIN))
}

#[cfg(test)]
mod tests {
    use super::*;
    use x25519_dalek::PublicKey as X25519PublicKey;

    #[test]
    fn phrase_round_trips_through_the_seed() {
        let seed = generate_recovery_seed();
        let phrase = recovery_phrase_from_seed(&seed);
        assert_eq!(phrase.split_whitespace().count(), 24);
        let recovered = recovery_seed_from_phrase(&phrase).unwrap();
        assert_eq!(recovered, seed);
    }

    #[test]
    fn phrase_decoding_tolerates_stray_whitespace() {
        let seed = generate_recovery_seed();
        let phrase = recovery_phrase_from_seed(&seed);
        let noisy = format!("  {}  ", phrase.replace(' ', "   "));
        assert_eq!(recovery_seed_from_phrase(&noisy).unwrap(), seed);
    }

    #[test]
    fn rejects_a_bad_checksum() {
        let seed = generate_recovery_seed();
        let phrase = recovery_phrase_from_seed(&seed);
        let mut words: Vec<&str> = phrase.split_whitespace().collect();
        let last = words.len() - 1;
        words.swap(0, last);
        let tampered = words.join(" ");
        assert!(recovery_seed_from_phrase(&tampered).is_err());
    }

    #[test]
    fn checks_a_partial_phrase_word_by_word() {
        let check = check_recovery_phrase("  abandon ABILITY  notaword able ");
        assert_eq!(check.word_count, 4);
        assert_eq!(check.unknown_word_positions, vec![2]);
        assert!(!check.valid);
        assert_eq!(check_recovery_phrase("").word_count, 0);
    }

    #[test]
    fn only_a_complete_checksummed_phrase_is_valid() {
        let phrase = recovery_phrase_from_seed(&generate_recovery_seed());
        let check = check_recovery_phrase(&phrase.to_uppercase());
        assert_eq!(check, RecoveryPhraseCheck { word_count: 24, unknown_word_positions: vec![], valid: true });

        let mut words: Vec<&str> = phrase.split_whitespace().collect();
        let last = words.len() - 1;
        words.swap(0, last);
        let swapped = check_recovery_phrase(&words.join(" "));
        assert!(swapped.unknown_word_positions.is_empty());
        assert!(!swapped.valid);

        let extra = check_recovery_phrase(&format!("{phrase} abandon"));
        assert_eq!(extra.word_count, 25);
        assert!(!extra.valid);
    }

    #[test]
    fn derived_keys_are_deterministic_and_domain_separated() {
        let seed = generate_recovery_seed();
        let x25519_a = recovery_x25519_secret(&seed);
        let x25519_b = recovery_x25519_secret(&seed);
        assert_eq!(X25519PublicKey::from(&x25519_a).to_bytes(), X25519PublicKey::from(&x25519_b).to_bytes());

        let ed25519_a = recovery_ed25519_signing_key(&seed);
        let ed25519_b = recovery_ed25519_signing_key(&seed);
        assert_eq!(ed25519_a.verifying_key(), ed25519_b.verifying_key());

        // Different domains must not collide: an X25519 public key derived
        // from the same seed differs from the Ed25519 verifying key bytes.
        assert_ne!(X25519PublicKey::from(&x25519_a).to_bytes(), ed25519_a.verifying_key().to_bytes());
    }

    #[test]
    fn different_seeds_derive_different_keys() {
        let a = recovery_x25519_secret(&generate_recovery_seed());
        let b = recovery_x25519_secret(&generate_recovery_seed());
        assert_ne!(X25519PublicKey::from(&a).to_bytes(), X25519PublicKey::from(&b).to_bytes());
    }
}
