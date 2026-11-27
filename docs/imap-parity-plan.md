# IMAP parity and Gmail fallback

**Status:** Stages 0 and 1 complete; Stage 2 complete except account-wide keyword
rename/removal; Stage 3 initial conversion (**Switch to Gmail via IMAP…**)
complete; Stage 4 partially complete (single-pass flag/label fetch landed).
This supplements [the IMAP design](imap-design.md) and summarizes shipped
extension behavior in [Gmail IMAP extensions](imap-gmail-extensions.md). The
[Gmail connection implementation plan](gmail-connection-plan.md) sequences
this IMAP work and the later organization-supplied OAuth option.

Progress: identity fixtures, persistent message/thread identity mappings
(`X-GM-MSGID`, `X-GM-THRID`), and legacy Gmail thread repair are implemented.
[All Mail, archive, and search completeness](imap-gmail-extensions.md#all-mail-archive-and-search-completeness)
is shipped, along with native label creation, apply/remove, hierarchy rename/delete,
plain IMAP folder-tree renames, and keyword apply/remove. **Settings → Mail
Accounts → Switch to Gmail via IMAP…** converts an existing Gmail API account
in place per device.

The Gmail app-password setup route reuses the IMAP/SMTP connection test
and mailbox review. IMAP can verify interrupted sends against a unique
server-generated Sent copy and supports local default sending addresses
and Reply-To settings.

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

- [x] Trace message/thread IDs through IMAP acquisition, deduplication, thread
  aliases, tasks, goals, snoozes, reader state, drafts, queued mutations/sends,
  and encrypted replication. Record each reference and its migration behavior.
- [x] Decide how advertised `X-GM-MSGID` becomes an external identity mapping
  without invalidating existing header-derived IMAP IDs: prefer stable internal
  IDs with external mappings over changing primary keys on reconnect.
  Implemented in [identity mappings](imap-gmail-extensions.md#message-and-thread-identity-mappings).
- [x] Map advertised `X-GM-THRID` conversation identities for acquired IMAP
  mail. Use the binding for reply drafts and snoozed conversation lookup.
- [x] Recheck retained UIDs on Gmail and repair verified legacy
  header-derived splits and merges. Keep stable message IDs, move links tied
  to those messages, and retain a thread alias for a proven whole merge.
  [Gmail thread repair](imap-gmail-extensions.md#legacy-gmail-thread-repair)
- [x] Define ambiguous identity handling: no subject-only matches and no silent
  reassociation when Message-ID headers are missing or duplicated. Preserve and
  expose unresolved references rather than deleting workflow data.
- [x] Add fixtures for a plain IMAP server and an extension-capable server,
  overlapping folders, missing/duplicate Message-ID headers, late replies, and
  UIDVALIDITY changes. All subsequent stages reuse these contracts.

Acceptance: enabling an extension or reconnecting cannot silently change which
message an existing workflow record refers to. This is a design and test
foundation, not a claim that all threading enhancements ship in this stage.

### 1. Complete archive and search access

Implemented behavior and validation are recorded in
[All Mail, archive, and search completeness](imap-gmail-extensions.md#all-mail-archive-and-search-completeness).
Ordinary physical-folder moves remain distinct from negotiated label
membership operations.

- [x] Design aggregate mailbox acquisition and reconciliation without treating
  All Mail as an additional user label or downloading each body per location.
- [x] For advertised label extensions, archive removes Inbox membership while
  preserving other labels. Keep ordinary Archive-folder moves for plain IMAP.
- [x] Define correct trash, spam, unarchive, undo, and snooze-return behavior
  across both models. Removing a folder membership must not imply global deletion.
- [x] Make default search cover relevant server mail, including archive-only and
  folder-only messages. Deduplicate hits; retain explicit Trash/Spam exclusions.
  Support advertised `X-GM-RAW` with a generic IMAP search fallback.
- [x] Surface incomplete coverage, unsupported terms, deadlines, and acquisition
  limits. Keep bounded acquisition and retention; completeness of search access
  does not require downloading the entire mailbox.

Acceptance: archive-only messages are found; archive survives restart and sync
from another client; other labels remain intact; search has no duplicate rows.
Tests cover both server models, partial results, offline cached results, undo,
and negative cases where a label removal must not delete mail.

### 2. Make labels and folders usable

Existing labels on servers advertising `X-GM-EXT-1` can now be applied and
removed without a configured container. Their existing IDs are preserved.
Gmail label renames carry nested labels with them, and deleting a parent removes
only the selected label. Plain IMAP can create physical folders, rename folder
trees, and safely delete individual folders without children. Label-folder storage requires a configured
container. Keyword storage supports creating, applying, and removing valid
keywords on mailboxes that permit them.

- [x] Read/write advertised `X-GM-LABELS` and expose existing labels directly.
- [x] Apply/remove existing native labels with verified, additive membership
  writes; preserve unrelated labels and reconcile external membership changes.
- [x] Create native labels, including hierarchy and non-ASCII names, with live
  verification and retry-safe publication.
- [x] Rename and delete individual native Gmail labels. Keep a stable local
  identity for renames, preserve deleted rules for explicit relinking, and
  recover acknowledged edits through a durable local journal. Encrypted settings
  export carries the identity bindings.
- [x] Rename native Gmail label hierarchies while preserving the IDs and
  Split Inboxes for each verified descendant. Delete a parent label while
  confirming that the nested labels are still present. See [native label edits](imap-gmail-extensions.md#editing-label-hierarchies).
- [x] Coordinate verified ID bindings across enrolled devices with one
  designated editor per account. A verified external disappearance or
  UIDVALIDITY change can leave a label unresolved for explicit relinking.
  [Cross-device edits](imap-gmail-extensions.md#cross-device-edits)
- [x] Create ordinary folders and rename or delete leaf folders and label
  folders with stable references. Reuse the edit journal to preserve Split
  Inboxes and retire deleted IDs so a new folder cannot inherit a saved rule.
- [x] In keyword mode, use verified additive or subtractive flag writes only
  when the selected mailbox advertises permanent support. Keep keyword
  membership separate from mailbox moves.
- [x] Show physical folder management separately from applying and removing
  labels. Require a live surviving copy for every message before deleting a
  physical folder and verify that its complete UID set is accounted for.
- [x] Extend verified rename to plain IMAP folder trees and coordinate their
  bindings across devices. Keep physical deletion limited to leaf folders.
- [ ] Add keyword rename and account-wide removal with resumable per-message
  reconciliation. Verify new membership before removing the original keyword,
  check every selectable mailbox that can contain it, and report coverage
  gaps and partial results. See [keyword scope](imap-gmail-extensions.md#keyword-edits).

Acceptance: labels created or changed elsewhere reconcile correctly; renaming
does not orphan saved rules or duplicate mail; failures roll back coherently.
No writable capability is offered unless the provider can fulfill it.

### 3. Offer safe connection conversion

**Switch to Gmail via IMAP** is the first conversion path. It tests both
connections, reviews label-based Split Inboxes, then changes the Gmail account in
place. It keeps task IDs, goal IDs, account preferences, sender rules, signature
defaults, and cached sending addresses. A native Gmail ID reservation reconnects
mail-linked tasks as mail arrives. Opening an older task conversation can
recover it by native ID outside the usual sync window. The review flags legacy
task references and lets the user choose a label for any unmatched Split Inbox.

Each device can switch independently while staying in Replicated Sync.
It keeps the shared account, rules, and snoozes. For each label-based Split
Inbox, it uses the label confirmed on that device. If a peer creates a rule
using the other provider or changes its underlying label, choose a matching
label in Split Inboxes on this device. A replacement chosen for a rule from
another provider stays local.

The switch rebuilds the local mail cache and clears drafts, Outbox items,
and pending mail writes on this device. It cancels unsent schedules owned by
this installation while preserving the reports and delivery authority of
other devices. Any future mail acquired with verified native Gmail IDs keeps
the same message and thread IDs as the Gmail API on converted devices. A
snooze with an older provider-local ID stays recorded and can be acted on by
the connection that recognizes it.

The remaining checklist covers a fuller, resumable conversion that also
preserves drafts, outbox/queued/scheduled sends, delivery receipts, pending
mutations, and device-local reader/UI state (`readerPresentation.ts`,
`readingPositions.ts`, `returnNavigation.ts`, `useReaderState.ts`), as well as
cached AI/triage/unsubscribe references before clearing old threads:

- [x] Provide a per-account connection-change entry point (**Switch to Gmail via
  IMAP…**), validating IMAP and SMTP before retiring the existing connection and
  keeping Calendar authorization independent.
- [ ] Preserve drafts, queued/scheduled sends, pending mail mutations, delivery
  receipts, and local reader presentation across conversion; resolve references
  using stage 0 mappings and report unresolved links explicitly.
- [ ] Freeze account mutations and sending during conversion; preserve delivery
  receipts and uncertain sends so switching cannot trigger duplicate delivery.
- [ ] Define a resumable conversion journal and failure recovery. Keep the old
  connection usable until conversion commits; credential revocation must not
  make an interrupted conversion unrecoverable.
- [x] Allow enrolled devices to switch one at a time. Keep the installed provider
  and the reviewed label bindings local. Settings export carries those bindings
  with backward-compatible defaults.
- [ ] Verify retention and replay of drafts, queued writes, and delivery receipts
  for a future conversion that preserves the local outbox.

Acceptance: conversion survives interruption and restart without workflow loss,
wrong-message references, duplicate sends, or cross-device provider conflicts.
Test both enrolled and non-enrolled installations, pending drafts/sends, and
exports from the preceding schema if transfer fields change.

### 4. Validate and improve sync efficiency

Measure throughout stages 1–3; this final stage closes observed regressions.

- [x] Avoid fetching flags and labels twice on initial acquisition. Verify
  command counts and flag reconciliation on a subsequent poll.
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
| Sending identities | Manual addresses, default From, Reply-To, and reply address selection are available. Automatic Gmail Send mail as discovery still requires the API. Check alias acceptance in the live Gmail SMTP pilot. |
| Unresolved conversation evidence | Cached groups with incomplete or contradictory Gmail evidence stay available with their existing links. A repair also waits for outstanding conversation-wide work to settle. [Repair and deferral rules](imap-gmail-extensions.md#legacy-gmail-thread-repair) |
| Gmail signature import over IMAP | Implemented for Gmail API connections (`gmail_signatures::import_gmail_signature`); optional convenience over IMAP remains unbuilt. Existing ThreeStrands signatures and defaults stay usable during conversion. |
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
leave Tart branches ready for host landing.

## References

- [Google IMAP extensions](https://developers.google.com/workspace/gmail/imap/imap-extensions)
- [Google IMAP OAuth scopes](https://developers.google.com/workspace/gmail/imap/xoauth2-protocol)
- [App-password eligibility](https://support.google.com/mail/answer/185833?hl=en)
- [Thunderbird Gmail archive and folder behavior](https://support.mozilla.org/en-US/kb/thunderbird-and-gmail)
- [Apple Mail mailbox mapping](https://support.apple.com/guide/mail/cpmlprefacctmbox/mac)
