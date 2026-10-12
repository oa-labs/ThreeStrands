# Database modularization plan

Implemented in application version `0.86.2`. The root `db.rs` is now 167
lines; feature operations, helpers, and tests live under `db/`. All 108
original root database tests remain represented, with three additional
rollback regressions covering batch ingestion/upsert and settings import.
Validation: 862 Rust tests passed (the same three pre-existing tests remain
ignored), and all 60 end-to-end tests passed.

## Objective and scope

Split `src-tauri/src/db.rs` into cohesive persistence modules using the
existing `db/` pattern. Preserve the `Database` API, SQL behavior, storage
formats, and transaction boundaries. This is an organizational refactor;
query optimization, schema changes, and a repository/service redesign are
separate work.

At planning time, `db.rs` has 6,451 lines. Its main `impl Database` spans
roughly lines 429–2757, and its test module starts at line 3162. Moving only
methods would leave more than 3,000 lines of tests and many domain helpers
in the root. Move those with their owners as well.

The existing modules are uneven: `threads.rs`, `triage.rs`, `snippets.rs`,
and `split_inboxes.rs` have only 43, 52, 14, and 16 lines respectively,
while `contacts.rs` already has 1,092. Complete the small modules, and give
contact suggestions their own module rather than expanding the address-book
module further.

## Target ownership

Keep `db.rs` as the module entry point: declarations, stable re-exports,
`Database` fields, connection/transaction combinators, `DatabaseError`,
`DbResult`, and a few genuinely shared conversion helpers. Aim for roughly
200–300 lines; ownership matters more than a hard line limit.

All paths below are relative to `src-tauri/src/db/`. Existing feature
modules not listed retain their current responsibilities.

| Module | Responsibility and code to move from `db.rs` |
| --- | --- |
| `threads.rs` (existing) | Mailbox lists and paging, thread detail lookup, summary updates, unread counts including split buckets, deletion and retention pruning. Own `THREAD_COLUMNS`, `thread_from_row`, and local thread identity. |
| `split_inboxes.rs` (existing) | Rule CRUD/reordering, rule row decoding/matching, and split-inbox paging. Expose matching only within `db` for ordinary inbox exclusion/counts. |
| `snippets.rs` (existing) | Snippet CRUD and row decoding. |
| `triage.rs` (existing) | Sender statistics, event persistence, sender attribution, and event enum-to-storage mappings. |
| `contact_suggestions.rs` (new) | Public suggestions, suggestions including suppressed addresses, and their common ranking/filtering query. Preserve current `pub(crate)` entry points. |
| `messages.rs` (new) | Message IDs, attachment source lookup, unsubscribe begin/finish, raw metadata get/put, compression/decompression and metadata-storage helpers. |
| `search.rs` (new) | FTS queries, query normalization, bounded reindexing, and search/list preview generation including entity decoding. Ingestion uses the same preview helpers. |
| `mutations.rs` (new) | Optimistic apply/queue, undelivered replay, batch mutation, claim/complete/reject/retry, and `PendingMutation`. |
| `ingestion.rs` (new) | `apply_thread`, single/batch upsert, and ingestion with quarantine records. Coordinates contact indexing, message storage, search rows, and mutation replay inside the existing transaction. |
| `mail_sync.rs` (new) | Mail-provider sync status/cursors, sync recovery, reconciliation timestamps, sent backfill, provider-thread enumeration, and dismissal of sync problems. Own `SentBackfillProgress`. |
| `maintenance.rs` (new) | WAL checkpoints, snapshots and backup retention, VACUUM/reclamation, bounded body/metadata compression, and orphaned-metadata cleanup. Reindexing stays with search; thread retention stays with threads. |
| `recovery.rs` (new) | `Database::open`, test-only `open_memory`, startup indexes/seeding, integrity checks, corruption classification/quarantine, `open_with_recovery`, `OpenError`, and `RecoveryOutcome`. Calls narrowly exposed snapshot helpers in maintenance. |
| `settings_transfer.rs` (new) | Atomic native settings import. Keep its orchestration together rather than routing through separately committing feature methods. |
| `test_support.rs` (new, `cfg(test)`) | Shared database/message fixtures, seed cleanup, and temporary database paths. Domain assertions stay beside their implementation. |

