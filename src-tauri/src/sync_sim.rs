//! A seeded, multi-device simulation of replicated sync, driving real
//! in-memory databases through the same enrollment, push, and pull code the
//! app runs, over one fault-injecting fake transport.
//!
//! Each seed produces a schedule of rounds. In a round every online device
//! may write, edit, or delete snippets through the app's own mutations, then
//! sync. Between rounds devices go offline (briefly, or for months), new
//! devices join, the group's keys rotate, and the transport drops, delays,
//! or reorders what it returns. After the schedule, every device comes back
//! and syncs until nothing changes.
//!
//! The oracle is independent of the replica code: the harness records every
//! write and deletion as it happens, with the values it replaced, and feeds
//! that history to the reference `threestrands_sync_core::OperationGraph`.
//! At the end every device must hold exactly the values the graph says
//! survive (so the same values *and* the same unresolved conflicts), show
//! the graph's winning snippet rows, and have an identical replica state.
//!
//! A failure names its seed; pin it in `pinned_seeds_stay_green` once
//! fixed.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use threestrands_sync_core::{Dot, FieldKey, Operation, OperationGraph, OperationId, WinnerStamp};
use threestrands_sync_protocol::EntityType;
use threestrands_sync_transport::fake::FakeTransport;
use threestrands_sync_transport::{Cid as TransportCid, SyncTransport};

use crate::db::Database;
use crate::enrollment::test_support::{local_keys_for, test_identity, FakeEpochKeyStore};
use crate::enrollment::{begin_genesis, join_with_recovery_phrase, rotate_epoch, run_enrollment_sweep};
use crate::replicated_sync::{encode_id, pull_from_transports, push_local_state, DeviceIdentity};

const HOUR_MS: i64 = 60 * 60 * 1000;
const DAY_MS: i64 = 24 * HOUR_MS;
const MAX_DEVICES: usize = 5;

struct SimDevice {
    name: String,
    database: Database,
    identity: DeviceIdentity,
    epoch_keys: FakeEpochKeyStore,
    /// Offline until this round (exclusive).
    offline_until: usize,
}

impl SimDevice {
    fn device_id_hex(&self) -> String {
        encode_id(self.identity.device_id.as_bytes())
    }

    fn active_epoch(&self) -> u32 {
        self.database
            .connection()
            .unwrap()
            .query_row("SELECT active_epoch FROM sync_spaces", [], |row| row.get(0))
            .unwrap()
    }

    fn snippet_ids(&self) -> Vec<String> {
        let connection = self.database.connection().unwrap();
        let mut statement = connection.prepare("SELECT id FROM snippets ORDER BY id").unwrap();
        let ids = statement.query_map([], |row| row.get(0)).unwrap().collect::<Result<Vec<String>, _>>().unwrap();
        ids
    }
}

struct Sim {
    seed: u64,
    rng: StdRng,
    transport: Arc<FakeTransport>,
    transports: Vec<Arc<dyn SyncTransport>>,
    devices: Vec<SimDevice>,
    phrase: String,
    now: i64,
    round: usize,
    names: usize,
    /// Every write and deletion so far, as the reference graph sees it.
    history: OperationGraph,
    /// The graph's deletion markers: they stay in its frontier but stand
    /// for "no value".
    markers: BTreeSet<OperationId>,
}

impl Sim {
    async fn new(seed: u64) -> Self {
        crate::sync_policy::set_test_clock(Some(1_800_000_000_000));
        let transport = Arc::new(FakeTransport::new("shared"));
        let transports: Vec<Arc<dyn SyncTransport>> = vec![transport.clone()];
        let founder = new_device("device-0");
        let phrase = begin_genesis(&founder.database, &founder.identity, &founder.epoch_keys, &transports, false)
            .await
            .unwrap();
        let mut sim = Sim {
            seed,
            rng: StdRng::seed_from_u64(seed),
            transport,
            transports,
            devices: vec![founder],
            phrase,
            now: 1_800_000_000_000,
            round: 0,
            names: 0,
            history: OperationGraph::new(),
            markers: BTreeSet::new(),
        };
        if sim.rng.gen_bool(0.5) {
            sim.transport.enable_scan_reordering();
        }
        for _ in 0..sim.rng.gen_range(1..=2) {
            sim.join().await;
        }
        sim
    }

