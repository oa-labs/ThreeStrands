# Sync compaction and pruning plan

**Status:** Proposed
**Date:** 2026-09-23
**Scope:** replicated sync (`crates/sync-*`, `src-tauri/src/replicated_sync.rs`,
`src-tauri/src/enrollment.rs`, `src-tauri/src/sync_projection.rs`, transports)

## Goals

1. A new device joins by loading one snapshot plus recent events, not by
   replaying the group's whole history.
2. Local tables and connector storage stop growing without bound.
3. Deleted records, and history older than the offline horizon, are removed
   from connectors. Keys for epochs that no longer protect any stored object
   can be forgotten.

Compaction must never change what any device displays. Every test in this plan
checks that the materialized state and the set of unresolved conflicts are
identical to the state and conflicts of a run that never compacted.

## Decisions

| Decision | Choice |
|---|---|
| Offline horizon | **30 days.** A device that has not published a head for 30 days no longer holds back compaction. If it comes back after history it never received has been pruned, it re-bootstraps. |
| Existing test groups | **Not migrated.** The work ships as sync protocol v2. v1 corpora are detected and the user is asked to create a new group. This removes all dual-format code. |
| Rotation objects | **Kept indefinitely.** They are small. Returning devices and recovery-phrase joins need them to follow the trust and key chain. Only event, snapshot and enrollment-traffic objects are pruned. |
| Who compacts | **The lowest device id among active, non-dormant devices.** The safety rules below make an accidental second compactor harmless, so leader election does not need to be exact. |

The encrypted settings export does not carry any sync data (`transfer.rs` has no
sync coupling), so AGENTS.md's settings-transfer rules are unaffected.

## What the code does today

- Each device publishes a signed head (`crates/sync-envelope/src/device_head.rs`)
  holding `contiguous_sequence` and `latest_event_cid`. The head has no time
  field and no record of what the device has received from its peers.
- `pull_device_chain` (`src-tauri/src/replicated_sync.rs:1577`) starts at a
  peer's head and walks `previous_device_event` backwards. It stops when it
  reaches an object that already exists in local `sync_objects`. Once local
  objects are pruned, that stop condition no longer holds.
- `sync_operations.event_id` is `REFERENCES sync_events ON DELETE CASCADE`
  (`schema.rs:655`). Deleting an old event would also delete the operation that
  holds a field's current value.
- The device list's "last change" is `MAX(sync_events.created_at)`
  (`enrollment.rs:246`), so it depends on event rows being kept.
- `ObjectKind::Snapshot` is reserved (`crates/sync-envelope/src/header.rs:24`),
  but nothing produces or reads it.

### Fixed in 0.28.5: history did not survive a key rotation

Before 0.28.5, `pull_device_chain` opened every event with only the current
epoch key, and grants and join-code invitations sealed only that key. Every
join-code use, expiry and cancellation rotates the epoch, so a device that
joined after any rotation could not open older events. The error aborted the
whole chain walk, so even the newer, readable events were not applied.

The fix, done ahead of the rest of this plan as "Step 0":

- **Earlier keys travel with enrollment.** `EnrollmentGrant` and `Invitation`
  carry `earlier_epoch_keys`, each sealed to the same recipient as the current
  key and limited by `MAX_EARLIER_EPOCH_KEYS`. The field is left out of the
  encoding when empty, so objects without it are byte-identical to the old
  format and still verify.
- **Recovery-phrase join** opens the recovery stanza of every self-consistent
  rotation. It adopts the newest one instead of whichever the scan met first.
- **Pull** opens each event with the key for the epoch in its header
  (`LocalKeys::epoch_key`).
- **The active epoch never moves backwards** when a device meets an older
  rotation after a newer one.
- **A related bug in the chain walk is also fixed.** It used to save each
  fetched event locally before the walk finished. If a later fetch failed,
  those saved events were never applied, and every later walk stopped at them.
  The walk now saves an event only after applying it, and
  `forget_unapplied_remote_messages` removes events that older builds left
  saved but unapplied, so they are fetched again.

The Step 1 epoch-keyring item below is therefore done. Protocol v2 only needs
to carry it forward.

## Core model

