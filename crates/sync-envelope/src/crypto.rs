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
use x25519_dalek::{EphemeralSecret, PublicKey as X25519PublicKey, StaticSecret as X25519StaticSecret};

use crate::error::EnvelopeError;
use crate::header::NONCE_LEN;

const EPOCH_KEY_HKDF_DOMAIN: &[u8] = b"threestrands/sync-envelope/epoch-key/v1";
const SEALED_BOX_HKDF_DOMAIN: &[u8] = b"threestrands/sync-envelope/sealed-box/v1";
const SEALED_BOX_AAD: &[u8] = b"threestrands/sync-envelope/sealed-box/v1";

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

/// Signs `domain || message_id || canonical_unsigned_body`. The domain names
/// the object kind (see `crate::object_signature_domain`), so a signature
/// over one kind of sealed object can never verify as another.
/// `canonical_unsigned_body` is the canonical DAG-CBOR encoding of the
/// object's body without its signature.
pub fn sign_object(
    signing_key: &SigningKey,
    domain: &[u8],
    message_id: &[u8; 8],
    canonical_unsigned_body: &[u8],
) -> [u8; 64] {
    let preimage = signing_preimage(domain, message_id, canonical_unsigned_body);
    signing_key.sign(&preimage).to_bytes()
}

pub fn verify_object(
    verifying_key: &VerifyingKey,
    domain: &[u8],
    message_id: &[u8; 8],
    canonical_unsigned_body: &[u8],
    signature: &[u8; 64],
) -> Result<(), EnvelopeError> {
    let preimage = signing_preimage(domain, message_id, canonical_unsigned_body);
    let signature = EdSignature::from_bytes(signature);
    verifying_key
        .verify(&preimage, &signature)
        .map_err(|_| EnvelopeError::SignatureInvalid)
}

/// Signs arbitrary canonical bytes under a caller-chosen fixed domain,
/// separate from the sealed-object signature domains. Used for protocol objects that
/// are not [`crate::SyncEvent`]s but still need a device signature over
/// their canonical DAG-CBOR encoding, such as a signed device head.
pub fn sign_bytes(signing_key: &SigningKey, domain: &[u8], canonical_body: &[u8]) -> [u8; 64] {
    let mut preimage = Vec::with_capacity(domain.len() + canonical_body.len());
    preimage.extend_from_slice(domain);
    preimage.extend_from_slice(canonical_body);
    signing_key.sign(&preimage).to_bytes()
}

pub fn verify_bytes(
    verifying_key: &VerifyingKey,
    domain: &[u8],
    canonical_body: &[u8],
    signature: &[u8; 64],
) -> Result<(), EnvelopeError> {
    let mut preimage = Vec::with_capacity(domain.len() + canonical_body.len());
    preimage.extend_from_slice(domain);
    preimage.extend_from_slice(canonical_body);
    let signature = EdSignature::from_bytes(signature);
    verifying_key
        .verify(&preimage, &signature)
        .map_err(|_| EnvelopeError::SignatureInvalid)
}

