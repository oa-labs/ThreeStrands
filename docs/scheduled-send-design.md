# Scheduled send with synchronized visibility

Status: proposed implementation plan (2026-10-10). Not implemented.

## Product contract

Each scheduled message belongs to the computer where it was scheduled. Only
that installation can submit it. Other computers in the same sync group can
see a summary and the last reported status. Message bodies, recipients,
attachments, drafts, and account credentials remain local.

ThreeStrands must be running on the sending computer, which must be awake,
connected, and authorized to send. The scheduled time means an attempt to
submit the message, not a guarantee of recipient arrival. Another computer
never takes over automatically.

The first release supports scheduling with or without replicated sync.
Visibility on other computers requires an enrolled sync group and eventually
working connectors. Gmail can ship first; IMAP accounts become eligible only
when their SMTP submission implementation is complete. Do not accept a
schedule for a provider that cannot send. Add a provider-neutral send-support
capability rather than probing `prepare_delivery` when creating a schedule;
scheduling an otherwise supported account can work offline.

On the sending computer, users can cancel, reschedule, or return the message
to an editable draft until delivery starts. On other computers, entries are
read-only and say which computer owns them. Remote cancellation requests,
ownership transfer, automatic failover, background helpers, and server-side
scheduling are separate features.

## Existing foundations and required changes

- `src-tauri/src/correspondence.rs` already freezes MIME into a durable outbox,
  protects duplicate queue submissions with `(draft_id, revision)`, and claims
  delivery with a conditional SQL update. Its worker runs independently of
  mailbox polling and distinguishes definite rejection from uncertain delivery.
- `src-tauri/src/schema.rs` currently resets every `undo_pending` deadline on
  startup. Scheduled timestamps must survive recovery unchanged.
- `finish_exit` in `src-tauri/src/lib.rs` waits for pending undo windows.
  A scheduled message must not keep the app open until its scheduled time.
- Replicated sync carries selected entities through `sync-protocol`,
  `sync_state.rs`, and `sync_projection.rs`. It currently excludes queued mail.
  Add a summary entity, rather than replicating the outbox table.
- `src/correspondence.ts`, `src/useCorrespondence.tsx`, `src/Composer.tsx`, and
  `src/data/demoCorrespondence.ts` own the existing compose/outbox contracts.
  Keep summaries separate from `OutboxItem`, which contains a complete draft.

## Scheduling and missed times

The picker accepts a future date, time, and explicit IANA timezone. Resolve
the selection in Rust and persist the UTC instant plus the original timezone.
Reject nonexistent daylight-saving times and require a choice of offset for
ambiguous times. Changing the computer's timezone later does not move the
scheduled instant. Show the original time and zone, with a local equivalent
on computers using another zone.

Use a proposed 60-second dispatch grace period. A continuously running worker
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

Extend `outbox_messages` additively with nullable fields for:

- `scheduled_at`: UTC epoch milliseconds; null for ordinary sends.
- `scheduled_time_zone`: the original IANA timezone.
- `owner_installation_id`: immutable for scheduled messages.
- `visibility_group_id`: the original sync group, if sharing was enabled.
- `report_revision` and `status_changed_at`: owner-generated status sequence
  and timestamp, advanced together on every relevant transition.

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

- Add **Send later** beside Send and to the command palette. Flush autosave
  before scheduling and retain all existing compose checks and native validation.
- The picker shows **Sends from this computer (MacBook)**, its timezone, and
  the requirement to keep ThreeStrands running and the computer awake.
- Outbox shows local schedules and remote summaries. Local pre-delivery rows
  offer reschedule, cancel/restore, and overdue Send now. Remote rows show
  **Manage on MacBook** without mutation controls or a content preview.
- Separate schedule status from visibility status: **Scheduled on this
  computer; waiting to sync** is a valid combination. Ordinary immediate sends
  remain local and do not acquire summary records.
- Show explicit uncertain/unverifiable outcomes. Remote summaries do not
  trigger local sent-mail refreshes, queued-reply previews, follow-up completion,
  archive actions, or provider reconciliation.
