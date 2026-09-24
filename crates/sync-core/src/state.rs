//! State-based replication: each device's whole replica is one mergeable
//! value, so devices exchange their latest state instead of an event log.
//!
//! A [`ReplicaState`] is an observed-remove map from fields to multi-value
//! registers, with a causal context:
//!
//! - Every local write mints a **dot** `(device, counter)` from the writing
//!   device's next counter and replaces the field's values with the one new
//!   value. Writing several fields at once shares one dot.
//! - The **context** is a version vector: for each device, the highest
//!   counter this replica has seen. Because devices only ever exchange whole
//!   states, what a replica has seen from a device is always a contiguous
//!   `1..=counter` range, so a version vector describes it exactly.
//! - **Merging** keeps a value when both sides hold it, or when the side
//!   that lacks it has never seen its dot. A value one side lacks *but has
//!   seen* was overwritten or deleted there, so it's dropped. Contexts merge
//!   by pointwise maximum.
//! - **Deleting** an entity simply removes its values. No tombstone is
//!   needed: every replica that later merges this one sees those dots in
//!   its context and drops them too.
//!
//! Merge is commutative, associative, and idempotent, and gives the same
//! surviving values as the operation graph in [`crate::OperationGraph`]
//! would for the same history, with each write naming the values it
//! replaced as parents. That graph is kept as the reference model the
//! property tests check this against.
//!
//! A field with more than one surviving value holds a conflict. The
//! working value is the one with the greatest `(lamport, device, counter)`;
//! the rest stay visible for review, and resolving is just another write.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::{DeviceId, EntityType};

/// One write's identity: the writing device and its counter at the time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Dot {
    pub device_id: DeviceId,
    pub counter: u64,
}

/// One surviving value of a field.
#[derive(Clone, Debug, PartialEq)]
pub struct StateValue {
    pub dot: Dot,
    /// The writing device's Lamport clock at the write, for choosing a
    /// working value among concurrent ones. Never used for causality.
    pub lamport: u64,
    pub value: Option<Value>,
}

impl StateValue {
    /// The deterministic ordering used to pick a field's working value.
    pub fn rank(&self) -> (u64, DeviceId, u64) {
        (self.lamport, self.dot.device_id, self.dot.counter)
    }
}

/// Which field of which entity.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FieldKey {
    pub entity_type: EntityType,
    pub entity_id: String,
    pub field: String,
}

/// A field's surviving values: the working value, and every other one still
/// in conflict with it.
pub struct Resolution<'a> {
    pub winner: &'a StateValue,
    pub conflicting: Vec<&'a StateValue>,
}

/// Why a state from elsewhere was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateError {
    /// A value's dot isn't covered by the state's own context, so the state
    /// claims to hold a write it says it has never seen.
    ValueOutsideContext,
    /// A field lists the same dot twice.
    DuplicateDot,
    /// A field is listed with no values.
    EmptyField,
    /// The same field is listed twice.
    DuplicateField,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReplicaState {
    context: BTreeMap<DeviceId, u64>,
    fields: BTreeMap<FieldKey, Vec<StateValue>>,
}

