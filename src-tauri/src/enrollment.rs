//! Phase 5: the key hierarchy and device-to-device enrollment protocol.
//!
//! Enrollment/rotation objects are signed-but-public control-plane objects
//! (`threestrands_sync_envelope::enrollment`), published and discovered
//! through the exact same content-addressed object store and `scan()`
//! enumeration every other transport object uses — no new `SyncTransport`
//! capability is needed. The only real secret that ever crosses this layer
//! is an epoch key, and it only ever does so as an anonymous
//! [`threestrands_sync_envelope::seal_to_x25519`] stanza that solely the
//! intended recipient's static X25519 secret can open.
//!
//! Trust bootstraps two ways:
//! - **Peer enrollment**: a new device's request and an existing device's
//!   grant are only self-consistently verified by software. The actual
//!   trust decision is a human comparing [`enrollment_fingerprint`] between
//!   two screens before the new device imports the grant — see
//!   [`confirm_and_import_grant`].
//! - **Recovery-phrase import**: the recovery X25519 public key is never
//!   published anywhere in cleartext (it rides in the roster's `RosterEntry`
//!   list is a *device* list; recovery is a distinct, fixed, always-eligible
//!   recipient carried only inside grant/rotation payloads a device must
//!   already be able to decrypt). Successfully opening a sealed stanza with
//!   a phrase-derived recovery secret is itself the trust proof — see
//!   [`join_with_recovery_phrase`].

use std::sync::Arc;

use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use serde_bytes::ByteBuf;
use threestrands_sync_envelope::{
    compute_cid, decode_signed_enrollment_grant, decode_signed_enrollment_request,
    decode_signed_key_rotation, encode_signed_enrollment_grant, encode_signed_enrollment_request,
    encode_signed_key_rotation, enrollment_fingerprint, generate_recovery_seed,
    recovery_ed25519_signing_key, recovery_phrase_from_seed, recovery_seed_from_phrase,
    recovery_x25519_secret, seal_to_x25519, sign_enrollment_grant, sign_enrollment_request,
    sign_key_rotation, try_open_sealed_box, verify_enrollment_grant, verify_enrollment_request,
    verify_key_rotation, DeviceId as EnvelopeDeviceId, EnrollmentGrant, EnrollmentRequest,
    KeyRotation, RequestId, RosterEntry, SignedEnrollmentGrant, SignedEnrollmentRequest,
    SignedKeyRotation, VerifyingKey, X25519PublicKey,
};
use threestrands_sync_transport::{Cid as TransportCid, SyncTransport};

use crate::db::Database;
use crate::error_text::display;
use crate::replicated_sync::{decode_id, encode_id, random_id, x25519_public_bytes, DeviceIdentity, LocalKeys, SPACE_ID};

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

/// Where an adopted epoch key's secret bytes are persisted. Production uses
/// the OS keychain ([`KeychainEpochKeyStore`]); tests inject an in-memory
/// fake so the enrollment protocol's crypto and SQL logic can be exercised
/// without ever touching the real keychain — the same discipline
/// `Database::local_replicated_keys` already follows by never being called
/// from a test.
pub(crate) trait EpochKeyStore: Send + Sync {
    fn store(&self, key_epoch: u32, key: &[u8; 32]) -> Result<(), String>;
}

pub(crate) struct KeychainEpochKeyStore;

impl EpochKeyStore for KeychainEpochKeyStore {
    fn store(&self, key_epoch: u32, key: &[u8; 32]) -> Result<(), String> {
        crate::replicated_sync::store_epoch_key(key_epoch, key)
    }
}

type RecoveryPublicKeys = ([u8; 32], [u8; 32]);
type RawRecoveryPublicKeyRow = (Option<Vec<u8>>, Option<Vec<u8>>);

fn roster_entry_verifying_key(entry: &RosterEntry) -> Result<VerifyingKey, String> {
    let bytes: [u8; 32] = entry.ed25519_public.as_slice().try_into().map_err(|_| "Invalid roster public key".to_string())?;
    VerifyingKey::from_bytes(&bytes).map_err(|_| "Invalid roster public key".to_string())
}

fn roster_entry_x25519(entry: &RosterEntry) -> Result<[u8; 32], String> {
    entry.x25519_public.as_slice().try_into().map_err(|_| "Invalid roster X25519 key".to_string())
}

