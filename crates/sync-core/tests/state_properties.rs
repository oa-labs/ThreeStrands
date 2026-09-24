//! Randomized property tests for [`ReplicaState`]: merge is commutative,
//! associative, and idempotent, replicas that have merged everything agree,
//! and what they agree on is exactly what the reference [`OperationGraph`]
//! says should survive the same history.
//!
//! Each case expands a proptest seed into a history over a few replicas:
//! writes to one or more fields of an entity, entity deletions, and merges
//! of one replica's state into another's, interleaved at random so writes
//! and deletions are often concurrent.

use std::collections::{BTreeMap, BTreeSet};

use proptest::prelude::*;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde_json::json;
use threestrands_sync_core::{
    Dot, EntityType, FieldKey, Operation, OperationGraph, OperationId, ReplicaState, WinnerStamp,
};

const ENTITIES: [&str; 2] = ["task-1", "task-2"];
const FIELDS: [&str; 3] = ["title", "notes", "status"];

fn device(index: usize) -> [u8; 16] {
    [index as u8 + 1; 16]
}

/// The reference model's id for one field's value from one write.
fn operation_id(dot: Dot, key: &FieldKey) -> OperationId {
    let mut id = [0u8; 16];
    for (index, byte) in dot
        .device_id
        .iter()
        .copied()
        .chain(dot.counter.to_be_bytes())
        .chain(key.entity_id.bytes())
        .chain(key.field.bytes())
        .enumerate()
    {
        id[index % 16] = id[index % 16].wrapping_mul(31).wrapping_add(byte);
    }
    id
}

/// A replayed history: every replica's final state, a snapshot taken at
/// random moments (for the merge laws), and the reference graph.
struct History {
    replicas: Vec<ReplicaState>,
    snapshots: Vec<ReplicaState>,
    graph: OperationGraph,
    /// Ids of the graph's deletion markers, which survive in the graph's
    /// frontier but stand for "no value".
    markers: BTreeSet<OperationId>,
}

fn run(seed: u64, replica_count: usize, steps: usize) -> History {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut replicas = vec![ReplicaState::new(); replica_count];
    let mut snapshots = Vec::new();
    let mut graph = OperationGraph::new();
    let mut markers = BTreeSet::new();
    let mut lamport = 0u64;
    let mut marker_count = 0u64;

    for _ in 0..steps {
        let replica = rng.gen_range(0..replica_count);
        let entity = ENTITIES[rng.gen_range(0..ENTITIES.len())];
        match rng.gen_range(0..10) {
            0..=4 => {
                lamport += 1;
                let fields: Vec<&str> = FIELDS.iter().copied().filter(|_| rng.gen_bool(0.6)).collect();
                let fields = if fields.is_empty() { vec![FIELDS[0]] } else { fields };
                let parents: BTreeMap<FieldKey, Vec<OperationId>> = fields
                    .iter()
                    .map(|field| {
                        let key = FieldKey { entity_type: EntityType::Task, entity_id: entity.to_string(), field: field.to_string() };
                        let parents = replicas[replica].values(&key).iter().map(|value| operation_id(value.dot, &key)).collect();
                        (key, parents)
                    })
                    .collect();
                let dot = replicas[replica].write(
                    device(replica),
                    lamport,
                    EntityType::Task,
                    entity,
                    fields.iter().map(|field| (field.to_string(), Some(json!(format!("{field} {lamport}"))))),
                );
                for (key, parents) in parents {
                    graph.apply(Operation {
                        operation_id: operation_id(dot, &key),
                        entity_type: key.entity_type,
                        entity_id: key.entity_id.clone(),
                        field: key.field.clone(),
                        value: Some(json!(format!("{} {lamport}", key.field))),
                        parents,
                        stamp: WinnerStamp {
                            lamport,
                            device_id: dot.device_id,
                            event_id: [0u8; 16],
                            operation_id: {
                                let mut counter = [0u8; 16];
                                counter[8..].copy_from_slice(&dot.counter.to_be_bytes());
                                counter
                            },
                        },
                    });
                }
            }
            5 => {
                let held: Vec<(FieldKey, Vec<OperationId>)> = replicas[replica]
                    .fields()
                    .iter()
                    .filter(|(key, _)| key.entity_id == entity)
                    .map(|(key, values)| (key.clone(), values.iter().map(|value| operation_id(value.dot, key)).collect()))
                    .collect();
                replicas[replica].remove_entity(EntityType::Task, entity);
                for (key, parents) in held {
                    marker_count += 1;
                    let mut marker = [0xffu8; 16];
                    marker[..8].copy_from_slice(&marker_count.to_be_bytes());
                    markers.insert(marker);
                    graph.apply(Operation {
                        operation_id: marker,
                        entity_type: key.entity_type,
                        entity_id: key.entity_id,
                        field: key.field,
                        value: None,
                        parents,
                        stamp: WinnerStamp { lamport: 0, device_id: [0u8; 16], event_id: [0u8; 16], operation_id: marker },
                    });
                }
            }
            _ => {
                let from = rng.gen_range(0..replica_count);
                let source = replicas[from].clone();
                replicas[replica].merge(&source);
            }
        }
        if rng.gen_bool(0.2) {
            snapshots.push(replicas[rng.gen_range(0..replica_count)].clone());
        }
    }
    History { replicas, snapshots, graph, markers }
}