**Applied.** For each device X, `applied[X]` is the contiguous prefix of X's
feed applied locally. Events apply strictly in feed order, and an arriving event
at or below `applied[X]` is dropped (invariant 4).

**Progress vector.** For each device X, `progress[X]` is the highest sequence s
such that:
- every event from X numbered 1 to s has been applied locally, and
- every event those depend on is also covered by `progress`.

Progress is computed as a fixpoint over per-event causal vectors. An event's
causal vector is its author's *applied* vector when sealing it. That vector may
be more than the event depends on, but never less, because every parent an
operation names was already in the author's graph. Along one feed causal vectors
only grow, so a prefix `1..=s` is closed exactly when event `s`'s vector is
covered. Progress is always causally closed: if an event is covered, so is
everything it depends on.

**Ack.** Each device publishes its progress vector in its signed head. That
published vector is its ack.

**Cut.** A causally closed progress vector. The compaction cut is the
component-wise minimum of the acks of every active, non-dormant device,
including the compactor's own. The intersection of closed cuts is closed, so the
cut is closed.

**Snapshot.** The graph state at a cut, signed by its author. For every field
that is still live, it records **every frontier member**, including conflict
losers, each with its original `operation_id`, value and `WinnerStamp`. Later
operations can name these ids as parents, and those operations resolve against
them normally.

**Dominance.** Snapshot S1 dominates S2 when `S1.cut >= S2.cut` in every
component. Retained snapshots are the ones referenced by active devices' heads
that no other referenced snapshot dominates.

**Floor.** The component-wise minimum of the cuts of all retained snapshots,
further limited by the minimum ack the pruning device can see. Only events at or
below the floor may be pruned. Because of this rule, every retained snapshot can
still bootstrap a device: all events above its cut are still stored.

**Dormant.** A device whose latest head is more than 30 days old. Age is judged
by two signals, the signed `published_at_ms` and the time the local device last
saw that head change. The device is dormant only if both are more than 30 days
old. A `published_at_ms` in the future is treated as now.

**Stale.** A device is stale when `progress[X] < floor[X]` for some device X,
meaning events it never applied may already be deleted. A stale device
re-bootstraps. It never tries a partial catch-up.

## Invariants

Each invariant has named tests, and later steps cite them by number.

1. **Prune floor.** An event object is deleted from a connector only if it is at
   or below the floor. Every retained snapshot must already be delivered to that
   connector before anything is deleted.
2. **Closed cuts.** A snapshot's cut is causally closed, and it never exceeds
   the author's own published progress.
3. **Conflicts survive.** Loading a snapshot and replaying later events gives
   the same frontier sets as a full replay.
4. **Late ancestors are ignored.** An arriving event whose sequence is at or
   below the local `progress` for its device is dropped without being applied.
   Otherwise a retransmitted, already-compacted ancestor would rejoin a frontier
   as a false conflict.
5. **Tombstone expiry.** An entity is left out of a snapshot only if its
   `_entity` frontier is exactly one `false` operation. When it is left out, all
   of its fields go with it. Plain field edits never set `_entity = true`, so
   leaving an entity out cannot bring it back.
6. **Trusted snapshots only.** A snapshot is used only if its author is active
   in the local roster, its signature verifies, and it is sealed at an epoch this
   device holds.
7. **Nothing local is dropped.** Re-bootstrapping keeps this device's own events
   that no peer has acknowledged. They are applied on top of the snapshot and
   then published. Any that race newer data appear in the existing conflict
   review. They are never discarded.
8. **Key retirement.** An epoch key is forgotten only when that epoch is not
   the active one, and no retained local object or retained snapshot is sealed
   at it.
9. **Pruning does not change state.** Materialized app tables and the conflict
   list are the same before and after any compaction or prune.
10. **Repair does not re-upload pruned objects.** `enqueue_repair_deliveries`
    never schedules objects at or below the floor, or dominated snapshots.

---

## Step 1 — Protocol v2 groundwork and test harness

**Status: done in 0.29.0.** Existing test groups are left on upgrade (see
below) and must be recreated. No data is deleted from connectors.

### Wire format (sync-envelope, protocol v2)

- **Epoch keyring:** done in 0.28.5 as `earlier_epoch_keys`. In Step 3, limit
  it to retained epochs.