Move account-only constants and row decoders into `accounts.rs`. Keep the
current sender-normalization semantics shared between triage and split-rule
matching; a small named helper in the root is sufficient. Do not replace
that parser during extraction.

Re-export moved types/functions from `db.rs` so existing paths such as
`db::PendingMutation`, `db::RecoveryOutcome`, and `db::open_with_recovery`
continue to compile. Keep `db.rs` as the entry point; renaming it to
`db/mod.rs` adds no benefit to this change.

## Dependency and transaction rules

1. Keep one shared `Mutex<Connection>` and the existing `connection`,
   `with_connection`, and `with_transaction` semantics. Preserve the
   `replicated_sync_projecting` field and its thread-scoped behavior.
2. Public operations acquire a connection or transaction. Helpers called
   while it is held accept `&Connection` or `&Transaction`; they must not
   call a method that reacquires the mutex or independently commits.
3. Ingestion remains one atomic operation across thread/message writes,
   contact interactions, FTS replacement, reindex dequeueing, quarantine
   updates, and replay of pending/running mutations. Keep the current order.
4. Optimistic local mutation and durable queue insertion remain atomic.
   Preserve target-message selection, deduplication, replay ordering, and
   outbox `archive_on_send` updates.
5. Settings import remains one transaction across accounts, rules,
   snippets, contacts/groups, and retention. Preserve credentials/status
   handling and the distinction between absent and empty contact groups.
6. Keep maintenance connection selection intact: periodic snapshots and
   checkpoints use their dedicated connection; other operations retain
   their current locking. VACUUM must remain outside a transaction.
7. New modules use explicit imports. Replace `use super::*` in existing
   modules as they are touched, so dependencies are visible. Use private
   helpers by default and `pub(super)` for sibling access; do not expand
   the application-facing API to make extraction compile.

The intended dependency direction is feature operations → shared
connection infrastructure. Ingestion may coordinate transaction-level
helpers from messages, contacts, search, and mutations. Thread views may use
split matching; split paging may call thread listing after releasing its
rule lookup connection. Preserve that existing lock sequencing.

There are also `impl Database` blocks outside `db/`, including replicated
sync, sync projection/state, enrollment, and correspondence. Leave them in
place for this effort. In particular, `db/mail_sync.rs` handles provider
mail synchronization and is distinct from the existing crate-level
`sync_state.rs`, which handles replicated application state.

## Delivery sequence

Each step should be independently reviewable and green. Move code with its
tests before considering cleanup; avoid mixing SQL rewrites, error-message
changes, or broad formatting changes into extraction diffs.

### 1. Establish the baseline and shared test support

- Run `cargo test` from `src-tauri` and record existing failures before
  changing code. Capture the named test inventory with `cargo test -- --list`.
- Extract shared fixtures into `db/test_support.rs`, preserving their
  seeded-versus-empty behavior. Do not silently replace `database()` with
  an empty database.
- Update `correspondence.rs`, which currently uses
  `crate::db::tests::TempDbPath`, to the new support path.
- Keep domain tests in place initially. Inventory helpers used across
  concerns so later moves do not require broad visibility changes.

### 2. Complete the small feature modules

- Move snippet and split-rule CRUD and their row decoders into their
  existing modules; move rule matching and split paging together.
- Move triage statistics and attribution helpers into `triage.rs`.
- Extract contact suggestions into `contact_suggestions.rs`.
- Move the corresponding tests, including account-isolation, suppression,
  ranking, and split-inbox exclusions. Inbox/count methods can remain in
  the root until step 3 and import the relocated matcher explicitly.

