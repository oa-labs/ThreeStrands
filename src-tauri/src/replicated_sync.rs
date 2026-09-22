//! Local persistence and projection scaffolding for the pluggable
//! replicated-sync engine (see `docs`/the sync rewrite plan, Phase 2).
//!
//! This module owns the local operation graph: one row per logical event
//! (`sync_events`), one row per field-level write (`sync_operations`), its
//! parent edges (`sync_operation_parents`), and the current frontier per
//! field (`sync_field_frontier`) — the same model implemented and property
//! tested in `crates/sync-core`, here reduced to its local-only case: every
//! operation recorded through this module originates on this device, so a
//! write's parents are always exactly the field's current frontier and can
//! never already be "consumed" by an unseen child. The general (possibly
//! out-of-order, possibly duplicate) case belongs to a future phase's
//! remote-apply path, which can reuse `threestrands_sync_core::OperationGraph`
//! directly instead of this module's simpler SQL.
//!
//! Entirely inert unless [`enabled`] returns `true` (gated by the
//! `THREESTRANDS_REPLICATED_SYNC` environment variable, unset by default):
//! no shipped behavior changes, and no application version bump is owed for
//! this phase. `Database::enqueue_cloud_entity` and
//! `Database::enqueue_cloud_deletion` call into this module at the end of
//! their existing bodies without changing their signatures, so no mutation
//! call site in `lib.rs` changes.
//!
//! The rest of this module (from "Key material" on) is the Phase 3/4
//! replicator: sealing local events, delivering them to configured
//! transports, pulling and applying remote events, anti-entropy repair, and
//! health aggregation, plus the folder transport (`sync_folder.rs`). Unlike
//! the section above, `apply_remote_operation` implements the *general*
//! operation-graph algorithm (out-of-order and duplicate tolerant), because
//! a remote origin genuinely can deliver a child before its parent.

use std::collections::BTreeSet;
use std::str::FromStr;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use chrono::Utc;
use keyring::Entry;
use rand::{rngs::OsRng, RngCore};
use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use threestrands_sync_core::{EntityType, WinnerStamp, ENTITY_EXISTENCE_FIELD};
use threestrands_sync_envelope::{
    compute_cid, open_message, seal_event, sign_device_head, verify_device_head, DeviceHead,
    DeviceId as EnvelopeDeviceId, EventId as EnvelopeEventId, FieldOperation, ObjectKind,
    OpenParams, OperationId as EnvelopeOperationId, SealParams, SignedDeviceHead, SigningKey,
    SyncEvent, UnsignedSyncEvent, VerifyingKey,
};
use threestrands_sync_transport::{
    Cid as TransportCid, HeadLocator, SyncTransport, TransportError, TransportHealth,
    TransportInstanceId,
};

use crate::db::Database;

/// The single local sync space Phase 2 supports. Multiple concurrent spaces
/// are not a product concept yet; this is simply a stable primary key.
const SPACE_ID: &str = "default";

/// Whether the replicated-sync engine is active. Disabled by default so this
/// entire phase ships inert; set `THREESTRANDS_REPLICATED_SYNC=1` (or any
/// other non-empty value other than `0`/`false`) to exercise it.
pub fn enabled() -> bool {
    parse_flag(std::env::var("THREESTRANDS_REPLICATED_SYNC").ok().as_deref())
}

fn parse_flag(value: Option<&str>) -> bool {
    match value.map(str::trim) {
        None | Some("") => false,
        Some(value) => !value.eq_ignore_ascii_case("0") && !value.eq_ignore_ascii_case("false"),
    }
}

/// The outcome of checking whether a resolved, validated entity is ready to
/// materialize into application tables. Mirrors the plan's projection
/// contract: "Materialize parent entities before dependent entities... A
/// calendar selection that arrives before its calendar account remains
/// pending and is retried after the account materializes." Reuses the exact
/// same dependency check `cloud_sync::upsert_cloud_calendar_selection`
/// already performs for the legacy system.
// No production caller until a transport actually delivers remote
// operations to project (a later phase); exercised directly by this
// module's tests in the meantime.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectionReadiness {
    Ready,
    Pending { reason: String },
}

impl Database {
    /// Records a local field-level write (creation or update) into the
    /// replicated-sync operation graph, in one transaction: device-sequence
    /// increment, lamport increment, event insert, one operation per field,
    /// and frontier maintenance. A no-op while a remote projection is being
    /// applied (see [`Self::with_remote_projection`]), so applying an
    /// already-authenticated remote write can reuse the same materializer
    /// path this module will grow without creating an echo.
    ///
    /// `fields` and `payload` are exactly what `enqueue_cloud_entity`
    /// already computes for the legacy outbox, reused as-is.
    pub fn record_replicated_write(
        &self,
        entity_type: EntityType,
        entity_id: &str,
        fields: &BTreeSet<String>,
        payload: &Value,
    ) -> Result<(), String> {
        if self.is_projecting_remote_operation() {
            return Ok(());
        }
        let mut connection = self.connection()?;
        let tx = connection.transaction().map_err(display)?;
        let device_id = ensure_space_and_device(&tx)?;
        let creating = !entity_has_any_operation(&tx, entity_type, entity_id)?;
        let (event_id_hex, event_id, lamport) = begin_event(&tx, device_id)?;

        if creating {
            apply_field_operation(
                &tx,
                entity_type,
                entity_id,
                ENTITY_EXISTENCE_FIELD,
                Some(Value::Bool(true)),
                &event_id_hex,
                device_id,
                event_id,
                lamport,
            )?;
        }
        for field in fields {
            if field == "*" {
                continue;
            }
            let value = payload.get(field).cloned();
            apply_field_operation(
                &tx,
                entity_type,
                entity_id,
                field,
                value,
                &event_id_hex,
                device_id,
                event_id,
                lamport,
            )?;
        }

        tx.commit().map_err(display)
    }

    /// Records a local deletion: an ordinary write of `_entity = false`, the
    /// reserved existence field. See the module doc for why deletion never
    /// touches any other field's frontier.
    pub fn record_replicated_deletion(&self, entity_type: EntityType, entity_id: &str) -> Result<(), String> {
        if self.is_projecting_remote_operation() {
            return Ok(());
        }
        let mut connection = self.connection()?;
        let tx = connection.transaction().map_err(display)?;
        let device_id = ensure_space_and_device(&tx)?;
        let (event_id_hex, event_id, lamport) = begin_event(&tx, device_id)?;
        apply_field_operation(
            &tx,
            entity_type,
            entity_id,
            ENTITY_EXISTENCE_FIELD,
            Some(Value::Bool(false)),
            &event_id_hex,
            device_id,
            event_id,
            lamport,
        )?;
        tx.commit().map_err(display)
    }

    /// True while an already-authenticated remote (or conflict-resolution)
    /// operation is being applied. A shared materializer checks this before
    /// calling [`Self::record_replicated_write`] / [`Self::record_replicated_deletion`]
    /// so projecting a remote write never re-enqueues it as a new local
    /// event — the echo-prevention the plan calls for. Nothing sets this yet
    /// outside tests; a future transport-aware phase wraps its projection
    /// application in [`Self::with_remote_projection`].
    pub(crate) fn is_projecting_remote_operation(&self) -> bool {
        self.replicated_sync_projecting.load(Ordering::SeqCst)
    }

    /// Runs `work` with remote-projection suppression engaged. Always
    /// restores the flag afterward, including when `work` returns an error.
    /// No production caller until a transport actually delivers remote
    /// operations to project; exercised directly by this module's tests.
    #[allow(dead_code)]
    pub(crate) fn with_remote_projection<R>(&self, work: impl FnOnce() -> Result<R, String>) -> Result<R, String> {
        self.replicated_sync_projecting.store(true, Ordering::SeqCst);
        let result = work();
        self.replicated_sync_projecting.store(false, Ordering::SeqCst);
        result
    }

