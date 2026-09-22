//! `sync-core`: the pure operation graph / multi-value register at the
//! heart of ThreeStrands replicated sync.
//!
//! This crate has no knowledge of envelopes, transports, or SQLite. It
//! implements a single idea: for one field on one entity, every applied
//! operation names its parents as the field's current frontier (the set of
//! operations not yet consumed by a known child). Applying an operation is
//! commutative, associative, and idempotent, so the same set of operations
//! converges to the same frontier no matter what order, how many times, or
//! with what gaps they arrive in.
//!
//! - A frontier of exactly one operation is conflict-free: that operation's
//!   value is the field's value.
//! - A frontier of more than one operation is a concurrent write. Pick the
//!   greatest [`WinnerStamp`] as the working value and keep every other
//!   frontier member around for conflict review via [`FieldResolution`].
//! - Resolving a conflict is just an ordinary new operation whose `parents`
//!   is the entire current frontier, applied like any other operation.
//!
//! Entity existence is modeled as an ordinary field named
//! [`ENTITY_EXISTENCE_FIELD`], not as special casing in this crate: creation
//! writes `true`, deletion writes `false`, and a concurrent edit/delete race
//! is simply a frontier of more than one on that field like any other.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

pub use threestrands_sync_protocol::EntityType;

pub type OperationId = [u8; 16];
pub type EventId = [u8; 16];
pub type DeviceId = [u8; 16];

/// The reserved field name recording whether an entity currently exists.
/// Creation writes `_entity = true` alongside every required field;
/// deletion writes `_entity = false`. Ordinary field edits never implicitly
/// resurrect a deleted entity — that is a property of how a caller chooses
/// to construct operations, not of this graph.
pub const ENTITY_EXISTENCE_FIELD: &str = "_entity";

/// The deterministic tie-breaker for concurrent writes to the same field:
/// `(lamport, device_id, event_id, operation_id)`. Field declaration order
/// is comparison priority order (derived `Ord` compares fields in
/// declaration order), so the greatest stamp is always well-defined and
/// never depends on wall-clock time.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct WinnerStamp {
    pub lamport: u64,
    pub device_id: DeviceId,
    pub event_id: EventId,
    pub operation_id: OperationId,
}

