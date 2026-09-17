# Database recovery plan

Dispatch's local cache (`<app_data_dir>/dispatch.sqlite`) already gets WAL mode,
foreign keys, and transactional mutations right (`src-tauri/src/db.rs`,
`src-tauri/src/schema.rs`). What it has never had is a plan for the day the
file itself is damaged: no startup integrity check, no backup of any kind, no
durability pragmas beyond the two defaults baked into `INITIAL_SCHEMA`, no
bounded WAL checkpoint policy, and no tested behavior when a migration or a
write fails partway or the disk fills up. Today any open/migration failure is
fatal — it panics the whole app via `.expect("error while building Dispatch")`
in `lib.rs`, with no recovery path and no user-facing explanation.

Because Gmail remains the source of truth and the local database is a
resyncable cache (`reconciliation_due`/`begin_sync_recovery` already assume
this), the goal here is not zero data loss — it's *never lose the user's
mailbox access to a corrupt cache file*, and *never silently serve corrupt
data*. Losing a cache and resyncing is an acceptable outcome; a permanently
unlaunchable app or an app that opens a broken database without noticing is
not.

## Current state (for reference)

- `Database::open` (`db.rs:83-100`) always re-applies `INITIAL_SCHEMA`
  (`journal_mode=WAL`, `foreign_keys=ON`), runs `migrate()`, then indexes and
  seeds. No `synchronous`, `busy_timeout`, or `wal_autocheckpoint` pragma is
  ever set; no `quick_check`/`integrity_check` is ever run.
- `migrate()` (`schema.rs:127-381`) runs the entire version ladder in one
  transaction, so a mid-migration failure leaves `user_version` unchanged —
  good — but there is no backup taken beforehand, and two statements
  (outbox `sending`→`uncertain` cleanup) run unconditionally *after* the
  commit, outside that transaction.
- There is no backup mechanism anywhere (`VACUUM INTO`, `.backup()`, or a
  plain file copy) and no periodic WAL checkpoint beyond SQLite's own
  automatic checkpointing.
- Any error from `Connection::open`, `execute_batch`, or `migrate` propagates
  out of `Database::open` and is turned into a hard `.expect()` panic in
  `lib.rs`'s `.setup()` closure — the app does not start, and there is no
  repair, fallback, or user-facing message.
- Zero test coverage exists for corruption, disk-full, or any other fault
  injection.

## Plan

### 1. Durability pragmas (do first — cheap, no schema/behavior risk)

Set explicitly in `Database::open`, right after `Connection::open`, instead
of relying on SQLite defaults:

- `PRAGMA synchronous = NORMAL` — safe under WAL (won't corrupt the database),
  trades "durable against OS/power loss for the last commit" for a real
  write-latency win. Acceptable here because the cache is resyncable; call
  this out explicitly since it's the one pragma that's a deliberate
  correctness/performance tradeoff, not a free win.
- `PRAGMA busy_timeout = 5000` — currently unset (defaults to 0 /
  immediate `SQLITE_BUSY`). Low risk today since the app funnels all access
  through one `Mutex<Connection>`, but becomes load-bearing as soon as a
  second connection exists (backups, quick_check on a copy, etc. — see below).
- `PRAGMA wal_autocheckpoint = 1000` — make the existing default explicit
  rather than implicit, so the checkpoint policy in step 4 isn't silently
  overridden if SQLite's own default ever changes.
- `PRAGMA journal_size_limit = <e.g. 64 MiB>` — bounds how large the WAL file
  can grow between checkpoints on a long-running or checkpoint-starved
  session.

### 2. Checkpoint policy

Add an explicit, bounded checkpoint step to the existing 6-hour maintenance
loop in `lib.rs` (the same loop that already calls `prune_expired_threads`
and `reclaim_space`): run `PRAGMA wal_checkpoint(TRUNCATE)` after pruning.
`TRUNCATE` both checkpoints and shrinks the `-wal` file back down, which
matters for an app that can be left open for days. Because the single
`Mutex<Connection>` serializes all access, there's no concurrent-reader
`SQLITE_BUSY` risk to a `TRUNCATE` checkpoint here the way there would be with
multiple connections.

### 3. Backups, via `VACUUM INTO`

No new cargo dependency needed — `rusqlite`'s `"backup"` feature (for the
step-wise online backup API) isn't enabled today, and `VACUUM INTO 'path'` is
a plain SQL statement that produces a consistent, defragmented, standalone
snapshot honoring WAL, runnable through the same `execute` the rest of `db.rs`
already uses.

