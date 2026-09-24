# Replicated sync: state-based design

**Status:** Implemented in 0.30.0 (sync protocol version 3)
**Date:** 2026-09-23
**Scope:** `crates/sync-core`, `crates/sync-envelope`, `src-tauri/src/sync_state.rs`,
`src-tauri/src/replicated_sync.rs`, `src-tauri/src/enrollment.rs`

This replaces the event-log design, protocol versions 1 and 2, and the
compaction plan that went with it. In that design each device published an
append-only event log. Readers walked it, and it would have needed
snapshots, acks, a 30-day offline horizon, pruning and tombstone expiry to
stay bounded. The synchronized data is small (tasks, snippets, Split Inboxes,
preferences, account metadata), so a log was more machinery than the data
needed. Enrollment, keys, connectors and the envelope format carry over
unchanged.

## Model

Each device's synchronized data is one mergeable state:
`threestrands_sync_core::ReplicaState`. It is an observed-remove map from
fields to multi-value registers, with a causal context.

- **Writes:** each write takes a *dot*, `(device, counter)`, from the device's
  next counter. The write replaces the values of every field it writes, and
  the fields in one write share the dot.
- **Context:** a version vector giving, for each device, the highest counter
  this replica has seen. Devices only ever exchange whole states, so what a
  replica has seen from a device is always the contiguous range
  `1..=counter`.
- **Merge:** a value survives if both sides hold it, or if the side that lacks
  it has never seen its dot. A value that one side lacks but *has* seen was
  replaced or deleted there, so it is dropped. Contexts merge by pointwise
  maximum.
- **Delete:** removes the entity's values. No tombstone is kept; the context
  stops any older copy bringing them back.
- **Conflicts:**
  - A field with more than one surviving value is in conflict.
  - The working value is the one with the greatest
    `(lamport, device, counter)`, the rest stay listed for review, and
    resolving a conflict is an ordinary write.
  - When a device merges, its Lamport time moves past every merged value, so
    its next write outranks what it has seen.
- **An edit racing a delete** keeps the entity deleted, as in the event-log
  design. The edited field survives, but `_entity` doesn't, so the entity
  doesn't materialize.

Merge is commutative, associative and idempotent. For any history it leaves
exactly the values the reference `OperationGraph` leaves, where each write
names the values it replaced as its parents. The graph is kept in `sync-core`
as that reference.

## Wire format (protocol 3)

- **Replica snapshot** (`ReplicaSnapshot`, sealed as `ObjectKind::Snapshot`):
  - Contents: the author, a `state_sequence` that increases with every
    snapshot it publishes, the context, and every field's values.
  - Canonical order is enforced on sealing and checked on opening, so a
    state has exactly one encoding.
  - Every value must sit inside the context.
  - Limits, in `sync-envelope/src/limits.rs`: `MAX_SNAPSHOT_FIELDS`,
    `MAX_VALUES_PER_FIELD`, `MAX_VALUE_BYTES` and the existing message-size
    limits.
  - The message id is derived from `(author, state_sequence)`, and opening
    checks it.
- **Signed head v3:** the author, its key epoch, `state_sequence`, the
  snapshot's chunk-index CID, and the publication time. The signature domain
  is `…/device-head-signature/v3`, and v1 and v2 heads are refused.
- **Sealed objects:** each object kind has its own signature domain, and
  opening checks the header's kind.
- **Protocol marker:** a well-known object reading `threestrands-sync
  protocol 3`. A connector holding a group without it is reported as
  `Legacy` and refused.
- **Epoch keyring:** grants and invitations carry `earlier_epoch_keys`, which
  predates this design and is unchanged.

## Engine

### Local store (`sync_state.rs`, schema v36)

- **Tables:** `sync_values` holds one row per surviving value, `sync_context`
  holds the version vector, and `sync_local_state` records whether the
  replica changed since the last snapshot and which snapshot sequence and
  epoch that was.
- **Writes and deletions** go straight to SQL, through
  `record_replicated_write` and `record_replicated_deletion`.
- **Merging** loads the state, merges and writes back the changed fields in
  one transaction, so a concurrent local write can't be lost. It returns the
  touched entities, which are then materialized to app tables through the
  existing projection path.
- **Taking a snapshot** clears the "changed" flag in the same transaction, so
  a write made while sealing reseals next time.

### Push (`push_local_state`)

1. **Seal** a new snapshot when the replica changed, or when it was sealed
   under an older epoch. Rotation therefore re-encrypts everything under the
   new key.
2. **Deliver** every pending object to every transport.
3. **Publish each transport's head.** It names the current snapshot only once
   that transport has every one of its objects. Until then the transport keeps
   its previous head, whose snapshot is still stored there.
4. **Delete the old snapshot** from a transport once that transport's head
   names a newer one (`sync_retired_objects`).
5. **Heartbeat:** a head whose content hasn't changed is republished only
   every `HEAD_HEARTBEAT_MS`.

### Pull (`pull_from_transports`)

- **Which heads:** for every trusted, active peer's verified head, fetch the
  snapshot only when its `state_sequence` is newer than the last one merged
  from that peer.
- **Checks before merging:**
  - every object's bytes are checked against their CID;
  - the snapshot is opened with the key for the epoch in its header;
  - it must be the head's own author and sequence.