### 3. Extract thread reads, message storage, and search

- Move mailbox queries, thread reads/summaries, unread counts, and
  thread deletion/retention into `threads.rs`.
- Extract message/metadata/unsubscribe operations and storage codecs into
  `messages.rs`, then FTS queries, previews, and reindexing into `search.rs`.
- Give the still-rooted ingestion code explicit access to these helpers.
  Keep row-column order, body fallback behavior, attachment normalization,
  preview encoding, search ordering, page limits, and filtering unchanged.
- Relocate read/search/storage tests. Keep cross-feature assertions intact,
  such as deletion removing search rows and ingestion updating previews.

### 4. Extract mutations, ingestion, and provider sync

- Move the mutation lifecycle first, exposing only its transaction-level
  replay helper to ingestion; preserve `db::PendingMutation` by re-export.
- Move ingestion as a unit, keeping `apply_thread` and its callers together.
- Extract provider sync state and `SentBackfillProgress` into `mail_sync.rs`.
  Keep `sync_status` and `dismiss_sync_problems` as coordinating operations
  over their existing tables, rather than splitting their transactions.
- Relocate mutation/sync tests. Fill uncovered transaction boundaries with
  rollback tests: inject a late ingestion failure and assert no partial
  messages, contacts, FTS, quarantine, or mutation-state changes survive.
  Reuse existing rollback coverage wherever it already proves the invariant.

### 5. Extract maintenance, startup/recovery, and settings import

- Move snapshots/checkpoints/VACUUM/compression into maintenance, then move
  startup and recovery with their filesystem tests. Preserve startup order,
  pragmas, backup naming/retention, permissions, and corruption-only recovery.
- Keep tests proving snapshots/checkpoints do not wait for the shared
  connection, non-corruption errors leave files untouched, interrupted
  delivery recovers, and legacy compressed/plaintext rows still read.
- Move settings import without changing the transfer schema or fixture
  bytes. Preserve tests in `transfer.rs`, including historical exports.
- Add an import rollback regression if not already covered: a late failure
  must leave all earlier imported tables and destination account state
  unchanged. Use deterministic SQLite failure injection, not timing.

### 6. Finish ownership and document the boundaries

- Move remaining account helpers and domain tests to their owners; remove
  the emptied inline root test module only after every test is accounted for.
- Keep connection/error tests with the root infrastructure. Add concise
  module docs explaining each responsibility and any transaction-level
  helper contracts.
- Audit imports/re-exports and helper visibility. No generic `utils.rs`,
  forwarding wrappers for every method, new connection pools, or traits
  introduced solely for this refactor.
- Compare the named test inventory with the baseline, accounting for module
  path changes; every original test and assertion must remain represented.

## Validation and completion

Run `cargo test` in `src-tauri` after each Rust extraction. Run
`pnpm test:e2e` for steps touching triage, compose-related contact suggestions
or mutation behavior, and email body/preview paths. Use the automated
harness; do not launch the app or inspect its appearance manually. Run
`pnpm test` if frontend code changes become necessary; none are planned.
Check formatting on touched Rust files without reformatting unrelated code.

Completion requires:

- `db.rs` contains infrastructure and module wiring, with no feature SQL
  or large inline domain test suite.
- Each extracted concern owns its operations, private helpers, and tests.
- Existing callers compile unchanged apart from test-support imports;
  transaction, security, settings-transfer, and rendering invariants hold.
- No schema migration, transfer-format change, query/behavior change, or
  new dependency is required. Any discovered bug becomes a separate change.
- All required tests pass, or a reproducible environmental blocker is
  reported without deleting/skipping tests or claiming success.

This planning document does not change the shipped application and needs
no version bump. Implementation tasks that change shipped Rust code should
include a patch bump under `AGENTS.md`, keeping `package.json`,
`src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`, and the `threestrands`
entry in `src-tauri/Cargo.lock` identical. Choose the next patch from the
version present when implementing rather than pinning it in this plan.
