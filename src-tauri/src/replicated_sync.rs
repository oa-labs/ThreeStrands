//! The replicated-sync engine: keys, transports, and the push/pull cycle
//! that replicates this device's replica state (`sync_state.rs`) with its
//! peers'.
//!
//! Each device keeps one sealed, encrypted snapshot of its whole replica
//! on every connector and a signed head pointing at it. A push seals a new
//! snapshot when the replica changed, delivers it, points each connector's
//! head at it once that connector has all of it, and deletes the previous
//! snapshot there. A pull merges each peer's latest snapshot that is newer
//! than the one last merged from it, then materializes whatever changed.
//! There is no log to replay, walk, or compact: a device offline for any
//! length of time just merges its peers' current states.
//!
//! Entirely inert unless [`Database::replicated_sync_active`] is true (the
//! Settings beta toggle, or the `THREESTRANDS_REPLICATED_SYNC` environment
//! override). Mutation commands in `lib.rs` reach the replica through
//! `Database::record_local_entity_write` and
//! `Database::record_local_entity_deletion` in `sync_projection.rs`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use chrono::Utc;
use keyring::Entry;
use rand::{rngs::OsRng, RngCore};
use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use threestrands_sync_core::{EntityType, ENTITY_EXISTENCE_FIELD};
use threestrands_sync_envelope::{
    compute_cid, message_key_epoch, open_snapshot, protocol_marker_cid, seal_snapshot, sign_device_head, verify_device_head,
    DeviceHead, DeviceId as EnvelopeDeviceId, ObjectKind, OpenParams, SealParams, SignedDeviceHead, SigningKey, VerifyingKey,
    PROTOCOL_MARKER,
};
use threestrands_sync_transport::{
    Cid as TransportCid, HeadLocator, SyncTransport, TransportError, TransportHealth,
    TransportInstanceId,
};

use crate::{
    backoff::retry_at,
    db::{Database, DatabaseError, DbResult},
    error_text::display,
    s3_transport::{S3Config, S3Credentials, S3ProbeReport, S3Transport},
    sync_connectors::{
        is_known_kind, Connector, ConnectorCredentials, ConnectorProbe, FolderConfig, IpfsRpcConfig, TransportConfig,
        TransportSecrets,
    },
};

/// The single local sync space Phase 2 supports. Multiple concurrent spaces
/// are not a product concept yet; this is simply a stable primary key.
pub(crate) const SPACE_ID: &str = "default";

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

impl Database {
    /// Whether replicated sync is active for this device: the hard
    /// `THREESTRANDS_REPLICATED_SYNC` env-var override (dev/CI), or the
    /// persisted "Enable replicated sync" Settings toggle a user turned on
    /// themselves. Reuses `sync_spaces.enabled`, which every earlier phase
    /// reserved for exactly this without ever wiring it up.
    pub fn replicated_sync_active(&self) -> Result<bool, String> {
        Ok(enabled() || self.beta_features_enabled()?)
    }

    /// The persisted state of the Settings "Enable replicated sync" toggle.
    /// `false` (not an error) when no `sync_spaces` row exists yet — nothing
    /// has ever been turned on.
    pub fn beta_features_enabled(&self) -> Result<bool, String> {
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
    pub fn set_beta_features_enabled(&self, on: bool) -> Result<(), String> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO sync_spaces(id, active_epoch, lamport, enabled) VALUES (?1, 0, 0, ?2)
                 ON CONFLICT(id) DO UPDATE SET enabled=excluded.enabled",
                params![SPACE_ID, on],
            )?;
            Ok(())
        })?;
        Ok(())
    }
}

/// The outcome of checking whether a resolved, validated entity is ready to
/// materialize into application tables. Mirrors the plan's projection
/// contract: "Materialize parent entities before dependent entities... A
/// calendar selection that arrives before its calendar account remains
/// pending and is retried after the account materializes." Reuses the exact
/// same dependency check `sync_projection::upsert_synced_calendar_selection`
/// performs. Used by `materialize_one_entity`
/// while projecting a merged peer snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectionReadiness {
    Ready,
    Pending { reason: String },
}

/// One candidate value still in a field's frontier — a write from some
/// device that has not (yet) been superseded.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontierConflictCandidate {
    pub operation_id: String,
    pub device_id: String,
    pub value: Option<Value>,
}

/// One field whose frontier currently has more than one member: a genuine
/// concurrent write, not arrival-order noise. The conflict review UI works
/// over exactly these — see [`Database::list_frontier_conflicts`] and
/// [`Database::resolve_frontier_conflict`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontierConflict {
    pub entity_type: String,
    pub entity_id: String,
    pub field: String,
    pub candidates: Vec<FrontierConflictCandidate>,
}

impl Database {
    /// Enqueues `entity_id` as a creation if the graph has no operation for
    /// it yet; a no-op otherwise. `record_replicated_write` already treats
    /// "no prior operation" as creation and writes `_entity=true` plus
    /// every field in `payload`, so this is just that call guarded by the
    /// existence check.
    fn reconcile_one_entity(&self, entity_type: EntityType, entity_id: &str, payload: Value) -> Result<bool, String> {
        if self.entity_recorded(entity_type, entity_id)? {
            return Ok(false);
        }
        let fields: BTreeSet<String> = payload
            .as_object()
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default();
        self.record_replicated_write(entity_type, entity_id, &fields, &payload)?;
        Ok(true)
    }

    /// Repairs the one durability gap `record_replicated_write` leaves open:
    /// a mutation command's app-table write (`create_task`, `create_snippet`,
    /// ...) and its enqueue call remain two separate statements — so every
    /// existing mutation call site can keep calling them exactly as before —
    /// which means a crash in the narrow window between them would silently
    /// leave that one entity permanently un-enqueued, since nothing else
    /// ever retries a "local row exists, graph never heard about it" gap.
    ///
    /// This sweep closes it: enumerate every local entity and enqueue
    /// any one the graph has never seen. A no-op for anything already
    /// recorded, so it is safe and cheap to run on every sync cycle, not
    /// just at startup — `ReplicatedSync::sync_once` does exactly that.
    /// Portable preferences are intentionally not covered here: their
    /// readable snapshot and replica write commit in one transaction (see
    /// `update_synced_preferences`), so there is no gap to reconcile.
    ///
    /// Does not check [`enabled`] itself — like `record_replicated_write`,
    /// that is the caller's job (`ReplicatedSync::sync_once` already gates
    /// on it), so this stays directly callable from a test without an
    /// environment variable to fiddle with.
    pub fn reconcile_replicated_sync_backlog(&self) -> Result<usize, String> {
        let mut repaired = 0usize;
        for task in self.list_tasks(None, None)? {
            if self.reconcile_one_entity(EntityType::Task, &task.id, serde_json::to_value(&task).map_err(display)?)? {
                repaired += 1;
            }
        }
        for goal in self.list_goals(None)? {
            if self.reconcile_one_entity(EntityType::Goal, &goal.id, serde_json::to_value(&goal).map_err(display)?)? {
                repaired += 1;
            }
        }
        for snippet in self.list_snippets()? {
            if self.reconcile_one_entity(EntityType::Snippet, &snippet.id, serde_json::to_value(&snippet).map_err(display)?)? {
                repaired += 1;
            }
        }
        for contact in self.list_saved_contact_profiles()? {
            repaired += usize::from(self.reconcile_one_entity(EntityType::Contact, &contact.id, serde_json::to_value(crate::models::ContactRecord::from(&contact)).map_err(display)?)?);
        }
        for id in self.list_contact_group_ids()? {
            if let Some(record) = self.contact_group_record(&id)? {
                repaired += usize::from(self.reconcile_one_entity(EntityType::ContactGroup, &id, record)?);
            }
        }
        for split in self.list_split_inboxes()? {
            if self.reconcile_one_entity(EntityType::SplitInbox, &split.id, serde_json::to_value(&split).map_err(display)?)? {
                repaired += 1;
            }
        }
        for account in self.list_accounts()? {
            let payload = json!({
                "email": account.email,
                "displayName": account.display_name,
                "color": account.color,
                "provider": account.provider,
                "sortOrder": account.sort_order,
            });
            if self.reconcile_one_entity(EntityType::MailAccount, &account.email.to_ascii_lowercase(), payload)? {
                repaired += 1;
            }
        }
        for account in self.list_calendar_accounts()? {
            let email = account.email.to_ascii_lowercase();
            if self.reconcile_one_entity(EntityType::CalendarAccount, &email, json!({ "email": account.email }))? {
                repaired += 1;
            }
            if let Some(ids) = self.calendar_selection(&account.email)? {
                let payload = json!({ "accountId": account.email, "calendarIds": ids });
                if self.reconcile_one_entity(EntityType::CalendarSelection, &email, payload)? {
                    repaired += 1;
                }
            }
        }
        // Unset retention is the unlimited default, not a choice this device
        // made. Seeding it would assert "forever" as a concurrent write the
        // moment a fresh device joins, conflicting with whatever the space
        // already agreed on. An explicit choice — including switching back
        // to unlimited — is recorded by the `set_retention_days` command.
        if let Some(days) = self.retention_days()? {
            if self.reconcile_one_entity(EntityType::Retention, "mail", json!({ "days": days }))? {
                repaired += 1;
            }
        }
        Ok(repaired)
    }

    /// True while the calling thread is applying an already-authenticated
    /// remote (or conflict-resolution) operation. A shared materializer
    /// checks this before calling [`Self::record_replicated_write`] /
    /// [`Self::record_replicated_deletion`] so projecting a remote write
    /// never re-enqueues it as a new local write — the echo-prevention the
    /// plan calls for. Other threads are unaffected, so a local edit made
    /// while a projection runs is still recorded.
    pub(crate) fn is_projecting_remote_operation(&self) -> bool {
        projecting_threads(self).contains_key(&std::thread::current().id())
    }

    /// Runs `work` with remote-projection suppression engaged for the
    /// calling thread. `work` is synchronous, so the projection cannot hop
    /// threads. Always restores the previous state afterward, including
    /// when `work` returns an error or panics.
    /// Wraps `materialize_touched_entities`'s projection of a merged snapshot.
    pub(crate) fn with_remote_projection<R>(&self, work: impl FnOnce() -> Result<R, String>) -> Result<R, String> {
        struct Projection<'a> {
            database: &'a Database,
            thread: std::thread::ThreadId,
        }
        impl Drop for Projection<'_> {
            fn drop(&mut self) {
                let mut threads = projecting_threads(self.database);
                if let Some(depth) = threads.get_mut(&self.thread) {
                    *depth -= 1;
                    if *depth == 0 {
                        threads.remove(&self.thread);
                    }
                }
            }
        }
        let thread = std::thread::current().id();
        *projecting_threads(self).entry(thread).or_insert(0) += 1;
        let _projection = Projection { database: self, thread };
        work()
    }

    /// Validates a fully resolved entity payload and checks any known
    /// materialization dependency, without writing anything. Called before
    /// invoking the real per-entity upsert, and retried later on
    /// [`ProjectionReadiness::Pending`] rather than treating a
    /// not-yet-materialized dependency as an error.
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
        return Ok(decode_id(&hex)?);
    }
    let device_id = random_id();
    tx.execute(
        "INSERT INTO sync_devices(device_id, status, is_self) VALUES (?1, 'active', 1)",
        params![encode_id(&device_id)],
    )?;
    Ok(device_id)
}

pub(crate) fn random_id() -> [u8; 16] {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    bytes
}

pub(crate) fn encode_id(bytes: &[u8; 16]) -> String {
    hex_encode(bytes)
}

pub(crate) fn decode_id(hex: &str) -> Result<[u8; 16], String> {
    let bytes = hex_decode(hex)?;
    bytes
        .try_into()
        .map_err(|_| "Invalid replicated-sync identifier".to_string())
}

pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn hex_decode(hex: &str) -> Result<Vec<u8>, String> {
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

// ============================== Key material ==============================

pub(crate) const KEYCHAIN_SERVICE: &str = "app.threestrands.replicated-sync";
const SIGNING_KEY_ENTRY: &str = "device-signing-key";
const X25519_KEY_ENTRY: &str = "device-x25519-key";

fn epoch_key_entry(key_epoch: u32) -> String {
    format!("epoch-key-{key_epoch}")
}

/// The minimal single-device key material push/pull need to seal and open
/// messages for real: an Ed25519 device signing key, the active epoch's
/// symmetric key, and every earlier epoch's key this device holds, all kept
/// in the OS keychain. Snapshots are sealed only under the active epoch;
/// earlier keys exist to open history sealed before a rotation. The X25519
/// device key stays on [`DeviceIdentity`] — only enrollment/rotation
/// sealed-box handling needs it, not ordinary push/pull.
pub struct LocalKeys {
    pub signing_key: SigningKey,
    pub k_epoch: [u8; 32],
    pub key_epoch: u32,
    pub earlier_epoch_keys: BTreeMap<u32, [u8; 32]>,
    pub device_id: EnvelopeDeviceId,
    pub sync_space_id: Vec<u8>,
}

impl LocalKeys {
    /// The key for `key_epoch`, whether active or earlier, or `None` if this
    /// device never received it.
    pub fn epoch_key(&self, key_epoch: u32) -> Option<&[u8; 32]> {
        if key_epoch == self.key_epoch {
            Some(&self.k_epoch)
        } else {
            self.earlier_epoch_keys.get(&key_epoch)
        }
    }
}

/// This device's identity — always available once replicated sync is turned
/// on, regardless of enrollment state. Enough to sign and publish an
/// enrollment request/grant/rotation object; not enough to seal or open an
/// ordinary snapshot, which additionally needs an active epoch key (see
/// [`Database::local_replicated_keys`]).
pub struct DeviceIdentity {
    pub signing_key: SigningKey,
    pub verifying_key: VerifyingKey,
    pub x25519_secret: [u8; 32],
    pub device_id: EnvelopeDeviceId,
    pub sync_space_id: Vec<u8>,
}

impl Database {
    /// Loads (provisioning on first use) this device's signing and X25519
    /// keypairs and trusts them for itself. Touches the OS keychain — never
    /// call this from a test.
    pub fn local_device_identity(&self) -> Result<DeviceIdentity, String> {
        let device_id = self.with_transaction(ensure_space_and_device)?;
        let signing_key = load_or_create_signing_key()?;
        let x25519_secret = load_or_create_device_x25519_secret()?;
        let verifying_key = signing_key.verifying_key();
        let x25519_public = x25519_public_bytes(&x25519_secret);
        self.trust_device_keys(&device_id, &verifying_key, &x25519_public)?;
        self.ensure_self_device_name(&encode_id(&device_id))?;
        Ok(DeviceIdentity {
            verifying_key,
            signing_key,
            x25519_secret,
            device_id: EnvelopeDeviceId::from_bytes(device_id),
            sync_space_id: SPACE_ID.as_bytes().to_vec(),
        })
    }

    /// Loads this device's full replicated-sync key material, additionally
    /// requiring that enrollment has already supplied an epoch key for the
    /// sync space's current `active_epoch` — see `enrollment.rs`. Returns an
    /// error (not a panic or a silently generated fresh epoch) if this
    /// device has not completed enrollment yet; callers treat that as "skip
    /// push/pull this cycle," not a hard failure.
    pub fn local_replicated_keys(&self) -> Result<LocalKeys, String> {
        let identity = self.local_device_identity()?;
        let active_epoch: u32 = self.with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT active_epoch FROM sync_spaces WHERE id=?1",
                params![SPACE_ID],
                |row| row.get(0),
            )?)
        })?;
        let k_epoch = load_epoch_key(active_epoch)?
            .ok_or_else(|| "This device has not completed replicated-sync enrollment yet".to_string())?;
        let earlier_epochs: Vec<u32> = self.with_connection(|connection| {
            let mut statement =
                connection.prepare("SELECT key_epoch FROM sync_epoch_history WHERE key_epoch < ?1 ORDER BY key_epoch")?;
            let rows = statement
                .query_map(params![active_epoch], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;
        let mut earlier_epoch_keys = BTreeMap::new();
        for key_epoch in earlier_epochs {
            if let Some(key) = load_epoch_key(key_epoch)? {
                earlier_epoch_keys.insert(key_epoch, key);
            }
        }
        Ok(LocalKeys {
            signing_key: identity.signing_key,
            k_epoch,
            key_epoch: active_epoch,
            earlier_epoch_keys,
            device_id: identity.device_id,
            sync_space_id: identity.sync_space_id,
        })
    }

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
                let device_id = EnvelopeDeviceId::from_bytes(decode_id(&device_id_hex)?);
                let key_array: [u8; 32] = key_bytes
                    .try_into()
                    .map_err(|_| "Stored device public key is invalid".to_string())?;
                let verifying_key = VerifyingKey::from_bytes(&key_array)
                    .map_err(|_| "Stored device public key is invalid".to_string())?;
                Ok((device_id, verifying_key))
            })
            .collect::<Result<_, String>>()
            .map_err(DatabaseError::from)
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

pub(crate) fn x25519_public_bytes(secret: &[u8; 32]) -> [u8; 32] {
    threestrands_sync_envelope::X25519PublicKey::from(&threestrands_sync_envelope::X25519StaticSecret::from(*secret)).to_bytes()
}

fn load_or_create_device_x25519_secret() -> Result<[u8; 32], String> {
    let entry = Entry::new(KEYCHAIN_SERVICE, X25519_KEY_ENTRY).map_err(display)?;
    match entry.get_password() {
        Ok(hex) => hex_decode(&hex)?
            .try_into()
            .map_err(|_| "Stored device X25519 key is invalid".to_string()),
        Err(keyring::Error::NoEntry) => {
            let mut secret = [0u8; 32];
            OsRng.fill_bytes(&mut secret);
            entry.set_password(&hex_encode(&secret)).map_err(display)?;
            Ok(secret)
        }
        Err(error) => Err(display(error)),
    }
}

/// Reads a previously stored epoch key from the keychain, or `None` if this
/// device has never received (or generated, at genesis) that epoch.
pub(crate) fn load_epoch_key(key_epoch: u32) -> Result<Option<[u8; 32]>, String> {
    let entry = Entry::new(KEYCHAIN_SERVICE, &epoch_key_entry(key_epoch)).map_err(display)?;
    match entry.get_password() {
        Ok(hex) => hex_decode(&hex)?
            .try_into()
            .map(Some)
            .map_err(|_| "Stored epoch key is invalid".to_string()),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(display(error)),
    }
}

