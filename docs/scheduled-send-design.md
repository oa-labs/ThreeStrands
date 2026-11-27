# Scheduled send with synchronized visibility

**Status:** Implemented in 0.98.1 (2026-10-10; shortcut in 0.98.2) for Gmail,
and extended to IMAP/SMTP once SMTP submission shipped (`send_supported: true`).
Live-account and native sleep/wake verification remain developer release checks.

## Product contract

Each scheduled message belongs to the computer where it was scheduled. Only
that installation can submit it. Other computers in the same sync group can
see a summary and the last reported status. Message bodies, recipients,
attachments, drafts, and account credentials remain local.

ThreeStrands must be running on the sending computer, which must be awake,
connected, and authorized to send. The scheduled time means an attempt to
submit the message, not a guarantee of recipient arrival. Another computer
never takes over automatically.

Scheduling works with or without replicated sync, for both Gmail and IMAP/SMTP
(`ProviderCapabilities.send_supported`). Visibility on other computers requires
an enrolled sync group and working connectors.

On the sending computer, users can cancel, reschedule, or return the message
to an editable draft until delivery starts. On other computers, entries are
read-only and say which computer owns them. Remote cancellation requests,
ownership transfer, automatic failover, background helpers, and server-side
scheduling are separate features.

## Implementation modules

- `src-tauri/src/correspondence/{mime,model,service}.rs`,
  `src-tauri/src/scheduled_send.rs`, and `src-tauri/src/db/scheduled_send.rs`
  own scheduling policy (`DISPATCH_GRACE_MS = 60_000`), MIME freezing, outbox
  claims, and startup recovery (future schedules and elapsed `overdue` states
  survive restart unchanged).
- `finish_exit` in `src-tauri/src/commands/mail.rs` waits for pending undo
  windows and active sends, while leaving future/overdue schedules saved and
  exiting.
- `crates/sync-protocol`, `db/sync_state.rs`, and `sync_projection.rs` carry
  `EntityType::ScheduledSendSummary` (`ScheduledSendReport` with 12 fields),
  keeping raw outbox payloads local.
- `src/correspondence.ts`, `src/useCorrespondence.tsx`,
  `src/ScheduleSendPicker.tsx`, `src/ComposeSendActions.tsx`, and
  `src/data/demoCorrespondence.ts` own the frontend compose/outbox contracts.

## Scheduling and missed times

The picker accepts a future date, time, and explicit IANA timezone. Resolve
the selection in Rust and persist the UTC instant plus the original timezone.
Reject nonexistent daylight-saving times and require a choice of offset for
ambiguous times. Changing the computer's timezone later does not move the
scheduled instant. Show the original time and zone, with a local equivalent
on computers using another zone.

Use a 60-second dispatch grace period. A continuously running worker
may claim a message from its target instant through the end of that period.
If it cannot claim within that period, the message becomes `overdue` and
requires an action on its owner: **Send now**, **Reschedule**, or **Return to
draft**. Restart or wake after the target instant also makes an unclaimed
message overdue, even if still inside the grace period. This avoids sending
an old scheduled message unexpectedly when a computer returns.

Put the grace period and timing policy in a native scheduling policy module;
inject time and resume signals for deterministic tests. Recheck the time
window at the atomic claim, including after authorization takes time. Clock
changes must never cause a claim before the persisted UTC instant; a forward
jump past the window produces overdue. Ordinary timer jitter does not.

Scheduling does not add ten seconds to the chosen target time. Cancellation
is available throughout the scheduled wait. **Send now** from overdue uses
the existing ten-second undo path. The ordinary Send action keeps its current
undo/restart contract.

## Ownership and local persistence

