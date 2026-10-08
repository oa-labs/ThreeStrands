//! Join codes and the invitation objects behind them.
//!
//! A join code is a bearer secret an enrolled device hands to a new one:
//! a random invite secret, the content address of a signed [`Invitation`]
//! the inviter published to its connectors, and the connector settings the
//! new device needs to reach it. Connector config and secrets are opaque
//! JSON strings here — this crate has no provider concepts.
//!
//! Code text: `TSJOIN1-` + base64url(no padding) of
//! `DAG-CBOR(JoinCode) || first 4 bytes of SHA-256(DAG-CBOR)`. The checksum
//! tells "incomplete or mistyped" apart from "not a join code". This is a
//! persistent cross-version format: a code made by one app version must
//! keep decoding in every later one.
//!
//! The invite secret derives (HKDF-SHA256, fixed domains) an Ed25519 key
//! that signs the joining device's [`InvitationRedemption`] — proof it holds
//! the code — and an X25519 secret that opens the epoch key sealed inside
//! the [`Invitation`].

use base64::Engine;
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use serde_bytes::ByteBuf;
use sha2::{Digest, Sha256};
use x25519_dalek::StaticSecret as X25519StaticSecret;

use crate::crypto;
use crate::enrollment::{validate_earlier_epoch_keys, RosterEntry, SealedEpochKey};
use crate::error::EnvelopeError;
use crate::ids::{DeviceId, Signature};
use crate::limits::{
    MAX_JOIN_CODE_CHARS, MAX_JOIN_CONNECTORS, MAX_JOIN_CONNECTOR_CONFIG_BYTES, MAX_JOIN_CONNECTOR_KIND_BYTES,
    MAX_JOIN_CONNECTOR_SECRETS_BYTES, MAX_JOIN_NAME_CHARS,
};
use crate::{canonical_dag_cbor, decode_canonical_dag_cbor, validate_cid_reference, SigningKey, VerifyingKey};

pub const JOIN_CODE_PREFIX: &str = "TSJOIN1-";
pub const JOIN_CODE_VERSION: u16 = 1;
pub const INVITE_SECRET_LEN: usize = 32;

/// Every join code prefix starts with this, followed by the version digit
/// and a hyphen — so `TSJOIN2-…` is recognized as "newer", not "garbage".
const JOIN_CODE_FAMILY: &str = "TSJOIN";
const CHECKSUM_LEN: usize = 4;

const INVITE_ED25519_DOMAIN: &[u8] = b"threestrands/sync-envelope/invite-ed25519/v1";
const INVITE_X25519_DOMAIN: &[u8] = b"threestrands/sync-envelope/invite-x25519/v1";
const INVITATION_SIGNATURE_DOMAIN: &[u8] = b"threestrands/sync-envelope/invitation-signature/v1";
const REDEMPTION_INVITE_SIGNATURE_DOMAIN: &[u8] =
    b"threestrands/sync-envelope/invitation-redemption-invite-signature/v1";
const REDEMPTION_DEVICE_SIGNATURE_DOMAIN: &[u8] =
    b"threestrands/sync-envelope/invitation-redemption-device-signature/v1";

// ============================== Key derivation ===============================

pub fn generate_invite_secret() -> [u8; INVITE_SECRET_LEN] {
    let mut secret = [0u8; INVITE_SECRET_LEN];
    rand::Rng::fill_bytes(&mut crate::os_rng(), &mut secret);
    secret
}

fn hkdf_derive(secret: &[u8; INVITE_SECRET_LEN], domain: &[u8]) -> [u8; 32] {
    let hkdf = Hkdf::<Sha256>::new(None, secret);
    let mut out = [0u8; 32];
    hkdf.expand(domain, &mut out)
        .expect("32-byte output is within HKDF-SHA256's expand limit");
    out
}

/// Signs the joining device's redemption: proof it holds the code.
pub fn invite_ed25519_signing_key(secret: &[u8; INVITE_SECRET_LEN]) -> SigningKey {
    SigningKey::from_bytes(&hkdf_derive(secret, INVITE_ED25519_DOMAIN))
}

