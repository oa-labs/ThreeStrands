# Replicated sync: state-based design

**Status:** Implemented in 0.30.0 (sync protocol version 3). Protocol 4
anchors key rotations so a recovery-phrase join can trust them (see
[Recovery-phrase joins](#recovery-phrase-joins)).
**Date:** 2026-09-23
**Scope:** `crates/sync-core`, `crates/sync-envelope`, `crates/sync-protocol`,
`crates/sync-transport`, `src-tauri/src/db/sync_state.rs`,
`src-tauri/src/replicated_sync.rs`, `src-tauri/src/sync_projection.rs`,
`src-tauri/src/sync_connectors.rs`, `src-tauri/src/sync_key_store.rs`,
`src-tauri/src/sync_policy.rs`, `src-tauri/src/sync_sim.rs`,
`src-tauri/src/enrollment/`

This replaces the event-log design, protocol versions 1 and 2, and the
compaction plan that went with it. In that design each device published an
append-only event log. Readers walked it, and it would have needed
snapshots, acks, a 30-day offline horizon, pruning and tombstone expiry to
stay bounded. The synchronized data is structured state (tasks, notes, contacts,
snippets, Split Inboxes, preferences, account metadata, snoozes, and later
additions), so a log was more machinery than the data needed. Enrollment, keys,
connectors and the envelope format carry over unchanged.

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

## Wire format (protocol 4)

Protocol 4 changed only key rotations and the marker. Snapshots and heads
are as protocol 3 left them, and keep their v3 names and vectors.


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
- **Key rotations** (`SignedKeyRotation`): besides the initiator's device
  signature (`…/key-rotation-signature/v1`), each carries exactly one proof
  over the same canonical rotation:
  - `recovery_anchor`, on epoch 0 only: a signature by the recovery
    Ed25519 key, under `…/key-rotation-recovery-anchor/v1`. Genesis makes
    it while it still holds the seed.
  - `epoch_link`, on every epoch `n + 1` only: a signature under
    `…/key-rotation-epoch-link/v1` by an Ed25519 key derived from the raw
    `K_n` with HKDF-SHA256, `info` = `…/epoch-link-key/v1` followed by `n`
    as a big-endian `u32`. A holder of `K_n` derives the same public key to
    check it.

  Both fields are left out of the encoding when absent, so a protocol 3
  rotation still decodes byte for byte (which is how its group is
  recognized as `Legacy`), but neither verifier accepts it. Epoch 0 with a
  link, or a later epoch with an anchor, is refused the same way.
- **Protocol marker:** a well-known object reading `threestrands-sync
  protocol 4`. A connector holding a group without it is reported as
  `Legacy` and refused.
- **Epoch keyring:** grants and invitations carry `earlier_epoch_keys`, which
  predates this design and is unchanged.

## Engine

### Local store (`db/sync_state.rs`)

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

### Recovery-phrase joins

A recovering device has the phrase and nothing else: no roster to check
initiators against. Opening a rotation's stanza with the phrase-derived
recovery X25519 secret proves nothing, because the recovery public keys
travel in cleartext in every rotation and anyone who can write to the
connector can seal a key of their choosing to them. Before protocol 4 a
recovery join trusted any self-signed rotation that opened that way, so
someone with only write access to the storage could plant a rotation at the
next epoch naming themselves; the recovering device adopted it and announced
the planted roster under the recovery key, and every member then trusted
the intruder and shared every epoch key with it.

Protocol 4 anchors the chain on two facts: only holders of the phrase can
sign with the recovery Ed25519 key, and only members hold epoch keys.
`join_with_recovery_phrase`:

1. Derives both recovery public keys from the phrase and ignores every
   rotation naming different ones. It stores the phrase-derived keys, never
   a rotation's.
2. Keeps the rotations that verify against their own initiator and open
   with the recovery secret, as candidates.
3. Accepts at epoch 0 only the candidate whose anchor verifies under the
   phrase-derived key. None is `NoRecoverableRotation`; more than one
   (only possible if the phrase leaked) is `AmbiguousRecoveryGroup`.
4. Accepts at each later epoch only candidates whose link verifies under
   the key of a rotation accepted at the epoch before, and takes the
   longest chain these links form, so a rotation the chain continues from
   wins over a dead end. At the tip, of several linked rotations the one
   with the greater content address wins: its initiator is the one that
   rotates again to settle the collision (`must_rotate_after_collision`),
   so the chain continues from it.
5. Stores keys and activations for the chain only, adopts the tip's
   roster, and refuses with `RecoveryIncomplete` if any epoch below the
   newest candidate has no candidate at all, or if a member's verified head
   is past the tip. A candidate right after the tip that doesn't link is
   ignored, not treated as a gap.

Members still apply a rotation only when its initiator is in their own
roster, and don't check links.

## Device authority for external operations

Assign one owner to each operation that can make a conflicting or duplicate
change outside ThreeStrands. Use the narrowest scope that needs one writer.
An IMAP rename affects a mailbox namespace, so one device owns structural
edits for that account. A scheduled send has its own payload and deadline, so
each message has its own sending device. Ordinary replicated data continues to
use the merge rules above.

- **Assignment and eligibility.** Define how the initial owner is selected.
  Check that the device supports the operation and has the required account
  access and local state before it acts. Keep ownership stable across restarts
  and periods offline. Expose the owner, whether this device can act, and what
  the user can do when the owner is unavailable.
- **Execution.** Check ownership where the external write is issued. Record
  the operation and its outcome durably so a restart or retry can distinguish
  work that never started from work that may already have succeeded. Treat a
  replicated status report as information for the user. Execution requires a
  separate, valid grant of authority and any local state the operation needs.
- **Handoff.** Transfer ownership explicitly when the operation supports it.
  The outgoing owner stops taking new work, settles or reconciles in-flight
  work, and commits the loss of its permission before the incoming owner can
  act. Bind the handoff to the current ownership revision so a stale handoff
  cannot grant permission again. If the operation requires moving a payload
  or receipt, the incoming owner must have it before starting.
- **Unavailable owner.** A missed heartbeat, an expired timer, or removal
  from Replicated Sync cannot establish that the previous owner has stopped
  acting. An offline device may still have valid provider credentials. Pause
  the affected operation when safe transfer is unavailable. Any proposed
  automatic failover needs a way to prevent the old owner from acting or
  guarantee that concurrent submissions of the same operation have only one
  effect. A merge winner alone provides neither guarantee.

IMAP structural edits use account-scoped ownership with explicit
desktop-to-desktop handoff. [IMAP cross-device edits](imap-gmail-extensions.md#cross-device-edits)

Send Later keeps each message on the scheduling installation. Its keychain
identity works independently of sync enrollment, and peers see status reports
without receiving the send payload. A future handoff needs its own payload
transfer and delivery reconciliation design.
[Send Later ownership](scheduled-send-design.md#ownership-and-local-persistence)

Test ownership with two devices, including an offline owner, a delayed or
replayed handoff, restart during an external write, and an uncertain provider
result. Check that each scoped operation has at most one authorized writer and
that peers can report blocked work without executing it. Reuse the ownership
and status conventions across features. Extract shared handoff code when
another operation needs the same transitions.

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
  - v1 and v2 heads refused;
  - golden protocol 4 genesis and successor rotations, and a protocol 3
    rotation that decodes but proves nothing;
  - anchor and link signing and verification, domain separation between
    the device signature, anchor and link, a link bound to its epoch, and
    missing or extra proofs refused.
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
  - migration from protocol 2 groups, and leaving protocol 3 groups;
  - recovery joins next to rotations planted by someone with only storage
    access: after the newest epoch, competing at an existing one, a genesis
    without the phrase's anchor, substituted recovery keys, and a missing
    or tampered link. The intruder stays off every roster and never
    receives a key.
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
- **Protocol 4:** a recovery-phrase join could be taken over by anyone able
  to write to a connector (see [Recovery-phrase joins](#recovery-phrase-joins)).
  Rotations now carry a recovery anchor or an epoch link. Existing groups
  can't be upgraded in place, so updating leaves them, as protocol 3 did
  with protocol 2 groups: schema migration v121 clears the membership,
  queues the old keys for removal from the keychain, and shows the reset
  notice once.

### What the event-log design needed that this one doesn't

These no longer apply:
- chain walks and causal vectors;
- progress fixpoints and acks;
- snapshots-plus-events bootstrapping;
- the 30-day offline horizon and re-bootstrap;
- event pruning and tombstone expiry.

A connector holds about one snapshot per device, plus control objects.

## Known gaps and open questions

- **A former member can fork the chain for a recovering device.** A link
  proves only that its maker held `K_n`. A device revoked at epoch `n + 1`
  still holds `K_n` and can publish its own rotation `n + 1`. Members ignore
  it (its initiator is revoked), but a recovering device sees two linked
  rotations at the tip and takes the one with the greater content address,
  which the former member can choose by retrying. The same goes for a
  device that received a key on the losing side of a concurrent rotation.
  Existing members still refuse to reactivate a revoked device from the
  announcement that follows, but the recovering device ends up on the wrong
  key and roster. Telling such forks apart needs the revocation itself to
  be part of what the next link proves, which this design doesn't do yet.
- **Planted rotations can stall a recovery join.** A rotation planted
  further than one epoch past the tip looks like a key change that hasn't
  arrived, so the join waits. Anyone who can write to the storage can
  equally delete the group's files, so this adds no new denial of service.
- **Whole-state republishing:** every change reseals and re-uploads the whole
  snapshot. That is fine for kilobytes to a few megabytes, which is why
  compose drafts moved to the mail server in 0.149.0 instead of riding in
  snapshots. Files attached to notes travel beside the snapshot instead: each
  is sealed once as `ObjectKind::AttachmentPart` objects that the snapshot
  names by CID, so they are uploaded once rather than with every change (see
  [Images and attachments](notes-design.md#images-and-attachments)). Any
  other large synchronized data set would need the same, snapshots split by
  entity type, or deltas.
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

*(Resolved since initial design: concurrent rotation collisions are settled via
`must_rotate_after_collision` / `ControlObject::RotateToResolve` and tested in
`sync_sim.rs`; unverifiable objects from another group on a shared connector are
backed off and ignored after 7 days by `enrollment/deferred.rs`.)*
