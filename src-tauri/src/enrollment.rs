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
    compute_cid, decode_signed_enrollment_grant, decode_signed_enrollment_rejection, decode_signed_enrollment_request,
    decode_signed_key_rotation, encode_signed_enrollment_grant, encode_signed_enrollment_rejection, encode_signed_enrollment_request,
    encode_signed_key_rotation, enrollment_fingerprint, generate_recovery_seed,
    recovery_ed25519_signing_key, recovery_phrase_from_seed, recovery_seed_from_phrase,
    recovery_x25519_secret, seal_to_x25519, sign_enrollment_grant, sign_enrollment_rejection, sign_enrollment_request,
    sign_key_rotation, try_open_sealed_box, verify_enrollment_grant, verify_enrollment_rejection, verify_enrollment_request,
    verify_key_rotation, DeviceId as EnvelopeDeviceId, EnrollmentGrant, EnrollmentRequest,
    EnrollmentRejection, KeyRotation, RequestId, RosterEntry, SealedEpochKey, SignedEnrollmentGrant, SignedEnrollmentRequest,
    SignedKeyRotation, VerifyingKey, X25519PublicKey,
};
use threestrands_sync_envelope::limits::MAX_EARLIER_EPOCH_KEYS;
use threestrands_sync_envelope::{protocol_marker_cid, PROTOCOL_MARKER};
use threestrands_sync_transport::{Cid as TransportCid, SyncTransport, TransportError};

use crate::db::Database;
use crate::error_text::display;

mod join_codes;
pub(crate) use join_codes::{
    cancel_join_code, create_join_code, join_with_code, preview_join_code, process_join_codes, JoinCodeConnectorChoice,
    JoinCodeNotice, JoinCodePreview, JoinCredentialsChoice, JoinFolderChoice, OutstandingJoinCode,
};
#[cfg(test)]
use join_codes::{
    find_invitation, verify_fetched_invitation, CREDENTIALS_REJECTED, INVITATION_NOT_FOUND, MAX_JOIN_CODE_HOURS, MIN_JOIN_CODE_HOURS,
    STORAGE_UNREACHABLE,
};
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
    Rejected { request_id: String, fingerprint: String },
    Enrolled {
        device_count: usize,
        /// The inviter's name while this device, having joined with a join
        /// code, waits for the inviter to finish admitting it.
        awaiting_admission_from: Option<String>,
    },
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
    /// Whether this device joined the group with a join code.
    pub joined_with_join_code: bool,
}

/// Longest local device label accepted, in characters.
pub const MAX_DEVICE_LABEL_CHARS: usize = 60;

/// Seals every earlier epoch key this device holds to `recipient_x25519`,
/// for a grant or invitation, so the new device can open history sealed
/// before the current epoch.
pub(crate) fn seal_earlier_epoch_keys(keys: &LocalKeys, recipient_x25519: &[u8; 32]) -> Result<Vec<SealedEpochKey>, String> {
    if keys.earlier_epoch_keys.len() > MAX_EARLIER_EPOCH_KEYS {
        return Err(format!(
            "This sync group has changed its keys more than {MAX_EARLIER_EPOCH_KEYS} times, so it can't hand its history to a new device. Create a new sync group instead."
        ));
    }
    Ok(keys
        .earlier_epoch_keys
        .iter()
        .map(|(key_epoch, key)| SealedEpochKey {
            key_epoch: *key_epoch,
            sealed_key: ByteBuf::from(seal_to_x25519(recipient_x25519, key)),
        })
        .collect())
}

/// Opens every earlier epoch key in a grant or invitation with
/// `recipient_secret`. `None` if any entry fails to open: a partial
/// history is never adopted silently.
pub(crate) fn open_earlier_epoch_keys(recipient_secret: &[u8; 32], entries: &[SealedEpochKey]) -> Option<Vec<(u32, [u8; 32])>> {
    entries
        .iter()
        .map(|entry| {
            let key: [u8; 32] = try_open_sealed_box(recipient_secret, &entry.sealed_key)?.try_into().ok()?;
            Some((entry.key_epoch, key))
        })
        .collect()
}