    /// Validates a fully resolved entity payload and checks any known
    /// materialization dependency, without writing anything. A future
    /// projection path calls this before invoking the real per-entity
    /// upsert, and retries later on [`ProjectionReadiness::Pending`] rather
    /// than treating a not-yet-materialized dependency as an error. No
    /// production caller until a transport exists; exercised directly by
    /// this module's tests.
    #[allow(dead_code)]
    pub(crate) fn check_projection_readiness(
        &self,
        entity_type: EntityType,
        payload: &Value,
    ) -> Result<ProjectionReadiness, String> {
        entity_type.validate_payload(payload)?;
        if entity_type == EntityType::CalendarSelection {
            let account_id = payload.get("accountId").and_then(Value::as_str).unwrap_or_default();
            let known = self
                .list_calendar_accounts()?
                .iter()
                .any(|account| account.email == account_id);
            if !known {
                return Ok(ProjectionReadiness::Pending {
                    reason: format!("calendar account {account_id} has not materialized yet"),
                });
            }
        }
        Ok(ProjectionReadiness::Ready)
    }
}

fn ensure_space_and_device(tx: &Transaction) -> Result<[u8; 16], String> {
    tx.execute(
        "INSERT OR IGNORE INTO sync_spaces(id, active_epoch, lamport, enabled) VALUES (?1, 0, 0, 1)",
        params![SPACE_ID],
    )
    .map_err(display)?;
    let existing: Option<String> = tx
        .query_row("SELECT device_id FROM sync_devices LIMIT 1", [], |row| row.get(0))
        .optional()
        .map_err(display)?;
    if let Some(hex) = existing {
        return decode_id(&hex);
    }
    let device_id = random_id();
    tx.execute(
        "INSERT INTO sync_devices(device_id, status) VALUES (?1, 'active')",
        params![encode_id(&device_id)],
    )
    .map_err(display)?;
    Ok(device_id)
}

fn entity_has_any_operation(tx: &Transaction, entity_type: EntityType, entity_id: &str) -> Result<bool, String> {
    tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_operations WHERE entity_type=?1 AND entity_id=?2)",
        params![entity_type.as_str(), entity_id],
        |row| row.get(0),
    )
    .map_err(display)
}

/// Bumps this device's sequence and the space's lamport, inserts the event
/// row, and returns identifiers every operation in the event shares.
fn begin_event(tx: &Transaction, device_id: [u8; 16]) -> Result<(String, [u8; 16], u64), String> {
    let device_id_hex = encode_id(&device_id);
    let device_sequence: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(device_sequence),0)+1 FROM sync_events WHERE device_id=?1",
            params![device_id_hex],
            |row| row.get(0),
        )
        .map_err(display)?;
    let lamport: i64 = tx
        .query_row(
            "UPDATE sync_spaces SET lamport = lamport + 1 WHERE id=?1 RETURNING lamport",
            params![SPACE_ID],
            |row| row.get(0),
        )
        .map_err(display)?;
    let event_id = random_id();
    let event_id_hex = encode_id(&event_id);
    tx.execute(
        "INSERT INTO sync_events(event_id, epoch, device_id, device_sequence, lamport, state, created_at)
         VALUES (?1,0,?2,?3,?4,'recorded',?5)",
        params![event_id_hex, device_id_hex, device_sequence, lamport, Utc::now().to_rfc3339()],
    )
    .map_err(display)?;
    Ok((event_id_hex, event_id, lamport as u64))
}

/// Names the current frontier as parents, inserts the new operation and its
/// parent edges, and replaces the field's frontier with just this operation
/// — correct because every caller is local-only (see the module doc).
#[allow(clippy::too_many_arguments)]
fn apply_field_operation(
    tx: &Transaction,
    entity_type: EntityType,
    entity_id: &str,
    field: &str,
    value: Option<Value>,
    event_id_hex: &str,
    device_id: [u8; 16],
    event_id: [u8; 16],
    lamport: u64,
) -> Result<(), String> {
    let parents: Vec<String> = {
        let mut statement = tx
            .prepare(
                "SELECT operation_id FROM sync_field_frontier
                 WHERE entity_type=?1 AND entity_id=?2 AND field=?3",
            )
            .map_err(display)?;
        let rows = statement
            .query_map(params![entity_type.as_str(), entity_id, field], |row| row.get(0))
            .map_err(display)?
            .collect::<Result<_, _>>()
            .map_err(display)?;
        rows
    };

    let operation_id = random_id();
    let operation_id_hex = encode_id(&operation_id);
    let stamp = WinnerStamp {
        lamport,
        device_id,
        event_id,
        operation_id,
    };
    tx.execute(
        "INSERT INTO sync_operations(operation_id, event_id, entity_type, entity_id, field, value, winner_stamp)
         VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![
            operation_id_hex,
            event_id_hex,
            entity_type.as_str(),
            entity_id,
            field,
            value.as_ref().map(Value::to_string),
            encode_winner_stamp(&stamp),
        ],
    )
    .map_err(display)?;

    for parent in &parents {
        tx.execute(
            "INSERT INTO sync_operation_parents(operation_id, parent_operation_id) VALUES (?1,?2)",
            params![operation_id_hex, parent],
        )
        .map_err(display)?;
    }
    tx.execute(
        "DELETE FROM sync_field_frontier WHERE entity_type=?1 AND entity_id=?2 AND field=?3",
        params![entity_type.as_str(), entity_id, field],
    )
    .map_err(display)?;
    tx.execute(
        "INSERT INTO sync_field_frontier(entity_type, entity_id, field, operation_id) VALUES (?1,?2,?3,?4)",
        params![entity_type.as_str(), entity_id, field, operation_id_hex],
    )
    .map_err(display)?;
    Ok(())
}

fn random_id() -> [u8; 16] {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    bytes
}

fn encode_id(bytes: &[u8; 16]) -> String {
    hex_encode(bytes)
}

fn decode_id(hex: &str) -> Result<[u8; 16], String> {
    let bytes = hex_decode(hex)?;
    bytes
        .try_into()
        .map_err(|_| "Invalid replicated-sync identifier".to_string())
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hex_decode(hex: &str) -> Result<Vec<u8>, String> {
    if !hex.len().is_multiple_of(2) {
        return Err("Invalid replicated-sync hex value".to_string());
    }
    (0..hex.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&hex[index..index + 2], 16)
                .map_err(|_| "Invalid replicated-sync hex value".to_string())
        })
        .collect()
}

fn encode_winner_stamp(stamp: &WinnerStamp) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(8 + 16 + 16 + 16);
    bytes.extend_from_slice(&stamp.lamport.to_be_bytes());
    bytes.extend_from_slice(&stamp.device_id);
    bytes.extend_from_slice(&stamp.event_id);
    bytes.extend_from_slice(&stamp.operation_id);
    bytes
}

// ============================== Key material ==============================

const KEYCHAIN_SERVICE: &str = "app.threestrands.replicated-sync";
const SIGNING_KEY_ENTRY: &str = "device-signing-key";
const EPOCH_KEY_ENTRY: &str = "epoch-key-0";

/// The minimal single-device key material push/pull need to seal and open
/// messages for real: an Ed25519 device signing key and a symmetric epoch
/// key, generated once and kept in the OS keychain.
///
/// This is deliberately **not** the real key hierarchy: there is no sealed
/// distribution of the epoch key to other devices, no rotation, and no
/// recovery phrase. That is a later phase's job (key recovery and
/// enrollment). This is just enough for one device to exercise real
/// encryption end to end, and for a second *test* device (with its own
/// synthetic `LocalKeys`, trusted via [`Database::trust_device_public_key`])
/// to prove the general multi-device path works.
pub struct LocalKeys {
    pub signing_key: SigningKey,
    pub verifying_key: VerifyingKey,
    pub k_epoch: [u8; 32],
    pub key_epoch: u32,
    pub device_id: EnvelopeDeviceId,
    pub sync_space_id: Vec<u8>,
}

impl Database {
    /// Loads this device's replicated-sync key material, provisioning it on
    /// first use. Touches the OS keychain — never call this from a test;
    /// tests build a [`LocalKeys`] directly and pass it to push/pull.
    pub fn local_replicated_keys(&self) -> Result<LocalKeys, String> {
        let device_id = {
            let mut connection = self.connection()?;
            let tx = connection.transaction().map_err(display)?;
            let device_id = ensure_space_and_device(&tx)?;
            tx.commit().map_err(display)?;
            device_id
        };
        let signing_key = load_or_create_signing_key()?;
        let k_epoch = load_or_create_epoch_key()?;
        let verifying_key = signing_key.verifying_key();
        self.trust_device_public_key(&device_id, &verifying_key)?;
        Ok(LocalKeys {
            verifying_key,
            signing_key,
            k_epoch,
            key_epoch: 0,
            device_id: EnvelopeDeviceId::from_bytes(device_id),
            sync_space_id: SPACE_ID.as_bytes().to_vec(),
        })
    }