- Keep counts consistent, deduplicate owner rows, and preserve keyboard scopes,
  Escape behavior, focus restoration, save-on-close, and accessible announcements.
  Extend the demo client to simulate remote summaries and controlled time.

## Compatibility and delivery sequence

1. **Persistence and contracts.** Add the installation identity, additive
   migration, scheduling policy, native/client models, and demo support. Old
   drafts and outbox rows retain their existing behavior. Ownership and status
   reports are excluded from settings exports; keep the export format unchanged.
2. **Native scheduler.** Implement queue/reschedule/cancel, conditional claims,
   recovery/resume/exit behavior, and atomic owner report recording. Complete
   deterministic delivery and crash coverage before exposing real scheduled send.
3. **Visibility replication.** Add the summary entity, strict validation,
   projection, group binding, backlog repair, and combined read API. Verify
   two-device isolation and failed/reordered publication behavior.
4. **Composer and Outbox.** Add the picker, local management, peer visibility,
   timing and freshness wording, palette actions, and demo fixtures. Preserve
   ordinary sending and existing correspondence side effects.
5. **Release.** Update cross-device sync's data boundary, correspondence status,
   and user guidance. Add a minor SemVer bump at implementation time across
   `package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`, and the
   `threestrands` entry in `src-tauri/Cargo.lock`. This plan alone needs no bump.

New sync entity variants currently make snapshots unreadable to older builds.
Use the existing documented update-all-devices behavior, preserve old snapshot
reading in new builds, and provide an actionable compatibility message. Test
the old reader's rejection before any partial merge. Do not reset groups or
silently discard unknown entities to ship this feature. Document the minimum
supported version when the implementation version is assigned.

## Automated acceptance coverage

Extend the existing correspondence, migration/recovery, sync, composer, and
Outbox test suites rather than replacing their contracts.

| Area | Required cases |
| --- | --- |
| Timing | Before/exactly at/after target; exact grace boundary and beyond; authorization crossing the boundary; clock jumps; future and elapsed restart/resume; overdue confirmation |
| Timezones | DST gap and repeated time; selected offset; travel/system zone change; peer display in a different zone |
| Ownership | Two running computers produce exactly one provider call; remote summary never creates local payload; forged command owner rejected; missing/mismatched identity; restored SQLite on another installation |
| Races | Repeated schedule submits once; reschedule/cancel versus claim; stale revisions; stop before dispatch, after acceptance, and before local acknowledgement |
| Uncertainty | No automatic resend; provider-specific reconciliation and unverifiable outcomes preserved; a reported success never implies recipient delivery |
| Persistence | Previous-schema exports/drafts/outbox fixtures unchanged; future timestamp preserved; ordinary undo grace preserved; scheduled work does not block exit; referenced attachments retained |
| Sync | Two-device create/reschedule/cancel/send reports; peer without credentials/account metadata; delayed, repeated, reordered, relayed snapshots; conflicts cannot grant sending permission; later owner update supersedes stale history |
| Sync lifecycle | Connector failure during delivery; atomic local/report failure; disable/re-enable; leave/rejoin with new roster ID; different-group privacy; revoked/removed peer with stale status |
| Data boundary | Reject extra fields and invalid values; no body/HTML/recipients/Bcc/attachment locators/raw errors/credentials in summary, snapshot, logs, or settings transfer |
| UI | Save before schedule; picker and palette keyboard flows; local versus peer controls; deduplication/counts; stale-status wording; ordinary Send/undo/reply previews/follow-up/archive behavior unchanged |
| Compatibility | New build imports preceding snapshot and schema; preceding reader rejects new entity without partial merge; update guidance; retained terminal reports |

Run `pnpm test`, `pnpm test:e2e`, `pnpm build`, `cargo test` in `src-tauri`,
and the affected sync workspace crate tests. Use fake providers, temporary
databases/connectors, controlled clocks, and failure injection; do not send
real email as part of agent validation. The developer owns running-app and
look-and-feel checks, per `AGENTS.md`.