fn delete_keychain_entry(name: &str) -> Result<(), String> {
    match Entry::new(KEYCHAIN_SERVICE, name).map_err(display)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(display(error)),
    }
}

/// Removes every epoch key up to `highest_epoch` and this device's own
/// signing and X25519 keys, so the next identity load provisions a fresh
/// device. Keeps going past a failure so one stuck entry does not leave
/// the rest behind, then reports the first failure.
pub(crate) fn forget_sync_space_keys(highest_epoch: u32) -> Result<(), String> {
    let mut first_error = None;
    let entries = (0..=highest_epoch)
        .map(epoch_key_entry)
        .chain([SIGNING_KEY_ENTRY.to_string(), X25519_KEY_ENTRY.to_string()]);
    for name in entries {
        if let Err(error) = delete_keychain_entry(&name) {
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// Persists an epoch key this device just generated (genesis) or received
/// and opened (enrollment grant, rotation).
pub(crate) fn store_epoch_key(key_epoch: u32, key: &[u8; 32]) -> Result<(), String> {
    let entry = Entry::new(KEYCHAIN_SERVICE, &epoch_key_entry(key_epoch)).map_err(display)?;
    entry.set_password(&hex_encode(key)).map_err(display)
}

// ================================ Sealing ==================================

/// What a device head's `state_cid` actually points to: not a chunk's CID
/// directly (a multi-chunk snapshot has several, and there is no way to
/// derive the rest from just one), but this small, unauthenticated index
/// listing every chunk CID in order. It needs no signature of its own:
/// every chunk is independently AEAD-authenticated, every chunk in one
/// message shares an authenticated hash of the complete reassembled
/// plaintext, and the reassembled snapshot carries its author's signature —
/// a forged or corrupted index can only ever make `open_snapshot` fail,
/// never make a wrong snapshot succeed.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct ChunkIndex {
    chunk_cids: Vec<String>,
}

impl Database {
    /// Seals a new snapshot of this replica when it changed since the last
    /// one, or was sealed under an older epoch (so a key rotation re-seals
    /// everything under the new key): builds its encrypted chunks and chunk
    /// index, stores them, schedules delivery to every transport in
    /// `transports`, and retires the previous snapshot's objects. Pure local
    /// bookkeeping — no network I/O. Returns whether it sealed one.
    pub fn seal_local_state(&self, keys: &LocalKeys, transports: &[TransportInstanceId]) -> Result<bool, String> {
        let (dirty, sequence, sealed_epoch) = self.local_state_status()?;
        if !dirty && sequence > 0 && sealed_epoch == Some(keys.key_epoch) {
            return Ok(false);
        }
        let next = sequence + 1;
        let (snapshot, snapshot_generation) = self.take_local_snapshot(keys.device_id, next, crate::sync_policy::now_ms())?;
        let sealed = match seal_snapshot(
            snapshot,
            &SealParams {
                sync_space_id: &keys.sync_space_id,
                k_epoch: &keys.k_epoch,
                key_epoch: keys.key_epoch,
                object_kind: ObjectKind::Snapshot,
                signing_key: &keys.signing_key,
            },
        ) {
            Ok(sealed) => sealed,
            Err(error) => {
                self.mark_replica_changed()?;
                return Err(display(error));
            }
        };
        let stored = self.with_transaction(|tx| {
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

            let chunk_count = sealed.chunks.len() as i64;
            let mut chunk_cids = Vec::with_capacity(sealed.chunks.len());
            let mut objects: Vec<(String, &str, i64, i64, Vec<u8>)> = Vec::with_capacity(sealed.chunks.len() + 1);
            for (index, chunk) in sealed.chunks.iter().enumerate() {
                let cid = compute_cid(chunk);
                objects.push((cid.clone(), "state_chunk", index as i64, chunk_count, chunk.clone()));
                chunk_cids.push(cid);
            }
            let index_bytes = serde_json::to_vec(&ChunkIndex { chunk_cids }).map_err(display)?;
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
                params![next as i64, keys.key_epoch],
            )?;
            crate::sync_state::clear_dirty_if_generation(tx, snapshot_generation)?;
            Ok(())
        });
        if let Err(error) = stored {
            self.mark_replica_changed()?;
            return Err(String::from(error));
        }
        Ok(true)
    }

    /// The snapshot `transport_instance_id` can honestly be pointed at: this
    /// device's current snapshot, once every one of its objects has reached
    /// that transport. `None` while it hasn't, and the transport's head then
    /// keeps naming the previous snapshot, which stays there until it's
    /// replaced.
    fn delivered_state_head(&self, transport_instance_id: &str) -> DbResult<Option<(u64, String)>> {
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

    /// Deletes this device's superseded snapshot objects from a transport
    /// whose head now names `head_sequence`. Best effort: a failed delete is
    /// retried next time; an object already gone counts as deleted.
    async fn delete_retired_objects(&self, transport: &dyn SyncTransport, head_sequence: u64) -> Result<(), String> {
        let instance_id = transport.instance_id().0;
        let retired: Vec<String> = self.with_connection(|connection| {
            let mut statement = connection
                .prepare("SELECT cid FROM sync_retired_objects WHERE transport_instance_id=?1 AND state_sequence < ?2")?;
            let rows = statement
                .query_map(params![instance_id, head_sequence as i64], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;
        for cid in retired {
            match transport.delete_object(&TransportCid(cid.clone())).await {
                Ok(()) | Err(TransportError::NotFound) => {
                    self.with_connection(|connection| {
                        connection.execute(
                            "DELETE FROM sync_retired_objects WHERE cid=?1 AND transport_instance_id=?2",
                            params![cid, instance_id],
                        )?;
                        Ok(())
                    })?;
                }
                Err(error) => log::debug!(target: "replicated_sync", "deleting an old snapshot object from {instance_id} failed: {error}"),
            }
        }
        Ok(())
    }

    /// The latest snapshot sequence merged from `device_id_hex`.
    fn merged_state_sequence(&self, device_id_hex: &str) -> DbResult<u64> {
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

    fn record_merged_state(&self, device_id_hex: &str, state_sequence: u64) -> DbResult<()> {
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
    fn head_publication_due(&self, transport_instance_id: &str, head_content: &str, now_ms: i64) -> DbResult<bool> {
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

    fn record_head_publication(&self, transport_instance_id: &str, head_content: &str, now_ms: i64) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO sync_head_publications(transport_instance_id, head_content, published_at_ms) VALUES (?1,?2,?3)
                 ON CONFLICT(transport_instance_id) DO UPDATE SET head_content=excluded.head_content, published_at_ms=excluded.published_at_ms",
                params![transport_instance_id, head_content, now_ms],
            )?;
            Ok(())
        })
    }
}

impl Database {
    /// Records the protocol marker as a local object, so anti-entropy
    /// repair stores it on every connector this device uses, including ones
    /// added later. Idempotent.
    pub(crate) fn ensure_protocol_marker_object(&self) -> Result<(), String> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT OR IGNORE INTO sync_objects(cid,object_kind,chunk_index,chunk_count,bytes) VALUES (?1,'protocol_marker',0,1,?2)",
                params![protocol_marker_cid(), PROTOCOL_MARKER],
            )?;
            Ok(())
        })
        .map_err(String::from)
    }

    /// Whether to tell the user that upgrading reset sync: this device left
    /// a group created by an earlier, incompatible build.
    pub fn protocol_reset_notice(&self) -> Result<bool, String> {
        self.with_connection(|connection| {
            Ok(connection.query_row("SELECT EXISTS(SELECT 1 FROM sync_notices WHERE kind='protocol_reset')", [], |row| row.get(0))?)
        })
        .map_err(String::from)
    }

    pub fn dismiss_protocol_reset_notice(&self) -> Result<(), String> {
        self.with_connection(|connection| {
            connection.execute("DELETE FROM sync_notices WHERE kind='protocol_reset'", [])?;
            Ok(())
        })
        .map_err(String::from)
    }

    /// The highest epoch whose keychain entries a schema migration left
    /// behind (it can't reach the keychain itself), if any.
    fn pending_keychain_cleanup(&self) -> Result<Option<u32>, String> {
        self.with_connection(|connection| {
            Ok(connection
                .query_row("SELECT highest_epoch FROM sync_pending_keychain_cleanup WHERE id=1", [], |row| row.get(0))
                .optional()?)
        })
        .map_err(String::from)
    }

    fn clear_pending_keychain_cleanup(&self) -> Result<(), String> {
        self.with_connection(|connection| {
            connection.execute("DELETE FROM sync_pending_keychain_cleanup", [])?;
            Ok(())
        })
        .map_err(String::from)
    }

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
    fn pending_delivery_items(&self, transport_instance_id: &str) -> DbResult<Vec<DeliveryItem>> {
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

    fn record_delivery_success(&self, cid: &str, transport_instance_id: &str, remote_id: Option<&str>) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE sync_deliveries SET state='delivered', remote_id=?3, last_error=NULL, retry_at=NULL
                 WHERE cid=?1 AND transport_instance_id=?2",
                params![cid, transport_instance_id, remote_id],
            )?;
            Ok(())
        })
    }

    fn record_delivery_failure(
        &self,
        cid: &str,
        transport_instance_id: &str,
        attempts: i64,
        error: &TransportError,
    ) -> DbResult<()> {
        let next_attempts = attempts + 1;
        let (state, retry_timestamp) = if error.is_retryable() && next_attempts < MAX_DELIVERY_ATTEMPTS {
            (
                "pending",
                Some(retry_at(
                    next_attempts.max(0).try_into().unwrap_or(u32::MAX),
                )),
            )
        } else {
            ("failed", None)
        };
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE sync_deliveries SET state=?3, attempts=?4, retry_at=?5, last_error=?6
                 WHERE cid=?1 AND transport_instance_id=?2",
                params![cid, transport_instance_id, state, next_attempts, retry_timestamp, error.to_string()],
            )?;
            Ok(())
        })
    }

    /// Ensures a pending delivery row exists for every locally known object
    /// on every transport in `transport_ids` that doesn't already have one
    /// (pending, delivered, or failed) — the anti-entropy behavior that
    /// turns transport union into replication. Cheap to call repeatedly:
    /// `INSERT OR IGNORE` only ever adds rows for a truly new pairing.
    pub fn enqueue_repair_deliveries(&self, transport_ids: &[TransportInstanceId]) -> Result<usize, String> {
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
        .map_err(String::from)
    }

    /// Pending/delivered/failed delivery counts for one transport instance,
    /// for the Settings UI.
    pub fn delivery_counts(&self, transport_instance_id: &str) -> Result<(i64, i64, i64), String> {
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
        .map_err(String::from)
    }
}

/// The result of one push cycle.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PushOutcome {
    pub sealed_snapshot: bool,
    pub delivered: usize,
    pub failed: usize,
}

/// Seals a new snapshot if the replica changed, then attempts delivery of
/// every pending object to every transport concurrently, then publishes
/// each transport's head. Never holds a SQLite transaction across a
/// `put_object` call: sealing commits first, and each delivery outcome is
/// recorded in its own short transaction after the network call returns.
pub async fn push_local_state(
    database: &Database,
    keys: &LocalKeys,
    transports: &[Arc<dyn SyncTransport>],
) -> Result<PushOutcome, String> {
    let instance_ids: Vec<TransportInstanceId> = transports.iter().map(|transport| transport.instance_id()).collect();
    let sealed_snapshot = database.seal_local_state(keys, &instance_ids)?;

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
    Ok(PushOutcome { sealed_snapshot, delivered, failed })
}

/// Publishes this device's signed head to every transport where it's due,
/// best-effort: one transport failing to accept the head never blocks
/// publishing to the others, and never fails the push cycle.
///
/// Each transport's head names this device's current snapshot only once
/// that transport has every one of its objects, so a reader never follows
/// a head to a snapshot that isn't there yet; until then the transport
/// keeps its previous head, whose snapshot is still stored. Once a head
/// names the new snapshot, the old one's objects are deleted from that
/// transport. A head is due when its content (everything except the
/// publication time) changed since it was last published there, or once
/// the heartbeat interval has passed.
async fn publish_local_head(database: &Database, keys: &LocalKeys, transports: &[Arc<dyn SyncTransport>]) {
    let now = crate::sync_policy::now_ms();
    for transport in transports {
        let instance_id = transport.instance_id();
        let Ok(Some((state_sequence, state_cid))) = database.delivered_state_head(&instance_id.0) else {
            continue;
        };
        let head = DeviceHead {
            sync_space_id: keys.sync_space_id.clone(),
            device_id: keys.device_id,
            epoch: keys.key_epoch,
            state_sequence,
            state_cid: Some(state_cid),
            published_at_ms: now,
        };
        let content = json!({ "epoch": head.epoch, "stateSequence": head.state_sequence, "stateCid": head.state_cid }).to_string();
        if !database.head_publication_due(&instance_id.0, &content, now).unwrap_or(true) {
            continue;
        }
        let Ok(signed) = sign_device_head(&keys.signing_key, head) else {
            continue;
        };
        match transport.publish_head(&signed).await {
            Ok(_) => {
                if let Err(error) = database.record_head_publication(&instance_id.0, &content, now) {
                    log::debug!(target: "replicated_sync", "recording the head publication to {} failed: {error}", instance_id.0);
                }
                if let Err(error) = database.delete_retired_objects(transport.as_ref(), state_sequence).await {
                    log::debug!(target: "replicated_sync", "retiring old snapshot objects on {} failed: {error}", instance_id.0);
                }
            }
            Err(error) => log::debug!(
                target: "replicated_sync",
                "publishing the device head to {} failed: {error}",
                instance_id.0
            ),
        }
    }
}

