//! Join-code bookkeeping: invitations this device created, received, or
//! observed, and the redemptions of them. Code generation, signing, and
//! publishing stay in `enrollment/join_codes.rs`.

use rusqlite::{params, OptionalExtension};

use super::enrollment::default_device_name;
use super::{Database, DbResult};
use crate::enrollment::{JoinCodeNotice, OutstandingJoinCode, MAX_DEVICE_LABEL_CHARS};
use crate::replicated_sync::{encode_id, DeviceIdentity};

/// A pending redemption of one of this device's codes: redemption cid,
/// invitation cid, device id, Ed25519 and X25519 public keys, device name.
pub(crate) type PendingRedemption = (String, String, String, Vec<u8>, Vec<u8>, String);
type RedemptionInvitation = (String, Option<Vec<u8>>, String);

pub(crate) fn rfc3339_from_ms(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms).map(|time| time.to_rfc3339()).unwrap_or_default()
}

impl Database {
    /// This device's shared name, for the code and the redemption.
    pub(crate) fn self_device_name(&self, identity: &DeviceIdentity) -> DbResult<String> {
        let device_id = encode_id(identity.device_id.as_bytes());
        self.ensure_self_device_name(&device_id)?;
        let label: Option<String> = self
            .connection()?
            .query_row("SELECT label FROM sync_device_labels WHERE device_id=?1", params![device_id], |row| row.get(0))
            .optional()
            ?;
        Ok(label
            .unwrap_or_else(default_device_name)
            .chars()
            .take(MAX_DEVICE_LABEL_CHARS)
            .collect())
    }

