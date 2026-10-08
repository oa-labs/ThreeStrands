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
use crate::os_rng;
use rand::Rng;
use sha2::Sha256;
use x25519_dalek::StaticSecret as X25519StaticSecret;

pub const RECOVERY_SEED_LEN: usize = 32;

const RECOVERY_X25519_DOMAIN: &[u8] = b"threestrands/sync-envelope/recovery-x25519/v1";
const RECOVERY_ED25519_DOMAIN: &[u8] = b"threestrands/sync-envelope/recovery-ed25519/v1";

/// A fresh, random 32-byte recovery seed — 256 bits of entropy, encoded as a
/// 24-word BIP-39 mnemonic.
pub fn generate_recovery_seed() -> [u8; RECOVERY_SEED_LEN] {
    let mut seed = [0u8; RECOVERY_SEED_LEN];
    os_rng().fill_bytes(&mut seed);
    seed
}

/// Encodes a recovery seed as a human-writable 24-word phrase.
pub fn recovery_phrase_from_seed(seed: &[u8; RECOVERY_SEED_LEN]) -> String {
    Mnemonic::from_entropy(seed)
        .expect("a 32-byte entropy array is always valid BIP-39 input")
        .to_string()
}

/// Decodes a 24-word phrase back into its 32-byte seed. Ignores letter case
/// and extra whitespace — the same normalization [`check_recovery_phrase`]
/// applies, so a phrase it reports valid always decodes; rejects an invalid
/// checksum or word list membership.
pub fn recovery_seed_from_phrase(phrase: &str) -> Result<[u8; RECOVERY_SEED_LEN], String> {
    let normalized = phrase.split_whitespace().map(str::to_lowercase).collect::<Vec<_>>().join(" ");
    let mnemonic = Mnemonic::parse_normalized(&normalized).map_err(|error| error.to_string())?;
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
    fn phrase_decoding_ignores_letter_case_like_the_phrase_checker() {
        let seed = generate_recovery_seed();
        let phrase = recovery_phrase_from_seed(&seed);
        let words: Vec<&str> = phrase.split_whitespace().collect();
        // Uppercase, and a phone keyboard's auto-capitalized first word.
        let shouted = phrase.to_uppercase();
        let capitalized = {
            let mut first = words[0].to_string();
            first[..1].make_ascii_uppercase();
            std::iter::once(first.as_str()).chain(words[1..].iter().copied()).collect::<Vec<_>>().join(" ")
        };
        for variant in [shouted, capitalized] {
            assert!(check_recovery_phrase(&variant).valid, "{variant}");
            assert_eq!(recovery_seed_from_phrase(&variant).unwrap(), seed, "{variant}");
        }
    }

    #[test]
    fn phrase_decoding_tolerates_stray_whitespace() {
        let seed = generate_recovery_seed();
        let phrase = recovery_phrase_from_seed(&seed);
        let noisy = format!("  {}  ", phrase.replace(' ', "   "));
        assert_eq!(recovery_seed_from_phrase(&noisy).unwrap(), seed);
    }

    /// A fixed seed whose phrase, with its first and last words swapped, is
    /// known to fail the BIP-39 checksum. A random seed would pass the
    /// swapped checksum by chance in roughly one run in 256.
    const SWAP_TAMPER_SEED: [u8; RECOVERY_SEED_LEN] = [
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
        25, 26, 27, 28, 29, 30, 31,
    ];

    fn swap_first_and_last_words(phrase: &str) -> String {
        let mut words: Vec<&str> = phrase.split_whitespace().collect();
        let last = words.len() - 1;
        assert_ne!(words[0], words[last], "the tamper must change the phrase");
        words.swap(0, last);
        words.join(" ")
    }

    #[test]
    fn rejects_a_bad_checksum() {
        let phrase = recovery_phrase_from_seed(&SWAP_TAMPER_SEED);
        let tampered = swap_first_and_last_words(&phrase);
        let error = recovery_seed_from_phrase(&tampered).unwrap_err();
        assert!(error.to_lowercase().contains("checksum"), "unexpected error: {error}");
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
        let phrase = recovery_phrase_from_seed(&SWAP_TAMPER_SEED);
        let check = check_recovery_phrase(&phrase.to_uppercase());
        assert_eq!(check, RecoveryPhraseCheck { word_count: 24, unknown_word_positions: vec![], valid: true });

        let swapped = check_recovery_phrase(&swap_first_and_last_words(&phrase));
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