/// The per-thread projection depths. The map holds plain counters that are
/// always left consistent, so a poisoned lock is safe to recover.
fn projecting_threads(database: &Database) -> std::sync::MutexGuard<'_, std::collections::HashMap<std::thread::ThreadId, usize>> {
    database.replicated_sync_projecting.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Database {
    pub(crate) fn materialize_touched_entities(&self, touched: &[(EntityType, String)]) -> Result<(), String> {
        // A merged snapshot is acknowledged even when an individual local
        // projection cannot yet be applied. Keep those entities durable and
        // retry them on every pull cycle, even when the peer has no newer
        // snapshot to send.
        let mut pending: Vec<(EntityType, String)> = touched.to_vec();
        pending.extend(self.pending_entity_materializations()?);
        pending.sort_by(|a, b| a.1.cmp(&b.1));
        pending.dedup();
        if pending.is_empty() {
            return Ok(());
        }
        self.with_remote_projection(|| {
            // Two passes: a dependency that materializes within this same
            // batch (e.g. a calendar account and its selection arriving
            // together) becomes ready on the second pass.
            let mut failed_this_batch = Vec::<(EntityType, String)>::new();
            for _ in 0..2 {
                let mut still_pending = Vec::new();
                for (entity_type, entity_id) in &pending {
                    let key=(*entity_type,entity_id.clone());
                    if failed_this_batch.contains(&key) {
                        still_pending.push(key);
                        continue;
                    }
                    match self.materialize_one_entity(*entity_type, entity_id) {
                        Ok(ProjectionReadiness::Ready) => self.clear_pending_entity_materialization(*entity_type,entity_id)?,
                        Ok(ProjectionReadiness::Pending { reason }) => {
                            self.remember_pending_entity_materialization(*entity_type,entity_id,&reason)?;
                            still_pending.push(key);
                        }
                        Err(error) => {
                            log::warn!(target:"replicated_sync", "materializing remote {} {} failed; queued for retry: {error}",entity_type.as_str(),entity_id);
                            self.remember_pending_entity_materialization(*entity_type,entity_id,&error)?;
                            failed_this_batch.push(key.clone());
                            still_pending.push(key);
                        }
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

    fn pending_entity_materializations(&self)->Result<Vec<(EntityType,String)>,String>{
        let rows=self.with_connection(|connection|{
            let mut statement=connection.prepare("SELECT entity_type,entity_id FROM pending_entity_materializations ORDER BY updated_at")?;
            let rows=statement.query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?)))?;
            rows.collect::<Result<Vec<_>,_>>().map_err(Into::into)
        }).map_err(display)?;
        Ok(rows.into_iter().filter_map(|(kind,id)|{
            serde_json::from_value::<EntityType>(Value::String(kind.clone())).ok().map(|entity_type|(entity_type,id)).or_else(||{
                log::warn!(target:"replicated_sync", "ignoring pending materialization for unknown entity type {kind}");
                None
            })
        }).collect())
    }

    fn remember_pending_entity_materialization(&self,entity_type:EntityType,entity_id:&str,reason:&str)->Result<(),String>{
        let reason=reason.chars().take(2_000).collect::<String>();
        self.with_connection(|connection|{
            connection.execute("INSERT INTO pending_entity_materializations(entity_type,entity_id,reason,updated_at) VALUES(?1,?2,?3,?4) ON CONFLICT(entity_type,entity_id) DO UPDATE SET reason=excluded.reason,updated_at=excluded.updated_at",params![entity_type.as_str(),entity_id,reason,Utc::now().to_rfc3339()])?;
            Ok(())
        }).map_err(display)
    }

    fn clear_pending_entity_materialization(&self,entity_type:EntityType,entity_id:&str)->Result<(),String>{
        self.with_connection(|connection|{
            connection.execute("DELETE FROM pending_entity_materializations WHERE entity_type=?1 AND entity_id=?2",params![entity_type.as_str(),entity_id])?;
            Ok(())
        }).map_err(display)
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

}


/// Fetches, verifies, opens, and merges one peer's snapshot named by its
/// verified head, then materializes whatever it changed. Every fetched
/// object's bytes are checked against the CID they were requested by
/// before being parsed or decrypted, the snapshot is opened with the key
/// for the epoch its header names, and it must be the head's own author and
/// sequence. Merging is idempotent, so a crash after merging only means the
/// next pull merges the same snapshot again, harmlessly.
async fn pull_device_state(
    database: &Database,
    transport: &dyn SyncTransport,
    verifying_key: &VerifyingKey,
    keys: &LocalKeys,
    signed_head: &SignedDeviceHead,
) -> Result<(), String> {
    let head = &signed_head.head;
    let author_hex = encode_id(head.device_id.as_bytes());
    let index_cid = head.state_cid.clone().ok_or_else(|| format!("Device {author_hex}'s head names no snapshot"))?;
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
    let key_epoch = chunks
        .first()
        .ok_or_else(|| "A chunk index lists no chunks".to_string())
        .and_then(|chunk| message_key_epoch(chunk).map_err(display))?;
    let k_epoch = keys
        .epoch_key(key_epoch)
        .ok_or_else(|| format!("This device doesn't have the key for sync epoch {key_epoch}"))?;
    let snapshot = open_snapshot(&chunks, &OpenParams { sync_space_id: &keys.sync_space_id, k_epoch, key_epoch, verifying_key })
        .map_err(display)?;
    if snapshot.device_id != head.device_id || snapshot.state_sequence != head.state_sequence {
        return Err(format!("Device {author_hex}'s head and snapshot disagree about which snapshot it is"));
    }
    let remote = crate::sync_state::snapshot_to_state(&snapshot)?;
    let touched = database.merge_replica_state(&remote)?;
    database.record_merged_state(&author_hex, snapshot.state_sequence)?;
    database.materialize_touched_entities(&touched)
}

/// The result of one pull cycle.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PullOutcome {
    pub merged_states: usize,
    pub failed_transports: usize,
}

/// Resolves every known device's head through every enabled transport
/// independently, and merges each peer's snapshot that is newer than the
/// one last merged from it. One failing transport is recorded and skipped —
/// never allowed to block pulling from the others. This device's own head,
/// and heads from devices that aren't trusted and active, are skipped.
pub async fn pull_from_transports(
    database: &Database,
    keys: &LocalKeys,
    transports: &[Arc<dyn SyncTransport>],
) -> Result<PullOutcome, String> {
    let roster = database.known_device_roster()?;
    let locators: Vec<HeadLocator> = roster
        .iter()
        .map(|(device_id, _)| HeadLocator { device_id: *device_id, remote_id: None })
        .collect();

    let mut merged_states = 0usize;
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
            if signed_head.head.device_id == keys.device_id {
                continue;
            }
            let Some((_, verifying_key)) = roster.iter().find(|(device_id, _)| *device_id == signed_head.head.device_id) else {
                continue;
            };
            if verify_device_head(verifying_key, &signed_head).is_err() {
                continue;
            }
            if signed_head.head.sync_space_id.as_slice() != keys.sync_space_id.as_slice() {
                continue;
            }
            database.record_head_observation(&signed_head.head, crate::sync_policy::now_ms())?;
            let author_hex = encode_id(signed_head.head.device_id.as_bytes());
            if signed_head.head.state_sequence <= database.merged_state_sequence(&author_hex)? {
                continue;
            }
            match pull_device_state(database, transport.as_ref(), verifying_key, keys, &signed_head).await {
                Ok(()) => merged_states += 1,
                Err(error) => {
                    log::warn!(
                        target: "replicated_sync",
                        "pulling device {author_hex}'s snapshot from transport {} failed: {error}",
                        transport.instance_id().0
                    );
                    failed_transports += 1;
                }
            }
        }
    }

    // Retry projections left pending by an earlier merged snapshot. This is
    // independent of head sequence so an unchanged peer can still converge
    // after a local ownership conflict is resolved.
    database.materialize_touched_entities(&[])?;

    let instance_ids: Vec<TransportInstanceId> = transports.iter().map(|transport| transport.instance_id()).collect();
    database.enqueue_repair_deliveries(&instance_ids)?;
    Ok(PullOutcome { merged_states, failed_transports })
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

// ============================ Transport config ==============================

/// One configured transport instance's persisted row — a durable record of
/// "the user selected this folder," independent of whatever live
/// `Arc<dyn SyncTransport>` gets constructed from it at startup or on
/// demand.
pub struct ConfiguredTransport {
    pub instance_id: String,
    pub kind: String,
    pub config_json: String,
    pub enabled: bool,
    pub last_success_at: Option<String>,
    pub last_error: Option<String>,
}

impl Database {
    pub fn configured_transports(&self) -> Result<Vec<ConfiguredTransport>, String> {
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
        .map_err(String::from)
    }

    /// Persists (or reconfigures, if `instance_id` already exists) one
    /// connector: validates it, writes its non-secret config to
    /// `sync_transports`, then stores `secrets` in the OS keychain — or,
    /// with `None`, clears any secret a previous configuration left.
    pub fn add_transport(
        &self,
        instance_id: &str,
        config: &TransportConfig,
        secrets: Option<&TransportSecrets>,
    ) -> Result<(), String> {
        config.validate(instance_id, secrets)?;
        let config_json = config.to_config_json()?;
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO sync_transports(instance_id,kind,config_json,required,enabled) VALUES (?1,?2,?3,1,1)
                 ON CONFLICT(instance_id) DO UPDATE SET kind=excluded.kind, config_json=excluded.config_json, enabled=1",
                params![instance_id, config.kind(), config_json],
            )?;
            Ok(())
        })?;
        match secrets {
            Some(secrets) => secrets.store(instance_id),
            None => TransportSecrets::delete(config.kind(), instance_id),
        }
    }

    /// Persists (or reconfigures) a folder transport instance. Only the
    /// path is stored — no credential, no content.
    pub fn add_folder_transport(&self, instance_id: &str, path: &std::path::Path) -> Result<(), String> {
        let config = TransportConfig::Folder(FolderConfig {
            path: path.to_string_lossy().into_owned(),
            label: None,
        });
        self.add_transport(instance_id, &config, None)
    }

    /// Persists (or reconfigures) an IPFS RPC transport instance. Only the
    /// base URL is stored in `sync_transports`; the access token (if any)
    /// goes to the OS keychain under this instance's id, never into SQLite.
    pub fn add_ipfs_rpc_transport(&self, instance_id: &str, base_url: &str, token: Option<&str>) -> Result<(), String> {
        let config = TransportConfig::IpfsRpc(IpfsRpcConfig {
            base_url: base_url.to_string(),
            label: None,
        });
        let secrets = token.map(|token| TransportSecrets::IpfsRpcToken(token.to_string()));
        self.add_transport(instance_id, &config, secrets.as_ref())
    }

    /// Persists (or reconfigures) an S3-compatible storage transport
    /// instance, validated before anything is written.
    pub fn add_s3_transport(
        &self,
        instance_id: &str,
        config: &crate::s3_transport::S3Config,
        credentials: &crate::s3_transport::S3Credentials,
    ) -> Result<(), String> {
        self.add_transport(
            instance_id,
            &TransportConfig::S3(config.clone()),
            Some(&TransportSecrets::S3(credentials.clone())),
        )
    }

    /// Changes an existing connector's config (for example its name) and,
    /// when `secrets` is given, replaces its stored secret — keeping its
    /// delivery ledger, enabled state, and last-success time. The kind
    /// can't change; remove and re-add for that.
    pub fn update_transport_config(
        &self,
        instance_id: &str,
        config: &TransportConfig,
        secrets: Option<&TransportSecrets>,
    ) -> Result<(), String> {
        let existing = self
            .configured_transports()?
            .into_iter()
            .find(|row| row.instance_id == instance_id)
            .ok_or_else(|| "That connector no longer exists".to_string())?;
        if existing.kind != config.kind() {
            return Err("A connector's kind can't be changed; remove it and add a new one".to_string());
        }
        let stored;
        let effective_secrets = match secrets {
            Some(secrets) => Some(secrets),
            None => {
                stored = TransportSecrets::load(config.kind(), instance_id)?;
                stored.as_ref()
            }
        };
        config.validate(instance_id, effective_secrets)?;
        let config_json = config.to_config_json()?;
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE sync_transports SET config_json=?2 WHERE instance_id=?1",
                params![instance_id, config_json],
            )?;
            Ok(())
        })?;
        match secrets {
            Some(secrets) => secrets.store(instance_id),
            None => Ok(()),
        }
    }

    /// Forgets a configured transport instance, its delivery ledger rows,
    /// and its keychain secret, if its kind has one. Does not touch the
    /// remote corpus itself — the caller deletes that first (through the
    /// live connector) if the user asked for that.
    pub fn remove_transport(&self, instance_id: &str) -> Result<(), String> {
        let kind = self
            .configured_transports()?
            .into_iter()
            .find(|row| row.instance_id == instance_id)
            .map(|row| row.kind);
        self.with_connection(|connection| {
            connection.execute("DELETE FROM sync_transports WHERE instance_id=?1", params![instance_id])?;
            connection.execute("DELETE FROM sync_deliveries WHERE transport_instance_id=?1", params![instance_id])?;
            connection.execute("DELETE FROM sync_head_publications WHERE transport_instance_id=?1", params![instance_id])?;
            connection.execute("DELETE FROM sync_retired_objects WHERE transport_instance_id=?1", params![instance_id])?;
            Ok(())
        })?;
        match kind {
            Some(kind) if is_known_kind(&kind) => TransportSecrets::delete(&kind, instance_id),
            // No row, or a kind this version doesn't know: clear every
            // secret this instance id could have.
            _ => TransportSecrets::delete_every_kind(instance_id),
        }
    }

    fn set_transport_success(&self, instance_id: &str) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE sync_transports SET last_success_at=?2, last_error=NULL WHERE instance_id=?1",
                params![instance_id, Utc::now().to_rfc3339()],
            )?;
            Ok(())
        })
    }

    fn set_transport_error(&self, instance_id: &str, error: &str) -> DbResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE sync_transports SET last_error=?2 WHERE instance_id=?1",
                params![instance_id, error],
            )?;
            Ok(())
        })
    }
}

impl ConfiguredTransport {
    /// This row's typed config, or `None` for an unknown kind or malformed
    /// JSON.
    pub fn config(&self) -> Option<TransportConfig> {
        TransportConfig::from_row(&self.kind, &self.config_json)
    }

    /// Opens this row's live connector, loading its secret from the
    /// keychain. `None` if it's misconfigured, of an unknown kind, or its
    /// storage can't be opened.
    pub async fn open_connector(&self) -> Option<Connector> {
        Connector::open_persisted(&self.instance_id, &self.config()?).await.ok()
    }
}

/// Builds the live transport for one configured row, or `None` if it's
/// disabled, misconfigured, or of an unknown kind.
async fn build_transport_from_row(row: &ConfiguredTransport) -> Option<Arc<dyn SyncTransport>> {
    if !row.enabled {
        return None;
    }
    Some(row.open_connector().await?.into_transport())
}

/// Builds the live transport for every enabled configured row. A row whose
/// transport fails to open (folder missing, permission denied, endpoint
/// URL no longer valid, ...) is skipped rather than failing the whole set
/// — its own health will report `Unavailable` on the next status check.
pub async fn build_configured_transports(database: &Database) -> Vec<Arc<dyn SyncTransport>> {
    let mut transports: Vec<Arc<dyn SyncTransport>> = Vec::new();
    let Ok(rows) = database.configured_transports() else {
        return transports;
    };
    for row in rows {
        if let Some(transport) = build_transport_from_row(&row).await {
            transports.push(transport);
        }
    }
    transports
}

// ============================== Engine ======================================

const SYNC_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// One transport instance's status for the Settings UI: identity, live
/// health, delivery ledger counts, and a best-effort storage estimate.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplicatedSyncTransportStatus {
    pub instance_id: String,
    /// `"folder"`, `"ipfs_rpc"`, or `"s3"`.
    pub kind: String,
    /// The user-chosen name, if any.
    pub label: Option<String>,
    /// A folder's path, an RPC endpoint's base URL, or an S3 endpoint with
    /// its bucket and prefix — never a credential.
    pub location: String,
    /// Whether "delete files and disconnect" can remove this connector's
    /// synchronized data (folder and S3; not IPFS pins).
    pub supports_delete_data: bool,
    /// An S3 connector's non-secret settings, so Settings can re-test
    /// replacement credentials against the same bucket. Never credentials.
    pub s3_config: Option<S3Config>,
    pub health: String,
    /// Whether this instance can currently discover other devices'
    /// signed heads on its own (a folder's `heads/` directory; an RPC
    /// endpoint's dedicated bucket pin index) — `false` means "storage-only": still a
    /// valid write/read replica, but it cannot bootstrap a new device by
    /// itself.
    pub head_discovery: bool,
    pub pending: i64,
    pub delivered: i64,
    pub failed: i64,
    pub last_success_at: Option<String>,
    pub last_error: Option<String>,
    pub storage_bytes: Option<u64>,
}

/// What "Test connection" reports for a candidate S3 connector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct S3ConnectionTest {
    #[serde(flatten)]
    pub checks: S3ProbeReport,
    /// Whether this bucket and prefix already hold a sync group; `None`
    /// when the key couldn't list and read, so nothing could be checked.
    pub space_presence: Option<crate::enrollment::SyncSpacePresence>,
}

/// Owns the replicated-sync background cycle: a cheaply `Clone`-able handle
/// held directly in `AppState`, constructed once, cloned into commands and
/// the background task.
#[derive(Clone)]
pub struct ReplicatedSync {
    database: Arc<Database>,
    gate: Arc<tokio::sync::Mutex<()>>,
}