Send Later uses per-message ownership under the [device authority contract](replicated-sync-design.md#device-authority-for-external-operations).

Introduce a stable sending-installation UUID, stored in the OS keychain.
Exclude restoration of the local identity from settings transfer and SQLite
backup restoration; a public owner identifier in a summary cannot establish
the receiving installation's identity. Keep it
independent of sync enrollment: leaving and rejoining a group currently
creates a new sync device ID. A keychain access failure blocks scheduling
with an actionable error; it must not silently generate a replacement ID.

Every new schedule captures this identifier in Rust. Callers cannot supply
or change the owner. At dispatch, compare the persisted owner with the
installation identity. A restored database on a different computer, a
missing keychain identity, or an ownership mismatch pauses the schedule for
review; it never grants sending authority. Document that copying both a
profile and its keychain identity is outside this device-isolation guarantee.

`outbox_messages.schedule_json` and `scheduled_send_summaries` live in the
baseline schema (`INITIAL_SCHEMA` v74 in `src-tauri/src/schema.rs`). Ordinary
sends keep `schedule_json` null. Scheduled metadata holds the UTC target,
original IANA timezone, immutable installation owner, original visibility
group, and owner-generated report revision and change timestamp as one value.
Remote reports live separately in `scheduled_send_summaries`; they contain
no executable send payload.

Use a stable public group identifier or fingerprint, never key material, for
the visibility binding. A schedule created outside a group stays local in
this release. Do not automatically publish it when joining later. Re-enabling
sync in the same group publishes missed owner updates; joining a different
group must not publish old schedules there.

The queue transaction validates the draft revision, provider send support,
sender, time selection, and attachments; freezes the content; removes the
editable draft; and creates the schedule and its initial replicated report
when sharing is active. A publication failure does not lose the local
schedule. Surface that its visibility is waiting for sync.

Rescheduling updates the target and report atomically, with an expected
revision and a pre-delivery state check. Returning to draft uses the existing
transactional restore path. Editing then scheduling again creates a new
operation identity. Preserve reply headers, attachment retention, sender
account isolation, and send-and-archive behavior.

## Local state machine and recovery

```text
scheduled -> sending -> sent | failed | uncertain
scheduled -> overdue
scheduled / overdue -> canceled + restored draft
scheduled / overdue -> scheduled         (explicit reschedule)
overdue -> undo_pending -> sending       (explicit Send now)
uncertain -> sent | unverifiable         (existing provider reconciliation)
```

Blocked conditions such as missing authorization or mismatched ownership are
reported separately and prevent claims; they do not imply delivery failure.
An account disconnect pauses affected scheduled work using the existing
account-isolation rules. Reconnection does not automatically release an
overdue message. Permanent rejection remains recoverable only on the owner.

Claiming `sending`, incrementing attempts, and recording its summary must
commit together before provider submission. Committing the provider result
and its summary must also be atomic. If the provider may have accepted a
message but the local commit fails, keep the existing uncertain recovery
behavior. Neither a stale summary nor a missing Sent search result authorizes
a resend.

Startup preserves future schedules, marks elapsed unclaimed schedules overdue,
and marks interrupted `sending` rows uncertain. Resume applies the same
missed-time policy before ordinary dispatch. Quit waits for active delivery
and ordinary undo windows, but leaves future/overdue schedules saved and exits.

Sync outages never prevent otherwise eligible owner delivery. Turning sync
off or leaving the group stops visibility updates without transferring or
canceling local work. Revocation is not remote cancellation: an offline
computer may still send with its own mail credentials. Peers label the last
status accordingly. Removing the mail account locally pauses its scheduled
work; synchronized account deletion takes effect on a computer when received.

## Replicated summaries

Append a `ScheduledSendSummary` entity to `EntityType` without changing the
canonical ordering of existing variants. Namespace its ID by owner and
operation ID. Use one coherent `report` field containing:

| Field | Purpose |
| --- | --- |
| `operationId`, `ownerInstallationId` | Stable identity and ownership label |
| `ownerSyncDeviceId`, `ownerNameAtCreation` | Roster name lookup and fallback |
| `account`, `subject` | Identify the outgoing message |
| `scheduledAt`, `timeZone` | Display the chosen schedule |
| `state`, `blockedReason` | Last reported delivery state and safe reason code |
| `reportRevision`, `statusChangedAt` | Report sequence and owner change time |

The UI should explain that account, subject, timing, and delivery status are
shared encrypted metadata. Exclude all recipients (including Bcc), body text,
HTML, attachment data/names/paths, raw MIME, provider tokens, and raw errors
that could expose content or server details. Validate an explicit field
allowlist, types, bounded strings, time ranges, identifiers, and reason codes.

Only local owner commands and worker transitions author reports. Replication
projects reports into a separate summary table; it never inserts drafts or
sendable outbox rows. Commands and the worker authorize against local outbox
ownership, never against a merged report. A whole-report value avoids merging
a new scheduled time with an old status. Conflicts, stale snapshots, relaying,
and the generic conflict resolver cannot change delivery authority. Preserve
the existing trusted-member sync model; do not claim cryptographic enforcement
of original authorship from the outer snapshot signer, since peers relay data.

Extend backlog reconciliation to republish owner summaries from local truth,
including transitions made while replication was disabled. Never reauthor a
remote report. Retain terminal reports as history in this release, matching
the existing outbox retention behavior; unresolved outcomes must not expire.

Summary display must not depend on local account authorization or arrival of
the roster/account entity first. Resolve names when available; otherwise use
the creation label. On the owner, combine entries by operation identity so
the same message appears once and local authoritative state wins. On peers,
show the report change time and existing last-contact information separately.
An old report is not proof that the sending computer is offline or that the
message remains unsent. If its time passed without a new report, display
**Scheduled time passed; awaiting an update from MacBook**, not a fabricated
delivery result.

## UI and command behavior

- **Send Later** is available beside Send, in the command palette, and through
  **Command+Shift+L** on macOS (**Ctrl+Shift+L** elsewhere) while composing.
  The shortcut opens the picker; it does not queue or send the message.
  Flush autosave before scheduling and retain all existing compose checks and native validation.
- The picker shows **Sends from this computer (MacBook)**, its timezone, and
  the requirement to keep ThreeStrands running and the computer awake.
- Outbox shows local schedules and remote summaries. Local pre-delivery rows
  offer reschedule, cancel/restore, and overdue Send now. Remote rows show
  **Manage on MacBook** without mutation controls or a content preview.
- Separate schedule status (`Scheduled` / `Scheduled time missed`) from
  visibility status (`Saved on this computer`, `Published to sync storage`,
  `Visibility sync paused`, or `Waiting to sync visibility`). Ordinary
  immediate sends remain local and do not acquire summary records.
- Show explicit uncertain/unverifiable outcomes. Remote summaries do not
  trigger local sent-mail refreshes, queued-reply previews, follow-up completion,
  archive actions, or provider reconciliation.
- Keep counts consistent, deduplicate owner rows, and preserve keyboard scopes,
  Escape behavior, focus restoration, save-on-close, and accessible announcements.
  Extend the demo client to simulate remote summaries and controlled time.

## Compatibility and verification

- All devices sharing `EntityType::ScheduledSendSummary` must run **0.98.1 or
  later**; older snapshots import cleanly, while older readers reject newer
  entity types without partial merges.
- Follow `AGENTS.md` for testing (`pnpm test`, `pnpm test:e2e`, `cargo test` in
  `src-tauri`, and `sync-protocol` tests) and changelog updates (`changes/` +
  `pnpm release`; never bump versions manually). Live-account and sleep/wake
  checks stay with the developer.