- **Signed head v2:**
  - Adds `published_at_ms`, `ack` (the progress vector) and `snapshot_cid`,
    under the signature domain `…/device-head-signature/v2`.
  - Vectors are `SequenceEntry { device_id, sequence }` lists in ascending id
    order with no zero entries, limited by `MAX_SEQUENCE_VECTOR_ENTRIES`.
- **Events v2:** `PROTOCOL_VERSION = 2` and `causal_vector`. Sealing and opening
  refuse any other version and any vector that names its own author.
- **Generic sealing:**
  - `seal_object` / `open_object` sign `domain || message_id || body`, with a
    domain per `ObjectKind`. `open_object` also refuses a header of the wrong
    kind.
  - `seal_event` / `open_message` are wrappers. `open_message` also checks
    that the message id is the one derived from the event id.
- **Protocol marker (changed from the plan).** Instead of a per-transport
  marker file, every v2 group stores `PROTOCOL_MARKER` as an ordinary
  content-addressed object on each connector. It works unchanged on every
  transport, and it is recorded locally so repair delivers it to connectors
  added later.
  - `inspect_sync_space` reports `Legacy` when a self-consistent rotation
    exists but the marker is definitely absent. A marker that can't be
    fetched for any other reason counts as `Unknown`.
  - Genesis, recovery-phrase join, enrollment requests and join codes all
    refuse a legacy connector before any side effect.
  - A shared folder that is still syncing can look the same as a legacy
    group, so the message tells the user to wait and retry before deleting
    anything.
- **Golden vectors:** the v2 vectors are checked in. The v1 vectors are kept,
  and tests pin that they are refused.

### Local schema (version 35)

- **Leaving a v1 group.** A device that belonged to a v1 group leaves it in the
  migration, as "Leave this sync group" does. App data, connectors and the beta
  toggle stay.
  - `sync_notices` holds a one-time `protocol_reset` notice, shown in
    Settings.
  - `sync_pending_keychain_cleanup` holds the highest epoch to forget. The
    sync loop does the keychain cleanup, since a migration can't reach the
    keychain.
- **`sync_operations`** is rebuilt without the cascade. Each row has exactly
  one of `event_id` or `snapshot_id` (a `CHECK` constraint).
- **`sync_events`** gains `causal_vector`.
- **`sync_device_progress`** stores `applied_sequence`, `progress_sequence`,
  `last_event_at`, and the last head's publication time, when this device saw
  it, and its ack. The device list reads "last change" from here.
- **`sync_head_publications`** records what was last published to each
  transport, for the heartbeat.
- **`sync_snapshots` moves to Step 2.** It is local-only, so it doesn't need
  the reset.

### Engine

- **Chain walk (`pull_device_chain`):**
  - Stops at `applied[author]`, not at "object already stored".
  - Checks the feed is gapless and every event is the author's.
  - Skips this device's own head.
- **All or nothing per walk (changed from the plan).** The events walked
  before a failure are *newer* than the failed one, so applying them would
  break the applied prefix. A failed walk applies nothing, the next pull walks
  again, and an event is remembered only after it is applied.
- **Heads:**
  - Published even before any event exists.
  - Republished when content changes, or after `HEAD_HEARTBEAT_MS` (6 h, in
    `sync_policy.rs`).
  - **Per transport (added):** each transport's head names only events whose
    objects *that transport* has confirmed receiving. Before this, one failed
    upload made every reader's walk fail until the retry landed.
- **Pulls** record each peer's verified head (ack, publication time, time
  seen), then recompute progress.

### Bugs the harness found, now fixed

These were already in the code before v2; the harness exposed them.
- **A rotation seen before its initiator was known was dropped for good.** An
  offline device could scan a new member's rotation before that member's
  announcement. It is now retried: at the end of the sweep, and on later
  sweeps. A known but revoked initiator is still ignored.
- **Control objects were never re-delivered.** `publish_to_all` was
  best-effort. Rotations, grants, requests, rejections and announcements are
  now recorded as local objects, so repair re-delivers them. Before, a
  rotation published during an outage locked every other device out of the
  new epoch.
- **A recovery-phrase join could miss a rotation permanently.** If any
  rotation hadn't reached the connector yet, the device joined without that
  epoch's key and could never get it. A join now waits
  (`RECOVERY_INCOMPLETE`) when epochs have a gap, or when a member's head names
  a newer epoch than any visible rotation.