/// Stores opened earlier epoch keys and records each epoch as known, so
/// `Database::local_replicated_keys` loads them for opening old history.
pub(crate) fn store_earlier_epoch_keys(
    database: &Database,
    epoch_keys: &dyn EpochKeyStore,
    earlier: &[(u32, [u8; 32])],
    source_cid: &str,
) -> Result<(), String> {
    for (key_epoch, key) in earlier {
        epoch_keys.store(*key_epoch, key)?;
        database.record_epoch_activation(*key_epoch, source_cid)?;
    }
    Ok(())
}

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
            drop(connection);
            return Ok(EnrollmentStatus::Enrolled {
                device_count: device_count as usize,
                awaiting_admission_from: self.awaiting_admission_from()?,
            });
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
        let rejected: Option<(String, String)> = connection
            .query_row(
                "SELECT request_id, fingerprint FROM replicated_sync_enrollment_requests
                 WHERE direction='outgoing' AND status='rejected' ORDER BY created_at DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(display)?;
        match pending {
            Some((request_id, fingerprint, created_at)) => Ok(EnrollmentStatus::AwaitingGrant { request_id, fingerprint, created_at }),
            None => match rejected {
                Some((request_id, fingerprint)) => Ok(EnrollmentStatus::Rejected { request_id, fingerprint }),
                None => Ok(EnrollmentStatus::NotStarted),
            },
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
                        CASE WHEN d.is_self = 1 THEN (SELECT last_change_at FROM sync_local_state WHERE id = 1)
                             ELSE (SELECT r.merged_at FROM sync_remote_states r WHERE r.device_id = d.device_id) END,
                        EXISTS(SELECT 1 FROM replicated_sync_invitation_redemptions r
                               WHERE r.device_id = d.device_id AND r.state IN ('admitted','observed'))
                        OR (d.is_self = 1 AND EXISTS(SELECT 1 FROM replicated_sync_invitations i WHERE i.direction='incoming'))
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
                    joined_with_join_code: row.get(5)?,
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
                    EXISTS(SELECT 1 FROM sync_values WHERE entity_type='preferences' AND entity_id='portable'),
                    EXISTS(SELECT 1 FROM sync_values WHERE entity_type='preferences' AND entity_id='portable' AND field=?1)
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
            "SELECT EXISTS(SELECT 1 FROM sync_values WHERE entity_type='preferences' AND entity_id='portable')",
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
             DELETE FROM sync_values;
             DELETE FROM sync_context;
             DELETE FROM sync_objects;
             DELETE FROM sync_epoch_history;
             DELETE FROM replicated_sync_enrollment_requests;
             DELETE FROM replicated_sync_invitation_redemptions;
             DELETE FROM replicated_sync_invitations;
             DELETE FROM sync_control_objects_seen;
             DELETE FROM sync_device_labels;
             DELETE FROM sync_devices;
             DELETE FROM sync_remote_states;
             DELETE FROM sync_local_state;
             DELETE FROM sync_retired_objects;
             DELETE FROM sync_head_publications;",
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

    fn advance_active_epoch(&self, key_epoch: u32) -> Result<(), String> {
        self.connection()?
            .execute(
                "UPDATE sync_spaces SET active_epoch=?2 WHERE id=?1 AND active_epoch < ?2",
                params![SPACE_ID, key_epoch],
            )
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

/// Publishes a control object (a rotation, grant, request, rejection, or
/// announcement) to every transport now, and records it as a local object
/// so anti-entropy repair keeps delivering it: to a transport that failed
/// just now, and to any connector added later. A control object that never
/// reaches storage would otherwise be lost silently — a rotation lost that
/// way leaves every other device without the new epoch's key.
async fn publish_control_object(database: &Database, transports: &[Arc<dyn SyncTransport>], bytes: &[u8]) -> Result<String, String> {
    let cid = compute_cid(bytes);
    database
        .connection()?
        .execute(
            "INSERT OR IGNORE INTO sync_objects(cid,object_kind,chunk_index,chunk_count,bytes) VALUES (?1,'control',0,1,?2)",
            params![cid, bytes],
        )
        .map_err(display)?;
    publish_to_all(transports, bytes).await;
    Ok(cid)
}

/// Puts `bytes` on every transport, best effort, without recording it for
/// repair. Tests use it to plant objects no device would publish.
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
    /// A rotation exists but the protocol marker doesn't: a group created
    /// by an earlier test build, which this version can neither join nor
    /// share a connector with.
    Legacy,
}

pub const EXISTING_SPACE_REFUSAL: &str =
    "A sync group already exists in this connector. Join it with a join code, its recovery phrase, or approval from another device, or confirm that you want a separate new group.";

pub const LEGACY_SPACE_REFUSAL: &str =
    "This connector holds a sync group this version of Three Strands can't use: either it was created by an earlier test version, or a shared folder hasn't finished syncing yet. If you just set up sync on another device, wait for your sync app to finish, then try again. Otherwise update every device, delete this connector's files (Connectors → Delete files and disconnect), add it again, and create a new sync group.";

/// What one transport holds, as far as a scan can tell.
enum TransportSpace {
    Current,
    Legacy,
    Empty,
    Unknown,
}

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
/// of starting a second, disconnected space in the same location, and to
/// recognize a group created by an earlier, incompatible build.
pub async fn inspect_sync_space(transports: &[Arc<dyn SyncTransport>]) -> SyncSpacePresence {
    let mut incomplete = transports.is_empty();
    let mut legacy = false;
    for transport in transports {
        match inspect_transport(transport.as_ref()).await {
            TransportSpace::Current => return SyncSpacePresence::Existing,
            TransportSpace::Legacy => legacy = true,
            TransportSpace::Empty => {}
            TransportSpace::Unknown => incomplete = true,
        }
    }
    if legacy {
        SyncSpacePresence::Legacy
    } else if incomplete {
        SyncSpacePresence::Unknown
    } else {
        SyncSpacePresence::None
    }
}

/// Scans one transport until it finds a self-consistent rotation, then
/// asks whether the protocol marker is there too. Only a definite absence
/// counts as a legacy group; a marker that can't be fetched for any other
/// reason is unknown, never legacy, so a flaky connector is never mistaken
/// for one to delete.
async fn inspect_transport(transport: &dyn SyncTransport) -> TransportSpace {
    let mut incomplete = false;
    let mut cursor: Option<String> = None;
    loop {
        let page = match transport.scan(cursor.as_deref()).await {
            Ok(Some(page)) => page,
            Ok(None) => break,
            Err(_) => return TransportSpace::Unknown,
        };
        for locator in &page.objects {
            match transport.get_object(&locator.cid).await {
                Ok(bytes) if is_self_consistent_rotation(&bytes) => {
                    return match transport.get_object(&TransportCid(protocol_marker_cid())).await {
                        Ok(marker) if marker == PROTOCOL_MARKER => TransportSpace::Current,
                        Err(TransportError::NotFound) => TransportSpace::Legacy,
                        _ => TransportSpace::Unknown,
                    };
                }
                Ok(_) => {}
                Err(_) => incomplete = true,
            }
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    if incomplete { TransportSpace::Unknown } else { TransportSpace::Empty }
}

pub(crate) const RECOVERY_INCOMPLETE: &str =
    "Some of this sync group's files haven't reached this connector yet, so joining now would leave part of its history unreadable. Wait for your sync app to finish, then try again.";

/// The newest key epoch any listed, still-active device's verified head
/// says it's using, on any transport. Heads that can't be fetched or
/// verified are skipped.
async fn newest_epoch_in_heads(transports: &[Arc<dyn SyncTransport>], roster: &[RosterEntry]) -> u32 {
    let locators: Vec<threestrands_sync_transport::HeadLocator> = roster
        .iter()
        .filter(|entry| entry.status == "active")
        .map(|entry| threestrands_sync_transport::HeadLocator { device_id: entry.device_id, remote_id: None })
        .collect();
    let mut newest = 0;
    for transport in transports {
        let Ok(heads) = transport.resolve_heads(&locators).await else { continue };
        for signed in heads {
            let verified = roster
                .iter()
                .find(|entry| entry.device_id == signed.head.device_id)
                .and_then(|entry| roster_entry_verifying_key(entry).ok())
                .is_some_and(|key| threestrands_sync_envelope::verify_device_head(&key, &signed).is_ok());
            if verified {
                newest = newest.max(signed.head.epoch);
            }
        }
    }
    newest
}

/// Refuses to set up or join sync over a connector that holds a group from
/// an earlier, incompatible build.
async fn refuse_legacy_space(transports: &[Arc<dyn SyncTransport>]) -> Result<(), String> {
    if inspect_sync_space(transports).await == SyncSpacePresence::Legacy {
        return Err(LEGACY_SPACE_REFUSAL.to_string());
    }
    Ok(())
}

/// Whether every transport definitely lacks the protocol marker. Where a
/// group is known to exist (an invitation was just found), that means the
/// group is from an earlier, incompatible build. Cheaper than a full
/// [`inspect_sync_space`] scan.
pub(crate) async fn protocol_marker_missing(transports: &[Arc<dyn SyncTransport>]) -> bool {
    let cid = TransportCid(protocol_marker_cid());
    for transport in transports {
        match transport.get_object(&cid).await {
            Err(TransportError::NotFound) => {}
            _ => return false,
        }
    }
    !transports.is_empty()
}

/// Stores the protocol marker on every transport now, and records it as a
/// local object so repair delivers it to any connector added later.
pub(crate) async fn publish_protocol_marker(database: &Database, transports: &[Arc<dyn SyncTransport>]) -> Result<(), String> {
    database.ensure_protocol_marker_object()?;
    let cid = TransportCid(protocol_marker_cid());
    for transport in transports {
        if let Err(error) = transport.put_object(&cid, PROTOCOL_MARKER).await {
            log::debug!(target: "replicated_sync", "storing the protocol marker on {} failed: {error}", transport.instance_id().0);
        }
    }
    Ok(())
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
    match inspect_sync_space(transports).await {
        SyncSpacePresence::Legacy => return Err(LEGACY_SPACE_REFUSAL.to_string()),
        SyncSpacePresence::Existing if !allow_existing_space => return Err(EXISTING_SPACE_REFUSAL.to_string()),
        _ => {}
    }
    database.set_beta_features_enabled(true)?;
    // The marker goes out before the genesis rotation, so a device that
    // inspects this connector never sees the group without it.
    publish_protocol_marker(database, transports).await?;

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
    let cid = publish_control_object(database, transports, &bytes).await?;

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
    refuse_legacy_space(transports).await?;
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
    let cid = publish_control_object(database, transports, &bytes).await?;
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
    // Objects arrive in content-address order, so a join-code redemption
    // can come before the invitation it names. Those are retried once the
    // whole sweep has run, rather than waiting for the next cycle.
    let mut deferred: Vec<(String, Vec<u8>)> = Vec::new();
    for transport in transports {
        let mut cursor: Option<String> = None;
        loop {
            let Some(page) = transport.scan(cursor.as_deref()).await.map_err(display)? else { break };
            for locator in &page.objects {
                if database.seen_control_object(&locator.cid.0)? {
                    continue;
                }
                let Ok(bytes) = transport.get_object(&locator.cid).await else { continue };
                match try_apply_control_object(database, identity, epoch_keys, &locator.cid.0, &bytes)? {
                    ControlObject::Applied => database.mark_control_object_seen(&locator.cid.0, "control")?,
                    // Not recognized as a control object at all (most
                    // objects are sealed snapshots) — still mark
                    // seen so we never re-fetch it.
                    ControlObject::NotControl => database.mark_control_object_seen(&locator.cid.0, "other")?,
                    // A join-code redemption whose invitation hasn't
                    // arrived yet: retry after this sweep, and leave it
                    // unseen if it still can't be applied.
                    ControlObject::RetryLater => deferred.push((locator.cid.0.clone(), bytes)),
                }
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
    }
    for (cid, bytes) in deferred {
        if matches!(try_apply_control_object(database, identity, epoch_keys, &cid, &bytes)?, ControlObject::Applied) {
            database.mark_control_object_seen(&cid, "control")?;
        }
    }
    Ok(())
}

enum ControlObject {
    Applied,
    NotControl,
    RetryLater,
}

fn try_apply_control_object(database: &Database, identity: &DeviceIdentity, epoch_keys: &dyn EpochKeyStore, cid: &str, bytes: &[u8]) -> Result<ControlObject, String> {
    if let Ok(signed) = decode_signed_enrollment_request(bytes) {
        apply_incoming_request(database, identity, signed)?;
        return Ok(ControlObject::Applied);
    }
    if let Ok(signed) = decode_signed_enrollment_grant(bytes) {
        match apply_key_share(database, identity, epoch_keys, &signed, cid)? {
            KeyShare::Applied => return Ok(ControlObject::Applied),
            KeyShare::UnknownSender => return Ok(ControlObject::RetryLater),
            KeyShare::Other => {}
        }
        apply_incoming_grant(database, identity, epoch_keys, signed)?;
        return Ok(ControlObject::Applied);
    }
    if let Ok(signed) = decode_signed_enrollment_rejection(bytes) {
        return Ok(if apply_incoming_rejection(database, signed)? { ControlObject::Applied } else { ControlObject::RetryLater });
    }
    if let Ok(signed) = decode_signed_key_rotation(bytes) {
        return Ok(match apply_incoming_rotation(database, identity, epoch_keys, signed, cid)? {
            RotationOutcome::Done => ControlObject::Applied,
            RotationOutcome::UnknownInitiator => ControlObject::RetryLater,
        });
    }
    if let Ok(signed) = threestrands_sync_envelope::decode_signed_invitation(bytes) {
        join_codes::apply_incoming_invitation(database, identity, signed, cid)?;
        return Ok(ControlObject::Applied);
    }
    if let Ok(signed) = threestrands_sync_envelope::decode_signed_invitation_redemption(bytes) {
        return Ok(match join_codes::apply_incoming_redemption(database, identity, signed, cid, now_ms())? {
            join_codes::JoinObjectOutcome::Done => ControlObject::Applied,
            join_codes::JoinObjectOutcome::RetryLater => ControlObject::RetryLater,
        });
    }
    Ok(ControlObject::NotControl)
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

    // This grant may be addressed to another device that received the same
    // request. Clear our copy only when an already trusted active peer signed
    // it and the roster names the exact requester keys we recorded.
    let unresolved_incoming: Option<(String, Vec<u8>, Vec<u8>)> = database
        .connection()?
        .query_row(
            "SELECT device_id, ed25519_public, x25519_public FROM replicated_sync_enrollment_requests
             WHERE request_id=?1 AND direction='incoming' AND status IN ('pending','rejected')",
            params![request_id_hex],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(display)?;
    if !signed.grant.signed_by_recovery {
        let trusted_approver: Option<Vec<u8>> = database
            .connection()?
            .query_row(
                "SELECT public_key FROM sync_devices WHERE device_id=?1 AND status='active' AND public_key IS NOT NULL",
                params![encode_id(signed.grant.approver_device_id.as_bytes())],
                |row| row.get(0),
            )
            .optional()
            .map_err(display)?;
        let approver_verifies = trusted_approver
            .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
            .and_then(|bytes| VerifyingKey::from_bytes(&bytes).ok())
            .map(|key| verify_enrollment_grant(&key, &signed).is_ok())
            .unwrap_or(false);
        let approved_requester = signed.grant.roster.iter().find(|entry| {
            entry.status == "active"
                && unresolved_incoming.as_ref().is_none_or(|(requester_id, requester_ed25519, requester_x25519)| {
                    encode_id(entry.device_id.as_bytes()) == *requester_id
                        && entry.ed25519_public.as_slice() == requester_ed25519
                        && entry.x25519_public.as_slice() == requester_x25519
                })
        });
        if approver_verifies {
            if let Some(entry) = approved_requester {
                if let Some((requester_id, _, _)) = unresolved_incoming {
                    if encode_id(entry.device_id.as_bytes()) == requester_id {
                        database
                            .connection()?
                            .execute(
                                "UPDATE replicated_sync_enrollment_requests SET status='approved' WHERE request_id=?1 AND direction='incoming' AND status IN ('pending','rejected')",
                                params![request_id_hex],
                            )
                            .map_err(display)?;
                    }
                } else {
                    // A peer may have been offline until after both objects
                    // were published. Keep an approved marker so a grant
                    // scanned before its request cannot resurrect a stale
                    // pending notice when that request is seen later.
                    let requester_id = encode_id(entry.device_id.as_bytes());
                    let requester_ed25519 = entry.ed25519_public.as_slice();
                    if let Ok(requester_x25519) = roster_entry_x25519(entry) {
                        let fingerprint = enrollment_fingerprint(requester_ed25519, &requester_x25519);
                        database
                            .connection()?
                            .execute(
                                "INSERT OR IGNORE INTO replicated_sync_enrollment_requests(request_id, direction, device_id, ed25519_public, x25519_public, fingerprint, status, created_at)
                                 VALUES (?1,'incoming',?2,?3,?4,?5,'approved',?6)",
                                params![
                                    request_id_hex,
                                    requester_id,
                                    requester_ed25519,
                                    requester_x25519.to_vec(),
                                    fingerprint,
                                    Utc::now().to_rfc3339(),
                                ],
                            )
                            .map_err(display)?;
                    }
                }
            }
        }
    }

    let matches_our_request: Option<String> = database
        .connection()?
        .query_row(
            "SELECT request_id FROM replicated_sync_enrollment_requests WHERE request_id=?1 AND direction='outgoing' AND status IN ('pending','rejected')",
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
            "UPDATE replicated_sync_enrollment_requests SET status='staged', pending_grant_cbor=?2 WHERE request_id=?1 AND direction='outgoing' AND status IN ('pending','rejected')",
            params![request_id_hex, grant_cbor],
        )
        .map_err(display)?;
    Ok(())
}

// ============================== Key catch-up ==============================

/// What a received grant turned out to be, as a key share.
enum KeyShare {
    /// A key share for this device from a trusted member: applied (or, from
    /// a revoked member or with a bad signature, deliberately ignored).
    Applied,
    /// Sealed to this device by a member it hasn't heard of yet: retry once
    /// the announcement that introduces that member arrives.
    UnknownSender,
    /// An ordinary enrollment grant, or not addressed to this device.
    Other,
}

/// A key share is a grant an enrolled member publishes to an enrolled
/// peer it sees lagging behind on key epochs (see
/// [`share_keys_with_lagging_peers`]): the sender's current and earlier
/// epoch keys, sealed to that peer. It needs no human confirmation, for
/// the same reason a rotation doesn't: it comes from a device this one
/// already trusts, and only hands over keys the group already uses. That
/// catches a device up when a rotation was never sealed to it — because the
/// rotating device hadn't heard of it yet, or because it joined by recovery
/// phrase before that rotation reached its connector.
fn apply_key_share(
    database: &Database,
    identity: &DeviceIdentity,
    epoch_keys: &dyn EpochKeyStore,
    signed: &SignedEnrollmentGrant,
    cid: &str,
) -> Result<KeyShare, String> {
    let grant = &signed.grant;
    if grant.signed_by_recovery {
        return Ok(KeyShare::Other);
    }
    let enrolled: bool = database
        .connection()?
        .query_row("SELECT EXISTS(SELECT 1 FROM sync_epoch_history)", [], |row| row.get(0))
        .map_err(display)?;
    let answers_our_request: bool = database
        .connection()?
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM replicated_sync_enrollment_requests WHERE request_id=?1 AND direction='outgoing' AND status IN ('pending','rejected','staged'))",
            params![encode_id(grant.request_id.as_bytes())],
            |row| row.get(0),
        )
        .map_err(display)?;
    if !enrolled || answers_our_request {
        return Ok(KeyShare::Other);
    }
    let Some(current) = try_open_sealed_box(&identity.x25519_secret, &grant.sealed_epoch_key) else {
        return Ok(KeyShare::Other);
    };
    let sender: Option<(String, Option<Vec<u8>>)> = database
        .connection()?
        .query_row(
            "SELECT status, public_key FROM sync_devices WHERE device_id=?1",
            params![encode_id(grant.approver_device_id.as_bytes())],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(display)?;
    let Some((status, public_key)) = sender else {
        return Ok(KeyShare::UnknownSender);
    };
    let verified = status == "active"
        && public_key
            .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
            .and_then(|bytes| VerifyingKey::from_bytes(&bytes).ok())
            .is_some_and(|key| verify_enrollment_grant(&key, signed).is_ok());
    let Ok(current): Result<[u8; 32], _> = current.try_into() else {
        return Ok(KeyShare::Applied);
    };
    let Some(earlier) = open_earlier_epoch_keys(&identity.x25519_secret, &grant.earlier_epoch_keys) else {
        return Ok(KeyShare::Applied);
    };
    if !verified {
        return Ok(KeyShare::Applied);
    }
    database.adopt_roster(&grant.roster)?;
    epoch_keys.store(grant.key_epoch, &current)?;
    database.record_epoch_activation(grant.key_epoch, cid)?;
    store_earlier_epoch_keys(database, epoch_keys, &earlier, cid)?;
    database.advance_active_epoch(grant.key_epoch)?;
    Ok(KeyShare::Applied)
}

/// Publishes a key share to every trusted, active peer whose latest head
/// says it is on an older key epoch than this device, once per peer per
/// epoch. A peer that is merely slow to apply a rotation gets a copy of
/// keys it would have received anyway; one the rotation was never sealed to
/// gets the only copy. See [`apply_key_share`].
pub(crate) async fn share_keys_with_lagging_peers(
    database: &Database,
    keys: &LocalKeys,
    transports: &[Arc<dyn SyncTransport>],
) -> Result<usize, String> {
    let lagging: Vec<(String, Vec<u8>)> = {
        let connection = database.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT d.device_id, d.x25519_public FROM sync_remote_states r
                 JOIN sync_devices d ON d.device_id = r.device_id
                 WHERE d.status = 'active' AND d.is_self = 0 AND d.x25519_public IS NOT NULL
                   AND r.last_head_epoch < ?1 AND (r.keys_shared_epoch IS NULL OR r.keys_shared_epoch < ?1)",
            )
            .map_err(display)?;
        let rows = statement
            .query_map(params![keys.key_epoch], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(display)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(display)?;
        rows
    };
    if lagging.is_empty() {
        return Ok(0);
    }
    let (recovery_ed25519, recovery_x25519) = database
        .recovery_public_keys()?
        .ok_or_else(|| "No recovery keys on record for this sync group".to_string())?;
    let roster = database.full_roster_snapshot()?;
    for (device_id, x25519_public) in &lagging {
        let recipient: [u8; 32] = x25519_public.as_slice().try_into().map_err(|_| "Invalid peer X25519 key".to_string())?;
        let share = EnrollmentGrant {
            request_id: RequestId::from_bytes(random_id()),
            approver_device_id: keys.device_id,
            signed_by_recovery: false,
            key_epoch: keys.key_epoch,
            sealed_epoch_key: ByteBuf::from(seal_to_x25519(&recipient, &keys.k_epoch)),
            roster: roster.clone(),
            recovery_ed25519_public: ByteBuf::from(recovery_ed25519.to_vec()),
            recovery_x25519_public: ByteBuf::from(recovery_x25519.to_vec()),
            created_at_ms: now_ms(),
            earlier_epoch_keys: seal_earlier_epoch_keys(keys, &recipient)?,
        };
        let signed = sign_enrollment_grant(&keys.signing_key, share).map_err(display)?;
        let bytes = encode_signed_enrollment_grant(&signed).map_err(display)?;
        let cid = publish_control_object(database, transports, &bytes).await?;
        database.mark_control_object_seen(&cid, "key_share")?;
        database
            .connection()?
            .execute("UPDATE sync_remote_states SET keys_shared_epoch=?2 WHERE device_id=?1", params![device_id, keys.key_epoch])
            .map_err(display)?;
    }
    Ok(lagging.len())
}

/// Existing members honor rejections only from a currently trusted active
/// peer. A joining device can verify the response signature as informational
/// status before it has a roster of its own. Missing requests are retried
/// after the rest of the sweep so object ordering cannot leave peers stale.
fn apply_incoming_rejection(database: &Database, signed: threestrands_sync_envelope::SignedEnrollmentRejection) -> Result<bool, String> {
    let request_id = encode_id(signed.rejection.request_id.as_bytes());
    let rejector_id = encode_id(signed.rejection.rejector_device_id.as_bytes());
    let trusted_key: Option<Vec<u8>> = database
        .connection()?
        .query_row(
            "SELECT public_key FROM sync_devices WHERE device_id=?1 AND status='active' AND public_key IS NOT NULL",
            params![rejector_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(display)?;
    let embedded_key = <[u8; 32]>::try_from(signed.rejection.rejector_ed25519_public.as_slice())
        .ok()
        .and_then(|bytes| VerifyingKey::from_bytes(&bytes).ok());
    let Some(embedded_key) = embedded_key else { return Ok(true) };
    let is_trusted_peer = trusted_key
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        .is_some_and(|bytes| bytes == embedded_key.to_bytes());

    let request_direction: Option<String> = database
        .connection()?
        .query_row(
            "SELECT direction FROM replicated_sync_enrollment_requests WHERE request_id=?1",
            params![request_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(display)?;
    let Some(direction) = request_direction else { return Ok(false) };
    // Existing devices only honor decisions from a peer already in their
    // trusted roster. A joining device has no roster yet, so it can verify
    // the signature as an informational response; a later approval grant
    // still takes precedence and enables explicit fingerprint review.
    if (direction == "incoming" && !is_trusted_peer) || verify_enrollment_rejection(&embedded_key, &signed).is_err() {
        return Ok(true);
    }

    // A published grant is the approval decision and cannot be undone by a
    // later rejection. Until a grant arrives, rejection resolves both the
    // joiner's outgoing request and every peer's incoming copy.
    database
        .connection()?
        .execute(
            "UPDATE replicated_sync_enrollment_requests SET status='rejected'
             WHERE request_id=?1 AND status='pending'",
            params![request_id],
        )
        .map_err(display)?;
    Ok(true)
}

pub async fn reject_enrollment_request(
    database: &Database,
    identity: &DeviceIdentity,
    request_id_hex: &str,
    transports: &[Arc<dyn SyncTransport>],
) -> Result<(), String> {
    let exists: bool = database
        .connection()?
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM replicated_sync_enrollment_requests WHERE request_id=?1 AND direction='incoming' AND status='pending')",
            params![request_id_hex],
            |row| row.get(0),
        )
        .map_err(display)?;
    if !exists {
        return Err("This request is no longer pending.".to_string());
    }
    let rejection = EnrollmentRejection {
        request_id: RequestId::from_bytes(decode_id(request_id_hex)?),
        rejector_device_id: identity.device_id,
        rejector_ed25519_public: ByteBuf::from(identity.verifying_key.to_bytes().to_vec()),
        rejected_at_ms: now_ms(),
    };
    let signed = sign_enrollment_rejection(&identity.signing_key, rejection).map_err(display)?;
    let bytes = encode_signed_enrollment_rejection(&signed).map_err(display)?;
    let cid = publish_control_object(database, transports, &bytes).await?;
    database.mark_control_object_seen(&cid, "enrollment_rejection")?;
    database
        .connection()?
        .execute(
            "UPDATE replicated_sync_enrollment_requests SET status='rejected' WHERE request_id=?1 AND direction='incoming' AND status='pending'",
            params![request_id_hex],
        )
        .map_err(display)?;
    Ok(())
}

/// Whether a rotation was applied (or deliberately ignored), or names an
/// initiator this device hasn't heard of yet and should be retried.
enum RotationOutcome {
    Done,
    UnknownInitiator,
}

fn apply_incoming_rotation(database: &Database, identity: &DeviceIdentity, epoch_keys: &dyn EpochKeyStore, signed: SignedKeyRotation, cid: &str) -> Result<RotationOutcome, String> {
    // A rotation is only auto-applied when its signer is already in our
    // roster: no new trust decision, just an authenticated update to an
    // existing one. A genesis/recovery-only rotation (signer not yet
    // trusted) is picked up by `join_with_recovery_phrase` instead.
    //
    // A signer this device has never heard of is usually a member that
    // joined while it was away: the announcement that introduces it can be
    // scanned after this rotation, in the same sweep or a later one. Such a
    // rotation is retried rather than dropped, or this device would never
    // receive that epoch's key. A signer that is known but revoked is
    // ignored for good.
    let roster = database.known_device_roster()?;
    let Some((_, verifying_key)) = roster.iter().find(|(device_id, _)| *device_id == signed.rotation.initiator_device_id) else {
        let known_but_revoked: bool = database
            .connection()?
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_devices WHERE device_id=?1)",
                params![encode_id(signed.rotation.initiator_device_id.as_bytes())],
                |row| row.get(0),
            )
            .map_err(display)?;
        return Ok(if known_but_revoked { RotationOutcome::Done } else { RotationOutcome::UnknownInitiator });
    };
    if verify_key_rotation(verifying_key, &signed).is_err() {
        return Ok(RotationOutcome::Done);
    }
    apply_rotation_common(database, identity, epoch_keys, &signed, cid)?;
    Ok(RotationOutcome::Done)
}

fn apply_rotation_common(database: &Database, identity: &DeviceIdentity, epoch_keys: &dyn EpochKeyStore, signed: &SignedKeyRotation, cid: &str) -> Result<(), String> {
    database.adopt_roster(&signed.rotation.roster)?;
    join_codes::note_admission_if_listed(database, identity, &signed.rotation.roster)?;
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
        // Rotations are scanned in content-address order, not epoch order,
        // so a device catching up can meet an older rotation after a newer
        // one. Keep its key for opening that epoch's history, but never move
        // the active epoch backwards.
        database.advance_active_epoch(signed.rotation.key_epoch)?;
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
    let earlier = open_earlier_epoch_keys(&identity.x25519_secret, &signed.grant.earlier_epoch_keys)
        .ok_or_else(|| "This grant's earlier keys were not sealed to this device".to_string())?;

    database.adopt_roster(&signed.grant.roster)?;
    let recovery_ed25519: [u8; 32] = signed.grant.recovery_ed25519_public.as_slice().try_into().map_err(|_| "Invalid recovery key".to_string())?;
    let recovery_x25519: [u8; 32] = signed.grant.recovery_x25519_public.as_slice().try_into().map_err(|_| "Invalid recovery key".to_string())?;
    database.set_recovery_public_keys(&recovery_ed25519, &recovery_x25519)?;

    epoch_keys.store(signed.grant.key_epoch, &k_epoch)?;
    database.set_active_epoch(signed.grant.key_epoch)?;
    let source_cid = compute_cid(&grant_cbor);
    database.record_epoch_activation(signed.grant.key_epoch, &source_cid)?;
    store_earlier_epoch_keys(database, epoch_keys, &earlier, &source_cid)?;

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
    let (recovery_ed25519, recovery_x25519) = database.recovery_public_keys()?.ok_or_else(|| "No recovery keys on record for this sync group".to_string())?;

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
        earlier_epoch_keys: seal_earlier_epoch_keys(keys, &requester_x25519)?,
    };
    let signed = sign_enrollment_grant(&identity.signing_key, grant).map_err(display)?;
    let bytes = encode_signed_enrollment_grant(&signed).map_err(display)?;
    let cid = publish_control_object(database, transports, &bytes).await?;
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
    let (recovery_ed25519, recovery_x25519) = database.recovery_public_keys()?.ok_or_else(|| "No recovery keys on record for this sync group".to_string())?;

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
    let cid = publish_control_object(database, transports, &bytes).await?;
    database.mark_control_object_seen(&cid, "key_rotation")?;

    epoch_keys.store(next_epoch, &k_epoch)?;
    database.set_active_epoch(next_epoch)?;
    database.record_epoch_activation(next_epoch, &cid)?;
    Ok(())
}

// ============================ Recovery-phrase import ============================

/// Joins an existing sync space using only a recovery phrase — no peer
/// device needs to be online. Scans every transport for rotation objects
/// whose sealed stanzas open with the phrase-derived recovery X25519
/// secret; that success is itself the trust proof (see the module docs).
///
/// Every rotation carries a recovery stanza, so the scan collects the key
/// for every epoch it finds, which is what lets this device open history
/// sealed before the latest rotation. It adopts the roster and active epoch
/// of the newest rotation, not whichever one the scan happens to meet
/// first. Only rotations that verify against their own initiator count, so
/// arbitrary bytes that happen to decode can't steer the choice.
pub async fn join_with_recovery_phrase(database: &Database, identity: &DeviceIdentity, epoch_keys: &dyn EpochKeyStore, phrase: &str, transports: &[Arc<dyn SyncTransport>]) -> Result<(), String> {
    refuse_legacy_space(transports).await?;
    database.set_beta_features_enabled(true)?;
    let seed = recovery_seed_from_phrase(phrase)?;
    let recovery_secret_bytes = recovery_x25519_secret(&seed).to_bytes();

    let mut opened: std::collections::BTreeMap<u32, (SignedKeyRotation, String, [u8; 32])> = std::collections::BTreeMap::new();
    for transport in transports {
        let mut cursor: Option<String> = None;
        loop {
            let Some(page) = transport.scan(cursor.as_deref()).await.map_err(display)? else { break };
            for locator in &page.objects {
                let Ok(bytes) = transport.get_object(&locator.cid).await else { continue };
                if !is_self_consistent_rotation(&bytes) {
                    continue;
                }
                let Ok(signed) = decode_signed_key_rotation(&bytes) else { continue };
                let Some(k_epoch_bytes) = signed
                    .rotation
                    .sealed_stanzas
                    .iter()
                    .find_map(|stanza| try_open_sealed_box(&recovery_secret_bytes, stanza))
                else {
                    continue;
                };
                let Ok(k_epoch): Result<[u8; 32], _> = k_epoch_bytes.try_into() else { continue };
                opened.entry(signed.rotation.key_epoch).or_insert((signed, locator.cid.0.clone(), k_epoch));
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
    }
    let Some((&latest_epoch, (latest, _, latest_key))) = opened.iter().next_back() else {
        return Err("No rotation object on any configured transport opened with this recovery phrase yet".to_string());
    };
    let latest_key = *latest_key;
    // Joining without every epoch's key is permanent: the missing epoch's
    // rotation was sealed only to the members of the time and the recovery
    // key, and this device won't keep the phrase. Epochs are consecutive,
    // and every device's head names the epoch it's on, so a rotation that
    // hasn't reached this connector yet shows up as a gap or as a newer
    // epoch in use. Either way, wait rather than join incomplete.
    let newest_in_use = newest_epoch_in_heads(transports, &latest.rotation.roster).await;
    if (0..=latest_epoch).any(|epoch| !opened.contains_key(&epoch)) || newest_in_use > latest_epoch {
        return Err(RECOVERY_INCOMPLETE.to_string());
    }

    database.adopt_roster(&latest.rotation.roster)?;
    database.set_recovery_public_keys(
        latest.rotation.recovery_ed25519_public.as_slice().try_into().map_err(|_| "Invalid recovery key".to_string())?,
        latest.rotation.recovery_x25519_public.as_slice().try_into().map_err(|_| "Invalid recovery key".to_string())?,
    )?;
    // Trust ourselves alongside the recovered roster — a fresh device
    // recovering has no prior self-entry there.
    let self_x25519_public = x25519_public_bytes(&identity.x25519_secret);
    database.trust_device_keys(identity.device_id.as_bytes(), &identity.verifying_key, &self_x25519_public)?;
    for (key_epoch, (_, cid, k_epoch)) in &opened {
        epoch_keys.store(*key_epoch, k_epoch)?;
        database.record_epoch_activation(*key_epoch, cid)?;
    }
    database.set_active_epoch(latest_epoch)?;

    // Broadcast our own membership so every other device (which has no
    // pending request matching this — nothing asked it to expect us) learns
    // about and trusts us too. Signed by the recovery key, not a peer, so
    // every device that already knows the recovery public key applies it
    // immediately — see `apply_incoming_grant`.
    let recovery_ed25519_public = recovery_ed25519_signing_key(&seed).verifying_key();
    let announcement = EnrollmentGrant {
        request_id: RequestId::from_bytes(random_id()),
        approver_device_id: identity.device_id,
        signed_by_recovery: true,
        key_epoch: latest_epoch,
        sealed_epoch_key: ByteBuf::from(seal_to_x25519(&self_x25519_public, &latest_key)),
        roster: database.full_roster_snapshot()?,
        recovery_ed25519_public: ByteBuf::from(recovery_ed25519_public.to_bytes().to_vec()),
        recovery_x25519_public: latest.rotation.recovery_x25519_public.clone(),
        created_at_ms: now_ms(),
        earlier_epoch_keys: vec![],
    };
    database.ensure_protocol_marker_object()?;
    let signed_announcement = sign_enrollment_grant(&recovery_ed25519_signing_key(&seed), announcement).map_err(display)?;
    let announcement_bytes = encode_signed_enrollment_grant(&signed_announcement).map_err(display)?;
    let announcement_cid = publish_control_object(database, transports, &announcement_bytes).await?;
    database.mark_control_object_seen(&announcement_cid, "enrollment_grant")?;
    Ok(())
}

/// Keychain-free stand-ins for a device's identity and epoch-key storage,
/// shared by every test that drives enrollment and sync end to end.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use rand::RngCore;
    use threestrands_sync_envelope::SigningKey;

    /// Builds a synthetic device identity directly, exactly as `test_keys`
    /// does in `replicated_sync.rs` — never touches the OS keychain.
    pub(crate) fn test_identity(database: &Database) -> DeviceIdentity {
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
    pub(crate) struct FakeEpochKeyStore(pub(crate) std::sync::Mutex<std::collections::HashMap<u32, [u8; 32]>>);

    impl EpochKeyStore for FakeEpochKeyStore {
        fn store(&self, key_epoch: u32, key: &[u8; 32]) -> Result<(), String> {
            self.0.lock().unwrap().insert(key_epoch, *key);
            Ok(())
        }
    }

    impl FakeEpochKeyStore {
        pub(crate) fn get(&self, key_epoch: u32) -> Option<[u8; 32]> {
            self.0.lock().unwrap().get(&key_epoch).copied()
        }
    }

    /// Builds this device's `LocalKeys` for push/pull straight from its
    /// synthetic identity and fake epoch-key store — the test equivalent of
    /// `Database::local_replicated_keys`, which touches the real keychain
    /// and so is never called from a test.
    pub(crate) fn local_keys_for(database: &Database, identity: &DeviceIdentity, epoch_keys: &FakeEpochKeyStore) -> LocalKeys {
        let active_epoch: u32 = database
            .connection()
            .unwrap()
            .query_row("SELECT active_epoch FROM sync_spaces WHERE id=?1", params![SPACE_ID], |row| row.get(0))
            .unwrap();
        let k_epoch = epoch_keys.get(active_epoch).expect("epoch key must already be stored for this test");
        let earlier_epoch_keys = epoch_keys.0.lock().unwrap().iter().filter(|(epoch, _)| **epoch < active_epoch).map(|(epoch, key)| (*epoch, *key)).collect();
        LocalKeys {
            signing_key: identity.signing_key.clone(),
            k_epoch,
            key_epoch: active_epoch,
            earlier_epoch_keys,
            device_id: identity.device_id,
            sync_space_id: identity.sync_space_id.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::test_support::{local_keys_for, test_identity, FakeEpochKeyStore};
    use threestrands_sync_protocol::EntityType;
    use threestrands_sync_transport::fake::FakeTransport;

    use crate::replicated_sync::{pull_from_transports, push_local_state};

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
            (
                EnrollmentStatus::Rejected { request_id: "request-3".to_string(), fingerprint: "AAAA-CCCC".to_string() },
                serde_json::json!({ "state": "rejected", "requestId": "request-3", "fingerprint": "AAAA-CCCC" }),
            ),
            (
                EnrollmentStatus::Enrolled { device_count: 2, awaiting_admission_from: None },
                serde_json::json!({ "state": "enrolled", "deviceCount": 2, "awaitingAdmissionFrom": null }),
            ),
            (
                EnrollmentStatus::Enrolled { device_count: 1, awaiting_admission_from: Some("Work laptop".to_string()) },
                serde_json::json!({ "state": "enrolled", "deviceCount": 1, "awaitingAdmissionFrom": "Work laptop" }),
            ),
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
            EnrollmentStatus::Enrolled { device_count, .. } => assert_eq!(device_count, 1),
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

    #[tokio::test]
    async fn another_existing_device_clears_a_request_after_a_trusted_peer_approves_it() {
        let transports = fake_transports("shared");
        let database_a = Database::open_memory();
        let identity_a = test_identity(&database_a);
        let epoch_keys_a = FakeEpochKeyStore::default();
        let phrase = begin_genesis(&database_a, &identity_a, &epoch_keys_a, &transports, false).await.unwrap();

        let database_b = Database::open_memory();
        let identity_b = test_identity(&database_b);
        let epoch_keys_b = FakeEpochKeyStore::default();
        join_with_recovery_phrase(&database_b, &identity_b, &epoch_keys_b, &phrase, &transports).await.unwrap();
        run_enrollment_sweep(&database_a, &identity_a, &epoch_keys_a, &transports).await.unwrap();

        let database_c = Database::open_memory();
        let identity_c = test_identity(&database_c);
        publish_enrollment_request(&database_c, &identity_c, &transports).await.unwrap();
        run_enrollment_sweep(&database_a, &identity_a, &epoch_keys_a, &transports).await.unwrap();
        run_enrollment_sweep(&database_b, &identity_b, &epoch_keys_b, &transports).await.unwrap();
        let pending_a = database_a.pending_incoming_enrollment_requests().unwrap();
        let pending_b = database_b.pending_incoming_enrollment_requests().unwrap();
        assert_eq!(pending_a.len(), 1);
        assert_eq!(pending_b.len(), 1);

        let keys_a = local_keys_for(&database_a, &identity_a, &epoch_keys_a);
        approve_enrollment_request(&database_a, &identity_a, &keys_a, &pending_a[0].request_id, &transports).await.unwrap();
        run_enrollment_sweep(&database_b, &identity_b, &epoch_keys_b, &transports).await.unwrap();
        assert!(database_b.pending_incoming_enrollment_requests().unwrap().is_empty());
    }

    #[tokio::test]
    async fn rejecting_a_request_resolves_it_for_the_group_and_the_joining_device() {
        let transports = fake_transports("shared");
        let database_a = Database::open_memory();
        let identity_a = test_identity(&database_a);
        let epoch_keys_a = FakeEpochKeyStore::default();
        let phrase = begin_genesis(&database_a, &identity_a, &epoch_keys_a, &transports, false).await.unwrap();

        let database_b = Database::open_memory();
        let identity_b = test_identity(&database_b);
        let epoch_keys_b = FakeEpochKeyStore::default();
        join_with_recovery_phrase(&database_b, &identity_b, &epoch_keys_b, &phrase, &transports).await.unwrap();
        run_enrollment_sweep(&database_a, &identity_a, &epoch_keys_a, &transports).await.unwrap();

        let database_c = Database::open_memory();
        let identity_c = test_identity(&database_c);
        publish_enrollment_request(&database_c, &identity_c, &transports).await.unwrap();
        run_enrollment_sweep(&database_a, &identity_a, &epoch_keys_a, &transports).await.unwrap();
        run_enrollment_sweep(&database_b, &identity_b, &epoch_keys_b, &transports).await.unwrap();
        let request_id = database_a.pending_incoming_enrollment_requests().unwrap()[0].request_id.clone();

        reject_enrollment_request(&database_a, &identity_a, &request_id, &transports).await.unwrap();
        run_enrollment_sweep(&database_b, &identity_b, &epoch_keys_b, &transports).await.unwrap();
        run_enrollment_sweep(&database_c, &identity_c, &FakeEpochKeyStore::default(), &transports).await.unwrap();

        assert!(database_b.pending_incoming_enrollment_requests().unwrap().is_empty());
        assert!(matches!(database_c.enrollment_status().unwrap(), EnrollmentStatus::Rejected { .. }));
    }

    #[tokio::test]
    async fn a_published_approval_takes_precedence_over_a_concurrent_rejection() {
        let transports = fake_transports("shared");
        let database_a = Database::open_memory();
        let identity_a = test_identity(&database_a);
        let epoch_keys_a = FakeEpochKeyStore::default();
        let phrase = begin_genesis(&database_a, &identity_a, &epoch_keys_a, &transports, false).await.unwrap();

        let database_b = Database::open_memory();
        let identity_b = test_identity(&database_b);
        let epoch_keys_b = FakeEpochKeyStore::default();
        join_with_recovery_phrase(&database_b, &identity_b, &epoch_keys_b, &phrase, &transports).await.unwrap();
        run_enrollment_sweep(&database_a, &identity_a, &epoch_keys_a, &transports).await.unwrap();

        let database_c = Database::open_memory();
        let identity_c = test_identity(&database_c);
        let epoch_keys_c = FakeEpochKeyStore::default();
        publish_enrollment_request(&database_c, &identity_c, &transports).await.unwrap();
        run_enrollment_sweep(&database_a, &identity_a, &epoch_keys_a, &transports).await.unwrap();
        run_enrollment_sweep(&database_b, &identity_b, &epoch_keys_b, &transports).await.unwrap();
        let request_id = database_a.pending_incoming_enrollment_requests().unwrap()[0].request_id.clone();

        let keys_a = local_keys_for(&database_a, &identity_a, &epoch_keys_a);
        approve_enrollment_request(&database_a, &identity_a, &keys_a, &request_id, &transports).await.unwrap();
        reject_enrollment_request(&database_b, &identity_b, &request_id, &transports).await.unwrap();
        run_enrollment_sweep(&database_c, &identity_c, &epoch_keys_c, &transports).await.unwrap();
        run_enrollment_sweep(&database_b, &identity_b, &epoch_keys_b, &transports).await.unwrap();

        assert!(matches!(database_c.enrollment_status().unwrap(), EnrollmentStatus::AwaitingConfirmation { .. }));
        assert!(database_b.pending_incoming_enrollment_requests().unwrap().is_empty());
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
        push_local_state(&database_c, &keys_c, &transports_c).await.unwrap();
        let keys_a = local_keys_for(&database_a, &identity_a, &epoch_keys_a);
        let outcome = pull_from_transports(&database_a, &keys_a, &transports_a).await.unwrap();
        assert_eq!(outcome.merged_states, 1);
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
            EnrollmentStatus::Enrolled { device_count, .. } => assert_eq!(device_count, 2),
            other => panic!("expected Enrolled, got {other:?}"),
        }

        // Prove it actually works end to end: B writes a snippet, pushes,
        // and A — who now trusts B — pulls and materializes it.
        database_b
            .record_replicated_write(EntityType::Snippet, "b-1", &fields(&["id", "name", "body", "createdAt"]), &snippet_payload("b-1", "From B"))
            .unwrap();
        let keys_b = local_keys_for(&database_b, &identity_b, &epoch_keys_b);
        push_local_state(&database_b, &keys_b, &transports).await.unwrap();

        let keys_a_after = local_keys_for(&database_a, &identity_a, &epoch_keys_a);
        let outcome = pull_from_transports(&database_a, &keys_a_after, &transports).await.unwrap();
        assert_eq!(outcome.merged_states, 1);
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
        push_local_state(&database_b, &keys_b_after, &transports).await.unwrap();
        let keys_a_after_revoke = local_keys_for(&database_a, &identity_a, &epoch_keys_a);
        let outcome = pull_from_transports(&database_a, &keys_a_after_revoke, &transports).await.unwrap();
        assert_eq!(outcome.merged_states, 0);
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
            EnrollmentStatus::Enrolled { device_count, .. } => assert_eq!(device_count, 2),
            other => panic!("expected Enrolled, got {other:?}"),
        }

        // A's sweep discovers C's recovery-signed self-announcement and
        // trusts it without any staged confirmation step.
        run_enrollment_sweep(&database_a, &identity_a, &epoch_keys_a, &transports).await.unwrap();
        let roster = database_a.known_device_roster().unwrap();
        assert_eq!(roster.len(), 2);
        assert!(roster.iter().any(|(id, _)| *id == identity_c.device_id));
    }

    /// One simulated device with its own database, identity, and keychain.
    struct Member {
        database: Database,
        identity: DeviceIdentity,
        epoch_keys: FakeEpochKeyStore,
    }

    impl Member {
        fn new() -> Self {
            let database = Database::open_memory();
            let identity = test_identity(&database);
            Self { database, identity, epoch_keys: FakeEpochKeyStore::default() }
        }

        fn keys(&self) -> LocalKeys {
            local_keys_for(&self.database, &self.identity, &self.epoch_keys)
        }

        fn active_epoch(&self) -> u32 {
            self.database
                .connection()
                .unwrap()
                .query_row("SELECT active_epoch FROM sync_spaces WHERE id=?1", params![SPACE_ID], |row| row.get(0))
                .unwrap()
        }

        fn write_snippet(&self, id: &str) {
            self.database
                .record_replicated_write(EntityType::Snippet, id, &fields(&["id", "name", "body", "createdAt"]), &snippet_payload(id, id))
                .unwrap();
        }

        async fn push(&self, transports: &[Arc<dyn SyncTransport>]) {
            push_local_state(&self.database, &self.keys(), transports).await.unwrap();
        }

        async fn pull(&self, transports: &[Arc<dyn SyncTransport>]) -> crate::replicated_sync::PullOutcome {
            pull_from_transports(&self.database, &self.keys(), transports).await.unwrap()
        }

        async fn sweep(&self, transports: &[Arc<dyn SyncTransport>]) {
            run_enrollment_sweep(&self.database, &self.identity, &self.epoch_keys, transports).await.unwrap();
        }

        async fn rotate(&self, transports: &[Arc<dyn SyncTransport>]) {
            rotate_epoch(&self.database, &self.identity, &self.keys(), &self.epoch_keys, transports, None).await.unwrap();
        }

        fn snippet(&self, id: &str) -> Option<String> {
            self.database
                .connection()
                .unwrap()
                .query_row("SELECT name FROM snippets WHERE id=?1", params![id], |row| row.get(0))
                .optional()
                .unwrap()
        }
    }

    /// A founding device that wrote under epoch 0, rotated twice, and wrote
    /// under epoch 2.
    async fn founder_with_history_across_rotations(transports: &[Arc<dyn SyncTransport>]) -> Member {
        let a = Member::new();
        begin_genesis(&a.database, &a.identity, &a.epoch_keys, transports, false).await.unwrap();
        a.write_snippet("epoch-0");
        a.push(transports).await;
        a.rotate(transports).await;
        a.rotate(transports).await;
        assert_eq!(a.active_epoch(), 2);
        a.write_snippet("epoch-2");
        a.push(transports).await;
        a
    }

    #[tokio::test]
    async fn a_device_approved_after_rotations_reads_history_from_every_epoch() {
        let transports = fake_transports("shared");
        let a = founder_with_history_across_rotations(&transports).await;

        let b = Member::new();
        publish_enrollment_request(&b.database, &b.identity, &transports).await.unwrap();
        a.sweep(&transports).await;
        let pending = a.database.pending_incoming_enrollment_requests().unwrap();
        approve_enrollment_request(&a.database, &a.identity, &a.keys(), &pending[0].request_id, &transports).await.unwrap();
        b.sweep(&transports).await;
        let request_id: String = b
            .database
            .connection()
            .unwrap()
            .query_row("SELECT request_id FROM replicated_sync_enrollment_requests WHERE direction='outgoing'", [], |row| row.get(0))
            .unwrap();
        confirm_and_import_grant(&b.database, &b.identity, &b.epoch_keys, &request_id).await.unwrap();

        assert_eq!(b.active_epoch(), 2);
        for epoch in 0..=2 {
            assert_eq!(b.epoch_keys.get(epoch), a.epoch_keys.get(epoch), "epoch {epoch}");
        }
        let outcome = b.pull(&transports).await;
        assert_eq!(outcome.failed_transports, 0);
        assert_eq!(outcome.merged_states, 1);
        assert_eq!(b.snippet("epoch-0").as_deref(), Some("epoch-0"));
        assert_eq!(b.snippet("epoch-2").as_deref(), Some("epoch-2"));
    }

    #[tokio::test]
    async fn a_grant_whose_earlier_keys_are_not_sealed_to_this_device_is_refused() {
        let transports = fake_transports("shared");
        let a = founder_with_history_across_rotations(&transports).await;
        let b = Member::new();
        publish_enrollment_request(&b.database, &b.identity, &transports).await.unwrap();
        a.sweep(&transports).await;
        let pending = a.database.pending_incoming_enrollment_requests().unwrap();
        approve_enrollment_request(&a.database, &a.identity, &a.keys(), &pending[0].request_id, &transports).await.unwrap();
        b.sweep(&transports).await;

        // Swap one earlier key for a box sealed to someone else, re-signed
        // by the approver so only the sealing is wrong.
        let (request_id, grant_cbor): (String, Vec<u8>) = b
            .database
            .connection()
            .unwrap()
            .query_row(
                "SELECT request_id, pending_grant_cbor FROM replicated_sync_enrollment_requests WHERE direction='outgoing'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let mut grant = decode_signed_enrollment_grant(&grant_cbor).unwrap().grant;
        grant.earlier_epoch_keys[0].sealed_key = ByteBuf::from(seal_to_x25519(&[9u8; 32], &[1u8; 32]));
        let resigned = encode_signed_enrollment_grant(&sign_enrollment_grant(&a.identity.signing_key, grant).unwrap()).unwrap();
        b.database
            .connection()
            .unwrap()
            .execute("UPDATE replicated_sync_enrollment_requests SET pending_grant_cbor=?1 WHERE request_id=?2", params![resigned, request_id])
            .unwrap();

        assert!(confirm_and_import_grant(&b.database, &b.identity, &b.epoch_keys, &request_id).await.is_err());
        assert!(b.epoch_keys.get(2).is_none());
        assert!(!matches!(b.database.enrollment_status().unwrap(), EnrollmentStatus::Enrolled { .. }));
    }

    #[tokio::test]
    async fn a_recovery_phrase_join_after_rotations_adopts_the_newest_epoch_and_reads_every_one() {
        let transports = fake_transports("shared");
        let a = Member::new();
        let phrase = begin_genesis(&a.database, &a.identity, &a.epoch_keys, &transports, false).await.unwrap();
        a.write_snippet("epoch-0");
        a.push(&transports).await;
        a.rotate(&transports).await;
        a.rotate(&transports).await;
        a.write_snippet("epoch-2");
        a.push(&transports).await;

        let c = Member::new();
        join_with_recovery_phrase(&c.database, &c.identity, &c.epoch_keys, &phrase, &transports).await.unwrap();
        assert_eq!(c.active_epoch(), 2);
        for epoch in 0..=2 {
            assert_eq!(c.epoch_keys.get(epoch), a.epoch_keys.get(epoch), "epoch {epoch}");
        }
        let outcome = c.pull(&transports).await;
        assert_eq!(outcome.failed_transports, 0);
        assert_eq!(c.snippet("epoch-0").as_deref(), Some("epoch-0"));
        assert_eq!(c.snippet("epoch-2").as_deref(), Some("epoch-2"));
    }

    #[tokio::test]
    async fn a_recovery_join_waits_until_every_epochs_rotation_has_arrived() {
        let transports = fake_transports("shared");
        let a = Member::new();
        let phrase = begin_genesis(&a.database, &a.identity, &a.epoch_keys, &transports, false).await.unwrap();
        a.rotate(&transports).await;
        a.rotate(&transports).await;
        a.write_snippet("epoch-2");
        a.push(&transports).await;

        let mut rotations: std::collections::BTreeMap<u32, (String, Vec<u8>)> = std::collections::BTreeMap::new();
        let page = transports[0].scan(None).await.unwrap().unwrap();
        for locator in page.objects {
            let bytes = transports[0].get_object(&locator.cid).await.unwrap();
            if let Ok(signed) = decode_signed_key_rotation(&bytes) {
                rotations.insert(signed.rotation.key_epoch, (locator.cid.0, bytes));
            }
        }
        assert_eq!(rotations.keys().copied().collect::<Vec<_>>(), vec![0, 1, 2]);
        let hide = |epoch: u32| {
            let transport = transports[0].clone();
            let cid = rotations[&epoch].0.clone();
            async move { transport.delete_object(&TransportCid(cid)).await.unwrap() }
        };
        let restore = |epoch: u32| {
            let transport = transports[0].clone();
            let (cid, bytes) = rotations[&epoch].clone();
            async move { transport.put_object(&TransportCid(cid), &bytes).await.unwrap() }
        };

        // A gap in the middle.
        hide(1).await;
        let c = Member::new();
        let refused = join_with_recovery_phrase(&c.database, &c.identity, &c.epoch_keys, &phrase, &transports).await;
        assert_eq!(refused.unwrap_err(), RECOVERY_INCOMPLETE);
        restore(1).await;

        // The newest rotation missing, while A's head already says epoch 2.
        hide(2).await;
        let refused = join_with_recovery_phrase(&c.database, &c.identity, &c.epoch_keys, &phrase, &transports).await;
        assert_eq!(refused.unwrap_err(), RECOVERY_INCOMPLETE);
        assert!(c.epoch_keys.get(0).is_none(), "a refused join stores no keys");
        restore(2).await;

        join_with_recovery_phrase(&c.database, &c.identity, &c.epoch_keys, &phrase, &transports).await.unwrap();
        assert_eq!(c.active_epoch(), 2);
        c.pull(&transports).await;
        assert_eq!(c.snippet("epoch-2").as_deref(), Some("epoch-2"));
    }

    #[tokio::test]
    async fn a_member_offline_across_a_rotation_catches_up_on_events_from_before_it() {
        let transports = fake_transports("shared");
        let a = Member::new();
        let phrase = begin_genesis(&a.database, &a.identity, &a.epoch_keys, &transports, false).await.unwrap();
        let c = Member::new();
        join_with_recovery_phrase(&c.database, &c.identity, &c.epoch_keys, &phrase, &transports).await.unwrap();
        a.sweep(&transports).await;

        // While C is away, A writes, rotates, and writes again.
        a.write_snippet("before-rotation");
        a.push(&transports).await;
        a.rotate(&transports).await;
        a.write_snippet("after-rotation");
        a.push(&transports).await;

        c.sweep(&transports).await;
        assert_eq!(c.active_epoch(), 1);
        let outcome = c.pull(&transports).await;
        assert_eq!(outcome.failed_transports, 0);
        assert_eq!(outcome.merged_states, 1);
        assert_eq!(c.snippet("before-rotation").as_deref(), Some("before-rotation"));
        assert_eq!(c.snippet("after-rotation").as_deref(), Some("after-rotation"));
    }

    #[tokio::test]
    async fn a_member_a_rotation_was_never_sealed_to_catches_up_from_a_peer() {
        let transports = fake_transports("shared");
        let a = Member::new();
        let phrase = begin_genesis(&a.database, &a.identity, &a.epoch_keys, &transports, false).await.unwrap();
        let c = Member::new();
        join_with_recovery_phrase(&c.database, &c.identity, &c.epoch_keys, &phrase, &transports).await.unwrap();

        // A rotates before hearing that C joined, so the new key isn't
        // sealed to C, and writes under it.
        a.rotate(&transports).await;
        a.write_snippet("after-rotation");
        a.push(&transports).await;
        c.sweep(&transports).await;
        assert_eq!(c.active_epoch(), 0);
        assert!(c.epoch_keys.get(1).is_none());
        c.push(&transports).await;
        assert_eq!(c.pull(&transports).await.failed_transports, 1, "C can't open A's snapshot yet");

        // A learns of C, sees C's head still on epoch 0, and shares its keys.
        a.sweep(&transports).await;
        a.pull(&transports).await;
        assert_eq!(share_keys_with_lagging_peers(&a.database, &a.keys(), &transports).await.unwrap(), 1);
        assert_eq!(share_keys_with_lagging_peers(&a.database, &a.keys(), &transports).await.unwrap(), 0, "once per epoch");

        c.sweep(&transports).await;
        assert_eq!(c.active_epoch(), 1);
        assert_eq!(c.epoch_keys.get(1), a.epoch_keys.get(1));
        let outcome = c.pull(&transports).await;
        assert_eq!((outcome.merged_states, outcome.failed_transports), (1, 0));
        assert_eq!(c.snippet("after-rotation").as_deref(), Some("after-rotation"));
    }

    #[tokio::test]
    async fn a_key_share_from_a_revoked_or_unknown_member_is_not_applied() {
        let transports = fake_transports("shared");
        let a = Member::new();
        let phrase = begin_genesis(&a.database, &a.identity, &a.epoch_keys, &transports, false).await.unwrap();
        let c = Member::new();
        join_with_recovery_phrase(&c.database, &c.identity, &c.epoch_keys, &phrase, &transports).await.unwrap();
        a.sweep(&transports).await;
        a.rotate(&transports).await;
        // A share signed by a stranger, sealed to C, carrying a made-up key.
        let stranger = threestrands_sync_envelope::SigningKey::generate(&mut rand::rngs::OsRng);
        let c_x25519 = x25519_public_bytes(&c.identity.x25519_secret);
        let (recovery_ed25519, recovery_x25519) = a.database.recovery_public_keys().unwrap().unwrap();
        let forged = sign_enrollment_grant(
            &stranger,
            EnrollmentGrant {
                request_id: RequestId::from_bytes(random_id()),
                approver_device_id: threestrands_sync_envelope::DeviceId::from_bytes([0x5a; 16]),
                signed_by_recovery: false,
                key_epoch: 7,
                sealed_epoch_key: ByteBuf::from(seal_to_x25519(&c_x25519, &[0x42; 32])),
                roster: vec![],
                recovery_ed25519_public: ByteBuf::from(recovery_ed25519.to_vec()),
                recovery_x25519_public: ByteBuf::from(recovery_x25519.to_vec()),
                created_at_ms: now_ms(),
                earlier_epoch_keys: vec![],
            },
        )
        .unwrap();
        publish_to_all(&transports, &encode_signed_enrollment_grant(&forged).unwrap()).await;
        c.sweep(&transports).await;
        assert!(c.epoch_keys.get(7).is_none());
        assert_eq!(c.active_epoch(), 1, "the real rotation still applies");

        // A share from a member C has since revoked is ignored too.
        let shared_before_revocation = {
            let keys = a.keys();
            let recipient = c_x25519;
            sign_enrollment_grant(
                &a.identity.signing_key,
                EnrollmentGrant {
                    request_id: RequestId::from_bytes(random_id()),
                    approver_device_id: a.identity.device_id,
                    signed_by_recovery: false,
                    key_epoch: 9,
                    sealed_epoch_key: ByteBuf::from(seal_to_x25519(&recipient, &[0x43; 32])),
                    roster: vec![],
                    recovery_ed25519_public: ByteBuf::from(recovery_ed25519.to_vec()),
                    recovery_x25519_public: ByteBuf::from(recovery_x25519.to_vec()),
                    created_at_ms: now_ms(),
                    earlier_epoch_keys: seal_earlier_epoch_keys(&keys, &recipient).unwrap(),
                },
            )
            .unwrap()
        };
        c.database.revoke_device(a.identity.device_id.as_bytes()).unwrap();
        publish_to_all(&transports, &encode_signed_enrollment_grant(&shared_before_revocation).unwrap()).await;
        c.sweep(&transports).await;
        assert!(c.epoch_keys.get(9).is_none());
    }

    #[tokio::test]
    async fn meeting_an_older_rotation_after_a_newer_one_never_moves_the_active_epoch_back() {
        let transports = fake_transports("shared");
        let a = Member::new();
        let phrase = begin_genesis(&a.database, &a.identity, &a.epoch_keys, &transports, false).await.unwrap();
        let c = Member::new();
        join_with_recovery_phrase(&c.database, &c.identity, &c.epoch_keys, &phrase, &transports).await.unwrap();
        a.sweep(&transports).await;
        a.rotate(&transports).await;
        a.rotate(&transports).await;

        let rotation_cids: Vec<String> = {
            let connection = a.database.connection().unwrap();
            let mut statement = connection.prepare("SELECT cid FROM sync_control_objects_seen WHERE object_kind='key_rotation'").unwrap();
            let rows = statement.query_map([], |row| row.get(0)).unwrap().collect::<Result<Vec<String>, _>>().unwrap();
            rows
        };
        let mut rotations = Vec::new();
        for cid in rotation_cids {
            let bytes = transports[0].get_object(&TransportCid(cid.clone())).await.unwrap();
            rotations.push((decode_signed_key_rotation(&bytes).unwrap(), cid));
        }
        rotations.sort_by_key(|(signed, _)| std::cmp::Reverse(signed.rotation.key_epoch));
        for (signed, cid) in rotations {
            assert!(matches!(
                apply_incoming_rotation(&c.database, &c.identity, &c.epoch_keys, signed, &cid).unwrap(),
                RotationOutcome::Done
            ));
        }
        assert_eq!(c.active_epoch(), 2);
        assert_eq!(c.epoch_keys.get(1), a.epoch_keys.get(1));
        assert_eq!(c.epoch_keys.get(2), a.epoch_keys.get(2));
    }

    /// A group as an earlier build left it on `transports`: a genesis
    /// rotation with no protocol marker. Returns its recovery phrase.
    async fn legacy_group(transports: &[Arc<dyn SyncTransport>]) -> String {
        let a = Member::new();
        let phrase = begin_genesis(&a.database, &a.identity, &a.epoch_keys, transports, false).await.unwrap();
        for transport in transports {
            transport.delete_object(&TransportCid(protocol_marker_cid())).await.unwrap();
        }
        phrase
    }

    #[tokio::test]
    async fn genesis_stores_the_protocol_marker_where_every_connector_gets_it() {
        let transports: Vec<Arc<dyn SyncTransport>> = vec![Arc::new(FakeTransport::new("one")), Arc::new(FakeTransport::new("two"))];
        let a = Member::new();
        begin_genesis(&a.database, &a.identity, &a.epoch_keys, &transports, false).await.unwrap();
        for transport in &transports {
            assert_eq!(transport.get_object(&TransportCid(protocol_marker_cid())).await.unwrap(), PROTOCOL_MARKER);
        }
        assert_eq!(inspect_sync_space(&transports).await, SyncSpacePresence::Existing);

        // A connector added later receives it through repair.
        let later = fake_transports("later");
        a.database.enqueue_repair_deliveries(&[later[0].instance_id()]).unwrap();
        a.push(&later).await;
        assert_eq!(later[0].get_object(&TransportCid(protocol_marker_cid())).await.unwrap(), PROTOCOL_MARKER);
    }

    #[tokio::test]
    async fn a_group_from_an_earlier_build_is_reported_as_legacy_and_never_joined_or_replaced() {
        let transports = fake_transports("shared");
        let phrase = legacy_group(&transports).await;
        assert_eq!(inspect_sync_space(&transports).await, SyncSpacePresence::Legacy);
        let objects_before = transports[0].scan(None).await.unwrap().unwrap().objects.len();

        let b = Member::new();
        for allow_existing_space in [false, true] {
            let refused = begin_genesis(&b.database, &b.identity, &b.epoch_keys, &transports, allow_existing_space).await;
            assert_eq!(refused.unwrap_err(), LEGACY_SPACE_REFUSAL);
        }
        let refused = join_with_recovery_phrase(&b.database, &b.identity, &b.epoch_keys, &phrase, &transports).await;
        assert_eq!(refused.unwrap_err(), LEGACY_SPACE_REFUSAL);
        let refused = publish_enrollment_request(&b.database, &b.identity, &transports).await;
        assert_eq!(refused.unwrap_err(), LEGACY_SPACE_REFUSAL);
        // Nothing was set up or published on the way to refusing.
        assert_eq!(transports[0].scan(None).await.unwrap().unwrap().objects.len(), objects_before);
        assert!(b.epoch_keys.get(0).is_none());
        let recorded: i64 = b
            .database
            .connection()
            .unwrap()
            .query_row("SELECT (SELECT COUNT(*) FROM sync_epoch_history) + (SELECT COUNT(*) FROM replicated_sync_enrollment_requests)", [], |row| row.get(0))
            .unwrap();
        assert_eq!(recorded, 0);
        assert_eq!(inspect_sync_space(&transports).await, SyncSpacePresence::Legacy);
    }

    #[tokio::test]
    async fn an_s3_group_from_an_earlier_build_is_reported_as_legacy() {
        use crate::s3_transport::fake_server::FakeS3Server;
        let server = FakeS3Server::spawn().await;
        let (transports, _) = shared_s3_transports(&server).await;
        legacy_group(&transports).await;
        let engine = crate::replicated_sync::ReplicatedSync::new(Arc::new(Database::open_memory()));
        let found = engine.probe_s3(&server.config("group"), &FakeS3Server::credentials()).await.unwrap();
        assert_eq!(found.space_presence, Some(SyncSpacePresence::Legacy));
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
        let mut rotation = None;
        for locator in &page.objects {
            let bytes = genesis_transports[0].get_object(&locator.cid).await.unwrap();
            if let Ok(decoded) = decode_signed_key_rotation(&bytes) {
                rotation = Some(decoded);
            }
        }
        let mut tampered = rotation.expect("genesis publishes a rotation");
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
        assert!(matches!(database_b.enrollment_status().unwrap(), EnrollmentStatus::Enrolled { device_count: 1, .. }));
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
            .execute(
                "INSERT INTO sync_remote_states(device_id, state_sequence, merged_at) VALUES (?1, 2, '2026-09-21T10:00:00+00:00')",
                params![peer],
            )
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
        for table in ["sync_values", "sync_context", "sync_remote_states", "sync_local_state", "sync_objects"] {
            assert_eq!(count(&format!("SELECT COUNT(*) FROM {table}")), 0, "{table} should be empty");
        }
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

    /// Join codes end to end, over real sync folders (and S3 through the
    /// fake server), with an explicit clock.
    mod join_code_flows {
        use super::*;
        use crate::replicated_sync::build_configured_transports;
        use threestrands_sync_envelope::{
            decode_join_code, encode_join_code, encode_signed_invitation, encode_signed_invitation_redemption,
            invite_ed25519_signing_key, invite_x25519_secret, sign_invitation, sign_invitation_redemption,
            Invitation, InvitationRedemption, SigningKey,
        };

        struct SharedFolder {
            path: std::path::PathBuf,
        }

        impl SharedFolder {
            fn new() -> Self {
                let path = std::env::temp_dir().join(format!("threestrands-join-code-{}", uuid::Uuid::new_v4()));
                std::fs::create_dir_all(&path).unwrap();
                Self { path }
            }

            fn choice(&self, index: usize) -> JoinFolderChoice {
                JoinFolderChoice { connector_index: index, path: self.path.to_string_lossy().into_owned() }
            }
        }

        impl Drop for SharedFolder {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.path);
            }
        }

        struct Device {
            database: Database,
            identity: DeviceIdentity,
            epoch_keys: FakeEpochKeyStore,
        }

        impl Device {
            fn new() -> Self {
                let database = Database::open_memory();
                let identity = test_identity(&database);
                Self { database, identity, epoch_keys: FakeEpochKeyStore::default() }
            }

            fn keys(&self) -> LocalKeys {
                local_keys_for(&self.database, &self.identity, &self.epoch_keys)
            }

            async fn transports(&self) -> Vec<Arc<dyn SyncTransport>> {
                build_configured_transports(&self.database).await
            }

            async fn sweep(&self) {
                run_enrollment_sweep(&self.database, &self.identity, &self.epoch_keys, &self.transports().await).await.unwrap();
            }

            async fn process(&self, now: i64) -> bool {
                process_join_codes(&self.database, &self.identity, &self.keys(), &self.epoch_keys, &self.transports().await, now)
                    .await
                    .unwrap()
            }

            fn active_epoch(&self) -> u32 {
                self.database
                    .connection()
                    .unwrap()
                    .query_row("SELECT active_epoch FROM sync_spaces WHERE id=?1", params![SPACE_ID], |row| row.get(0))
                    .unwrap()
            }

            fn trusts(&self, other: &Device) -> bool {
                self.database.known_device_roster().unwrap().iter().any(|(id, _)| *id == other.identity.device_id)
            }

            fn awaiting_admission(&self) -> Option<String> {
                match self.database.enrollment_status().unwrap() {
                    EnrollmentStatus::Enrolled { awaiting_admission_from, .. } => awaiting_admission_from,
                    other => panic!("expected Enrolled, got {other:?}"),
                }
            }

            fn device_id_hex(&self) -> String {
                encode_id(self.identity.device_id.as_bytes())
            }
        }

        fn choice(instance_id: &str, include_credentials: bool) -> JoinCodeConnectorChoice {
            JoinCodeConnectorChoice { instance_id: instance_id.to_string(), include_credentials }
        }

        /// A (genesis) and C (joined by recovery phrase) sharing `folder`.
        async fn group(folder: &SharedFolder) -> (Device, Device) {
            let a = Device::new();
            a.database.add_folder_transport("folder-a", &folder.path).unwrap();
            let phrase = begin_genesis(&a.database, &a.identity, &a.epoch_keys, &a.transports().await, false).await.unwrap();
            let c = Device::new();
            c.database.add_folder_transport("folder-c", &folder.path).unwrap();
            join_with_recovery_phrase(&c.database, &c.identity, &c.epoch_keys, &phrase, &c.transports().await).await.unwrap();
            a.sweep().await;
            (a, c)
        }

        async fn create(a: &Device, now: i64) -> String {
            create_join_code(&a.database, &a.identity, &a.keys(), &a.transports().await, &[choice("folder-a", true)], 24, now)
                .await
                .unwrap()
        }

        async fn join(device: &Device, code: &str, folder: &SharedFolder, now: i64) -> Result<(), String> {
            join_with_code(&device.database, &device.identity, &device.epoch_keys, code, &[folder.choice(0)], vec![], now).await
        }

        fn snippet_name(database: &Database, id: &str) -> Option<String> {
            database.connection().unwrap().query_row("SELECT name FROM snippets WHERE id=?1", params![id], |row| row.get(0)).optional().unwrap()
        }

        #[tokio::test]
        async fn a_join_code_adds_a_device_that_every_member_then_trusts() {
            let folder = SharedFolder::new();
            let (a, c) = group(&folder).await;
            a.database
                .record_replicated_write(EntityType::Snippet, "a-1", &fields(&["id", "name", "body", "createdAt"]), &snippet_payload("a-1", "Before B"))
                .unwrap();
            push_local_state(&a.database, &a.keys(), &a.transports().await).await.unwrap();

            let now = now_ms();
            let code = create(&a, now).await;
            assert!(code.starts_with("TSJOIN1-"));
            let epoch_before = a.active_epoch();

            let b = Device::new();
            join(&b, &code, &folder, now + 1_000).await.unwrap();
            // B's connector is saved, it can read at once, and it's waiting.
            let rows = b.database.configured_transports().unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].kind, "folder");
            assert!(b.awaiting_admission().is_some());
            pull_from_transports(&b.database, &b.keys(), &b.transports().await).await.unwrap();
            assert_eq!(snippet_name(&b.database, "a-1").as_deref(), Some("Before B"));
            assert!(!c.trusts(&b));

            // A admits B by rotating; C and B apply that rotation.
            a.sweep().await;
            assert!(a.process(now + 2_000).await);
            assert_eq!(a.active_epoch(), epoch_before + 1);
            assert!(a.trusts(&b));
            c.sweep().await;
            b.sweep().await;
            assert!(c.trusts(&b));
            assert_eq!(b.awaiting_admission(), None);
            assert_eq!(b.active_epoch(), epoch_before + 1);

            // Everyone can see how B joined.
            let notices = c.database.join_code_notices().unwrap();
            assert_eq!(notices.len(), 1);
            assert_eq!(notices[0].kind, "joined");
            assert_eq!(notices[0].device_id, b.device_id_hex());
            assert_eq!(notices[0].inviter_device_id.as_deref(), Some(a.device_id_hex().as_str()));
            c.database.dismiss_join_code_notice(&notices[0].redemption_cid).unwrap();
            assert!(c.database.join_code_notices().unwrap().is_empty());
            let roster = c.database.device_roster().unwrap();
            assert!(roster.iter().find(|entry| entry.device_id == b.device_id_hex()).unwrap().joined_with_join_code);
            assert!(!roster.iter().find(|entry| entry.device_id == a.device_id_hex()).unwrap().joined_with_join_code);
            assert!(b.database.device_roster().unwrap().iter().find(|entry| entry.is_self).unwrap().joined_with_join_code);
            let codes = a.database.outstanding_join_codes().unwrap();
            assert_eq!(codes[0].status, "redeemed");
            assert_eq!(codes[0].redeemed_by_device_id.as_deref(), Some(b.device_id_hex().as_str()));

            // The redeemed invitation stays for members that sync later,
            // then goes once the code would have expired — without another
            // rotation.
            let invitation_cid = TransportCid(decode_join_code(&code).unwrap().invitation_cid);
            assert!(a.transports().await[0].get_object(&invitation_cid).await.is_ok());
            let epoch = a.active_epoch();
            assert!(!a.process(now + 25 * 3_600_000).await);
            assert_eq!(a.active_epoch(), epoch);
            assert!(a.transports().await[0].get_object(&invitation_cid).await.is_err());

            // B's writes now reach A and C.
            b.database
                .record_replicated_write(EntityType::Snippet, "b-1", &fields(&["id", "name", "body", "createdAt"]), &snippet_payload("b-1", "From B"))
                .unwrap();
            push_local_state(&b.database, &b.keys(), &b.transports().await).await.unwrap();
            pull_from_transports(&a.database, &a.keys(), &a.transports().await).await.unwrap();
            pull_from_transports(&c.database, &c.keys(), &c.transports().await).await.unwrap();
            assert_eq!(snippet_name(&a.database, "b-1").as_deref(), Some("From B"));
            assert_eq!(snippet_name(&c.database, "b-1").as_deref(), Some("From B"));
        }

        #[tokio::test]
        async fn a_device_joining_by_code_after_a_rotation_reads_history_from_every_epoch() {
            let folder = SharedFolder::new();
            let (a, _c) = group(&folder).await;
            a.database
                .record_replicated_write(EntityType::Snippet, "old", &fields(&["id", "name", "body", "createdAt"]), &snippet_payload("old", "Old"))
                .unwrap();
            push_local_state(&a.database, &a.keys(), &a.transports().await).await.unwrap();

            // A first join code admits B, which rotates the epoch.
            let now = now_ms();
            let b = Device::new();
            join(&b, &create(&a, now).await, &folder, now + 1_000).await.unwrap();
            a.sweep().await;
            assert!(a.process(now + 2_000).await);
            let rotated = a.active_epoch();
            a.database
                .record_replicated_write(EntityType::Snippet, "new", &fields(&["id", "name", "body", "createdAt"]), &snippet_payload("new", "New"))
                .unwrap();
            push_local_state(&a.database, &a.keys(), &a.transports().await).await.unwrap();

            // D joins with a second code, under the rotated epoch.
            let d = Device::new();
            join(&d, &create(&a, now + 3_000).await, &folder, now + 4_000).await.unwrap();
            assert_eq!(d.active_epoch(), rotated);
            let outcome = pull_from_transports(&d.database, &d.keys(), &d.transports().await).await.unwrap();
            assert_eq!(outcome.failed_transports, 0);
            assert_eq!(snippet_name(&d.database, "old").as_deref(), Some("Old"));
            assert_eq!(snippet_name(&d.database, "new").as_deref(), Some("New"));
        }

        #[tokio::test]
        async fn a_join_code_for_a_group_from_an_earlier_build_is_refused() {
            let folder = SharedFolder::new();
            let (a, _c) = group(&folder).await;
            let now = now_ms();
            let code = create(&a, now).await;
            a.transports().await[0].delete_object(&TransportCid(protocol_marker_cid())).await.unwrap();
            assert_eq!(inspect_sync_space(&a.transports().await).await, SyncSpacePresence::Legacy);

            let d = Device::new();
            assert_eq!(join(&d, &code, &folder, now + 1_000).await.unwrap_err(), LEGACY_SPACE_REFUSAL);
            assert!(d.database.configured_transports().unwrap().is_empty());
            assert!(matches!(d.database.enrollment_status().unwrap(), EnrollmentStatus::NotStarted));
        }

        /// Objects are scanned in content-address order, so a redemption can
        /// come before the invitation it names. One sweep must still record
        /// it, not leave it for the next cycle.
        #[tokio::test]
        async fn one_sweep_records_a_redemption_scanned_before_its_invitation() {
            let folder = SharedFolder::new();
            let (a, c) = group(&folder).await;
            let now = now_ms();
            let code = create(&a, now).await;
            let b = Device::new();
            join(&b, &code, &folder, now + 1_000).await.unwrap();
            let invitation_cid = decode_join_code(&code).unwrap().invitation_cid;
            let redemption_cid: String = b
                .database
                .connection()
                .unwrap()
                .query_row("SELECT redemption_cid FROM replicated_sync_invitations WHERE direction='incoming'", [], |row| row.get(0))
                .unwrap();

            // Just these two objects, invitation first, scanned in reverse.
            let shared = a.transports().await;
            let reordered = threestrands_sync_transport::fake::FakeTransport::new("reordered");
            for cid in [&invitation_cid, &redemption_cid] {
                let bytes = shared[0].get_object(&TransportCid(cid.clone())).await.unwrap();
                reordered.put_object(&TransportCid(cid.clone()), &bytes).await.unwrap();
            }
            reordered.enable_scan_reordering();
            let transports: Vec<Arc<dyn SyncTransport>> = vec![Arc::new(reordered)];

            run_enrollment_sweep(&c.database, &c.identity, &c.epoch_keys, &transports).await.unwrap();
            let state: String = c
                .database
                .connection()
                .unwrap()
                .query_row(
                    "SELECT state FROM replicated_sync_invitation_redemptions WHERE redemption_cid=?1",
                    params![redemption_cid],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(state, "observed");
            assert!(c.database.seen_control_object(&redemption_cid).unwrap());
        }

        #[tokio::test]
        async fn a_code_admits_only_its_first_redemption() {
            let folder = SharedFolder::new();
            let (a, c) = group(&folder).await;
            let now = now_ms();
            let code = create(&a, now).await;
            let b = Device::new();
            let d = Device::new();
            join(&b, &code, &folder, now + 1_000).await.unwrap();
            join(&d, &code, &folder, now + 1_500).await.unwrap();

            a.sweep().await;
            assert!(a.process(now + 2_000).await);
            c.sweep().await;
            d.sweep().await;
            assert!(a.trusts(&b) && c.trusts(&b));
            assert!(!a.trusts(&d) && !c.trusts(&d));
            assert!(d.awaiting_admission().is_some());

            let codes = a.database.outstanding_join_codes().unwrap();
            assert_eq!(codes[0].rejected_attempts, 1);
            let notices = a.database.join_code_notices().unwrap();
            assert!(notices.iter().any(|notice| notice.kind == "rejectedAttempt" && notice.device_id == d.device_id_hex()));
            // D never shows as "joined" anywhere.
            assert!(!c.database.join_code_notices().unwrap().iter().any(|notice| notice.device_id == d.device_id_hex()));
        }

        #[tokio::test]
        async fn a_redemption_arriving_after_expiry_is_rejected_and_the_old_key_stops_working() {
            let folder = SharedFolder::new();
            let (a, _c) = group(&folder).await;
            let now = now_ms();
            let code = create(&a, now).await;
            let parsed = decode_join_code(&code).unwrap();
            let b = Device::new();
            join(&b, &code, &folder, now + 1_000).await.unwrap();

            a.sweep().await;
            let after_expiry = now + 25 * 3_600_000;
            assert!(a.process(after_expiry).await);
            assert!(!a.trusts(&b));
            let codes = a.database.outstanding_join_codes().unwrap();
            assert_eq!(codes[0].status, "expired");
            assert_eq!(codes[0].rejected_attempts, 1);

            // The invitation object is gone, and the new epoch's stanzas
            // don't open with the invite secret.
            let transports = a.transports().await;
            assert!(transports[0].get_object(&TransportCid(parsed.invitation_cid.clone())).await.is_err());
            let invite_secret = invite_x25519_secret(&parsed.invite_secret()).to_bytes();
            let mut rotations_checked = 0;
            let page = transports[0].scan(None).await.unwrap().unwrap();
            for locator in page.objects {
                let Ok(bytes) = transports[0].get_object(&locator.cid).await else { continue };
                let Ok(rotation) = decode_signed_key_rotation(&bytes) else { continue };
                if rotation.rotation.key_epoch == a.active_epoch() {
                    rotations_checked += 1;
                    assert!(rotation.rotation.sealed_stanzas.iter().all(|stanza| try_open_sealed_box(&invite_secret, stanza).is_none()));
                }
            }
            assert_eq!(rotations_checked, 1);
        }

        #[tokio::test]
        async fn an_unused_code_expires_with_a_rotation_and_its_invitation_is_deleted() {
            let folder = SharedFolder::new();
            let (a, _c) = group(&folder).await;
            let now = now_ms();
            let code = create(&a, now).await;
            let cid = decode_join_code(&code).unwrap().invitation_cid;
            assert!(!a.process(now + 3_600_000).await, "still open within its lifetime");
            let epoch = a.active_epoch();
            assert!(a.process(now + 25 * 3_600_000).await);
            assert_eq!(a.active_epoch(), epoch + 1);
            assert_eq!(a.database.outstanding_join_codes().unwrap()[0].status, "expired");
            assert!(a.transports().await[0].get_object(&TransportCid(cid)).await.is_err());
            assert!(!a.process(now + 26 * 3_600_000).await, "an expired code rotates only once");
        }

        #[tokio::test]
        async fn cancelling_a_code_rotates_and_refuses_later_joins() {
            let folder = SharedFolder::new();
            let (a, _c) = group(&folder).await;
            let now = now_ms();
            let code = create(&a, now).await;
            let cid = decode_join_code(&code).unwrap().invitation_cid;
            let epoch = a.active_epoch();

            cancel_join_code(&a.database, &a.identity, &a.keys(), &a.epoch_keys, &a.transports().await, &cid).await.unwrap();
            assert_eq!(a.active_epoch(), epoch + 1);
            assert_eq!(a.database.outstanding_join_codes().unwrap()[0].status, "cancelled");
            assert!(cancel_join_code(&a.database, &a.identity, &a.keys(), &a.epoch_keys, &a.transports().await, &cid).await.is_err());

            let b = Device::new();
            let error = join(&b, &code, &folder, now + 1_000).await.unwrap_err();
            assert!(error.contains("Couldn't find"), "{error}");
            assert!(b.database.configured_transports().unwrap().is_empty());
            assert!(matches!(b.database.enrollment_status().unwrap(), EnrollmentStatus::NotStarted));
        }

        #[tokio::test]
        async fn a_redemption_that_raced_a_cancellation_is_rejected() {
            let folder = SharedFolder::new();
            let (a, _c) = group(&folder).await;
            let now = now_ms();
            let code = create(&a, now).await;
            let b = Device::new();
            join(&b, &code, &folder, now + 1_000).await.unwrap();
            let cid = decode_join_code(&code).unwrap().invitation_cid;
            cancel_join_code(&a.database, &a.identity, &a.keys(), &a.epoch_keys, &a.transports().await, &cid).await.unwrap();
            a.sweep().await;
            assert!(!a.process(now + 2_000).await);
            assert!(!a.trusts(&b));
            assert_eq!(a.database.outstanding_join_codes().unwrap()[0].rejected_attempts, 1);
        }

        #[tokio::test]
        async fn a_code_pointing_at_the_wrong_object_or_with_the_wrong_secret_saves_nothing() {
            let folder = SharedFolder::new();
            let (a, _c) = group(&folder).await;
            let now = now_ms();
            let code = decode_join_code(&create(&a, now).await).unwrap();

            let mut missing = code.clone();
            missing.invitation_cid = compute_cid(b"no such invitation");
            let b = Device::new();
            let error = join(&b, &encode_join_code(&missing).unwrap(), &folder, now).await.unwrap_err();
            assert!(error.contains("Couldn't find"), "{error}");

            let mut wrong_secret = code.clone();
            wrong_secret.invite_secret = ByteBuf::from(vec![9u8; 32]);
            let error = join(&b, &encode_join_code(&wrong_secret).unwrap(), &folder, now).await.unwrap_err();
            assert!(error.contains("damaged"), "{error}");

            assert!(b.database.configured_transports().unwrap().is_empty());
            assert!(matches!(b.database.enrollment_status().unwrap(), EnrollmentStatus::NotStarted));
            assert!(verify_fetched_invitation(b"not the invitation", &code).is_err());
        }

        #[tokio::test]
        async fn a_planted_look_alike_invitation_is_never_used() {
            let folder = SharedFolder::new();
            let (a, _c) = group(&folder).await;
            let now = now_ms();
            let code = decode_join_code(&create(&a, now).await).unwrap();
            let secret = code.invite_secret();

            // Someone with write access to the folder publishes an
            // invitation with the same (public) invite keys but their own
            // roster and epoch key.
            let mallory_key = SigningKey::generate(&mut rand::rngs::OsRng);
            let mallory_id = EnvelopeDeviceId::from_bytes([0xee; 16]);
            let invite_x25519_public = X25519PublicKey::from(&invite_x25519_secret(&secret)).to_bytes();
            let planted = sign_invitation(
                &mallory_key,
                Invitation {
                    inviter_device_id: mallory_id,
                    invite_ed25519_public: ByteBuf::from(invite_ed25519_signing_key(&secret).verifying_key().to_bytes().to_vec()),
                    invite_x25519_public: ByteBuf::from(invite_x25519_public.to_vec()),
                    key_epoch: 0,
                    sealed_epoch_key: ByteBuf::from(seal_to_x25519(&invite_x25519_public, &[0x42; 32])),
                    roster: vec![RosterEntry {
                        device_id: mallory_id,
                        ed25519_public: ByteBuf::from(mallory_key.verifying_key().to_bytes().to_vec()),
                        x25519_public: ByteBuf::from(vec![1u8; 32]),
                        status: "active".to_string(),
                    }],
                    recovery_ed25519_public: ByteBuf::from(vec![2u8; 32]),
                    recovery_x25519_public: ByteBuf::from(vec![3u8; 32]),
                    created_at_ms: now,
                    expires_at_ms: now + 3_600_000,
                    earlier_epoch_keys: vec![],
                },
            )
            .unwrap();
            let planted_bytes = encode_signed_invitation(&planted).unwrap();
            publish_to_all(&a.transports().await, &planted_bytes).await;
            assert!(verify_fetched_invitation(&planted_bytes, &code).is_err());

            let b = Device::new();
            join(&b, &encode_join_code(&code).unwrap(), &folder, now).await.unwrap();
            assert!(b.trusts(&a));
            assert!(!b.database.known_device_roster().unwrap().iter().any(|(id, _)| *id == mallory_id));
        }

        #[tokio::test]
        async fn a_redemption_without_the_code_or_reusing_a_device_id_is_never_admitted() {
            let folder = SharedFolder::new();
            let (a, c) = group(&folder).await;
            let now = now_ms();
            let code = decode_join_code(&create(&a, now).await).unwrap();
            let transports = a.transports().await;
            let intruder = SigningKey::generate(&mut rand::rngs::OsRng);

            // Signed with an invite key that isn't the code's.
            let forged = sign_invitation_redemption(
                &invite_ed25519_signing_key(&[7u8; 32]),
                &intruder,
                InvitationRedemption {
                    invitation_cid: code.invitation_cid.clone(),
                    device_id: EnvelopeDeviceId::from_bytes([0xab; 16]),
                    ed25519_public: ByteBuf::from(intruder.verifying_key().to_bytes().to_vec()),
                    x25519_public: ByteBuf::from(vec![4u8; 32]),
                    device_name: "Intruder".to_string(),
                    created_at_ms: now,
                },
            )
            .unwrap();
            publish_to_all(&transports, &encode_signed_invitation_redemption(&forged).unwrap()).await;

            // Holding the code, but claiming C's device id to replace C's keys.
            let hijack = sign_invitation_redemption(
                &invite_ed25519_signing_key(&code.invite_secret()),
                &intruder,
                InvitationRedemption {
                    invitation_cid: code.invitation_cid.clone(),
                    device_id: c.identity.device_id,
                    ed25519_public: ByteBuf::from(intruder.verifying_key().to_bytes().to_vec()),
                    x25519_public: ByteBuf::from(vec![5u8; 32]),
                    device_name: "Not C".to_string(),
                    created_at_ms: now + 1,
                },
            )
            .unwrap();
            publish_to_all(&transports, &encode_signed_invitation_redemption(&hijack).unwrap()).await;

            a.sweep().await;
            assert!(!a.process(now + 2_000).await);
            let c_key = a.database.known_device_roster().unwrap().into_iter().find(|(id, _)| *id == c.identity.device_id).unwrap().1;
            assert_eq!(c_key, c.identity.verifying_key);
            let codes = a.database.outstanding_join_codes().unwrap();
            assert_eq!(codes[0].status, "open");
            assert_eq!(codes[0].rejected_attempts, 1, "only the code-holding attempt is recorded");
        }

        #[tokio::test]
        async fn an_invitation_alone_is_not_evidence_of_a_group() {
            let transports = fake_transports("empty");
            let key = SigningKey::generate(&mut rand::rngs::OsRng);
            let id = EnvelopeDeviceId::from_bytes([1; 16]);
            let invitation = sign_invitation(
                &key,
                Invitation {
                    inviter_device_id: id,
                    invite_ed25519_public: ByteBuf::from(vec![1u8; 32]),
                    invite_x25519_public: ByteBuf::from(vec![2u8; 32]),
                    key_epoch: 0,
                    sealed_epoch_key: ByteBuf::from(vec![3u8; 72]),
                    roster: vec![RosterEntry {
                        device_id: id,
                        ed25519_public: ByteBuf::from(key.verifying_key().to_bytes().to_vec()),
                        x25519_public: ByteBuf::from(vec![4u8; 32]),
                        status: "active".to_string(),
                    }],
                    recovery_ed25519_public: ByteBuf::from(vec![5u8; 32]),
                    recovery_x25519_public: ByteBuf::from(vec![6u8; 32]),
                    created_at_ms: 1,
                    expires_at_ms: 2,
                    earlier_epoch_keys: vec![],
                },
            )
            .unwrap();
            publish_to_all(&transports, &encode_signed_invitation(&invitation).unwrap()).await;
            assert_eq!(inspect_sync_space(&transports).await, SyncSpacePresence::None);
        }

        #[tokio::test]
        async fn joining_is_refused_for_an_enrolled_device_or_an_expired_code() {
            let folder = SharedFolder::new();
            let (a, c) = group(&folder).await;
            let now = now_ms();
            let code = create(&a, now).await;
            let error = join(&c, &code, &folder, now).await.unwrap_err();
            assert!(error.contains("already belongs"), "{error}");

            let b = Device::new();
            let error = join(&b, &code, &folder, now + 25 * 3_600_000).await.unwrap_err();
            assert!(error.contains("expired"), "{error}");
            assert!(b.database.configured_transports().unwrap().is_empty());

            let error = join_with_code(&b.database, &b.identity, &b.epoch_keys, &code, &[], vec![], now).await.unwrap_err();
            assert!(error.contains("Choose this device's copy"), "{error}");
        }

        #[tokio::test]
        async fn leaving_the_group_forgets_join_codes() {
            let folder = SharedFolder::new();
            let (a, _c) = group(&folder).await;
            create(&a, now_ms()).await;
            let b = Device::new();
            join(&b, &create(&a, now_ms()).await, &folder, now_ms()).await.unwrap();
            a.sweep().await;
            a.database.leave_sync_space().unwrap();
            b.database.leave_sync_space().unwrap();
            for database in [&a.database, &b.database] {
                let count: i64 = database
                    .connection()
                    .unwrap()
                    .query_row(
                        "SELECT (SELECT COUNT(*) FROM replicated_sync_invitations) + (SELECT COUNT(*) FROM replicated_sync_invitation_redemptions)",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(count, 0);
            }
        }

        #[tokio::test]
        async fn creating_a_code_validates_lifetime_and_connectors() {
            let folder = SharedFolder::new();
            let (a, _c) = group(&folder).await;
            let now = now_ms();
            let transports = a.transports().await;
            let create_for = |hours: u32, choices: Vec<JoinCodeConnectorChoice>| {
                let transports = transports.clone();
                let a = &a;
                async move { create_join_code(&a.database, &a.identity, &a.keys(), &transports, &choices, hours, now).await }
            };
            for (hours, ok) in [
                (MIN_JOIN_CODE_HOURS - 1, false),
                (MIN_JOIN_CODE_HOURS, true),
                (MIN_JOIN_CODE_HOURS + 1, true),
                (MAX_JOIN_CODE_HOURS - 1, true),
                (MAX_JOIN_CODE_HOURS, true),
                (MAX_JOIN_CODE_HOURS + 1, false),
            ] {
                assert_eq!(create_for(hours, vec![choice("folder-a", true)]).await.is_ok(), ok, "{hours} hours");
            }
            assert!(create_for(24, vec![]).await.is_err());
            assert!(create_for(24, vec![choice("missing", true)]).await.is_err());
            let too_many = vec![choice("folder-a", true); threestrands_sync_envelope::limits::MAX_JOIN_CONNECTORS + 1];
            let before = a.database.outstanding_join_codes().unwrap().len();
            assert!(create_for(24, too_many).await.is_err());
            assert_eq!(a.database.outstanding_join_codes().unwrap().len(), before, "nothing published for a code that can't encode");
        }

        /// Every text or blob value in every table, for "this secret is
        /// stored nowhere in SQLite" assertions.
        fn database_contains(database: &Database, needle: &[u8]) -> bool {
            let connection = database.connection().unwrap();
            let tables: Vec<String> = connection
                .prepare("SELECT name FROM sqlite_master WHERE type='table'")
                .unwrap()
                .query_map([], |row| row.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            for table in tables {
                let mut statement = connection.prepare(&format!("SELECT * FROM \"{table}\"")).unwrap();
                let columns = statement.column_count();
                let mut rows = statement.query([]).unwrap();
                while let Some(row) = rows.next().unwrap() {
                    for index in 0..columns {
                        let bytes: &[u8] = match row.get_ref(index).unwrap() {
                            rusqlite::types::ValueRef::Text(bytes) | rusqlite::types::ValueRef::Blob(bytes) => bytes,
                            _ => continue,
                        };
                        if bytes.windows(needle.len()).any(|window| window == needle) {
                            return true;
                        }
                    }
                }
            }
            false
        }

        #[tokio::test]
        async fn a_join_code_over_s3_carries_everything_and_secrets_stay_out_of_sqlite() {
            use crate::s3_transport::fake_server::FakeS3Server;
            let server = FakeS3Server::spawn().await;
            let a = Device::new();
            let a_connector = format!("s3-{}", uuid::Uuid::new_v4());
            a.database.add_s3_transport(&a_connector, &server.config("group"), &FakeS3Server::credentials()).unwrap();
            begin_genesis(&a.database, &a.identity, &a.epoch_keys, &a.transports().await, false).await.unwrap();

            let now = now_ms();
            let code = create_join_code(&a.database, &a.identity, &a.keys(), &a.transports().await, &[choice(&a_connector, true)], 1, now)
                .await
                .unwrap();
            let secret = decode_join_code(&code).unwrap().invite_secret();
            assert!(!database_contains(&a.database, code.as_bytes()));
            assert!(!database_contains(&a.database, &secret));
            assert!(!database_contains(&a.database, crate::s3_transport::fake_server::SECRET_KEY.as_bytes()));

            let preview = preview_join_code(&code, now).unwrap();
            assert_eq!(preview.connectors.len(), 1);
            assert!(preview.connectors[0].supported && preview.connectors[0].credentials_included);
            assert!(!preview.connectors[0].needs_credentials && !preview.connectors[0].needs_folder);

            // No input beyond the code.
            let b = Device::new();
            join_with_code(&b.database, &b.identity, &b.epoch_keys, &code, &[], vec![], now).await.unwrap();
            let rows = b.database.configured_transports().unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].kind, "s3");
            assert!(!database_contains(&b.database, crate::s3_transport::fake_server::SECRET_KEY.as_bytes()));
            assert!(!database_contains(&b.database, &secret));
            assert_eq!(
                crate::sync_connectors::TransportSecrets::load("s3", &rows[0].instance_id).unwrap(),
                Some(crate::sync_connectors::TransportSecrets::S3(FakeS3Server::credentials()))
            );

            a.sweep().await;
            assert!(a.process(now + 1_000).await);
            b.sweep().await;
            assert_eq!(b.awaiting_admission(), None);
        }

        #[tokio::test]
        async fn a_code_without_credentials_asks_the_joiner_for_them() {
            use crate::s3_transport::fake_server::FakeS3Server;
            let server = FakeS3Server::spawn().await;
            let a = Device::new();
            let a_connector = format!("s3-{}", uuid::Uuid::new_v4());
            a.database.add_s3_transport(&a_connector, &server.config("group"), &FakeS3Server::credentials()).unwrap();
            begin_genesis(&a.database, &a.identity, &a.epoch_keys, &a.transports().await, false).await.unwrap();
            let now = now_ms();
            let code = create_join_code(&a.database, &a.identity, &a.keys(), &a.transports().await, &[choice(&a_connector, false)], 1, now)
                .await
                .unwrap();
            assert!(decode_join_code(&code).unwrap().connectors[0].secrets_json.is_none());
            assert!(preview_join_code(&code, now).unwrap().connectors[0].needs_credentials);

            let b = Device::new();
            let error = join_with_code(&b.database, &b.identity, &b.epoch_keys, &code, &[], vec![], now).await.unwrap_err();
            assert!(error.contains("access key"), "{error}");
            let credentials: crate::sync_connectors::ConnectorCredentials = serde_json::from_value(serde_json::json!({
                "kind": "s3",
                "accessKeyId": crate::s3_transport::fake_server::ACCESS_KEY,
                "secretAccessKey": crate::s3_transport::fake_server::SECRET_KEY,
            }))
            .unwrap();
            join_with_code(
                &b.database,
                &b.identity,
                &b.epoch_keys,
                &code,
                &[],
                vec![JoinCredentialsChoice { connector_index: 0, credentials }],
                now,
            )
            .await
            .unwrap();
            assert!(b.awaiting_admission().is_some());
        }

        #[test]
        fn preview_describes_each_connector_without_side_effects() {
            let code = threestrands_sync_envelope::JoinCode {
                version: 1,
                invite_secret: ByteBuf::from(vec![1u8; 32]),
                invitation_cid: compute_cid(b"invitation"),
                inviter_name: "Laptop".to_string(),
                expires_at_ms: 2_000_000_000_000,
                connectors: vec![
                    threestrands_sync_envelope::JoinConnector {
                        kind: "folder".to_string(),
                        config_json: r#"{"folderName":"Dropbox Sync","label":"Home"}"#.to_string(),
                        secrets_json: None,
                    },
                    threestrands_sync_envelope::JoinConnector {
                        kind: "ipfs_rpc".to_string(),
                        config_json: r#"{"baseUrl":"https://rpc.filebase.io"}"#.to_string(),
                        secrets_json: None,
                    },
                    threestrands_sync_envelope::JoinConnector {
                        kind: "carrier-pigeon".to_string(),
                        config_json: "{}".to_string(),
                        secrets_json: None,
                    },
                ],
            };
            let text = encode_join_code(&code).unwrap();
            let preview = preview_join_code(&text, 1_000).unwrap();
            assert_eq!(preview.inviter_name, "Laptop");
            assert!(!preview.expired);
            assert!(preview_join_code(&text, 3_000_000_000_000).unwrap().expired);
            let folder = &preview.connectors[0];
            assert!(folder.supported && folder.needs_folder);
            assert_eq!(folder.folder_name.as_deref(), Some("Dropbox Sync"));
            assert_eq!(folder.label.as_deref(), Some("Home"));
            let ipfs = &preview.connectors[1];
            assert!(ipfs.supported && !ipfs.needs_credentials && !ipfs.needs_folder);
            assert_eq!(ipfs.location, "https://rpc.filebase.io");
            assert!(!preview.connectors[2].supported);

            assert!(preview_join_code("hello", 0).unwrap_err().contains("isn't a ThreeStrands join code"));
            assert!(preview_join_code("TSJOIN9-abc", 0).unwrap_err().contains("newer version"));
        }

        #[test]
        fn connectors_of_unknown_kinds_are_skipped_and_all_unknown_is_an_error() {
            let base = threestrands_sync_envelope::JoinCode {
                version: 1,
                invite_secret: ByteBuf::from(vec![1u8; 32]),
                invitation_cid: compute_cid(b"invitation"),
                inviter_name: String::new(),
                expires_at_ms: 1,
                connectors: vec![threestrands_sync_envelope::JoinConnector {
                    kind: "carrier-pigeon".to_string(),
                    config_json: "{}".to_string(),
                    secrets_json: None,
                }],
            };
            let error = join_codes::prepare_join_connectors(&base, &[], vec![]).unwrap_err();
            assert!(error.contains("Update this app"), "{error}");
            let mut mixed = base;
            mixed.connectors.push(threestrands_sync_envelope::JoinConnector {
                kind: "ipfs_rpc".to_string(),
                config_json: r#"{"baseUrl":"https://rpc.filebase.io"}"#.to_string(),
                secrets_json: Some(r#"{"token":"t"}"#.to_string()),
            });
            let prepared = join_codes::prepare_join_connectors(&mixed, &[], vec![]).unwrap();
            assert_eq!(prepared.len(), 1);
            assert_eq!(prepared[0].0.kind(), "ipfs_rpc");
        }

        #[tokio::test]
        async fn find_invitation_says_why_the_invitation_could_not_be_fetched() {
            let code = threestrands_sync_envelope::JoinCode {
                version: 1,
                invite_secret: ByteBuf::from(vec![1u8; 32]),
                invitation_cid: compute_cid(b"invitation"),
                inviter_name: String::new(),
                expires_at_ms: 1,
                connectors: vec![],
            };
            let rejecting = threestrands_sync_transport::fake::FakeTransport::new("rejecting");
            rejecting.set_authentication_failure(true);
            let offline = threestrands_sync_transport::fake::FakeTransport::new("offline");
            offline.inject_transient_outage(10);
            let empty = threestrands_sync_transport::fake::FakeTransport::new("empty");
            let as_transports = |list: Vec<threestrands_sync_transport::fake::FakeTransport>| -> Vec<Arc<dyn SyncTransport>> {
                list.into_iter().map(|transport| Arc::new(transport) as Arc<dyn SyncTransport>).collect()
            };

            let error = find_invitation(&code, &as_transports(vec![rejecting])).await.err().unwrap();
            assert_eq!(error, CREDENTIALS_REJECTED);
            let error = find_invitation(&code, &as_transports(vec![offline])).await.err().unwrap();
            assert_eq!(error, STORAGE_UNREACHABLE);

            // Rejected credentials outrank an outage or an empty connector.
            let rejecting = threestrands_sync_transport::fake::FakeTransport::new("rejecting");
            rejecting.set_authentication_failure(true);
            let offline = threestrands_sync_transport::fake::FakeTransport::new("offline");
            offline.inject_transient_outage(10);
            let error = find_invitation(&code, &as_transports(vec![empty, offline, rejecting])).await.err().unwrap();
            assert_eq!(error, CREDENTIALS_REJECTED);

            // An object that doesn't match the code outranks everything.
            let wrong = threestrands_sync_transport::fake::FakeTransport::new("wrong");
            wrong.put_object(&TransportCid(code.invitation_cid.clone()), b"not the invitation").await.unwrap();
            let rejecting = threestrands_sync_transport::fake::FakeTransport::new("rejecting");
            rejecting.set_authentication_failure(true);
            let error = find_invitation(&code, &as_transports(vec![rejecting, wrong])).await.err().unwrap();
            assert!(error.contains("damaged"), "{error}");
        }

        #[tokio::test]
        async fn joining_with_rejected_s3_credentials_says_so_and_saves_nothing() {
            use crate::s3_transport::fake_server::FakeS3Server;
            let server = FakeS3Server::spawn().await;
            let a = Device::new();
            let a_connector = format!("s3-{}", uuid::Uuid::new_v4());
            a.database.add_s3_transport(&a_connector, &server.config("group"), &FakeS3Server::credentials()).unwrap();
            begin_genesis(&a.database, &a.identity, &a.epoch_keys, &a.transports().await, false).await.unwrap();
            let now = now_ms();
            let code = create_join_code(&a.database, &a.identity, &a.keys(), &a.transports().await, &[choice(&a_connector, false)], 1, now)
                .await
                .unwrap();

            let b = Device::new();
            let wrong: crate::sync_connectors::ConnectorCredentials = serde_json::from_value(serde_json::json!({
                "kind": "s3",
                "accessKeyId": crate::s3_transport::fake_server::ACCESS_KEY,
                "secretAccessKey": "a-revoked-secret",
            }))
            .unwrap();
            let error = join_with_code(
                &b.database,
                &b.identity,
                &b.epoch_keys,
                &code,
                &[],
                vec![JoinCredentialsChoice { connector_index: 0, credentials: wrong }],
                now,
            )
            .await
            .unwrap_err();
            assert_eq!(error, CREDENTIALS_REJECTED);
            assert!(b.database.configured_transports().unwrap().is_empty());
            assert!(matches!(b.database.enrollment_status().unwrap(), EnrollmentStatus::NotStarted));
        }

        #[tokio::test]
        async fn find_invitation_reports_a_missing_object() {
            let code = threestrands_sync_envelope::JoinCode {
                version: 1,
                invite_secret: ByteBuf::from(vec![1u8; 32]),
                invitation_cid: compute_cid(b"invitation"),
                inviter_name: String::new(),
                expires_at_ms: 1,
                connectors: vec![],
            };
            let error = find_invitation(&code, &fake_transports("empty")).await.err().unwrap();
            assert_eq!(error, INVITATION_NOT_FOUND);
        }
    }
}
