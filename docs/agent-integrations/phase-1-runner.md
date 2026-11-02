# Phase 1: durable delegated-work foundation

**Implemented:** 2026-11-02 for 0.195.0. This is internal groundwork, with no
agent workspace or real research tools enabled. See the
[parent plan](../delegated-agents-plan.md) and
[phase-0 contract](decision-record.md).

## Available contracts

The native command surface and MailClient expose enable/read status, create,
get/list, pause, resume, retry, cancel, archive/unarchive, mark read, answer a
question, and delete. Execution is disabled by default on every installation.
The local enable flag is not part of settings transfer or encrypted sync.

Only `fixture_research_v1` can be submitted. Unknown workflows are rejected before
insertion. A request includes an objective and account, with optional source
conversation and personal-task links, both validated against the account. The
fixture never reads email bodies, takes provider credentials, or makes model,
search, routing, or other external calls. Source snapshots and real provider
configuration belong to the later research implementation.

The fixture asks a required question, completes an independent evidence step,
then waits without dispatching more work until the user answers. Answers may
arrive before the independent step finishes. Resubmitting the same answer is
idempotent; conflicting or stale answers are rejected. Publication produces one
immutable, explicitly labeled fixture artifact with `notResearched: true`.
Completing it does not change the linked personal task's status.

The demo client implements the same visible lifecycle in memory, advancing the
fixture without real time or network calls. It is a preview adapter; only the
native implementation provides durability and worker leases. Native read/mutation
payloads have direct client contract coverage.

## Persistence and worker behavior

Schema 95 adds local tasks, runs, turns, step attempts, and versioned artifacts.
Foreign keys cascade account/task deletion through all agent-owned records.
Removing cached conversations or personal tasks detaches their links; agent
results remain discoverable independently. Removing or disconnecting the mail
account on this device deletes its agent data. These records are not replicated
entities and do not change existing task or settings-transfer formats.

Native modules are `src-tauri/src/agents/`, `db/agents.rs`, and
`commands/agents.rs`; SQL stays behind Database methods. Public frontend types
live in `src/domain.ts`, transport in `src/data/client.ts`, and preview behavior
in `src/data/demoAgents.ts`.

Two worker loops start with the app but cannot claim work until explicitly
enabled. Each claim and intent is committed before executing a step outside the
database lock. There is at most one active run per delegated task and two active
leases globally. Completion validates run identity, checkpoint, generation,
owner, unexpired lease, and enabled status, then commits the result/checkpoint
and artifact together. Cancel, pause, disable, account removal, and deletion
prevent late workers from publishing or recreating data.

A lease lasts at most 60 seconds, shortened to the remaining active-time
allowance. Expired work is recovered at the exact lease boundary, with the old
attempt marked ambiguous and its dispatch/time reservation retained. Already
committed checkpoints are not replayed. Normal exit and updater relaunch stop
the runner and release its leases; abrupt exit relies on expiry. Shutdown errors
are logged without preventing the app from exiting.

Transient failures allow two automatic retries, with durable backoff deadlines.
An explicit retry starts another attempt group at the current checkpoint but
preserves cumulative dispatch and active-time usage. Pause/resume never resets
the task allowance. This fixture foundation caps total step dispatches at 20 and
active time at 900 seconds; real tool-specific, token, byte, pricing, query, and
URL enforcement remains phase 3 work. Phase-0 allowance parity is tested for the
limits already used by the scheduler.

## Verification

- Native database tests cover the preceding-schema upgrade, waiting and early
  answers, duplicate answers, restart and exact lease expiry, checkpoint replay,
  concurrent claims, cancellation/deletion/disable races, retry backoff, budget
  boundaries, account scoping/cleanup, task unlinking, and atomic publication.
- Async runner tests inject the clock and a blocked executor to verify that
  database operations remain available during work and cancelled workers cannot
  publish. Shutdown releases leases and invalidates old workers.
- Direct demo and native-client payload tests cover explicit enablement,
  clarification, completion, archive/read, cancellation, deletion, and personal
  task independence. Existing suites retain their behavior contracts.