    fn fail(&self, message: impl std::fmt::Display) -> ! {
        panic!("sync simulation seed {} failed in round {}: {message}", self.seed, self.round);
    }

    /// Joins a new device by recovery phrase. A connector that looks like
    /// an earlier build's group because a delayed object hid the protocol
    /// marker is refused before anything is set up, an injected outage can
    /// interrupt the join's scan, and a delayed rotation can leave nothing
    /// yet to open, or a gap that would make part of the history unreadable.
    /// In each case the user would simply try again later, so the join is
    /// skipped this round.
    async fn join(&mut self) {
        let device = new_device(&format!("device-{}", self.devices.len()));
        match join_with_recovery_phrase(&device.database, &device.identity, &device.epoch_keys, &self.phrase, &self.transports).await {
            Ok(()) => self.devices.push(device),
            Err(error) if is_retryable_join_failure(&error) => {}
            Err(error) => self.fail(format!("{} couldn't join: {error}", device.name)),
        }
    }

    /// Sweeps, pushes, and pulls one device. Returns a summary when it
    /// sealed a new snapshot, merged a peer's, or hit a failure, so healing
    /// knows when to stop and can say what kept happening.
    async fn sync(&self, index: usize) -> Option<String> {
        let device = &self.devices[index];
        let transports = &self.transports;
        if let Err(error) = run_enrollment_sweep(&device.database, &device.identity, &device.epoch_keys, transports).await {
            // An injected outage can fail a sweep; it simply runs again next round.
            if !error.contains("transient") {
                self.fail(format!("{} sweep: {error}", device.name));
            }
        }
        let keys = local_keys_for(&device.database, &device.identity, &device.epoch_keys);
        let pushed = push_local_state(&device.database, &keys, transports)
            .await
            .unwrap_or_else(|error| self.fail(format!("{} push: {error}", device.name)));
        let pulled = pull_from_transports(&device.database, &keys, transports)
            .await
            .unwrap_or_else(|error| self.fail(format!("{} pull: {error}", device.name)));
        let shared = crate::enrollment::share_keys_with_lagging_peers(&device.database, &keys, transports)
            .await
            .unwrap_or_else(|error| self.fail(format!("{} key share: {error}", device.name)));
        if let Err(error) = device.database.load_replica_state() {
            self.fail(format!("{}'s replica is malformed: {error}", device.name));
        }
        let busy = pushed.sealed_snapshot || pushed.failed > 0 || pulled.merged_states > 0 || pulled.failed_transports > 0 || shared > 0;
        busy.then(|| format!("{}: {pushed:?} {pulled:?} shared keys with {shared}", device.name))
    }

    fn write(&mut self, index: usize) {
        let ids = self.devices[index].snippet_ids();
        let roll = self.rng.gen_range(0..10);
        self.names += 1;
        let name = format!("name {}", self.names);
        let database = &self.devices[index].database;
        let (entity_id, result) = if ids.is_empty() || roll < 4 {
            match database.create_snippet(&name, "body") {
                Ok(snippet) => {
                    let result = database.record_local_entity_write(
                        EntityType::Snippet,
                        &snippet.id,
                        serde_json::to_value(&snippet).unwrap(),
                        None,
                    );
                    (snippet.id, result)
                }
                Err(error) => self.fail(format!("create: {error}")),
            }
        } else {
            let id = ids[self.rng.gen_range(0..ids.len())].clone();
            let before = entity_values(database, &id);
            if roll < 8 {
                let result = database.update_snippet(&id, &name, "edited").map_err(String::from).and_then(|snippet| {
                    database.record_local_entity_write(
                        EntityType::Snippet,
                        &id,
                        serde_json::to_value(&snippet).unwrap(),
                        Some(BTreeSet::from(["name".to_string(), "body".to_string()])),
                    )
                });
                self.record_write(index, &id, &before);
                return self.check_write(index, result);
            }
            let result = database
                .delete_snippet(&id)
                .map_err(String::from)
                .and_then(|_| database.record_local_entity_deletion(EntityType::Snippet, &id));
            self.record_deletion(&before);
            return self.check_write(index, result);
        };
        self.record_write(index, &entity_id, &BTreeMap::new());
        self.check_write(index, result);
    }