/// Opens the epoch key sealed inside the invitation.
pub fn invite_x25519_secret(secret: &[u8; INVITE_SECRET_LEN]) -> X25519StaticSecret {
    X25519StaticSecret::from(hkdf_derive(secret, INVITE_X25519_DOMAIN))
}

// ================================= Join code =================================

/// One connector carried by a join code: its kind tag, non-secret config,
/// and (unless the inviter left it out) its secret, both as opaque JSON.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinConnector {
    pub kind: String,
    pub config_json: String,
    pub secrets_json: Option<String>,
}

impl std::fmt::Debug for JoinConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JoinConnector")
            .field("kind", &self.kind)
            .field("config_json", &self.config_json)
            .field("secrets_json", &self.secrets_json.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// The decoded contents of a join code (format version 1).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinCode {
    pub version: u16,
    pub invite_secret: ByteBuf,
    pub invitation_cid: String,
    pub inviter_name: String,
    pub expires_at_ms: i64,
    pub connectors: Vec<JoinConnector>,
}

impl std::fmt::Debug for JoinCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JoinCode")
            .field("version", &self.version)
            .field("invite_secret", &"<redacted>")
            .field("invitation_cid", &self.invitation_cid)
            .field("inviter_name", &self.inviter_name)
            .field("expires_at_ms", &self.expires_at_ms)
            .field("connectors", &self.connectors)
            .finish()
    }
}

impl JoinCode {
    /// The invite secret as a fixed array; validated to be exactly
    /// [`INVITE_SECRET_LEN`] bytes by encode and decode.
    pub fn invite_secret(&self) -> [u8; INVITE_SECRET_LEN] {
        self.invite_secret.as_slice().try_into().expect("validated at encode/decode")
    }
}

/// Why a pasted join code couldn't be read. Messages are written for the
/// person pasting it.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum JoinCodeError {
    #[error("This isn't a ThreeStrands join code.")]
    NotAJoinCode,
    #[error("This join code is incomplete or mistyped. Copy it again.")]
    Incomplete,
    #[error("This join code was made by a newer version of ThreeStrands. Update this app, then paste it again.")]
    NewerVersion,
    #[error("This join code is damaged. Ask for a new one.")]
    Malformed,
    #[error("This join code is longer than any real join code.")]
    TooLong,
    #[error("This join code exceeds a limit: {0}")]
    LimitExceeded(&'static str),
}

#[derive(Deserialize)]
struct VersionProbe {
    version: u16,
}

fn validate_join_code(code: &JoinCode) -> Result<(), JoinCodeError> {
    if code.version != JOIN_CODE_VERSION {
        return Err(JoinCodeError::Malformed);
    }
    if code.invite_secret.len() != INVITE_SECRET_LEN {
        return Err(JoinCodeError::Malformed);
    }
    validate_cid_reference(&code.invitation_cid).map_err(|_| JoinCodeError::Malformed)?;
    if code.inviter_name.chars().count() > MAX_JOIN_NAME_CHARS {
        return Err(JoinCodeError::LimitExceeded("inviter name"));
    }
    if code.expires_at_ms <= 0 {
        return Err(JoinCodeError::Malformed);
    }
    if code.connectors.is_empty() {
        return Err(JoinCodeError::Malformed);
    }
    if code.connectors.len() > MAX_JOIN_CONNECTORS {
        return Err(JoinCodeError::LimitExceeded("connector count"));
    }
    for connector in &code.connectors {
        if connector.kind.is_empty() || connector.kind.len() > MAX_JOIN_CONNECTOR_KIND_BYTES {
            return Err(JoinCodeError::LimitExceeded("connector kind"));
        }
        if connector.config_json.len() > MAX_JOIN_CONNECTOR_CONFIG_BYTES {
            return Err(JoinCodeError::LimitExceeded("connector config"));
        }
        if connector.secrets_json.as_ref().is_some_and(|secrets| secrets.len() > MAX_JOIN_CONNECTOR_SECRETS_BYTES) {
            return Err(JoinCodeError::LimitExceeded("connector secrets"));
        }
    }
    Ok(())
}