    /// Records a device's public key as trusted for signature verification.
    /// For our own device, [`Self::local_replicated_keys`] calls this
    /// automatically. Trusting another device's key is enrollment's job (a
    /// later phase); tests call this directly to simulate an already
    /// completed enrollment.
    pub fn trust_device_public_key(&self, device_id: &[u8; 16], verifying_key: &VerifyingKey) -> Result<(), String> {
        let connection = self.connection()?;
        connection
            .execute(
                "INSERT INTO sync_devices(device_id, public_key, status) VALUES (?1,?2,'active')
                 ON CONFLICT(device_id) DO UPDATE SET public_key=excluded.public_key",
                params![encode_id(device_id), verifying_key.to_bytes().to_vec()],
            )
            .map_err(display)?;
        Ok(())
    }

    /// Every device this local database currently trusts a public key for.
    fn known_device_roster(&self) -> Result<Vec<(EnvelopeDeviceId, VerifyingKey)>, String> {
        let connection = self.connection()?;
        let rows: Vec<(String, Vec<u8>)> = {
            let mut statement = connection
                .prepare("SELECT device_id, public_key FROM sync_devices WHERE public_key IS NOT NULL")
                .map_err(display)?;
            let rows = statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(display)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(display)?;
            rows
        };
        rows.into_iter()
            .map(|(device_id_hex, key_bytes)| {
                let device_id = EnvelopeDeviceId::from_bytes(decode_id(&device_id_hex)?);
                let key_array: [u8; 32] = key_bytes
                    .try_into()
                    .map_err(|_| "Stored device public key is invalid".to_string())?;
                let verifying_key = VerifyingKey::from_bytes(&key_array)
                    .map_err(|_| "Stored device public key is invalid".to_string())?;
                Ok((device_id, verifying_key))
            })
            .collect()
    }
}

fn load_or_create_signing_key() -> Result<SigningKey, String> {
    let entry = Entry::new(KEYCHAIN_SERVICE, SIGNING_KEY_ENTRY).map_err(display)?;
    match entry.get_password() {
        Ok(hex) => {
            let bytes: [u8; 32] = hex_decode(&hex)?
                .try_into()
                .map_err(|_| "Stored device signing key is invalid".to_string())?;
            Ok(SigningKey::from_bytes(&bytes))
        }
        Err(keyring::Error::NoEntry) => {
            let mut seed = [0u8; 32];
            OsRng.fill_bytes(&mut seed);
            entry.set_password(&hex_encode(&seed)).map_err(display)?;
            Ok(SigningKey::from_bytes(&seed))
        }
        Err(error) => Err(display(error)),
    }
}

fn load_or_create_epoch_key() -> Result<[u8; 32], String> {
    let entry = Entry::new(KEYCHAIN_SERVICE, EPOCH_KEY_ENTRY).map_err(display)?;
    match entry.get_password() {
        Ok(hex) => hex_decode(&hex)?
            .try_into()
            .map_err(|_| "Stored epoch key is invalid".to_string()),
        Err(keyring::Error::NoEntry) => {
            let mut key = [0u8; 32];
            OsRng.fill_bytes(&mut key);
            entry.set_password(&hex_encode(&key)).map_err(display)?;
            Ok(key)
        }
        Err(error) => Err(display(error)),
    }
}

// ================================ Sealing ==================================

/// The discovery hint an event's `previous_device_event`/a device head's
/// `latest_event_cid` actually point to: not a chunk's CID directly (a
/// multi-chunk message has several, and there is no way to derive the rest
/// from just one), but this small, unauthenticated index listing every
/// chunk CID in order. It needs no signature of its own: every chunk is
/// independently AEAD-authenticated, every chunk in one message shares an
/// authenticated hash of the complete reassembled plaintext, and the
/// reassembled event itself carries a device signature — a forged or
/// corrupted index can only ever make `open_message` fail, never make a
/// wrong message succeed.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct ChunkIndex {
    chunk_cids: Vec<String>,
}

impl Database {
    /// Seals every locally recorded event not yet sealed: builds its
    /// encrypted chunks and chunk index, stores them in `sync_objects`, and
    /// creates one pending delivery row per object per transport instance
    /// in `transports`. Pure local bookkeeping — no network I/O.
    pub fn seal_pending_events(&self, keys: &LocalKeys, transports: &[TransportInstanceId]) -> Result<usize, String> {
        let pending: Vec<(String, String, i64)> = {
            let connection = self.connection()?;
            let mut statement = connection
                .prepare("SELECT event_id, device_id, device_sequence FROM sync_events WHERE state='recorded' ORDER BY device_sequence")
                .map_err(display)?;
            let rows = statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .map_err(display)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(display)?;
            rows
        };

        for (event_id_hex, device_id_hex, device_sequence) in &pending {
            self.seal_one_event(event_id_hex, device_id_hex, *device_sequence, keys, transports)?;
        }
        Ok(pending.len())
    }

    fn seal_one_event(
        &self,
        event_id_hex: &str,
        device_id_hex: &str,
        device_sequence: i64,
        keys: &LocalKeys,
        transports: &[TransportInstanceId],
    ) -> Result<(), String> {
        let mut connection = self.connection()?;
        let tx = connection.transaction().map_err(display)?;

        let operations = load_operations_for_event(&tx, event_id_hex)?;
        let lamport: i64 = tx
            .query_row("SELECT lamport FROM sync_events WHERE event_id=?1", params![event_id_hex], |row| row.get(0))
            .map_err(display)?;
        let previous_device_event: Option<String> = if device_sequence > 1 {
            tx.query_row(
                "SELECT so.cid FROM sync_objects so JOIN sync_events se ON se.event_id = so.event_id
                 WHERE se.device_id=?1 AND se.device_sequence=?2 AND so.object_kind='chunk_index'",
                params![device_id_hex, device_sequence - 1],
                |row| row.get(0),
            )
            .optional()
            .map_err(display)?
        } else {
            None
        };

        let unsigned = UnsignedSyncEvent {
            event_id: EnvelopeEventId::from_bytes(decode_id(event_id_hex)?),
            protocol_version: 1,
            key_epoch: keys.key_epoch,
            device_id: keys.device_id,
            device_sequence: device_sequence as u64,
            previous_device_event,
            lamport: lamport as u64,
            created_at_ms: Utc::now().timestamp_millis(),
            operations,
        };

        let (_, sealed) = seal_event(
            unsigned,
            &SealParams {
                sync_space_id: &keys.sync_space_id,
                k_epoch: &keys.k_epoch,
                key_epoch: keys.key_epoch,
                object_kind: ObjectKind::Operations,
                signing_key: &keys.signing_key,
            },
        )
        .map_err(display)?;

        let chunk_count = sealed.chunks.len() as i64;
        let mut chunk_cids = Vec::with_capacity(sealed.chunks.len());
        for (index, chunk) in sealed.chunks.iter().enumerate() {
            let cid = compute_cid(chunk);
            tx.execute(
                "INSERT OR IGNORE INTO sync_objects(cid,event_id,object_kind,chunk_index,chunk_count,bytes) VALUES (?1,?2,'operations',?3,?4,?5)",
                params![cid, event_id_hex, index as i64, chunk_count, chunk],
            )
            .map_err(display)?;
            for transport_id in transports {
                tx.execute(
                    "INSERT OR IGNORE INTO sync_deliveries(cid,transport_instance_id,state,attempts) VALUES (?1,?2,'pending',0)",
                    params![cid, transport_id.0],
                )
                .map_err(display)?;
            }
            chunk_cids.push(cid);
        }

        let index_bytes = serde_json::to_vec(&ChunkIndex { chunk_cids }).map_err(display)?;
        let index_cid = compute_cid(&index_bytes);
        tx.execute(
            "INSERT OR IGNORE INTO sync_objects(cid,event_id,object_kind,chunk_index,chunk_count,bytes) VALUES (?1,?2,'chunk_index',0,1,?3)",
            params![index_cid, event_id_hex, index_bytes],
        )
        .map_err(display)?;
        for transport_id in transports {
            tx.execute(
                "INSERT OR IGNORE INTO sync_deliveries(cid,transport_instance_id,state,attempts) VALUES (?1,?2,'pending',0)",
                params![index_cid, transport_id.0],
            )
            .map_err(display)?;
        }

        tx.execute("UPDATE sync_events SET state='sealed' WHERE event_id=?1", params![event_id_hex])
            .map_err(display)?;
        tx.commit().map_err(display)
    }