    fn check_write(&self, index: usize, result: Result<(), String>) {
        if let Err(error) = result {
            self.fail(format!("{} write: {error}", self.devices[index].name));
        }
    }

    /// Adds a write just made on device `index` to the history: one
    /// operation per field it wrote, naming as parents the values that field
    /// held on that device just before (`before`).
    fn record_write(&mut self, index: usize, entity_id: &str, before: &BTreeMap<FieldKey, Vec<(Dot, u64)>>) {
        let device = &self.devices[index];
        let state = device.database.load_replica_state().unwrap();
        let own = *device.identity.device_id.as_bytes();
        let counter = state.context().get(&own).copied().unwrap_or(0);
        for (key, values) in state.fields().iter().filter(|(key, _)| key.entity_id == entity_id) {
            let Some(written) = values.iter().find(|value| value.dot == Dot { device_id: own, counter }) else { continue };
            let parents = before.get(key).map(|dots| dots.iter().map(|(dot, _)| operation_id(*dot, key)).collect()).unwrap_or_default();
            self.history.apply(Operation {
                operation_id: operation_id(written.dot, key),
                entity_type: key.entity_type,
                entity_id: key.entity_id.clone(),
                field: key.field.clone(),
                value: written.value.clone(),
                parents,
                stamp: stamp(written.dot, written.lamport),
            });
        }
    }

    /// Adds a deletion to the history: a marker on every field the entity
    /// held, replacing those values.
    fn record_deletion(&mut self, before: &BTreeMap<FieldKey, Vec<(Dot, u64)>>) {
        for (key, dots) in before {
            let mut marker = [0xffu8; 16];
            marker[..8].copy_from_slice(&(self.markers.len() as u64 + 1).to_be_bytes());
            self.markers.insert(marker);
            self.history.apply(Operation {
                operation_id: marker,
                entity_type: key.entity_type,
                entity_id: key.entity_id.clone(),
                field: key.field.clone(),
                value: None,
                parents: dots.iter().map(|(dot, _)| operation_id(*dot, key)).collect(),
                stamp: WinnerStamp { lamport: 0, device_id: [0u8; 16], event_id: [0u8; 16], operation_id: marker },
            });
        }
    }

    /// Rotates from a device that is on the newest epoch, so no two
    /// rotations ever claim the same epoch (concurrent rotations are a
    /// known gap outside this harness). The rotating device may not have
    /// heard of every member yet; key catch-up covers the ones its rotation
    /// isn't sealed to.
    async fn maybe_rotate(&mut self) {
        let newest = self.devices.iter().map(SimDevice::active_epoch).max().unwrap();
        let candidates: Vec<usize> = (0..self.devices.len())
            .filter(|&index| self.devices[index].offline_until <= self.round && self.devices[index].active_epoch() == newest)
            .collect();
        if candidates.is_empty() {
            return;
        }
        let device = &self.devices[candidates[self.rng.gen_range(0..candidates.len())]];
        let keys = local_keys_for(&device.database, &device.identity, &device.epoch_keys);
        if let Err(error) = rotate_epoch(&device.database, &device.identity, &keys, &device.epoch_keys, &self.transports, None).await {
            self.fail(format!("{} rotate: {error}", device.name));
        }
    }