    /// Join codes this device created, newest first.
    pub fn outstanding_join_codes(&self) -> DbResult<Vec<OutstandingJoinCode>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT i.invitation_cid, i.created_at, i.expires_at_ms, i.status, i.redeemed_by_device_id,
                        (SELECT r.device_name FROM replicated_sync_invitation_redemptions r WHERE r.redemption_cid = i.redemption_cid),
                        (SELECT COUNT(*) FROM replicated_sync_invitation_redemptions r
                          WHERE r.invitation_cid = i.invitation_cid AND r.state='rejected')
                 FROM replicated_sync_invitations i
                 WHERE i.direction='outgoing'
                 ORDER BY i.created_at DESC",
            )
            ?;
        let rows = statement
            .query_map([], |row| {
                Ok(OutstandingJoinCode {
                    invitation_cid: row.get(0)?,
                    created_at: row.get(1)?,
                    expires_at: rfc3339_from_ms(row.get(2)?),
                    status: row.get(3)?,
                    redeemed_by_device_id: row.get(4)?,
                    redeemed_by_name: row.get(5)?,
                    rejected_attempts: row.get(6)?,
                })
            })
            ?
            .collect::<Result<Vec<_>, _>>()
            ?;
        Ok(rows)
    }

    /// Undismissed join-code notices: devices that joined with a code (once
    /// they're actually in the roster), and refused attempts on this
    /// device's own codes.
    pub fn join_code_notices(&self) -> DbResult<Vec<JoinCodeNotice>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT r.redemption_cid,
                        CASE WHEN r.state='rejected' THEN 'rejectedAttempt' ELSE 'joined' END,
                        r.device_id, r.device_name, r.inviter_device_id,
                        (SELECT label FROM sync_device_labels l WHERE l.device_id = r.inviter_device_id),
                        r.created_at_ms
                 FROM replicated_sync_invitation_redemptions r
                 WHERE r.notice_dismissed = 0
                   AND (r.state = 'rejected'
                        OR (r.state IN ('admitted','observed')
                            AND EXISTS (SELECT 1 FROM sync_devices d
                                        WHERE d.device_id = r.device_id AND d.status='active' AND d.is_self=0)))
                 ORDER BY r.created_at_ms",
            )
            ?;
        let rows = statement
            .query_map([], |row| {
                Ok(JoinCodeNotice {
                    redemption_cid: row.get(0)?,
                    kind: row.get(1)?,
                    device_id: row.get(2)?,
                    device_name: row.get(3)?,
                    inviter_device_id: row.get(4)?,
                    inviter_name: row.get(5)?,
                    at: rfc3339_from_ms(row.get(6)?),
                })
            })
            ?
            .collect::<Result<Vec<_>, _>>()
            ?;
        Ok(rows)
    }

    pub fn dismiss_join_code_notice(&self, redemption_cid: &str) -> DbResult<()> {
        self.connection()?
            .execute(
                "UPDATE replicated_sync_invitation_redemptions SET notice_dismissed=1 WHERE redemption_cid=?1",
                params![redemption_cid],
            )
            ?;
        Ok(())
    }

    /// The inviter's name while this device waits for its join-code
    /// admission; `None` once admitted or when it didn't join by code.
    pub(crate) fn awaiting_admission_from(&self) -> DbResult<Option<String>> {
        let name: Option<Option<String>> = self
            .connection()?
            .query_row(
                "SELECT inviter_name FROM replicated_sync_invitations WHERE direction='incoming' AND status='pending' LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            ?;
        Ok(name.map(|name| name.filter(|name| !name.trim().is_empty()).unwrap_or_else(|| "another device".to_string())))
    }

    // ---- Invitations ----

    pub(crate) fn record_outgoing_invitation(
        &self,
        invitation_cid: &str,
        inviter_device_id: &str,
        inviter_name: &str,
        invite_ed25519_public: &[u8],
        created_at_ms: i64,
        expires_at_ms: i64,
    ) -> DbResult<()> {
        self.connection()?.execute(
            "INSERT INTO replicated_sync_invitations(invitation_cid, direction, status, inviter_device_id, inviter_name,
                 invite_ed25519_public, created_at, expires_at_ms)
             VALUES (?1,'outgoing','open',?2,?3,?4,?5,?6)",
            params![invitation_cid, inviter_device_id, inviter_name, invite_ed25519_public, rfc3339_from_ms(created_at_ms), expires_at_ms],
        )?;
        Ok(())
    }

    /// Records the code this device is joining with, replacing any earlier
    /// attempt with the same code.
    pub(crate) fn record_incoming_invitation(
        &self,
        invitation_cid: &str,
        inviter_device_id: &str,
        inviter_name: &str,
        created_at_ms: i64,
        expires_at_ms: i64,
        redemption_cid: &str,
    ) -> DbResult<()> {
        self.connection()?.execute(
            "INSERT OR REPLACE INTO replicated_sync_invitations(invitation_cid, direction, status, inviter_device_id,
                 inviter_name, created_at, expires_at_ms, redemption_cid)
             VALUES (?1,'incoming','pending',?2,?3,?4,?5,?6)",
            params![invitation_cid, inviter_device_id, inviter_name, rfc3339_from_ms(created_at_ms), expires_at_ms, redemption_cid],
        )?;
        Ok(())
    }

    /// Records another member's invitation, so its redemptions can be
    /// verified here.
    pub(crate) fn record_observed_invitation(
        &self,
        invitation_cid: &str,
        inviter_device_id: &str,
        invite_ed25519_public: &[u8],
        created_at_ms: i64,
        expires_at_ms: i64,
    ) -> DbResult<()> {
        self.connection()?.execute(
            "INSERT OR IGNORE INTO replicated_sync_invitations(invitation_cid, direction, status, inviter_device_id,
                 invite_ed25519_public, created_at, expires_at_ms)
             VALUES (?1,'observed','observed',?2,?3,?4,?5)",
            params![invitation_cid, inviter_device_id, invite_ed25519_public, rfc3339_from_ms(created_at_ms), expires_at_ms],
        )?;
        Ok(())
    }

    /// An invitation's direction, invite key, and inviter, if known here.
    pub(crate) fn invitation_for_redemption(&self, invitation_cid: &str) -> DbResult<Option<RedemptionInvitation>> {
        Ok(self
            .connection()?
            .query_row(
                "SELECT direction, invite_ed25519_public, inviter_device_id FROM replicated_sync_invitations WHERE invitation_cid=?1",
                params![invitation_cid],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?)
    }

    pub(crate) fn invitation_status_and_expiry(&self, invitation_cid: &str) -> DbResult<(String, i64)> {
        Ok(self.connection()?.query_row(
            "SELECT status, expires_at_ms FROM replicated_sync_invitations WHERE invitation_cid=?1",
            params![invitation_cid],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?)
    }

    /// This device was admitted: its pending incoming invitations are done.
    pub(crate) fn mark_incoming_invitations_admitted(&self) -> DbResult<()> {
        self.connection()?.execute(
            "UPDATE replicated_sync_invitations SET status='admitted' WHERE direction='incoming' AND status='pending'",
            [],
        )?;
        Ok(())
    }

    /// Codes of this device's still open past `now_ms`.
    pub(crate) fn open_invitations_expired_by(&self, now_ms: i64) -> DbResult<Vec<String>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT invitation_cid FROM replicated_sync_invitations
             WHERE direction='outgoing' AND status='open' AND expires_at_ms < ?1",
        )?;
        let rows = statement.query_map(params![now_ms], |row| row.get(0))?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Redeemed codes of this device's whose objects outlived the code.
    pub(crate) fn redeemed_invitations_expired_by(&self, now_ms: i64) -> DbResult<Vec<String>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT invitation_cid FROM replicated_sync_invitations
             WHERE direction='outgoing' AND status='redeemed' AND object_deleted=0 AND expires_at_ms < ?1",
        )?;
        let rows = statement.query_map(params![now_ms], |row| row.get(0))?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub(crate) fn expire_invitation(&self, invitation_cid: &str) -> DbResult<()> {
        self.connection()?.execute(
            "UPDATE replicated_sync_invitations SET status='expired' WHERE invitation_cid=?1",
            params![invitation_cid],
        )?;
        Ok(())
    }

    /// Closes one of this device's open codes. Returns the rows changed:
    /// 0 when it was no longer open.
    pub(crate) fn cancel_open_invitation(&self, invitation_cid: &str) -> DbResult<usize> {
        Ok(self.connection()?.execute(
            "UPDATE replicated_sync_invitations SET status='cancelled'
             WHERE invitation_cid=?1 AND direction='outgoing' AND status='open'",
            params![invitation_cid],
        )?)
    }

    pub(crate) fn mark_invitation_object_deleted(&self, invitation_cid: &str) -> DbResult<()> {
        self.connection()?.execute(
            "UPDATE replicated_sync_invitations SET object_deleted=1 WHERE invitation_cid=?1",
            params![invitation_cid],
        )?;
        Ok(())
    }

    // ---- Redemptions ----

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record_redemption(
        &self,
        redemption_cid: &str,
        invitation_cid: &str,
        inviter_device_id: &str,
        device_id: &str,
        ed25519_public: &[u8],
        x25519_public: &[u8],
        device_name: &str,
        created_at_ms: i64,
        state: &str,
        received_at_ms: i64,
    ) -> DbResult<()> {
        self.connection()?.execute(
            "INSERT OR IGNORE INTO replicated_sync_invitation_redemptions(redemption_cid, invitation_cid, inviter_device_id,
                 device_id, ed25519_public, x25519_public, device_name, created_at_ms, state, received_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                redemption_cid,
                invitation_cid,
                inviter_device_id,
                device_id,
                ed25519_public,
                x25519_public,
                device_name,
                created_at_ms,
                state,
                rfc3339_from_ms(received_at_ms),
            ],
        )?;
        Ok(())
    }

    /// Pending redemptions of this device's own codes, oldest first.
    pub(crate) fn pending_redemptions(&self) -> DbResult<Vec<PendingRedemption>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT r.redemption_cid, r.invitation_cid, r.device_id, r.ed25519_public, r.x25519_public, r.device_name
             FROM replicated_sync_invitation_redemptions r
             JOIN replicated_sync_invitations i ON i.invitation_cid = r.invitation_cid
             WHERE r.state='pending' AND i.direction='outgoing'
             ORDER BY r.created_at_ms, r.redemption_cid",
        )?;
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Marks a code redeemed by `device_id` and its redemption admitted.
    pub(crate) fn admit_redemption(&self, invitation_cid: &str, device_id: &str, redemption_cid: &str) -> DbResult<()> {
        let connection = self.connection()?;
        connection.execute(
            "UPDATE replicated_sync_invitations SET status='redeemed', redeemed_by_device_id=?2, redemption_cid=?3
             WHERE invitation_cid=?1",
            params![invitation_cid, device_id, redemption_cid],
        )?;
        connection.execute(
            "UPDATE replicated_sync_invitation_redemptions SET state='admitted' WHERE redemption_cid=?1",
            params![redemption_cid],
        )?;
        Ok(())
    }

    pub(crate) fn reject_redemption(&self, redemption_cid: &str) -> DbResult<()> {
        self.connection()?.execute(
            "UPDATE replicated_sync_invitation_redemptions SET state='rejected' WHERE redemption_cid=?1",
            params![redemption_cid],
        )?;
        Ok(())
    }
}