    /// The greatest contiguous device-sequence (no gaps starting at 1) this
    /// device has sealed, and that event's chunk-index CID — the pair a
    /// signed device head publishes.
    fn contiguous_head(&self, device_id_hex: &str) -> Result<(u64, Option<String>), String> {
        let connection = self.connection()?;
        let sequences: Vec<i64> = {
            let mut statement = connection
                .prepare("SELECT device_sequence FROM sync_events WHERE device_id=?1 AND state='sealed' ORDER BY device_sequence")
                .map_err(display)?;
            let rows = statement
                .query_map(params![device_id_hex], |row| row.get(0))
                .map_err(display)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(display)?;
            rows
        };
        let mut contiguous = 0i64;
        for sequence in &sequences {
            if *sequence == contiguous + 1 {
                contiguous = *sequence;
            } else {
                break;
            }
        }
        if contiguous == 0 {
            return Ok((0, None));
        }
        let latest_event_cid: Option<String> = connection
            .query_row(
                "SELECT so.cid FROM sync_objects so JOIN sync_events se ON se.event_id = so.event_id
                 WHERE se.device_id=?1 AND se.device_sequence=?2 AND so.object_kind='chunk_index'",
                params![device_id_hex, contiguous],
                |row| row.get(0),
            )
            .optional()
            .map_err(display)?;
        Ok((contiguous as u64, latest_event_cid))
    }

    fn object_exists(&self, cid: &str) -> Result<bool, String> {
        self.connection()?
            .query_row("SELECT EXISTS(SELECT 1 FROM sync_objects WHERE cid=?1)", params![cid], |row| row.get(0))
            .map_err(display)
    }

    /// Stores a remotely fetched, already-authenticated message's chunk
    /// index and chunks locally, so a later chain walk recognizes it
    /// without re-fetching, and so anti-entropy repair can deliver it to
    /// another transport without going back to the transport it came from.
    fn remember_remote_message(
        &self,
        index_cid: &str,
        chunk_cids: &[String],
        chunks: &[Vec<u8>],
        event_id_hex: &str,
    ) -> Result<(), String> {
        let mut connection = self.connection()?;
        let tx = connection.transaction().map_err(display)?;
        let index_bytes = serde_json::to_vec(&ChunkIndex {
            chunk_cids: chunk_cids.to_vec(),
        })
        .map_err(display)?;
        tx.execute(
            "INSERT OR IGNORE INTO sync_objects(cid,event_id,object_kind,chunk_index,chunk_count,bytes) VALUES (?1,?2,'chunk_index',0,1,?3)",
            params![index_cid, event_id_hex, index_bytes],
        )
        .map_err(display)?;
        let chunk_count = chunks.len() as i64;
        for (index, (cid, bytes)) in chunk_cids.iter().zip(chunks.iter()).enumerate() {
            tx.execute(
                "INSERT OR IGNORE INTO sync_objects(cid,event_id,object_kind,chunk_index,chunk_count,bytes) VALUES (?1,?2,'operations',?3,?4,?5)",
                params![cid, event_id_hex, index as i64, chunk_count, bytes],
            )
            .map_err(display)?;
        }
        tx.commit().map_err(display)
    }
}

fn load_operations_for_event(tx: &Transaction, event_id_hex: &str) -> Result<Vec<FieldOperation>, String> {
    let rows: Vec<(String, String, String, String, Option<String>)> = {
        let mut statement = tx
            .prepare("SELECT operation_id, entity_type, entity_id, field, value FROM sync_operations WHERE event_id=?1")
            .map_err(display)?;
        let rows = statement
            .query_map(params![event_id_hex], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
            })
            .map_err(display)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(display)?;
        rows
    };

    let mut operations = Vec::with_capacity(rows.len());
    for (operation_id_hex, entity_type_str, entity_id, field, value_json) in rows {
        let parent_hexes: Vec<String> = {
            let mut statement = tx
                .prepare("SELECT parent_operation_id FROM sync_operation_parents WHERE operation_id=?1")
                .map_err(display)?;
            let rows = statement
                .query_map(params![operation_id_hex], |row| row.get(0))
                .map_err(display)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(display)?;
            rows
        };
        let parents = parent_hexes
            .into_iter()
            .map(|hex| decode_id(&hex).map(EnvelopeOperationId::from_bytes))
            .collect::<Result<Vec<_>, _>>()?;

        operations.push(FieldOperation {
            operation_id: EnvelopeOperationId::from_bytes(decode_id(&operation_id_hex)?),
            entity_type: EntityType::from_str(&entity_type_str)?,
            entity_id,
            field,
            value: value_json.map(|json| serde_json::from_str(&json)).transpose().map_err(display)?,
            parents,
        });
    }
    Ok(operations)
}

// =============================== Delivery ===================================

/// A failed delivery is retried this many times (with exponential backoff)
/// before it is treated as settled-failed rather than pending.
const MAX_DELIVERY_ATTEMPTS: i64 = 20;

struct DeliveryItem {
    cid: String,
    bytes: Vec<u8>,
    attempts: i64,
}

impl Database {
    fn pending_delivery_items(&self, transport_instance_id: &str) -> Result<Vec<DeliveryItem>, String> {
        let connection = self.connection()?;
        let now = Utc::now().to_rfc3339();
        let mut statement = connection
            .prepare(
                "SELECT sd.cid, so.bytes, sd.attempts
                 FROM sync_deliveries sd JOIN sync_objects so ON so.cid = sd.cid
                 WHERE sd.transport_instance_id=?1 AND sd.state='pending'
                   AND (sd.retry_at IS NULL OR sd.retry_at <= ?2)",
            )
            .map_err(display)?;
        let rows = statement
            .query_map(params![transport_instance_id, now], |row| {
                Ok(DeliveryItem {
                    cid: row.get(0)?,
                    bytes: row.get(1)?,
                    attempts: row.get(2)?,
                })
            })
            .map_err(display)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(display)?;
        Ok(rows)
    }

    fn record_delivery_success(&self, cid: &str, transport_instance_id: &str, remote_id: Option<&str>) -> Result<(), String> {
        self.connection()?
            .execute(
                "UPDATE sync_deliveries SET state='delivered', remote_id=?3, last_error=NULL, retry_at=NULL
                 WHERE cid=?1 AND transport_instance_id=?2",
                params![cid, transport_instance_id, remote_id],
            )
            .map_err(display)?;
        Ok(())
    }

    fn record_delivery_failure(
        &self,
        cid: &str,
        transport_instance_id: &str,
        attempts: i64,
        error: &TransportError,
    ) -> Result<(), String> {
        let next_attempts = attempts + 1;
        let (state, retry_at) = if error.is_retryable() && next_attempts < MAX_DELIVERY_ATTEMPTS {
            ("pending", Some(backoff_retry_at(next_attempts)))
        } else {
            ("failed", None)
        };
        self.connection()?
            .execute(
                "UPDATE sync_deliveries SET state=?3, attempts=?4, retry_at=?5, last_error=?6
                 WHERE cid=?1 AND transport_instance_id=?2",
                params![cid, transport_instance_id, state, next_attempts, retry_at, error.to_string()],
            )
            .map_err(display)?;
        Ok(())
    }