impl ReplicaState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuilds a state from its stored or received parts, checking that it
    /// is well formed: every value inside the context, no field empty, no
    /// dot twice in one field.
    pub fn from_parts(
        context: BTreeMap<DeviceId, u64>,
        fields: impl IntoIterator<Item = (FieldKey, Vec<StateValue>)>,
    ) -> Result<Self, StateError> {
        let mut state = ReplicaState { context, fields: BTreeMap::new() };
        for (key, mut values) in fields {
            if values.is_empty() {
                return Err(StateError::EmptyField);
            }
            values.sort_by_key(|value| value.dot);
            if values.windows(2).any(|pair| pair[0].dot == pair[1].dot) {
                return Err(StateError::DuplicateDot);
            }
            if values.iter().any(|value| !state.has_seen(value.dot)) {
                return Err(StateError::ValueOutsideContext);
            }
            if state.fields.insert(key, values).is_some() {
                return Err(StateError::DuplicateField);
            }
        }
        Ok(state)
    }

    pub fn context(&self) -> &BTreeMap<DeviceId, u64> {
        &self.context
    }

    pub fn fields(&self) -> &BTreeMap<FieldKey, Vec<StateValue>> {
        &self.fields
    }

    pub fn values(&self, key: &FieldKey) -> &[StateValue] {
        self.fields.get(key).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Whether this replica has seen `dot` (whether or not it still holds
    /// its value).
    pub fn has_seen(&self, dot: Dot) -> bool {
        dot.counter <= self.context.get(&dot.device_id).copied().unwrap_or(0)
    }

    /// Writes `values` to fields of one entity as a single new write by
    /// `device_id`, replacing whatever each field held. Returns the dot.
    pub fn write(
        &mut self,
        device_id: DeviceId,
        lamport: u64,
        entity_type: EntityType,
        entity_id: &str,
        values: impl IntoIterator<Item = (String, Option<Value>)>,
    ) -> Dot {
        let counter = self.context.entry(device_id).or_insert(0);
        *counter += 1;
        let dot = Dot { device_id, counter: *counter };
        for (field, value) in values {
            let key = FieldKey { entity_type, entity_id: entity_id.to_string(), field };
            self.fields.insert(key, vec![StateValue { dot, lamport, value }]);
        }
        dot
    }

    /// Removes every value of one entity. Returns the removed fields.
    pub fn remove_entity(&mut self, entity_type: EntityType, entity_id: &str) -> Vec<FieldKey> {
        let keys: Vec<FieldKey> = self
            .fields
            .keys()
            .filter(|key| key.entity_type == entity_type && key.entity_id == entity_id)
            .cloned()
            .collect();
        for key in &keys {
            self.fields.remove(key);
        }
        keys
    }

    /// Merges `other` into this replica. Returns every field whose values
    /// changed here.
    pub fn merge(&mut self, other: &ReplicaState) -> BTreeSet<FieldKey> {
        let keys: BTreeSet<FieldKey> = self.fields.keys().chain(other.fields.keys()).cloned().collect();
        let mut changed = BTreeSet::new();
        for key in keys {
            let mine = self.values(&key);
            let theirs = other.values(&key);
            let mut merged: Vec<StateValue> = Vec::with_capacity(mine.len().max(theirs.len()));
            for value in mine {
                match theirs.iter().find(|candidate| candidate.dot == value.dot) {
                    // Both sides hold it. The same dot is the same write, so
                    // the values agree; if a corrupt peer disagrees, keep one
                    // deterministically so merge stays commutative.
                    Some(their_value) => merged.push(pick_consistently(value, their_value).clone()),
                    None if !other.has_seen(value.dot) => merged.push(value.clone()),
                    None => {}
                }
            }
            for value in theirs {
                if !mine.iter().any(|candidate| candidate.dot == value.dot) && !self.has_seen(value.dot) {
                    merged.push(value.clone());
                }
            }
            merged.sort_by_key(|value| value.dot);
            if merged != mine {
                changed.insert(key.clone());
            }
            if merged.is_empty() {
                self.fields.remove(&key);
            } else {
                self.fields.insert(key, merged);
            }
        }
        for (device_id, counter) in &other.context {
            let seen = self.context.entry(*device_id).or_insert(0);
            *seen = (*seen).max(*counter);
        }
        changed
    }

    /// The working value of a field and any values still in conflict with
    /// it, or `None` if the field holds nothing.
    pub fn resolve(&self, key: &FieldKey) -> Option<Resolution<'_>> {
        let mut values: Vec<&StateValue> = self.values(key).iter().collect();
        values.sort_by_key(|value| value.rank());
        let winner = values.pop()?;
        Some(Resolution { winner, conflicting: values })
    }
}