fn merged(left: &ReplicaState, right: &ReplicaState) -> ReplicaState {
    let mut result = left.clone();
    result.merge(right);
    result
}

/// Every replica merges every other's final state (twice round, so each
/// has seen everything), and all must end up equal.
fn converge(replicas: &mut [ReplicaState]) {
    for _ in 0..2 {
        for target in 0..replicas.len() {
            for source in 0..replicas.len() {
                let other = replicas[source].clone();
                replicas[target].merge(&other);
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn merge_is_commutative_associative_and_idempotent(seed in any::<u64>(), steps in 5usize..60) {
        let history = run(seed, 3, steps);
        let states: Vec<&ReplicaState> = history.replicas.iter().chain(history.snapshots.iter()).collect();
        for a in &states {
            prop_assert_eq!(merged(a, a), (*a).clone());
            for b in &states {
                prop_assert_eq!(merged(a, b), merged(b, a));
                for c in states.iter().take(4) {
                    prop_assert_eq!(merged(&merged(a, b), c), merged(a, &merged(b, c)));
                }
            }
        }
    }

    #[test]
    fn replicas_that_merge_everything_agree_with_the_reference_graph(seed in any::<u64>(), replica_count in 2usize..5, steps in 5usize..80) {
        let mut history = run(seed, replica_count, steps);
        converge(&mut history.replicas);
        for replica in &history.replicas[1..] {
            prop_assert_eq!(replica, &history.replicas[0]);
        }
        let state = &history.replicas[0];

        let mut keys: BTreeSet<FieldKey> = state.fields().keys().cloned().collect();
        for entity in ENTITIES {
            for field in FIELDS {
                keys.insert(FieldKey { entity_type: EntityType::Task, entity_id: entity.to_string(), field: field.to_string() });
            }
        }
        for key in keys {
            let expected: BTreeSet<OperationId> = history
                .graph
                .frontier(key.entity_type, &key.entity_id, &key.field)
                .into_iter()
                .filter(|id| !history.markers.contains(id))
                .collect();
            let actual: BTreeSet<OperationId> = state.values(&key).iter().map(|value| operation_id(value.dot, &key)).collect();
            prop_assert_eq!(&actual, &expected, "field {:?}", key);
            if let (Some(resolution), false) = (state.resolve(&key), expected.is_empty()) {
                let graph_winner = history
                    .graph
                    .frontier(key.entity_type, &key.entity_id, &key.field)
                    .into_iter()
                    .filter(|id| !history.markers.contains(id))
                    .map(|id| history.graph.operation(&id).unwrap().clone())
                    .max_by_key(|operation| operation.stamp)
                    .unwrap();
                prop_assert_eq!(operation_id(resolution.winner.dot, &key), graph_winner.operation_id);
            }
        }
    }
}