/// One field-level operation as understood by the graph: enough to place it
/// in the graph, and enough to be a candidate winning value.
#[derive(Clone, Debug)]
pub struct Operation {
    pub operation_id: OperationId,
    pub entity_type: EntityType,
    pub entity_id: String,
    pub field: String,
    pub value: Option<Value>,
    pub parents: Vec<OperationId>,
    pub stamp: WinnerStamp,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ApplyOutcome {
    Applied,
    /// This operation id was already known; the graph is unchanged.
    Duplicate,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct FieldKey {
    entity_type: EntityType,
    entity_id: String,
    field: String,
}

#[derive(Default)]
struct FieldState {
    /// Operations for this field not yet consumed by a known child.
    frontier: HashSet<OperationId>,
    /// Every operation id ever named as a parent for this field, whether or
    /// not it has arrived yet. An operation that arrives after something
    /// already named it as a parent must not re-enter the frontier.
    consumed: HashSet<OperationId>,
}

/// The operation graph for however many entities and fields a caller feeds
/// it. Callers typically keep one graph per sync space.
#[derive(Default)]
pub struct OperationGraph {
    operations: HashMap<OperationId, Operation>,
    fields: HashMap<FieldKey, FieldState>,
}

impl OperationGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies one operation. Duplicate operation ids are a no-op: this is
    /// what makes at-least-once transport delivery safe.
    ///
    /// An operation's parents are recorded even when a parent has not
    /// arrived yet (out-of-order / child-before-parent delivery), and the
    /// operation itself is kept out of the frontier if something already
    /// known named it as a parent before it arrived.
    pub fn apply(&mut self, operation: Operation) -> ApplyOutcome {
        if self.operations.contains_key(&operation.operation_id) {
            return ApplyOutcome::Duplicate;
        }

        let key = FieldKey {
            entity_type: operation.entity_type,
            entity_id: operation.entity_id.clone(),
            field: operation.field.clone(),
        };
        let state = self.fields.entry(key).or_default();

        for parent in &operation.parents {
            state.consumed.insert(*parent);
            state.frontier.remove(parent);
        }

        let operation_id = operation.operation_id;
        if !state.consumed.contains(&operation_id) {
            state.frontier.insert(operation_id);
        }

        self.operations.insert(operation_id, operation);
        ApplyOutcome::Applied
    }

    /// The current frontier for one field: operations not yet consumed by a
    /// known child. Empty if the field has never been written.
    pub fn frontier(
        &self,
        entity_type: EntityType,
        entity_id: &str,
        field: &str,
    ) -> Vec<OperationId> {
        let key = FieldKey {
            entity_type,
            entity_id: entity_id.to_string(),
            field: field.to_string(),
        };
        self.fields
            .get(&key)
            .map(|state| state.frontier.iter().copied().collect())
            .unwrap_or_default()
    }

    pub fn operation(&self, operation_id: &OperationId) -> Option<&Operation> {
        self.operations.get(operation_id)
    }

    /// Resolves a field's current value: the frontier member with the
    /// greatest [`WinnerStamp`], plus every other frontier member for
    /// conflict review. `None` if the field has never been written.
    ///
    /// A caller resolving the conflict emits a new ordinary operation whose
    /// `parents` is exactly [`Self::frontier`] for this field, applied like
    /// any other operation.
    pub fn resolve_field(
        &self,
        entity_type: EntityType,
        entity_id: &str,
        field: &str,
    ) -> Option<FieldResolution<'_>> {
        let frontier = self.frontier(entity_type, entity_id, field);
        if frontier.is_empty() {
            return None;
        }
        let mut candidates: Vec<&Operation> = frontier
            .iter()
            .map(|id| {
                self.operations
                    .get(id)
                    .expect("a frontier id always names a known operation")
            })
            .collect();
        candidates.sort_by_key(|op| op.stamp);
        let winner = candidates.pop().expect("frontier is non-empty");
        Some(FieldResolution {
            winner,
            conflicting: candidates,
        })
    }

    /// Convenience over [`Self::resolve_field`] for the reserved
    /// [`ENTITY_EXISTENCE_FIELD`], interpreting the winning value as a bool.
    /// `None` if existence has never been recorded for this entity.
    pub fn entity_exists(&self, entity_type: EntityType, entity_id: &str) -> Option<bool> {
        self.resolve_field(entity_type, entity_id, ENTITY_EXISTENCE_FIELD)?
            .winner
            .value
            .as_ref()
            .and_then(Value::as_bool)
    }
}

/// The result of resolving a field: the deterministic winner, plus every
/// other value still in the frontier (empty when the frontier had exactly
/// one member, i.e. the field is currently conflict-free).
pub struct FieldResolution<'a> {
    pub winner: &'a Operation,
    pub conflicting: Vec<&'a Operation>,
}