/// Seals `plaintext` to `recipient_public` using an anonymous, single-use
/// X25519 sealed box: a fresh ephemeral keypair, X25519 Diffie-Hellman with
/// the recipient's static public key, HKDF-SHA256 over the shared secret
/// (bound to both public keys), and XChaCha20-Poly1305. The wire format is
/// `ephemeral_public(32) || nonce(24) || ciphertext_and_tag`. There is no
/// recipient identifier in the output — per the key hierarchy's "recipient
/// stanzas are anonymous" rule, a holder of a candidate static secret must
/// call [`try_open_sealed_box`] and see whether it opens.
pub fn seal_to_x25519(recipient_public: &[u8; 32], plaintext: &[u8]) -> Vec<u8> {
    let ephemeral_secret = EphemeralSecret::random_from_rng(OsRng);
    let ephemeral_public = X25519PublicKey::from(&ephemeral_secret);
    let recipient = X25519PublicKey::from(*recipient_public);
    let shared = ephemeral_secret.diffie_hellman(&recipient);
    let key = derive_sealed_box_key(shared.as_bytes(), ephemeral_public.as_bytes(), recipient_public);
    let nonce = random_nonce();
    let ciphertext = aead_encrypt(&key, &nonce, SEALED_BOX_AAD, plaintext);

    let mut out = Vec::with_capacity(32 + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(ephemeral_public.as_bytes());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    out
}

/// Tries to open a sealed box with `recipient_secret`, returning `None`
/// (never an error) on any failure — malformed input and "this box was not
/// addressed to me" are indistinguishable by design, since stanzas carry no
/// recipient identifier.
pub fn try_open_sealed_box(recipient_secret: &[u8; 32], sealed: &[u8]) -> Option<Vec<u8>> {
    if sealed.len() < 32 + NONCE_LEN {
        return None;
    }
    let ephemeral_public_bytes: [u8; 32] = sealed[0..32].try_into().ok()?;
    let nonce: [u8; NONCE_LEN] = sealed[32..32 + NONCE_LEN].try_into().ok()?;
    let ciphertext = &sealed[32 + NONCE_LEN..];

    let secret = X25519StaticSecret::from(*recipient_secret);
    let recipient_public_bytes = X25519PublicKey::from(&secret).to_bytes();
    let ephemeral_public = X25519PublicKey::from(ephemeral_public_bytes);
    let shared = secret.diffie_hellman(&ephemeral_public);
    let key = derive_sealed_box_key(shared.as_bytes(), &ephemeral_public_bytes, &recipient_public_bytes);
    aead_decrypt(&key, &nonce, SEALED_BOX_AAD, ciphertext).ok()
}

fn derive_sealed_box_key(shared_secret: &[u8; 32], ephemeral_public: &[u8; 32], recipient_public: &[u8; 32]) -> [u8; 32] {
    let hkdf = Hkdf::<Sha256>::new(None, shared_secret);
    let mut info = Vec::with_capacity(SEALED_BOX_HKDF_DOMAIN.len() + 64);
    info.extend_from_slice(SEALED_BOX_HKDF_DOMAIN);
    info.extend_from_slice(ephemeral_public);
    info.extend_from_slice(recipient_public);
    let mut out = [0u8; 32];
    hkdf.expand(&info, &mut out)
        .expect("32-byte output is within HKDF-SHA256's expand limit");
    out
}

fn signing_preimage(domain: &[u8], message_id: &[u8; 8], canonical_unsigned_body: &[u8]) -> Vec<u8> {
    let mut preimage = Vec::with_capacity(domain.len() + 8 + canonical_unsigned_body.len());
    preimage.extend_from_slice(domain);
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
    fn sealed_box_round_trips_and_is_anonymous_to_the_wrong_key() {
        let recipient_secret = [4u8; 32];
        let recipient_public = X25519PublicKey::from(&X25519StaticSecret::from(recipient_secret)).to_bytes();
        let sealed = seal_to_x25519(&recipient_public, b"epoch key bytes");
        assert_eq!(try_open_sealed_box(&recipient_secret, &sealed), Some(b"epoch key bytes".to_vec()));

        let wrong_secret = [5u8; 32];
        assert_eq!(try_open_sealed_box(&wrong_secret, &sealed), None);
    }

    #[test]
    fn sealed_box_is_freshly_randomized_per_call() {
        let recipient_secret = [6u8; 32];
        let recipient_public = X25519PublicKey::from(&X25519StaticSecret::from(recipient_secret)).to_bytes();
        let a = seal_to_x25519(&recipient_public, b"same plaintext");
        let b = seal_to_x25519(&recipient_public, b"same plaintext");
        assert_ne!(a, b);
    }

    #[test]
    fn sealed_box_rejects_truncated_and_tampered_input() {
        let recipient_secret = [7u8; 32];
        let recipient_public = X25519PublicKey::from(&X25519StaticSecret::from(recipient_secret)).to_bytes();
        let sealed = seal_to_x25519(&recipient_public, b"payload");

        assert_eq!(try_open_sealed_box(&recipient_secret, &sealed[..10]), None);

        let mut tampered = sealed.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert_eq!(try_open_sealed_box(&recipient_secret, &tampered), None);
    }

    #[test]
    fn signature_round_trips_and_rejects_tampering() {
        let signing_key = SigningKey::generate(&mut RandOsRng);
        let verifying_key = signing_key.verifying_key();
        let domain = b"test-domain";
        let message_id = [9u8; 8];
        let body = b"canonical-unsigned-body";
        let signature = sign_object(&signing_key, domain, &message_id, body);
        verify_object(&verifying_key, domain, &message_id, body, &signature).unwrap();

        let other_key = SigningKey::generate(&mut RandOsRng).verifying_key();
        assert!(verify_object(&other_key, domain, &message_id, body, &signature).is_err());
        assert!(verify_object(&verifying_key, domain, &message_id, b"different-body", &signature).is_err());
        assert!(verify_object(&verifying_key, domain, &[0u8; 8], body, &signature).is_err());
        assert!(verify_object(&verifying_key, b"other-domain", &message_id, body, &signature).is_err());
    }
}