// ============================ Status and listing ============================

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum EnrollmentStatus {
    NotStarted,
    AwaitingGrant { request_id: String, fingerprint: String, created_at: String },
    AwaitingConfirmation { request_id: String, fingerprint: String, approver_fingerprint: String },
    Enrolled { device_count: usize },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IncomingEnrollmentRequest {
    pub request_id: String,
    pub fingerprint: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRosterEntry {
    pub device_id: String,
    pub status: String,
    pub is_self: bool,
}

impl Database {
    /// A pure-SQL status read (no keychain I/O) for the Settings panel:
    /// whether this device has never started, is waiting on a grant, has a
    /// grant staged for human fingerprint confirmation, or is fully
    /// enrolled. "Enrolled" is defined as: this device has adopted key
    /// material for the sync space's current `active_epoch`.
    pub fn enrollment_status(&self) -> Result<EnrollmentStatus, String> {
        let connection = self.connection()?;
        let active_epoch: Option<i64> = connection
            .query_row("SELECT active_epoch FROM sync_spaces WHERE id=?1", params![SPACE_ID], |row| row.get(0))
            .optional()
            .map_err(display)?;
        let Some(active_epoch) = active_epoch else {
            return Ok(EnrollmentStatus::NotStarted);
        };
        let adopted: bool = connection
            .query_row("SELECT 1 FROM sync_epoch_history WHERE key_epoch=?1", params![active_epoch], |_| Ok(true))
            .optional()
            .map_err(display)?
            .unwrap_or(false);
        if adopted {
            let device_count: i64 = connection
                .query_row("SELECT COUNT(*) FROM sync_devices WHERE status='active'", [], |row| row.get(0))
                .map_err(display)?;
            return Ok(EnrollmentStatus::Enrolled { device_count: device_count as usize });
        }

        let staged: Option<(String, String, Vec<u8>)> = connection
            .query_row(
                "SELECT request_id, fingerprint, pending_grant_cbor FROM replicated_sync_enrollment_requests
                 WHERE direction='outgoing' AND status='staged' ORDER BY created_at DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(display)?;
        if let Some((request_id, fingerprint, grant_cbor)) = staged {
            let approver_fingerprint = decode_signed_enrollment_grant(&grant_cbor)
                .ok()
                .and_then(|signed| signed.grant.roster.iter().find(|entry| entry.device_id == signed.grant.approver_device_id).cloned())
                .and_then(|entry| Some(enrollment_fingerprint(&entry.ed25519_public, roster_entry_x25519(&entry).ok()?.as_slice())))
                .unwrap_or_else(|| "(unavailable)".to_string());
            return Ok(EnrollmentStatus::AwaitingConfirmation { request_id, fingerprint, approver_fingerprint });
        }

        let pending: Option<(String, String, String)> = connection
            .query_row(
                "SELECT request_id, fingerprint, created_at FROM replicated_sync_enrollment_requests
                 WHERE direction='outgoing' AND status='pending' ORDER BY created_at DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(display)?;
        match pending {
            Some((request_id, fingerprint, created_at)) => Ok(EnrollmentStatus::AwaitingGrant { request_id, fingerprint, created_at }),
            None => Ok(EnrollmentStatus::NotStarted),
        }
    }

    pub fn pending_incoming_enrollment_requests(&self) -> Result<Vec<IncomingEnrollmentRequest>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT request_id, fingerprint, created_at FROM replicated_sync_enrollment_requests
                 WHERE direction='incoming' AND status='pending' ORDER BY created_at ASC",
            )
            .map_err(display)?;
        let rows = statement
            .query_map([], |row| {
                Ok(IncomingEnrollmentRequest { request_id: row.get(0)?, fingerprint: row.get(1)?, created_at: row.get(2)? })
            })
            .map_err(display)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(display)?;
        Ok(rows)
    }

    pub fn reject_enrollment_request(&self, request_id_hex: &str) -> Result<(), String> {
        self.connection()?
            .execute(
                "UPDATE replicated_sync_enrollment_requests SET status='rejected' WHERE request_id=?1 AND direction='incoming'",
                params![request_id_hex],
            )
            .map_err(display)?;
        Ok(())
    }

    pub fn device_roster(&self) -> Result<Vec<DeviceRosterEntry>, String> {
        let self_device_id = self.local_self_device_id_hex()?;
        let connection = self.connection()?;
        let mut statement = connection.prepare("SELECT device_id, status FROM sync_devices ORDER BY device_id").map_err(display)?;
        let rows = statement
            .query_map([], |row| {
                let device_id: String = row.get(0)?;
                let status: String = row.get(1)?;
                Ok((device_id, status))
            })
            .map_err(display)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(display)?;
        Ok(rows
            .into_iter()
            .map(|(device_id, status)| {
                let is_self = self_device_id.as_deref() == Some(device_id.as_str());
                DeviceRosterEntry { device_id, status, is_self }
            })
            .collect())
    }

    fn local_self_device_id_hex(&self) -> Result<Option<String>, String> {
        self.connection()?
            .query_row("SELECT device_id FROM sync_devices LIMIT 1", [], |row| row.get(0))
            .optional()
            .map_err(display)
    }

    fn seen_control_object(&self, cid: &str) -> Result<bool, String> {
        self.connection()?
            .query_row("SELECT 1 FROM sync_control_objects_seen WHERE cid=?1", params![cid], |_| Ok(true))
            .optional()
            .map_err(display)
            .map(|found| found.unwrap_or(false))
    }

    fn mark_control_object_seen(&self, cid: &str, kind: &str) -> Result<(), String> {
        self.connection()?
            .execute(
                "INSERT OR IGNORE INTO sync_control_objects_seen(cid, object_kind) VALUES (?1,?2)",
                params![cid, kind],
            )
            .map_err(display)?;
        Ok(())
    }

    fn record_epoch_activation(&self, key_epoch: u32, source_cid: &str) -> Result<(), String> {
        self.connection()?
            .execute(
                "INSERT OR IGNORE INTO sync_epoch_history(key_epoch, activated_at, source_cid) VALUES (?1,?2,?3)",
                params![key_epoch, Utc::now().to_rfc3339(), source_cid],
            )
            .map_err(display)?;
        Ok(())
    }

    fn set_active_epoch(&self, key_epoch: u32) -> Result<(), String> {
        self.connection()?
            .execute("UPDATE sync_spaces SET active_epoch=?2 WHERE id=?1", params![SPACE_ID, key_epoch])
            .map_err(display)?;
        Ok(())
    }

    fn set_recovery_public_keys(&self, ed25519_public: &[u8; 32], x25519_public: &[u8; 32]) -> Result<(), String> {
        self.connection()?
            .execute(
                "UPDATE sync_spaces SET recovery_public_key=?2, recovery_x25519_public=?3 WHERE id=?1",
                params![SPACE_ID, ed25519_public.to_vec(), x25519_public.to_vec()],
            )
            .map_err(display)?;
        Ok(())
    }

    fn recovery_public_keys(&self) -> Result<Option<RecoveryPublicKeys>, String> {
        let row: Option<RawRecoveryPublicKeyRow> = self
            .connection()?
            .query_row("SELECT recovery_public_key, recovery_x25519_public FROM sync_spaces WHERE id=?1", params![SPACE_ID], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()
            .map_err(display)?;
        let Some((ed25519, x25519)) = row else { return Ok(None) };
        let (Some(ed25519), Some(x25519)) = (ed25519, x25519) else { return Ok(None) };
        let ed25519: [u8; 32] = ed25519.try_into().map_err(|_| "Invalid stored recovery Ed25519 key".to_string())?;
        let x25519: [u8; 32] = x25519.try_into().map_err(|_| "Invalid stored recovery X25519 key".to_string())?;
        Ok(Some((ed25519, x25519)))
    }

    fn adopt_roster(&self, roster: &[RosterEntry]) -> Result<(), String> {
        for entry in roster {
            let verifying_key = roster_entry_verifying_key(entry)?;
            let x25519_public = roster_entry_x25519(entry)?;
            let device_id = *entry.device_id.as_bytes();
            self.trust_device_keys(&device_id, &verifying_key, &x25519_public)?;
            if entry.status == "revoked" {
                self.revoke_device(&device_id)?;
            }
        }
        Ok(())
    }
}

// ============================ Publishing helpers ============================

async fn publish_to_all(transports: &[Arc<dyn SyncTransport>], bytes: &[u8]) -> String {
    let cid = compute_cid(bytes);
    let locator = TransportCid(cid.clone());
    for transport in transports {
        let _ = transport.put_object(&locator, bytes).await;
    }
    cid
}

impl Database {
    /// Every device this local database knows about, active *or* revoked —
    /// the full snapshot published inside a grant or rotation object, so a
    /// recipient learns a revocation explicitly (a `RosterEntry` with
    /// `status: "revoked"`) rather than merely inferring it from an
    /// omission, which a device that missed the rotation could not do.
    fn full_roster_snapshot(&self) -> Result<Vec<RosterEntry>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare("SELECT device_id, public_key, x25519_public, status FROM sync_devices WHERE public_key IS NOT NULL AND x25519_public IS NOT NULL")
            .map_err(display)?;
        let rows: Vec<(String, Vec<u8>, Vec<u8>, String)> = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
            .map_err(display)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(display)?;
        rows.into_iter()
            .map(|(device_id_hex, ed25519, x25519, status)| {
                Ok(RosterEntry {
                    device_id: EnvelopeDeviceId::from_bytes(decode_id(&device_id_hex)?),
                    ed25519_public: ByteBuf::from(ed25519),
                    x25519_public: ByteBuf::from(x25519),
                    status,
                })
            })
            .collect()
    }
}

// ============================ Genesis and joining ============================

/// Starts a brand-new sync space on this device: generates its own device
/// keys, a fresh epoch-0 key, and a recovery seed, publishes a genesis
/// [`KeyRotation`] (epoch 0, sealed to itself and to the recovery X25519
/// key) so a later recovery-phrase import has something to find, and
/// returns the recovery phrase. **The phrase is shown to the caller exactly
/// once and is never persisted anywhere** — only its two derived public
/// keys are kept, in `sync_spaces`.
pub async fn begin_genesis(database: &Database, identity: &DeviceIdentity, epoch_keys: &dyn EpochKeyStore, transports: &[Arc<dyn SyncTransport>]) -> Result<String, String> {
    database.set_beta_features_enabled(true)?;

    let seed = generate_recovery_seed();
    let phrase = recovery_phrase_from_seed(&seed);
    let recovery_x25519_public = X25519PublicKey::from(&recovery_x25519_secret(&seed)).to_bytes();
    let recovery_ed25519_public = recovery_ed25519_signing_key(&seed).verifying_key().to_bytes();
    database.set_recovery_public_keys(&recovery_ed25519_public, &recovery_x25519_public)?;

    let identity_x25519_public = x25519_public_bytes(&identity.x25519_secret);
    let mut k_epoch = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut k_epoch);

    let rotation = KeyRotation {
        key_epoch: 0,
        initiator_device_id: identity.device_id,
        roster: vec![RosterEntry {
            device_id: identity.device_id,
            ed25519_public: ByteBuf::from(identity.verifying_key.to_bytes().to_vec()),
            x25519_public: ByteBuf::from(identity_x25519_public.to_vec()),
            status: "active".to_string(),
        }],
        sealed_stanzas: vec![
            ByteBuf::from(seal_to_x25519(&identity_x25519_public, &k_epoch)),
            ByteBuf::from(seal_to_x25519(&recovery_x25519_public, &k_epoch)),
        ],
        recovery_ed25519_public: ByteBuf::from(recovery_ed25519_public.to_vec()),
        recovery_x25519_public: ByteBuf::from(recovery_x25519_public.to_vec()),
        created_at_ms: now_ms(),
    };
    let signed = sign_key_rotation(&identity.signing_key, rotation).map_err(display)?;
    let bytes = encode_signed_key_rotation(&signed).map_err(display)?;
    let cid = publish_to_all(transports, &bytes).await;

    epoch_keys.store(0, &k_epoch)?;
    database.set_active_epoch(0)?;
    database.record_epoch_activation(0, &cid)?;
    database.mark_control_object_seen(&cid, "key_rotation")?;

    Ok(phrase)
}

/// Starts this device as a joiner: bootstraps its own device keys and
/// publishes a signed enrollment request. Returns the request's fingerprint
/// for display — the same value an approving device must see and confirm.
pub async fn publish_enrollment_request(database: &Database, identity: &DeviceIdentity, transports: &[Arc<dyn SyncTransport>]) -> Result<String, String> {
    database.set_beta_features_enabled(true)?;
    let x25519_public = x25519_public_bytes(&identity.x25519_secret);
    let fingerprint = enrollment_fingerprint(identity.verifying_key.as_bytes(), &x25519_public);

    let request_id = RequestId::from_bytes(random_id());
    let request = EnrollmentRequest {
        request_id,
        device_id: identity.device_id,
        ed25519_public: ByteBuf::from(identity.verifying_key.to_bytes().to_vec()),
        x25519_public: ByteBuf::from(x25519_public.to_vec()),
        created_at_ms: now_ms(),
    };
    let signed = sign_enrollment_request(&identity.signing_key, request).map_err(display)?;
    let bytes = encode_signed_enrollment_request(&signed).map_err(display)?;
    let cid = publish_to_all(transports, &bytes).await;
    database.mark_control_object_seen(&cid, "enrollment_request")?;

    database
        .connection()?
        .execute(
            "INSERT INTO replicated_sync_enrollment_requests(request_id, direction, device_id, ed25519_public, x25519_public, fingerprint, status, created_at)
             VALUES (?1,'outgoing',?2,?3,?4,?5,'pending',?6)
             ON CONFLICT(request_id) DO NOTHING",
            params![
                encode_id(request_id.as_bytes()),
                encode_id(identity.device_id.as_bytes()),
                identity.verifying_key.to_bytes().to_vec(),
                x25519_public.to_vec(),
                fingerprint,
                Utc::now().to_rfc3339(),
            ],
        )
        .map_err(display)?;
    Ok(fingerprint)
}

// ============================ The periodic sweep ============================

/// Scans every transport for new enrollment-request, grant, and rotation
/// objects, applying what it safely can:
/// - A new request from an unrecognized device is recorded for the human to
///   review (never auto-approved).
/// - A grant matching one of our own pending outgoing requests is staged
///   for human fingerprint confirmation (never auto-imported).
/// - A rotation signed by a device we already trust is applied immediately
///   — no new trust decision is involved, since the signer is already
///   authenticated by our existing roster.
pub async fn run_enrollment_sweep(database: &Database, identity: &DeviceIdentity, epoch_keys: &dyn EpochKeyStore, transports: &[Arc<dyn SyncTransport>]) -> Result<(), String> {
    for transport in transports {
        let mut cursor: Option<String> = None;
        loop {
            let Some(page) = transport.scan(cursor.as_deref()).await.map_err(display)? else { break };
            for locator in &page.objects {
                if database.seen_control_object(&locator.cid.0)? {
                    continue;
                }
                let Ok(bytes) = transport.get_object(&locator.cid).await else { continue };
                if try_apply_control_object(database, identity, epoch_keys, &locator.cid.0, &bytes)? {
                    database.mark_control_object_seen(&locator.cid.0, "control")?;
                } else {
                    // Not recognized as an enrollment/rotation object at
                    // all (most objects are ordinary sealed events) —
                    // still mark seen so we never re-fetch it.
                    database.mark_control_object_seen(&locator.cid.0, "other")?;
                }
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
    }
    Ok(())
}

fn try_apply_control_object(database: &Database, identity: &DeviceIdentity, epoch_keys: &dyn EpochKeyStore, cid: &str, bytes: &[u8]) -> Result<bool, String> {
    if let Ok(signed) = decode_signed_enrollment_request(bytes) {
        apply_incoming_request(database, identity, signed)?;
        return Ok(true);
    }
    if let Ok(signed) = decode_signed_enrollment_grant(bytes) {
        apply_incoming_grant(database, identity, epoch_keys, signed)?;
        return Ok(true);
    }
    if let Ok(signed) = decode_signed_key_rotation(bytes) {
        apply_incoming_rotation(database, identity, epoch_keys, signed, cid)?;
        return Ok(true);
    }
    Ok(false)
}

fn apply_incoming_request(database: &Database, identity: &DeviceIdentity, signed: SignedEnrollmentRequest) -> Result<(), String> {
    if signed.request.device_id == identity.device_id {
        return Ok(()); // our own request, not an incoming one to review
    }
    let verifying_key_bytes: [u8; 32] = match signed.request.ed25519_public.as_slice().try_into() {
        Ok(bytes) => bytes,
        Err(_) => return Ok(()),
    };
    let Ok(verifying_key) = VerifyingKey::from_bytes(&verifying_key_bytes) else { return Ok(()) };
    if verify_enrollment_request(&verifying_key, &signed).is_err() {
        return Ok(());
    }
    let fingerprint = enrollment_fingerprint(&signed.request.ed25519_public, &signed.request.x25519_public);
    database
        .connection()?
        .execute(
            "INSERT INTO replicated_sync_enrollment_requests(request_id, direction, device_id, ed25519_public, x25519_public, fingerprint, status, created_at)
             VALUES (?1,'incoming',?2,?3,?4,?5,'pending',?6)
             ON CONFLICT(request_id) DO NOTHING",
            params![
                encode_id(signed.request.request_id.as_bytes()),
                encode_id(signed.request.device_id.as_bytes()),
                signed.request.ed25519_public.to_vec(),
                signed.request.x25519_public.to_vec(),
                fingerprint,
                Utc::now().to_rfc3339(),
            ],
        )
        .map_err(display)?;
    Ok(())
}

fn apply_incoming_grant(database: &Database, identity: &DeviceIdentity, epoch_keys: &dyn EpochKeyStore, signed: SignedEnrollmentGrant) -> Result<(), String> {
    let request_id_hex = encode_id(signed.grant.request_id.as_bytes());
    let matches_our_request: Option<String> = database
        .connection()?
        .query_row(
            "SELECT request_id FROM replicated_sync_enrollment_requests WHERE request_id=?1 AND direction='outgoing' AND status='pending'",
            params![request_id_hex],
            |row| row.get(0),
        )
        .optional()
        .map_err(display)?;
    if matches_our_request.is_none() {
        // Not addressed to any request of ours. The only other reason to
        // apply a grant is a recovery-signed roster announcement — a
        // device that just recovery-imported broadcasting its own
        // membership (see `join_with_recovery_phrase`). Applied the same
        // way a trusted rotation is: automatically, no staging, since the
        // recovery key is our root trust anchor already on file.
        if !signed.grant.signed_by_recovery {
            return Ok(());
        }
        let Some((recovery_ed25519, _)) = database.recovery_public_keys()? else { return Ok(()) };
        let Ok(verifying_key) = VerifyingKey::from_bytes(&recovery_ed25519) else { return Ok(()) };
        if verify_enrollment_grant(&verifying_key, &signed).is_err() {
            return Ok(());
        }
        database.adopt_roster(&signed.grant.roster)?;
        if let Some(bytes) = try_open_sealed_box(&identity.x25519_secret, &signed.grant.sealed_epoch_key) {
            if let Ok(k_epoch) = bytes.try_into() {
                epoch_keys.store(signed.grant.key_epoch, &k_epoch)?;
            }
        }
        return Ok(());
    }

    // Self-consistency only: the embedded approver key in the grant's own
    // roster, or the recovery key already known to this device. Either
    // way, importing still requires an explicit human confirmation step —
    // see `confirm_and_import_grant`.
    let verifies = if signed.grant.signed_by_recovery {
        database
            .recovery_public_keys()?
            .and_then(|(ed25519, _)| VerifyingKey::from_bytes(&ed25519).ok())
            .map(|verifying_key| verify_enrollment_grant(&verifying_key, &signed).is_ok())
            .unwrap_or(false)
    } else {
        signed
            .grant
            .roster
            .iter()
            .find(|entry| entry.device_id == signed.grant.approver_device_id)
            .and_then(|entry| roster_entry_verifying_key(entry).ok())
            .map(|verifying_key| verify_enrollment_grant(&verifying_key, &signed).is_ok())
            .unwrap_or(false)
    };
    if !verifies {
        return Ok(());
    }
    // Only stage it if it is actually addressed to us: it must open with
    // our own X25519 secret.
    if try_open_sealed_box(&identity.x25519_secret, &signed.grant.sealed_epoch_key).is_none() {
        return Ok(());
    }

    // `enrollment_status` re-derives the approver's fingerprint from the
    // staged CBOR for display, so nothing further to compute here.
    let grant_cbor = encode_signed_enrollment_grant(&signed).map_err(display)?;
    database
        .connection()?
        .execute(
            "UPDATE replicated_sync_enrollment_requests SET status='staged', pending_grant_cbor=?2 WHERE request_id=?1",
            params![request_id_hex, grant_cbor],
        )
        .map_err(display)?;
    Ok(())
}

fn apply_incoming_rotation(database: &Database, identity: &DeviceIdentity, epoch_keys: &dyn EpochKeyStore, signed: SignedKeyRotation, cid: &str) -> Result<(), String> {
    // A rotation is only auto-applied when its signer is already in our
    // roster: no new trust decision, just an authenticated update to an
    // existing one. A genesis/recovery-only rotation (signer not yet
    // trusted) is picked up by `join_with_recovery_phrase` instead.
    let roster = database.known_device_roster()?;
    let Some((_, verifying_key)) = roster.iter().find(|(device_id, _)| *device_id == signed.rotation.initiator_device_id) else {
        return Ok(());
    };
    if verify_key_rotation(verifying_key, &signed).is_err() {
        return Ok(());
    }
    apply_rotation_common(database, identity, epoch_keys, &signed, cid)
}

fn apply_rotation_common(database: &Database, identity: &DeviceIdentity, epoch_keys: &dyn EpochKeyStore, signed: &SignedKeyRotation, cid: &str) -> Result<(), String> {
    database.adopt_roster(&signed.rotation.roster)?;
    database.set_recovery_public_keys(
        signed.rotation.recovery_ed25519_public.as_slice().try_into().map_err(|_| "Invalid recovery key".to_string())?,
        signed.rotation.recovery_x25519_public.as_slice().try_into().map_err(|_| "Invalid recovery key".to_string())?,
    )?;

    // Try every sealed stanza with our own device secret first, then (if we
    // hold recovery material) the recovery secret. Anonymous stanzas mean
    // we cannot know in advance which one, if any, is addressed to us.
    let mut opened: Option<[u8; 32]> = None;
    for stanza in &signed.rotation.sealed_stanzas {
        if let Some(bytes) = try_open_sealed_box(&identity.x25519_secret, stanza) {
            if let Ok(key) = bytes.try_into() {
                opened = Some(key);
                break;
            }
        }
    }
    if let Some(k_epoch) = opened {
        epoch_keys.store(signed.rotation.key_epoch, &k_epoch)?;
        database.set_active_epoch(signed.rotation.key_epoch)?;
        database.record_epoch_activation(signed.rotation.key_epoch, cid)?;
    }
    Ok(())
}

/// The joining device's explicit action after visually comparing
/// [`enrollment_fingerprint`] on both screens: imports a staged grant,
/// adopting the epoch key and the whole roster it carries.
pub async fn confirm_and_import_grant(database: &Database, identity: &DeviceIdentity, epoch_keys: &dyn EpochKeyStore, request_id_hex: &str) -> Result<(), String> {
    let grant_cbor: Vec<u8> = database
        .connection()?
        .query_row(
            "SELECT pending_grant_cbor FROM replicated_sync_enrollment_requests WHERE request_id=?1 AND direction='outgoing' AND status='staged'",
            params![request_id_hex],
            |row| row.get(0),
        )
        .map_err(display)?;
    let signed = decode_signed_enrollment_grant(&grant_cbor).map_err(display)?;
    let k_epoch_bytes = try_open_sealed_box(&identity.x25519_secret, &signed.grant.sealed_epoch_key)
        .ok_or_else(|| "This grant was not sealed to this device".to_string())?;
    let k_epoch: [u8; 32] = k_epoch_bytes.try_into().map_err(|_| "Invalid sealed epoch key".to_string())?;

    database.adopt_roster(&signed.grant.roster)?;
    let recovery_ed25519: [u8; 32] = signed.grant.recovery_ed25519_public.as_slice().try_into().map_err(|_| "Invalid recovery key".to_string())?;
    let recovery_x25519: [u8; 32] = signed.grant.recovery_x25519_public.as_slice().try_into().map_err(|_| "Invalid recovery key".to_string())?;
    database.set_recovery_public_keys(&recovery_ed25519, &recovery_x25519)?;

    epoch_keys.store(signed.grant.key_epoch, &k_epoch)?;
    database.set_active_epoch(signed.grant.key_epoch)?;
    let source_cid = compute_cid(&grant_cbor);
    database.record_epoch_activation(signed.grant.key_epoch, &source_cid)?;

    database
        .connection()?
        .execute(
            "UPDATE replicated_sync_enrollment_requests SET status='completed' WHERE request_id=?1",
            params![request_id_hex],
        )
        .map_err(display)?;
    Ok(())
}

// ============================ Approving a peer ============================

/// The existing-device side of enrollment: after a human has compared
/// fingerprints and approved an incoming request, builds and publishes a
/// grant carrying the current roster and the active epoch key sealed to the
/// requester's X25519 public key alone.
pub async fn approve_enrollment_request(
    database: &Database,
    identity: &DeviceIdentity,
    keys: &LocalKeys,
    request_id_hex: &str,
    transports: &[Arc<dyn SyncTransport>],
) -> Result<(), String> {
    let (requester_device_id, requester_ed25519, requester_x25519): (String, Vec<u8>, Vec<u8>) = database
        .connection()?
        .query_row(
            "SELECT device_id, ed25519_public, x25519_public FROM replicated_sync_enrollment_requests WHERE request_id=?1 AND direction='incoming' AND status='pending'",
            params![request_id_hex],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(display)?;
    let requester_x25519: [u8; 32] = requester_x25519.try_into().map_err(|_| "Invalid requester X25519 key".to_string())?;
    let requester_ed25519_bytes: [u8; 32] = requester_ed25519.try_into().map_err(|_| "Invalid requester Ed25519 key".to_string())?;
    let requester_verifying_key = VerifyingKey::from_bytes(&requester_ed25519_bytes).map_err(|_| "Invalid requester Ed25519 key".to_string())?;
    let requester_device_id_bytes = decode_id(&requester_device_id)?;

    // The approved device must appear in its own grant's roster (and in
    // every roster snapshot this device hands out from now on), so trust
    // it locally before building the snapshot below.
    database.trust_device_keys(&requester_device_id_bytes, &requester_verifying_key, &requester_x25519)?;

    let roster = database.full_roster_snapshot()?;
    let (recovery_ed25519, recovery_x25519) = database.recovery_public_keys()?.ok_or_else(|| "No recovery keys on record for this sync space".to_string())?;

    let grant = EnrollmentGrant {
        request_id: RequestId::from_bytes(decode_id(request_id_hex)?),
        approver_device_id: identity.device_id,
        signed_by_recovery: false,
        key_epoch: keys.key_epoch,
        sealed_epoch_key: ByteBuf::from(seal_to_x25519(&requester_x25519, &keys.k_epoch)),
        roster,
        recovery_ed25519_public: ByteBuf::from(recovery_ed25519.to_vec()),
        recovery_x25519_public: ByteBuf::from(recovery_x25519.to_vec()),
        created_at_ms: now_ms(),
    };
    let signed = sign_enrollment_grant(&identity.signing_key, grant).map_err(display)?;
    let bytes = encode_signed_enrollment_grant(&signed).map_err(display)?;
    let cid = publish_to_all(transports, &bytes).await;
    database.mark_control_object_seen(&cid, "enrollment_grant")?;

    database
        .connection()?
        .execute(
            "UPDATE replicated_sync_enrollment_requests SET status='approved' WHERE request_id=?1",
            params![request_id_hex],
        )
        .map_err(display)?;
    Ok(())
}

// ============================ Rotation and revocation ============================

/// Rotates to a fresh epoch, sealing it to every currently active device
/// (optionally omitting one to revoke it) plus the recovery key. Every
/// other device picks this up on its next sweep via
/// [`run_enrollment_sweep`].
pub async fn rotate_epoch(
    database: &Database,
    identity: &DeviceIdentity,
    keys: &LocalKeys,
    epoch_keys: &dyn EpochKeyStore,
    transports: &[Arc<dyn SyncTransport>],
    revoke_device_id_hex: Option<&str>,
) -> Result<(), String> {
    let (recovery_ed25519, recovery_x25519) = database.recovery_public_keys()?.ok_or_else(|| "No recovery keys on record for this sync space".to_string())?;

    if let Some(hex) = revoke_device_id_hex {
        let device_id = decode_id(hex)?;
        database.revoke_device(&device_id)?;
    }

    // The published roster carries every device, including anyone just
    // revoked, so a device that missed this rotation still learns the
    // revocation explicitly the next time it applies this object. Only
    // the still-active subset receives a sealed stanza for the new key.
    let roster = database.full_roster_snapshot()?;
    let active_recipients: Vec<&RosterEntry> = roster.iter().filter(|entry| entry.status == "active").collect();

    let mut k_epoch = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut k_epoch);
    let next_epoch = keys.key_epoch + 1;

    let mut sealed_stanzas = Vec::with_capacity(active_recipients.len() + 1);
    for entry in &active_recipients {
        let x25519 = roster_entry_x25519(entry)?;
        sealed_stanzas.push(ByteBuf::from(seal_to_x25519(&x25519, &k_epoch)));
    }
    sealed_stanzas.push(ByteBuf::from(seal_to_x25519(&recovery_x25519, &k_epoch)));

    let rotation = KeyRotation {
        key_epoch: next_epoch,
        initiator_device_id: identity.device_id,
        roster,
        sealed_stanzas,
        recovery_ed25519_public: ByteBuf::from(recovery_ed25519.to_vec()),
        recovery_x25519_public: ByteBuf::from(recovery_x25519.to_vec()),
        created_at_ms: now_ms(),
    };
    let signed = sign_key_rotation(&identity.signing_key, rotation).map_err(display)?;
    let bytes = encode_signed_key_rotation(&signed).map_err(display)?;
    let cid = publish_to_all(transports, &bytes).await;
    database.mark_control_object_seen(&cid, "key_rotation")?;

    epoch_keys.store(next_epoch, &k_epoch)?;
    database.set_active_epoch(next_epoch)?;
    database.record_epoch_activation(next_epoch, &cid)?;
    Ok(())
}

// ============================ Recovery-phrase import ============================

/// Joins an existing sync space using only a recovery phrase — no peer
/// device needs to be online. Scans every transport for a rotation object
/// whose sealed stanzas open with the phrase-derived recovery X25519
/// secret; that success is itself the trust proof (see the module docs).
pub async fn join_with_recovery_phrase(database: &Database, identity: &DeviceIdentity, epoch_keys: &dyn EpochKeyStore, phrase: &str, transports: &[Arc<dyn SyncTransport>]) -> Result<(), String> {
    database.set_beta_features_enabled(true)?;
    let seed = recovery_seed_from_phrase(phrase)?;
    let recovery_secret_bytes = recovery_x25519_secret(&seed).to_bytes();

    for transport in transports {
        let mut cursor: Option<String> = None;
        loop {
            let Some(page) = transport.scan(cursor.as_deref()).await.map_err(display)? else { break };
            for locator in &page.objects {
                let Ok(bytes) = transport.get_object(&locator.cid).await else { continue };
                let Ok(signed) = decode_signed_key_rotation(&bytes) else { continue };
                let opened = signed
                    .rotation
                    .sealed_stanzas
                    .iter()
                    .find_map(|stanza| try_open_sealed_box(&recovery_secret_bytes, stanza));
                let Some(k_epoch_bytes) = opened else { continue };
                let Ok(k_epoch): Result<[u8; 32], _> = k_epoch_bytes.try_into() else { continue };

                database.adopt_roster(&signed.rotation.roster)?;
                database.set_recovery_public_keys(
                    signed.rotation.recovery_ed25519_public.as_slice().try_into().map_err(|_| "Invalid recovery key".to_string())?,
                    signed.rotation.recovery_x25519_public.as_slice().try_into().map_err(|_| "Invalid recovery key".to_string())?,
                )?;
                // Trust ourselves alongside the recovered roster — a fresh
                // device recovering has no prior self-entry there.
                let self_x25519_public = x25519_public_bytes(&identity.x25519_secret);
                database.trust_device_keys(identity.device_id.as_bytes(), &identity.verifying_key, &self_x25519_public)?;
                epoch_keys.store(signed.rotation.key_epoch, &k_epoch)?;
                database.set_active_epoch(signed.rotation.key_epoch)?;
                database.record_epoch_activation(signed.rotation.key_epoch, &locator.cid.0)?;

                // Broadcast our own membership so every other device
                // (which has no pending request matching this — nothing
                // asked it to expect us) learns about and trusts us too.
                // Signed by the recovery key, not a peer, so every device
                // that already knows the recovery public key applies it
                // immediately — see `apply_incoming_grant`.
                let recovery_ed25519_public = recovery_ed25519_signing_key(&seed).verifying_key();
                let announcement = EnrollmentGrant {
                    request_id: RequestId::from_bytes(random_id()),
                    approver_device_id: identity.device_id,
                    signed_by_recovery: true,
                    key_epoch: signed.rotation.key_epoch,
                    sealed_epoch_key: ByteBuf::from(seal_to_x25519(&self_x25519_public, &k_epoch)),
                    roster: database.full_roster_snapshot()?,
                    recovery_ed25519_public: ByteBuf::from(recovery_ed25519_public.to_bytes().to_vec()),
                    recovery_x25519_public: signed.rotation.recovery_x25519_public.clone(),
                    created_at_ms: now_ms(),
                };
                let signed_announcement = sign_enrollment_grant(&recovery_ed25519_signing_key(&seed), announcement).map_err(display)?;
                let announcement_bytes = encode_signed_enrollment_grant(&signed_announcement).map_err(display)?;
                let announcement_cid = publish_to_all(transports, &announcement_bytes).await;
                database.mark_control_object_seen(&announcement_cid, "enrollment_grant")?;
                return Ok(());
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
    }
    Err("No rotation object on any configured transport opened with this recovery phrase yet".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::RngCore;
    use threestrands_sync_envelope::SigningKey;
    use threestrands_sync_protocol::EntityType;
    use threestrands_sync_transport::fake::FakeTransport;

    use crate::replicated_sync::{pull_from_transports, push_pending_events};

    /// Builds a synthetic device identity directly, exactly as `test_keys`
    /// does in `replicated_sync.rs` — never touches the OS keychain.
    fn test_identity(database: &Database) -> DeviceIdentity {
        let device_id = {
            let mut connection = database.connection().unwrap();
            let tx = connection.transaction().unwrap();
            let device_id = crate::replicated_sync::ensure_space_and_device(&tx).unwrap();
            tx.commit().unwrap();
            device_id
        };
        let signing_key = SigningKey::generate(&mut rand::rngs::OsRng);
        let verifying_key = signing_key.verifying_key();
        let mut x25519_secret = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut x25519_secret);
        let x25519_public = x25519_public_bytes(&x25519_secret);
        database.trust_device_keys(&device_id, &verifying_key, &x25519_public).unwrap();
        DeviceIdentity {
            signing_key,
            verifying_key,
            x25519_secret,
            device_id: EnvelopeDeviceId::from_bytes(device_id),
            sync_space_id: b"test-space".to_vec(),
        }
    }

    /// A per-device in-memory stand-in for the OS keychain's epoch-key
    /// storage, so `begin_genesis`/`confirm_and_import_grant`/`rotate_epoch`
    /// etc. can be exercised without ever touching the real keychain. Each
    /// simulated device gets its own instance, exactly like separate
    /// physical machines each having their own keychain.
    #[derive(Default)]
    struct FakeEpochKeyStore(std::sync::Mutex<std::collections::HashMap<u32, [u8; 32]>>);

    impl EpochKeyStore for FakeEpochKeyStore {
        fn store(&self, key_epoch: u32, key: &[u8; 32]) -> Result<(), String> {
            self.0.lock().unwrap().insert(key_epoch, *key);
            Ok(())
        }
    }

    impl FakeEpochKeyStore {
        fn get(&self, key_epoch: u32) -> Option<[u8; 32]> {
            self.0.lock().unwrap().get(&key_epoch).copied()
        }
    }

    /// Builds this device's `LocalKeys` for push/pull straight from its
    /// synthetic identity and fake epoch-key store — the test equivalent of
    /// `Database::local_replicated_keys`, which touches the real keychain
    /// and so is never called from a test.
    fn local_keys_for(database: &Database, identity: &DeviceIdentity, epoch_keys: &FakeEpochKeyStore) -> LocalKeys {
        let active_epoch: u32 = database
            .connection()
            .unwrap()
            .query_row("SELECT active_epoch FROM sync_spaces WHERE id=?1", params![SPACE_ID], |row| row.get(0))
            .unwrap();
        let k_epoch = epoch_keys.get(active_epoch).expect("epoch key must already be stored for this test");
        LocalKeys {
            signing_key: identity.signing_key.clone(),
            k_epoch,
            key_epoch: active_epoch,
            device_id: identity.device_id,
            sync_space_id: identity.sync_space_id.clone(),
        }
    }

    fn fake_transports(name: &str) -> Vec<Arc<dyn SyncTransport>> {
        let transport: Arc<dyn SyncTransport> = Arc::new(FakeTransport::new(name));
        vec![transport]
    }

    #[test]
    fn beta_toggle_defaults_off_and_persists() {
        let database = Database::open_memory();
        assert!(!database.beta_features_enabled().unwrap());
        database.set_beta_features_enabled(true).unwrap();
        assert!(database.beta_features_enabled().unwrap());
        database.set_beta_features_enabled(false).unwrap();
        assert!(!database.beta_features_enabled().unwrap());
    }

    #[tokio::test]
    async fn enrollment_status_progresses_from_not_started_to_enrolled_after_genesis() {
        let database = Database::open_memory();
        assert!(matches!(database.enrollment_status().unwrap(), EnrollmentStatus::NotStarted));

        let identity = test_identity(&database);
        let epoch_keys = FakeEpochKeyStore::default();
        let transports = fake_transports("genesis");
        let phrase = begin_genesis(&database, &identity, &epoch_keys, &transports).await.unwrap();
        assert_eq!(phrase.split_whitespace().count(), 24);

        match database.enrollment_status().unwrap() {
            EnrollmentStatus::Enrolled { device_count } => assert_eq!(device_count, 1),
            other => panic!("expected Enrolled, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn cross_device_sync_counts_as_enrolled_only_while_the_beta_is_on() {
        let database = Database::open_memory();
        let identity = test_identity(&database);
        let epoch_keys = FakeEpochKeyStore::default();
        begin_genesis(&database, &identity, &epoch_keys, &fake_transports("genesis")).await.unwrap();

        database.set_beta_features_enabled(true).unwrap();
        assert!(database.cross_device_sync_enrolled().unwrap());
        // Turning the beta off returns account removal to local-only
        // semantics even though the enrollment itself is kept.
        database.set_beta_features_enabled(false).unwrap();
        assert!(!database.cross_device_sync_enrolled().unwrap());
    }

    #[tokio::test]
    async fn two_device_peer_enrollment_lets_the_new_device_push_and_the_first_pull_it() {
        let database_a = Database::open_memory();
        let database_b = Database::open_memory();
        let identity_a = test_identity(&database_a);
        let identity_b = test_identity(&database_b);
        let epoch_keys_a = FakeEpochKeyStore::default();
        let epoch_keys_b = FakeEpochKeyStore::default();
        let transports = fake_transports("shared");

        // A creates the space.
        begin_genesis(&database_a, &identity_a, &epoch_keys_a, &transports).await.unwrap();

        // B requests to join.
        let fingerprint_b = publish_enrollment_request(&database_b, &identity_b, &transports).await.unwrap();
        assert_eq!(fingerprint_b.split('-').count(), 4);

        // A's sweep discovers the request.
        run_enrollment_sweep(&database_a, &identity_a, &epoch_keys_a, &transports).await.unwrap();
        let pending = database_a.pending_incoming_enrollment_requests().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].fingerprint, fingerprint_b);

        // A approves it, publishing a grant.
        let keys_a = local_keys_for(&database_a, &identity_a, &epoch_keys_a);
        approve_enrollment_request(&database_a, &identity_a, &keys_a, &pending[0].request_id, &transports).await.unwrap();

        // B's sweep discovers and stages the grant, matching the fingerprint
        // it already displayed when it made the request.
        run_enrollment_sweep(&database_b, &identity_b, &epoch_keys_b, &transports).await.unwrap();
        match database_b.enrollment_status().unwrap() {
            EnrollmentStatus::AwaitingConfirmation { fingerprint, approver_fingerprint, .. } => {
                assert_eq!(fingerprint, fingerprint_b);
                assert!(!approver_fingerprint.is_empty());
            }
            other => panic!("expected AwaitingConfirmation, got {other:?}"),
        }

        // B confirms (the human compared fingerprints across two screens).
        let outgoing_request_id: String = database_b
            .connection()
            .unwrap()
            .query_row("SELECT request_id FROM replicated_sync_enrollment_requests WHERE direction='outgoing'", [], |row| row.get(0))
            .unwrap();
        confirm_and_import_grant(&database_b, &identity_b, &epoch_keys_b, &outgoing_request_id).await.unwrap();
        match database_b.enrollment_status().unwrap() {
            EnrollmentStatus::Enrolled { device_count } => assert_eq!(device_count, 2),
            other => panic!("expected Enrolled, got {other:?}"),
        }

        // Prove it actually works end to end: B writes a snippet, pushes,
        // and A — who now trusts B — pulls and materializes it.
        database_b
            .record_replicated_write(EntityType::Snippet, "b-1", &fields(&["id", "name", "body", "createdAt"]), &snippet_payload("b-1", "From B"))
            .unwrap();
        let keys_b = local_keys_for(&database_b, &identity_b, &epoch_keys_b);
        push_pending_events(&database_b, &keys_b, &transports).await.unwrap();

        let keys_a_after = local_keys_for(&database_a, &identity_a, &epoch_keys_a);
        let outcome = pull_from_transports(&database_a, &keys_a_after, &transports).await.unwrap();
        assert_eq!(outcome.applied_events, 1);
        let snippet_name: String = database_a.connection().unwrap().query_row("SELECT name FROM snippets WHERE id='b-1'", [], |row| row.get(0)).unwrap();
        assert_eq!(snippet_name, "From B");
    }

    #[tokio::test]
    async fn revoking_a_device_stops_its_future_head_from_being_trusted() {
        let database_a = Database::open_memory();
        let database_b = Database::open_memory();
        let identity_a = test_identity(&database_a);
        let identity_b = test_identity(&database_b);
        let epoch_keys_a = FakeEpochKeyStore::default();
        let epoch_keys_b = FakeEpochKeyStore::default();
        let transports = fake_transports("shared");

        begin_genesis(&database_a, &identity_a, &epoch_keys_a, &transports).await.unwrap();
        publish_enrollment_request(&database_b, &identity_b, &transports).await.unwrap();
        run_enrollment_sweep(&database_a, &identity_a, &epoch_keys_a, &transports).await.unwrap();
        let pending = database_a.pending_incoming_enrollment_requests().unwrap();
        let keys_a = local_keys_for(&database_a, &identity_a, &epoch_keys_a);
        approve_enrollment_request(&database_a, &identity_a, &keys_a, &pending[0].request_id, &transports).await.unwrap();
        run_enrollment_sweep(&database_b, &identity_b, &epoch_keys_b, &transports).await.unwrap();
        let outgoing_request_id: String = database_b
            .connection()
            .unwrap()
            .query_row("SELECT request_id FROM replicated_sync_enrollment_requests WHERE direction='outgoing'", [], |row| row.get(0))
            .unwrap();
        confirm_and_import_grant(&database_b, &identity_b, &epoch_keys_b, &outgoing_request_id).await.unwrap();

        // B is enrolled and trusted by A.
        assert_eq!(database_a.known_device_roster().unwrap().len(), 2);

        // A revokes B.
        let keys_a_2 = local_keys_for(&database_a, &identity_a, &epoch_keys_a);
        rotate_epoch(&database_a, &identity_a, &keys_a_2, &epoch_keys_a, &transports, Some(&encode_id(identity_b.device_id.as_bytes())))
            .await
            .unwrap();

        // A's active roster now excludes B.
        let roster_after = database_a.known_device_roster().unwrap();
        assert_eq!(roster_after.len(), 1);
        assert!(!roster_after.iter().any(|(id, _)| *id == identity_b.device_id));

        // B is still able to decrypt everything it already had, but a
        // fresh push from B after revocation is never pulled by A: B's
        // device id is no longer in A's locator set at all.
        database_b
            .record_replicated_write(EntityType::Snippet, "after-revoke", &fields(&["id", "name", "body", "createdAt"]), &snippet_payload("after-revoke", "Should not sync"))
            .unwrap();
        let keys_b_after = local_keys_for(&database_b, &identity_b, &epoch_keys_b);
        push_pending_events(&database_b, &keys_b_after, &transports).await.unwrap();
        let keys_a_after_revoke = local_keys_for(&database_a, &identity_a, &epoch_keys_a);
        let outcome = pull_from_transports(&database_a, &keys_a_after_revoke, &transports).await.unwrap();
        assert_eq!(outcome.applied_events, 0);
        let exists: Option<String> = database_a
            .connection()
            .unwrap()
            .query_row("SELECT name FROM snippets WHERE id='after-revoke'", [], |row| row.get(0))
            .optional()
            .unwrap();
        assert_eq!(exists, None);
    }

    #[tokio::test]
    async fn recovery_phrase_bootstraps_a_third_device_and_announces_it_to_the_first() {
        let database_a = Database::open_memory();
        let database_c = Database::open_memory();
        let identity_a = test_identity(&database_a);
        let identity_c = test_identity(&database_c);
        let epoch_keys_a = FakeEpochKeyStore::default();
        let epoch_keys_c = FakeEpochKeyStore::default();
        let transports = fake_transports("shared");

        let phrase = begin_genesis(&database_a, &identity_a, &epoch_keys_a, &transports).await.unwrap();

        join_with_recovery_phrase(&database_c, &identity_c, &epoch_keys_c, &phrase, &transports).await.unwrap();
        match database_c.enrollment_status().unwrap() {
            // C now trusts both the recovered roster (A) and itself.
            EnrollmentStatus::Enrolled { device_count } => assert_eq!(device_count, 2),
            other => panic!("expected Enrolled, got {other:?}"),
        }

        // A's sweep discovers C's recovery-signed self-announcement and
        // trusts it without any staged confirmation step.
        run_enrollment_sweep(&database_a, &identity_a, &epoch_keys_a, &transports).await.unwrap();
        let roster = database_a.known_device_roster().unwrap();
        assert_eq!(roster.len(), 2);
        assert!(roster.iter().any(|(id, _)| *id == identity_c.device_id));
    }

    #[test]
    fn wrong_recovery_phrase_never_opens_the_genesis_rotation() {
        let seed_a = generate_recovery_seed();
        let seed_b = generate_recovery_seed();
        let secret_a = threestrands_sync_envelope::recovery_x25519_secret(&seed_a);
        let public_a = X25519PublicKey::from(&secret_a).to_bytes();
        let sealed = seal_to_x25519(&public_a, b"epoch key");
        assert!(try_open_sealed_box(&seed_b, &sealed).is_none());
        assert_eq!(try_open_sealed_box(&seed_a, &sealed), None); // seed bytes alone are not the derived secret
    }

    fn fields(names: &[&str]) -> std::collections::BTreeSet<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn snippet_payload(id: &str, name: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "name": name,
            "body": "body",
            "createdAt": "2026-01-01T00:00:00Z",
        })
    }
}