### Test harness (`src-tauri/src/sync_sim.rs`)

- **Devices:** seeded schedules drive up to five in-memory devices through the
  app's own snippet mutations, enrollment, push and pull.
- **Faults:** one fake transport with outages, delayed visibility and
  reordered scans, plus an injected clock (`sync_policy::set_test_clock`).
- **Events:** devices go offline for days or weeks, new devices join by
  recovery phrase, and the group rotates keys.
- **Checked after every sync:** progress ≤ applied, and progress is causally
  closed.
- **Checked at the end:** every device has the exact frontier (values *and*
  unresolved conflicts) of an oracle, has the oracle's snippet rows, and has
  applied every other device's whole feed, with progress equal to applied. The
  oracle is the union of all devices' operations fed through
  `threestrands_sync_core`.
- **Runs:** 24 seeds by default, with `THREESTRANDS_SYNC_SIM_SEEDS` for more
  and `THREESTRANDS_SYNC_SIM_SEED` for one. Seeds that found bugs are pinned.
- **Compaction comparison:** comparing runs with compaction on and off arrives
  with Steps 2 and 3. There is no compaction to switch yet.

---

## Step 2 — Snapshots for bootstrapping

The aim of this step is for new devices to join from a snapshot. Nothing is
deleted yet, so the risk is low.

### Snapshot object

A snapshot is one sealed `ObjectKind::Snapshot` message with a chunk index,
exactly like an event. Its body holds:

```text
SnapshotBody {
  format: 1,
  sync_space_id, author_device_id, key_epoch,
  cut: [(DeviceId, u64)],          // sorted, closed
  lamport_high: u64,               // max lamport at or below the cut
  created_at_ms: i64,              // display only
  entries: [SnapshotEntry {
    entity_type, entity_id, field,
    frontier: [{ operation_id, value, stamp: WinnerStamp }]  // every member
  }]
}
```

Add `MAX_SNAPSHOT_ENTRIES` and `MAX_FRONTIER_PER_FIELD` to `limits.rs`. The
existing 4 MB compressed / 32 MB decompressed message limits apply. If a
snapshot would exceed a limit, building it fails with a logged error and no
snapshot is written. It is never clamped silently. Splitting a snapshot into
parts is deferred until real data gets close to the limit.

### Building a snapshot

This step builds the snapshot on the device that will later compact.

1. Seed a `threestrands_sync_core::OperationGraph` from the previous snapshot,
   or from nothing.
2. Apply every stored operation from events in `(previous cut, new cut]`.
3. Emit the resulting frontiers. Using the in-memory graph means the frontier
   rules are the ones already property-tested in `crates/sync-core`.

In this step the cut is the author's own progress vector, which is already
closed. Step 3 lowers it to the minimum ack.

A snapshot is built when either of these holds:
- at least `SNAPSHOT_MIN_INTERVAL` (7 days) has passed and the cut has
  advanced, or
- `SNAPSHOT_EVENT_THRESHOLD` (2,000) events have been applied since the last
  cut.

It is also built on the first sync cycle after a rotation, so newcomers get a
snapshot at the current epoch. The snapshot is stored in `sync_snapshots`,
delivered like any object, and referenced from the author's head
`snapshot_cid`. Timing constants go in a new `src-tauri/src/sync_policy.rs`.

### Bootstrapping a device

A device bootstraps from a snapshot when it holds epoch keys and has applied no
remote events (every remote `progress` entry is 0). "Empty graph" is the wrong
test, because backlog reconciliation may already have recorded local entities.

1. Resolve heads for the roster. Collect `snapshot_cid`s from active devices,
   drop dominated snapshots, then pick the greatest by sum of the cut, breaking
   ties by author id.
2. Fetch the snapshot, verify its CID, open it with the key for its epoch, and
   verify the author (invariant 6). If anything fails, try the next candidate.
   If no candidate works, replay the full history.
3. In one transaction:
   - insert the operations with `snapshot_id` and their frontier rows;
   - set `progress = cut`;
   - raise `sync_spaces.lamport` to at least `lamport_high`.
4. Materialize every entity the snapshot touched, using the existing two-pass
   `materialize_touched_entities`.