impl ReplicatedSync {
    pub fn new(database: Arc<Database>) -> Self {
        Self {
            database,
            gate: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// Every configured transport's current status, for the Settings UI.
    pub async fn status(&self) -> Result<Vec<ReplicatedSyncTransportStatus>, String> {
        let rows = self.database.configured_transports()?;
        let mut statuses = Vec::with_capacity(rows.len());
        for row in rows {
            let (pending, delivered, failed) = self.database.delivery_counts(&row.instance_id).unwrap_or((0, 0, 0));
            let config = row.config();

            let mut health = "unavailable: not configured".to_string();
            let mut head_discovery = false;
            let mut storage_bytes = None;
            if row.enabled {
                if let Some(connector) = row.open_connector().await {
                    let transport = connector.transport();
                    health = match transport.health().await {
                        Ok(TransportHealth::Healthy) => "healthy".to_string(),
                        Ok(TransportHealth::Degraded(message)) => format!("degraded: {message}"),
                        Ok(TransportHealth::Unavailable(message)) => format!("unavailable: {message}"),
                        Err(error) => format!("unavailable: {error}"),
                    };
                    head_discovery = transport.capabilities().head_discovery;
                    storage_bytes = connector.corpus_size_bytes().await;
                }
            }

            statuses.push(ReplicatedSyncTransportStatus {
                instance_id: row.instance_id,
                kind: row.kind,
                label: config.as_ref().and_then(|config| config.label()).map(str::to_string),
                location: config.as_ref().map(TransportConfig::location).unwrap_or_default(),
                supports_delete_data: config.as_ref().is_some_and(TransportConfig::supports_delete_data),
                s3_config: match &config {
                    Some(TransportConfig::S3(s3)) => Some(s3.clone()),
                    _ => None,
                },
                health,
                head_discovery,
                pending,
                delivered,
                failed,
                last_success_at: row.last_success_at,
                last_error: row.last_error,
                storage_bytes,
            });
        }
        Ok(statuses)
    }

    /// Validates a candidate IPFS RPC endpoint without persisting anything
    /// — the "test connection" step Settings runs before letting the user
    /// enable a replica, per the plan's "explain a missing required
    /// capability before the user enables the replica."
    pub async fn probe_ipfs_rpc_endpoint(
        &self,
        base_url: &str,
        token: Option<&str>,
    ) -> Result<crate::ipfs_transport::ProbeReport, String> {
        let config = TransportConfig::IpfsRpc(IpfsRpcConfig {
            base_url: base_url.to_string(),
            label: None,
        });
        let secrets = token.map(|token| TransportSecrets::IpfsRpcToken(token.to_string()));
        match Connector::open("probe", &config, secrets).await?.probe().await? {
            ConnectorProbe::IpfsRpc(report) => Ok(report),
            _ => unreachable!("an IPFS RPC connector reports an IPFS RPC probe"),
        }
    }

    /// "Test connection" for a candidate S3 connector, persisting nothing:
    /// the permission checklist, plus — when the key can list and read —
    /// whether the bucket and prefix already hold a sync group. `Err` only
    /// when the config or credentials are invalid before any request.
    pub async fn probe_s3(&self, config: &S3Config, credentials: &S3Credentials) -> Result<S3ConnectionTest, String> {
        let transport = S3Transport::new("probe", config, credentials).map_err(|error| error.to_string())?;
        let checks = transport.probe().await;
        let space_presence = if checks.can_list && checks.can_read {
            let transports: Vec<Arc<dyn SyncTransport>> = vec![Arc::new(transport)];
            Some(crate::enrollment::inspect_sync_space(&transports).await)
        } else {
            None
        };
        Ok(S3ConnectionTest { checks, space_presence })
    }

    /// Renames a connector (`label`: `None` keeps the name, a blank string
    /// clears it) and/or replaces its credentials, keeping its delivery
    /// ledger. Credentials of the wrong kind, or for a folder, are refused.
    pub fn update_connector(
        &self,
        instance_id: &str,
        label: Option<&str>,
        credentials: Option<ConnectorCredentials>,
    ) -> Result<(), String> {
        let row = self
            .database
            .configured_transports()?
            .into_iter()
            .find(|row| row.instance_id == instance_id)
            .ok_or_else(|| "That connector no longer exists".to_string())?;
        let mut config = row
            .config()
            .ok_or_else(|| "This connector's settings can't be read by this version of ThreeStrands".to_string())?;
        if let Some(label) = label {
            config.set_label(Some(label));
        }
        let secrets = credentials.map(ConnectorCredentials::into_secrets).transpose()?;
        self.database.update_transport_config(instance_id, &config, secrets.as_ref())
    }

    /// Runs one push-then-pull cycle against every configured transport,
    /// plus the enrollment/rotation control-object sweep. A no-op if the
    /// feature is disabled or nothing is configured yet. Serialized against
    /// concurrent calls (the periodic loop and a manual "sync now" click)
    /// by `gate`.
    pub async fn sync_once(&self) -> Result<(), String> {
        if !self.database.replicated_sync_active()? {
            return Ok(());
        }
        let _guard = self.gate.lock().await;
        // Finish leaving a group from an earlier, incompatible build: the
        // schema migration left that group but couldn't reach the keychain.
        // Forgetting the device keys too means this device joins its next
        // group as a fresh identity.
        if let Some(highest_epoch) = self.database.pending_keychain_cleanup()? {
            forget_sync_space_keys(highest_epoch)?;
            self.database.clear_pending_keychain_cleanup()?;
        }
        // Best-effort: catch up any entity a crash left un-enqueued before
        // doing anything else, so it is never more than one cycle behind
        // even with no transport configured yet.
        if let Err(error) = self.database.reconcile_replicated_sync_backlog() {
            log::warn!(target: "replicated_sync", "backlog reconciliation failed: {error}");
        }
        let transports = build_configured_transports(&self.database).await;
        if transports.is_empty() {
            return Ok(());
        }
        let identity = self.database.local_device_identity()?;
        let must_rotate = match crate::enrollment::run_enrollment_sweep(
            &self.database,
            &identity,
            &crate::enrollment::KeychainEpochKeyStore,
            &transports,
        )
        .await
        {
            Ok(must_rotate) => must_rotate,
            Err(error) => {
                log::warn!(target: "replicated_sync", "enrollment sweep failed: {error}");
                false
            }
        };

        // A device that has not finished enrollment yet (no epoch key)
        // still benefits from the sweep above; it just has nothing to
        // push/pull until a grant or genesis supplies one.
        let mut keys = match self.database.local_replicated_keys() {
            Ok(keys) => keys,
            Err(error) => {
                log::debug!(
                    target: "replicated_sync",
                    "local replicated-sync keys are unavailable; skipping push/pull this cycle: {error}"
                );
                return Ok(());
            }
        };
        // Another device created our active epoch with a different key at
        // the same moment, and this device is the one that resolves it. If
        // the rotation fails, the colliding object stays unseen and the next
        // cycle tries again.
        if must_rotate {
            match crate::enrollment::rotate_epoch(&self.database, &identity, &keys, &crate::enrollment::KeychainEpochKeyStore, &transports, None).await {
                Ok(()) => keys = self.database.local_replicated_keys()?,
                Err(error) => log::warn!(target: "replicated_sync", "rotating to resolve an epoch collision failed: {error}"),
            }
        }
        // Admit or refuse join-code redemptions and expire old codes; a
        // closed invitation rotates the epoch, so reload the keys after.
        match crate::enrollment::process_join_codes(
            &self.database,
            &identity,
            &keys,
            &crate::enrollment::KeychainEpochKeyStore,
            &transports,
            Utc::now().timestamp_millis(),
        )
        .await
        {
            Ok(true) => keys = self.database.local_replicated_keys()?,
            Ok(false) => {}
            Err(error) => log::warn!(target: "replicated_sync", "join code processing failed: {error}"),
        }
        self.database.record_self_device_name_if_missing(&encode_id(identity.device_id.as_bytes()))?;
        self.database.ensure_protocol_marker_object()?;

        let push_result = push_local_state(&self.database, &keys, &transports).await;
        let pull_result = pull_from_transports(&self.database, &keys, &transports).await;
        if let Err(error) = crate::enrollment::share_keys_with_lagging_peers(&self.database, &keys, &transports).await {
            log::warn!(target: "replicated_sync", "sharing keys with lagging devices failed: {error}");
        }

        for (instance_id, health) in transport_health(&transports).await {
            let recorded = match health {
                TransportHealth::Healthy => self.database.set_transport_success(&instance_id.0),
                TransportHealth::Degraded(message) | TransportHealth::Unavailable(message) => {
                    self.database.set_transport_error(&instance_id.0, &message)
                }
            };
            if let Err(error) = recorded {
                log::warn!(
                    target: "replicated_sync",
                    "recording health for transport {} failed: {error}",
                    instance_id.0
                );
            }
        }

        push_result?;
        pull_result?;
        Ok(())
    }

    /// Starts a brand-new sync space on this device and returns the
    /// recovery phrase, shown to the user exactly once. Refused when a
    /// configured transport already holds a space, unless the user
    /// explicitly chose to start a separate one.
    pub async fn begin_genesis(&self, allow_existing_space: bool) -> Result<String, String> {
        let _guard = self.gate.lock().await;
        let identity = self.database.local_device_identity()?;
        let transports = build_configured_transports(&self.database).await;
        crate::enrollment::begin_genesis(&self.database, &identity, &crate::enrollment::KeychainEpochKeyStore, &transports, allow_existing_space).await
    }

    /// Whether the configured transports already hold a sync space, so
    /// Settings can steer a new device toward joining it.
    pub async fn inspect_sync_space(&self) -> crate::enrollment::SyncSpacePresence {
        let transports = build_configured_transports(&self.database).await;
        crate::enrollment::inspect_sync_space(&transports).await
    }

    /// Publishes a signed enrollment request for this (new) device and
    /// returns its fingerprint for display.
    pub async fn request_enrollment(&self) -> Result<String, String> {
        let _guard = self.gate.lock().await;
        let identity = self.database.local_device_identity()?;
        let transports = build_configured_transports(&self.database).await;
        crate::enrollment::publish_enrollment_request(&self.database, &identity, &transports).await
    }

    /// Approves a pending incoming request, publishing a grant.
    pub async fn approve_enrollment_request(&self, request_id_hex: &str) -> Result<(), String> {
        let _guard = self.gate.lock().await;
        let identity = self.database.local_device_identity()?;
        let keys = self.database.local_replicated_keys()?;
        let transports = build_configured_transports(&self.database).await;
        crate::enrollment::approve_enrollment_request(&self.database, &identity, &keys, request_id_hex, &transports).await
    }

    /// Rejects an incoming request and publishes the signed group-wide
    /// decision to every configured connector.
    pub async fn reject_enrollment_request(&self, request_id_hex: &str) -> Result<(), String> {
        let _guard = self.gate.lock().await;
        let identity = self.database.local_device_identity()?;
        let transports = build_configured_transports(&self.database).await;
        crate::enrollment::reject_enrollment_request(&self.database, &identity, request_id_hex, &transports).await
    }

    /// Imports a staged grant after the user confirms its fingerprint.
    pub async fn confirm_enrollment(&self, request_id_hex: &str) -> Result<(), String> {
        let _guard = self.gate.lock().await;
        let identity = self.database.local_device_identity()?;
        crate::enrollment::confirm_and_import_grant(&self.database, &identity, &crate::enrollment::KeychainEpochKeyStore, request_id_hex).await
    }

    /// Rotates the active epoch, optionally revoking a device. This and the
    /// other commands that change enrollment or keys hold the sync gate, so
    /// they never interleave with a cycle that has already loaded the
    /// current epoch's keys (or with that cycle's own rotations).
    pub async fn rotate_epoch(&self, revoke_device_id_hex: Option<&str>) -> Result<(), String> {
        let _guard = self.gate.lock().await;
        let identity = self.database.local_device_identity()?;
        let keys = self.database.local_replicated_keys()?;
        let transports = build_configured_transports(&self.database).await;
        crate::enrollment::rotate_epoch(&self.database, &identity, &keys, &crate::enrollment::KeychainEpochKeyStore, &transports, revoke_device_id_hex).await
    }

    /// Leaves the sync space on this device only; see
    /// [`Database::leave_sync_space`]. Holds the sync gate so a cycle in
    /// flight finishes before the log it is using disappears.
    pub async fn leave_sync_space(&self) -> Result<(), String> {
        let _guard = self.gate.lock().await;
        let highest_epoch = self.database.leave_sync_space()?;
        forget_sync_space_keys(highest_epoch)
            .map_err(|error| format!("Left the sync group, but some keys could not be removed from the keychain: {error}"))
    }

    /// Creates a join code for the chosen connectors. Requires this device
    /// to be enrolled.
    pub async fn create_join_code(
        &self,
        connectors: &[crate::enrollment::JoinCodeConnectorChoice],
        expires_in_hours: u32,
    ) -> Result<String, String> {
        let _guard = self.gate.lock().await;
        let identity = self.database.local_device_identity()?;
        let keys = self
            .database
            .local_replicated_keys()
            .map_err(|_| "This device must belong to a sync group before it can invite another device".to_string())?;
        let transports = build_configured_transports(&self.database).await;
        crate::enrollment::create_join_code(
            &self.database,
            &identity,
            &keys,
            &transports,
            connectors,
            expires_in_hours,
            Utc::now().timestamp_millis(),
        )
        .await
    }

    /// Cancels an open join code and rotates keys. Holds the sync gate so
    /// the rotation never races a cycle's own.
    pub async fn cancel_join_code(&self, invitation_cid: &str) -> Result<(), String> {
        let _guard = self.gate.lock().await;
        let identity = self.database.local_device_identity()?;
        let keys = self.database.local_replicated_keys()?;
        let transports = build_configured_transports(&self.database).await;
        crate::enrollment::cancel_join_code(
            &self.database,
            &identity,
            &keys,
            &crate::enrollment::KeychainEpochKeyStore,
            &transports,
            invitation_cid,
        )
        .await
    }

    /// Joins a sync group with a pasted join code, setting up its
    /// connectors on this device.
    pub async fn join_with_code(
        &self,
        code: &str,
        folders: &[crate::enrollment::JoinFolderChoice],
        credentials: Vec<crate::enrollment::JoinCredentialsChoice>,
    ) -> Result<(), String> {
        let _guard = self.gate.lock().await;
        let identity = self.database.local_device_identity()?;
        crate::enrollment::join_with_code(
            &self.database,
            &identity,
            &crate::enrollment::KeychainEpochKeyStore,
            code,
            folders,
            credentials,
            Utc::now().timestamp_millis(),
        )
        .await
    }

    /// Joins an existing sync space using only a recovery phrase.
    pub async fn join_with_recovery_phrase(&self, phrase: &str) -> Result<(), String> {
        let _guard = self.gate.lock().await;
        let identity = self.database.local_device_identity()?;
        let transports = build_configured_transports(&self.database).await;
        crate::enrollment::join_with_recovery_phrase(&self.database, &identity, &crate::enrollment::KeychainEpochKeyStore, phrase, &transports).await
    }

    /// Spawns the periodic push/pull loop. Only ever does real work when
    /// [`enabled`] is true and at least one transport is configured;
    /// otherwise `sync_once` returns immediately, so this is cheap to
    /// spawn unconditionally at startup. `on_synced` runs after every
    /// successful cycle, so the app can react to entities a pull removed.
    pub fn spawn<F>(self, handle: tauri::AppHandle, on_synced: F)
    where
        F: Fn(&tauri::AppHandle) + Send + Sync + 'static,
    {
        tauri::async_runtime::spawn(async move {
            let mut interval = tokio::time::interval(SYNC_INTERVAL);
            let mut was_active = false;
            loop {
                // `interval` ticks immediately once, so a device catches up
                // on startup instead of waiting for the first 30-second tick.
                interval.tick().await;
                let active = self.database.replicated_sync_active().unwrap_or(false);
                // While the feature is off, `sync_once` does nothing, so a
                // status event would only make every listener re-read (and
                // re-save) preferences for no change. The tick right after
                // it turns off still reports, so status views settle.
                if !should_report_cycle(was_active, active) {
                    continue;
                }
                was_active = active;
                match self.sync_once().await {
                    Ok(()) => on_synced(&handle),
                    Err(error) => log::warn!(target: "replicated_sync", "periodic sync failed: {error}"),
                }
                use tauri::Emitter;
                let _ = handle.emit("replicated-sync-status", ());
            }
        });
    }
}

/// Whether a periodic cycle should run its callback and status event: always
/// while the feature is active, plus the first tick after it turns off.
fn should_report_cycle(was_active: bool, active: bool) -> bool {
    active || was_active
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeSet;

    #[test]
    fn periodic_cycles_stay_silent_while_the_feature_is_off() {
        assert!(!should_report_cycle(false, false));
        assert!(should_report_cycle(false, true));
        assert!(should_report_cycle(true, true));
        // One last report when the feature turns off, then silence.
        assert!(should_report_cycle(true, false));
    }

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

    fn values(db: &Database, entity_id: &str) -> Vec<(String, i64, Option<String>)> {
        let connection = db.connection().unwrap();
        let mut statement = connection
            .prepare("SELECT field, counter, value FROM sync_values WHERE entity_id=?1 ORDER BY field, counter")
            .unwrap();
        let rows = statement
            .query_map(params![entity_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        rows
    }

    fn own_counter(db: &Database) -> i64 {
        db.connection()
            .unwrap()
            .query_row(
                "SELECT c.counter FROM sync_context c JOIN sync_devices d ON d.device_id = c.device_id WHERE d.is_self = 1",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    #[test]
    fn recording_a_creation_writes_every_field_and_existence_as_one_write() {
        let db = Database::open_memory();
        db.record_replicated_write(EntityType::Snippet, "one", &fields(&["name", "body"]), &json!({"name": "n", "body": "b"}))
            .unwrap();
        // _entity=true, name, and body, all the device's first write.
        assert_eq!(
            values(&db, "one"),
            vec![
                ("_entity".to_string(), 1, Some("true".to_string())),
                ("body".to_string(), 1, Some("\"b\"".to_string())),
                ("name".to_string(), 1, Some("\"n\"".to_string())),
            ]
        );
        assert_eq!(own_counter(&db), 1);
        assert!(db.local_state_status().unwrap().0, "a write marks the replica changed");
    }

    #[test]
    fn a_later_update_replaces_the_value_and_keeps_existence() {
        let db = Database::open_memory();
        db.record_replicated_write(EntityType::Snippet, "one", &fields(&["name"]), &json!({"name": "n"})).unwrap();
        db.record_replicated_write(EntityType::Snippet, "one", &fields(&["name"]), &json!({"name": "n2"})).unwrap();
        assert_eq!(
            values(&db, "one"),
            vec![("_entity".to_string(), 1, Some("true".to_string())), ("name".to_string(), 2, Some("\"n2\"".to_string()))]
        );
        assert_eq!(own_counter(&db), 2);
    }

    #[test]
    fn every_write_advances_lamport_time() {
        let db = Database::open_memory();
        let lamport = |db: &Database| -> i64 {
            db.connection().unwrap().query_row("SELECT MAX(lamport) FROM sync_values", [], |row| row.get(0)).unwrap()
        };
        db.record_replicated_write(EntityType::Snippet, "one", &fields(&["name"]), &json!({"name": "n"})).unwrap();
        let first = lamport(&db);
        db.record_replicated_write(EntityType::Snippet, "two", &fields(&["name"]), &json!({"name": "n"})).unwrap();
        assert!(lamport(&db) > first);
    }

    #[test]
    fn deletion_removes_every_value_and_leaves_no_tombstone() {
        let db = Database::open_memory();
        db.record_replicated_write(EntityType::Snippet, "one", &fields(&["name"]), &json!({"name": "n"})).unwrap();
        db.record_replicated_deletion(EntityType::Snippet, "one").unwrap();
        assert!(values(&db, "one").is_empty());
        // The context still records the deleted write, which is what keeps
        // a peer's older copy from bringing it back.
        assert_eq!(own_counter(&db), 1);
        assert!(!db.entity_recorded(EntityType::Snippet, "one").unwrap());
    }

    #[test]
    fn remote_projection_suppresses_local_recording() {
        let db = Database::open_memory();
        db.with_remote_projection(|| {
            db.record_replicated_write(EntityType::Snippet, "one", &fields(&["name"]), &json!({"name": "n"}))
        })
        .unwrap();
        assert!(values(&db, "one").is_empty());
        assert!(!db.is_projecting_remote_operation());
    }

    #[test]
    fn field_winner_and_known_fields_read_the_replica() {
        let db = Database::open_memory();
        assert_eq!(db.resolve_field_winner(EntityType::Snippet, "one", "name").unwrap(), None);
        db.record_replicated_write(EntityType::Snippet, "one", &fields(&["name"]), &json!({"name": "first"})).unwrap();
        db.record_replicated_write(EntityType::Snippet, "one", &fields(&["name"]), &json!({"name": "second"})).unwrap();
        assert_eq!(db.resolve_field_winner(EntityType::Snippet, "one", "name").unwrap(), Some(json!("second")));
        let mut known = db.known_fields(EntityType::Snippet, "one").unwrap();
        known.sort();
        assert_eq!(known, vec![ENTITY_EXISTENCE_FIELD.to_string(), "name".to_string()]);
        assert!(db.entity_recorded(EntityType::Snippet, "one").unwrap());
        assert!(!db.entity_recorded(EntityType::Snippet, "two").unwrap());
    }

    #[test]
    fn remote_projection_does_not_suppress_a_local_write_on_another_thread() {
        let db = Arc::new(Database::open_memory());
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let projecting = {
            let db = Arc::clone(&db);
            std::thread::spawn(move || {
                db.with_remote_projection(|| {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    db.record_replicated_write(EntityType::Snippet, "remote", &fields(&["name"]), &json!({"name": "r"}))
                })
                .unwrap();
            })
        };
        entered_rx.recv().unwrap();
        // A local command on another thread, mid-projection.
        assert!(!db.is_projecting_remote_operation());
        db.record_replicated_write(EntityType::Snippet, "local", &fields(&["name"]), &json!({"name": "l"})).unwrap();
        release_tx.send(()).unwrap();
        projecting.join().unwrap();
        assert!(!values(&db, "local").is_empty());
        assert!(values(&db, "remote").is_empty());
    }

    #[test]
    fn nested_remote_projection_stays_engaged_until_the_outer_call_ends() {
        let db = Database::open_memory();
        db.with_remote_projection(|| {
            db.with_remote_projection(|| Ok(()))?;
            assert!(db.is_projecting_remote_operation());
            Ok(())
        })
        .unwrap();
        assert!(!db.is_projecting_remote_operation());
    }

    #[test]
    fn remote_projection_flag_is_restored_after_a_panic() {
        let db = Database::open_memory();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            db.with_remote_projection::<()>(|| panic!("boom"))
        }));
        assert!(result.is_err());
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

    #[test]
    fn conflicted_contact_does_not_block_other_entities_and_is_retried_later() {
        let db=Database::open_memory();
        let owner=db.save_contact_profile(&crate::models::SaveContactRequest{birthday:None,keep_in_touch:None,
            id:Some("local-owner".into()),display_name:Some("Local owner".into()),role:None,company:None,
            location:None,bio:None,notes:Some("Keep local profile".into()),links:vec![],photo_data:None,
            favorite:false,addresses:vec!["shared@example.com".into()],
        }).unwrap();
        let contact=json!({"id":"remote-contact","displayName":"Remote person","role":null,"company":null,"location":null,"bio":null,"notes":null,"links":[],"photoData":null,"favorite":false,"addresses":["shared@example.com"]});
        db.record_replicated_write(
            EntityType::Contact,"remote-contact",
            &fields(&["id","displayName","role","company","location","bio","notes","links","photoData","favorite","addresses"]),
            &contact,
        ).unwrap();
        db.record_replicated_write(
            EntityType::Snippet,"remote-snippet",
            &fields(&["id","name","body","createdAt"]),
            &snippet_payload("remote-snippet","Materialized despite contact conflict"),
        ).unwrap();

        db.materialize_touched_entities(&[(EntityType::Contact,"remote-contact".into()),(EntityType::Snippet,"remote-snippet".into())]).unwrap();
        let snippet:Option<String>=db.connection().unwrap().query_row("SELECT name FROM snippets WHERE id='remote-snippet'",[],|row|row.get(0)).unwrap();
        assert_eq!(snippet.as_deref(),Some("Materialized despite contact conflict"));
        let queued:i64=db.connection().unwrap().query_row("SELECT COUNT(*) FROM pending_entity_materializations WHERE entity_type='contact' AND entity_id='remote-contact'",[],|row|row.get(0)).unwrap();
        assert_eq!(queued,1);
        assert!(db.get_contact_profile("remote-contact").unwrap().is_none());

        db.save_contact_profile(&crate::models::SaveContactRequest{birthday:None,keep_in_touch:None,
            id:Some(owner.id),display_name:Some("Local owner".into()),role:None,company:None,
            location:None,bio:None,notes:Some("Keep local profile".into()),links:vec![],photo_data:None,
            favorite:false,addresses:vec!["owner@example.com".into()],
        }).unwrap();
        // No newer remote snapshot is needed; pending projection state is
        // retried even when this call has no newly touched entities.
        db.materialize_touched_entities(&[]).unwrap();
        let queued:i64=db.connection().unwrap().query_row("SELECT COUNT(*) FROM pending_entity_materializations WHERE entity_type='contact' AND entity_id='remote-contact'",[],|row|row.get(0)).unwrap();
        assert_eq!(queued,0);
        let remote=db.get_contact_profile("remote-contact").unwrap().unwrap();
        assert_eq!(remote.addresses,vec!["shared@example.com"]);
    }

    #[test]
    fn a_contact_record_from_an_older_build_keeps_local_birthday_and_keep_in_touch() {
        let db=Database::open_memory();
        db.save_contact_profile(&crate::models::SaveContactRequest{birthday:Some("03-14".into()),keep_in_touch:None,
            id:Some("kit-contact".into()),display_name:Some("Local".into()),role:None,company:None,
            location:None,bio:None,notes:None,links:vec![],photo_data:None,
            favorite:false,addresses:vec!["kit@example.com".into()],
        }).unwrap();
        let local=db.set_keep_in_touch(&["kit-contact".into()],Some(21)).unwrap().remove(0);
        // The exact key set every build before 0.67.0 sends.
        let legacy=json!({"id":"kit-contact","displayName":"Renamed elsewhere","role":null,"company":null,"location":null,"bio":null,"notes":null,"links":[],"photoData":null,"favorite":false,"addresses":["kit@example.com"]});
        db.upsert_synced_contact(&legacy).unwrap();
        let merged=db.get_contact_profile("kit-contact").unwrap().unwrap();
        assert_eq!(merged.display_name.as_deref(),Some("Renamed elsewhere"));
        assert_eq!(merged.birthday.as_deref(),Some("03-14"));
        assert_eq!(merged.keep_in_touch,local.keep_in_touch);

        // A current build's record carries both keys, so it can clear them.
        let mut current=serde_json::to_value(crate::models::ContactRecord::from(&merged)).unwrap();
        current["birthday"]=Value::Null;
        current["keepInTouch"]=json!({});
        db.upsert_synced_contact(&current).unwrap();
        let cleared=db.get_contact_profile("kit-contact").unwrap().unwrap();
        assert_eq!(cleared.birthday,None);
        assert_eq!(cleared.keep_in_touch,crate::models::KeepInTouch::default());
    }

    #[test]
    fn a_recorded_contact_survives_snapshot_and_merge_into_another_replica() {
        let database = Database::open_memory();
        let keys = test_keys(&database);
        let contact = json!({"id":"synced-contact","displayName":"Synced person","role":null,"company":null,"location":null,"bio":null,"notes":null,"links":[],"photoData":null,"favorite":false,"addresses":["synced@example.com"]});
        database.record_replicated_write(
            EntityType::Contact, "synced-contact",
            &fields(&["id","displayName","role","company","location","bio","notes","links","photoData","favorite","addresses"]),
            &contact,
        ).unwrap();

        database.take_local_snapshot(keys.device_id, 1, 1).unwrap();
        let state = database.load_replica_state().unwrap();

        let other = Database::open_memory();
        let touched = other.merge_replica_state(&state).unwrap();
        other.materialize_touched_entities(&touched).unwrap();
        let synced = other.get_contact_profile("synced-contact").unwrap().unwrap();
        assert_eq!(synced.display_name.as_deref(), Some("Synced person"));
        assert_eq!(synced.addresses, vec!["synced@example.com"]);
    }

    #[test]
    fn concurrent_contact_group_membership_edits_merge_without_a_conflict() {
        let save = |database: &Database, email: &str| {
            database
                .save_contact_profile(&crate::models::SaveContactRequest {
                    id: None, display_name: Some(email.into()), role: None, company: None, location: None,
                    bio: None, notes: None, links: Vec::new(), photo_data: None, favorite: false,
                    addresses: vec![email.into()], birthday: None, keep_in_touch: None,
                })
                .unwrap()
                .id
        };
        let record = |database: &Database, write: crate::db::contact_groups::ContactGroupWrite| {
            let payload = database.contact_group_record(&write.group.id).unwrap().unwrap();
            database.record_replicated_write(EntityType::ContactGroup, &write.group.id, &write.fields, &payload).unwrap();
            write.group
        };
        let exchange = |from: &Database, to: &Database| {
            let touched = to.merge_replica_state(&from.load_replica_state().unwrap()).unwrap();
            to.materialize_touched_entities(&touched).unwrap();
        };
        let (a, b) = (Database::open_memory(), Database::open_memory());
        let [ada, bob, cyd] = ["ada@example.com", "bob@example.com", "cyd@example.com"].map(|email| {
            let id = save(&a, email);
            assert_eq!(save(&b, email), id);
            id
        });
        let group = record(&a, a.create_contact_group("Board", &[ada.clone(), bob.clone()], &[]).unwrap());
        exchange(&a, &b);
        assert_eq!(b.get_contact_group(&group.id).unwrap().unwrap().member_ids, vec![ada.clone(), bob.clone()]);

        record(&a, a.remove_contact_group_members(&group.id, &[bob.clone()]).unwrap());
        record(&b, b.add_contact_group_members(&group.id, &[cyd.clone()], &[]).unwrap());
        exchange(&a, &b);
        exchange(&b, &a);
        for database in [&a, &b] {
            assert_eq!(database.get_contact_group(&group.id).unwrap().unwrap().member_ids, vec![ada.clone(), cyd.clone()]);
            assert!(database.list_frontier_conflicts().unwrap().is_empty());
        }

        a.delete_contact_group(&group.id).unwrap();
        a.record_replicated_deletion(EntityType::ContactGroup, &group.id).unwrap();
        exchange(&a, &b);
        assert!(b.get_contact_group(&group.id).unwrap().is_none());
        assert!(b.get_contact_profile(&ada).unwrap().is_some());
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
        database.trust_device_public_key(&device_id, &signing_key.verifying_key()).unwrap();
        LocalKeys {
            signing_key,
            k_epoch: [7u8; 32],
            key_epoch: 0,
            earlier_epoch_keys: BTreeMap::new(),
            device_id: EnvelopeDeviceId::from_bytes(device_id),
            sync_space_id: b"test-space".to_vec(),
        }
    }

    /// The same device's keys, as they would be after its group rotated to
    /// `key_epoch`, holding `earlier` for older epochs.
    fn at_epoch(keys: &LocalKeys, key_epoch: u32, k_epoch: [u8; 32], earlier: &[(u32, [u8; 32])]) -> LocalKeys {
        LocalKeys {
            signing_key: keys.signing_key.clone(),
            k_epoch,
            key_epoch,
            earlier_epoch_keys: earlier.iter().copied().collect(),
            device_id: keys.device_id,
            sync_space_id: keys.sync_space_id.clone(),
        }
    }

    fn trust(database: &Database, keys: &LocalKeys) {
        database.trust_device_public_key(keys.device_id.as_bytes(), &keys.signing_key.verifying_key()).unwrap();
    }

    fn fake(name: &str) -> Vec<Arc<dyn SyncTransport>> {
        vec![Arc::new(FakeTransport::new(name))]
    }

    /// A local snippet write as the app makes one: the app table first, then
    /// the replica.
    fn write_snippet(database: &Database, id: &str, name: &str) {
        database
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO snippets(id, name, body, created_at) VALUES (?1, ?2, 'body', '2026-01-01T00:00:00Z')
                 ON CONFLICT(id) DO UPDATE SET name=excluded.name",
                params![id, name],
            )
            .unwrap();
        database
            .record_replicated_write(EntityType::Snippet, id, &fields(&["id", "name", "body", "createdAt"]), &snippet_payload(id, name))
            .unwrap();
    }

    fn rename_snippet(database: &Database, id: &str, name: &str) {
        database.connection().unwrap().execute("UPDATE snippets SET name=?2 WHERE id=?1", params![id, name]).unwrap();
        database
            .record_replicated_write(EntityType::Snippet, id, &fields(&["name"]), &snippet_payload(id, name))
            .unwrap();
    }

    fn delete_snippet(database: &Database, id: &str) {
        database.connection().unwrap().execute("DELETE FROM snippets WHERE id=?1", params![id]).unwrap();
        database.record_replicated_deletion(EntityType::Snippet, id).unwrap();
    }

    fn stored_snippet_name(database: &Database, id: &str) -> Option<String> {
        database
            .connection()
            .unwrap()
            .query_row("SELECT name FROM snippets WHERE id=?1", params![id], |row| row.get(0))
            .optional()
            .unwrap()
    }

    fn state_objects(database: &Database) -> Vec<(String, String, i64)> {
        let connection = database.connection().unwrap();
        let mut statement = connection
            .prepare("SELECT cid, object_kind, state_sequence FROM sync_objects WHERE state_sequence IS NOT NULL ORDER BY object_kind, cid")
            .unwrap();
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap().collect::<Result<_, _>>().unwrap();
        rows
    }

    async fn published_head(transports: &[Arc<dyn SyncTransport>], keys: &LocalKeys) -> Option<SignedDeviceHead> {
        transports[0]
            .resolve_heads(&[HeadLocator { device_id: keys.device_id, remote_id: None }])
            .await
            .unwrap()
            .into_iter()
            .next()
    }

    #[test]
    fn snapshot_capture_keeps_dirty_until_storage_and_preserves_a_racing_write() {
        let database = Database::open_memory();
        let keys = test_keys(&database);
        write_snippet(&database, "one", "before");

        let (_, generation) = database.take_local_snapshot(keys.device_id, 1, 1).unwrap();
        assert!(database.local_state_status().unwrap().0, "a crash before storage must leave the replica dirty");

        rename_snippet(&database, "one", "after");
        database
            .with_transaction(|tx| crate::sync_state::clear_dirty_if_generation(tx, generation))
            .unwrap();
        assert!(database.local_state_status().unwrap().0, "storing an older snapshot must not clear a later write");
    }

    #[tokio::test]
    async fn sealing_stores_one_snapshot_and_only_reseals_after_a_change() {
        let database = Database::open_memory();
        let keys = test_keys(&database);
        let transport = TransportInstanceId("folder-a".to_string());
        write_snippet(&database, "one", "n");

        assert!(database.seal_local_state(&keys, std::slice::from_ref(&transport)).unwrap());
        let first = state_objects(&database);
        assert!(first.iter().any(|(_, kind, sequence)| kind == "state_index" && *sequence == 1));
        assert!(first.iter().any(|(_, kind, _)| kind == "state_chunk"));
        let (pending, _, _) = database.delivery_counts("folder-a").unwrap();
        assert_eq!(pending as usize, first.len());

        assert!(!database.seal_local_state(&keys, std::slice::from_ref(&transport)).unwrap(), "nothing changed");

        rename_snippet(&database, "one", "n2");
        assert!(database.seal_local_state(&keys, std::slice::from_ref(&transport)).unwrap());
        let second = state_objects(&database);
        assert!(second.iter().all(|(_, _, sequence)| *sequence == 2), "only the current snapshot is kept locally");
    }

    #[tokio::test]
    async fn a_rotation_reseals_the_replica_under_the_new_key() {
        let database = Database::open_memory();
        let keys = test_keys(&database);
        write_snippet(&database, "one", "n");
        assert!(database.seal_local_state(&keys, &[]).unwrap());
        assert!(!database.seal_local_state(&keys, &[]).unwrap());
        let rotated = at_epoch(&keys, 1, [9u8; 32], &[(0, keys.k_epoch)]);
        assert!(database.seal_local_state(&rotated, &[]).unwrap());
        assert_eq!(database.local_state_status().unwrap(), (false, 2, Some(1)));
    }

    #[tokio::test]
    async fn push_delivers_the_snapshot_and_points_the_head_at_it() {
        let database = Database::open_memory();
        let keys = test_keys(&database);
        let transports = fake("folder-a");
        write_snippet(&database, "one", "n");

        let outcome = push_local_state(&database, &keys, &transports).await.unwrap();
        assert!(outcome.sealed_snapshot);
        assert_eq!(outcome.failed, 0);
        assert!(outcome.delivered > 0);
        let (pending, delivered, failed) = database.delivery_counts("folder-a").unwrap();
        assert_eq!((pending, failed), (0, 0));
        assert!(delivered > 0);

        let head = published_head(&transports, &keys).await.unwrap().head;
        assert_eq!(head.state_sequence, 1);
        let index_cid = state_objects(&database).into_iter().find(|(_, kind, _)| kind == "state_index").unwrap().0;
        assert_eq!(head.state_cid.as_deref(), Some(index_cid.as_str()));
    }

    #[tokio::test]
    async fn push_keeps_a_transient_failure_pending_for_retry() {
        let database = Database::open_memory();
        write_snippet(&database, "one", "n");
        let keys = test_keys(&database);
        let fake = FakeTransport::new("folder-a");
        fake.inject_transient_outage(1000);
        let transports: Vec<Arc<dyn SyncTransport>> = vec![Arc::new(fake)];

        let outcome = push_local_state(&database, &keys, &transports).await.unwrap();
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
    async fn a_head_keeps_naming_the_old_snapshot_until_the_new_one_has_arrived_then_the_old_one_goes() {
        let database = Database::open_memory();
        let keys = test_keys(&database);
        let flaky = Arc::new(FakeTransport::new("flaky"));
        let transports: Vec<Arc<dyn SyncTransport>> = vec![flaky.clone()];
        write_snippet(&database, "one", "n");
        push_local_state(&database, &keys, &transports).await.unwrap();
        let first_objects: Vec<String> = state_objects(&database).into_iter().map(|(cid, _, _)| cid).collect();

        rename_snippet(&database, "one", "n2");
        flaky.inject_transient_outage(1);
        push_local_state(&database, &keys, &transports).await.unwrap();
        assert_eq!(published_head(&transports, &keys).await.unwrap().head.state_sequence, 1);
        for cid in &first_objects {
            assert!(transports[0].get_object(&TransportCid(cid.clone())).await.is_ok(), "the snapshot the head names stays");
        }

        database.connection().unwrap().execute("UPDATE sync_deliveries SET retry_at=NULL", []).unwrap();
        push_local_state(&database, &keys, &transports).await.unwrap();
        assert_eq!(published_head(&transports, &keys).await.unwrap().head.state_sequence, 2);
        for cid in &first_objects {
            assert_eq!(transports[0].get_object(&TransportCid(cid.clone())).await, Err(TransportError::NotFound));
        }
        let retired: i64 = database.connection().unwrap().query_row("SELECT COUNT(*) FROM sync_retired_objects", [], |row| row.get(0)).unwrap();
        assert_eq!(retired, 0);
    }

    #[tokio::test]
    async fn two_devices_converge_through_a_shared_transport() {
        let (database_a, database_b) = (Database::open_memory(), Database::open_memory());
        let transports = fake("shared");
        write_snippet(&database_b, "b-1", "From B");
        let keys_b = test_keys(&database_b);
        push_local_state(&database_b, &keys_b, &transports).await.unwrap();

        let keys_a = test_keys(&database_a);
        trust(&database_a, &keys_b);
        let outcome = pull_from_transports(&database_a, &keys_a, &transports).await.unwrap();
        assert_eq!((outcome.merged_states, outcome.failed_transports), (1, 0));
        assert_eq!(stored_snippet_name(&database_a, "b-1").as_deref(), Some("From B"));

        // Nothing new: the same snapshot isn't merged again.
        assert_eq!(pull_from_transports(&database_a, &keys_a, &transports).await.unwrap().merged_states, 0);
    }

    #[tokio::test]
    async fn pull_skips_a_head_from_an_untrusted_device() {
        let (database_a, database_b) = (Database::open_memory(), Database::open_memory());
        let transports = fake("shared");
        write_snippet(&database_b, "b-1", "From B");
        let keys_b = test_keys(&database_b);
        push_local_state(&database_b, &keys_b, &transports).await.unwrap();

        // A never trusts B.
        let keys_a = test_keys(&database_a);
        assert_eq!(pull_from_transports(&database_a, &keys_a, &transports).await.unwrap().merged_states, 0);
        assert_eq!(stored_snippet_name(&database_a, "b-1"), None);
    }

    #[tokio::test]
    async fn pull_skips_a_trusted_head_from_another_sync_space_before_observing_or_fetching_it() {
        let (database_a, database_b) = (Database::open_memory(), Database::open_memory());
        let transports = fake("shared");
        let (keys_a, keys_b) = (test_keys(&database_a), test_keys(&database_b));
        trust(&database_a, &keys_b);

        let foreign_head = DeviceHead {
            sync_space_id: b"another-sync-space".to_vec(),
            device_id: keys_b.device_id,
            epoch: 7,
            state_sequence: 1,
            state_cid: Some(TransportCid::for_bytes(b"foreign snapshot index").0),
            published_at_ms: 1_800_000_000_000,
        };
        let signed_foreign_head = sign_device_head(&keys_b.signing_key, foreign_head).unwrap();
        transports[0].publish_head(&signed_foreign_head).await.unwrap();

        let outcome = pull_from_transports(&database_a, &keys_a, &transports).await.unwrap();
        assert_eq!((outcome.merged_states, outcome.failed_transports), (0, 0));
        let observed_foreign_heads: i64 = database_a
            .connection()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM sync_remote_states WHERE device_id=?1 AND last_head_epoch IS NOT NULL",
                params![encode_id(keys_b.device_id.as_bytes())],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(observed_foreign_heads, 0);
    }

    #[tokio::test]
    async fn concurrent_edits_stay_a_visible_conflict_until_one_device_resolves_it() {
        let (database_a, database_b) = (Database::open_memory(), Database::open_memory());
        let transports = fake("shared");
        let (keys_a, keys_b) = (test_keys(&database_a), test_keys(&database_b));
        trust(&database_a, &keys_b);
        trust(&database_b, &keys_a);
        write_snippet(&database_a, "note", "original");
        push_local_state(&database_a, &keys_a, &transports).await.unwrap();
        pull_from_transports(&database_b, &keys_b, &transports).await.unwrap();

        // Both rename it before hearing from the other.
        rename_snippet(&database_a, "note", "from A");
        rename_snippet(&database_b, "note", "from B");
        for (database, keys) in [(&database_a, &keys_a), (&database_b, &keys_b)] {
            push_local_state(database, keys, &transports).await.unwrap();
        }
        for (database, keys) in [(&database_a, &keys_a), (&database_b, &keys_b)] {
            pull_from_transports(database, keys, &transports).await.unwrap();
        }
        let conflicts_a = database_a.list_frontier_conflicts().unwrap();
        let conflicts_b = database_b.list_frontier_conflicts().unwrap();
        assert_eq!(conflicts_a.len(), 1);
        assert_eq!(conflicts_a[0].field, "name");
        assert_eq!(conflicts_a[0].candidates.len(), 2);
        assert_eq!(conflicts_b.len(), 1);
        // Both show the same working value.
        assert_eq!(stored_snippet_name(&database_a, "note"), stored_snippet_name(&database_b, "note"));

        // A keeps B's name; the choice settles the field on B too.
        let from_b = conflicts_a[0].candidates.iter().find(|candidate| candidate.value == Some(serde_json::json!("from B"))).unwrap();
        database_a.resolve_frontier_conflict(EntityType::Snippet, "note", "name", &from_b.operation_id).unwrap();
        push_local_state(&database_a, &keys_a, &transports).await.unwrap();
        pull_from_transports(&database_b, &keys_b, &transports).await.unwrap();
        assert!(database_a.list_frontier_conflicts().unwrap().is_empty());
        assert!(database_b.list_frontier_conflicts().unwrap().is_empty());
        assert_eq!(stored_snippet_name(&database_b, "note").as_deref(), Some("from B"));
    }

    #[tokio::test]
    async fn a_deletion_reaches_peers_and_an_older_copy_never_brings_it_back() {
        let (database_a, database_b, database_c) = (Database::open_memory(), Database::open_memory(), Database::open_memory());
        let transports = fake("shared");
        let (keys_a, keys_b, keys_c) = (test_keys(&database_a), test_keys(&database_b), test_keys(&database_c));
        for (database, peers) in [(&database_a, [&keys_b, &keys_c]), (&database_b, [&keys_a, &keys_c]), (&database_c, [&keys_a, &keys_b])] {
            for peer in peers {
                trust(database, peer);
            }
        }
        write_snippet(&database_a, "doomed", "doomed");
        push_local_state(&database_a, &keys_a, &transports).await.unwrap();
        // C merges it, then goes quiet with its copy.
        pull_from_transports(&database_c, &keys_c, &transports).await.unwrap();
        assert_eq!(stored_snippet_name(&database_c, "doomed").as_deref(), Some("doomed"));
        push_local_state(&database_c, &keys_c, &transports).await.unwrap();

        delete_snippet(&database_a, "doomed");
        push_local_state(&database_a, &keys_a, &transports).await.unwrap();
        pull_from_transports(&database_b, &keys_b, &transports).await.unwrap();
        assert_eq!(stored_snippet_name(&database_b, "doomed"), None);
        let values: i64 = database_b.connection().unwrap().query_row("SELECT COUNT(*) FROM sync_values", [], |row| row.get(0)).unwrap();
        assert_eq!(values, 0, "a deletion leaves no tombstone behind");

        // C's older snapshot still holds the snippet; merging it brings
        // nothing back, and C itself drops it once it hears of the deletion.
        pull_from_transports(&database_a, &keys_a, &transports).await.unwrap();
        assert_eq!(stored_snippet_name(&database_a, "doomed"), None);
        pull_from_transports(&database_c, &keys_c, &transports).await.unwrap();
        assert_eq!(stored_snippet_name(&database_c, "doomed"), None);
    }

    #[tokio::test]
    async fn a_device_away_for_months_merges_current_states_and_keeps_its_own_edits() {
        let (database_a, database_b) = (Database::open_memory(), Database::open_memory());
        let transports = fake("shared");
        let (keys_a, keys_b) = (test_keys(&database_a), test_keys(&database_b));
        trust(&database_a, &keys_b);
        trust(&database_b, &keys_a);
        write_snippet(&database_a, "kept", "v1");
        push_local_state(&database_a, &keys_a, &transports).await.unwrap();
        pull_from_transports(&database_b, &keys_b, &transports).await.unwrap();

        // B goes away. A changes and deletes things for a long time.
        write_snippet(&database_b, "written-offline", "offline");
        for round in 0..50 {
            rename_snippet(&database_a, "kept", &format!("v{}", round + 2));
            write_snippet(&database_a, &format!("temp-{round}"), "temp");
            delete_snippet(&database_a, &format!("temp-{round}"));
            push_local_state(&database_a, &keys_a, &transports).await.unwrap();
        }

        // B returns: one merge of A's current state catches it up, and its
        // own offline write reaches A.
        let outcome = pull_from_transports(&database_b, &keys_b, &transports).await.unwrap();
        assert_eq!(outcome.merged_states, 1);
        assert_eq!(stored_snippet_name(&database_b, "kept").as_deref(), Some("v51"));
        push_local_state(&database_b, &keys_b, &transports).await.unwrap();
        pull_from_transports(&database_a, &keys_a, &transports).await.unwrap();
        assert_eq!(stored_snippet_name(&database_a, "written-offline").as_deref(), Some("offline"));
        // The connector holds one snapshot per device, however long the history.
        let snapshots = transports[0].scan(None).await.unwrap().unwrap().objects.len();
        assert!(snapshots <= 6, "expected a couple of objects per device, found {snapshots}");
    }

    #[tokio::test]
    async fn a_device_relays_what_it_merged_to_its_other_connectors() {
        let (database_a, database_b, database_c) = (Database::open_memory(), Database::open_memory(), Database::open_memory());
        let (only_a, only_c) = (fake("a-and-b"), fake("b-and-c"));
        let both: Vec<Arc<dyn SyncTransport>> = only_a.iter().chain(only_c.iter()).cloned().collect();
        let (keys_a, keys_b, keys_c) = (test_keys(&database_a), test_keys(&database_b), test_keys(&database_c));
        trust(&database_b, &keys_a);
        trust(&database_c, &keys_b);

        write_snippet(&database_a, "from-a", "From A");
        push_local_state(&database_a, &keys_a, &only_a).await.unwrap();
        pull_from_transports(&database_b, &keys_b, &both).await.unwrap();
        // B's own snapshot now includes A's write, and goes to both of B's
        // connectors, so C gets it without ever reaching A's.
        push_local_state(&database_b, &keys_b, &both).await.unwrap();
        pull_from_transports(&database_c, &keys_c, &only_c).await.unwrap();
        assert_eq!(stored_snippet_name(&database_c, "from-a").as_deref(), Some("From A"));
    }

    #[tokio::test]
    async fn each_peer_snapshot_opens_with_the_key_for_its_epoch() {
        let (database_a, database_b) = (Database::open_memory(), Database::open_memory());
        let transports = fake("shared");
        let keys_a = test_keys(&database_a);
        write_snippet(&database_a, "sealed-at-epoch-0", "old key");
        push_local_state(&database_a, &keys_a, &transports).await.unwrap();

        let keys_b = test_keys(&database_b);
        trust(&database_b, &keys_a);
        let missing = at_epoch(&keys_b, 1, [9u8; 32], &[]);
        let outcome = pull_from_transports(&database_b, &missing, &transports).await.unwrap();
        assert_eq!((outcome.merged_states, outcome.failed_transports), (0, 1));

        let complete = at_epoch(&keys_b, 1, [9u8; 32], &[(0, [7u8; 32])]);
        let outcome = pull_from_transports(&database_b, &complete, &transports).await.unwrap();
        assert_eq!((outcome.merged_states, outcome.failed_transports), (1, 0));
        assert_eq!(stored_snippet_name(&database_b, "sealed-at-epoch-0").as_deref(), Some("old key"));
    }

    fn signed_head_for(keys: &LocalKeys, state_sequence: u64, state_cid: Option<String>) -> SignedDeviceHead {
        sign_device_head(
            &keys.signing_key,
            DeviceHead {
                sync_space_id: keys.sync_space_id.clone(),
                device_id: keys.device_id,
                epoch: keys.key_epoch,
                state_sequence,
                state_cid,
                published_at_ms: crate::sync_policy::now_ms(),
            },
        )
        .unwrap()
    }

    #[tokio::test]
    async fn a_head_that_disagrees_with_its_snapshot_merges_nothing() {
        let (database_a, database_b) = (Database::open_memory(), Database::open_memory());
        let transports = fake("shared");
        let keys_a = test_keys(&database_a);
        write_snippet(&database_a, "one", "n");
        push_local_state(&database_a, &keys_a, &transports).await.unwrap();
        let index_cid = published_head(&transports, &keys_a).await.unwrap().head.state_cid;

        // A head claiming a later snapshot than the one it names.
        transports[0].publish_head(&signed_head_for(&keys_a, 7, index_cid)).await.unwrap();
        let keys_b = test_keys(&database_b);
        trust(&database_b, &keys_a);
        let outcome = pull_from_transports(&database_b, &keys_b, &transports).await.unwrap();
        assert_eq!((outcome.merged_states, outcome.failed_transports), (0, 1));
        assert_eq!(stored_snippet_name(&database_b, "one"), None);
    }

    #[tokio::test]
    async fn a_stale_head_is_never_merged_again() {
        let (database_a, database_b) = (Database::open_memory(), Database::open_memory());
        let transports = fake("shared");
        let lagging = fake("lagging");
        let keys_a = test_keys(&database_a);
        write_snippet(&database_a, "one", "first");
        push_local_state(&database_a, &keys_a, &[transports[0].clone(), lagging[0].clone()]).await.unwrap();
        rename_snippet(&database_a, "one", "second");
        push_local_state(&database_a, &keys_a, &transports).await.unwrap();

        let keys_b = test_keys(&database_b);
        trust(&database_b, &keys_a);
        pull_from_transports(&database_b, &keys_b, &transports).await.unwrap();
        // The lagging connector still names A's first snapshot: it's older
        // than what B merged, so it isn't fetched at all.
        let outcome = pull_from_transports(&database_b, &keys_b, &lagging).await.unwrap();
        assert_eq!((outcome.merged_states, outcome.failed_transports), (0, 0));
        assert_eq!(stored_snippet_name(&database_b, "one").as_deref(), Some("second"));
    }

    #[tokio::test]
    async fn the_same_snapshot_from_two_connectors_merges_once() {
        let (database_a, database_b) = (Database::open_memory(), Database::open_memory());
        let both: Vec<Arc<dyn SyncTransport>> = vec![Arc::new(FakeTransport::new("one")), Arc::new(FakeTransport::new("two"))];
        let keys_a = test_keys(&database_a);
        write_snippet(&database_a, "one", "n");
        push_local_state(&database_a, &keys_a, &both).await.unwrap();
        let keys_b = test_keys(&database_b);
        trust(&database_b, &keys_a);
        let outcome = pull_from_transports(&database_b, &keys_b, &both).await.unwrap();
        assert_eq!((outcome.merged_states, outcome.failed_transports), (1, 0));
    }

    #[tokio::test]
    async fn a_device_restored_from_an_old_backup_never_reuses_a_write_counter() {
        let (database_a, database_b) = (Database::open_memory(), Database::open_memory());
        let transports = fake("shared");
        let keys_a = test_keys(&database_a);
        let keys_b = test_keys(&database_b);
        trust(&database_a, &keys_b);
        trust(&database_b, &keys_a);
        for round in 0..3 {
            write_snippet(&database_a, &format!("s{round}"), "n");
        }
        push_local_state(&database_a, &keys_a, &transports).await.unwrap();
        pull_from_transports(&database_b, &keys_b, &transports).await.unwrap();
        push_local_state(&database_b, &keys_b, &transports).await.unwrap();

        // A's database is replaced by an empty one with the same identity,
        // as a restore from before any of those writes would leave it.
        let restored = Database::open_memory();
        restored
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO sync_devices(device_id, status, is_self) VALUES (?1, 'active', 1)",
                params![encode_id(keys_a.device_id.as_bytes())],
            )
            .unwrap();
        let restored_keys = LocalKeys { signing_key: keys_a.signing_key.clone(), ..at_epoch(&keys_a, 0, [7u8; 32], &[]) };
        trust(&restored, &keys_b);
        pull_from_transports(&restored, &restored_keys, &transports).await.unwrap();
        write_snippet(&restored, "after-restore", "new");
        let counter: i64 = restored
            .connection()
            .unwrap()
            .query_row("SELECT counter FROM sync_values WHERE entity_id='after-restore' AND field='name'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(counter, 4, "the next write continues after what peers already saw");
    }

    #[tokio::test]
    async fn an_unchanged_head_is_republished_only_when_the_heartbeat_is_due() {
        let database = Database::open_memory();
        let transports = fake("shared");
        let keys = test_keys(&database);
        let start = 1_800_000_000_000;
        crate::sync_policy::set_test_clock(Some(start));

        // An empty replica still has a snapshot and a head.
        push_local_state(&database, &keys, &transports).await.unwrap();
        let head = published_head(&transports, &keys).await.unwrap();
        assert_eq!((head.head.published_at_ms, head.head.state_sequence), (start, 1));

        crate::sync_policy::set_test_clock(Some(start + crate::sync_policy::HEAD_HEARTBEAT_MS - 1));
        push_local_state(&database, &keys, &transports).await.unwrap();
        assert_eq!(published_head(&transports, &keys).await.unwrap().head.published_at_ms, start);

        let due = start + crate::sync_policy::HEAD_HEARTBEAT_MS;
        crate::sync_policy::set_test_clock(Some(due));
        push_local_state(&database, &keys, &transports).await.unwrap();
        assert_eq!(published_head(&transports, &keys).await.unwrap().head.published_at_ms, due);

        // A change is published at once.
        crate::sync_policy::set_test_clock(Some(due + 1));
        write_snippet(&database, "note", "n");
        push_local_state(&database, &keys, &transports).await.unwrap();
        let head = published_head(&transports, &keys).await.unwrap().head;
        assert_eq!((head.published_at_ms, head.state_sequence), (due + 1, 2));
        crate::sync_policy::set_test_clock(None);
    }

    #[tokio::test]
    async fn a_pull_records_each_peers_publication_time() {
        let (database_a, database_b) = (Database::open_memory(), Database::open_memory());
        let transports = fake("shared");
        crate::sync_policy::set_test_clock(Some(1_800_000_000_000));
        let keys_a = test_keys(&database_a);
        write_snippet(&database_a, "note", "n");
        push_local_state(&database_a, &keys_a, &transports).await.unwrap();

        crate::sync_policy::set_test_clock(Some(1_800_000_060_000));
        let keys_b = test_keys(&database_b);
        trust(&database_b, &keys_a);
        pull_from_transports(&database_b, &keys_b, &transports).await.unwrap();
        let (published, seen, sequence): (i64, i64, i64) = database_b
            .connection()
            .unwrap()
            .query_row(
                "SELECT last_head_published_at_ms, last_head_seen_at_ms, state_sequence FROM sync_remote_states WHERE device_id=?1",
                params![encode_id(keys_a.device_id.as_bytes())],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!((published, seen, sequence), (1_800_000_000_000, 1_800_000_060_000, 1));
        crate::sync_policy::set_test_clock(None);
    }

    #[tokio::test]
    async fn one_failed_transport_never_blocks_pull_from_a_healthy_one() {
        let (database_a, database_b) = (Database::open_memory(), Database::open_memory());
        write_snippet(&database_b, "b-1", "From B");
        let keys_b = test_keys(&database_b);
        let healthy: Arc<dyn SyncTransport> = Arc::new(FakeTransport::new("healthy"));
        let failing_fake = FakeTransport::new("failing");
        failing_fake.set_authentication_failure(true);
        let failing: Arc<dyn SyncTransport> = Arc::new(failing_fake);
        push_local_state(&database_b, &keys_b, std::slice::from_ref(&healthy)).await.unwrap();

        let keys_a = test_keys(&database_a);
        trust(&database_a, &keys_b);
        let outcome = pull_from_transports(&database_a, &keys_a, &[failing, healthy]).await.unwrap();
        assert_eq!((outcome.merged_states, outcome.failed_transports), (1, 1));
    }

    #[test]
    fn roster_drops_a_device_once_it_is_revoked() {
        let database = Database::open_memory();
        let keys = test_keys(&database);
        let peer = random_id();
        let peer_key = SigningKey::generate(&mut OsRng).verifying_key();
        database.trust_device_keys(&peer, &peer_key, &[9u8; 32]).unwrap();

        let roster = database.known_device_roster().unwrap();
        assert_eq!(roster.len(), 2);
        assert!(roster.iter().any(|(device_id, key)| *device_id.as_bytes() == peer && *key == peer_key));

        database.revoke_device(&peer).unwrap();
        let roster = database.known_device_roster().unwrap();
        assert_eq!(roster.len(), 1);
        assert!(roster[0].0 == keys.device_id);
    }

    #[test]
    fn shared_retry_at_is_in_the_future_and_grows_with_attempts() {
        let now = Utc::now().to_rfc3339();
        let first = retry_at(1);
        let later = retry_at(6);
        assert!(first.as_str() > now.as_str());
        assert!(later.as_str() > first.as_str());
    }
}

#[cfg(test)]
mod config_tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;

    struct TempFolder {
        path: std::path::PathBuf,
    }

    impl TempFolder {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("threestrands-replicated-sync-config-test-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempFolder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn add_folder_transport_persists_and_lists_it() {
        let database = Database::open_memory();
        let folder = TempFolder::new();
        database.add_folder_transport("folder-1", &folder.path).unwrap();

        let rows = database.configured_transports().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].instance_id, "folder-1");
        assert_eq!(rows[0].kind, "folder");
        assert!(rows[0].enabled);
        assert_eq!(
            rows[0].config(),
            Some(TransportConfig::Folder(FolderConfig { path: folder.path.to_string_lossy().into_owned(), label: None }))
        );
    }

    #[test]
    fn remove_transport_clears_config_and_deliveries() {
        let database = Database::open_memory();
        let folder = TempFolder::new();
        database.add_folder_transport("folder-1", &folder.path).unwrap();
        database
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO sync_deliveries(cid,transport_instance_id,state,attempts) VALUES ('c1','folder-1','pending',0)",
                [],
            )
            .unwrap();

        database.remove_transport("folder-1").unwrap();

        assert!(database.configured_transports().unwrap().is_empty());
        let remaining: i64 = database
            .connection()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM sync_deliveries WHERE transport_instance_id='folder-1'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(remaining, 0);
    }

    #[tokio::test]
    async fn build_configured_transports_skips_a_disabled_or_unopenable_row() {
        let database = Database::open_memory();
        let folder = TempFolder::new();
        database.add_folder_transport("folder-1", &folder.path).unwrap();
        database
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO sync_transports(instance_id,kind,config_json,required,enabled) VALUES ('folder-2','folder','{\"path\":\"/nonexistent/definitely-not-real\"}',1,1)",
                [],
            )
            .unwrap();
        database
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO sync_transports(instance_id,kind,config_json,required,enabled) VALUES ('folder-3','folder',?1,1,0)",
                params![serde_json::json!({"path": folder.path.to_string_lossy()}).to_string()],
            )
            .unwrap();

        let transports = build_configured_transports(&database).await;
        assert_eq!(transports.len(), 1);
        assert_eq!(transports[0].instance_id().0, "folder-1");
    }