impl FieldResolution<'_> {
    pub fn is_conflict(&self) -> bool {
        !self.conflicting.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn id(byte: u8) -> OperationId {
        [byte; 16]
    }

    fn op(operation_id: u8, lamport: u64, parents: &[u8], value: i64) -> Operation {
        Operation {
            operation_id: id(operation_id),
            entity_type: EntityType::Task,
            entity_id: "task-1".to_string(),
            field: "title".to_string(),
            value: Some(json!(value)),
            parents: parents.iter().map(|b| id(*b)).collect(),
            stamp: WinnerStamp {
                lamport,
                device_id: [0u8; 16],
                event_id: [0u8; 16],
                operation_id: id(operation_id),
            },
        }
    }

    #[test]
    fn a_lone_write_is_conflict_free() {
        let mut graph = OperationGraph::new();
        graph.apply(op(1, 1, &[], 10));
        let resolution = graph
            .resolve_field(EntityType::Task, "task-1", "title")
            .unwrap();
        assert!(!resolution.is_conflict());
        assert_eq!(resolution.winner.value, Some(json!(10)));
    }

    #[test]
    fn a_second_write_consumes_the_first() {
        let mut graph = OperationGraph::new();
        graph.apply(op(1, 1, &[], 10));
        graph.apply(op(2, 2, &[1], 20));
        let frontier = graph.frontier(EntityType::Task, "task-1", "title");
        assert_eq!(frontier, vec![id(2)]);
    }

    #[test]
    fn concurrent_writes_are_a_conflict_with_a_deterministic_winner() {
        let mut graph = OperationGraph::new();
        graph.apply(op(1, 1, &[], 10));
        graph.apply(op(2, 2, &[1], 20));
        graph.apply(op(3, 2, &[1], 30)); // same lamport as op 2: concurrent

        let resolution = graph
            .resolve_field(EntityType::Task, "task-1", "title")
            .unwrap();
        assert!(resolution.is_conflict());
        // Winner is deterministic: greatest (lamport, device_id, event_id, operation_id).
        assert_eq!(resolution.winner.operation_id, id(3));
    }

    #[test]
    fn a_child_arriving_before_its_parent_never_lets_the_parent_join_the_frontier() {
        let mut graph = OperationGraph::new();
        // Child names an unseen parent.
        graph.apply(op(2, 2, &[1], 20));
        assert_eq!(
            graph.frontier(EntityType::Task, "task-1", "title"),
            vec![id(2)]
        );
        // The parent arrives late and must not re-enter the frontier.
        graph.apply(op(1, 1, &[], 10));
        assert_eq!(
            graph.frontier(EntityType::Task, "task-1", "title"),
            vec![id(2)]
        );
    }

    #[test]
    fn duplicate_application_is_a_no_op() {
        let mut graph = OperationGraph::new();
        assert_eq!(graph.apply(op(1, 1, &[], 10)), ApplyOutcome::Applied);
        assert_eq!(graph.apply(op(1, 1, &[], 10)), ApplyOutcome::Duplicate);
        assert_eq!(
            graph.frontier(EntityType::Task, "task-1", "title"),
            vec![id(1)]
        );
    }

    #[test]
    fn conflict_resolution_is_an_ordinary_operation_over_the_frontier() {
        let mut graph = OperationGraph::new();
        graph.apply(op(1, 1, &[], 10));
        graph.apply(op(2, 2, &[1], 20));
        graph.apply(op(3, 2, &[1], 30));

        let frontier = graph.frontier(EntityType::Task, "task-1", "title");
        assert_eq!(frontier.len(), 2);
        let resolution_op = Operation {
            operation_id: id(4),
            parents: frontier,
            stamp: WinnerStamp {
                lamport: 3,
                device_id: [0u8; 16],
                event_id: [0u8; 16],
                operation_id: id(4),
            },
            ..op(4, 3, &[], 99)
        };
        graph.apply(resolution_op);
        let resolution = graph
            .resolve_field(EntityType::Task, "task-1", "title")
            .unwrap();
        assert!(!resolution.is_conflict());
        assert_eq!(resolution.winner.operation_id, id(4));
    }

    #[test]
    fn entity_existence_is_an_ordinary_field() {
        let mut graph = OperationGraph::new();
        let mut create = op(1, 1, &[], 0);
        create.field = ENTITY_EXISTENCE_FIELD.to_string();
        create.value = Some(json!(true));
        graph.apply(create);
        assert_eq!(graph.entity_exists(EntityType::Task, "task-1"), Some(true));

        let mut delete = op(2, 2, &[1], 0);
        delete.field = ENTITY_EXISTENCE_FIELD.to_string();
        delete.value = Some(json!(false));
        graph.apply(delete);
        assert_eq!(graph.entity_exists(EntityType::Task, "task-1"), Some(false));
    }
}
