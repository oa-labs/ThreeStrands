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

use std::collections::BTreeSet;
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
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "state")]
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
    pub device_id: Option<String>,
    pub fingerprint: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRosterEntry {
    pub device_id: String,
    pub status: String,
    pub is_self: bool,
    /// The resolved shared device name, cached in `sync_device_labels`.
    pub label: Option<String>,
    /// When this device last recorded (for itself) or received (for a
    /// peer) a change from that device. Not a liveness signal: an idle but
    /// online device keeps its last value.
    pub last_change_at: Option<String>,
}

/// Longest local device label accepted, in characters.
pub const MAX_DEVICE_LABEL_CHARS: usize = 60;

fn default_device_name() -> String {
    let hostname = gethostname::gethostname().to_string_lossy().trim().to_string();
    let hostname = hostname.chars().take(MAX_DEVICE_LABEL_CHARS).collect::<String>();
    if hostname.is_empty() { "This device".to_string() } else { hostname }
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
                "SELECT request_id, device_id, fingerprint, created_at FROM replicated_sync_enrollment_requests
                 WHERE direction='incoming' AND status='pending' ORDER BY created_at ASC",
            )
            .map_err(display)?;
        let rows = statement
            .query_map([], |row| {
                Ok(IncomingEnrollmentRequest {
                    request_id: row.get(0)?,
                    device_id: row.get(1)?,
                    fingerprint: row.get(2)?,
                    created_at: row.get(3)?,
                })
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

    /// This device first, then active peers, then revoked ones.
    pub fn device_roster(&self) -> Result<Vec<DeviceRosterEntry>, String> {
        let self_device_id: Option<String> = self.connection()?.query_row(
            "SELECT device_id FROM sync_devices WHERE is_self=1 LIMIT 1",
            [],
            |row| row.get(0),
        ).optional().map_err(display)?;
        if let Some(device_id) = self_device_id {
            self.ensure_self_device_name(&device_id)?;
        }
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT d.device_id, d.status, d.is_self, l.label,
                        (SELECT MAX(e.created_at) FROM sync_events e WHERE e.device_id = d.device_id)
                 FROM sync_devices d LEFT JOIN sync_device_labels l ON l.device_id = d.device_id
                 ORDER BY d.is_self DESC, d.status = 'revoked', d.device_id",
            )
            .map_err(display)?;
        let rows = statement
            .query_map([], |row| {
                Ok(DeviceRosterEntry {
                    device_id: row.get(0)?,
                    status: row.get(1)?,
                    is_self: row.get(2)?,
                    label: row.get(3)?,
                    last_change_at: row.get(4)?,
                })
            })
            .map_err(display)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(display)?;
        Ok(rows)
    }

    /// Gives this device its hostname the first time it appears in the roster.
    /// The sync loop records it once the portable preference entity exists.
    pub(crate) fn ensure_self_device_name(&self, device_id_hex: &str) -> Result<(), String> {
        self.connection()?
            .execute(
                "INSERT OR IGNORE INTO sync_device_labels(device_id,label) SELECT ?1,?2
                 WHERE EXISTS (SELECT 1 FROM sync_devices WHERE device_id=?1 AND is_self=1)",
                params![device_id_hex, default_device_name()],
            )
            .map_err(display)?;
        Ok(())
    }

    pub(crate) fn record_self_device_name_if_missing(&self, device_id_hex: &str) -> Result<(), String> {
        let field = format!("deviceName:{device_id_hex}");
        let (label, preferences_exist, already_recorded): (Option<String>, bool, bool) = self.connection()?.query_row(
            "SELECT l.label,
                    EXISTS(SELECT 1 FROM sync_operations WHERE entity_type='preferences' AND entity_id='portable'),
                    EXISTS(SELECT 1 FROM sync_operations WHERE entity_type='preferences' AND entity_id='portable' AND field=?1)
             FROM sync_device_labels l WHERE l.device_id=?2",
            params![field, device_id_hex],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).optional().map_err(display)?.unwrap_or((None, false, false));
        if preferences_exist && !already_recorded {
            if let Some(label) = label {
                let fields = BTreeSet::from([field.clone()]);
                let payload = serde_json::json!({ (field): label });
                self.record_local_entity_write(threestrands_sync_protocol::EntityType::Preferences, "portable", payload, Some(fields))?;
            }
        }
        Ok(())
    }

    /// Applies a resolved shared device-name field to this device's roster cache.
    pub(crate) fn materialize_device_name(&self, device_id_hex: &str, name: Option<&str>) -> Result<(), String> {
        let connection = self.connection()?;
        match name {
            Some(name) => {
                connection.execute(
                    "INSERT INTO sync_device_labels(device_id,label) VALUES (?1,?2)
                     ON CONFLICT(device_id) DO UPDATE SET label=excluded.label",
                    params![device_id_hex, name],
                ).map_err(display)?;
            }
            None => {
                connection.execute("DELETE FROM sync_device_labels WHERE device_id=?1", params![device_id_hex]).map_err(display)?;
            }
        }
        Ok(())
    }

    /// Sets a shared roster name. A blank self-name restores this machine's
    /// hostname; a blank peer name removes its name from the shared roster.
    pub fn set_device_label(&self, device_id_hex: &str, label: &str) -> Result<(), String> {
        let label = if label.trim().is_empty() {
            let is_self: bool = self.connection()?.query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_devices WHERE device_id=?1 AND is_self=1)",
                params![device_id_hex],
                |row| row.get(0),
            ).map_err(display)?;
            if is_self { default_device_name() } else { String::new() }
        } else {
            label.trim().to_string()
        };
        if label.chars().count() > MAX_DEVICE_LABEL_CHARS {
            return Err(format!("Device names can be at most {MAX_DEVICE_LABEL_CHARS} characters."));
        }
        let connection = self.connection()?;
        if label.is_empty() {
            connection.execute("DELETE FROM sync_device_labels WHERE device_id=?1", params![device_id_hex]).map_err(display)?;
        } else {
            connection
                .execute(
                    "INSERT INTO sync_device_labels(device_id,label) VALUES (?1,?2)
                     ON CONFLICT(device_id) DO UPDATE SET label=excluded.label",
                    params![device_id_hex, label],
                )
                .map_err(display)?;
        }
        drop(connection);

        let field = format!("deviceName:{device_id_hex}");
        let fields = BTreeSet::from([field.clone()]);
        let payload = serde_json::json!({ (field): if label.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(label) } });
        let preferences_exist: bool = self.connection()?.query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_operations WHERE entity_type='preferences' AND entity_id='portable')",
            [],
            |row| row.get(0),
        ).map_err(display)?;
        if preferences_exist {
            self.record_local_entity_write(threestrands_sync_protocol::EntityType::Preferences, "portable", payload, Some(fields))?;
        }
        Ok(())
    }

    /// Forgets this device's membership in the sync space, locally only:
    /// the replication log, roster, enrollment state, and labels go, while
    /// materialized data (tasks, snippets, accounts, preferences), the
    /// configured transports, and the beta toggle stay. The self row goes
    /// too, so the next identity load provisions a fresh device id.
    ///
    /// Returns the highest epoch this device may hold a key for, so the
    /// caller can remove `0..=` that range from the keychain afterwards —
    /// after, so a keychain failure never leaves the database claiming an
    /// enrollment whose keys are already gone.
    pub fn leave_sync_space(&self) -> Result<u32, String> {
        let mut connection = self.connection()?;
        let tx = connection.transaction().map_err(display)?;
        let highest_epoch: u32 = tx
            .query_row(
                "SELECT MAX(COALESCE((SELECT MAX(key_epoch) FROM sync_epoch_history), 0),
                            COALESCE((SELECT active_epoch FROM sync_spaces WHERE id=?1), 0))",
                params![SPACE_ID],
                |row| row.get(0),
            )
            .map_err(display)?;
        tx.execute_batch(
            "DELETE FROM sync_deliveries;
             DELETE FROM sync_field_frontier;
             DELETE FROM sync_operation_parents;
             DELETE FROM sync_operations;
             DELETE FROM sync_objects;
             DELETE FROM sync_events;
             DELETE FROM sync_epoch_history;
             DELETE FROM replicated_sync_enrollment_requests;
             DELETE FROM sync_control_objects_seen;
             DELETE FROM sync_device_labels;
             DELETE FROM sync_devices;",
        )
        .map_err(display)?;
        tx.execute(
            "UPDATE sync_spaces SET active_epoch=0, lamport=0, recovery_public_key=NULL, recovery_x25519_public=NULL, last_error=NULL WHERE id=?1",
            params![SPACE_ID],
        )
        .map_err(display)?;
        tx.commit().map_err(display)?;
        Ok(highest_epoch)
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
/// Whether the configured transports already hold a sync space, as far as
/// a device that has not enrolled yet can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncSpacePresence {
    /// A self-consistently signed key-rotation object exists on at least
    /// one transport, so another device already started a space here.
    Existing,
    /// Every transport was scanned completely and none holds a rotation.
    None,
    /// No transport is configured, or at least one could not be scanned
    /// completely, and no rotation was found on the ones that could.
    Unknown,
}