5. Pull normally. The progress-based stop from Step 1 fetches only events above
   the cut.

### Tests for Step 2

- Snapshot round-trip:
  - tampering with the body, the cut or the author breaks the signature;
  - a snapshot signed by a revoked or unknown device is refused (invariant 6);
  - a snapshot sealed at an epoch this device lacks falls back to the next
    candidate.
- Invariant 3 across harness seeds: bootstrapping from a snapshot and replaying
  later events matches a full replay, including open conflicts. Covers at
  least:
  - a two-way field conflict, and
  - an edit racing a delete.
- A resolution written after the snapshot, naming snapshot frontier ids as
  parents, collapses the conflict on both a bootstrapped and a fully replayed
  device.
- Choice between snapshots: of two, the dominated one is ignored; of two that
  don't dominate each other, the choice is deterministic.
- Boundary tests for each new limit: below, exactly at, and above it.
- A newcomer with local entities recorded before joining still bootstraps. A
  singleton preference conflict appears the same way it does today.
- A snapshot is built after a rotation. A device that joins afterwards
  bootstraps under the new epoch.
- A newcomer fetches only events above the cut: count `get_object` calls on the
  fake transport.

**Done when:** new devices bootstrap from snapshots and every invariant 3 seed
passes. **Version:** minor, 0.28.0 → 0.29.0.

---

## Step 3 — Pruning, tombstone expiry, key retirement, re-bootstrap

The aim of this step is to delete what is safe to delete, and to recover
cleanly the devices that fall behind.

### Compaction cut and snapshot

- **Choosing the compactor.** The compactor is the lowest device id among
  active, non-dormant devices. The compactor must not itself be stale.
- **Cut.** The component-wise minimum of the acks of all active, non-dormant
  devices, taken from their latest verified heads plus local progress. Dormant
  and revoked devices are excluded.
- **Tombstones.** The snapshot builder applies invariant 5 and leaves out
  entities whose deletion is settled.

### Pruning

Every device prunes, not only the compactor, because devices may have
connectors that others lack. On each sync cycle:

1. **Compute the floor** from retained snapshots, limited by the minimum
   visible ack.
2. **Detect staleness.** If `progress[X] < floor[X]` for any X, re-bootstrap
   (see below) and skip pruning this cycle.
3. **Prune connectors.** For each connector, once every retained snapshot is
   delivered there, call `delete_object` for:
   - chunks and chunk indexes of known events at or below the floor;
   - dominated snapshots;
   - resolved enrollment requests, grants and rejections older than 30 days.

   Rotation objects are kept. A failed delete is retried on a later cycle and
   never blocks sync.
4. **Prune locally**, in one transaction:
   - delete operations from events at or below the floor that are not in a
     frontier, along with their parent rows;
   - delete the `sync_objects` and `sync_deliveries` rows of those events;
   - delete `sync_events` rows at or below the floor;
   - delete operations of expired tombstones outright;
   - replace the operations that came from the previous snapshot with those
     from the new one.

   The materialized state does not change (invariant 9).
5. **Guard repair uploads.** `enqueue_repair_deliveries` skips anything at or
   below the floor (invariant 10).
6. **Retire keys** per invariant 8. Delete the keychain entries and
   `sync_epoch_history` rows, and drop those epochs from the keyring that later
   grants and invitations carry.

### Re-bootstrap (stale device)

1. Set aside this device's own sealed or recorded events that no peer has acked,
   meaning their sequence is above `min ack[self]`.
2. Clear the graph tables and progress, then run Step 2's bootstrap.
3. Re-apply the set-aside events as operations on the new graph, keeping their
   ids, parents and stamps.
   - A parent that is still a frontier member makes the event supersede that
     value cleanly.
   - A parent that was pruned leaves the event alongside the current value, as
     an ordinary conflict.
   - Events not yet sealed are sealed with lamports above `lamport_high`.
4. Push and pull as usual. Settings shows "This device was offline for more than
   30 days and has resynced" and links to conflict review when there are
   conflicts.

The local app tables are never wiped. They are re-materialized from the graph,
exactly as when a remote update arrives.

### UI and docs

- The device list shows dormant devices as "Last synced *N* days ago — will
  resync when it returns". Automatic revocation of dormant devices is out of
  scope.
