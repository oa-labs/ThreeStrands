//! Randomized property tests for the operation graph, as required by Phase
//! 1 of the replicated-sync plan: permutation, duplication,
//! omission/reinsertion, and snapshot-replay all converge to the same
//! frontier and the same winning value.
//!
//! Each test takes a small seed and shape from `proptest` and expands it,
//! with a deterministic `rand::rngs::StdRng`, into a randomized DAG of
//! operations that mimics real usage: every operation's parents are a
//! (possibly proper) subset of the field's frontier at the moment it was
//! created, and siblings are sometimes created from the same frontier
//! snapshot to model genuine concurrent writers.

use std::collections::HashSet;

use proptest::prelude::*;
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::{RngExt, SeedableRng};
use serde_json::json;
use threestrands_sync_core::{
    ApplyOutcome, EntityType, Operation, OperationGraph, OperationId, WinnerStamp,
};

const ENTITY_ID: &str = "task-1";
const FIELD: &str = "title";

fn random_operation_id(rng: &mut StdRng) -> OperationId {
    let mut bytes = [0u8; 16];
    rng.fill(&mut bytes);
    bytes
}

/// Generates a causally valid, sometimes-concurrent sequence of operations
/// against one field: every operation's parents are drawn from the live
/// frontier at its creation time, and up to two siblings may be created
/// from the same frontier snapshot to simulate concurrent writers.
fn generate_operations(rng: &mut StdRng, count: usize) -> Vec<Operation> {
    let mut ops = Vec::with_capacity(count);
    let mut frontier: Vec<OperationId> = Vec::new();
    let mut lamport = 0u64;
    let mut created = 0usize;

    while created < count {
        let branch_width = if frontier.is_empty() {
            1
        } else {
            rng.random_range(1..=2)
        }
        .min(count - created);
        let snapshot = frontier.clone();
        let mut new_ids = Vec::new();

        for _ in 0..branch_width {
            lamport += 1;
            let operation_id = random_operation_id(rng);
            let parents = if snapshot.is_empty() {
                Vec::new()
            } else {
                let take = rng.random_range(1..=snapshot.len());
                let mut chosen = snapshot.clone();
                chosen.shuffle(rng);
                chosen.truncate(take);
                chosen
            };
            let device_id = [(created % 3) as u8; 16];
            let stamp = WinnerStamp {
                lamport,
                device_id,
                event_id: random_operation_id(rng),
                operation_id,
            };
            ops.push(Operation {
                operation_id,
                entity_type: EntityType::Task,
                entity_id: ENTITY_ID.to_string(),
                field: FIELD.to_string(),
                value: Some(json!(created)),
                parents,
                stamp,
            });
            new_ids.push(operation_id);
            created += 1;
        }

        for op in ops.iter().rev().take(new_ids.len()) {
            for parent in &op.parents {
                frontier.retain(|existing| existing != parent);
            }
        }
        frontier.extend(new_ids);
    }

    ops
}

fn sorted(mut ids: Vec<OperationId>) -> Vec<OperationId> {
    ids.sort();
    ids
}