- **After merging:** materialize, and record the merged sequence.
- **Idempotent:** merging the same snapshot twice changes nothing.
- **Relaying:** a device's own snapshot includes what it merged, so a device
  with two connectors relays data between them.

### Key catch-up (`share_keys_with_lagging_peers`, `apply_key_share`)

After pulling, a member sends its current and earlier epoch keys to any
trusted, active peer whose head is on an older epoch. It sends at most one
such share per peer per epoch, as a grant sealed to the peer. An enrolled
device applies a share from a trusted, active member without confirmation,
as it would a rotation. A share from a member it hasn't heard of yet is
retried, and one from a revoked member is ignored.

This closes two holes:
- a rotation made before its initiator heard of a new member;
- a recovery-phrase join that raced a rotation still on its way to the
  connector.

## Trust model

Every member is trusted with the group's data. A snapshot includes values
written by other devices, so a member can forge or drop other devices'
values. A member could also inflate the context to hide other devices'
future writes. None of this lets a non-member read or write anything:
snapshots are sealed with the group's key and signed by a trusted member.
The previous design had the same trust model; it just exposed less.

## Tests

- **`sync-core`:**
  - unit tests for each rule above;
  - proptest properties over random multi-replica histories with writes,
    deletes and partial merges;
  - the properties: merge is commutative, associative and idempotent, and
    converged replicas match the reference graph exactly, including the
    winning value.
- **`sync-envelope`:**
  - snapshot round-trips (single and multi-chunk, in any chunk order);
  - canonical encoding;
  - contract refusals, and every limit below, at and above its boundary;
  - the fails-closed suite, now on snapshots;
  - golden v3 snapshot and head vectors;
  - v1 and v2 heads refused.
- **`src-tauri`:**
  - sealing, delivery and heads, including a head waiting for its snapshot
    and old snapshots being deleted afterwards;
  - two-device convergence, and conflicts plus their resolution;
  - deletions, including an old copy not resurrecting a deleted item;
  - a device away for months, and relaying between connectors;
  - per-epoch keys, and a mismatched head or snapshot;
  - stale heads, and duplicate connectors;
  - never reusing a write counter after a restore from backup;
  - the heartbeat;
  - key catch-up, plus refusing shares from revoked or unknown members;
  - migration from protocol 2 groups.
- **Simulation (`sync_sim.rs`):**
  - **Schedules:** seeded runs of up to five devices, using the app's own
    snippet mutations, with outages, delayed objects, reordered scans,
    devices away for days or months, joins and key rotations.
  - **Oracle:** the harness records each write and deletion with the values
    it replaced, and feeds that history to `OperationGraph`. It never reads
    the replica code's own results to build the reference.
  - **Checked at the end:** every device holds exactly the surviving values,
    shows the winning snippet rows, and has an identical replica.
  - **Runs:** 24 seeds by default, `THREESTRANDS_SYNC_SIM_SEEDS` to run more,
    and `THREESTRANDS_SYNC_SIM_SEED` for one seed. Seeds that found bugs are
    pinned.
  - **Mutation check:** with merge deliberately broken (dropped values kept),
    both the harness and the unit tests fail.

## History

### Bugs fixed along the way

- **0.28.5:** history did not survive a key rotation. Grants and invitations
  sealed only the current key.
- **0.29.0**, found by the harness:
  - rotations from members not yet known were dropped;
  - control objects were never re-delivered;
  - a recovery join could miss a rotation for good.
- **0.30.0:** a member a rotation was never sealed to had no way to get that
  key, now handled by key catch-up.

### What the event-log design needed that this one doesn't

These no longer apply:
- chain walks and causal vectors;
- progress fixpoints and acks;
- snapshots-plus-events bootstrapping;
- the 30-day offline horizon and re-bootstrap;
- event pruning and tombstone expiry.

A connector holds about one snapshot per device, plus control objects.

## Known gaps and open questions

- **Concurrent rotations** by two devices both claim the next epoch number,
  and each device keeps one key per epoch. The harness avoids this case. It
  needs its own fix, for example a deterministic tie-break plus a follow-up
  rotation.
- **Whole-state republishing:** every change reseals and re-uploads the whole
  snapshot. That is fine for kilobytes to a few megabytes. A much larger
  synchronized data set, such as drafts, would need snapshots split by entity
  type, or sent as deltas.
- **Snapshot size:** one message holds at most 32 MB decompressed (4 MB
  compressed) and `MAX_SNAPSHOT_FIELDS` fields.
- **Control objects accumulate:** enrollment requests, grants, rotations and
  key shares are never pruned. They are small and infrequent. Pruning
  resolved requests and old shares is a possible follow-up.
- **Enrollment sweep:** lists the connector's objects every cycle. With about
  one snapshot per device that list stays short, but a dedicated
  control-object prefix would scale better.
- **Deleted data at the provider:** deleted data leaves the snapshots, but
  provider version history (S3 versioning, cloud-drive trash, other IPFS
  pins) keeps old copies until it expires.
- **Separate groups on one connector:** each group's devices refetch the
  other group's rotations every sweep. This is harmless but wasteful, and the
  setup is discouraged.