    /// Ensures a pending delivery row exists for every locally known object
    /// on every transport in `transport_ids` that doesn't already have one
    /// (pending, delivered, or failed) — the anti-entropy behavior that
    /// turns transport union into replication. Cheap to call repeatedly:
    /// `INSERT OR IGNORE` only ever adds rows for a truly new pairing.
    pub fn enqueue_repair_deliveries(&self, transport_ids: &[TransportInstanceId]) -> Result<usize, String> {
        let connection = self.connection()?;
        let cids: Vec<String> = {
            let mut statement = connection.prepare("SELECT cid FROM sync_objects").map_err(display)?;
            let rows = statement
                .query_map([], |row| row.get(0))
                .map_err(display)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(display)?;
            rows
        };
        let mut created = 0;
        for cid in &cids {
            for transport_id in transport_ids {
                created += connection
                    .execute(
                        "INSERT OR IGNORE INTO sync_deliveries(cid,transport_instance_id,state,attempts) VALUES (?1,?2,'pending',0)",
                        params![cid, transport_id.0],
                    )
                    .map_err(display)?;
            }
        }
        Ok(created)
    }

    /// Pending/delivered/failed delivery counts for one transport instance,
    /// for the Settings UI.
    pub fn delivery_counts(&self, transport_instance_id: &str) -> Result<(i64, i64, i64), String> {
        let connection = self.connection()?;
        let count = |state: &str| -> Result<i64, String> {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM sync_deliveries WHERE transport_instance_id=?1 AND state=?2",
                    params![transport_instance_id, state],
                    |row| row.get(0),
                )
                .map_err(display)
        };
        Ok((count("pending")?, count("delivered")?, count("failed")?))
    }
}

/// Exponential backoff with jitter, capped at one hour. `attempts` is the
/// number of failed attempts so far (>= 1).
fn backoff_retry_at(attempts: i64) -> String {
    let base_secs = 5i64.saturating_mul(1i64 << attempts.clamp(0, 12));
    let capped = base_secs.min(3600);
    let jitter_ms = (rand::random::<u32>() % 1000) as i64;
    let delay = chrono::Duration::seconds(capped) + chrono::Duration::milliseconds(jitter_ms);
    (Utc::now() + delay).to_rfc3339()
}

/// The result of one push cycle.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PushOutcome {
    pub sealed_events: usize,
    pub delivered: usize,
    pub failed: usize,
}

/// Seals every pending local event, then attempts delivery to every
/// transport concurrently. Never holds a SQLite transaction across a
/// `put_object` call: sealing commits first, and each delivery outcome is
/// recorded in its own short transaction after the network call returns.
pub async fn push_pending_events(
    database: &Database,
    keys: &LocalKeys,
    transports: &[Arc<dyn SyncTransport>],
) -> Result<PushOutcome, String> {
    let instance_ids: Vec<TransportInstanceId> = transports.iter().map(|transport| transport.instance_id()).collect();
    let sealed_events = database.seal_pending_events(keys, &instance_ids)?;

    let mut delivered = 0usize;
    let mut failed = 0usize;
    let mut tasks = tokio::task::JoinSet::new();
    for transport in transports {
        let transport = Arc::clone(transport);
        let items = database.pending_delivery_items(&transport.instance_id().0)?;
        tasks.spawn(async move {
            let mut outcomes = Vec::with_capacity(items.len());
            for item in items {
                let cid = TransportCid(item.cid.clone());
                let result = transport.put_object(&cid, &item.bytes).await;
                outcomes.push((item, transport.instance_id(), result));
            }
            outcomes
        });
    }
    while let Some(result) = tasks.join_next().await {
        let outcomes = result.map_err(display)?;
        for (item, instance_id, outcome) in outcomes {
            match outcome {
                Ok(locator) => {
                    database.record_delivery_success(&item.cid, &instance_id.0, locator.remote_id.as_deref())?;
                    delivered += 1;
                }
                Err(error) => {
                    database.record_delivery_failure(&item.cid, &instance_id.0, item.attempts, &error)?;
                    failed += 1;
                }
            }
        }
    }

    publish_local_head(database, keys, transports).await;
    Ok(PushOutcome {
        sealed_events,
        delivered,
        failed,
    })
}

/// Publishes this device's current signed head to every transport,
/// best-effort: one transport failing to accept the head never blocks
/// publishing to the others, and never fails the push cycle.
async fn publish_local_head(database: &Database, keys: &LocalKeys, transports: &[Arc<dyn SyncTransport>]) {
    let device_id_hex = encode_id(keys.device_id.as_bytes());
    let Ok((contiguous_sequence, latest_event_cid)) = database.contiguous_head(&device_id_hex) else {
        return;
    };
    if contiguous_sequence == 0 {
        return;
    }
    let head = DeviceHead {
        sync_space_id: keys.sync_space_id.clone(),
        device_id: keys.device_id,
        epoch: keys.key_epoch,
        contiguous_sequence,
        latest_event_cid,
    };
    let Ok(signed) = sign_device_head(&keys.signing_key, head) else {
        return;
    };
    for transport in transports {
        let _ = transport.publish_head(&signed).await;
    }
}

// ============================ Remote apply / pull ============================

