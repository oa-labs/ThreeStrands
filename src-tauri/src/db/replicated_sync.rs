//! Replicated-sync bookkeeping: the sync space and device roster, sealed
//! snapshot objects and their per-transport delivery ledger, peer heads and
//! merge progress, configured transport rows, and projections waiting on a
//! dependency. Keys live in the OS keychain and network I/O happens in
//! `replicated_sync.rs`; this module only reads and writes these tables.

use chrono::Utc;
use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::Value;
use threestrands_sync_core::EntityType;
use threestrands_sync_envelope::{
    compute_cid, protocol_marker_cid, DeviceHead, DeviceId as EnvelopeDeviceId, VerifyingKey, PROTOCOL_MARKER,
};
use threestrands_sync_transport::TransportInstanceId;

use super::{serialization_error, Database, DatabaseError, DbResult};
use crate::replicated_sync::{
    decode_id, encode_id, random_id, ChunkIndex, ConfiguredTransport, DeliveryItem, SPACE_ID,
};

pub(crate) fn ensure_space_and_device(tx: &Transaction) -> DbResult<[u8; 16]> {
    tx.execute(
        "INSERT OR IGNORE INTO sync_spaces(id, active_epoch, lamport, enabled) VALUES (?1, 0, 0, 1)",
        params![SPACE_ID],
    )?;
    // `is_self` — not "the first row" — is what identifies this device's
    // own entry once enrollment means `sync_devices` also holds peers.
    let existing: Option<String> = tx
        .query_row("SELECT device_id FROM sync_devices WHERE is_self=1 LIMIT 1", [], |row| row.get(0))
        .optional()?;
    if let Some(hex) = existing {
        return decode_id(&hex).map_err(DatabaseError::Corrupt);
    }
    let device_id = random_id();
    tx.execute(
        "INSERT INTO sync_devices(device_id, status, is_self) VALUES (?1, 'active', 1)",
        params![encode_id(&device_id)],
    )?;
    Ok(device_id)
}

impl Database {
    /// The persisted state of the Settings "Enable replicated sync" toggle.
    /// `false` (not an error) when no `sync_spaces` row exists yet — nothing
    /// has ever been turned on.
    pub fn beta_features_enabled(&self) -> DbResult<bool> {
        let enabled: Option<bool> = self.with_connection(|connection| {
            Ok(connection
                .query_row("SELECT enabled FROM sync_spaces WHERE id=?1", params![SPACE_ID], |row| row.get(0))
                .optional()?)
        })?;
        Ok(enabled.unwrap_or(false))
    }