pub const EXISTING_SPACE_REFUSAL: &str =
    "A sync space already exists in this location. Join it from another device or with its recovery phrase, or confirm that you want a separate new space.";

/// A rotation counts as evidence of a space only when it verifies against
/// its own initiator's roster entry — the same self-consistency check a
/// grant gets. This never establishes trust in the space; it only keeps
/// arbitrary bytes that happen to decode from steering the setup choice.
fn is_self_consistent_rotation(bytes: &[u8]) -> bool {
    let Ok(signed) = decode_signed_key_rotation(bytes) else { return false };
    let Some(initiator) = signed.rotation.roster.iter().find(|entry| entry.device_id == signed.rotation.initiator_device_id) else {
        return false;
    };
    let Ok(verifying_key) = roster_entry_verifying_key(initiator) else { return false };
    verify_key_rotation(&verifying_key, &signed).is_ok()
}

/// Read-only scan for an existing sync space, stopping at the first one
/// found. Used to steer a not-yet-enrolled device toward joining instead
/// of starting a second, disconnected space in the same location.
pub async fn inspect_sync_space(transports: &[Arc<dyn SyncTransport>]) -> SyncSpacePresence {
    let mut incomplete = transports.is_empty();
    for transport in transports {
        let mut cursor: Option<String> = None;
        loop {
            let page = match transport.scan(cursor.as_deref()).await {
                Ok(Some(page)) => page,
                Ok(None) => break,
                Err(_) => {
                    incomplete = true;
                    break;
                }
            };
            for locator in &page.objects {
                match transport.get_object(&locator.cid).await {
                    Ok(bytes) if is_self_consistent_rotation(&bytes) => return SyncSpacePresence::Existing,
                    Ok(_) => {}
                    Err(_) => incomplete = true,
                }
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
    }
    if incomplete { SyncSpacePresence::Unknown } else { SyncSpacePresence::None }
}

/// Starts a new sync space. Refuses, before any side effect, when a space
/// already exists on a configured transport unless `allow_existing_space`
/// records the user's explicit choice to start a separate one anyway.
pub async fn begin_genesis(
    database: &Database,
    identity: &DeviceIdentity,
    epoch_keys: &dyn EpochKeyStore,
    transports: &[Arc<dyn SyncTransport>],
    allow_existing_space: bool,
) -> Result<String, String> {
    if !allow_existing_space && inspect_sync_space(transports).await == SyncSpacePresence::Existing {
        return Err(EXISTING_SPACE_REFUSAL.to_string());
    }
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
    fn enrollment_status_serializes_all_variant_fields_in_camel_case() {
        let cases = [
            (EnrollmentStatus::NotStarted, serde_json::json!({ "state": "notStarted" })),
            (
                EnrollmentStatus::AwaitingGrant {
                    request_id: "request-1".to_string(),
                    fingerprint: "AAAA-BBBB".to_string(),
                    created_at: "2026-09-22T19:40:01Z".to_string(),
                },
                serde_json::json!({
                    "state": "awaitingGrant",
                    "requestId": "request-1",
                    "fingerprint": "AAAA-BBBB",
                    "createdAt": "2026-09-22T19:40:01Z",
                }),
            ),
            (
                EnrollmentStatus::AwaitingConfirmation {
                    request_id: "request-2".to_string(),
                    fingerprint: "CCCC-DDDD".to_string(),
                    approver_fingerprint: "EEEE-FFFF".to_string(),
                },
                serde_json::json!({
                    "state": "awaitingConfirmation",
                    "requestId": "request-2",
                    "fingerprint": "CCCC-DDDD",
                    "approverFingerprint": "EEEE-FFFF",
                }),
            ),
            (EnrollmentStatus::Enrolled { device_count: 2 }, serde_json::json!({ "state": "enrolled", "deviceCount": 2 })),
        ];

        for (status, expected) in cases {
            assert_eq!(serde_json::to_value(status).unwrap(), expected);
        }
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
        let phrase = begin_genesis(&database, &identity, &epoch_keys, &transports, false).await.unwrap();
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
        begin_genesis(&database, &identity, &epoch_keys, &fake_transports("genesis"), false).await.unwrap();

        database.set_beta_features_enabled(true).unwrap();
        assert!(database.cross_device_sync_enrolled().unwrap());
        // Turning the beta off returns account removal to local-only
        // semantics even though the enrollment itself is kept.
        database.set_beta_features_enabled(false).unwrap();
        assert!(!database.cross_device_sync_enrolled().unwrap());
    }

    #[tokio::test]
    async fn two_device_peer_enrollment_lets_the_new_device_push_and_the_first_pull_it() {
        assert_peer_enrollment_round_trips(fake_transports("shared")).await;
    }

    /// Two S3 connector instances (one per device) pointed at the same
    /// bucket, over a real SigV4-verifying fake S3 server.
    async fn shared_s3_transports(server: &crate::s3_transport::fake_server::FakeS3Server) -> (Vec<Arc<dyn SyncTransport>>, Vec<Arc<dyn SyncTransport>>) {
        use crate::s3_transport::{fake_server::FakeS3Server, S3Transport};
        let open = |id: &str| -> Vec<Arc<dyn SyncTransport>> {
            vec![Arc::new(S3Transport::new(id, &server.config("group"), &FakeS3Server::credentials()).unwrap())]
        };
        (open("s3-device-a"), open("s3-device-b"))
    }

    #[tokio::test]
    async fn peer_enrollment_round_trips_over_an_s3_connector() {
        let server = crate::s3_transport::fake_server::FakeS3Server::spawn().await;
        let (transports, _) = shared_s3_transports(&server).await;
        assert_peer_enrollment_round_trips(transports).await;
        assert!(!server.state().objects.is_empty());
    }

    #[tokio::test]
    async fn recovery_phrase_join_round_trips_over_s3_connectors() {
        let server = crate::s3_transport::fake_server::FakeS3Server::spawn().await;
        let (transports_a, transports_c) = shared_s3_transports(&server).await;
        let database_a = Database::open_memory();
        let database_c = Database::open_memory();
        let identity_a = test_identity(&database_a);
        let identity_c = test_identity(&database_c);
        let epoch_keys_a = FakeEpochKeyStore::default();
        let epoch_keys_c = FakeEpochKeyStore::default();

        let phrase = begin_genesis(&database_a, &identity_a, &epoch_keys_a, &transports_a, false).await.unwrap();
        assert_eq!(inspect_sync_space(&transports_c).await, SyncSpacePresence::Existing);
        join_with_recovery_phrase(&database_c, &identity_c, &epoch_keys_c, &phrase, &transports_c).await.unwrap();
        run_enrollment_sweep(&database_a, &identity_a, &epoch_keys_a, &transports_a).await.unwrap();
        assert!(database_a.known_device_roster().unwrap().iter().any(|(id, _)| *id == identity_c.device_id));

        database_c
            .record_replicated_write(EntityType::Snippet, "c-1", &fields(&["id", "name", "body", "createdAt"]), &snippet_payload("c-1", "From C over S3"))
            .unwrap();
        let keys_c = local_keys_for(&database_c, &identity_c, &epoch_keys_c);
        push_pending_events(&database_c, &keys_c, &transports_c).await.unwrap();
        let keys_a = local_keys_for(&database_a, &identity_a, &epoch_keys_a);
        let outcome = pull_from_transports(&database_a, &keys_a, &transports_a).await.unwrap();
        assert_eq!(outcome.applied_events, 1);
        let name: String = database_a.connection().unwrap().query_row("SELECT name FROM snippets WHERE id='c-1'", [], |row| row.get(0)).unwrap();
        assert_eq!(name, "From C over S3");
    }

    #[tokio::test]
    async fn an_s3_connection_test_finds_a_group_another_device_created() {
        use crate::s3_transport::fake_server::FakeS3Server;
        let server = FakeS3Server::spawn().await;
        let (transports_a, _) = shared_s3_transports(&server).await;
        let database_a = Database::open_memory();
        let identity_a = test_identity(&database_a);
        begin_genesis(&database_a, &identity_a, &FakeEpochKeyStore::default(), &transports_a, false).await.unwrap();

        let engine = crate::replicated_sync::ReplicatedSync::new(Arc::new(Database::open_memory()));
        let found = engine.probe_s3(&server.config("group"), &FakeS3Server::credentials()).await.unwrap();
        assert_eq!(found.space_presence, Some(SyncSpacePresence::Existing));
        let elsewhere = engine.probe_s3(&server.config("other-prefix"), &FakeS3Server::credentials()).await.unwrap();
        assert_eq!(elsewhere.space_presence, Some(SyncSpacePresence::None));
    }

    async fn assert_peer_enrollment_round_trips(transports: Vec<Arc<dyn SyncTransport>>) {
        let database_a = Database::open_memory();
        let database_b = Database::open_memory();
        let identity_a = test_identity(&database_a);
        let identity_b = test_identity(&database_b);
        let epoch_keys_a = FakeEpochKeyStore::default();
        let epoch_keys_b = FakeEpochKeyStore::default();

        // A creates the space.
        begin_genesis(&database_a, &identity_a, &epoch_keys_a, &transports, false).await.unwrap();

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

        begin_genesis(&database_a, &identity_a, &epoch_keys_a, &transports, false).await.unwrap();
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

        let phrase = begin_genesis(&database_a, &identity_a, &epoch_keys_a, &transports, false).await.unwrap();

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

    #[tokio::test]
    async fn inspecting_reports_no_space_until_genesis_publishes_one() {
        let database = Database::open_memory();
        let identity = test_identity(&database);
        let epoch_keys = FakeEpochKeyStore::default();
        let transports = fake_transports("shared");

        assert_eq!(inspect_sync_space(&[]).await, SyncSpacePresence::Unknown);
        assert_eq!(inspect_sync_space(&transports).await, SyncSpacePresence::None);

        begin_genesis(&database, &identity, &epoch_keys, &transports, false).await.unwrap();
        assert_eq!(inspect_sync_space(&transports).await, SyncSpacePresence::Existing);
    }

    #[tokio::test]
    async fn a_pending_join_request_or_unrelated_object_is_not_a_space() {
        let database = Database::open_memory();
        let identity = test_identity(&database);
        let transports = fake_transports("shared");

        publish_enrollment_request(&database, &identity, &transports).await.unwrap();
        publish_to_all(&transports, b"not a control object").await;
        assert_eq!(inspect_sync_space(&transports).await, SyncSpacePresence::None);
    }

    #[tokio::test]
    async fn a_rotation_that_fails_its_initiator_signature_is_not_a_space() {
        let database = Database::open_memory();
        let identity = test_identity(&database);
        let epoch_keys = FakeEpochKeyStore::default();
        let genesis_transports = fake_transports("genesis");
        begin_genesis(&database, &identity, &epoch_keys, &genesis_transports, false).await.unwrap();

        let page = genesis_transports[0].scan(None).await.unwrap().unwrap();
        let bytes = genesis_transports[0].get_object(&page.objects[0].cid).await.unwrap();
        let mut tampered = decode_signed_key_rotation(&bytes).unwrap();
        tampered.rotation.created_at_ms += 1;

        let transports = fake_transports("tampered");
        publish_to_all(&transports, &encode_signed_key_rotation(&tampered).unwrap()).await;
        assert_eq!(inspect_sync_space(&transports).await, SyncSpacePresence::None);
    }

    #[tokio::test]
    async fn an_incomplete_scan_is_unknown_rather_than_empty() {
        let fake = Arc::new(FakeTransport::new("flaky"));
        fake.inject_transient_outage(1);
        let transports: Vec<Arc<dyn SyncTransport>> = vec![fake];
        assert_eq!(inspect_sync_space(&transports).await, SyncSpacePresence::Unknown);
    }

    #[tokio::test]
    async fn genesis_refuses_an_existing_space_without_side_effects_unless_allowed() {
        let database_a = Database::open_memory();
        let database_b = Database::open_memory();
        let identity_a = test_identity(&database_a);
        let identity_b = test_identity(&database_b);
        let epoch_keys_a = FakeEpochKeyStore::default();
        let epoch_keys_b = FakeEpochKeyStore::default();
        let transports = fake_transports("shared");

        begin_genesis(&database_a, &identity_a, &epoch_keys_a, &transports, false).await.unwrap();

        database_b.set_beta_features_enabled(false).unwrap();
        let refused = begin_genesis(&database_b, &identity_b, &epoch_keys_b, &transports, false).await;
        assert_eq!(refused, Err(EXISTING_SPACE_REFUSAL.to_string()));
        assert!(matches!(database_b.enrollment_status().unwrap(), EnrollmentStatus::NotStarted));
        assert!(!database_b.beta_features_enabled().unwrap());
        assert_eq!(epoch_keys_b.get(0), None);

        let phrase = begin_genesis(&database_b, &identity_b, &epoch_keys_b, &transports, true).await.unwrap();
        assert_eq!(phrase.split_whitespace().count(), 24);
        assert!(matches!(database_b.enrollment_status().unwrap(), EnrollmentStatus::Enrolled { device_count: 1 }));
    }

    /// A and B enrolled into one space through peer approval.
    async fn two_enrolled_devices() -> (Database, Database, DeviceIdentity, DeviceIdentity, Vec<Arc<dyn SyncTransport>>, String) {
        let database_a = Database::open_memory();
        let database_b = Database::open_memory();
        let identity_a = test_identity(&database_a);
        let identity_b = test_identity(&database_b);
        let epoch_keys_a = FakeEpochKeyStore::default();
        let epoch_keys_b = FakeEpochKeyStore::default();
        let transports = fake_transports("shared");
        let phrase = begin_genesis(&database_a, &identity_a, &epoch_keys_a, &transports, false).await.unwrap();
        publish_enrollment_request(&database_b, &identity_b, &transports).await.unwrap();
        run_enrollment_sweep(&database_a, &identity_a, &epoch_keys_a, &transports).await.unwrap();
        let pending = database_a.pending_incoming_enrollment_requests().unwrap();
        assert_eq!(pending[0].device_id.as_deref(), Some(encode_id(identity_b.device_id.as_bytes()).as_str()));
        let keys_a = local_keys_for(&database_a, &identity_a, &epoch_keys_a);
        approve_enrollment_request(&database_a, &identity_a, &keys_a, &pending[0].request_id, &transports).await.unwrap();
        run_enrollment_sweep(&database_b, &identity_b, &epoch_keys_b, &transports).await.unwrap();
        let outgoing: String = database_b
            .connection()
            .unwrap()
            .query_row("SELECT request_id FROM replicated_sync_enrollment_requests WHERE direction='outgoing'", [], |row| row.get(0))
            .unwrap();
        confirm_and_import_grant(&database_b, &identity_b, &epoch_keys_b, &outgoing).await.unwrap();
        (database_a, database_b, identity_a, identity_b, transports, phrase)
    }

    #[tokio::test]
    async fn the_roster_marks_this_device_by_its_flag_and_lists_it_first() {
        let (database_a, database_b, identity_a, identity_b, _, _) = two_enrolled_devices().await;
        for (database, identity) in [(&database_a, &identity_a), (&database_b, &identity_b)] {
            let roster = database.device_roster().unwrap();
            assert_eq!(roster.len(), 2);
            assert!(roster[0].is_self);
            assert_eq!(roster[0].device_id, encode_id(identity.device_id.as_bytes()));
            assert!(!roster[1].is_self);
        }
    }

    #[tokio::test]
    async fn the_roster_reports_shared_labels_and_each_devices_latest_change() {
        let (database_a, _, identity_a, identity_b, _, _) = two_enrolled_devices().await;
        let peer = encode_id(identity_b.device_id.as_bytes());
        let own = encode_id(identity_a.device_id.as_bytes());
        database_a.set_device_label(&peer, "  Work laptop  ").unwrap();
        database_a
            .connection()
            .unwrap()
            .execute_batch(&format!(
                "INSERT INTO sync_events(event_id,epoch,device_id,device_sequence,lamport,state,created_at) VALUES
                   ('e1',0,'{peer}',1,1,'sealed','2026-09-20T10:00:00+00:00'),
                   ('e2',0,'{peer}',2,2,'sealed','2026-09-21T10:00:00+00:00');"
            ))
            .unwrap();

        let roster = database_a.device_roster().unwrap();
        let peer_entry = roster.iter().find(|entry| entry.device_id == peer).unwrap();
        assert_eq!(peer_entry.label.as_deref(), Some("Work laptop"));
        assert_eq!(peer_entry.last_change_at.as_deref(), Some("2026-09-21T10:00:00+00:00"));
        let own_entry = roster.iter().find(|entry| entry.device_id == own).unwrap();
        assert!(own_entry.label.as_deref().is_some_and(|label| !label.is_empty()));
        assert_eq!(own_entry.last_change_at, None);

        database_a.set_device_label(&peer, "   ").unwrap();
        assert_eq!(database_a.device_roster().unwrap().iter().find(|entry| entry.device_id == peer).unwrap().label, None);
    }

    #[test]
    fn device_labels_accept_up_to_the_limit_and_reject_beyond_it() {
        let database = Database::open_memory();
        let at_limit = "x".repeat(MAX_DEVICE_LABEL_CHARS);
        database.set_device_label("d1", &"x".repeat(MAX_DEVICE_LABEL_CHARS - 1)).unwrap();
        database.set_device_label("d1", &at_limit).unwrap();
        assert!(database.set_device_label("d1", &"x".repeat(MAX_DEVICE_LABEL_CHARS + 1)).is_err());
        let stored: String = database.connection().unwrap().query_row("SELECT label FROM sync_device_labels", [], |row| row.get(0)).unwrap();
        assert_eq!(stored, at_limit);
    }

    #[tokio::test]
    async fn leaving_resets_membership_but_keeps_local_data_locations_and_the_beta() {
        let (_, database_b, _, identity_b, transports, phrase) = two_enrolled_devices().await;
        database_b
            .connection()
            .unwrap()
            .execute_batch(
                "INSERT INTO snippets(id,name,body,created_at) VALUES ('s1','Kept','body','2026-09-22T00:00:00Z');
                 INSERT INTO sync_transports(instance_id,kind,enabled) VALUES ('folder-1','folder',1);",
            )
            .unwrap();
        database_b.set_device_label(&encode_id(identity_b.device_id.as_bytes()), "Old name").unwrap();

        assert_eq!(database_b.leave_sync_space().unwrap(), 0);

        assert!(matches!(database_b.enrollment_status().unwrap(), EnrollmentStatus::NotStarted));
        assert!(database_b.device_roster().unwrap().is_empty());
        assert!(database_b.recovery_public_keys().unwrap().is_none());
        assert!(database_b.beta_features_enabled().unwrap());
        let count = |sql: &str| -> i64 { database_b.connection().unwrap().query_row(sql, [], |row| row.get(0)).unwrap() };
        assert_eq!(count("SELECT COUNT(*) FROM sync_events"), 0);
        assert_eq!(count("SELECT COUNT(*) FROM sync_epoch_history"), 0);
        assert_eq!(count("SELECT COUNT(*) FROM replicated_sync_enrollment_requests"), 0);
        assert_eq!(count("SELECT COUNT(*) FROM sync_device_labels"), 0);
        assert_eq!(count("SELECT COUNT(*) FROM snippets WHERE id='s1'"), 1);
        assert_eq!(count("SELECT COUNT(*) FROM sync_transports WHERE instance_id='folder-1'"), 1);

        // A fresh identity rejoins the same space cleanly.
        let rejoined = test_identity(&database_b);
        assert_ne!(rejoined.device_id, identity_b.device_id);
        let epoch_keys = FakeEpochKeyStore::default();
        join_with_recovery_phrase(&database_b, &rejoined, &epoch_keys, &phrase, &transports).await.unwrap();
        assert!(matches!(database_b.enrollment_status().unwrap(), EnrollmentStatus::Enrolled { .. }));
    }

    #[tokio::test]
    async fn leaving_reports_the_highest_epoch_to_forget() {
        let (database_a, ..) = two_enrolled_devices().await;
        database_a.record_epoch_activation(1, "cid-1").unwrap();
        database_a.record_epoch_activation(2, "cid-2").unwrap();
        database_a.set_active_epoch(1).unwrap();
        assert_eq!(database_a.leave_sync_space().unwrap(), 2);
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