pub fn encode_join_code(code: &JoinCode) -> Result<String, JoinCodeError> {
    validate_join_code(code)?;
    let mut payload = canonical_dag_cbor(code).map_err(|_| JoinCodeError::Malformed)?;
    let checksum = Sha256::digest(&payload);
    payload.extend_from_slice(&checksum[..CHECKSUM_LEN]);
    let text = format!("{JOIN_CODE_PREFIX}{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload));
    if text.len() > MAX_JOIN_CODE_CHARS {
        return Err(JoinCodeError::TooLong);
    }
    Ok(text)
}

/// Decodes pasted join code text. Whitespace and line breaks anywhere in
/// it (which email and chat clients insert) are ignored.
pub fn decode_join_code(text: &str) -> Result<JoinCode, JoinCodeError> {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.len() > MAX_JOIN_CODE_CHARS {
        return Err(JoinCodeError::TooLong);
    }
    let body = match compact.strip_prefix(JOIN_CODE_PREFIX) {
        Some(body) => body,
        None => {
            let newer = compact
                .strip_prefix(JOIN_CODE_FAMILY)
                .and_then(|rest| rest.split_once('-'))
                .is_some_and(|(version, _)| version.parse::<u16>().is_ok_and(|version| version > JOIN_CODE_VERSION));
            return Err(if newer { JoinCodeError::NewerVersion } else { JoinCodeError::NotAJoinCode });
        }
    };
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(body)
        .map_err(|_| JoinCodeError::Incomplete)?;
    if payload.len() <= CHECKSUM_LEN {
        return Err(JoinCodeError::Incomplete);
    }
    let (cbor, checksum) = payload.split_at(payload.len() - CHECKSUM_LEN);
    if Sha256::digest(cbor)[..CHECKSUM_LEN] != *checksum {
        return Err(JoinCodeError::Incomplete);
    }
    let probe: VersionProbe = decode_canonical_dag_cbor(cbor).map_err(|_| JoinCodeError::Malformed)?;
    if probe.version > JOIN_CODE_VERSION {
        return Err(JoinCodeError::NewerVersion);
    }
    let code: JoinCode = decode_canonical_dag_cbor(cbor).map_err(|_| JoinCodeError::Malformed)?;
    validate_join_code(&code)?;
    Ok(code)
}

// ================================= Invitation ================================

/// Published by the inviting device to every connector when it creates a
/// join code: the current epoch key sealed to the invite X25519 key, plus
/// everything a new device needs to trust the group — the roster and the
/// recovery public keys. Signed by the inviter's device key.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Invitation {
    pub inviter_device_id: DeviceId,
    pub invite_ed25519_public: ByteBuf,
    pub invite_x25519_public: ByteBuf,
    pub key_epoch: u32,
    pub sealed_epoch_key: ByteBuf,
    pub roster: Vec<RosterEntry>,
    pub recovery_ed25519_public: ByteBuf,
    pub recovery_x25519_public: ByteBuf,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
    /// Every earlier epoch the inviter holds, sealed to the invite X25519
    /// key like `sealed_epoch_key`. Omitted from the encoding when empty, so
    /// an invitation with none is byte-identical to one made before this
    /// field existed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub earlier_epoch_keys: Vec<SealedEpochKey>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SignedInvitation {
    pub invitation: Invitation,
    pub signature: Signature,
}

pub fn sign_invitation(signing_key: &SigningKey, invitation: Invitation) -> Result<SignedInvitation, EnvelopeError> {
    validate_earlier_epoch_keys(&invitation.earlier_epoch_keys, invitation.key_epoch)?;
    let canonical = canonical_dag_cbor(&invitation)?;
    let signature = Signature(crypto::sign_bytes(signing_key, INVITATION_SIGNATURE_DOMAIN, &canonical));
    Ok(SignedInvitation { invitation, signature })
}

