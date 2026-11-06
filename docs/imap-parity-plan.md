# IMAP parity and Gmail fallback

Status: proposed implementation sequence, recorded 2026-11-06. No runtime
changes are implemented by this document. Update the checklist as work lands.
This supplements [the IMAP design](imap-design.md), whose historical phase
status is not a complete description of today's implementation.

## Goal and scope

Make IMAP a dependable alternative if Gmail API authorization is unavailable,
without losing existing ThreeStrands workflow data. Cover ordinary IMAP servers
as well as servers advertising Gmail extensions. Retain the local-first model,
separate calendar connection, current rendering boundary, and safe retry rules.

Use advertised capabilities and mailbox attributes, never hostname or brand
checks, to select protocol behavior. An extension-specific adapter is acceptable
when the server advertises that extension; ordinary IMAP must keep working.

Authentication is a separate constraint: the current password/app-password path
is the fallback assumed here. Gmail IMAP OAuth requires the full
`https://mail.google.com/` scope and verification; implementing XOAUTH2 does not
remove the approval dependency. Some accounts cannot use app passwords. Do not
promise universal Gmail access or add Google scopes as part of this work.

## Essential implementation sequence

Each stage should be a separate reviewable change, split further when necessary.
Keep existing Gmail connections working throughout. Do not ask users to remove
and re-add accounts to test the new path.

### 0. Establish identity and compatibility contracts

- [ ] Trace message/thread IDs through IMAP acquisition, deduplication, thread
  aliases, tasks, goals, snoozes, reader state, drafts, queued mutations/sends,
  and encrypted replication. Record each reference and its migration behavior.
- [ ] Decide how advertised `X-GM-MSGID` and `X-GM-THRID` become external identity
  mappings without invalidating existing header-derived IMAP IDs. Prefer stable
  internal IDs with external mappings over changing primary keys on reconnect.
- [ ] Define ambiguous identity handling: no subject-only matches and no silent
  reassociation when Message-ID headers are missing or duplicated. Preserve and
  expose unresolved references rather than deleting workflow data.
- [ ] Add fixtures for a plain IMAP server and an extension-capable server,
  overlapping folders, missing/duplicate Message-ID headers, late replies, and
  UIDVALIDITY changes. All subsequent stages reuse these contracts.

Acceptance: enabling an extension or reconnecting cannot silently change which
message an existing workflow record refers to. This is a design and test
foundation, not a claim that all threading enhancements ship in this stage.

### 1. Complete archive and search access

Current gaps: `plan.rs` excludes `\All`; default server search selects Inbox,
Archive, and Sent. Existing archived mail available only through All Mail can
therefore be absent. Ordinary IMAP archive moves also need distinct handling
from a server's aggregate/label model.

- [ ] Design aggregate mailbox acquisition and reconciliation without treating
  All Mail as an additional user label or downloading each body per location.
- [ ] For advertised label extensions, archive removes Inbox membership while
  preserving other labels. Keep ordinary Archive-folder moves for plain IMAP.
- [ ] Define correct trash, spam, unarchive, undo, and snooze-return behavior
  across both models. Removing a folder membership must not imply global deletion.
- [ ] Make default search cover relevant server mail, including archive-only and
  folder-only messages. Deduplicate hits; retain explicit Trash/Spam exclusions.
  Support advertised `X-GM-RAW` with a generic IMAP search fallback.
- [ ] Surface incomplete coverage, unsupported terms, deadlines, and acquisition
  limits. Keep bounded acquisition and retention; completeness of search access
  does not require downloading the entire mailbox.

Acceptance: archive-only messages are found; archive survives restart and sync
from another client; other labels remain intact; search has no duplicate rows.
Tests cover both server models, partial results, offline cached results, undo,
and negative cases where a label removal must not delete mail.

### 2. Make labels and folders usable

Current gaps: writable IMAP labels require a configured container; label-folder
rename is disabled; keyword writes are not implemented. Existing Gmail labels
are exposed as folders rather than native writable labels.

- [ ] Read/write advertised `X-GM-LABELS` and expose existing labels directly.
- [ ] Support label create, rename, apply, remove, and delete, including hierarchy
  and non-ASCII names. Preserve Split Inbox references and other saved label
  references through rename; refresh changes made in other clients.
- [ ] For plain IMAP, complete folder management and label-folder rename with
  stable references. Implement keyword writes only when permanent flags permit
  them; otherwise describe the available folder behavior clearly.
- [ ] Keep removing a label, deleting a folder, moving mail, and trashing a message
  distinct in the provider contract and UI. Specify what happens to the last
  remaining location before implementing destructive actions.