impl Database {
    /// Applies one already-authenticated remote field operation to the
    /// local graph, tolerating out-of-order and duplicate delivery —
    /// idempotent by `operation_id`. This is the general case; contrast
    /// with `apply_field_operation` above, which assumes local-only,
    /// always-in-order writes.
    #[allow(clippy::too_many_arguments)]
    fn apply_remote_operation(
        tx: &Transaction,
        entity_type: EntityType,
        entity_id: &str,
        field: &str,
        value: Option<&Value>,
        event_id_hex: &str,
        operation_id_hex: &str,
        parents: &[String],
        stamp: &WinnerStamp,
    ) -> Result<bool, String> {
        let already_known: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_operations WHERE operation_id=?1)",
                params![operation_id_hex],
                |row| row.get(0),
            )
            .map_err(display)?;
        if already_known {
            return Ok(false);
        }

        tx.execute(
            "INSERT INTO sync_operations(operation_id,event_id,entity_type,entity_id,field,value,winner_stamp) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                operation_id_hex,
                event_id_hex,
                entity_type.as_str(),
                entity_id,
                field,
                value.map(Value::to_string),
                encode_winner_stamp(stamp),
            ],
        )
        .map_err(display)?;

        for parent in parents {
            tx.execute(
                "INSERT OR IGNORE INTO sync_operation_parents(operation_id, parent_operation_id) VALUES (?1,?2)",
                params![operation_id_hex, parent],
            )
            .map_err(display)?;
            tx.execute(
                "DELETE FROM sync_field_frontier WHERE entity_type=?1 AND entity_id=?2 AND field=?3 AND operation_id=?4",
                params![entity_type.as_str(), entity_id, field, parent],
            )
            .map_err(display)?;
        }

        let consumed: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_operation_parents WHERE parent_operation_id=?1)",
                params![operation_id_hex],
                |row| row.get(0),
            )
            .map_err(display)?;
        if !consumed {
            tx.execute(
                "INSERT OR IGNORE INTO sync_field_frontier(entity_type, entity_id, field, operation_id) VALUES (?1,?2,?3,?4)",
                params![entity_type.as_str(), entity_id, field, operation_id_hex],
            )
            .map_err(display)?;
        }
        Ok(true)
    }

    /// Applies a complete, already-signature-verified event to the graph in
    /// one transaction, then materializes every entity it touched only
    /// after that transaction commits — matching the plan's projection
    /// ordering exactly.
    fn apply_sealed_message_and_materialize(&self, event: SyncEvent) -> Result<(), String> {
        let event_id_hex = encode_id(event.event_id.as_bytes());
        let device_id_hex = encode_id(event.device_id.as_bytes());

        let touched = {
            let mut connection = self.connection()?;
            let tx = connection.transaction().map_err(display)?;

            let already_known: bool = tx
                .query_row("SELECT EXISTS(SELECT 1 FROM sync_events WHERE event_id=?1)", params![event_id_hex], |row| row.get(0))
                .map_err(display)?;
            if !already_known {
                tx.execute(
                    "INSERT INTO sync_events(event_id,epoch,device_id,device_sequence,lamport,state,created_at) VALUES (?1,?2,?3,?4,?5,'sealed',?6)",
                    params![
                        event_id_hex,
                        event.key_epoch,
                        device_id_hex,
                        event.device_sequence as i64,
                        event.lamport as i64,
                        Utc::now().to_rfc3339(),
                    ],
                )
                .map_err(display)?;
            }

            let mut touched: Vec<(EntityType, String)> = Vec::new();
            for op in &event.operations {
                let operation_id_hex = hex_encode(op.operation_id.as_bytes());
                let parents: Vec<String> = op.parents.iter().map(|parent| hex_encode(parent.as_bytes())).collect();
                let stamp = WinnerStamp {
                    lamport: event.lamport,
                    device_id: *event.device_id.as_bytes(),
                    event_id: *event.event_id.as_bytes(),
                    operation_id: *op.operation_id.as_bytes(),
                };
                let applied = Self::apply_remote_operation(
                    &tx,
                    op.entity_type,
                    &op.entity_id,
                    &op.field,
                    op.value.as_ref(),
                    &event_id_hex,
                    &operation_id_hex,
                    &parents,
                    &stamp,
                )?;
                if applied {
                    touched.push((op.entity_type, op.entity_id.clone()));
                }
            }
            tx.commit().map_err(display)?;
            touched
        };

        self.materialize_touched_entities(&touched)
    }

    fn materialize_touched_entities(&self, touched: &[(EntityType, String)]) -> Result<(), String> {
        let mut pending: Vec<(EntityType, String)> = touched.to_vec();
        pending.sort_by(|a, b| a.1.cmp(&b.1));
        pending.dedup();
        if pending.is_empty() {
            return Ok(());
        }
        self.with_remote_projection(|| {
            // Two passes: a dependency that materializes within this same
            // batch (e.g. a calendar account and its selection arriving
            // together) becomes ready on the second pass.
            for _ in 0..2 {
                let mut still_pending = Vec::new();
                for (entity_type, entity_id) in &pending {
                    match self.materialize_one_entity(*entity_type, entity_id)? {
                        ProjectionReadiness::Ready => {}
                        ProjectionReadiness::Pending { .. } => still_pending.push((*entity_type, entity_id.clone())),
                    }
                }
                let done = still_pending.len() == pending.len();
                pending = still_pending;
                if pending.is_empty() || done {
                    break;
                }
            }
            Ok(())
        })
    }

    fn materialize_one_entity(&self, entity_type: EntityType, entity_id: &str) -> Result<ProjectionReadiness, String> {
        let exists = self
            .resolve_field_winner(entity_type, entity_id, ENTITY_EXISTENCE_FIELD)?
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        if !exists {
            self.materialize_entity(entity_type, entity_id, None, true)?;
            return Ok(ProjectionReadiness::Ready);
        }

        let mut payload = serde_json::Map::new();
        for field in self.known_fields(entity_type, entity_id)? {
            if field == ENTITY_EXISTENCE_FIELD {
                continue;
            }
            if let Some(value) = self.resolve_field_winner(entity_type, entity_id, &field)? {
                payload.insert(field, value);
            }
        }
        let payload = Value::Object(payload);
        let readiness = self.check_projection_readiness(entity_type, &payload)?;
        if readiness == ProjectionReadiness::Ready {
            self.materialize_entity(entity_type, entity_id, Some(&payload), false)?;
        }
        Ok(readiness)
    }

    fn known_fields(&self, entity_type: EntityType, entity_id: &str) -> Result<Vec<String>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare("SELECT DISTINCT field FROM sync_operations WHERE entity_type=?1 AND entity_id=?2")
            .map_err(display)?;
        let rows = statement
            .query_map(params![entity_type.as_str(), entity_id], |row| row.get(0))
            .map_err(display)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(display)?;
        Ok(rows)
    }

    /// The current winning value for one field: the frontier member with
    /// the greatest [`WinnerStamp`]. Comparing the stored stamp *bytes*
    /// lexicographically gives the same order as comparing
    /// `(lamport, device_id, event_id, operation_id)`, because
    /// `encode_winner_stamp` writes them in that priority order with a
    /// fixed-width big-endian lamport — no need to decode a candidate to
    /// rank it.
    fn resolve_field_winner(&self, entity_type: EntityType, entity_id: &str, field: &str) -> Result<Option<Value>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT so.value, so.winner_stamp FROM sync_field_frontier sf
                 JOIN sync_operations so ON so.operation_id = sf.operation_id
                 WHERE sf.entity_type=?1 AND sf.entity_id=?2 AND sf.field=?3",
            )
            .map_err(display)?;
        let candidates: Vec<(Option<String>, Vec<u8>)> = statement
            .query_map(params![entity_type.as_str(), entity_id, field], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(display)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(display)?;
        let Some(winner) = candidates.into_iter().max_by(|a, b| a.1.cmp(&b.1)) else {
            return Ok(None);
        };
        winner.0.map(|json| serde_json::from_str(&json).map_err(display)).transpose()
    }
}

/// Resolves and walks one device's signed head back through
/// `previous_device_event` until reaching an already-known chunk index or
/// genesis, verifying every fetched object's bytes against its requested
/// CID before it is parsed or decrypted, then applies every newly-seen
/// event oldest first.
async fn pull_device_chain(
    database: &Database,
    transport: &dyn SyncTransport,
    verifying_key: &VerifyingKey,
    keys: &LocalKeys,
    signed_head: &SignedDeviceHead,
) -> Result<usize, String> {
    let mut cursor = signed_head.head.latest_event_cid.clone();
    let mut chain: Vec<SyncEvent> = Vec::new();

    while let Some(index_cid) = cursor {
        if database.object_exists(&index_cid)? {
            break;
        }
        let index_bytes = transport.get_object(&TransportCid(index_cid.clone())).await.map_err(display)?;
        if compute_cid(&index_bytes) != index_cid {
            return Err("Fetched chunk index bytes do not match the requested CID".to_string());
        }
        let index: ChunkIndex = serde_json::from_slice(&index_bytes).map_err(display)?;

        let mut chunks = Vec::with_capacity(index.chunk_cids.len());
        for chunk_cid in &index.chunk_cids {
            let bytes = transport.get_object(&TransportCid(chunk_cid.clone())).await.map_err(display)?;
            if &compute_cid(&bytes) != chunk_cid {
                return Err("Fetched chunk bytes do not match the requested CID".to_string());
            }
            chunks.push(bytes);
        }

        let event = open_message(
            &chunks,
            &OpenParams {
                sync_space_id: &keys.sync_space_id,
                k_epoch: &keys.k_epoch,
                key_epoch: keys.key_epoch,
                verifying_key,
            },
        )
        .map_err(display)?;

        let event_id_hex = encode_id(event.event_id.as_bytes());
        database.remember_remote_message(&index_cid, &index.chunk_cids, &chunks, &event_id_hex)?;

        let next_cursor = event.previous_device_event.clone();
        chain.push(event);
        cursor = next_cursor;
    }

    let count = chain.len();
    for event in chain.into_iter().rev() {
        database.apply_sealed_message_and_materialize(event)?;
    }
    Ok(count)
}

/// The result of one pull cycle.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PullOutcome {
    pub applied_events: usize,
    pub failed_transports: usize,
}

/// Resolves every known device's head through every enabled transport
/// independently, walks and applies whatever is new, then enqueues
/// anti-entropy repair so a newly pulled object also reaches every other
/// enabled transport. One failing transport is recorded and skipped —
/// never allowed to block pulling from the others.
pub async fn pull_from_transports(
    database: &Database,
    keys: &LocalKeys,
    transports: &[Arc<dyn SyncTransport>],
) -> Result<PullOutcome, String> {
    let roster = database.known_device_roster()?;
    let locators: Vec<HeadLocator> = roster
        .iter()
        .map(|(device_id, _)| HeadLocator {
            device_id: *device_id,
            remote_id: None,
        })
        .collect();

    let mut applied_events = 0usize;
    let mut failed_transports = 0usize;

    for transport in transports {
        let heads = match transport.resolve_heads(&locators).await {
            Ok(heads) => heads,
            Err(_) => {
                failed_transports += 1;
                continue;
            }
        };
        for signed_head in heads {
            let Some((_, verifying_key)) = roster.iter().find(|(device_id, _)| *device_id == signed_head.head.device_id) else {
                continue;
            };
            if verify_device_head(verifying_key, &signed_head).is_err() {
                continue;
            }
            match pull_device_chain(database, transport.as_ref(), verifying_key, keys, &signed_head).await {
                Ok(count) => applied_events += count,
                Err(_) => failed_transports += 1,
            }
        }
    }

    let instance_ids: Vec<TransportInstanceId> = transports.iter().map(|transport| transport.instance_id()).collect();
    database.enqueue_repair_deliveries(&instance_ids)?;

    Ok(PullOutcome {
        applied_events,
        failed_transports,
    })
}

