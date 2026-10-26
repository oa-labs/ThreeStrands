# Keeping persistence inside `db/`

Status: accepted (2026-10-26). Phases 0 and 1 done (0.173.1); Phase 2 done (0.173.2); Phase 5 done (0.173.3). Phase 3: sync state done (0.173.4), replicated sync done (0.173.5).

`src-tauri/src/db.rs` says "Feature modules own their SQL and tests": `db/`
owns persistence, and the rest of the app calls named `Database` methods.
Today SQL, `rusqlite` imports, and connection access also appear in about 16
modules outside `db/`. This plan moves that code back into `db/` in small
phases, then makes the compiler enforce the boundary.

[`commands/accounts.rs`](../src-tauri/src/commands/accounts.rs) is the model
to copy. It has no SQL, and every persistence step is one named `Database`
call (`list_accounts`, `adopt_mail_account`, `save_imap_account_settings`).

## What the audit found

Counts are non-test database calls (`connection`, `with_connection`,
`with_transaction`, `execute`, `query_row`, `prepare`, ...) as of 0.172.2.

**Misplaced `impl Database` blocks (about 85%).** These are `Database` methods
defined in service files, so callers still go through `Database`. The problem
is where the schema knowledge lives, not a bypass.

| File | Calls | Tables |
| --- | --- | --- |
| `replicated_sync.rs` (8 blocks) | 85 | `sync_values`, `sync_deliveries`, `sync_transports`, `sync_devices`, `sync_objects`, ... |
| `correspondence.rs` (line 440) | 48 | `outbox_messages`, `drafts`, `compose_settings`, `discarded_drafts`, ... |
| `enrollment.rs` (lines 226, 670), `enrollment/join_codes.rs` | 81 + 43 | `replicated_sync_enrollment_requests`, `replicated_sync_invitations`, `sync_spaces`, ... |
| `draft_push.rs`, `draft_pull.rs` | 39 + 27 | `drafts`, `draft_server_links`, `draft_server_deletions` |
| `scheduled_send.rs` | 25 + 5 free fn | schedules, `scheduled_send_summaries` |
| `sync_state.rs` | 23 + 13 free fn | sync cursors and state |
| `sync_projection.rs` | 22 | feature tables (see below) |
| `signatures.rs`, `gmail_signatures.rs` | 6 + 4 | signatures |

**Code that really bypasses `Database`:**

- `Correspondence` takes the connection directly 7 times
  (`correspondence.rs` lines 1514, 1571, 2203, 2209, 2434, 2456, 2643).
- `ImapStateStore` in `provider/imap/mod.rs`: 145 calls, the largest single
  offender. Also `BodyCache` (`provider/imap/fetch.rs`) and
  `ImapSettingsStore` (`provider/imap/settings.rs`).
- Free functions in `sync_state.rs`, `enrollment/join_codes.rs`, and
  `scheduled_send.rs`.
- Tests outside `db/` seed state with raw SQL: `lib.rs`, `sync.rs`,
  `sync_sim.rs`, `transfer.rs`, and the IMAP provider tests.

**Copied SQL, the real correctness risk.** The upserts for `tasks`,
`snippets`, and `split_inboxes` exist up to three times: in the feature
module, in `db/settings_transfer.rs`, and in `sync_projection.rs` (lines
186-253). `sync_projection.rs` also writes `accounts`, `calendar_accounts`,
`scheduled_send_summaries`, and `sender_rules`, and `enrollment.rs` writes
`snippets` and `scheduled_send_summaries`. If a column is added in one place
and missed in another, replication or settings transfer silently drops it.

## Rules for every phase

- Each phase is its own worktree, branch, and PR.
- Move code without changing behavior. A transaction closure moves whole into
  `db/`. A single statement that guards a state change
  (`UPDATE ... WHERE state IN (...)`) stays one statement, in a method named
  after the transition, for example `pause_outbox_item_for_reconnect(id)`.
- What stays in a service is orchestration: MIME, provider and network calls,
  crypto, retry and undo timing. Persistence becomes a named `Database` method.
- Moved methods return `DbResult` instead of `Result<_, String>`.
  `From<DatabaseError> for String` keeps existing callers compiling.
  User-facing refusals become `DatabaseError::Validation`.