fn pick_consistently<'a>(left: &'a StateValue, right: &'a StateValue) -> &'a StateValue {
    let key = |value: &StateValue| (value.lamport, value.value.as_ref().map(Value::to_string));
    if key(left) >= key(right) {
        left
    } else {
        right
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const A: DeviceId = [1u8; 16];
    const B: DeviceId = [2u8; 16];

    fn key(field: &str) -> FieldKey {
        FieldKey { entity_type: EntityType::Task, entity_id: "task-1".to_string(), field: field.to_string() }
    }

    fn write(state: &mut ReplicaState, device: DeviceId, lamport: u64, field: &str, value: Value) -> Dot {
        state.write(device, lamport, EntityType::Task, "task-1", [(field.to_string(), Some(value))])
    }

    fn working(state: &ReplicaState, field: &str) -> Option<Value> {
        state.resolve(&key(field)).and_then(|resolution| resolution.winner.value.clone())
    }

    #[test]
    fn a_later_write_replaces_an_earlier_one_everywhere() {
        let mut a = ReplicaState::new();
        write(&mut a, A, 1, "title", json!("first"));
        let mut b = a.clone();
        write(&mut b, B, 2, "title", json!("second"));
        a.merge(&b);
        assert_eq!(working(&a, "title"), Some(json!("second")));
        assert_eq!(a.values(&key("title")).len(), 1);
    }

    #[test]
    fn concurrent_writes_keep_both_with_a_deterministic_winner() {
        let mut a = ReplicaState::new();
        let mut b = ReplicaState::new();
        write(&mut a, A, 1, "title", json!("from a"));
        write(&mut b, B, 1, "title", json!("from b"));
        let mut left = a.clone();
        left.merge(&b);
        let mut right = b.clone();
        right.merge(&a);
        assert_eq!(left, right);
        let resolution = left.resolve(&key("title")).unwrap();
        assert_eq!(resolution.conflicting.len(), 1);
        assert_eq!(resolution.winner.value, Some(json!("from b")));
    }

    #[test]
    fn resolving_a_conflict_is_a_write_that_supersedes_both() {
        let mut a = ReplicaState::new();
        let mut b = ReplicaState::new();
        write(&mut a, A, 1, "title", json!("from a"));
        write(&mut b, B, 1, "title", json!("from b"));
        a.merge(&b);
        write(&mut a, A, 2, "title", json!("chosen"));
        b.merge(&a);
        assert_eq!(b.values(&key("title")).len(), 1);
        assert_eq!(working(&b, "title"), Some(json!("chosen")));
    }

    #[test]
    fn a_deletion_leaves_nothing_behind_and_is_never_undone_by_an_old_copy() {
        let mut a = ReplicaState::new();
        write(&mut a, A, 1, "title", json!("doomed"));
        let stale = a.clone();
        a.remove_entity(EntityType::Task, "task-1");
        assert!(a.fields().is_empty());
        a.merge(&stale);
        assert!(a.fields().is_empty(), "an old copy must not bring a deleted entity back");
        let mut late = stale.clone();
        late.merge(&a);
        assert!(late.fields().is_empty());
    }

    #[test]
    fn an_edit_the_deleting_device_never_saw_survives_the_deletion() {
        let mut a = ReplicaState::new();
        write(&mut a, A, 1, "title", json!("shared"));
        let mut b = a.clone();
        a.remove_entity(EntityType::Task, "task-1");
        write(&mut b, B, 2, "notes", json!("concurrent note"));
        a.merge(&b);
        assert_eq!(working(&a, "notes"), Some(json!("concurrent note")));
        assert_eq!(working(&a, "title"), None);
    }

    #[test]
    fn merging_twice_or_with_itself_changes_nothing() {
        let mut a = ReplicaState::new();
        write(&mut a, A, 1, "title", json!("x"));
        let mut b = ReplicaState::new();
        write(&mut b, B, 1, "title", json!("y"));
        a.merge(&b);
        let once = a.clone();
        assert!(a.merge(&b).is_empty());
        assert!(a.merge(&once).is_empty());
        assert_eq!(a, once);
    }

    #[test]
    fn a_device_never_reuses_a_dot_it_already_published() {
        // A device restored from an old backup learns its own higher counter
        // back from any peer, so its next write can't collide with an old one.
        let mut original = ReplicaState::new();
        write(&mut original, A, 1, "title", json!("one"));
        write(&mut original, A, 2, "title", json!("two"));
        let mut restored = ReplicaState::new();
        restored.merge(&original);
        let dot = write(&mut restored, A, 3, "title", json!("three"));
        assert_eq!(dot.counter, 3);
    }

    #[test]
    fn a_malformed_state_is_refused() {
        let value = |counter| StateValue { dot: Dot { device_id: A, counter }, lamport: 1, value: Some(json!(1)) };
        let context = BTreeMap::from([(A, 1)]);
        assert_eq!(
            ReplicaState::from_parts(context.clone(), [(key("title"), vec![value(2)])]),
            Err(StateError::ValueOutsideContext)
        );
        assert_eq!(
            ReplicaState::from_parts(context.clone(), [(key("title"), vec![value(1), value(1)])]),
            Err(StateError::DuplicateDot)
        );
        assert_eq!(ReplicaState::from_parts(context.clone(), [(key("title"), vec![])]), Err(StateError::EmptyField));
        assert!(ReplicaState::from_parts(context, [(key("title"), vec![value(1)])]).is_ok());
    }
}
