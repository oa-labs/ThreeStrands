//! Frozen join-code and invitation vectors. A join code is a persistent,
//! cross-version format: a code created by one app version must keep
//! decoding in every later one. If any assertion here changes, the format
//! changed — bump `JOIN_CODE_VERSION` and keep decoding version 1 instead.

use serde_bytes::ByteBuf;
use threestrands_sync_envelope::{
    decode_join_code, decode_signed_invitation, decode_signed_invitation_redemption, encode_join_code,
    encode_signed_invitation, encode_signed_invitation_redemption, invite_ed25519_signing_key, invite_x25519_secret,
    sign_invitation, sign_invitation_redemption, verify_invitation, verify_invitation_redemption, DeviceId,
    Invitation, InvitationRedemption, JoinCode, JoinConnector, RosterEntry, SealedEpochKey, SigningKey, X25519PublicKey,
    JOIN_CODE_VERSION,
};

const INVITE_SECRET: [u8; 32] = [0x5a; 32];
const INVITER_SEED: [u8; 32] = [0x61; 32];
const JOINER_SEED: [u8; 32] = [0x62; 32];

const GOLDEN_JOIN_CODE: &str = include_str!("vectors/join_code_v1.txt");
const GOLDEN_INVITE_KEYS_HEX: &str = include_str!("vectors/invite_keys_v1.hex");
const GOLDEN_INVITATION_HEX: &str = include_str!("vectors/invitation_v1.hex");
const GOLDEN_REDEMPTION_HEX: &str = include_str!("vectors/invitation_redemption_v1.hex");

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(hex: &str) -> Vec<u8> {
    let hex = hex.trim();
    (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect()
}

fn golden_code() -> JoinCode {
    JoinCode {
        version: JOIN_CODE_VERSION,
        invite_secret: ByteBuf::from(INVITE_SECRET.to_vec()),
        invitation_cid: "bafkreihyp2mdkcvn2et4tbcjqsirtmpevqgemx5ab2ac5oioyzmfkwlhlu".to_string(),
        inviter_name: "Golden laptop".to_string(),
        expires_at_ms: 1_800_000_000_000,
        connectors: vec![
            JoinConnector {
                kind: "s3".to_string(),
                config_json: r#"{"endpoint":"https://s3.example.com","region":"auto","bucket":"golden","prefix":"sync","pathStyle":false}"#.to_string(),
                secrets_json: Some(r#"{"accessKeyId":"AKIAGOLDEN","secretAccessKey":"golden-secret"}"#.to_string()),
            },
            JoinConnector {
                kind: "folder".to_string(),
                config_json: r#"{"folderName":"ThreeStrands"}"#.to_string(),
                secrets_json: None,
            },
        ],
    }
}

fn golden_invitation() -> Invitation {
    let invite_signing = invite_ed25519_signing_key(&INVITE_SECRET);
    let invite_x25519 = X25519PublicKey::from(&invite_x25519_secret(&INVITE_SECRET));
    let inviter = SigningKey::from_bytes(&INVITER_SEED);
    Invitation {
        inviter_device_id: DeviceId::from_bytes([0x11; 16]),
        invite_ed25519_public: ByteBuf::from(invite_signing.verifying_key().to_bytes().to_vec()),
        invite_x25519_public: ByteBuf::from(invite_x25519.to_bytes().to_vec()),
        key_epoch: 3,
        // Fixed bytes: a real sealed box is randomized, and this vector
        // pins the encoding, not the sealing.
        sealed_epoch_key: ByteBuf::from(vec![0x77; 72]),
        roster: vec![RosterEntry {
            device_id: DeviceId::from_bytes([0x11; 16]),
            ed25519_public: ByteBuf::from(inviter.verifying_key().to_bytes().to_vec()),
            x25519_public: ByteBuf::from(vec![0x33; 32]),
            status: "active".to_string(),
        }],
        recovery_ed25519_public: ByteBuf::from(vec![0x44; 32]),
        recovery_x25519_public: ByteBuf::from(vec![0x55; 32]),
        created_at_ms: 1_700_000_000_000,
        expires_at_ms: 1_700_086_400_000,
        earlier_epoch_keys: vec![],
    }
}

#[test]
fn the_frozen_v1_join_code_decodes_and_re_encodes_identically() {
    let decoded = decode_join_code(GOLDEN_JOIN_CODE).unwrap();
    assert_eq!(decoded, golden_code());
    assert_eq!(encode_join_code(&golden_code()).unwrap(), GOLDEN_JOIN_CODE.trim());
}

#[test]
fn invite_keys_derive_to_the_frozen_public_keys() {
    let ed25519 = invite_ed25519_signing_key(&INVITE_SECRET).verifying_key().to_bytes();
    let x25519 = X25519PublicKey::from(&invite_x25519_secret(&INVITE_SECRET)).to_bytes();
    assert_eq!(format!("{}{}", hex_encode(&ed25519), hex_encode(&x25519)), GOLDEN_INVITE_KEYS_HEX.trim());
}

#[test]
fn the_signed_invitation_encodes_to_the_frozen_bytes() {
    let inviter = SigningKey::from_bytes(&INVITER_SEED);
    let signed = sign_invitation(&inviter, golden_invitation()).unwrap();
    let bytes = encode_signed_invitation(&signed).unwrap();
    assert_eq!(hex_encode(&bytes), GOLDEN_INVITATION_HEX.trim());
    let decoded = decode_signed_invitation(&hex_decode(GOLDEN_INVITATION_HEX)).unwrap();
    verify_invitation(&inviter.verifying_key(), &decoded).unwrap();
}

#[test]
fn an_invitation_carries_earlier_epoch_keys_under_its_signature() {
    let inviter = SigningKey::from_bytes(&INVITER_SEED);
    let mut invitation = golden_invitation();
    invitation.earlier_epoch_keys = (0..invitation.key_epoch)
        .map(|key_epoch| SealedEpochKey { key_epoch, sealed_key: ByteBuf::from(vec![key_epoch as u8; 72]) })
        .collect();
    let signed = sign_invitation(&inviter, invitation).unwrap();
    let bytes = encode_signed_invitation(&signed).unwrap();
    assert_ne!(hex_encode(&bytes), GOLDEN_INVITATION_HEX.trim());
    let decoded = decode_signed_invitation(&bytes).unwrap();
    assert_eq!(decoded, signed);
    verify_invitation(&inviter.verifying_key(), &decoded).unwrap();

    let mut tampered = decoded;
    tampered.invitation.earlier_epoch_keys.remove(0);
    assert!(verify_invitation(&inviter.verifying_key(), &tampered).is_err());

    let mut out_of_order = golden_invitation();
    out_of_order.earlier_epoch_keys = vec![
        SealedEpochKey { key_epoch: 2, sealed_key: ByteBuf::from(vec![2u8; 72]) },
        SealedEpochKey { key_epoch: 1, sealed_key: ByteBuf::from(vec![1u8; 72]) },
    ];
    assert!(sign_invitation(&inviter, out_of_order).is_err());
}

#[test]
fn the_signed_redemption_encodes_to_the_frozen_bytes() {
    let joiner = SigningKey::from_bytes(&JOINER_SEED);
    let redemption = InvitationRedemption {
        invitation_cid: "bafkreihyp2mdkcvn2et4tbcjqsirtmpevqgemx5ab2ac5oioyzmfkwlhlu".to_string(),
        device_id: DeviceId::from_bytes([0x22; 16]),
        ed25519_public: ByteBuf::from(joiner.verifying_key().to_bytes().to_vec()),
        x25519_public: ByteBuf::from(vec![0x66; 32]),
        device_name: "Golden phone".to_string(),
        created_at_ms: 1_700_000_100_000,
    };
    let invite_key = invite_ed25519_signing_key(&INVITE_SECRET);
    let signed = sign_invitation_redemption(&invite_key, &joiner, redemption).unwrap();
    let bytes = encode_signed_invitation_redemption(&signed).unwrap();
    assert_eq!(hex_encode(&bytes), GOLDEN_REDEMPTION_HEX.trim());
    let decoded = decode_signed_invitation_redemption(&hex_decode(GOLDEN_REDEMPTION_HEX)).unwrap();
    verify_invitation_redemption(&invite_key.verifying_key(), &decoded).unwrap();
}

/// Regenerates the frozen files. Run deliberately, only for an intentional
/// format change: `cargo test -p threestrands-sync-envelope --test
/// join_code_vectors -- --ignored regenerate`.
#[test]
#[ignore]
fn regenerate() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors");
    std::fs::write(dir.join("join_code_v1.txt"), format!("{}\n", encode_join_code(&golden_code()).unwrap())).unwrap();
    let ed25519 = invite_ed25519_signing_key(&INVITE_SECRET).verifying_key().to_bytes();
    let x25519 = X25519PublicKey::from(&invite_x25519_secret(&INVITE_SECRET)).to_bytes();
    std::fs::write(dir.join("invite_keys_v1.hex"), format!("{}{}\n", hex_encode(&ed25519), hex_encode(&x25519))).unwrap();
    let inviter = SigningKey::from_bytes(&INVITER_SEED);
    let invitation = encode_signed_invitation(&sign_invitation(&inviter, golden_invitation()).unwrap()).unwrap();
    std::fs::write(dir.join("invitation_v1.hex"), format!("{}\n", hex_encode(&invitation))).unwrap();
    let joiner = SigningKey::from_bytes(&JOINER_SEED);
    let redemption = InvitationRedemption {
        invitation_cid: "bafkreihyp2mdkcvn2et4tbcjqsirtmpevqgemx5ab2ac5oioyzmfkwlhlu".to_string(),
        device_id: DeviceId::from_bytes([0x22; 16]),
        ed25519_public: ByteBuf::from(joiner.verifying_key().to_bytes().to_vec()),
        x25519_public: ByteBuf::from(vec![0x66; 32]),
        device_name: "Golden phone".to_string(),
        created_at_ms: 1_700_000_100_000,
    };
    let signed = sign_invitation_redemption(&invite_ed25519_signing_key(&INVITE_SECRET), &joiner, redemption).unwrap();
    std::fs::write(dir.join("invitation_redemption_v1.hex"), format!("{}\n", hex_encode(&encode_signed_invitation_redemption(&signed).unwrap()))).unwrap();
}
