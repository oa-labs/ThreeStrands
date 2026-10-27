//! The enrollment protocol over the transports: inspecting a connector,
//! genesis, requests and grants, the sweep that applies control objects,
//! rotation, key catch-up, and recovery-phrase joining.

use std::sync::Arc;

use chrono::Utc;
use serde::Serialize;
use serde_bytes::ByteBuf;
use threestrands_sync_envelope::{
    compute_cid, decode_signed_enrollment_grant, decode_signed_enrollment_rejection, decode_signed_enrollment_request,
    decode_signed_key_rotation, encode_signed_enrollment_grant, encode_signed_enrollment_rejection, encode_signed_enrollment_request,
    encode_signed_key_rotation, enrollment_fingerprint, generate_recovery_seed,
    recovery_ed25519_signing_key, recovery_phrase_from_seed, recovery_seed_from_phrase,
    recovery_x25519_secret, seal_to_x25519, sign_enrollment_grant, sign_enrollment_rejection, sign_enrollment_request,
    sign_key_rotation, try_open_sealed_box, verify_enrollment_grant, verify_enrollment_rejection, verify_enrollment_request,
    verify_key_rotation, EnrollmentGrant, EnrollmentRequest,
    EnrollmentRejection, KeyRotation, RequestId, RosterEntry, SignedEnrollmentGrant, SignedEnrollmentRequest,
    SignedKeyRotation, VerifyingKey, X25519PublicKey,
};
use threestrands_sync_envelope::{protocol_marker_cid, PROTOCOL_MARKER};
use threestrands_sync_transport::{Cid as TransportCid, SyncTransport, TransportError};

use crate::db::{Database, DatabaseError, DbResult};
use crate::replicated_sync::{decode_id, encode_id, random_id, x25519_public_bytes, DeviceIdentity, LocalKeys};

use super::crypto::{
    adopt_epoch_key, is_self_consistent_rotation, open_earlier_epoch_keys, roster_entry_verifying_key, roster_entry_x25519,
    seal_earlier_epoch_keys, store_earlier_epoch_keys, EpochKeyStore,
};
use super::join_codes;
use super::{EnrollmentError, EnrollmentResult, EnrollmentStatus};

pub(super) fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

impl Database {
    /// Whether this device should rotate to resolve a collision at
    /// `key_epoch` with the rotation `incoming_cid`: it initiated the
    /// rotation it holds for that epoch, that rotation's content address is
    /// the greater of the two, and nothing has rotated past the epoch yet.
    pub(super) fn must_rotate_after_collision(&self, identity: &DeviceIdentity, key_epoch: u32, incoming_cid: &str) -> DbResult<bool> {
        if self.active_epoch()? != key_epoch {
            return Ok(false);
        }
        let own_rotation = self.epoch_source_control_object(key_epoch)?;
        let Some((own_cid, bytes)) = own_rotation else { return Ok(false) };
        let initiated_here = decode_signed_key_rotation(&bytes)
            .is_ok_and(|signed| signed.rotation.initiator_device_id == identity.device_id && signed.rotation.key_epoch == key_epoch);
        Ok(initiated_here && own_cid.as_str() > incoming_cid)
    }

