//! A seeded, multi-device simulation of replicated sync, driving real
//! in-memory databases through the same enrollment, push, and pull code the
//! app runs, over one fault-injecting fake transport.
//!
//! Each seed produces a schedule of rounds. In a round every online device
//! may write, edit, or delete snippets through the app's own mutations, then
//! sync. Between rounds devices go offline (briefly, or for weeks), new
//! devices join, the group's keys rotate, and the transport drops, delays,
//! or reorders what it returns. After the schedule, every device comes back
//! and syncs until quiet, and the run is checked against an oracle: the
//! union of every device's operations fed through `threestrands_sync_core`.
//!
//! Checked after every round: each device's progress is at most what it
//! applied, and causally closed. Checked at the end: every device has the
//! oracle's exact frontier for every field (so the same values *and* the
//! same unresolved conflicts), the oracle's winning snippet rows, every
//! other device's whole feed applied, and progress equal to applied.
//!
//! A failure names its seed; pin it in `pinned_seeds_stay_green` once
//! fixed.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use rusqlite::params;
use threestrands_sync_core::{OperationGraph, WinnerStamp};
use threestrands_sync_protocol::EntityType;
use threestrands_sync_transport::fake::FakeTransport;
use threestrands_sync_transport::{Cid as TransportCid, SyncTransport};