    #[test]
    fn add_ipfs_rpc_transport_persists_only_non_secret_config() {
        let database = Database::open_memory();
        // Secrets go to an in-memory stand-in for the keychain in tests
        // (see `sync_connectors::secret_store`).
        let id = format!("ipfs-{}", Uuid::new_v4());
        database.add_ipfs_rpc_transport(&id, "https://rpc.filebase.io", Some("ipfs-token-value")).unwrap();

        let rows = database.configured_transports().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "ipfs_rpc");
        assert_eq!(rows[0].config().map(|config| config.location()).as_deref(), Some("https://rpc.filebase.io"));
        // No credential ever appears in the stored config.
        assert!(!rows[0].config_json.to_ascii_lowercase().contains("token"));
        assert!(!rows[0].config_json.contains("ipfs-token-value"));
        assert_eq!(
            TransportSecrets::load("ipfs_rpc", &id).unwrap(),
            Some(TransportSecrets::IpfsRpcToken("ipfs-token-value".to_string()))
        );

        // Re-adding without a token clears the stored one.
        database.add_ipfs_rpc_transport(&id, "https://rpc.filebase.io", None).unwrap();
        assert_eq!(TransportSecrets::load("ipfs_rpc", &id).unwrap(), None);
    }

    #[tokio::test]
    async fn build_configured_transports_includes_an_ipfs_rpc_row() {
        let database = Database::open_memory();
        database.add_ipfs_rpc_transport("ipfs-1", "https://rpc.filebase.io", None).unwrap();
        let transports = build_configured_transports(&database).await;
        assert_eq!(transports.len(), 1);
        assert_eq!(transports[0].instance_id().0, "ipfs-1");
    }

    #[test]
    fn removing_an_ipfs_rpc_transport_clears_its_config_row() {
        let database = Database::open_memory();
        database.add_ipfs_rpc_transport("ipfs-1", "https://rpc.filebase.io", None).unwrap();
        database.remove_transport("ipfs-1").unwrap();
        assert!(database.configured_transports().unwrap().is_empty());
    }

    fn s3_test_config(endpoint: &str) -> crate::s3_transport::S3Config {
        crate::s3_transport::S3Config {
            endpoint: endpoint.to_string(),
            region: "us-east-1".to_string(),
            bucket: "sync-bucket".to_string(),
            prefix: "team".to_string(),
            path_style: false,
            label: Some("Team bucket".to_string()),
        }
    }

    fn s3_test_credentials() -> crate::s3_transport::S3Credentials {
        crate::s3_transport::S3Credentials {
            access_key_id: "AKIAEXAMPLE".to_string(),
            secret_access_key: "s3-secret-value".to_string(),
            session_token: Some("s3-session-token".to_string()),
        }
    }

    #[test]
    fn s3_transport_config_persists_without_any_secret() {
        let database = Database::open_memory();
        let id = format!("s3-{}", Uuid::new_v4());
        database
            .add_s3_transport(&id, &s3_test_config("https://s3.us-east-1.amazonaws.com"), &s3_test_credentials())
            .unwrap();

        let rows = database.configured_transports().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "s3");
        assert_eq!(rows[0].config(), Some(TransportConfig::S3(s3_test_config("https://s3.us-east-1.amazonaws.com"))));
        let lowered = rows[0].config_json.to_ascii_lowercase();
        for secret_field in ["secret", "accesskey", "access_key", "token", "credential", "akiaexample"] {
            assert!(!lowered.contains(secret_field), "{secret_field} in {lowered}");
        }
        assert_eq!(
            TransportSecrets::load("s3", &id).unwrap(),
            Some(TransportSecrets::S3(s3_test_credentials()))
        );
    }

    #[test]
    fn add_s3_transport_rejects_an_invalid_config_before_writing_anything() {
        let database = Database::open_memory();
        let id = format!("s3-{}", Uuid::new_v4());
        // Plaintext HTTP to a remote host fails validation, which runs
        // before both the SQLite and the keychain writes.
        let error = database
            .add_s3_transport(&id, &s3_test_config("http://s3.example.com"), &s3_test_credentials())
            .unwrap_err();
        assert!(error.contains("HTTPS"), "{error}");
        assert!(database.configured_transports().unwrap().is_empty());
        assert_eq!(TransportSecrets::load("s3", &id).unwrap(), None);
    }

    #[tokio::test]
    async fn an_s3_row_without_stored_credentials_is_skipped_and_reported_unavailable() {
        let database = Database::open_memory();
        let config_json = TransportConfig::S3(s3_test_config("https://s3.us-east-1.amazonaws.com")).to_config_json().unwrap();
        database
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO sync_transports(instance_id,kind,config_json,required,enabled) VALUES (?1,'s3',?2,1,1)",
                params![format!("s3-{}", Uuid::new_v4()), config_json],
            )
            .unwrap();
        assert!(build_configured_transports(&database).await.is_empty());

        let engine = ReplicatedSync::new(Arc::new(database));
        let statuses = engine.status().await.unwrap();
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].kind, "s3");
        assert_eq!(statuses[0].label.as_deref(), Some("Team bucket"));
        assert_eq!(statuses[0].location, "https://s3.us-east-1.amazonaws.com · sync-bucket/team");
        assert!(statuses[0].supports_delete_data);
        assert_eq!(statuses[0].s3_config, Some(s3_test_config("https://s3.us-east-1.amazonaws.com")));
        let json = serde_json::to_string(&statuses[0]).unwrap();
        assert!(json.contains("\"s3Config\":{\"endpoint\""), "{json}");
        assert!(!json.to_ascii_lowercase().contains("secret"), "{json}");
        assert!(statuses[0].health.starts_with("unavailable"));
    }

    #[test]
    fn removing_a_connector_clears_its_row_and_its_secret() {
        let database = Database::open_memory();
        let s3_id = format!("s3-{}", Uuid::new_v4());
        database
            .add_s3_transport(&s3_id, &s3_test_config("https://s3.us-east-1.amazonaws.com"), &s3_test_credentials())
            .unwrap();
        let ipfs_id = format!("ipfs-{}", Uuid::new_v4());
        database.add_ipfs_rpc_transport(&ipfs_id, "https://rpc.filebase.io", Some("token")).unwrap();

        database.remove_transport(&s3_id).unwrap();
        assert_eq!(TransportSecrets::load("s3", &s3_id).unwrap(), None);
        assert!(crate::sync_connectors::secret_store::values_for(&s3_id).is_empty());
        // The other connector and its secret are untouched.
        assert!(TransportSecrets::load("ipfs_rpc", &ipfs_id).unwrap().is_some());

        database.remove_transport(&ipfs_id).unwrap();
        assert!(database.configured_transports().unwrap().is_empty());
        assert!(crate::sync_connectors::secret_store::values_for(&ipfs_id).is_empty());
    }

    #[test]
    fn removing_a_row_of_an_unknown_kind_clears_every_possible_secret() {
        let database = Database::open_memory();
        let id = format!("future-{}", Uuid::new_v4());
        database
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO sync_transports(instance_id,kind,config_json,required,enabled) VALUES (?1,'carrier-pigeon','{}',1,1)",
                params![id],
            )
            .unwrap();
        TransportSecrets::IpfsRpcToken("stale".to_string()).store(&id).unwrap();
        database.remove_transport(&id).unwrap();
        assert!(crate::sync_connectors::secret_store::values_for(&id).is_empty());
    }

    #[tokio::test]
    async fn an_unknown_kind_is_skipped_and_reported_without_a_location() {
        let database = Database::open_memory();
        database
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO sync_transports(instance_id,kind,config_json,required,enabled) VALUES ('future-1','carrier-pigeon','{}',1,1)",
                [],
            )
            .unwrap();
        assert!(build_configured_transports(&database).await.is_empty());
        let statuses = ReplicatedSync::new(Arc::new(database)).status().await.unwrap();
        assert_eq!(statuses[0].location, "");
        assert_eq!(statuses[0].label, None);
        assert!(!statuses[0].supports_delete_data);
        assert_eq!(statuses[0].health, "unavailable: not configured");
    }

    #[test]
    fn update_renames_a_connector_without_losing_its_ledger_or_secret() {
        let database = Database::open_memory();
        let id = format!("s3-{}", Uuid::new_v4());
        database
            .add_s3_transport(&id, &s3_test_config("https://s3.us-east-1.amazonaws.com"), &s3_test_credentials())
            .unwrap();
        database
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO sync_deliveries(cid,transport_instance_id,state,attempts) VALUES ('c1',?1,'delivered',1)",
                params![id],
            )
            .unwrap();
        database.set_transport_success(&id).unwrap();

        let mut renamed = database.configured_transports().unwrap()[0].config().unwrap();
        renamed.set_label(Some("Renamed"));
        database.update_transport_config(&id, &renamed, None).unwrap();

        let row = &database.configured_transports().unwrap()[0];
        assert_eq!(row.config().unwrap().label(), Some("Renamed"));
        assert!(row.last_success_at.is_some());
        assert_eq!(database.delivery_counts(&id).unwrap(), (0, 1, 0));
        assert_eq!(TransportSecrets::load("s3", &id).unwrap(), Some(TransportSecrets::S3(s3_test_credentials())));
    }

    #[test]
    fn update_can_rotate_credentials_and_refuses_a_kind_change_or_invalid_config() {
        let database = Database::open_memory();
        let id = format!("s3-{}", Uuid::new_v4());
        let config = TransportConfig::S3(s3_test_config("https://s3.us-east-1.amazonaws.com"));
        database
            .add_transport(&id, &config, Some(&TransportSecrets::S3(s3_test_credentials())))
            .unwrap();

        let mut rotated = s3_test_credentials();
        rotated.secret_access_key = "rotated-secret".to_string();
        database.update_transport_config(&id, &config, Some(&TransportSecrets::S3(rotated.clone()))).unwrap();
        assert_eq!(TransportSecrets::load("s3", &id).unwrap(), Some(TransportSecrets::S3(rotated.clone())));

        let folder = TransportConfig::Folder(FolderConfig { path: "/tmp".to_string(), label: None });
        assert!(database.update_transport_config(&id, &folder, None).is_err());

        let TransportConfig::S3(mut insecure) = config.clone() else { unreachable!() };
        insecure.endpoint = "http://s3.example.com".to_string();
        assert!(database.update_transport_config(&id, &TransportConfig::S3(insecure), None).is_err());
        // Nothing changed after the refusals.
        assert_eq!(database.configured_transports().unwrap()[0].config(), Some(config));
        assert_eq!(TransportSecrets::load("s3", &id).unwrap(), Some(TransportSecrets::S3(rotated)));

        assert!(database.update_transport_config("missing", &folder, None).is_err());
    }

    #[tokio::test]
    async fn probe_s3_checks_permissions_and_an_empty_bucket_without_persisting_anything() {
        use crate::s3_transport::fake_server::FakeS3Server;
        let server = FakeS3Server::spawn().await;
        let database = Arc::new(Database::open_memory());
        let engine = ReplicatedSync::new(database.clone());

        let test = engine.probe_s3(&server.config("group"), &FakeS3Server::credentials()).await.unwrap();
        assert!(test.checks.can_list && test.checks.can_write && test.checks.can_read && test.checks.can_delete);
        assert_eq!(test.space_presence, Some(crate::enrollment::SyncSpacePresence::None));
        assert!(database.configured_transports().unwrap().is_empty());
        assert!(server.state().objects.is_empty());
    }

    #[tokio::test]
    async fn probe_s3_skips_the_group_check_when_the_key_cannot_list() {
        use crate::s3_transport::fake_server::FakeS3Server;
        let server = FakeS3Server::spawn().await;
        let mut credentials = FakeS3Server::credentials();
        credentials.secret_access_key = "wrong".to_string();
        let test = ReplicatedSync::new(Arc::new(Database::open_memory()))
            .probe_s3(&server.config(""), &credentials)
            .await
            .unwrap();
        assert!(test.checks.reachable && !test.checks.can_list);
        assert_eq!(test.space_presence, None);
    }

    #[tokio::test]
    async fn probe_s3_rejects_an_invalid_config_before_any_request() {
        use crate::s3_transport::fake_server::FakeS3Server;
        let server = FakeS3Server::spawn().await;
        let mut config = server.config("");
        config.path_style = false; // an IP-address endpoint needs path-style
        let error = ReplicatedSync::new(Arc::new(Database::open_memory()))
            .probe_s3(&config, &FakeS3Server::credentials())
            .await
            .unwrap_err();
        assert!(error.contains("path-style"), "{error}");
        assert_eq!(server.state().faults.requests, 0);
    }

    #[test]
    fn a_connection_test_serializes_flat_in_camel_case() {
        let test = S3ConnectionTest {
            checks: S3ProbeReport { reachable: true, can_list: true, versioning_enabled: Some(true), ..Default::default() },
            space_presence: Some(crate::enrollment::SyncSpacePresence::Existing),
        };
        assert_eq!(
            serde_json::to_value(&test).unwrap(),
            json!({
                "reachable": true, "canList": true, "canWrite": false, "canRead": false, "canDelete": false,
                "versioningEnabled": true, "error": null, "spacePresence": "existing"
            })
        );
    }

    #[tokio::test]
    async fn an_added_s3_connector_reports_healthy_with_a_storage_estimate() {
        use crate::s3_transport::fake_server::FakeS3Server;
        let server = FakeS3Server::spawn().await;
        let database = Arc::new(Database::open_memory());
        let id = format!("s3-{}", Uuid::new_v4());
        database.add_s3_transport(&id, &server.config("group"), &FakeS3Server::credentials()).unwrap();
        server.state().objects.insert("group/threestrands-sync/objects/ab/x.block".to_string(), vec![0; 10]);

        let statuses = ReplicatedSync::new(database).status().await.unwrap();
        assert_eq!(statuses[0].health, "healthy");
        assert_eq!(statuses[0].storage_bytes, Some(10));
        assert!(statuses[0].head_discovery);
    }

    #[test]
    fn update_connector_renames_clears_and_rotates_credentials() {
        let database = Arc::new(Database::open_memory());
        let engine = ReplicatedSync::new(database.clone());
        let id = format!("s3-{}", Uuid::new_v4());
        database
            .add_s3_transport(&id, &s3_test_config("https://s3.us-east-1.amazonaws.com"), &s3_test_credentials())
            .unwrap();
        let label = || database.configured_transports().unwrap()[0].config().unwrap().label().map(str::to_string);

        engine.update_connector(&id, Some("  Personal R2 "), None).unwrap();
        assert_eq!(label().as_deref(), Some("Personal R2"));

        // No label argument keeps the name.
        let rotated: ConnectorCredentials = serde_json::from_value(json!({
            "kind": "s3", "accessKeyId": "AKIAROTATED", "secretAccessKey": "rotated-secret"
        }))
        .unwrap();
        engine.update_connector(&id, None, Some(rotated)).unwrap();
        assert_eq!(label().as_deref(), Some("Personal R2"));
        match TransportSecrets::load("s3", &id).unwrap() {
            Some(TransportSecrets::S3(credentials)) => assert_eq!(credentials.access_key_id, "AKIAROTATED"),
            other => panic!("unexpected {other:?}"),
        }

        engine.update_connector(&id, Some(""), None).unwrap();
        assert_eq!(label(), None);
    }

    #[test]
    fn update_connector_refuses_mismatched_credentials_long_names_and_unknown_ids() {
        let database = Arc::new(Database::open_memory());
        let engine = ReplicatedSync::new(database.clone());
        let folder = TempFolder::new();
        database.add_folder_transport("folder-1", &folder.path).unwrap();

        let token: ConnectorCredentials = serde_json::from_value(json!({ "kind": "ipfs_rpc", "token": "t" })).unwrap();
        assert!(engine.update_connector("folder-1", None, Some(token)).is_err());
        assert!(engine.update_connector("folder-1", Some(&"x".repeat(61)), None).is_err());
        assert_eq!(database.configured_transports().unwrap()[0].config().unwrap().label(), None);
        assert!(engine.update_connector("missing", Some("name"), None).is_err());

        let ipfs_id = format!("ipfs-{}", Uuid::new_v4());
        database.add_ipfs_rpc_transport(&ipfs_id, "https://rpc.filebase.io", None).unwrap();
        let s3: ConnectorCredentials =
            serde_json::from_value(json!({ "kind": "s3", "accessKeyId": "a", "secretAccessKey": "b" })).unwrap();
        assert!(engine.update_connector(&ipfs_id, None, Some(s3)).is_err());
        let token: ConnectorCredentials = serde_json::from_value(json!({ "kind": "ipfs_rpc", "token": "new" })).unwrap();
        engine.update_connector(&ipfs_id, None, Some(token)).unwrap();
        assert_eq!(
            TransportSecrets::load("ipfs_rpc", &ipfs_id).unwrap(),
            Some(TransportSecrets::IpfsRpcToken("new".to_string()))
        );
    }

    #[tokio::test]
    async fn status_offers_data_deletion_for_folders_but_not_ipfs() {
        let database = Database::open_memory();
        let folder = TempFolder::new();
        database.add_folder_transport("folder-1", &folder.path).unwrap();
        database.add_ipfs_rpc_transport(&format!("ipfs-{}", Uuid::new_v4()), "https://rpc.filebase.io", None).unwrap();
        let statuses = ReplicatedSync::new(Arc::new(database)).status().await.unwrap();
        let by_kind = |kind: &str| statuses.iter().find(|status| status.kind == kind).unwrap();
        assert!(by_kind("folder").supports_delete_data);
        assert!(!by_kind("ipfs_rpc").supports_delete_data);
        assert_eq!(by_kind("folder").s3_config, None);
        assert_eq!(by_kind("folder").label, None);
    }

    #[tokio::test]
    async fn status_reports_health_and_a_storage_estimate_for_a_real_folder() {
        let database = Database::open_memory();
        let folder = TempFolder::new();
        database.add_folder_transport("folder-1", &folder.path).unwrap();

        let engine = ReplicatedSync::new(Arc::new(database));
        let statuses = engine.status().await.unwrap();
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].instance_id, "folder-1");
        assert_eq!(statuses[0].health, "healthy");
        assert_eq!(statuses[0].pending, 0);
        assert!(statuses[0].storage_bytes.is_some());
    }

    #[tokio::test]
    async fn sync_once_is_a_no_op_when_the_feature_is_disabled() {
        // `enabled()` reads THREESTRANDS_REPLICATED_SYNC, which is unset in
        // the test environment, so this never touches the OS keychain
        // (`local_replicated_keys` is only reached past that gate).
        assert!(!enabled());
        let database = Database::open_memory();
        let engine = ReplicatedSync::new(Arc::new(database));
        engine.sync_once().await.unwrap();
    }
}

