//! Enrollment bookkeeping: enrollment requests and staged grants, the device
//! roster and its shared names, key-epoch history, recovery public keys, and
//! control objects already applied. Signing, sealing, and the keychain stay
//! in `enrollment.rs`; this module only reads and writes these tables.

use std::collections::BTreeSet;

use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use serde_bytes::ByteBuf;
use threestrands_sync_envelope::limits::MAX_EARLIER_EPOCH_KEYS;
use threestrands_sync_envelope::{decode_signed_enrollment_grant, enrollment_fingerprint, DeviceId as EnvelopeDeviceId, RosterEntry};

use super::{Database, DatabaseError, DbResult};
use crate::enrollment::{
    roster_entry_x25519, DeviceRosterEntry, EnrollmentStatus, HistoryHandoffWarning, IncomingEnrollmentRequest,
    MAX_DEVICE_LABEL_CHARS,
};
use crate::replicated_sync::{decode_id, SPACE_ID};

pub(crate) type RecoveryPublicKeys = ([u8; 32], [u8; 32]);
type RawRecoveryPublicKeyRow = (Option<Vec<u8>>, Option<Vec<u8>>);

pub(crate) fn default_device_name() -> String {
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
    pub fn enrollment_status(&self) -> DbResult<EnrollmentStatus> {
        let connection = self.connection()?;
        let active_epoch: Option<i64> = connection
            .query_row("SELECT active_epoch FROM sync_spaces WHERE id=?1", params![SPACE_ID], |row| row.get(0))
            .optional()
            ?;
        let Some(active_epoch) = active_epoch else {
            return Ok(EnrollmentStatus::NotStarted);
        };
        let adopted: bool = connection
            .query_row("SELECT 1 FROM sync_epoch_history WHERE key_epoch=?1", params![active_epoch], |_| Ok(true))
            .optional()
            ?
            .unwrap_or(false);
        if adopted {
            let device_count: i64 = connection
                .query_row("SELECT COUNT(*) FROM sync_devices WHERE status='active'", [], |row| row.get(0))
                ?;
            let history_handoff_keys_used: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM sync_epoch_history WHERE key_epoch < ?1",
                    [active_epoch],
                    |row| row.get(0),
                )
                ?;
            let history_handoff_keys_used = history_handoff_keys_used as usize;
            let history_handoff_warning = (history_handoff_keys_used >= MAX_EARLIER_EPOCH_KEYS - MAX_EARLIER_EPOCH_KEYS / 5)
                .then_some(HistoryHandoffWarning {
                    used: history_handoff_keys_used,
                    limit: MAX_EARLIER_EPOCH_KEYS,
                });
            drop(connection);
            return Ok(EnrollmentStatus::Enrolled {
                device_count: device_count as usize,
                awaiting_admission_from: self.awaiting_admission_from()?,
                history_handoff_warning,
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
            ?;
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
            ?;
        let rejected: Option<(String, String)> = connection
            .query_row(
                "SELECT request_id, fingerprint FROM replicated_sync_enrollment_requests
                 WHERE direction='outgoing' AND status='rejected' ORDER BY created_at DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            ?;
        match pending {
            Some((request_id, fingerprint, created_at)) => Ok(EnrollmentStatus::AwaitingGrant { request_id, fingerprint, created_at }),
            None => match rejected {
                Some((request_id, fingerprint)) => Ok(EnrollmentStatus::Rejected { request_id, fingerprint }),
                None => Ok(EnrollmentStatus::NotStarted),
            },
        }
    }

    pub fn pending_incoming_enrollment_requests(&self) -> DbResult<Vec<IncomingEnrollmentRequest>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT request_id, device_id, fingerprint, created_at FROM replicated_sync_enrollment_requests
                 WHERE direction='incoming' AND status='pending' ORDER BY created_at ASC",
            )
            ?;
        let rows = statement
            .query_map([], |row| {
                Ok(IncomingEnrollmentRequest {
                    request_id: row.get(0)?,
                    device_id: row.get(1)?,
                    fingerprint: row.get(2)?,
                    created_at: row.get(3)?,
                })
            })
            ?
            .collect::<Result<Vec<_>, _>>()
            ?;
        Ok(rows)
    }

    /// This device first, then active peers, then revoked ones.
    pub fn device_roster(&self) -> DbResult<Vec<DeviceRosterEntry>> {
        let self_device_id: Option<String> = self.connection()?.query_row(
            "SELECT device_id FROM sync_devices WHERE is_self=1 LIMIT 1",
            [],
            |row| row.get(0),
        ).optional()?;
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
            ?;
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
            ?
            .collect::<Result<Vec<_>, _>>()
            ?;
        Ok(rows)
    }

    /// Gives this device its hostname the first time it appears in the roster.
    /// The sync loop records it once the portable preference entity exists.
    pub(crate) fn ensure_self_device_name(&self, device_id_hex: &str) -> DbResult<()> {
        self.connection()?
            .execute(
                "INSERT OR IGNORE INTO sync_device_labels(device_id,label) SELECT ?1,?2
                 WHERE EXISTS (SELECT 1 FROM sync_devices WHERE device_id=?1 AND is_self=1)",
                params![device_id_hex, default_device_name()],
            )
            ?;
        Ok(())
    }

    pub(crate) fn record_self_device_name_if_missing(&self, device_id_hex: &str) -> DbResult<()> {
        let field = format!("deviceName:{device_id_hex}");
        let (label, preferences_exist, already_recorded): (Option<String>, bool, bool) = self.connection()?.query_row(
            "SELECT l.label,
                    EXISTS(SELECT 1 FROM sync_values WHERE entity_type='preferences' AND entity_id='portable'),
                    EXISTS(SELECT 1 FROM sync_values WHERE entity_type='preferences' AND entity_id='portable' AND field=?1)
             FROM sync_device_labels l WHERE l.device_id=?2",
            params![field, device_id_hex],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).optional()?.unwrap_or((None, false, false));
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
    pub(crate) fn materialize_device_name(&self, device_id_hex: &str, name: Option<&str>) -> DbResult<()> {
        let connection = self.connection()?;
        match name {
            Some(name) => {
                connection.execute(
                    "INSERT INTO sync_device_labels(device_id,label) VALUES (?1,?2)
                     ON CONFLICT(device_id) DO UPDATE SET label=excluded.label",
                    params![device_id_hex, name],
                )?;
            }
            None => {
                connection.execute("DELETE FROM sync_device_labels WHERE device_id=?1", params![device_id_hex])?;
            }
        }
        Ok(())
    }

    /// Sets a shared roster name. A blank self-name restores this machine's
    /// hostname; a blank peer name removes its name from the shared roster.
    pub fn set_device_label(&self, device_id_hex: &str, label: &str) -> DbResult<()> {
        let label = if label.trim().is_empty() {
            let is_self: bool = self.connection()?.query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_devices WHERE device_id=?1 AND is_self=1)",
                params![device_id_hex],
                |row| row.get(0),
            )?;
            if is_self { default_device_name() } else { String::new() }
        } else {
            label.trim().to_string()
        };
        if label.chars().count() > MAX_DEVICE_LABEL_CHARS {
            return Err(DatabaseError::invalid(format!("Device names can be at most {MAX_DEVICE_LABEL_CHARS} characters.")));
        }
        let connection = self.connection()?;
        if label.is_empty() {
            connection.execute("DELETE FROM sync_device_labels WHERE device_id=?1", params![device_id_hex])?;
        } else {
            connection
                .execute(
                    "INSERT INTO sync_device_labels(device_id,label) VALUES (?1,?2)
                     ON CONFLICT(device_id) DO UPDATE SET label=excluded.label",
                    params![device_id_hex, label],
                )
                ?;
        }
        drop(connection);

        let field = format!("deviceName:{device_id_hex}");
        let fields = BTreeSet::from([field.clone()]);
        let payload = serde_json::json!({ (field): if label.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(label) } });
        let preferences_exist: bool = self.connection()?.query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_values WHERE entity_type='preferences' AND entity_id='portable')",
            [],
            |row| row.get(0),
        )?;
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
    pub fn leave_sync_space(&self) -> DbResult<u32> {
        let mut connection = self.connection()?;
        let tx = connection.transaction()?;
        let highest_epoch: u32 = tx
            .query_row(
                "SELECT MAX(COALESCE((SELECT MAX(key_epoch) FROM sync_epoch_history), 0),
                            COALESCE((SELECT active_epoch FROM sync_spaces WHERE id=?1), 0))",
                params![SPACE_ID],
                |row| row.get(0),
            )
            ?;
        tx.execute_batch(
            "DELETE FROM scheduled_send_summaries;
             DELETE FROM sync_deliveries;
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
        ?;
        tx.execute(
            "UPDATE sync_spaces SET active_epoch=0, lamport=0, recovery_public_key=NULL, recovery_x25519_public=NULL, last_error=NULL WHERE id=?1",
            params![SPACE_ID],
        )
        ?;
        tx.commit()?;
        Ok(highest_epoch)
    }

    pub(crate) fn seen_control_object(&self, cid: &str) -> DbResult<bool> {
        self.connection()?
            .query_row("SELECT 1 FROM sync_control_objects_seen WHERE cid=?1", params![cid], |_| Ok(true))
            .optional()
            .map(|found| found.unwrap_or(false))
            .map_err(DatabaseError::from)
    }

    pub(crate) fn mark_control_object_seen(&self, cid: &str, kind: &str) -> DbResult<()> {
        self.connection()?
            .execute(
                "INSERT OR IGNORE INTO sync_control_objects_seen(cid, object_kind) VALUES (?1,?2)",
                params![cid, kind],
            )
            ?;
        Ok(())
    }

    pub(crate) fn record_epoch_activation(&self, key_epoch: u32, source_cid: &str) -> DbResult<()> {
        self.connection()?
            .execute(
                "INSERT OR IGNORE INTO sync_epoch_history(key_epoch, activated_at, source_cid) VALUES (?1,?2,?3)",
                params![key_epoch, Utc::now().to_rfc3339(), source_cid],
            )
            ?;
        Ok(())
    }

    pub(crate) fn set_active_epoch(&self, key_epoch: u32) -> DbResult<()> {
        self.connection()?
            .execute("UPDATE sync_spaces SET active_epoch=?2 WHERE id=?1", params![SPACE_ID, key_epoch])
            ?;
        Ok(())
    }

    pub(crate) fn advance_active_epoch(&self, key_epoch: u32) -> DbResult<()> {
        self.connection()?
            .execute(
                "UPDATE sync_spaces SET active_epoch=?2 WHERE id=?1 AND active_epoch < ?2",
                params![SPACE_ID, key_epoch],
            )
            ?;
        Ok(())
    }

    pub(crate) fn set_recovery_public_keys(&self, ed25519_public: &[u8; 32], x25519_public: &[u8; 32]) -> DbResult<()> {
        self.connection()?
            .execute(
                "UPDATE sync_spaces SET recovery_public_key=?2, recovery_x25519_public=?3 WHERE id=?1",
                params![SPACE_ID, ed25519_public.to_vec(), x25519_public.to_vec()],
            )
            ?;
        Ok(())
    }

    pub(crate) fn recovery_public_keys(&self) -> DbResult<Option<RecoveryPublicKeys>> {
        let row: Option<RawRecoveryPublicKeyRow> = self
            .connection()?
            .query_row("SELECT recovery_public_key, recovery_x25519_public FROM sync_spaces WHERE id=?1", params![SPACE_ID], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()
            ?;
        let Some((ed25519, x25519)) = row else { return Ok(None) };
        let (Some(ed25519), Some(x25519)) = (ed25519, x25519) else { return Ok(None) };
        let ed25519: [u8; 32] = ed25519.try_into().map_err(|_| DatabaseError::corrupt("Invalid stored recovery Ed25519 key"))?;
        let x25519: [u8; 32] = x25519.try_into().map_err(|_| DatabaseError::corrupt("Invalid stored recovery X25519 key"))?;
        Ok(Some((ed25519, x25519)))
    }

    /// Every device this local database knows about, active *or* revoked —
    /// the full snapshot published inside a grant or rotation object, so a
    /// recipient learns a revocation explicitly (a `RosterEntry` with
    /// `status: "revoked"`) rather than merely inferring it from an
    /// omission, which a device that missed the rotation could not do.
    pub(crate) fn full_roster_snapshot(&self) -> DbResult<Vec<RosterEntry>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare("SELECT device_id, public_key, x25519_public, status FROM sync_devices WHERE public_key IS NOT NULL AND x25519_public IS NOT NULL")
            ?;
        let rows: Vec<(String, Vec<u8>, Vec<u8>, String)> = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
            ?
            .collect::<Result<Vec<_>, _>>()
            ?;
        rows.into_iter()
            .map(|(device_id_hex, ed25519, x25519, status)| {
                Ok(RosterEntry {
                    device_id: EnvelopeDeviceId::from_bytes(decode_id(&device_id_hex).map_err(DatabaseError::Corrupt)?),
                    ed25519_public: ByteBuf::from(ed25519),
                    x25519_public: ByteBuf::from(x25519),
                    status,
                })
            })
            .collect()
    }

    // ---- Enrollment requests ----

    /// Records a request this device sent (`outgoing`) or received
    /// (`incoming`) as pending, unless it is already known.
    pub(crate) fn record_enrollment_request(
        &self,
        direction: &str,
        request_id: &str,
        device_id: &str,
        ed25519_public: &[u8],
        x25519_public: &[u8],
        fingerprint: &str,
    ) -> DbResult<()> {
        self.connection()?.execute(
            "INSERT INTO replicated_sync_enrollment_requests(request_id, direction, device_id, ed25519_public, x25519_public, fingerprint, status, created_at)
             VALUES (?1,?7,?2,?3,?4,?5,'pending',?6)
             ON CONFLICT(request_id) DO NOTHING",
            params![request_id, device_id, ed25519_public, x25519_public, fingerprint, Utc::now().to_rfc3339(), direction],
        )?;
        Ok(())
    }

    /// The requester of an incoming request not yet approved.
    pub(crate) fn unresolved_incoming_request(&self, request_id: &str) -> DbResult<Option<(String, Vec<u8>, Vec<u8>)>> {
        Ok(self
            .connection()?
            .query_row(
                "SELECT device_id, ed25519_public, x25519_public FROM replicated_sync_enrollment_requests
                 WHERE request_id=?1 AND direction='incoming' AND status IN ('pending','rejected')",
                params![request_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?)
    }

    /// The requester of an incoming request still pending.
    pub(crate) fn pending_incoming_request(&self, request_id: &str) -> DbResult<(String, Vec<u8>, Vec<u8>)> {
        Ok(self.connection()?.query_row(
            "SELECT device_id, ed25519_public, x25519_public FROM replicated_sync_enrollment_requests WHERE request_id=?1 AND direction='incoming' AND status='pending'",
            params![request_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?)
    }

    pub(crate) fn approve_unresolved_incoming_request(&self, request_id: &str) -> DbResult<()> {
        self.connection()?.execute(
            "UPDATE replicated_sync_enrollment_requests SET status='approved' WHERE request_id=?1 AND direction='incoming' AND status IN ('pending','rejected')",
            params![request_id],
        )?;
        Ok(())
    }

    /// Records an incoming request another device already approved, so this
    /// device stops offering it.
    pub(crate) fn record_approved_incoming_request(
        &self,
        request_id: &str,
        device_id: &str,
        ed25519_public: &[u8],
        x25519_public: &[u8],
        fingerprint: &str,
    ) -> DbResult<()> {
        self.connection()?.execute(
            "INSERT OR IGNORE INTO replicated_sync_enrollment_requests(request_id, direction, device_id, ed25519_public, x25519_public, fingerprint, status, created_at)
             VALUES (?1,'incoming',?2,?3,?4,?5,'approved',?6)",
            params![request_id, device_id, ed25519_public, x25519_public, fingerprint, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    /// The id of this device's own request, while it still waits for a grant.
    pub(crate) fn unresolved_outgoing_request(&self, request_id: &str) -> DbResult<Option<String>> {
        Ok(self
            .connection()?
            .query_row(
                "SELECT request_id FROM replicated_sync_enrollment_requests WHERE request_id=?1 AND direction='outgoing' AND status IN ('pending','rejected')",
                params![request_id],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// Whether this device's own request is still open, grant staged or not.
    pub(crate) fn outgoing_request_open(&self, request_id: &str) -> DbResult<bool> {
        Ok(self.connection()?.query_row(
            "SELECT EXISTS(SELECT 1 FROM replicated_sync_enrollment_requests WHERE request_id=?1 AND direction='outgoing' AND status IN ('pending','rejected','staged'))",
            params![request_id],
            |row| row.get(0),
        )?)
    }

    /// Holds a received grant for the user to confirm by fingerprint.
    pub(crate) fn stage_enrollment_grant(&self, request_id: &str, grant_cbor: &[u8]) -> DbResult<()> {
        self.connection()?.execute(
            "UPDATE replicated_sync_enrollment_requests SET status='staged', pending_grant_cbor=?2 WHERE request_id=?1 AND direction='outgoing' AND status IN ('pending','rejected')",
            params![request_id, grant_cbor],
        )?;
        Ok(())
    }

    pub(crate) fn staged_enrollment_grant(&self, request_id: &str) -> DbResult<Vec<u8>> {
        Ok(self.connection()?.query_row(
            "SELECT pending_grant_cbor FROM replicated_sync_enrollment_requests WHERE request_id=?1 AND direction='outgoing' AND status='staged'",
            params![request_id],
            |row| row.get(0),
        )?)
    }

    pub(crate) fn enrollment_request_direction(&self, request_id: &str) -> DbResult<Option<String>> {
        Ok(self
            .connection()?
            .query_row(
                "SELECT direction FROM replicated_sync_enrollment_requests WHERE request_id=?1",
                params![request_id],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub(crate) fn incoming_request_pending(&self, request_id: &str) -> DbResult<bool> {
        Ok(self.connection()?.query_row(
            "SELECT EXISTS(SELECT 1 FROM replicated_sync_enrollment_requests WHERE request_id=?1 AND direction='incoming' AND status='pending')",
            params![request_id],
            |row| row.get(0),
        )?)
    }

    /// Marks a pending request rejected, whichever device it came from.
    pub(crate) fn mark_pending_request_rejected(&self, request_id: &str) -> DbResult<()> {
        self.connection()?.execute(
            "UPDATE replicated_sync_enrollment_requests SET status='rejected'
             WHERE request_id=?1 AND status='pending'",
            params![request_id],
        )?;
        Ok(())
    }

    pub(crate) fn reject_pending_incoming_request(&self, request_id: &str) -> DbResult<()> {
        self.connection()?.execute(
            "UPDATE replicated_sync_enrollment_requests SET status='rejected' WHERE request_id=?1 AND direction='incoming' AND status='pending'",
            params![request_id],
        )?;
        Ok(())
    }

    pub(crate) fn set_enrollment_request_status(&self, request_id: &str, status: &str) -> DbResult<()> {
        self.connection()?.execute(
            "UPDATE replicated_sync_enrollment_requests SET status=?2 WHERE request_id=?1",
            params![request_id, status],
        )?;
        Ok(())
    }

    // ---- Devices and epochs ----

    /// A device's Ed25519 public key, if it is trusted and active.
    pub(crate) fn active_device_public_key(&self, device_id: &str) -> DbResult<Option<Vec<u8>>> {
        Ok(self
            .connection()?
            .query_row(
                "SELECT public_key FROM sync_devices WHERE device_id=?1 AND status='active' AND public_key IS NOT NULL",
                params![device_id],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// A device's roster status and public key, if the roster has it.
    pub(crate) fn device_status_and_key(&self, device_id: &str) -> DbResult<Option<(String, Option<Vec<u8>>)>> {
        Ok(self
            .connection()?
            .query_row(
                "SELECT status, public_key FROM sync_devices WHERE device_id=?1",
                params![device_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?)
    }

    /// Whether the roster has ever held `device_id`, active or revoked.
    pub(crate) fn device_known(&self, device_id: &str) -> DbResult<bool> {
        Ok(self.connection()?.query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_devices WHERE device_id=?1)",
            params![device_id],
            |row| row.get(0),
        )?)
    }

    /// Whether this device has adopted any epoch key.
    pub(crate) fn has_epoch_history(&self) -> DbResult<bool> {
        Ok(self.connection()?.query_row("SELECT EXISTS(SELECT 1 FROM sync_epoch_history)", [], |row| row.get(0))?)
    }

    /// The control object (a rotation) that activated `key_epoch` here.
    pub(crate) fn epoch_source_control_object(&self, key_epoch: u32) -> DbResult<Option<(String, Vec<u8>)>> {
        Ok(self
            .connection()?
            .query_row(
                "SELECT h.source_cid, o.bytes FROM sync_epoch_history h
                 JOIN sync_objects o ON o.cid=h.source_cid AND o.object_kind='control'
                 WHERE h.key_epoch=?1",
                params![key_epoch],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?)
    }

    /// Active peers whose latest head is on an epoch older than `key_epoch`
    /// and that have not been sent its keys yet, with their X25519 keys.
    pub(crate) fn peers_behind_epoch(&self, key_epoch: u32) -> DbResult<Vec<(String, Vec<u8>)>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT d.device_id, d.x25519_public FROM sync_remote_states r
             JOIN sync_devices d ON d.device_id = r.device_id
             WHERE d.status = 'active' AND d.is_self = 0 AND d.x25519_public IS NOT NULL
               AND r.last_head_epoch < ?1 AND (r.keys_shared_epoch IS NULL OR r.keys_shared_epoch < ?1)",
        )?;
        let rows = statement
            .query_map(params![key_epoch], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub(crate) fn record_keys_shared(&self, device_id: &str, key_epoch: u32) -> DbResult<()> {
        self.connection()?
            .execute("UPDATE sync_remote_states SET keys_shared_epoch=?2 WHERE device_id=?1", params![device_id, key_epoch])?;
        Ok(())
    }

    /// Records a published control object locally, so anti-entropy repair
    /// keeps delivering it to every connector.
    pub(crate) fn store_control_object(&self, cid: &str, bytes: &[u8]) -> DbResult<()> {
        self.connection()?.execute(
            "INSERT OR IGNORE INTO sync_objects(cid,object_kind,chunk_index,chunk_count,bytes) VALUES (?1,'control',0,1,?2)",
            params![cid, bytes],
        )?;
        Ok(())
    }
}