use crate::db::Database;
use crate::enrollment::test_support::{local_keys_for, test_identity, FakeEpochKeyStore};
use crate::enrollment::{begin_genesis, join_with_recovery_phrase, rotate_epoch, run_enrollment_sweep};
use crate::replicated_sync::{decode_id, encode_id, pull_from_transports, push_pending_events, DeviceIdentity};

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

    async fn sync(&self, index: usize) {
        let device = &self.devices[index];
        let transports = &self.transports;
        if let Err(error) = run_enrollment_sweep(&device.database, &device.identity, &device.epoch_keys, transports).await {
            // An injected outage can fail a sweep; it simply runs again next round.
            if !error.contains("transient") {
                self.fail(format!("{} sweep: {error}", device.name));
            }
        }
        let keys = local_keys_for(&device.database, &device.identity, &device.epoch_keys);
        if let Err(error) = push_pending_events(&device.database, &keys, transports).await {
            self.fail(format!("{} push: {error}", device.name));
        }
        if let Err(error) = pull_from_transports(&device.database, &keys, transports).await {
            self.fail(format!("{} pull: {error}", device.name));
        }
        check_progress_is_closed(self, device);
    }

    fn write(&mut self, index: usize) {
        let ids = self.devices[index].snippet_ids();
        let roll = self.rng.gen_range(0..10);
        self.names += 1;
        let name = format!("name {}", self.names);
        let database = &self.devices[index].database;
        let result = if ids.is_empty() || roll < 4 {
            database.create_snippet(&name, "body").map_err(String::from).and_then(|snippet| {
                database.record_local_entity_write(
                    EntityType::Snippet,
                    &snippet.id,
                    serde_json::to_value(&snippet).unwrap(),
                    None,
                )
            })
        } else {
            let id = &ids[self.rng.gen_range(0..ids.len())];
            if roll < 8 {
                database.update_snippet(id, &name, "edited").map_err(String::from).and_then(|snippet| {
                    database.record_local_entity_write(
                        EntityType::Snippet,
                        id,
                        serde_json::to_value(&snippet).unwrap(),
                        Some(BTreeSet::from(["name".to_string(), "body".to_string()])),
                    )
                })
            } else {
                database
                    .delete_snippet(id)
                    .map_err(String::from)
                    .and_then(|_| database.record_local_entity_deletion(EntityType::Snippet, id))
            }
        };
        if let Err(error) = result {
            self.fail(format!("{} write: {error}", self.devices[index].name));
        }
    }

    /// Rotates from a device that is on the newest epoch and already knows
    /// every member, so no two rotations ever claim the same epoch and every
    /// member gets the new key. (Rotating concurrently, or before hearing of
    /// a new member, is a known gap outside this harness.)
    async fn maybe_rotate(&mut self) {
        let newest = self.devices.iter().map(SimDevice::active_epoch).max().unwrap();
        let members: BTreeSet<String> = self.devices.iter().map(SimDevice::device_id_hex).collect();
        let candidates: Vec<usize> = (0..self.devices.len())
            .filter(|&index| {
                let device = &self.devices[index];
                device.offline_until <= self.round
                    && device.active_epoch() == newest
                    && device
                        .database
                        .known_device_roster()
                        .unwrap()
                        .iter()
                        .map(|(id, _)| encode_id(id.as_bytes()))
                        .collect::<BTreeSet<_>>()
                        .is_superset(&members)
            })
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
                    // Weeks away: past the 30-day horizon Step 3 compacts at.
                    self.devices[index].offline_until = self.round + 8;
                    self.now += 5 * DAY_MS;
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
            self.sync(index).await;
        }
    }

    /// Brings every device back and syncs until nothing changes. Clearing
    /// `retry_at` stands in for the time delivery backoff would wait.
    async fn heal(&mut self) {
        for device in &mut self.devices {
            device.offline_until = 0;
        }
        for _ in 0..6 {
            self.round += 1;
            self.now += HOUR_MS;
            crate::sync_policy::set_test_clock(Some(self.now));
            for index in 0..self.devices.len() {
                self.devices[index].database.connection().unwrap().execute("UPDATE sync_deliveries SET retry_at=NULL", []).unwrap();
                self.sync(index).await;
            }
        }
    }

    /// How `device_id_hex`'s own objects stand: deliveries by state, and
    /// how many of its event objects the transport can't return.
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
            let mut statement = connection.prepare("SELECT cid FROM sync_objects WHERE event_id IS NOT NULL").unwrap();
            let cids: Vec<String> = statement.query_map([], |row| row.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
            (states, cids)
        };
        let missing: Vec<String> = cids
            .iter()
            .filter(|cid| (0..4).all(|_| futures_lite_block_on(self.transport.get_object(&TransportCid((*cid).clone()))).is_err()))
            .map(|cid| {
                let connection = device.database.connection().unwrap();
                let described: (String, Option<String>, Option<i64>, Option<String>) = connection
                    .query_row(
                        "SELECT so.object_kind, se.device_id, se.device_sequence, se.state FROM sync_objects so
                         LEFT JOIN sync_events se ON se.event_id = so.event_id WHERE so.cid=?1",
                        params![cid],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                    )
                    .unwrap();
                let delivery: Vec<(String, i64)> = {
                    let mut statement = connection.prepare("SELECT state, attempts FROM sync_deliveries WHERE cid=?1").unwrap();
                    let rows = statement.query_map(params![cid], |row| Ok((row.get(0)?, row.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap();
                    rows
                };
                format!("{cid} {described:?} {delivery:?}")
            })
            .collect();
        format!("{states:?}, unreadable {} of {} event objects: {missing:?}", missing.len(), cids.len())
    }

    fn check_convergence(&self) {
        let oracle = Oracle::from_devices(&self.devices);
        let own_sequences: HashMap<String, i64> =
            self.devices.iter().map(|device| (device.device_id_hex(), sealed_count(device))).collect();
        for device in &self.devices {
            let frontier = frontier_of(&device.database);
            if frontier != oracle.frontier {
                let missing: Vec<_> = oracle
                    .frontier
                    .iter()
                    .filter(|(key, ids)| frontier.get(*key) != Some(ids))
                    .take(3)
                    .map(|(key, ids)| (key.clone(), ids.clone(), frontier.get(key).cloned()))
                    .collect();
                let progress: Vec<(String, i64, (i64, i64))> = own_sequences
                    .iter()
                    .map(|(other, sealed)| (other.clone(), *sealed, progress_of(&device.database, other)))
                    .collect();
                let lagging: Vec<String> = progress
                    .iter()
                    .filter(|(_, sealed, (applied, _))| applied < sealed)
                    .map(|(other, _, _)| format!("{other}: {}", self.delivery_summary(other)))
                    .collect();
                self.fail(format!(
                    "{} disagrees with the oracle's frontier, e.g. (field, oracle, device) {missing:?}; (device, sealed, (applied, progress)) {progress:?}; lagging deliveries {lagging:?}",
                    device.name
                ));
            }
            let snippets = snippet_rows(&device.database);
            if snippets != oracle.snippets {
                self.fail(format!("{} materialized {snippets:?}, oracle {:?}", device.name, oracle.snippets));
            }
            for (other, sealed) in &own_sequences {
                let (applied, progress) = progress_of(&device.database, other);
                if applied != *sealed || progress != applied {
                    self.fail(format!(
                        "{} has applied {applied} (progress {progress}) of {other}'s {sealed} events; {other}'s deliveries: {}",
                        device.name,
                        self.delivery_summary(other)
                    ));
                }
            }
        }
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

fn sealed_count(device: &SimDevice) -> i64 {
    device
        .database
        .connection()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM sync_events WHERE device_id=?1 AND state='sealed'",
            params![device.device_id_hex()],
            |row| row.get(0),
        )
        .unwrap()
}

fn progress_of(database: &Database, device_id_hex: &str) -> (i64, i64) {
    database
        .connection()
        .unwrap()
        .query_row(
            "SELECT applied_sequence, progress_sequence FROM sync_device_progress WHERE device_id=?1",
            params![device_id_hex],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap_or((0, 0))
}

/// Progress never exceeds applied, and every device's progress prefix ends
/// in an event whose causal vector the progress vector covers.
fn check_progress_is_closed(sim: &Sim, device: &SimDevice) {
    let connection = device.database.connection().unwrap();
    let rows: Vec<(String, i64, i64)> = {
        let mut statement = connection.prepare("SELECT device_id, applied_sequence, progress_sequence FROM sync_device_progress").unwrap();
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        rows
    };
    let progress: HashMap<String, i64> = rows.iter().map(|(id, _, progress)| (id.clone(), *progress)).collect();
    for (id, applied, sequence) in &rows {
        if sequence > applied {
            sim.fail(format!("{} claims progress {sequence} past applied {applied} for {id}", device.name));
        }
        if *sequence == 0 {
            continue;
        }
        let vector: String = connection
            .query_row(
                "SELECT causal_vector FROM sync_events WHERE device_id=?1 AND device_sequence=?2",
                params![id, sequence],
                |row| row.get(0),
            )
            .unwrap_or_else(|error| sim.fail(format!("{} lacks the causal vector of {id}:{sequence}: {error}", device.name)));
        let needs: Vec<(String, i64)> = serde_json::from_str(&vector).unwrap();
        for (dependency, needed) in needs {
            if progress.get(&dependency).copied().unwrap_or(0) < needed {
                sim.fail(format!("{}'s progress {id}:{sequence} isn't closed: needs {dependency}:{needed}", device.name));
            }
        }
    }
}

type FieldKey = (String, String, String);

fn frontier_of(database: &Database) -> BTreeMap<FieldKey, BTreeSet<String>> {
    let connection = database.connection().unwrap();
    let mut statement = connection.prepare("SELECT entity_type, entity_id, field, operation_id FROM sync_field_frontier").unwrap();
    let mut frontier: BTreeMap<FieldKey, BTreeSet<String>> = BTreeMap::new();
    for row in statement
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?)))
        .unwrap()
    {
        let (entity_type, entity_id, field, operation_id) = row.unwrap();
        frontier.entry((entity_type, entity_id, field)).or_default().insert(operation_id);
    }
    frontier
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

/// What every device should converge to: the union of every device's
/// recorded operations, fed through the pure operation graph.
struct Oracle {
    frontier: BTreeMap<FieldKey, BTreeSet<String>>,
    snippets: BTreeMap<String, (String, String)>,
}

impl Oracle {
    fn from_devices(devices: &[SimDevice]) -> Self {
        let mut graph = OperationGraph::new();
        let mut fields: BTreeSet<FieldKey> = BTreeSet::new();
        for device in devices {
            let connection = device.database.connection().unwrap();
            let mut statement = connection
                .prepare("SELECT operation_id, entity_type, entity_id, field, value, winner_stamp FROM sync_operations")
                .unwrap();
            let operations = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Vec<u8>>(5)?,
                    ))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            for (operation_id, entity_type, entity_id, field, value, stamp) in operations {
                let parents: Vec<[u8; 16]> = {
                    let mut statement =
                        connection.prepare("SELECT parent_operation_id FROM sync_operation_parents WHERE operation_id=?1").unwrap();
                    let parents = statement
                        .query_map(params![operation_id], |row| row.get::<_, String>(0))
                        .unwrap()
                        .map(|parent| decode_id(&parent.unwrap()).unwrap())
                        .collect();
                    parents
                };
                fields.insert((entity_type.clone(), entity_id.clone(), field.clone()));
                graph.apply(threestrands_sync_core::Operation {
                    operation_id: decode_id(&operation_id).unwrap(),
                    entity_type: entity_type.parse().unwrap(),
                    entity_id,
                    field,
                    value: value.map(|json| serde_json::from_str(&json).unwrap()),
                    parents,
                    stamp: decode_stamp(&stamp),
                });
            }
        }

        let mut frontier = BTreeMap::new();
        for (entity_type, entity_id, field) in &fields {
            let ids: BTreeSet<String> = graph
                .frontier(entity_type.parse().unwrap(), entity_id, field)
                .iter()
                .map(encode_id)
                .collect();
            frontier.insert((entity_type.clone(), entity_id.clone(), field.clone()), ids);
        }

        let mut snippets = BTreeMap::new();
        let snippet_ids: BTreeSet<&String> =
            fields.iter().filter(|(entity_type, _, _)| entity_type == "snippet").map(|(_, id, _)| id).collect();
        for id in snippet_ids {
            if graph.entity_exists(EntityType::Snippet, id) != Some(true) {
                continue;
            }
            let winner = |field: &str| -> String {
                graph
                    .resolve_field(EntityType::Snippet, id, field)
                    .and_then(|resolution| resolution.winner.value.clone())
                    .and_then(|value| value.as_str().map(str::to_string))
                    .unwrap_or_default()
            };
            snippets.insert(id.clone(), (winner("name"), winner("body")));
        }
        Oracle { frontier, snippets }
    }
}

fn decode_stamp(bytes: &[u8]) -> WinnerStamp {
    let array = |range: std::ops::Range<usize>| -> [u8; 16] { bytes[range].try_into().unwrap() };
    WinnerStamp {
        lamport: u64::from_be_bytes(bytes[0..8].try_into().unwrap()),
        device_id: array(8..24),
        event_id: array(24..40),
        operation_id: array(40..56),
    }
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
/// - 5, 10: a rotation scanned before its initiator's announcement was
///   dropped, leaving devices without that epoch's key.
/// - 15, 30, 33: joins refused or interrupted while objects were delayed.
/// - 81: a recovery join that missed a delayed rotation joined without its
///   key for good.
/// - 278: a rotation published during an outage never reached storage.
#[tokio::test]
async fn pinned_seeds_stay_green() {
    for seed in [5, 10, 15, 30, 33, 81, 278] {
        run_seed(seed, 18).await;
    }
}