#[cfg(test)]
mod reconciliation_tests {
    use super::*;

    fn count(database: &Database, sql: &str) -> i64 {
        database.connection().unwrap().query_row(sql, [], |row| row.get(0)).unwrap()
    }

    fn table_exists(database: &Database, table: &str) -> bool {
        database
            .connection()
            .unwrap()
            .query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)", [table], |row| row.get(0))
            .unwrap()
    }

    /// Puts back the tables a protocol-2 build kept (in the columns the
    /// upgrade reads), fills them with a group, marks the schema as the
    /// version before the upgrade, and runs the migration again.
    fn upgrade_from_a_protocol_2_group(database: &Database, enrolled: bool) {
        database
            .connection()
            .unwrap()
            .execute_batch(
                "CREATE TABLE sync_events (event_id TEXT PRIMARY KEY, device_id TEXT, device_sequence INTEGER, state TEXT);
                 CREATE TABLE sync_operations (operation_id TEXT PRIMARY KEY, event_id TEXT, entity_id TEXT, field TEXT, value TEXT);
                 CREATE TABLE sync_field_frontier (entity_id TEXT, field TEXT, operation_id TEXT);
                 CREATE TABLE sync_device_progress (device_id TEXT PRIMARY KEY);
                 INSERT INTO sync_events VALUES ('e1','d1',1,'sealed');
                 INSERT INTO sync_operations VALUES ('o1','e1','s1','name','\"n\"');
                 INSERT INTO sync_devices(device_id,status,is_self) VALUES ('d1','active',1);
                 INSERT INTO sync_device_labels VALUES ('d1','Laptop');",
            )
            .unwrap();
        if enrolled {
            database
                .connection()
                .unwrap()
                .execute_batch(
                    "INSERT INTO sync_epoch_history VALUES (3,'2026-09-01T00:00:00Z','cid');
                     UPDATE sync_spaces SET active_epoch=3;",
                )
                .unwrap();
        }
        let mut connection = database.connection().unwrap();
        connection.pragma_update(None, "user_version", 35).unwrap();
        crate::schema::migrate(&mut connection).unwrap();
    }

    #[test]
    fn upgrading_leaves_a_protocol_2_group_but_keeps_local_data_connectors_and_the_beta() {
        let database = Database::open_memory();
        database.set_beta_features_enabled(true).unwrap();
        let snippet = database.create_snippet("Signature", "Best, Alex").unwrap();
        database.add_folder_transport("folder-1", &std::env::temp_dir()).unwrap();
        upgrade_from_a_protocol_2_group(&database, true);

        for table in ["sync_events", "sync_operations", "sync_field_frontier", "sync_device_progress"] {
            assert!(!table_exists(&database, table), "{table} should be gone");
        }
        for table in ["sync_values", "sync_context", "sync_devices", "sync_device_labels", "sync_epoch_history"] {
            assert_eq!(count(&database, &format!("SELECT COUNT(*) FROM {table}")), 0, "{table} should be empty");
        }
        assert_eq!(count(&database, "SELECT active_epoch FROM sync_spaces"), 0);
        assert!(database.beta_features_enabled().unwrap());
        assert_eq!(database.configured_transports().unwrap().len(), 1);
        assert_eq!(count(&database, "SELECT COUNT(*) FROM snippets"), 1);
        assert_eq!(database.pending_keychain_cleanup().unwrap(), Some(3));

        // The user is told once, and can dismiss it.
        assert!(database.protocol_reset_notice().unwrap());
        database.dismiss_protocol_reset_notice().unwrap();
        assert!(!database.protocol_reset_notice().unwrap());

        // The next sync re-records local data into the replica.
        assert!(database.reconcile_replicated_sync_backlog().unwrap() >= 1);
        let recorded: i64 = database
            .connection()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM sync_values WHERE entity_id=?1 AND field='_entity'", [&snippet.id], |row| row.get(0))
            .unwrap();
        assert_eq!(recorded, 1);
    }

    #[test]
    fn upgrading_a_device_that_never_joined_a_group_says_nothing() {
        let database = Database::open_memory();
        upgrade_from_a_protocol_2_group(&database, false);
        assert!(!database.protocol_reset_notice().unwrap());
        assert_eq!(database.pending_keychain_cleanup().unwrap(), None);
        assert!(!table_exists(&database, "sync_events"));
    }

    #[test]
    fn a_fresh_database_has_the_replica_tables_and_no_event_log() {
        let database = Database::open_memory();
        for table in ["sync_values", "sync_context", "sync_local_state", "sync_remote_states", "sync_objects", "sync_retired_objects"] {
            assert!(table_exists(&database, table), "{table} should exist");
        }
        for table in ["sync_events", "sync_operations", "sync_operation_parents", "sync_field_frontier", "sync_device_progress"] {
            assert!(!table_exists(&database, table), "{table} should not exist");
        }
    }

    #[test]
    fn catches_up_an_entity_created_without_being_enqueued() {
        let database = Database::open_memory();
        // Simulates the crash window: the app-table write happened, but the
        // enqueue call that should follow it never ran.
        let snippet = database.create_snippet("Signature", "Best, Alex").unwrap();
        let connection = database.connection().unwrap();
        let before: i64 = connection
            .query_row("SELECT COUNT(*) FROM sync_values WHERE entity_id=?1", [&snippet.id], |row| row.get(0))
            .unwrap();
        assert_eq!(before, 0);
        drop(connection);

        let repaired = database.reconcile_replicated_sync_backlog().unwrap();
        assert!(repaired >= 1);

        let connection = database.connection().unwrap();
        let existence: String = connection
            .query_row(
                "SELECT value FROM sync_values WHERE entity_id=?1 AND field='_entity'",
                [&snippet.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(existence, "true");
        let name: String = connection
            .query_row(
                "SELECT value FROM sync_values WHERE entity_id=?1 AND field='name'",
                [&snippet.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(name, "\"Signature\"");
    }

    #[test]
    fn is_a_no_op_for_an_entity_already_recorded() {
        let database = Database::open_memory();
        let snippet = database.create_snippet("Signature", "Best, Alex").unwrap();
        let payload = serde_json::to_value(&snippet).unwrap();
        let fields: std::collections::BTreeSet<String> =
            payload.as_object().unwrap().keys().cloned().collect();
        database
            .record_replicated_write(EntityType::Snippet, &snippet.id, &fields, &payload)
            .unwrap();

        let connection = database.connection().unwrap();
        let before: i64 = connection
            .query_row("SELECT COUNT(*) FROM sync_values WHERE entity_id=?1", [&snippet.id], |row| row.get(0))
            .unwrap();
        drop(connection);

        // Only assert the *snippet* is untouched — not that the whole sweep
        // found nothing to do.
        database.reconcile_replicated_sync_backlog().unwrap();

        let connection = database.connection().unwrap();
        let after: i64 = connection
            .query_row("SELECT COUNT(*) FROM sync_values WHERE entity_id=?1", [&snippet.id], |row| row.get(0))
            .unwrap();
        assert_eq!(before, after, "reconciling an already-recorded entity must not create duplicate operations");
    }

    #[test]
    fn covers_every_locally_enumerable_entity_type_in_one_sweep() {
        let database = Database::open_memory();
        database.create_snippet("Signature", "Best, Alex").unwrap();
        database
            .create_split_inbox("Newsletters", "domain", "news.example.com", "you@example.com")
            .unwrap();
        database.set_retention_days(Some(90)).unwrap();
        database.adopt_account("you@example.com").unwrap();
        database
            .create_task(&crate::models::CreateTaskRequest {
                account_id: "you@example.com".into(),
                thread_id: None,
                source_message_id: None,
                subject_snapshot: None,
                title: "Renew passport".into(),
                notes: None,
                kind: "action".into(),
                due_kind: "none".into(),
                due_value: None,
                time_zone: None,
                repeat_interval_days: None,
                evidence_text: None,
                goal_id: None,
            })
            .unwrap();
        database.save_contact_profile(&crate::models::SaveContactRequest{birthday:None,keep_in_touch:None,
            id:None,display_name:Some("Sweep person".into()),role:None,company:None,
            location:None,bio:None,notes:None,links:vec![],photo_data:None,
            favorite:false,addresses:vec!["sweep@example.com".into()],
        }).unwrap();

        database
            .create_goal(&crate::models::CreateGoalRequest {
                account_id: "you@example.com".into(),
                title: "Travel lighter".into(),
                notes: None,
                horizon: "year".into(),
                period: "2026".into(),
                parent_goal_id: None,
            })
            .unwrap();

        let repaired = database.reconcile_replicated_sync_backlog().unwrap();
        // Task, goal, snippet, contact, split inbox, mail account, and the
        // explicitly chosen retention setting.
        assert_eq!(repaired, 7);
        // A contact in the sweep must not break the snapshot that follows.
        database.load_replica_state().unwrap();

        let second_pass = database.reconcile_replicated_sync_backlog().unwrap();
        assert_eq!(second_pass, 0, "a second sweep over the same state must repair nothing");
    }

    fn retention_operation_count(database: &Database) -> i64 {
        database
            .connection()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM sync_values WHERE entity_type=?1 AND entity_id='mail'",
                [EntityType::Retention.as_str()],
                |row| row.get(0),
            )
            .unwrap()
    }

    #[test]
    fn does_not_seed_the_unlimited_retention_default() {
        // A freshly joining device has never chosen a retention period.
        // Seeding its unset default would enter the graph as a concurrent
        // "forever" write and conflict with the space's agreed value.
        let database = Database::open_memory();
        assert_eq!(database.retention_days().unwrap(), None);
        database.reconcile_replicated_sync_backlog().unwrap();
        assert_eq!(retention_operation_count(&database), 0);
    }

    #[test]
    fn seeds_an_explicit_retention_choice() {
        let database = Database::open_memory();
        database.set_retention_days(Some(365)).unwrap();
        database.reconcile_replicated_sync_backlog().unwrap();
        assert!(retention_operation_count(&database) > 0);
        let days: String = database
            .connection()
            .unwrap()
            .query_row(
                "SELECT value FROM sync_values WHERE entity_type=?1 AND entity_id='mail' AND field='days'",
                [EntityType::Retention.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(days, "365");
    }
}

#[cfg(test)]
mod frontier_conflict_tests {
    use super::*;
    use serde_json::json;

    fn fields(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    /// Forces a genuine two-way conflict on `field`: two other devices each
    /// start from this replica, rewrite the field before hearing from the
    /// other, and this replica merges both.
    fn force_conflict(database: &Database, entity_id: &str, field: &str, value_a: Value, value_b: Value) {
        let base = database.load_replica_state().unwrap();
        for value in [value_a, value_b] {
            let mut remote = base.clone();
            remote.write(random_id(), 10, EntityType::Snippet, entity_id, [(field.to_string(), Some(value))]);
            database.merge_replica_state(&remote).unwrap();
        }
    }

    #[test]
    fn lists_a_genuine_concurrent_write_as_a_conflict() {
        let database = Database::open_memory();
        database
            .record_replicated_write(
                EntityType::Snippet,
                "s1",
                &fields(&["id", "name", "body", "createdAt"]),
                &json!({"id": "s1", "name": "original", "body": "b", "createdAt": "2026-01-01T00:00:00Z"}),
            )
            .unwrap();

        force_conflict(&database, "s1", "name", json!("From A"), json!("From B"));

        let conflicts = database.list_frontier_conflicts().unwrap();
        let conflict = conflicts
            .iter()
            .find(|conflict| conflict.entity_id == "s1" && conflict.field == "name")
            .expect("the forced conflict should be listed");
        assert_eq!(conflict.entity_type, "snippet");
        assert_eq!(conflict.candidates.len(), 2);
        let values: std::collections::HashSet<_> = conflict.candidates.iter().map(|c| c.value.clone()).collect();
        assert!(values.contains(&Some(json!("From A"))));
        assert!(values.contains(&Some(json!("From B"))));

        // Every other field (untouched by the forced conflict) must not be
        // reported.
        assert!(!conflicts.iter().any(|c| c.field == "body"));
    }

    #[test]
    fn does_not_list_concurrent_writes_that_agree_on_the_value() {
        let database = Database::open_memory();
        database
            .record_replicated_write(
                EntityType::Snippet,
                "s1",
                &fields(&["id", "name", "body", "createdAt"]),
                &json!({"id": "s1", "name": "original", "body": "b", "createdAt": "2026-01-01T00:00:00Z"}),
            )
            .unwrap();

        force_conflict(&database, "s1", "name", json!("Same"), json!("Same"));

        let values: i64 = database
            .connection()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM sync_values WHERE entity_type='snippet' AND entity_id='s1' AND field='name'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(values, 2, "both concurrent writes stay in the field");
        assert!(!database
            .list_frontier_conflicts()
            .unwrap()
            .iter()
            .any(|conflict| conflict.entity_id == "s1" && conflict.field == "name"));
    }

    #[test]
    fn task_null_and_absent_optional_values_are_not_reported_as_conflicts() {
        let database = Database::open_memory();
        let payload = json!({
            "id": "t1", "title": "Follow up", "notes": null, "kind": "follow_up",
            "dueKind": "none", "dueValue": null, "timeZone": null,
            "repeatIntervalDays": null, "status": "open", "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-01T00:00:00Z"
        });
        let task_fields = payload.as_object().unwrap().keys().cloned().collect();
        database
            .record_replicated_write(EntityType::Task, "t1", &task_fields, &payload)
            .unwrap();

        for field in ["dueValue", "timeZone", "repeatIntervalDays", "notes"] {
            let base = database.load_replica_state().unwrap();
            for value in [None, Some(Value::Null)] {
                let mut remote = base.clone();
                remote.write(
                    random_id(),
                    10,
                    EntityType::Task,
                    "t1",
                    [(field.to_string(), value)],
                );
                database.merge_replica_state(&remote).unwrap();
            }
        }

        assert!(database.list_frontier_conflicts().unwrap().is_empty());
    }

    #[test]
    fn absent_required_values_remain_conflicts() {
        let database = Database::open_memory();
        database
            .record_replicated_write(
                EntityType::Snippet,
                "s1",
                &fields(&["id", "name", "body", "createdAt"]),
                &json!({
                    "id": "s1",
                    "name": "original",
                    "body": "b",
                    "createdAt": "2026-01-01T00:00:00Z"
                }),
            )
            .unwrap();

        let base = database.load_replica_state().unwrap();
        for value in [None, Some(json!(null))] {
            let mut remote = base.clone();
            remote.write(
                random_id(),
                10,
                EntityType::Snippet,
                "s1",
                [("name".to_string(), value)],
            );
            database.merge_replica_state(&remote).unwrap();
        }
        assert!(database
            .list_frontier_conflicts()
            .unwrap()
            .iter()
            .any(|conflict| { conflict.entity_id == "s1" && conflict.field == "name" }));
    }

    #[test]
    fn resolving_replaces_every_value_in_the_field_with_the_chosen_one() {
        let database = Database::open_memory();
        database
            .record_replicated_write(
                EntityType::Snippet,
                "s1",
                &fields(&["id", "name", "body", "createdAt"]),
                &json!({"id": "s1", "name": "original", "body": "b", "createdAt": "2026-01-01T00:00:00Z"}),
            )
            .unwrap();
        force_conflict(&database, "s1", "name", json!("From A"), json!("From B"));

        let conflict = database
            .list_frontier_conflicts()
            .unwrap()
            .into_iter()
            .find(|conflict| conflict.entity_id == "s1" && conflict.field == "name")
            .unwrap();
        let chosen = conflict.candidates.iter().find(|candidate| candidate.value == Some(json!("From B"))).unwrap();
        database.resolve_frontier_conflict(EntityType::Snippet, "s1", "name", &chosen.operation_id).unwrap();

        // One value is left, the chosen one, written by this device (so it
        // supersedes both candidates on every replica that merges it).
        assert!(database.list_frontier_conflicts().unwrap().is_empty());
        let rows: Vec<(String, Option<String>)> = {
            let connection = database.connection().unwrap();
            let mut statement = connection
                .prepare("SELECT device_id, value FROM sync_values WHERE entity_type='snippet' AND entity_id='s1' AND field='name'")
                .unwrap();
            let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap();
            rows
        };
        let self_device: String = database
            .connection()
            .unwrap()
            .query_row("SELECT device_id FROM sync_devices WHERE is_self=1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, vec![(self_device, Some("\"From B\"".to_string()))]);
        assert!(database.resolve_frontier_conflict(EntityType::Snippet, "s1", "name", &chosen.operation_id).is_err(), "a stale choice is refused");
    }
}
