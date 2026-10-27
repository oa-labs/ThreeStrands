//! Epoch-key storage and the sealing and signature checks enrollment relies
//! on: no transport access and no protocol decisions.

use serde_bytes::ByteBuf;
use threestrands_sync_envelope::limits::MAX_EARLIER_EPOCH_KEYS;
use threestrands_sync_envelope::{
    decode_signed_key_rotation, seal_to_x25519, try_open_sealed_box, verify_key_rotation, RosterEntry, SealedEpochKey,
    VerifyingKey,
};

use super::{EnrollmentError, EnrollmentResult};
use crate::db::Database;
use crate::replicated_sync::LocalKeys;

/// Where an adopted epoch key's secret bytes are persisted. Production uses
/// the OS keychain ([`KeychainEpochKeyStore`]); tests inject an in-memory
/// fake so the enrollment protocol's crypto and SQL logic can be exercised
/// without ever touching the real keychain — the same discipline
/// `Database::local_replicated_keys` already follows by never being called
/// from a test.
pub(crate) trait EpochKeyStore: Send + Sync {
    fn store(&self, key_epoch: u32, key: &[u8; 32]) -> EnrollmentResult<()>;
    fn load(&self, key_epoch: u32) -> EnrollmentResult<Option<[u8; 32]>>;
}

pub(crate) struct KeychainEpochKeyStore;

impl EpochKeyStore for KeychainEpochKeyStore {
    fn store(&self, key_epoch: u32, key: &[u8; 32]) -> EnrollmentResult<()> {
        crate::replicated_sync::store_epoch_key(key_epoch, key).map_err(EnrollmentError::Keychain)
    }

    fn load(&self, key_epoch: u32) -> EnrollmentResult<Option<[u8; 32]>> {
        crate::replicated_sync::load_epoch_key(key_epoch).map_err(EnrollmentError::Keychain)
    }
}

/// Stores a key for an epoch received from a peer, unless this device
/// already holds a *different* key for that epoch. Returns whether the key
/// is now the one held. Two devices that rotate at the same moment can both
/// create epoch N with different keys; replacing ours would strand
/// everything already sealed under it, and the two devices would simply
/// swap keys. The first key held stays, and the collision is resolved by a
/// fresh rotation instead (see [`RotationOutcome::Collision`](super::service::RotationOutcome::Collision)).
pub(super) fn adopt_epoch_key(epoch_keys: &dyn EpochKeyStore, key_epoch: u32, key: &[u8; 32]) -> EnrollmentResult<bool> {
    match epoch_keys.load(key_epoch)? {
        Some(existing) if existing != *key => Ok(false),
        Some(_) => Ok(true),
        None => {
            epoch_keys.store(key_epoch, key)?;
            Ok(true)
        }
    }
}


pub(super) fn roster_entry_verifying_key(entry: &RosterEntry) -> EnrollmentResult<VerifyingKey> {
    let bytes: [u8; 32] = entry.ed25519_public.as_slice().try_into().map_err(|_| EnrollmentError::MalformedKey("Invalid roster public key"))?;
    VerifyingKey::from_bytes(&bytes).map_err(|_| EnrollmentError::MalformedKey("Invalid roster public key"))
}

pub(crate) fn roster_entry_x25519(entry: &RosterEntry) -> EnrollmentResult<[u8; 32]> {
    entry.x25519_public.as_slice().try_into().map_err(|_| EnrollmentError::MalformedKey("Invalid roster X25519 key"))
}

/// Seals every earlier epoch key this device holds to `recipient_x25519`,
/// for a grant or invitation, so the new device can open history sealed
/// before the current epoch.
pub(crate) fn seal_earlier_epoch_keys(keys: &LocalKeys, recipient_x25519: &[u8; 32]) -> EnrollmentResult<Vec<SealedEpochKey>> {
    if keys.earlier_epoch_keys.len() > MAX_EARLIER_EPOCH_KEYS {
        return Err(EnrollmentError::HistoryLimitReached);
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
) -> EnrollmentResult<()> {
    for (key_epoch, key) in earlier {
        if adopt_epoch_key(epoch_keys, *key_epoch, key)? {
            database.record_epoch_activation(*key_epoch, source_cid)?;
        }
    }
    Ok(())
}

/// A rotation counts as evidence of a space only when it verifies against
/// its own initiator's roster entry — the same self-consistency check a
/// grant gets. This never establishes trust in the space; it only keeps
/// arbitrary bytes that happen to decode from steering the setup choice.
pub(super) fn is_self_consistent_rotation(bytes: &[u8]) -> bool {
    let Ok(signed) = decode_signed_key_rotation(bytes) else { return false };
    let Some(initiator) = signed.rotation.roster.iter().find(|entry| entry.device_id == signed.rotation.initiator_device_id) else {
        return false;
    };
    let Ok(verifying_key) = roster_entry_verifying_key(initiator) else { return false };
    verify_key_rotation(&verifying_key, &signed).is_ok()
}