    fn inject_faults(&mut self) {
        if self.rng.gen_bool(0.15) {
            self.transport.inject_transient_outage(self.rng.gen_range(1..=3));
        }
        if self.rng.gen_bool(0.2) {
            let device = &self.devices[self.rng.gen_range(0..self.devices.len())];
            let cids: Vec<String> = {
                let connection = device.database.connection().unwrap();
                let mut statement = connection.prepare("SELECT cid FROM sync_objects ORDER BY cid").unwrap();
                let cids = statement.query_map([], |row| row.get(0)).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
                cids
            };
            if !cids.is_empty() {
                let cid = cids[self.rng.gen_range(0..cids.len())].clone();
                self.transport.inject_delayed_visibility(&TransportCid(cid), self.rng.gen_range(1..=2));
            }
        }
    }

    async fn run_round(&mut self) {
        self.round += 1;
        self.now += HOUR_MS;
        crate::sync_policy::set_test_clock(Some(self.now));
        for index in 0..self.devices.len() {
            if self.devices[index].offline_until > self.round {
                continue;
            }
            match self.rng.gen_range(0..20) {
                0 => {
                    // A few days away.
                    self.devices[index].offline_until = self.round + self.rng.gen_range(2..6);
                    continue;
                }
                1 => {
                    // Months away.
                    self.devices[index].offline_until = self.round + 8;
                    self.now += 20 * DAY_MS;
                    continue;
                }
                2..=3 => {}
                _ => {
                    for _ in 0..self.rng.gen_range(1..=2) {
                        self.write(index);
                    }
                }
            }
        }
        if self.devices.len() < MAX_DEVICES && self.rng.gen_bool(0.1) {
            self.join().await;
        }
        if self.rng.gen_bool(0.12) {
            self.maybe_rotate().await;
        }
        self.inject_faults();
        let mut order: Vec<usize> = (0..self.devices.len()).filter(|&index| self.devices[index].offline_until <= self.round).collect();
        for position in (1..order.len()).rev() {
            order.swap(position, self.rng.gen_range(0..=position));
        }
        for index in order {
            let _ = self.sync(index).await;
        }
    }

    /// Brings every device back and syncs until a whole round changes
    /// nothing. Clearing `retry_at` stands in for the time delivery backoff
    /// would wait.
    async fn heal(&mut self) {
        for device in &mut self.devices {
            device.offline_until = 0;
        }
        let mut last_activity = Vec::new();
        for attempt in 0..20 {
            self.round += 1;
            self.now += HOUR_MS;
            crate::sync_policy::set_test_clock(Some(self.now));
            let mut activity = Vec::new();
            for index in 0..self.devices.len() {
                self.devices[index].database.connection().unwrap().execute("UPDATE sync_deliveries SET retry_at=NULL", []).unwrap();
                activity.extend(self.sync(index).await);
            }
            if activity.is_empty() && attempt > 0 {
                return;
            }
            last_activity = activity;
        }
        self.fail(format!("devices kept changing after every device came back: {last_activity:?}"));
    }