- **Pre-migration backup.** In `Database::open`, before calling
  `schema::migrate`, compare the on-disk `user_version` against the latest
  version the binary knows about. If a migration is about to run, first
  `VACUUM INTO` a sibling file named
  `dispatch.sqlite.pre-migration-v<old_version>.bak` in the same
  `app_data_dir`. Keep only the most recent 3 pre-migration backups (delete
  older ones by version number) so this can't grow unbounded across repeated
  upgrades. On migration failure, leave the backup in place and include its
  path in the returned error — no automatic restore, since a failed migration
  might indicate a hardware problem worth surfacing rather than papering over.
- **Periodic backup.** In the existing 6-hour maintenance loop, after the
  checkpoint in step 2, `VACUUM INTO` a rotating `dispatch.sqlite.backup-N`
  (keep the last 7, i.e. roughly the last 1.75 days at this interval — tune
  once real mailbox sizes/timings are known). Run this via `spawn_blocking`
  like the other maintenance calls.
- **Known tradeoff:** `VACUUM INTO` runs on the shared connection, so it holds
  the `Mutex` — and therefore blocks every other DB read/write — for as long
  as the copy takes. This is consistent with how `prune_expired_threads` and
  `reclaim_space` already behave in that same loop, but should be timed against
  a realistically large mailbox during implementation; if it turns out to
  stall the UI noticeably, the fallback is a second read-only connection
  (WAL allows concurrent readers) dedicated to backups, at the cost of also
  needing `busy_timeout` tuning between the two connections.

### 4. Startup integrity check

Running `PRAGMA quick_check` unconditionally on every launch is the simplest
option but adds latency to every startup for a check that's almost always a
no-op. Instead, run it only when startup looks suspicious:

- the `-wal` file exists and is non-empty when `Connection::open` is called
  (a clean shutdown checkpoints and can leave a zero-length or absent WAL), or
- the existing "reset running mutations to pending" step (`db.rs:90-95`)
  actually changed any rows — that already is the signal for "the app didn't
  shut down cleanly last time."

On either signal, run `PRAGMA quick_check` (not the much slower full
`integrity_check`) right after `migrate()`/`ensure_query_indexes` and before
`seed_if_empty`. A non-`ok` result hands off to the recovery flow in step 5
rather than continuing to open a database that's already known to be broken.

### 5. Corruption recovery UX

Replace the current `.expect()`-panics-the-app behavior for *any* fatal DB
error (open failure, migration failure, or a failed `quick_check`) with a
small recovery ladder, implemented as an `open_with_recovery` wrapper that
`lib.rs` calls instead of `Database::open` directly:

1. Try the normal `Database::open`.
2. On failure, move the broken file aside to
   `dispatch.sqlite.corrupt-<timestamp>` (keeping it for support/diagnosis,
   not deleting it) and attempt to restore the most recent periodic backup
   from step 3 in its place, then retry `Database::open` and confirm the
   restored copy passes `quick_check`.
3. If no usable backup exists or the restored copy still fails, fall back to
   a fresh empty database (today's schema seeded from scratch) so the app can
   still launch.
4. Whichever path was taken, record a "recovery report" (what happened, and
   the corrupt file's path if one was preserved) in Tauri-managed state, and
   expose it to the frontend via a command so the UI can show a banner
   ("Your mail cache needed to be rebuilt — resyncing now") and kick off a
   full resync rather than silently presenting an empty or stale inbox.

This turns today's unrecoverable panic into "worst case, the user loses the
local cache and gets a resync," which matches the resyncable-cache framing
above.

### 6. Fault-injection tests

- **Corruption:** build a helper that opens a temp-file DB, writes known
  data, closes it, flips bytes at a specific page offset, then asserts (a)
  `PRAGMA quick_check` on that file actually reports failure — a check that
  never fires is worse than no check — and (b) `open_with_recovery` recovers
  via backup-restore-or-fresh-DB rather than propagating a panic.
- **Migration failure:** add a test-only migration step that can be forced to
  fail partway, and assert both that `user_version` is left unchanged
  (transaction atomicity, already true today) and that the pre-migration
  backup file from step 3 exists and is itself openable.
- **Disk-full:** genuine `ENOSPC` needs a constrained filesystem (e.g. a
  small tmpfs/loopback mount), which isn't something to run in the default
  `cargo test` loop. Add it as an `#[ignore]`d test with a short setup
  comment, runnable manually or as a dedicated CI job, asserting that a
  write failure under `ENOSPC` surfaces as a normal `Err(String)` (as
  `display_error` already does) rather than a panic, and that a pending
  mutation isn't left in a half-applied state.

## Sequencing

Phases 1 and 2 are pure additions with no behavior change on the happy path
and should ship first. Phase 3 (backups) is the prerequisite for phase 5's
automatic recovery, so it comes next. Phases 4 and 5 (integrity check +
recovery UX) land together, since a check with no recovery path is just a
different panic. Fault-injection tests should be written alongside whichever
phase they're validating rather than saved entirely for the end, so each
piece is verified as it lands.