- Tests move with their code, usually to `db/tests/<feature>.rs`. Do not
  delete, skip, or weaken any test.
- Run `cargo test` in `src-tauri`. Refactor phases get a patch version bump
  and no changelog entry.
- Each phase lowers the counts in the Phase 0 guard's allowlist.

## Phases

Suggested order: 0, 1, 2, 5, 3, 4, 6. Phase 4 comes after Phase 3 so the sync
code is already in `db/` when the copies are merged.

### Phase 0: stop new leaks

A source-scan test (`src-tauri/src/db/tests/boundary.rs`) counts, per file
outside `db/`, `db.rs`, and `schema.rs`: SQL statement literals, `rusqlite`
references, and `connection()` / `with_connection()` / `with_transaction()`
calls. Comments are ignored, and IMAP commands that share SQL keywords
(`SELECT INBOX`, `CREATE {mailbox}`) are not counted. It compares the counts
to the `ALLOWED` table in that file:

- A file that is not listed, or a count above its allowance, fails with the
  file name and a pointer to this document.
- A count below its allowance also fails, telling you to lower the allowlist
  entry. That way the allowlist can only go down.

Tests only, so no version bump.

### Phase 1: simple moves

Move `signatures.rs`, `gmail_signatures.rs`, and `scheduled_send.rs`
persistence to `db/signatures.rs` and `db/scheduled_send.rs`. Turn the
scheduled-send free functions into `Database` methods. This phase sets the
pattern for the larger moves.

### Phase 2: correspondence and drafts

- Split the `impl Database` block in `correspondence.rs` into
  `db/drafts.rs` (drafts, discarded drafts, server links and deletions),
  `db/outbox.rs` (`outbox_messages`), and `db/compose_settings.rs`.
- Move the `draft_push.rs` and `draft_pull.rs` blocks into `db/drafts.rs`.
- Replace `Correspondence`'s 7 direct connection grabs with named methods.

### Phase 3: sync state, replicated sync, and enrollment

About three PRs.

- `replicated_sync.rs` and `sync_state.rs` persistence moves to a
  `db/replicated_sync/` directory (`values`, `deliveries`, `transports`,
  `devices`, `objects`).
- `enrollment.rs` and `enrollment/join_codes.rs` persistence moves to
  `db/enrollment.rs` and `db/join_codes.rs`.
- `ReplicatedSync` and the enrollment flow keep their protocol and crypto
  logic.

### Phase 4: one copy of each table's SQL

- Each feature module exposes `upsert_synced(tx, ...)` and
  `delete_synced(tx, id)` on the caller's transaction. Local saves,
  `sync_projection`, `db/settings_transfer.rs`, and the enrollment reset all
  use them.
- Where projection deliberately differs, it gets its own named function in the
  feature module. Example: a projected account arrives as `needs_reauth` with
  `on_this_device = 0`.
- Add round-trip tests for each projected entity type, and keep the settings
  transfer regression fixtures for earlier exports passing.

This is the one phase that can change behavior, so review it most carefully.

### Phase 5: IMAP state

Move the SQL in `ImapStateStore`, `BodyCache`, and `ImapSettingsStore` to
`db/imap.rs` (`imap_locations`, `imap_mailboxes`, `imap_copy_intents`,
`imap_sent_copy_detection`, the body cache, the settings row).
`ImapStateStore` and `BodyCache` are already per-account persistence
handles, so they moved into `db/imap.rs` whole and `provider::imap`
re-exports them; provider code is unchanged. `ImapSettingsStore` stays in
the provider and calls the existing `Database` settings methods.

### Phase 6: let the compiler enforce it

- Make `Database::connection`, `with_connection`, and `with_transaction`
  private to `db`. A new leak then fails to compile.
- Move test seeding outside `db/` to `db::test_support` helpers.
- Reduce the Phase 0 scan to SQL literals and `rusqlite` references, with an
  empty allowlist, and update the module comment in `db.rs`.

## Risks

- Phases 2, 3, and 5 move thousands of lines. Concurrent branches that touch
  the same files will conflict, and `git blame` gets noisier (use
  `git log --follow` or `blame -C`). Run these phases when little else is in
  flight in those files.
- Phase 4 touches replication and settings transfer, which are cross-version
  formats. It must not change what is written or read. See the settings
  transfer rules in `AGENTS.md`.
