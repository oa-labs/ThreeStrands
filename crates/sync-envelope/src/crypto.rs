//! AEAD encryption and device signatures. This module is pure key
//! material in, bytes out: it does not store, generate, seal, or rotate
//! epoch or device keys. That is the key hierarchy's job (a later phase).

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use ed25519_dalek::{Signature as EdSignature, Signer, SigningKey, Verifier, VerifyingKey};
use hkdf::Hkdf;
use rand::rngs::OsRng;
use rand::RngCore;
use sha2::Sha256;

use crate::error::EnvelopeError;
use crate::header::NONCE_LEN;

const EPOCH_KEY_HKDF_DOMAIN: &[u8] = b"threestrands/sync-envelope/epoch-key/v1";
const SIGNATURE_DOMAIN: &[u8] = b"threestrands/sync-envelope/event-signature/v1";

/// Derives the concrete per-epoch AEAD key from the raw `K_epoch` secret,
/// domain-separated by the fixed HKDF info string, the key epoch number, and
/// the stable sync-space id. The raw `K_epoch` value is never used directly
/// as an AEAD key.
pub fn derive_epoch_aead_key(k_epoch: &[u8; 32], sync_space_id: &[u8], key_epoch: u32) -> [u8; 32] {
    let hkdf = Hkdf::<Sha256>::new(None, k_epoch);
    let mut info = Vec::with_capacity(EPOCH_KEY_HKDF_DOMAIN.len() + 4 + sync_space_id.len());
    info.extend_from_slice(EPOCH_KEY_HKDF_DOMAIN);
    info.extend_from_slice(&key_epoch.to_be_bytes());
    info.extend_from_slice(sync_space_id);
    let mut out = [0u8; 32];
    hkdf.expand(&info, &mut out)
        .expect("32-byte output is within HKDF-SHA256's expand limit");
    out
}

/// Draws a fresh random nonce from the OS CSPRNG. Callers that need a
/// crash-safe, reproducible re-encryption (see the local-persistence
/// contract) must persist the nonce they used and supply it back on retry
/// rather than calling this again.
pub fn random_nonce() -> [u8; NONCE_LEN] {
    let mut nonce = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce);
    nonce
}

pub fn aead_encrypt(
    key: &[u8; 32],
    nonce: &[u8; NONCE_LEN],
    aad: &[u8],
    plaintext: &[u8],
) -> Vec<u8> {
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .encrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .expect("XChaCha20-Poly1305 encryption over an in-memory buffer cannot fail")
}

pub fn aead_decrypt(
    key: &[u8; 32],
    nonce: &[u8; NONCE_LEN],
    aad: &[u8],
    ciphertext_and_tag: &[u8],
) -> Result<Vec<u8>, EnvelopeError> {
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .decrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: ciphertext_and_tag,
                aad,
            },
        )
        .map_err(|_| EnvelopeError::DecryptionFailed)
}

/// Signs `message_id || canonical_unsigned_body` under the fixed signature
/// domain. `canonical_unsigned_body` is the canonical CBOR encoding of every
/// event field except the signature itself.
pub fn sign_event(
    signing_key: &SigningKey,
    message_id: &[u8; 8],
    canonical_unsigned_body: &[u8],
) -> [u8; 64] {
    let preimage = signing_preimage(message_id, canonical_unsigned_body);
    signing_key.sign(&preimage).to_bytes()
}

pub fn verify_event(
    verifying_key: &VerifyingKey,
    message_id: &[u8; 8],
    canonical_unsigned_body: &[u8],
    signature: &[u8; 64],
) -> Result<(), EnvelopeError> {
    let preimage = signing_preimage(message_id, canonical_unsigned_body);
    let signature = EdSignature::from_bytes(signature);
    verifying_key
        .verify(&preimage, &signature)
        .map_err(|_| EnvelopeError::SignatureInvalid)
}

fn signing_preimage(message_id: &[u8; 8], canonical_unsigned_body: &[u8]) -> Vec<u8> {
    let mut preimage =
        Vec::with_capacity(SIGNATURE_DOMAIN.len() + 8 + canonical_unsigned_body.len());
    preimage.extend_from_slice(SIGNATURE_DOMAIN);
    preimage.extend_from_slice(message_id);
    preimage.extend_from_slice(canonical_unsigned_body);
    preimage
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng as RandOsRng;

    #[test]
    fn epoch_key_derivation_is_deterministic_and_domain_separated() {
        let k_epoch = [1u8; 32];
        let a = derive_epoch_aead_key(&k_epoch, b"space-a", 1);
        let b = derive_epoch_aead_key(&k_epoch, b"space-a", 1);
        assert_eq!(a, b);

        let different_space = derive_epoch_aead_key(&k_epoch, b"space-b", 1);
        assert_ne!(a, different_space);

        let different_epoch = derive_epoch_aead_key(&k_epoch, b"space-a", 2);
        assert_ne!(a, different_epoch);
    }

    #[test]
    fn aead_round_trips_and_rejects_tampering() {
        let key = [2u8; 32];
        let nonce = [3u8; NONCE_LEN];
        let aad = b"header-and-domain";
        let ciphertext = aead_encrypt(&key, &nonce, aad, b"payload");
        let plaintext = aead_decrypt(&key, &nonce, aad, &ciphertext).unwrap();
        assert_eq!(plaintext, b"payload");

        assert!(aead_decrypt(&key, &nonce, b"different-aad", &ciphertext).is_err());
        let mut wrong_key = key;
        wrong_key[0] ^= 1;
        assert!(aead_decrypt(&wrong_key, &nonce, aad, &ciphertext).is_err());
        let mut tampered = ciphertext.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(aead_decrypt(&key, &nonce, aad, &tampered).is_err());
    }

    #[test]
    fn signature_round_trips_and_rejects_tampering() {
        let signing_key = SigningKey::generate(&mut RandOsRng);
        let verifying_key = signing_key.verifying_key();
        let message_id = [9u8; 8];
        let body = b"canonical-unsigned-body";
        let signature = sign_event(&signing_key, &message_id, body);
        verify_event(&verifying_key, &message_id, body, &signature).unwrap();

        let other_key = SigningKey::generate(&mut RandOsRng).verifying_key();
        assert!(verify_event(&other_key, &message_id, body, &signature).is_err());
        assert!(verify_event(&verifying_key, &message_id, b"different-body", &signature).is_err());
        assert!(verify_event(&verifying_key, &[0u8; 8], body, &signature).is_err());
    }
}