Acceptance: labels created or changed elsewhere reconcile correctly; renaming
does not orphan saved rules or duplicate mail; failures roll back coherently.
No writable capability is offered unless the provider can fulfill it.

### 3. Offer safe connection conversion

Current removal paths purge cached mail and, depending on sync enrollment and
removal scope, can also delete workflow records. Removal is not conversion.

- [ ] Add an explicit change-connection operation for the same mailbox/account.
  Validate IMAP and SMTP before retiring the existing connection; keep Calendar
  authorization independent.
- [ ] Preserve tasks, goals, snoozes, Split Inboxes, sender rules, signatures,
  drafts, queued/scheduled sends, mail links, and available reader presentation.
  Resolve references using stage 0 mappings; report unresolved links explicitly.
- [ ] Freeze account mutations and sending during conversion; preserve delivery
  receipts and uncertain sends so switching cannot trigger duplicate delivery.
- [ ] Define a resumable conversion journal and failure recovery. Keep the old
  connection usable until conversion commits; credential revocation must not
  make an interrupted conversion unrecoverable.
- [ ] Define behavior when another enrolled device still uses the old provider.
  Ensure conversion cannot bounce account metadata between providers or replay
  mutations under the wrong ID scheme. Preserve settings-transfer compatibility.

Acceptance: conversion survives interruption and restart without workflow loss,
wrong-message references, duplicate sends, or cross-device provider conflicts.
Test both enrolled and non-enrolled installations, pending drafts/sends, and
exports from the preceding schema if transfer fields change.

### 4. Validate and improve sync efficiency

Measure throughout stages 1–3; this final stage closes observed regressions.

- [ ] Add repeatable scripted-server workloads and record command counts, fetched
  body bytes, duplicate acquisitions, and foreground progress. Establish numeric
  budgets before optimizing; do not claim API parity without measurements.
- [ ] Cover large All Mail, many overlapping labels, several accounts, cold
  import, no-change sync, bulk flag changes, reconnect, sleep/wake, and mailbox
  reset. Inbox work must continue while older mail is acquired in bounded rounds.
- [ ] Implement incremental flag/deletion extensions such as CONDSTORE/QRESYNC
  where measured value warrants it. Keep the baseline path correct when absent.
- [ ] Verify IDLE reconnect/renewal, connection limits, backoff, cancellation, and
  cache bounds. Fetch each body once per stable message unless invalidated.

Acceptance: recorded budgets pass for baseline and extension paths, no-change
sync does not fetch bodies, and history import cannot starve Inbox updates.

## Remaining gaps to retain

| Gap | Follow-up and priority |
| --- | --- |
| Sending identities | Audit existing manual identities before adding anything. Close missing default From, Reply-To, addressed-reply selection, display names, and per-address signature behavior. Important after essentials; automatic Gmail alias discovery may remain unavailable without API access. |
| Conversation grouping | Use advertised server thread IDs under stage 0's identity contract; retain conservative reference-header threading elsewhere. Exact Gmail grouping is desirable, not required for general IMAP correctness. |
| Uncertain SMTP delivery | Where the server saves Sent copies, look up the outgoing Message-ID and require adequate evidence before declaring success. A missing copy does not prove non-delivery. Never automatically resend an ambiguous send. |
| Gmail signature import | Optional convenience. Existing ThreeStrands signatures and defaults should remain usable during conversion. |
| IMAP OAuth | Separate authentication project. Useful for broader provider compatibility but not an escape from Google's scope verification. |

## Validation and delivery

Use Rust unit/scripted-server tests for protocol, identity, search, and recovery;
hook/component tests for settings and conversion UI; slim App tests only for
wiring. Include regression tests for existing contracts and intentionally update
changed contracts without dropping assertions. Run `cargo test` in `src-tauri`
for Rust changes, `pnpm test` for frontend changes, and `pnpm test:e2e` for
compose, triage, or email-rendering changes. Follow AGENTS.md for workspace and
dependency checks. Developers own hands-on app validation.

Each user-visible slice gets a pending `changes/` entry and changelog validation;
do not change application versions. Work in isolated branches/worktrees and
leave Tart branches ready for host landing. This planning-only change needs no
release note.

## References

- [Google IMAP extensions](https://developers.google.com/workspace/gmail/imap/imap-extensions)
- [Google IMAP OAuth scopes](https://developers.google.com/workspace/gmail/imap/xoauth2-protocol)
- [App-password eligibility](https://support.google.com/mail/answer/185833?hl=en)
- [Thunderbird Gmail archive and folder behavior](https://support.mozilla.org/en-US/kb/thunderbird-and-gmail)
- [Apple Mail mailbox mapping](https://support.apple.com/guide/mail/cpmlprefacctmbox/mac)