- The exact post-baseline schema-object expectation intentionally includes the
  new tables and indexes; migration idempotency remains asserted.

Run `cargo test` in `src-tauri`, `pnpm test` at the root, and
`node --test scripts/agent-contracts.test.mjs`. Type checking and focused ESLint
checks cover the frontend contract additions. Run `pnpm test:e2e` to guard the
existing compose lifecycle around the new graceful shutdown hook.

The next phase provides the Agents list/detail view, conversation and artifact
presentation, and user-facing controls. Do not describe this foundation as
available invitation research or add it to the product's current feature list.

### Recorded verification on 2026-11-02

- `pnpm test`: 213 files, 3,155 tests passed.
- Frontend build, TypeScript, and focused ESLint checks passed.
- All 15 new native agent tests and the existing exact schema/idempotency check
  passed in the full native run. Existing schema expectations were extended for
  the seven new table/index objects; no assertions were removed.
- The full Rust run with four test threads finished with 2,049 passed, three
  existing URLSession tests failed, and seven existing ignored tests. The
  failures report that the peer address could not be confirmed. A serial retry
  of that module finished seven passed/one failed; the same failure in
  `admits_the_byte_limit_and_refuses_one_byte_more` reproduced in the pre-change
  cached test executable. The image-proxy implementation and tests are unchanged.
- `pnpm test:e2e`: 191 passed and nine existing rendering navigations timed out.
  All nine passed with `--last-failed --workers=1`, using unchanged timeouts and
  assertions. The complete set of 200 tests passed across those executions.
- All 31 phase-0 contract tests and 30 changelog-format/release-note tests passed.
  No changelog entry was added because this phase exposes no user-facing feature.

These earlier failures are intermittent; the subsequent passing run below does
not establish that their cause is fixed. Do not weaken peer-address verification
to hide failures.

### Pre-merge verification on 2026-11-02

Review added run/checkpoint identity checks and shortened leases to the remaining
active-time allowance, with two additional regression tests. The final full
`cargo test -- --test-threads=4` run passed: 2,054 passed, zero failed, seven
existing ignored. All 17 agent tests and the existing schema checks passed.
The 31 offline integration-contract tests also passed again. Frontend code is
unchanged since the passing frontend/build/E2E verification above.

Investigate URLSession metrics and E2E page-load intermittency as separate
reliability work. Completing the delegated-agent plan will not inherently fix
either issue, and a passing rerun is not evidence of a root-cause repair.

### Test reliability follow-up on 2026-11-02

Both test issues now have focused fixes outside the agent implementation:

- The URLSession fixture listener previously stopped after three seconds, even
  if CFNetwork's first request had not started. Instrumentation reproduced a
  listener exiting with zero requests before a connection-refused error and
  consequently missing peer metrics. A test-owned server guard now keeps the
  listener alive until the fetch completes and stops/joins it on explicit join
  or drop, including panic cleanup. Socket I/O remains bounded, and the client
  timeout and production peer validation are unchanged. A new regression checks
  cleanup of an unused listener.
- Rendering tests now navigate through DOMContentLoaded and assert initialized
  message content: the trusted email root, the reader-owned tone attribute, and
  a visible message container. All existing layout/security assertions remain.
  A new regression deliberately stalls an unrelated parent-page image; it
  reproduced the old page-load timeout and passes with content-based readiness.
  The original intermittent full-suite navigation failure did not reproduce in
  the diagnostic run, so this establishes removal of the unnecessary load-event
  dependency rather than identifying every possible navigation stall.

Verification passed: full native suite (2,055 passed, seven existing ignored),
frontend suite (3,155 passed), and E2E suite (201 passed). The nine URLSession
module tests also passed in five fresh processes, and 123 repeated affected
rendering cases passed with five workers under concurrent test load. Focused
ESLint and TypeScript checks passed. Temporary diagnostics were removed. These
are test-only changes; no application version or changelog change is needed.
