//! Keychain-free stand-ins for a device's identity and epoch-key storage,
//! shared by every test that drives enrollment and sync end to end.

use rand::Rng;
use rusqlite::params;
use threestrands_sync_envelope::DeviceId as EnvelopeDeviceId;
use threestrands_sync_envelope::SigningKey;

use super::crypto::EpochKeyStore;
use super::EnrollmentResult;
use crate::db::Database;
use crate::replicated_sync::{x25519_public_bytes, DeviceIdentity, LocalKeys, SPACE_ID};

/// Builds a synthetic device identity directly, exactly as `test_keys`
/// does in `replicated_sync.rs` — never touches the OS keychain.
pub(crate) fn test_identity(database: &Database) -> DeviceIdentity {
    let device_id = {
        let mut connection = database.test_connection().unwrap();
        let tx = connection.transaction().unwrap();
        let device_id = crate::db::replicated_sync::ensure_space_and_device(&tx).unwrap();
        tx.commit().unwrap();
        device_id
    };
    let signing_key = SigningKey::generate(&mut threestrands_sync_envelope::os_rng());
    let verifying_key = signing_key.verifying_key();
    let mut x25519_secret = [0u8; 32];
    threestrands_sync_envelope::os_rng().fill_bytes(&mut x25519_secret);
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
    fn store(&self, key_epoch: u32, key: &[u8; 32]) -> EnrollmentResult<()> {
        self.0.lock().unwrap().insert(key_epoch, *key);
        Ok(())
    }

    fn load(&self, key_epoch: u32) -> EnrollmentResult<Option<[u8; 32]>> {
        Ok(self.get(key_epoch))
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
        .test_connection()
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