/// Verifies the invitation was signed by whoever holds `verifying_key`.
/// Callers resolve it from the inviter's entry in the invitation's own
/// roster (a new device) or their own roster (an existing device).
pub fn verify_invitation(verifying_key: &VerifyingKey, signed: &SignedInvitation) -> Result<(), EnvelopeError> {
    let canonical = canonical_dag_cbor(&signed.invitation)?;
    crypto::verify_bytes(verifying_key, INVITATION_SIGNATURE_DOMAIN, &canonical, signed.signature.as_bytes())
}

pub fn encode_signed_invitation(signed: &SignedInvitation) -> Result<Vec<u8>, EnvelopeError> {
    canonical_dag_cbor(signed)
}

pub fn decode_signed_invitation(bytes: &[u8]) -> Result<SignedInvitation, EnvelopeError> {
    let signed: SignedInvitation = decode_canonical_dag_cbor(bytes)?;
    validate_earlier_epoch_keys(&signed.invitation.earlier_epoch_keys, signed.invitation.key_epoch)?;
    Ok(signed)
}

// ================================= Redemption ================================

/// Published by the joining device after it opens an invitation: its
/// public keys and name, addressed to one invitation by CID.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InvitationRedemption {
    pub invitation_cid: String,
    pub device_id: DeviceId,
    pub ed25519_public: ByteBuf,
    pub x25519_public: ByteBuf,
    pub device_name: String,
    pub created_at_ms: i64,
}

/// Signed twice: by the invite key (proves the joiner holds the code) and
/// by the joiner's own device key (proves it holds that device key).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SignedInvitationRedemption {
    pub redemption: InvitationRedemption,
    pub invite_signature: Signature,
    pub device_signature: Signature,
}

fn validate_redemption(redemption: &InvitationRedemption) -> Result<(), EnvelopeError> {
    validate_cid_reference(&redemption.invitation_cid)?;
    if redemption.device_name.chars().count() > MAX_JOIN_NAME_CHARS {
        return Err(EnvelopeError::LimitExceeded("device name"));
    }
    Ok(())
}

pub fn sign_invitation_redemption(
    invite_signing_key: &SigningKey,
    device_signing_key: &SigningKey,
    redemption: InvitationRedemption,
) -> Result<SignedInvitationRedemption, EnvelopeError> {
    validate_redemption(&redemption)?;
    let canonical = canonical_dag_cbor(&redemption)?;
    let invite_signature = Signature(crypto::sign_bytes(invite_signing_key, REDEMPTION_INVITE_SIGNATURE_DOMAIN, &canonical));
    let device_signature = Signature(crypto::sign_bytes(device_signing_key, REDEMPTION_DEVICE_SIGNATURE_DOMAIN, &canonical));
    Ok(SignedInvitationRedemption { redemption, invite_signature, device_signature })
}

/// Verifies both signatures: the invite signature against
/// `invite_verifying_key` (from the invitation this redemption names), and
/// the device signature against the redemption's own embedded key.
pub fn verify_invitation_redemption(
    invite_verifying_key: &VerifyingKey,
    signed: &SignedInvitationRedemption,
) -> Result<(), EnvelopeError> {
    validate_redemption(&signed.redemption)?;
    let canonical = canonical_dag_cbor(&signed.redemption)?;
    crypto::verify_bytes(
        invite_verifying_key,
        REDEMPTION_INVITE_SIGNATURE_DOMAIN,
        &canonical,
        signed.invite_signature.as_bytes(),
    )?;
    let device_key_bytes: [u8; 32] =
        signed.redemption.ed25519_public.as_slice().try_into().map_err(|_| EnvelopeError::SignatureInvalid)?;
    let device_key = VerifyingKey::from_bytes(&device_key_bytes).map_err(|_| EnvelopeError::SignatureInvalid)?;
    crypto::verify_bytes(
        &device_key,
        REDEMPTION_DEVICE_SIGNATURE_DOMAIN,
        &canonical,
        signed.device_signature.as_bytes(),
    )
}