- Add a "Retention" section to `docs/cross-device-sync.md` covering:
  - history is compacted, and a device offline for more than 30 days resyncs;
  - deleted items are removed from connectors after that window;
  - provider version history (S3 versioning, cloud-drive trash, other IPFS
    pins) keeps copies until it expires;
  - after compaction, a leaked old join code or epoch key opens nothing that is
    still stored.

### Tests for Step 3

- **Invariant 9 across harness seeds:** materialized state and conflicts are
  the same before and after a prune, on every device and on newcomers.
- **Invariant 1:**
  - nothing is deleted before retained snapshots are delivered to that
    connector;
  - with two concurrent snapshots whose cuts don't dominate each other,
    pruning stays at or below their meet and newcomers can still bootstrap
    from either;
  - a buggy or inflated snapshot cut does not push the floor past the minimum
    visible ack.
- **Dormancy boundaries:**
  - 30 days minus one second is active; exactly 30 days and 30 days plus one
    second are dormant;
  - a future `published_at_ms` is treated as now;
  - a recently observed head change keeps a device active even if its signed
    time is old.
- **Stale devices:**
  - a device offline 31 days, with pruning done meanwhile, detects staleness,
    re-bootstraps, and keeps its unacked edits;
  - an edit to a value that has since changed becomes a conflict; an edit to an
    unchanged value supersedes cleanly (invariant 7);
  - a device offline 31 days with **no** pruning meanwhile catches up normally,
    without re-bootstrapping.
- **Tombstones:**
  - a deleted entity is gone from the snapshot and from connectors after
    pruning;
  - an edit racing a delete stays a visible conflict and is not expired;
  - a later edit to an expired entity does not bring it back (invariant 5).
- **Keys:**
  - an old epoch key is forgotten only after its last object is pruned
    (invariant 8);
  - a newcomer's keyring excludes retired epochs;
  - a leaked retired key opens no remaining object on any connector.
- **Repair:** adding a new connector after a prune does not upload pruned
  objects or dominated snapshots (invariant 10).
- **Transports:** extend the `sync-transport` conformance suite so that
  deleting an object is idempotent and a deleted object returns `NotFound`.
  Run it against folder, S3 (fake server) and IPFS.
- **Rotations:** pruning never deletes a rotation object, so a device that
  returns after several rotations can still follow the trust chain.

**Done when:** every invariant has passing tests, the harness seeds pass with
pruning on, and the docs are updated. **Version:** minor, 0.29.0 → 0.30.0.

---

## Risks and open questions

- **Two concurrent rotations claim the same epoch number.** Each device keeps
  only one key per epoch. The harness avoids this, and it needs its own fix,
  for example a tie-break on the rotation CID plus re-keying.
- **A rotation made before its initiator heard of a new member** isn't sealed
  to that member. The new member never gets that epoch's key.
- **A connector shared by two separate groups** makes each group's devices
  refetch the other group's rotations on every sweep. That traffic is
  harmless but wasteful. This setup is discouraged but allowed.

- **Clock skew on the compactor.** If the compactor's clock runs far ahead, it
  can mark live devices dormant too early. The damage is recoverable, because
  those devices re-bootstrap and keep their changes as conflicts, but it is
  disruptive. Possible mitigation: skip compaction when the compactor's clock
  is more than a day ahead of the newest `published_at_ms` it has seen.
- **Head metadata is visible to storage providers.** The ack vector and
  `published_at_ms` are signed but not encrypted, like today's
  `contiguous_sequence`. Providers already see write timing, so this adds
  little. If that changes, acks could move into encrypted events.
- **Dormant devices never leave on their own.** They stop blocking compaction,
  but they stay in the roster and keep receiving key stanzas until someone
  revokes them. A later prompt could suggest revoking a device dormant for 90
  days.
- **Snapshot size.** A single-message snapshot is enough for tasks, snippets
  and preferences. Draft sync, if it comes later, would need snapshots in
  multiple parts and more thought about body-sized values.
- **Enrollment sweep cost.** `run_enrollment_sweep` lists the whole object
  store every cycle. Pruning shrinks that listing, but a dedicated
  control-object prefix would scale better. Out of scope here.