    /// Turns the Settings "Enable replicated sync" toggle on or off. Turning
    /// it off stops replication (the periodic loop and every push/pull
    /// call check this) without deleting local keys, roster, or graph
    /// state — matching "disabling sync and returning to local-only
    /// operation" rather than an irreversible reset.
    pub fn set_beta_features_enabled(&self, on: bool) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO sync_spaces(id, active_epoch, lamport, enabled) VALUES (?1, 0, 0, ?2)
                 ON CONFLICT(id) DO UPDATE SET enabled=excluded.enabled",
                params![SPACE_ID, on],
            )?;
            Ok(())
        })
    }


    // ---- Device roster ----

    /// Records a device's public key as trusted for signature verification,
    /// leaving any existing X25519 public key untouched. For our own
    /// device, [`Self::local_device_identity`] calls
    /// [`Self::trust_device_keys`] instead. Tests call this directly to
    /// simulate an already-trusted peer that only needs Ed25519 material.
    #[cfg(test)]
    pub fn trust_device_public_key(&self, device_id: &[u8; 16], verifying_key: &VerifyingKey) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO sync_devices(device_id, public_key, status) VALUES (?1,?2,'active')
                 ON CONFLICT(device_id) DO UPDATE SET public_key=excluded.public_key",
                params![encode_id(device_id), verifying_key.to_bytes().to_vec()],
            )?;
            Ok(())
        })
    }

    /// Records both of a device's public keys as trusted and active. Used
    /// for self-trust and by enrollment/rotation import to adopt a roster
    /// snapshot.
    pub(crate) fn trust_device_keys(
        &self,
        device_id: &[u8; 16],
        verifying_key: &VerifyingKey,
        x25519_public: &[u8; 32],
    ) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO sync_devices(device_id, public_key, x25519_public, status) VALUES (?1,?2,?3,'active')
                 ON CONFLICT(device_id) DO UPDATE SET public_key=excluded.public_key, x25519_public=excluded.x25519_public, status='active'",
                params![encode_id(device_id), verifying_key.to_bytes().to_vec(), x25519_public.to_vec()],
            )?;
            Ok(())
        })
    }

    /// Adopts one device from a peer's roster snapshot. Like
    /// [`Self::trust_device_keys`], except that a device this database has
    /// already revoked stays revoked with its keys untouched. Revocation is
    /// permanent for a device id (leaving a sync group discards the id), and
    /// rosters can arrive out of order: an older rotation or a lagging key
    /// share still lists the device as active and must not undo a newer
    /// revocation.
    pub(crate) fn trust_roster_device_keys(
        &self,
        device_id: &[u8; 16],
        verifying_key: &VerifyingKey,
        x25519_public: &[u8; 32],
    ) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO sync_devices(device_id, public_key, x25519_public, status) VALUES (?1,?2,?3,'active')
                 ON CONFLICT(device_id) DO UPDATE SET public_key=excluded.public_key, x25519_public=excluded.x25519_public, status='active'
                 WHERE sync_devices.status <> 'revoked'",
                params![encode_id(device_id), verifying_key.to_bytes().to_vec(), x25519_public.to_vec()],
            )?;
            Ok(())
        })
    }

    /// Marks a device revoked: it stops being trusted for future signature
    /// verification (snapshots, heads, and enrollment/rotation
    /// objects alike), though it cannot un-decrypt ciphertext it already
    /// received under a prior epoch.
    pub(crate) fn revoke_device(&self, device_id: &[u8; 16]) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE sync_devices SET status='revoked' WHERE device_id=?1",
                params![encode_id(device_id)],
            )?;
            Ok(())
        })
    }

    /// Every device this local database currently trusts a public key for
    /// and considers active (not revoked).
    pub(crate) fn known_device_roster(&self) -> DbResult<Vec<(EnvelopeDeviceId, VerifyingKey)>> {
        let rows: Vec<(String, Vec<u8>)> = self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT device_id, public_key FROM sync_devices WHERE public_key IS NOT NULL AND status='active'",
            )?;
            let rows = statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;
        rows.into_iter()
            .map(|(device_id_hex, key_bytes)| {
                let device_id = EnvelopeDeviceId::from_bytes(decode_id(&device_id_hex).map_err(DatabaseError::Corrupt)?);
                let key_array: [u8; 32] = key_bytes
                    .try_into()
                    .map_err(|_| DatabaseError::corrupt("Stored device public key is invalid"))?;
                let verifying_key = VerifyingKey::from_bytes(&key_array)
                    .map_err(|_| DatabaseError::corrupt("Stored device public key is invalid"))?;
                Ok((device_id, verifying_key))
            })
            .collect()
    }


    // ---- Snapshots, heads, and merges ----

    /// The snapshot `transport_instance_id` can honestly be pointed at: this
    /// device's current snapshot, once every one of its objects has reached
    /// that transport. `None` while it hasn't, and the transport's head then
    /// keeps naming the previous snapshot, which stays there until it's
    /// replaced.
    pub(crate) fn delivered_state_head(&self, transport_instance_id: &str) -> DbResult<Option<(u64, String)>> {
        self.with_connection(|connection| {
            let row: Option<(i64, i64, Option<String>)> = connection
                .query_row(
                    "SELECT so.state_sequence,
                            SUM(CASE WHEN sd.state='delivered' THEN 0 ELSE 1 END),
                            MAX(CASE WHEN so.object_kind='state_index' THEN so.cid END)
                     FROM sync_objects so
                     LEFT JOIN sync_deliveries sd ON sd.cid = so.cid AND sd.transport_instance_id = ?1
                     WHERE so.state_sequence IS NOT NULL
                     GROUP BY so.state_sequence ORDER BY so.state_sequence DESC LIMIT 1",
                    params![transport_instance_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;
            Ok(match row {
                Some((sequence, 0, Some(index_cid))) => Some((sequence as u64, index_cid)),
                _ => None,
            })
        })
    }

    /// The latest snapshot sequence merged from `device_id_hex`.
    pub(crate) fn merged_state_sequence(&self, device_id_hex: &str) -> DbResult<u64> {
        self.with_connection(|connection| {
            let sequence: Option<i64> = connection
                .query_row(
                    "SELECT state_sequence FROM sync_remote_states WHERE device_id=?1",
                    params![device_id_hex],
                    |row| row.get(0),
                )
                .optional()?;
            Ok(sequence.unwrap_or(0) as u64)
        })
    }

    pub(crate) fn record_merged_state(&self, device_id_hex: &str, state_sequence: u64) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO sync_remote_states(device_id, state_sequence, merged_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(device_id) DO UPDATE SET state_sequence=excluded.state_sequence, merged_at=excluded.merged_at
                 WHERE excluded.state_sequence > sync_remote_states.state_sequence",
                params![device_id_hex, state_sequence as i64, Utc::now().to_rfc3339()],
            )?;
            Ok(())
        })
    }

    /// Remembers when a peer's latest verified head was published, when this
    /// device first saw it, and which key epoch the peer is on. Only a head newer than the one on record
    /// (by its own publication time) replaces it, so a stale copy on a
    /// lagging transport never rolls the record back.
    pub(crate) fn record_head_observation(&self, head: &DeviceHead, seen_at_ms: i64) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO sync_remote_states(device_id, last_head_published_at_ms, last_head_seen_at_ms, last_head_epoch) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(device_id) DO UPDATE SET last_head_published_at_ms=excluded.last_head_published_at_ms,
                     last_head_seen_at_ms=excluded.last_head_seen_at_ms, last_head_epoch=excluded.last_head_epoch
                 WHERE sync_remote_states.last_head_published_at_ms IS NULL
                    OR sync_remote_states.last_head_published_at_ms < excluded.last_head_published_at_ms",
                params![encode_id(head.device_id.as_bytes()), head.published_at_ms, seen_at_ms, head.epoch],
            )?;
            Ok(())
        })
    }

    /// Whether `head_content` (a head's content minus its publication time)
    /// should be published to `transport_instance_id` now: it changed since
    /// the last publication there, or the heartbeat is due.
    pub(crate) fn head_publication_due(&self, transport_instance_id: &str, head_content: &str, now_ms: i64) -> DbResult<bool> {
        let last: Option<(String, i64)> = self.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT head_content, published_at_ms FROM sync_head_publications WHERE transport_instance_id=?1",
                    params![transport_instance_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?)
        })?;
        Ok(match last {
            None => true,
            Some((content, published_at_ms)) => {
                content != head_content || now_ms - published_at_ms >= crate::sync_policy::HEAD_HEARTBEAT_MS
            }
        })
    }

    pub(crate) fn record_head_publication(&self, transport_instance_id: &str, head_content: &str, now_ms: i64) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO sync_head_publications(transport_instance_id, head_content, published_at_ms) VALUES (?1,?2,?3)
                 ON CONFLICT(transport_instance_id) DO UPDATE SET head_content=excluded.head_content, published_at_ms=excluded.published_at_ms",
                params![transport_instance_id, head_content, now_ms],
            )?;
            Ok(())
        })
    }


    // ---- Notices and cleanup markers ----

    /// Records the protocol marker as a local object, so anti-entropy
    /// repair stores it on every connector this device uses, including ones
    /// added later. Idempotent.
    pub(crate) fn ensure_protocol_marker_object(&self) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT OR IGNORE INTO sync_objects(cid,object_kind,chunk_index,chunk_count,bytes) VALUES (?1,'protocol_marker',0,1,?2)",
                params![protocol_marker_cid(), PROTOCOL_MARKER],
            )?;
            Ok(())
        })
    }

    /// Whether to tell the user that upgrading reset sync: this device left
    /// a group created by an earlier, incompatible build.
    pub fn protocol_reset_notice(&self) -> DbResult<bool> {
        self.with_connection(|connection| {
            Ok(connection.query_row("SELECT EXISTS(SELECT 1 FROM sync_notices WHERE kind='protocol_reset')", [], |row| row.get(0))?)
        })
    }

    pub fn dismiss_protocol_reset_notice(&self) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute("DELETE FROM sync_notices WHERE kind='protocol_reset'", [])?;
            Ok(())
        })
    }

    /// The highest epoch whose keychain entries a schema migration left
    /// behind (it can't reach the keychain itself), if any.
    pub(crate) fn pending_keychain_cleanup(&self) -> DbResult<Option<u32>> {
        self.with_connection(|connection| {
            Ok(connection
                .query_row("SELECT highest_epoch FROM sync_pending_keychain_cleanup WHERE id=1", [], |row| row.get(0))
                .optional()?)
        })
    }

    pub(crate) fn clear_pending_keychain_cleanup(&self) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute("DELETE FROM sync_pending_keychain_cleanup", [])?;
            Ok(())
        })
    }


    // ---- Delivery ledger ----

    pub(crate) fn pending_delivery_items(&self, transport_instance_id: &str) -> DbResult<Vec<DeliveryItem>> {
        self.with_connection(|connection| {
            let now = Utc::now().to_rfc3339();
            let mut statement = connection.prepare(
                "SELECT sd.cid, so.bytes, sd.attempts
                 FROM sync_deliveries sd JOIN sync_objects so ON so.cid = sd.cid
                 WHERE sd.transport_instance_id=?1 AND sd.state='pending'
                   AND (sd.retry_at IS NULL OR sd.retry_at <= ?2)",
            )?;
            let rows = statement
                .query_map(params![transport_instance_id, now], |row| {
                    Ok(DeliveryItem {
                        cid: row.get(0)?,
                        bytes: row.get(1)?,
                        attempts: row.get(2)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    pub(crate) fn record_delivery_success(&self, cid: &str, transport_instance_id: &str, remote_id: Option<&str>) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE sync_deliveries SET state='delivered', remote_id=?3, last_error=NULL, retry_at=NULL
                 WHERE cid=?1 AND transport_instance_id=?2",
                params![cid, transport_instance_id, remote_id],
            )?;
            Ok(())
        })
    }

    /// Ensures a pending delivery row exists for every locally known object
    /// on every transport in `transport_ids` that doesn't already have one
    /// (pending, delivered, or failed) — the anti-entropy behavior that
    /// turns transport union into replication. Cheap to call repeatedly:
    /// `INSERT OR IGNORE` only ever adds rows for a truly new pairing.
    pub fn enqueue_repair_deliveries(&self, transport_ids: &[TransportInstanceId]) -> DbResult<usize> {
        self.with_connection(|connection| {
            let cids: Vec<String> = {
                let mut statement = connection.prepare("SELECT cid FROM sync_objects")?;
                let rows = statement
                    .query_map([], |row| row.get(0))?
                    .collect::<Result<Vec<_>, _>>()?;
                rows
            };
            let mut created = 0;
            for cid in &cids {
                for transport_id in transport_ids {
                    created += connection.execute(
                        "INSERT OR IGNORE INTO sync_deliveries(cid,transport_instance_id,state,attempts) VALUES (?1,?2,'pending',0)",
                        params![cid, transport_id.0],
                    )?;
                }
            }
            Ok(created)
        })
    }

    /// Pending/delivered/failed delivery counts for one transport instance,
    /// for the Settings UI.
    pub fn delivery_counts(&self, transport_instance_id: &str) -> DbResult<(i64, i64, i64)> {
        self.with_connection(|connection| {
            let count = |state: &str| -> DbResult<i64> {
                Ok(connection.query_row(
                    "SELECT COUNT(*) FROM sync_deliveries WHERE transport_instance_id=?1 AND state=?2",
                    params![transport_instance_id, state],
                    |row| row.get(0),
                )?)
            };
            Ok((count("pending")?, count("delivered")?, count("failed")?))
        })
    }


    // ---- Pending materializations ----

    pub(crate) fn pending_entity_materializations(&self)->DbResult<Vec<(EntityType,String)>>{
        let rows=self.with_connection(|connection|{
            let mut statement=connection.prepare("SELECT entity_type,entity_id FROM pending_entity_materializations ORDER BY updated_at")?;
            let rows=statement.query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?)))?;
            rows.collect::<Result<Vec<_>,_>>().map_err(Into::into)
        })?;
        Ok(rows.into_iter().filter_map(|(kind,id)|{
            serde_json::from_value::<EntityType>(Value::String(kind.clone())).ok().map(|entity_type|(entity_type,id)).or_else(||{
                log::warn!(target:"replicated_sync", "ignoring pending materialization for unknown entity type {kind}");
                None
            })
        }).collect())
    }

    pub(crate) fn remember_pending_entity_materialization(&self,entity_type:EntityType,entity_id:&str,reason:&str)->DbResult<()>{
        let reason=reason.chars().take(2_000).collect::<String>();
        self.with_connection(|connection|{
            connection.execute("INSERT INTO pending_entity_materializations(entity_type,entity_id,reason,updated_at) VALUES(?1,?2,?3,?4) ON CONFLICT(entity_type,entity_id) DO UPDATE SET reason=excluded.reason,updated_at=excluded.updated_at",params![entity_type.as_str(),entity_id,reason,Utc::now().to_rfc3339()])?;
            Ok(())
        })
    }

    pub(crate) fn clear_pending_entity_materialization(&self,entity_type:EntityType,entity_id:&str)->DbResult<()>{
        self.with_connection(|connection|{
            connection.execute("DELETE FROM pending_entity_materializations WHERE entity_type=?1 AND entity_id=?2",params![entity_type.as_str(),entity_id])?;
            Ok(())
        })
    }


    // ---- Transport rows ----

    pub fn configured_transports(&self) -> DbResult<Vec<ConfiguredTransport>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT instance_id, kind, config_json, enabled, last_success_at, last_error FROM sync_transports",
            )?;
            let rows = statement
                .query_map([], |row| {
                    Ok(ConfiguredTransport {
                        instance_id: row.get(0)?,
                        kind: row.get(1)?,
                        config_json: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                        enabled: row.get(3)?,
                        last_success_at: row.get(4)?,
                        last_error: row.get(5)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    pub(crate) fn set_transport_success(&self, instance_id: &str) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE sync_transports SET last_success_at=?2, last_error=NULL WHERE instance_id=?1",
                params![instance_id, Utc::now().to_rfc3339()],
            )?;
            Ok(())
        })
    }

    pub(crate) fn set_transport_error(&self, instance_id: &str, error: &str) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE sync_transports SET last_error=?2 WHERE instance_id=?1",
                params![instance_id, error],
            )?;
            Ok(())
        })
    }

    /// This device's id in the sync space, creating the space and this
    /// device's roster row on first use.
    pub(crate) fn local_sync_device_id(&self) -> DbResult<[u8; 16]> {
        self.with_transaction(ensure_space_and_device)
    }

    // ---- Epochs ----

    /// The sync space's current key epoch.
    pub(crate) fn active_epoch(&self) -> DbResult<u32> {
        self.with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT active_epoch FROM sync_spaces WHERE id=?1",
                params![SPACE_ID],
                |row| row.get(0),
            )?)
        })
    }

    /// Every recorded key epoch older than `key_epoch`, oldest first.
    pub(crate) fn epochs_before(&self, key_epoch: u32) -> DbResult<Vec<u32>> {
        self.with_connection(|connection| {
            let mut statement =
                connection.prepare("SELECT key_epoch FROM sync_epoch_history WHERE key_epoch < ?1 ORDER BY key_epoch")?;
            let rows = statement
                .query_map(params![key_epoch], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    // ---- Sealed snapshots ----

    /// Stores a newly sealed snapshot as state sequence `next`: retires the
    /// previous snapshot's objects, stores the chunks and their index,
    /// schedules delivery to every transport, and marks the replica clean if
    /// nothing changed since `snapshot_generation`. One transaction.
    pub(crate) fn store_sealed_snapshot(
        &self,
        chunks: &[Vec<u8>],
        next: u64,
        key_epoch: u32,
        transports: &[TransportInstanceId],
        snapshot_generation: i64,
    ) -> DbResult<()> {
        self.with_transaction(|tx| {
            // The previous snapshot stays on each transport that has it
            // until that transport's head names this one; see
            // `delete_retired_objects`.
            let previous: Vec<(String, i64)> = {
                let mut statement = tx.prepare("SELECT cid, state_sequence FROM sync_objects WHERE state_sequence IS NOT NULL")?;
                let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?.collect::<Result<Vec<_>, _>>()?;
                rows
            };
            for (cid, state_sequence) in previous {
                tx.execute(
                    "INSERT OR IGNORE INTO sync_retired_objects(cid, transport_instance_id, state_sequence)
                     SELECT cid, transport_instance_id, ?2 FROM sync_deliveries WHERE cid=?1 AND state='delivered'",
                    params![cid, state_sequence],
                )?;
                tx.execute("DELETE FROM sync_deliveries WHERE cid=?1", params![cid])?;
                tx.execute("DELETE FROM sync_objects WHERE cid=?1", params![cid])?;
            }

            let chunk_count = chunks.len() as i64;
            let mut chunk_cids = Vec::with_capacity(chunks.len());
            let mut objects: Vec<(String, &str, i64, i64, Vec<u8>)> = Vec::with_capacity(chunks.len() + 1);
            for (index, chunk) in chunks.iter().enumerate() {
                let cid = compute_cid(chunk);
                objects.push((cid.clone(), "state_chunk", index as i64, chunk_count, chunk.clone()));
                chunk_cids.push(cid);
            }
            let index_bytes = serde_json::to_vec(&ChunkIndex { chunk_cids }).map_err(serialization_error)?;
            objects.push((compute_cid(&index_bytes), "state_index", 0, 1, index_bytes));
            for (cid, kind, chunk_index, chunk_count, bytes) in objects {
                tx.execute(
                    "INSERT OR IGNORE INTO sync_objects(cid,object_kind,state_sequence,chunk_index,chunk_count,bytes) VALUES (?1,?2,?3,?4,?5,?6)",
                    params![cid, kind, next as i64, chunk_index, chunk_count, bytes],
                )?;
                for transport_id in transports {
                    tx.execute(
                        "INSERT OR IGNORE INTO sync_deliveries(cid,transport_instance_id,state,attempts) VALUES (?1,?2,'pending',0)",
                        params![cid, transport_id.0],
                    )?;
                }
            }
            tx.execute(
                "UPDATE sync_local_state
                 SET state_sequence=?1, sealed_epoch=?2
                 WHERE id=1",
                params![next as i64, key_epoch],
            )?;
            super::sync_state::clear_dirty_if_generation(tx, snapshot_generation)?;
            Ok(())
        })
    }

    /// Superseded snapshot objects still on `transport_instance_id` from
    /// before state sequence `head_sequence`.
    pub(crate) fn retired_objects(&self, transport_instance_id: &str, head_sequence: u64) -> DbResult<Vec<String>> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare("SELECT cid FROM sync_retired_objects WHERE transport_instance_id=?1 AND state_sequence < ?2")?;
            let rows = statement
                .query_map(params![transport_instance_id, head_sequence as i64], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    /// Forgets a retired object once it is gone from `transport_instance_id`.
    pub(crate) fn forget_retired_object(&self, cid: &str, transport_instance_id: &str) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "DELETE FROM sync_retired_objects WHERE cid=?1 AND transport_instance_id=?2",
                params![cid, transport_instance_id],
            )?;
            Ok(())
        })
    }

    /// Records a failed delivery attempt as decided by the caller's retry
    /// policy: still `pending` with a retry time, or `failed`.
    pub(crate) fn store_delivery_failure(
        &self,
        cid: &str,
        transport_instance_id: &str,
        state: &str,
        attempts: i64,
        retry_at: Option<String>,
        error: &str,
    ) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE sync_deliveries SET state=?3, attempts=?4, retry_at=?5, last_error=?6
                 WHERE cid=?1 AND transport_instance_id=?2",
                params![cid, transport_instance_id, state, attempts, retry_at, error],
            )?;
            Ok(())
        })
    }

    /// Writes a connector's non-secret config, enabling it.
    pub(crate) fn upsert_transport_row(&self, instance_id: &str, kind: &str, config_json: &str) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO sync_transports(instance_id,kind,config_json,required,enabled) VALUES (?1,?2,?3,1,1)
                 ON CONFLICT(instance_id) DO UPDATE SET kind=excluded.kind, config_json=excluded.config_json, enabled=1",
                params![instance_id, kind, config_json],
            )?;
            Ok(())
        })
    }

    pub(crate) fn set_transport_config_json(&self, instance_id: &str, config_json: &str) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE sync_transports SET config_json=?2 WHERE instance_id=?1",
                params![instance_id, config_json],
            )?;
            Ok(())
        })
    }

    /// Forgets a connector's row and every ledger row that names it.
    pub(crate) fn delete_transport_rows(&self, instance_id: &str) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute("DELETE FROM sync_transports WHERE instance_id=?1", params![instance_id])?;
            connection.execute("DELETE FROM sync_deliveries WHERE transport_instance_id=?1", params![instance_id])?;
            connection.execute("DELETE FROM sync_head_publications WHERE transport_instance_id=?1", params![instance_id])?;
            connection.execute("DELETE FROM sync_retired_objects WHERE transport_instance_id=?1", params![instance_id])?;
            Ok(())
        })
    }
}