// ================================= Health ===================================

/// Each configured transport's current health, for the replicator's
/// aggregation and the Settings UI. A transport whose `health()` call
/// itself errors is reported as `Unavailable` rather than propagating the
/// error — health reporting must never be the thing that fails.
pub async fn transport_health(transports: &[Arc<dyn SyncTransport>]) -> Vec<(TransportInstanceId, TransportHealth)> {
    let mut results = Vec::with_capacity(transports.len());
    for transport in transports {
        let health = match transport.health().await {
            Ok(health) => health,
            Err(error) => TransportHealth::Unavailable(error.to_string()),
        };
        results.push((transport.instance_id(), health));
    }
    results
}

fn display(value: impl std::fmt::Display) -> String {
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeSet;

    fn fields(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn parses_the_environment_flag_conservatively() {
        assert!(!parse_flag(None));
        assert!(!parse_flag(Some("")));
        assert!(!parse_flag(Some("0")));
        assert!(!parse_flag(Some("false")));
        assert!(!parse_flag(Some("FALSE")));
        assert!(parse_flag(Some("1")));
        assert!(parse_flag(Some("true")));
        assert!(parse_flag(Some("yes")));
    }

    #[test]
    fn recording_a_creation_writes_an_entity_existence_operation_first() {
        let db = Database::open_memory();
        db.record_replicated_write(
            EntityType::Snippet,
            "one",
            &fields(&["name", "body"]),
            &json!({"name": "n", "body": "b"}),
        )
        .unwrap();

        let connection = db.connection().unwrap();
        let operation_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sync_operations WHERE entity_id='one'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        // _entity=true, name, body.
        assert_eq!(operation_count, 3);
        let existence_value: String = connection
            .query_row(
                "SELECT value FROM sync_operations WHERE entity_id='one' AND field='_entity'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(existence_value, "true");
    }

    #[test]
    fn a_later_update_does_not_repeat_the_existence_operation() {
        let db = Database::open_memory();
        db.record_replicated_write(EntityType::Snippet, "one", &fields(&["name"]), &json!({"name": "n"}))
            .unwrap();
        db.record_replicated_write(
            EntityType::Snippet,
            "one",
            &fields(&["name"]),
            &json!({"name": "n2"}),
        )
        .unwrap();

        let connection = db.connection().unwrap();
        let existence_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sync_operations WHERE entity_id='one' AND field='_entity'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(existence_count, 1);

        // The field's frontier now has exactly the newest write as its sole
        // (conflict-free) member, and the old one is no longer in it.
        let frontier_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sync_field_frontier WHERE entity_id='one' AND field='name'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(frontier_count, 1);
        let winning_value: String = connection
            .query_row(
                "SELECT so.value FROM sync_field_frontier sf
                 JOIN sync_operations so ON so.operation_id = sf.operation_id
                 WHERE sf.entity_id='one' AND sf.field='name'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(winning_value, "\"n2\"");
    }

    #[test]
    fn the_new_write_names_the_old_frontier_as_its_parent() {
        let db = Database::open_memory();
        db.record_replicated_write(EntityType::Snippet, "one", &fields(&["name"]), &json!({"name": "n"}))
            .unwrap();
        let connection = db.connection().unwrap();
        let first_operation_id: String = connection
            .query_row(
                "SELECT operation_id FROM sync_operations WHERE entity_id='one' AND field='name'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        drop(connection);

        db.record_replicated_write(
            EntityType::Snippet,
            "one",
            &fields(&["name"]),
            &json!({"name": "n2"}),
        )
        .unwrap();

        let connection = db.connection().unwrap();
        let parent_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sync_operation_parents WHERE parent_operation_id=?1",
                [&first_operation_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(parent_count, 1);
    }

    #[test]
    fn deletion_writes_entity_false_without_touching_other_fields() {
        let db = Database::open_memory();
        db.record_replicated_write(EntityType::Snippet, "one", &fields(&["name"]), &json!({"name": "n"}))
            .unwrap();
        db.record_replicated_deletion(EntityType::Snippet, "one").unwrap();

        let connection = db.connection().unwrap();
        let existence_value: String = connection
            .query_row(
                "SELECT so.value FROM sync_field_frontier sf
                 JOIN sync_operations so ON so.operation_id = sf.operation_id
                 WHERE sf.entity_id='one' AND sf.field='_entity'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(existence_value, "false");
        // The name field's frontier is untouched by the deletion.
        let name_frontier: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sync_field_frontier WHERE entity_id='one' AND field='name'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(name_frontier, 1);
    }

    #[test]
    fn remote_projection_suppresses_local_recording() {
        let db = Database::open_memory();
        db.with_remote_projection(|| {
            db.record_replicated_write(EntityType::Snippet, "one", &fields(&["name"]), &json!({"name": "n"}))
        })
        .unwrap();

        let connection = db.connection().unwrap();
        let operation_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM sync_operations", [], |row| row.get(0))
            .unwrap();
        assert_eq!(operation_count, 0);
        assert!(!db.is_projecting_remote_operation());
    }

    #[test]
    fn remote_projection_flag_is_restored_even_after_an_error() {
        let db = Database::open_memory();
        let result = db.with_remote_projection(|| Err::<(), _>("boom".to_string()));
        assert!(result.is_err());
        assert!(!db.is_projecting_remote_operation());
    }

    #[test]
    fn projection_readiness_accepts_a_complete_valid_entity() {
        let db = Database::open_memory();
        let readiness = db
            .check_projection_readiness(
                EntityType::Snippet,
                &json!({"name": "n", "body": "b"}),
            )
            .unwrap();
        assert_eq!(readiness, ProjectionReadiness::Ready);
    }

    #[test]
    fn projection_readiness_rejects_an_incomplete_entity() {
        let db = Database::open_memory();
        let result = db.check_projection_readiness(EntityType::Snippet, &json!({"name": "n"}));
        assert!(result.is_err());
    }

    #[test]
    fn projection_readiness_defers_a_calendar_selection_missing_its_account() {
        let db = Database::open_memory();
        let readiness = db
            .check_projection_readiness(
                EntityType::CalendarSelection,
                &json!({"accountId": "missing@example.com", "calendarIds": ["primary"]}),
            )
            .unwrap();
        assert_eq!(
            readiness,
            ProjectionReadiness::Pending {
                reason: "calendar account missing@example.com has not materialized yet".to_string()
            }
        );
    }
}

#[cfg(test)]
mod replicator_tests {
    use super::*;
    use threestrands_sync_transport::fake::FakeTransport;

    fn fields(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn snippet_payload(id: &str, name: &str) -> Value {
        serde_json::json!({"id": id, "name": name, "body": "body", "createdAt": "2026-01-01T00:00:00Z"})
    }

    /// Synthetic key material matching whatever device id
    /// `record_replicated_write` already provisioned for `database`. Never
    /// touches the OS keychain — that's what makes this different from
    /// `Database::local_replicated_keys`.
    fn test_keys(database: &Database) -> LocalKeys {
        let device_id = {
            let mut connection = database.connection().unwrap();
            let tx = connection.transaction().unwrap();
            let device_id = ensure_space_and_device(&tx).unwrap();
            tx.commit().unwrap();
            device_id
        };
        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();
        database.trust_device_public_key(&device_id, &verifying_key).unwrap();
        LocalKeys {
            verifying_key,
            signing_key,
            k_epoch: [7u8; 32],
            key_epoch: 0,
            device_id: EnvelopeDeviceId::from_bytes(device_id),
            sync_space_id: b"test-space".to_vec(),
        }
    }

    #[tokio::test]
    async fn sealing_creates_chunk_and_index_objects_and_pending_deliveries() {
        let database = Database::open_memory();
        database
            .record_replicated_write(EntityType::Snippet, "one", &fields(&["id", "name", "body", "createdAt"]), &snippet_payload("one", "n"))
            .unwrap();
        let keys = test_keys(&database);
        let sealed = database.seal_pending_events(&keys, &[TransportInstanceId("t".to_string())]).unwrap();
        assert_eq!(sealed, 1);

        let connection = database.connection().unwrap();
        let object_count: i64 = connection.query_row("SELECT COUNT(*) FROM sync_objects", [], |row| row.get(0)).unwrap();
        assert!(object_count >= 2, "expected at least one chunk plus its index");
        let index_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM sync_objects WHERE object_kind='chunk_index'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(index_count, 1);
        let delivery_count: i64 = connection.query_row("SELECT COUNT(*) FROM sync_deliveries WHERE state='pending'", [], |row| row.get(0)).unwrap();
        assert_eq!(delivery_count, object_count);
        let event_state: String = connection.query_row("SELECT state FROM sync_events", [], |row| row.get(0)).unwrap();
        assert_eq!(event_state, "sealed");
    }

    #[tokio::test]
    async fn push_delivers_to_a_fake_transport_and_records_success() {
        let database = Database::open_memory();
        database
            .record_replicated_write(EntityType::Snippet, "one", &fields(&["id", "name", "body", "createdAt"]), &snippet_payload("one", "n"))
            .unwrap();
        let keys = test_keys(&database);
        let transport: Arc<dyn SyncTransport> = Arc::new(FakeTransport::new("folder-a"));
        let transports = vec![transport];

        let outcome = push_pending_events(&database, &keys, &transports).await.unwrap();
        assert_eq!(outcome.sealed_events, 1);
        assert_eq!(outcome.failed, 0);
        assert!(outcome.delivered > 0);

        let (pending, delivered, failed) = database.delivery_counts("folder-a").unwrap();
        assert_eq!(pending, 0);
        assert_eq!(failed, 0);
        assert!(delivered > 0);
    }

    #[tokio::test]
    async fn push_keeps_a_transient_failure_pending_for_retry() {
        let database = Database::open_memory();
        database
            .record_replicated_write(EntityType::Snippet, "one", &fields(&["id", "name", "body", "createdAt"]), &snippet_payload("one", "n"))
            .unwrap();
        let keys = test_keys(&database);
        let fake = FakeTransport::new("folder-a");
        fake.inject_transient_outage(1000);
        let transport: Arc<dyn SyncTransport> = Arc::new(fake);
        let transports = vec![transport];

        let outcome = push_pending_events(&database, &keys, &transports).await.unwrap();
        assert_eq!(outcome.delivered, 0);
        assert!(outcome.failed > 0);

        let (pending, delivered, failed) = database.delivery_counts("folder-a").unwrap();
        assert!(pending > 0);
        assert_eq!(delivered, 0);
        // Transient failures stay retryable rather than settling as
        // permanently failed.
        assert_eq!(failed, 0);
    }

    #[tokio::test]
    async fn two_devices_converge_through_a_shared_transport() {
        let database_a = Database::open_memory();
        let database_b = Database::open_memory();

        database_b
            .record_replicated_write(EntityType::Snippet, "b-1", &fields(&["id", "name", "body", "createdAt"]), &snippet_payload("b-1", "From B"))
            .unwrap();
        let keys_b = test_keys(&database_b);

        let shared: Arc<dyn SyncTransport> = Arc::new(FakeTransport::new("shared"));
        let transports = vec![shared];
        push_pending_events(&database_b, &keys_b, &transports).await.unwrap();

        // Device A trusts device B's key (simulating completed enrollment,
        // a later phase's job) and pulls.
        let keys_a = test_keys(&database_a);
        database_a.trust_device_public_key(keys_b.device_id.as_bytes(), &keys_b.verifying_key).unwrap();

        let outcome = pull_from_transports(&database_a, &keys_a, &transports).await.unwrap();
        assert_eq!(outcome.failed_transports, 0);
        assert_eq!(outcome.applied_events, 1);

        let snippet_name: String = database_a
            .connection()
            .unwrap()
            .query_row("SELECT name FROM snippets WHERE id='b-1'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(snippet_name, "From B");

        // Pulling again recognizes the already-known chunk index and
        // applies nothing new.
        let second = pull_from_transports(&database_a, &keys_a, &transports).await.unwrap();
        assert_eq!(second.applied_events, 0);
    }

    #[tokio::test]
    async fn pull_skips_a_head_from_an_untrusted_device() {
        let database_a = Database::open_memory();
        let database_b = Database::open_memory();

        database_b
            .record_replicated_write(EntityType::Snippet, "b-1", &fields(&["id", "name", "body", "createdAt"]), &snippet_payload("b-1", "From B"))
            .unwrap();
        let keys_b = test_keys(&database_b);
        let shared: Arc<dyn SyncTransport> = Arc::new(FakeTransport::new("shared"));
        let transports = vec![shared];
        push_pending_events(&database_b, &keys_b, &transports).await.unwrap();

        // A never calls trust_device_public_key for B this time.
        let keys_a = test_keys(&database_a);
        let outcome = pull_from_transports(&database_a, &keys_a, &transports).await.unwrap();
        assert_eq!(outcome.applied_events, 0);

        let missing = database_a
            .connection()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM snippets WHERE id='b-1'", [], |row| row.get::<_, i64>(0))
            .unwrap();
        assert_eq!(missing, 0);
    }

    #[tokio::test]
    async fn repair_delivers_a_pulled_object_to_a_second_transport() {
        let database_a = Database::open_memory();
        let database_b = Database::open_memory();

        database_b
            .record_replicated_write(EntityType::Snippet, "b-1", &fields(&["id", "name", "body", "createdAt"]), &snippet_payload("b-1", "From B"))
            .unwrap();
        let keys_b = test_keys(&database_b);

        let transport_x: Arc<dyn SyncTransport> = Arc::new(FakeTransport::new("x"));
        let transport_y: Arc<dyn SyncTransport> = Arc::new(FakeTransport::new("y"));

        // B only pushes to transport X.
        push_pending_events(&database_b, &keys_b, std::slice::from_ref(&transport_x)).await.unwrap();

        let keys_a = test_keys(&database_a);
        database_a.trust_device_public_key(keys_b.device_id.as_bytes(), &keys_b.verifying_key).unwrap();

        // A knows about both transports and pulls from X.
        let outcome = pull_from_transports(&database_a, &keys_a, &[transport_x.clone(), transport_y.clone()]).await.unwrap();
        assert_eq!(outcome.applied_events, 1);

        // Repair queued delivery to Y even though A never pushed anything
        // of its own.
        let (pending_y, _, _) = database_a.delivery_counts("y").unwrap();
        assert!(pending_y > 0);

        push_pending_events(&database_a, &keys_a, &[transport_x, transport_y]).await.unwrap();
        let (pending_y_after, delivered_y, _) = database_a.delivery_counts("y").unwrap();
        assert_eq!(pending_y_after, 0);
        assert!(delivered_y > 0);
    }

    #[tokio::test]
    async fn one_failed_transport_never_blocks_pull_from_a_healthy_one() {
        let database_a = Database::open_memory();
        let database_b = Database::open_memory();

        database_b
            .record_replicated_write(EntityType::Snippet, "b-1", &fields(&["id", "name", "body", "createdAt"]), &snippet_payload("b-1", "From B"))
            .unwrap();
        let keys_b = test_keys(&database_b);

        let healthy: Arc<dyn SyncTransport> = Arc::new(FakeTransport::new("healthy"));
        let failing_fake = FakeTransport::new("failing");
        failing_fake.set_authentication_failure(true);
        let failing: Arc<dyn SyncTransport> = Arc::new(failing_fake);

        push_pending_events(&database_b, &keys_b, std::slice::from_ref(&healthy)).await.unwrap();

        let keys_a = test_keys(&database_a);
        database_a.trust_device_public_key(keys_b.device_id.as_bytes(), &keys_b.verifying_key).unwrap();

        let outcome = pull_from_transports(&database_a, &keys_a, &[failing, healthy]).await.unwrap();
        assert_eq!(outcome.failed_transports, 1);
        assert_eq!(outcome.applied_events, 1);
    }

    #[test]
    fn backoff_retry_at_is_in_the_future_and_grows_with_attempts() {
        let now = Utc::now().to_rfc3339();
        let first = backoff_retry_at(1);
        let later = backoff_retry_at(6);
        assert!(first.as_str() > now.as_str());
        assert!(later.as_str() > first.as_str());
    }
}