pub fn encode_signed_invitation_redemption(signed: &SignedInvitationRedemption) -> Result<Vec<u8>, EnvelopeError> {
    canonical_dag_cbor(signed)
}

pub fn decode_signed_invitation_redemption(bytes: &[u8]) -> Result<SignedInvitationRedemption, EnvelopeError> {
    decode_canonical_dag_cbor(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute_cid;

    fn sample_code() -> JoinCode {
        JoinCode {
            version: JOIN_CODE_VERSION,
            invite_secret: ByteBuf::from(vec![7u8; INVITE_SECRET_LEN]),
            invitation_cid: compute_cid(b"invitation"),
            inviter_name: "Work laptop".to_string(),
            expires_at_ms: 1_800_000_000_000,
            connectors: vec![JoinConnector {
                kind: "s3".to_string(),
                config_json: r#"{"endpoint":"https://s3.example.com","region":"auto","bucket":"b-1"}"#.to_string(),
                secrets_json: Some(r#"{"accessKeyId":"AKIA","secretAccessKey":"secret-value"}"#.to_string()),
            }],
        }
    }

    #[test]
    fn round_trips_and_ignores_whitespace_and_line_breaks() {
        let code = sample_code();
        let text = encode_join_code(&code).unwrap();
        assert!(text.starts_with(JOIN_CODE_PREFIX));
        assert_eq!(decode_join_code(&text).unwrap(), code);

        let wrapped: String = text
            .chars()
            .enumerate()
            .flat_map(|(index, c)| if index % 20 == 19 { vec![c, '\n', ' '] } else { vec![c] })
            .collect();
        assert_eq!(decode_join_code(&format!("  {wrapped}\r\n")).unwrap(), code);
    }

    #[test]
    fn truncation_or_a_typo_is_incomplete_not_garbage() {
        let text = encode_join_code(&sample_code()).unwrap();
        assert_eq!(decode_join_code(&text[..text.len() - 5]), Err(JoinCodeError::Incomplete));
        let mut chars: Vec<char> = text.chars().collect();
        let index = chars.len() / 2;
        chars[index] = if chars[index] == 'A' { 'B' } else { 'A' };
        assert_eq!(decode_join_code(&chars.into_iter().collect::<String>()), Err(JoinCodeError::Incomplete));
        assert_eq!(decode_join_code("TSJOIN1-!!!not-base64"), Err(JoinCodeError::Incomplete));
        assert_eq!(decode_join_code("TSJOIN1-"), Err(JoinCodeError::Incomplete));
    }

    #[test]
    fn other_text_is_not_a_join_code() {
        assert_eq!(decode_join_code("abandon ability able about"), Err(JoinCodeError::NotAJoinCode));
        assert_eq!(decode_join_code(""), Err(JoinCodeError::NotAJoinCode));
        assert_eq!(decode_join_code("TSJOIN0-abc"), Err(JoinCodeError::NotAJoinCode));
    }

    #[test]
    fn a_newer_prefix_or_version_asks_for_an_update() {
        assert_eq!(decode_join_code("TSJOIN2-anything"), Err(JoinCodeError::NewerVersion));

        // A v1 prefix around a body declaring a newer version (with fields
        // this version doesn't know).
        #[derive(Serialize)]
        struct Future {
            version: u16,
            something_new: String,
        }
        let mut payload = canonical_dag_cbor(&Future { version: 2, something_new: "x".to_string() }).unwrap();
        let checksum = Sha256::digest(&payload);
        payload.extend_from_slice(&checksum[..CHECKSUM_LEN]);
        let text = format!("{JOIN_CODE_PREFIX}{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload));
        assert_eq!(decode_join_code(&text), Err(JoinCodeError::NewerVersion));
    }

    #[test]
    fn a_valid_checksum_over_invalid_contents_is_damaged() {
        let mut code = sample_code();
        code.invite_secret = ByteBuf::from(vec![1u8; 16]);
        let mut payload = canonical_dag_cbor(&code).unwrap();
        let checksum = Sha256::digest(&payload);
        payload.extend_from_slice(&checksum[..CHECKSUM_LEN]);
        let text = format!("{JOIN_CODE_PREFIX}{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload));
        assert_eq!(decode_join_code(&text), Err(JoinCodeError::Malformed));

        let mut bad_cid = sample_code();
        bad_cid.invitation_cid = "not-a-cid".to_string();
        assert_eq!(encode_join_code(&bad_cid), Err(JoinCodeError::Malformed));
        let mut empty = sample_code();
        empty.connectors.clear();
        assert_eq!(encode_join_code(&empty), Err(JoinCodeError::Malformed));
    }

    #[test]
    fn inviter_name_limit_below_at_and_above() {
        for (length, ok) in [(MAX_JOIN_NAME_CHARS - 1, true), (MAX_JOIN_NAME_CHARS, true), (MAX_JOIN_NAME_CHARS + 1, false)] {
            let mut code = sample_code();
            code.inviter_name = "é".repeat(length);
            assert_eq!(encode_join_code(&code).is_ok(), ok, "length {length}");
        }
    }

    #[test]
    fn connector_count_limit_below_at_and_above() {
        for (count, ok) in [(MAX_JOIN_CONNECTORS - 1, true), (MAX_JOIN_CONNECTORS, true), (MAX_JOIN_CONNECTORS + 1, false)] {
            let mut code = sample_code();
            code.connectors = vec![code.connectors[0].clone(); count];
            let result = encode_join_code(&code);
            assert_eq!(result.is_ok(), ok, "count {count}");
            if !ok {
                assert_eq!(result, Err(JoinCodeError::LimitExceeded("connector count")));
            }
        }
    }

    type FieldCase = (usize, fn(&mut JoinConnector, usize), &'static str);

    #[test]
    fn connector_field_limits_below_at_and_above() {
        let cases: [FieldCase; 3] = [
            (MAX_JOIN_CONNECTOR_KIND_BYTES, |c, n| c.kind = "k".repeat(n), "connector kind"),
            (MAX_JOIN_CONNECTOR_CONFIG_BYTES, |c, n| c.config_json = "c".repeat(n), "connector config"),
            (MAX_JOIN_CONNECTOR_SECRETS_BYTES, |c, n| c.secrets_json = Some("s".repeat(n)), "connector secrets"),
        ];
        for (limit, set, name) in cases {
            for (length, ok) in [(limit - 1, true), (limit, true), (limit + 1, false)] {
                let mut code = sample_code();
                set(&mut code.connectors[0], length);
                let result = encode_join_code(&code);
                assert_eq!(result.is_ok(), ok, "{name} length {length}");
                if !ok {
                    assert_eq!(result, Err(JoinCodeError::LimitExceeded(name)));
                }
            }
        }
    }

    #[test]
    fn total_length_limit_below_at_and_above() {
        let too_long = "A".repeat(MAX_JOIN_CODE_CHARS + 1);
        assert_eq!(decode_join_code(&too_long), Err(JoinCodeError::TooLong));
        // At the limit the length check passes and decoding proceeds (and
        // fails for an unrelated reason).
        let at_limit = format!("{JOIN_CODE_PREFIX}{}", "A".repeat(MAX_JOIN_CODE_CHARS - JOIN_CODE_PREFIX.len()));
        assert_ne!(decode_join_code(&at_limit), Err(JoinCodeError::TooLong));
        // Whitespace doesn't count toward the limit.
        let text = encode_join_code(&sample_code()).unwrap();
        assert!(decode_join_code(&format!("{text}{}", " ".repeat(MAX_JOIN_CODE_CHARS))).is_ok());
        // An encoder input that would exceed it is refused.
        let mut huge = sample_code();
        huge.connectors = vec![
            JoinConnector {
                kind: "s3".to_string(),
                config_json: "c".repeat(MAX_JOIN_CONNECTOR_CONFIG_BYTES),
                secrets_json: Some("s".repeat(MAX_JOIN_CONNECTOR_SECRETS_BYTES)),
            };
            MAX_JOIN_CONNECTORS
        ];
        assert_eq!(encode_join_code(&huge), Err(JoinCodeError::TooLong));
    }

    #[test]
    fn debug_output_redacts_the_invite_secret_and_connector_secrets() {
        let debug = format!("{:?}", sample_code());
        assert!(!debug.contains("secret-value"));
        assert!(!debug.contains("[7, 7"));
        assert!(debug.contains("<redacted>"));
    }

    #[test]
    fn derived_invite_keys_are_distinct_and_deterministic() {
        let secret = [9u8; INVITE_SECRET_LEN];
        assert_eq!(invite_ed25519_signing_key(&secret).to_bytes(), invite_ed25519_signing_key(&secret).to_bytes());
        assert_ne!(invite_ed25519_signing_key(&secret).to_bytes(), invite_x25519_secret(&secret).to_bytes());
        assert_ne!(invite_ed25519_signing_key(&secret).to_bytes(), invite_ed25519_signing_key(&[8u8; 32]).to_bytes());
    }

    fn sample_redemption(invitation_cid: String, device_key: &SigningKey) -> InvitationRedemption {
        InvitationRedemption {
            invitation_cid,
            device_id: DeviceId::from_bytes([5; 16]),
            ed25519_public: ByteBuf::from(device_key.verifying_key().to_bytes().to_vec()),
            x25519_public: ByteBuf::from(vec![6u8; 32]),
            device_name: "New phone".to_string(),
            created_at_ms: 1,
        }
    }

    #[test]
    fn a_redemption_needs_both_the_invite_and_the_device_signature() {
        let invite_key = invite_ed25519_signing_key(&[1u8; 32]);
        let device_key = SigningKey::from_bytes(&[2u8; 32]);
        let redemption = sample_redemption(compute_cid(b"invitation"), &device_key);
        let signed = sign_invitation_redemption(&invite_key, &device_key, redemption.clone()).unwrap();
        verify_invitation_redemption(&invite_key.verifying_key(), &signed).unwrap();

        // Wrong invite key (someone without the code).
        let other_invite = invite_ed25519_signing_key(&[3u8; 32]);
        assert_eq!(verify_invitation_redemption(&other_invite.verifying_key(), &signed), Err(EnvelopeError::SignatureInvalid));
        let forged = sign_invitation_redemption(&other_invite, &device_key, redemption.clone()).unwrap();
        assert_eq!(verify_invitation_redemption(&invite_key.verifying_key(), &forged), Err(EnvelopeError::SignatureInvalid));

        // A device signature from a key other than the embedded one.
        let impostor = SigningKey::from_bytes(&[4u8; 32]);
        let mismatched = sign_invitation_redemption(&invite_key, &impostor, redemption.clone()).unwrap();
        assert_eq!(verify_invitation_redemption(&invite_key.verifying_key(), &mismatched), Err(EnvelopeError::SignatureInvalid));

        // Any tampered field fails.
        let mut tampered = signed.clone();
        tampered.redemption.device_name = "Someone else".to_string();
        assert_eq!(verify_invitation_redemption(&invite_key.verifying_key(), &tampered), Err(EnvelopeError::SignatureInvalid));
        let mut swapped = signed;
        swapped.invite_signature = swapped.device_signature;
        assert_eq!(verify_invitation_redemption(&invite_key.verifying_key(), &swapped), Err(EnvelopeError::SignatureInvalid));
    }

    #[test]
    fn a_redemption_name_limit_and_cid_are_enforced() {
        let invite_key = invite_ed25519_signing_key(&[1u8; 32]);
        let device_key = SigningKey::from_bytes(&[2u8; 32]);
        let mut redemption = sample_redemption(compute_cid(b"invitation"), &device_key);
        redemption.device_name = "n".repeat(MAX_JOIN_NAME_CHARS);
        assert!(sign_invitation_redemption(&invite_key, &device_key, redemption.clone()).is_ok());
        redemption.device_name = "n".repeat(MAX_JOIN_NAME_CHARS + 1);
        assert!(matches!(
            sign_invitation_redemption(&invite_key, &device_key, redemption.clone()),
            Err(EnvelopeError::LimitExceeded("device name"))
        ));
        redemption.device_name = String::new();
        redemption.invitation_cid = "not-a-cid".to_string();
        assert!(matches!(
            sign_invitation_redemption(&invite_key, &device_key, redemption),
            Err(EnvelopeError::InvalidCidReference)
        ));
    }

    #[test]
    fn invitation_signatures_cover_every_field() {
        let inviter = SigningKey::from_bytes(&[2u8; 32]);
        let invitation = Invitation {
            inviter_device_id: DeviceId::from_bytes([1; 16]),
            invite_ed25519_public: ByteBuf::from(vec![3u8; 32]),
            invite_x25519_public: ByteBuf::from(vec![4u8; 32]),
            key_epoch: 2,
            sealed_epoch_key: ByteBuf::from(vec![5u8; 72]),
            roster: vec![],
            recovery_ed25519_public: ByteBuf::from(vec![6u8; 32]),
            recovery_x25519_public: ByteBuf::from(vec![7u8; 32]),
            created_at_ms: 10,
            expires_at_ms: 20,
            earlier_epoch_keys: vec![],
        };
        let signed = sign_invitation(&inviter, invitation).unwrap();
        verify_invitation(&inviter.verifying_key(), &signed).unwrap();
        let mut tampered = signed.clone();
        tampered.invitation.expires_at_ms = 30;
        assert_eq!(verify_invitation(&inviter.verifying_key(), &tampered), Err(EnvelopeError::SignatureInvalid));
        assert!(verify_invitation(&SigningKey::from_bytes(&[9u8; 32]).verifying_key(), &signed).is_err());
    }

    #[test]
    fn control_objects_never_decode_as_each_other() {
        let inviter = SigningKey::from_bytes(&[2u8; 32]);
        let invitation = sign_invitation(
            &inviter,
            Invitation {
                inviter_device_id: DeviceId::from_bytes([1; 16]),
                invite_ed25519_public: ByteBuf::from(vec![3u8; 32]),
                invite_x25519_public: ByteBuf::from(vec![4u8; 32]),
                key_epoch: 1,
                sealed_epoch_key: ByteBuf::from(vec![5u8; 72]),
                roster: vec![],
                recovery_ed25519_public: ByteBuf::from(vec![6u8; 32]),
                recovery_x25519_public: ByteBuf::from(vec![7u8; 32]),
                created_at_ms: 1,
                expires_at_ms: 2,
                earlier_epoch_keys: vec![],
            },
        )
        .unwrap();
        let invitation_bytes = encode_signed_invitation(&invitation).unwrap();
        let redemption = sign_invitation_redemption(
            &invite_ed25519_signing_key(&[1u8; 32]),
            &inviter,
            sample_redemption(compute_cid(&invitation_bytes), &inviter),
        )
        .unwrap();
        let redemption_bytes = encode_signed_invitation_redemption(&redemption).unwrap();

        assert!(decode_signed_invitation_redemption(&invitation_bytes).is_err());
        assert!(decode_signed_invitation(&redemption_bytes).is_err());
        for bytes in [&invitation_bytes, &redemption_bytes] {
            assert!(crate::decode_signed_enrollment_request(bytes).is_err());
            assert!(crate::decode_signed_enrollment_grant(bytes).is_err());
            assert!(crate::decode_signed_key_rotation(bytes).is_err());
            assert!(crate::decode_signed_head(bytes).is_err());
        }
    }
}