fn apply_all(ops: impl IntoIterator<Item = Operation>) -> OperationGraph {
    let mut graph = OperationGraph::new();
    for op in ops {
        graph.apply(op);
    }
    graph
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn permutation_of_delivery_order_converges(seed in any::<u64>(), count in 1usize..40, shuffle_seed in any::<u64>()) {
        let mut rng = StdRng::seed_from_u64(seed);
        let ops = generate_operations(&mut rng, count);

        let baseline = apply_all(ops.clone());
        let baseline_frontier = sorted(baseline.frontier(EntityType::Task, ENTITY_ID, FIELD));
        let baseline_winner = baseline
            .resolve_field(EntityType::Task, ENTITY_ID, FIELD)
            .map(|resolution| resolution.winner.operation_id);

        let mut shuffled = ops;
        let mut shuffle_rng = StdRng::seed_from_u64(shuffle_seed);
        shuffled.shuffle(&mut shuffle_rng);
        let shuffled_graph = apply_all(shuffled);

        prop_assert_eq!(
            sorted(shuffled_graph.frontier(EntityType::Task, ENTITY_ID, FIELD)),
            baseline_frontier
        );
        prop_assert_eq!(
            shuffled_graph
                .resolve_field(EntityType::Task, ENTITY_ID, FIELD)
                .map(|resolution| resolution.winner.operation_id),
            baseline_winner
        );
    }

    #[test]
    fn duplicate_delivery_is_a_no_op(seed in any::<u64>(), count in 1usize..40, redeliver_seed in any::<u64>()) {
        let mut rng = StdRng::seed_from_u64(seed);
        let ops = generate_operations(&mut rng, count);

        let mut graph = apply_all(ops.clone());
        let before = sorted(graph.frontier(EntityType::Task, ENTITY_ID, FIELD));

        let mut redelivered: Vec<Operation> = ops.clone().into_iter().chain(ops).collect();
        let mut redeliver_rng = StdRng::seed_from_u64(redeliver_seed);
        redelivered.shuffle(&mut redeliver_rng);
        for op in redelivered {
            graph.apply(op);
        }

        prop_assert_eq!(
            sorted(graph.frontier(EntityType::Task, ENTITY_ID, FIELD)),
            before
        );
    }

    #[test]
    fn omission_and_reinsertion_still_converges(
        seed in any::<u64>(),
        count in 2usize..40,
        omit_seed in any::<u64>(),
    ) {
        let mut rng = StdRng::seed_from_u64(seed);
        let ops = generate_operations(&mut rng, count);

        let mut omit_rng = StdRng::seed_from_u64(omit_seed);
        let mut indices: Vec<usize> = (0..ops.len()).collect();
        indices.shuffle(&mut omit_rng);
        let withhold_count = omit_rng.random_range(0..ops.len());
        let withheld: HashSet<usize> = indices.into_iter().take(withhold_count).collect();

        let mut graph = OperationGraph::new();
        let mut later = Vec::new();
        for (index, op) in ops.iter().cloned().enumerate() {
            if withheld.contains(&index) {
                later.push(op);
            } else {
                assert_eq!(graph.apply(op), ApplyOutcome::Applied);
            }
        }
        later.shuffle(&mut omit_rng);
        for op in later {
            graph.apply(op);
        }

        let baseline = apply_all(ops);
        prop_assert_eq!(
            sorted(graph.frontier(EntityType::Task, ENTITY_ID, FIELD)),
            sorted(baseline.frontier(EntityType::Task, ENTITY_ID, FIELD))
        );
        prop_assert_eq!(
            graph
                .resolve_field(EntityType::Task, ENTITY_ID, FIELD)
                .map(|resolution| resolution.winner.operation_id),
            baseline
                .resolve_field(EntityType::Task, ENTITY_ID, FIELD)
                .map(|resolution| resolution.winner.operation_id)
        );
    }

    #[test]
    fn snapshot_plus_descendants_equals_full_replay(
        seed in any::<u64>(),
        count in 2usize..40,
        cut_seed in any::<u64>(),
    ) {
        let mut rng = StdRng::seed_from_u64(seed);
        let ops = generate_operations(&mut rng, count);

        let mut cut_rng = StdRng::seed_from_u64(cut_seed);
        let cut = cut_rng.random_range(1..ops.len());
        let (prefix, suffix) = ops.split_at(cut);

        // Compact the prefix into "the frontier at the cut point", standing
        // in for a snapshot object: a fresh device bootstraps from exactly
        // those operation records, not the full prefix history.
        let prefix_graph = apply_all(prefix.iter().cloned());
        let frontier_ids: HashSet<OperationId> = prefix_graph
            .frontier(EntityType::Task, ENTITY_ID, FIELD)
            .into_iter()
            .collect();
        let snapshot_ops: Vec<Operation> = prefix
            .iter()
            .filter(|op| frontier_ids.contains(&op.operation_id))
            .cloned()
            .collect();

        let mut from_snapshot = apply_all(snapshot_ops);
        for op in suffix.iter().cloned() {
            from_snapshot.apply(op);
        }

        let full_replay = apply_all(ops);

        prop_assert_eq!(
            sorted(from_snapshot.frontier(EntityType::Task, ENTITY_ID, FIELD)),
            sorted(full_replay.frontier(EntityType::Task, ENTITY_ID, FIELD))
        );
        prop_assert_eq!(
            from_snapshot
                .resolve_field(EntityType::Task, ENTITY_ID, FIELD)
                .map(|resolution| resolution.winner.operation_id),
            full_replay
                .resolve_field(EntityType::Task, ENTITY_ID, FIELD)
                .map(|resolution| resolution.winner.operation_id)
        );
    }
}