    pub(super) fn adopt_roster(&self, roster: &[RosterEntry]) -> EnrollmentResult<()> {
        for entry in roster {
            let verifying_key = roster_entry_verifying_key(entry)?;
            let x25519_public = roster_entry_x25519(entry)?;
            let device_id = *entry.device_id.as_bytes();
            self.trust_roster_device_keys(&device_id, &verifying_key, &x25519_public)?;
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
async fn publish_control_object(database: &Database, transports: &[Arc<dyn SyncTransport>], bytes: &[u8]) -> EnrollmentResult<String> {
    let cid = compute_cid(bytes);
    database.store_control_object(&cid, bytes)?;
    publish_to_all(transports, bytes).await;
    Ok(cid)
}

/// Puts `bytes` on every transport, best effort, without recording it for
/// repair. Tests use it to plant objects no device would publish.
pub(super) async fn publish_to_all(transports: &[Arc<dyn SyncTransport>], bytes: &[u8]) -> String {
    let cid = compute_cid(bytes);
    let locator = TransportCid(cid.clone());
    for transport in transports {
        let _ = transport.put_object(&locator, bytes).await;
    }
    cid
}


// ============================ Genesis and joining ============================

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
async fn refuse_legacy_space(transports: &[Arc<dyn SyncTransport>]) -> EnrollmentResult<()> {
    if inspect_sync_space(transports).await == SyncSpacePresence::Legacy {
        return Err(EnrollmentError::LegacySpace);
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
pub(crate) async fn publish_protocol_marker(database: &Database, transports: &[Arc<dyn SyncTransport>]) -> EnrollmentResult<()> {
    database.ensure_protocol_marker_object()?;
    let cid = TransportCid(protocol_marker_cid());
    for transport in transports {
        if let Err(error) = transport.put_object(&cid, PROTOCOL_MARKER).await {
            log::debug!(target: "replicated_sync", "storing the protocol marker on {} failed: {error}", transport.instance_id().0);
        }
    }
    Ok(())
}

/// Starts a brand-new sync space on this device: generates its own device
/// keys, a fresh epoch-0 key, and a recovery seed, publishes a genesis
/// [`KeyRotation`] (epoch 0, sealed to itself and to the recovery X25519
/// key) so a later recovery-phrase import has something to find, and
/// returns the recovery phrase. **The phrase is shown to the caller exactly
/// once and is never persisted anywhere** — only its two derived public
/// keys are kept, in `sync_spaces`.
///
/// Refuses, before any side effect, when a space already exists on a
/// configured transport unless `allow_existing_space` records the user's
/// explicit choice to start a separate one anyway.
pub async fn begin_genesis(
    database: &Database,
    identity: &DeviceIdentity,
    epoch_keys: &dyn EpochKeyStore,
    transports: &[Arc<dyn SyncTransport>],
    allow_existing_space: bool,
) -> EnrollmentResult<String> {
    require_not_started(database)?;
    match inspect_sync_space(transports).await {
        SyncSpacePresence::Legacy => return Err(EnrollmentError::LegacySpace),
        SyncSpacePresence::Existing if !allow_existing_space => return Err(EnrollmentError::ExistingSpace),
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
    rand::Rng::fill_bytes(&mut threestrands_sync_envelope::os_rng(), &mut k_epoch);

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
    let signed = sign_key_rotation(&identity.signing_key, rotation)?;
    let bytes = encode_signed_key_rotation(&signed)?;
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
pub async fn publish_enrollment_request(database: &Database, identity: &DeviceIdentity, transports: &[Arc<dyn SyncTransport>]) -> EnrollmentResult<String> {
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
    let signed = sign_enrollment_request(&identity.signing_key, request)?;
    let bytes = encode_signed_enrollment_request(&signed)?;
    let cid = publish_control_object(database, transports, &bytes).await?;
    database.mark_control_object_seen(&cid, "enrollment_request")?;

    database.record_enrollment_request(
        "outgoing",
        &encode_id(request_id.as_bytes()),
        &encode_id(identity.device_id.as_bytes()),
        &identity.verifying_key.to_bytes(),
        &x25519_public,
        &fingerprint,
    )?;
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
///
/// Returns whether this device must rotate to resolve an epoch collision
/// (see [`RotationOutcome::Collision`]); the caller does so once it has
/// loaded its keys.
pub async fn run_enrollment_sweep(database: &Database, identity: &DeviceIdentity, epoch_keys: &dyn EpochKeyStore, transports: &[Arc<dyn SyncTransport>]) -> EnrollmentResult<bool> {
    // Objects arrive in content-address order, so a join-code redemption
    // can come before the invitation it names. Those are retried once the
    // whole sweep has run, rather than waiting for the next cycle.
    let mut deferred: Vec<(String, Vec<u8>)> = Vec::new();
    let mut must_rotate = false;
    for transport in transports {
        let mut cursor: Option<String> = None;
        loop {
            let Some(page) = transport.scan(cursor.as_deref()).await? else { break };
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
                    // Left unseen until this device has rotated past the
                    // colliding epoch, so a crash before the rotation is
                    // published can't lose it.
                    ControlObject::RotateToResolve => must_rotate = true,
                }
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
    }
    for (cid, bytes) in deferred {
        match try_apply_control_object(database, identity, epoch_keys, &cid, &bytes)? {
            ControlObject::Applied => database.mark_control_object_seen(&cid, "control")?,
            ControlObject::RotateToResolve => must_rotate = true,
            ControlObject::NotControl | ControlObject::RetryLater => {}
        }
    }
    Ok(must_rotate)
}

enum ControlObject {
    Applied,
    NotControl,
    RetryLater,
    RotateToResolve,
}

fn try_apply_control_object(database: &Database, identity: &DeviceIdentity, epoch_keys: &dyn EpochKeyStore, cid: &str, bytes: &[u8]) -> EnrollmentResult<ControlObject> {
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
            RotationOutcome::Collision => ControlObject::RotateToResolve,
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

fn apply_incoming_request(database: &Database, identity: &DeviceIdentity, signed: SignedEnrollmentRequest) -> EnrollmentResult<()> {
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
    database.record_enrollment_request(
        "incoming",
        &encode_id(signed.request.request_id.as_bytes()),
        &encode_id(signed.request.device_id.as_bytes()),
        &signed.request.ed25519_public,
        &signed.request.x25519_public,
        &fingerprint,
    )?;
    Ok(())
}

fn apply_incoming_grant(database: &Database, identity: &DeviceIdentity, epoch_keys: &dyn EpochKeyStore, signed: SignedEnrollmentGrant) -> EnrollmentResult<()> {
    let request_id_hex = encode_id(signed.grant.request_id.as_bytes());

    // This grant may be addressed to another device that received the same
    // request. Clear our copy only when an already trusted active peer signed
    // it and the roster names the exact requester keys we recorded.
    let unresolved_incoming = database.unresolved_incoming_request(&request_id_hex)?;
    if !signed.grant.signed_by_recovery {
        let trusted_approver = database.active_device_public_key(&encode_id(signed.grant.approver_device_id.as_bytes()))?;
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
                        database.approve_unresolved_incoming_request(&request_id_hex)?;
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
                        database.record_approved_incoming_request(
                            &request_id_hex,
                            &requester_id,
                            &requester_ed25519,
                            &requester_x25519,
                            &fingerprint,
                        )?;
                    }
                }
            }
        }
    }

    let matches_our_request = database.unresolved_outgoing_request(&request_id_hex)?;
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
    let grant_cbor = encode_signed_enrollment_grant(&signed)?;
    database.stage_enrollment_grant(&request_id_hex, &grant_cbor)?;
    Ok(())
}

// ============================== Key catch-up ==============================

/// What a received grant turned out to be, as a key share.
pub(super) enum KeyShare {
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
pub(super) fn apply_key_share(
    database: &Database,
    identity: &DeviceIdentity,
    epoch_keys: &dyn EpochKeyStore,
    signed: &SignedEnrollmentGrant,
    cid: &str,
) -> EnrollmentResult<KeyShare> {
    let grant = &signed.grant;
    if grant.signed_by_recovery {
        return Ok(KeyShare::Other);
    }
    let enrolled = database.has_epoch_history()?;
    let answers_our_request = database.outgoing_request_open(&encode_id(grant.request_id.as_bytes()))?;
    if !enrolled || answers_our_request {
        return Ok(KeyShare::Other);
    }
    let Some(current) = try_open_sealed_box(&identity.x25519_secret, &grant.sealed_epoch_key) else {
        return Ok(KeyShare::Other);
    };
    let sender = database.device_status_and_key(&encode_id(grant.approver_device_id.as_bytes()))?;
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
    if adopt_epoch_key(epoch_keys, grant.key_epoch, &current)? {
        database.record_epoch_activation(grant.key_epoch, cid)?;
    }
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
) -> EnrollmentResult<usize> {
    let lagging = database.peers_behind_epoch(keys.key_epoch)?;
    if lagging.is_empty() {
        return Ok(0);
    }
    let (recovery_ed25519, recovery_x25519) = database
        .recovery_public_keys()?
        .ok_or(EnrollmentError::MissingRecoveryKeys)?;
    let roster = database.full_roster_snapshot()?;
    for (device_id, x25519_public) in &lagging {
        let recipient: [u8; 32] = x25519_public.as_slice().try_into().map_err(|_| EnrollmentError::MalformedKey("Invalid peer X25519 key"))?;
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
        let signed = sign_enrollment_grant(&keys.signing_key, share)?;
        let bytes = encode_signed_enrollment_grant(&signed)?;
        let cid = publish_control_object(database, transports, &bytes).await?;
        database.mark_control_object_seen(&cid, "key_share")?;
        database.record_keys_shared(device_id, keys.key_epoch)?;
    }
    Ok(lagging.len())
}

/// Existing members honor rejections only from a currently trusted active
/// peer. A joining device can verify the response signature as informational
/// status before it has a roster of its own. Missing requests are retried
/// after the rest of the sweep so object ordering cannot leave peers stale.
fn apply_incoming_rejection(database: &Database, signed: threestrands_sync_envelope::SignedEnrollmentRejection) -> EnrollmentResult<bool> {
    let request_id = encode_id(signed.rejection.request_id.as_bytes());
    let rejector_id = encode_id(signed.rejection.rejector_device_id.as_bytes());
    let trusted_key = database.active_device_public_key(&rejector_id)?;
    let embedded_key = <[u8; 32]>::try_from(signed.rejection.rejector_ed25519_public.as_slice())
        .ok()
        .and_then(|bytes| VerifyingKey::from_bytes(&bytes).ok());
    let Some(embedded_key) = embedded_key else { return Ok(true) };
    let is_trusted_peer = trusted_key
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        .is_some_and(|bytes| bytes == embedded_key.to_bytes());

    let request_direction = database.enrollment_request_direction(&request_id)?;
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
    database.mark_pending_request_rejected(&request_id)?;
    Ok(true)
}

pub async fn reject_enrollment_request(
    database: &Database,
    identity: &DeviceIdentity,
    request_id_hex: &str,
    transports: &[Arc<dyn SyncTransport>],
) -> EnrollmentResult<()> {
    let exists = database.incoming_request_pending(&request_id_hex)?;
    if !exists {
        return Err(EnrollmentError::RequestNotPending);
    }
    let rejection = EnrollmentRejection {
        request_id: RequestId::from_bytes(decode_id(request_id_hex).map_err(EnrollmentError::MalformedId)?),
        rejector_device_id: identity.device_id,
        rejector_ed25519_public: ByteBuf::from(identity.verifying_key.to_bytes().to_vec()),
        rejected_at_ms: now_ms(),
    };
    let signed = sign_enrollment_rejection(&identity.signing_key, rejection)?;
    let bytes = encode_signed_enrollment_rejection(&signed)?;
    let cid = publish_control_object(database, transports, &bytes).await?;
    database.mark_control_object_seen(&cid, "enrollment_rejection")?;
    database.reject_pending_incoming_request(&request_id_hex)?;
    Ok(())
}

/// Whether a rotation was applied (or deliberately ignored), or names an
/// initiator this device hasn't heard of yet and should be retried.
pub(super) enum RotationOutcome {
    Done,
    UnknownInitiator,
    /// Another device created this epoch with a different key at the same
    /// time as this one, and this device is the one that resolves it: it
    /// rotates again, sealing a fresh key to the merged roster, so every
    /// device converges on the next epoch (and reseals under it). Exactly
    /// one of the two initiators does this — the one whose rotation has the
    /// greater content address — so they don't collide again.
    Collision,
}

pub(super) fn apply_incoming_rotation(database: &Database, identity: &DeviceIdentity, epoch_keys: &dyn EpochKeyStore, signed: SignedKeyRotation, cid: &str) -> EnrollmentResult<RotationOutcome> {
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
        let known_but_revoked = database.device_known(&encode_id(signed.rotation.initiator_device_id.as_bytes()))?;
        return Ok(if known_but_revoked { RotationOutcome::Done } else { RotationOutcome::UnknownInitiator });
    };
    if verify_key_rotation(verifying_key, &signed).is_err() {
        return Ok(RotationOutcome::Done);
    }
    apply_rotation_common(database, identity, epoch_keys, &signed, cid)
}

fn apply_rotation_common(database: &Database, identity: &DeviceIdentity, epoch_keys: &dyn EpochKeyStore, signed: &SignedKeyRotation, cid: &str) -> EnrollmentResult<RotationOutcome> {
    database.adopt_roster(&signed.rotation.roster)?;
    join_codes::note_admission_if_listed(database, identity, &signed.rotation.roster)?;
    database.set_recovery_public_keys(
        signed.rotation.recovery_ed25519_public.as_slice().try_into().map_err(|_| EnrollmentError::MalformedKey("Invalid recovery key"))?,
        signed.rotation.recovery_x25519_public.as_slice().try_into().map_err(|_| EnrollmentError::MalformedKey("Invalid recovery key"))?,
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
        if !adopt_epoch_key(epoch_keys, signed.rotation.key_epoch, &k_epoch)? {
            return Ok(if database.must_rotate_after_collision(identity, signed.rotation.key_epoch, cid)? {
                RotationOutcome::Collision
            } else {
                RotationOutcome::Done
            });
        }
        // Rotations are scanned in content-address order, not epoch order,
        // so a device catching up can meet an older rotation after a newer
        // one. Keep its key for opening that epoch's history, but never move
        // the active epoch backwards.
        database.advance_active_epoch(signed.rotation.key_epoch)?;
        database.record_epoch_activation(signed.rotation.key_epoch, cid)?;
    }
    Ok(RotationOutcome::Done)
}

/// The joining device's explicit action after visually comparing
/// [`enrollment_fingerprint`] on both screens: imports a staged grant,
/// adopting the epoch key and the whole roster it carries.
pub async fn confirm_and_import_grant(database: &Database, identity: &DeviceIdentity, epoch_keys: &dyn EpochKeyStore, request_id_hex: &str) -> EnrollmentResult<()> {
    let grant_cbor = database.staged_enrollment_grant(&request_id_hex)?;
    let signed = decode_signed_enrollment_grant(&grant_cbor)?;
    let k_epoch_bytes = try_open_sealed_box(&identity.x25519_secret, &signed.grant.sealed_epoch_key)
        .ok_or(EnrollmentError::NotSealedToThisDevice("This grant was not sealed to this device"))?;
    let k_epoch: [u8; 32] = k_epoch_bytes.try_into().map_err(|_| EnrollmentError::MalformedKey("Invalid sealed epoch key"))?;
    let earlier = open_earlier_epoch_keys(&identity.x25519_secret, &signed.grant.earlier_epoch_keys)
        .ok_or(EnrollmentError::NotSealedToThisDevice("This grant's earlier keys were not sealed to this device"))?;

    database.adopt_roster(&signed.grant.roster)?;
    let recovery_ed25519: [u8; 32] = signed.grant.recovery_ed25519_public.as_slice().try_into().map_err(|_| EnrollmentError::MalformedKey("Invalid recovery key"))?;
    let recovery_x25519: [u8; 32] = signed.grant.recovery_x25519_public.as_slice().try_into().map_err(|_| EnrollmentError::MalformedKey("Invalid recovery key"))?;
    database.set_recovery_public_keys(&recovery_ed25519, &recovery_x25519)?;

    epoch_keys.store(signed.grant.key_epoch, &k_epoch)?;
    database.set_active_epoch(signed.grant.key_epoch)?;
    let source_cid = compute_cid(&grant_cbor);
    database.record_epoch_activation(signed.grant.key_epoch, &source_cid)?;
    store_earlier_epoch_keys(database, epoch_keys, &earlier, &source_cid)?;

    database.set_enrollment_request_status(&request_id_hex, "completed")?;
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
) -> EnrollmentResult<()> {
    let (requester_device_id, requester_ed25519, requester_x25519) = database.pending_incoming_request(&request_id_hex)?;
    let requester_x25519: [u8; 32] = requester_x25519.try_into().map_err(|_| EnrollmentError::MalformedKey("Invalid requester X25519 key"))?;
    let requester_ed25519_bytes: [u8; 32] = requester_ed25519.try_into().map_err(|_| EnrollmentError::MalformedKey("Invalid requester Ed25519 key"))?;
    let requester_verifying_key = VerifyingKey::from_bytes(&requester_ed25519_bytes).map_err(|_| EnrollmentError::MalformedKey("Invalid requester Ed25519 key"))?;
    let requester_device_id_bytes = decode_id(&requester_device_id).map_err(DatabaseError::Corrupt)?;

    // The approved device must appear in its own grant's roster (and in
    // every roster snapshot this device hands out from now on), so trust
    // it locally before building the snapshot below.
    database.trust_device_keys(&requester_device_id_bytes, &requester_verifying_key, &requester_x25519)?;

    let roster = database.full_roster_snapshot()?;
    let (recovery_ed25519, recovery_x25519) = database.recovery_public_keys()?.ok_or(EnrollmentError::MissingRecoveryKeys)?;

    let grant = EnrollmentGrant {
        request_id: RequestId::from_bytes(decode_id(request_id_hex).map_err(EnrollmentError::MalformedId)?),
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
    let signed = sign_enrollment_grant(&identity.signing_key, grant)?;
    let bytes = encode_signed_enrollment_grant(&signed)?;
    let cid = publish_control_object(database, transports, &bytes).await?;
    database.mark_control_object_seen(&cid, "enrollment_grant")?;

    database.set_enrollment_request_status(&request_id_hex, "approved")?;
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
) -> EnrollmentResult<()> {
    // Keys loaded before another rotation landed would build the epoch that
    // rotation already created, under a different key.
    if database.active_epoch()? != keys.key_epoch {
        return Err(EnrollmentError::EpochChanged);
    }
    let (recovery_ed25519, recovery_x25519) = database.recovery_public_keys()?.ok_or(EnrollmentError::MissingRecoveryKeys)?;

    if let Some(hex) = revoke_device_id_hex {
        let device_id = decode_id(hex).map_err(EnrollmentError::MalformedId)?;
        database.revoke_device(&device_id)?;
    }

    // The published roster carries every device, including anyone just
    // revoked, so a device that missed this rotation still learns the
    // revocation explicitly the next time it applies this object. Only
    // the still-active subset receives a sealed stanza for the new key.
    let roster = database.full_roster_snapshot()?;
    let active_recipients: Vec<&RosterEntry> = roster.iter().filter(|entry| entry.status == "active").collect();

    let mut k_epoch = [0u8; 32];
    rand::Rng::fill_bytes(&mut threestrands_sync_envelope::os_rng(), &mut k_epoch);
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
    let signed = sign_key_rotation(&identity.signing_key, rotation)?;
    let bytes = encode_signed_key_rotation(&signed)?;
    let cid = publish_control_object(database, transports, &bytes).await?;
    database.mark_control_object_seen(&cid, "key_rotation")?;

    epoch_keys.store(next_epoch, &k_epoch)?;
    database.advance_active_epoch(next_epoch)?;
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
pub async fn join_with_recovery_phrase(database: &Database, identity: &DeviceIdentity, epoch_keys: &dyn EpochKeyStore, phrase: &str, transports: &[Arc<dyn SyncTransport>]) -> EnrollmentResult<()> {
    require_not_started(database)?;
    refuse_legacy_space(transports).await?;
    database.set_beta_features_enabled(true)?;
    let seed = recovery_seed_from_phrase(phrase).map_err(EnrollmentError::InvalidRecoveryPhrase)?;
    let recovery_secret_bytes = recovery_x25519_secret(&seed).to_bytes();

    let mut opened: std::collections::BTreeMap<u32, (SignedKeyRotation, String, [u8; 32])> = std::collections::BTreeMap::new();
    for transport in transports {
        let mut cursor: Option<String> = None;
        loop {
            let Some(page) = transport.scan(cursor.as_deref()).await? else { break };
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
        return Err(EnrollmentError::NoRecoverableRotation);
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
        return Err(EnrollmentError::RecoveryIncomplete);
    }

    database.adopt_roster(&latest.rotation.roster)?;
    database.set_recovery_public_keys(
        latest.rotation.recovery_ed25519_public.as_slice().try_into().map_err(|_| EnrollmentError::MalformedKey("Invalid recovery key"))?,
        latest.rotation.recovery_x25519_public.as_slice().try_into().map_err(|_| EnrollmentError::MalformedKey("Invalid recovery key"))?,
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
    let signed_announcement = sign_enrollment_grant(&recovery_ed25519_signing_key(&seed), announcement)?;
    let announcement_bytes = encode_signed_enrollment_grant(&signed_announcement)?;
    let announcement_cid = publish_control_object(database, transports, &announcement_bytes).await?;
    database.mark_control_object_seen(&announcement_cid, "enrollment_grant")?;
    Ok(())
}

pub(super) fn require_not_started(database: &Database) -> EnrollmentResult<()> {
    if !matches!(database.enrollment_status()?, EnrollmentStatus::NotStarted) {
        return Err(EnrollmentError::AlreadyEnrolled);
    }
    Ok(())
}