    /// How `device_id_hex`'s own objects stand: deliveries by state, and
    /// which of its snapshot objects the transport can't return.
    fn delivery_summary(&self, device_id_hex: &str) -> String {
        let Some(device) = self.devices.iter().find(|device| device.device_id_hex() == device_id_hex) else {
            return "not a simulated device".to_string();
        };
        let (states, cids) = {
            let connection = device.database.connection().unwrap();
            let mut statement =
                connection.prepare("SELECT state, COUNT(*), MAX(last_error) FROM sync_deliveries GROUP BY state").unwrap();
            let states: Vec<(String, i64, Option<String>)> =
                statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap().collect::<Result<_, _>>().unwrap();
            let mut statement = connection.prepare("SELECT cid FROM sync_objects WHERE state_sequence IS NOT NULL").unwrap();
            let cids: Vec<String> = statement.query_map([], |row| row.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
            (states, cids)
        };
        let missing: Vec<&String> = cids
            .iter()
            .filter(|cid| (0..4).all(|_| futures_lite_block_on(self.transport.get_object(&TransportCid((*cid).clone()))).is_err()))
            .collect();
        format!("{states:?}, unreadable {} of {} snapshot objects: {missing:?}", missing.len(), cids.len())
    }

    fn check_convergence(&self) {
        let states: Vec<_> = self.devices.iter().map(|device| device.database.load_replica_state().unwrap()).collect();
        let mut keys: BTreeSet<FieldKey> = states.iter().flat_map(|state| state.fields().keys().cloned()).collect();
        for device in &self.devices {
            for id in device.snippet_ids() {
                keys.insert(FieldKey { entity_type: EntityType::Snippet, entity_id: id, field: "name".to_string() });
            }
        }
        let mut expected_snippets: BTreeMap<String, (String, String)> = BTreeMap::new();
        let mut snippet_ids: BTreeSet<String> = BTreeSet::new();
        for key in &keys {
            let expected: BTreeSet<OperationId> = self.surviving(key).iter().map(|operation| operation.operation_id).collect();
            for (device, state) in self.devices.iter().zip(&states) {
                let actual: BTreeSet<OperationId> = state.values(key).iter().map(|value| operation_id(value.dot, key)).collect();
                if actual != expected {
                    let lagging: Vec<String> = self.devices.iter().map(|other| format!("{}: {}", other.name, self.delivery_summary(&other.device_id_hex()))).collect();
                    self.fail(format!(
                        "{} holds {} values for {key:?} where the history leaves {}; deliveries {lagging:?}",
                        device.name,
                        actual.len(),
                        expected.len()
                    ));
                }
            }
            if key.entity_type == EntityType::Snippet {
                snippet_ids.insert(key.entity_id.clone());
            }
        }
        for id in snippet_ids {
            let winner = |field: &str| -> Option<serde_json::Value> {
                let key = FieldKey { entity_type: EntityType::Snippet, entity_id: id.clone(), field: field.to_string() };
                self.surviving(&key).into_iter().max_by_key(|operation| operation.stamp).and_then(|operation| operation.value)
            };
            if winner("_entity") == Some(serde_json::json!(true)) {
                let text = |field: &str| winner(field).and_then(|value| value.as_str().map(str::to_string)).unwrap_or_default();
                expected_snippets.insert(id.clone(), (text("name"), text("body")));
            }
        }
        for (device, state) in self.devices.iter().zip(&states) {
            if state != &states[0] {
                self.fail(format!("{}'s replica differs from {}'s", device.name, self.devices[0].name));
            }
            let snippets = snippet_rows(&device.database);
            if snippets != expected_snippets {
                self.fail(format!("{} shows {snippets:?}, the history says {expected_snippets:?}", device.name));
            }
        }
    }

    /// The operations the history leaves standing for a field: its frontier
    /// without deletion markers.
    fn surviving(&self, key: &FieldKey) -> Vec<Operation> {
        self.history
            .frontier(key.entity_type, &key.entity_id, &key.field)
            .into_iter()
            .filter(|id| !self.markers.contains(id))
            .map(|id| self.history.operation(&id).unwrap().clone())
            .collect()
    }
}

/// Runs a fake-transport future to completion; the fake never awaits
/// anything real, so it is ready on the first poll.
fn futures_lite_block_on<F: std::future::Future>(future: F) -> F::Output {
    let waker = std::task::Waker::noop();
    let mut context = std::task::Context::from_waker(waker);
    let mut future = std::pin::pin!(future);
    match future.as_mut().poll(&mut context) {
        std::task::Poll::Ready(output) => output,
        std::task::Poll::Pending => panic!("the fake transport never blocks"),
    }
}

fn is_retryable_join_failure(error: &str) -> bool {
    error == crate::enrollment::LEGACY_SPACE_REFUSAL
        || error.contains("transient")
        || error.starts_with("No rotation object on any configured transport opened with this recovery phrase yet")
        || error == crate::enrollment::RECOVERY_INCOMPLETE
}

fn new_device(name: &str) -> SimDevice {
    let database = Database::open_memory();
    let identity = test_identity(&database);
    SimDevice { name: name.to_string(), database, identity, epoch_keys: FakeEpochKeyStore::default(), offline_until: 0 }
}

/// The values a device holds for one entity, by field.
fn entity_values(database: &Database, entity_id: &str) -> BTreeMap<FieldKey, Vec<(Dot, u64)>> {
    database
        .load_replica_state()
        .unwrap()
        .fields()
        .iter()
        .filter(|(key, _)| key.entity_id == entity_id)
        .map(|(key, values)| (key.clone(), values.iter().map(|value| (value.dot, value.lamport)).collect()))
        .collect()
}

/// The reference graph's id for one field's value from one write.
fn operation_id(dot: Dot, key: &FieldKey) -> OperationId {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(dot.device_id);
    hasher.update(dot.counter.to_be_bytes());
    hasher.update(key.entity_type.as_str());
    hasher.update([0]);
    hasher.update(&key.entity_id);
    hasher.update([0]);
    hasher.update(&key.field);
    hasher.finalize()[..16].try_into().unwrap()
}

/// Orders like the replica's working-value rule: `(lamport, device, counter)`.
fn stamp(dot: Dot, lamport: u64) -> WinnerStamp {
    let mut counter = [0u8; 16];
    counter[8..].copy_from_slice(&dot.counter.to_be_bytes());
    WinnerStamp { lamport, device_id: dot.device_id, event_id: [0u8; 16], operation_id: counter }
}

fn snippet_rows(database: &Database) -> BTreeMap<String, (String, String)> {
    let connection = database.connection().unwrap();
    let mut statement = connection.prepare("SELECT id, name, body FROM snippets").unwrap();
    let rows = statement
        .query_map([], |row| Ok((row.get::<_, String>(0)?, (row.get::<_, String>(1)?, row.get::<_, String>(2)?))))
        .unwrap()
        .collect::<Result<BTreeMap<_, _>, _>>()
        .unwrap();
    rows
}

async fn run_seed(seed: u64, rounds: usize) {
    let mut sim = Sim::new(seed).await;
    for _ in 0..rounds {
        sim.run_round().await;
    }
    sim.heal().await;
    sim.check_convergence();
    crate::sync_policy::set_test_clock(None);
}

/// 24 seeds by default. Set `THREESTRANDS_SYNC_SIM_SEEDS` to run more (for
/// example `THREESTRANDS_SYNC_SIM_SEEDS=500 cargo test --lib sync_sim`), or
/// `THREESTRANDS_SYNC_SIM_SEED` to run just one.
#[tokio::test]
async fn seeded_schedules_converge_to_the_oracle() {
    let env = |name: &str| std::env::var(name).ok().and_then(|value| value.parse::<u64>().ok());
    let seeds = match env("THREESTRANDS_SYNC_SIM_SEED") {
        Some(seed) => seed..seed + 1,
        None => 0..env("THREESTRANDS_SYNC_SIM_SEEDS").unwrap_or(24),
    };
    for seed in seeds {
        run_seed(seed, 18).await;
    }
}

/// Seeds that once failed, kept as regression cases. Operation ids are
/// random, so a seed replays its schedule but not always the exact object
/// order that exposed the bug; each still exercises that scenario.
/// - 5, 10: a rotation scanned before its initiator's announcement.
/// - 15, 30, 33: joins refused or interrupted while objects were delayed.
/// - 81: a recovery join while a rotation was still on its way.
/// - 136: a device that joined without an epoch's key, which only key
///   catch-up from a peer can repair.
/// - 278: a rotation published during an outage.
#[tokio::test]
async fn pinned_seeds_stay_green() {
    for seed in [5, 10, 15, 30, 33, 81, 136, 278] {
        run_seed(seed, 18).await;
    }
}
