# Meeting preparation and follow-through

**Status:** Proposed implementation plan; no feature code implemented.
**Baseline:** 0.210.0 repository, reviewed 2026-11-15; daily-loop entry points
added 2026-11-16.
**Priority:** Next feature in the [product plan](../PLAN.MD#recommended-sequence-and-next-feature),
delivered alongside a first daily plan and shutdown.

## Outcome and scope

Open an upcoming meeting and see the relevant notes, conversations, and open
commitments without assembling them across workspaces. After the meeting, review
its decisions and actions, keep them in the linked note, and prepare an email
through the normal composer. Every source remains reachable and every write is
an explicit user action.

Preparation and follow-through are part of the user's day, not only buttons on an
event. The product plan pairs this feature with a guided **Plan Today** routine
and a **Shutdown** in a reshaped Today view: the plan's meeting step shows each of
the day's meetings with its readiness and opens preparation, and Shutdown lists
the day's meetings still awaiting follow-through. This design owns the meeting
contracts those steps use. The routine itself (where the plan is stored, its
other steps, carry/defer/drop, and Today's layout) needs its own design and is not
specified here. Every meeting entry point must still work without the routine.

Deliver a useful local workflow first, then optional, on-demand AI assistance.
The complete first version includes preparation, reviewed note/task capture,
reviewed follow-up drafting, and optional synthesis. Each stage below must work
without the later stages. Do not describe AI synthesis as available after only
the local stages land.

Recording, transcription, background briefs, autonomous sending, automatic calendar
changes, a full relationship timeline, workspace-wide semantic search, and hosted
execution are outside this implementation. Long-series incremental summaries from
the Notes sketch are later work; the first AI request uses a bounded selection.

## Existing contracts to reuse

| Concern | Current implementation and constraint |
| --- | --- |
| Event identity | `ScheduleEvent` in `src/domain.ts` and `meetingEventReference` in `src/meetingNoteEntry.ts` identify an occurrence by account, calendar, and event ID, with a separate series ID. An iCalendar UID or matching title alone is insufficient. |
| Existing meeting note | `EventNoteAction`, `EventNoteBacklinks`, and `primaryMeetingNote` distinguish occurrence notes from series history. `App.tsx` rechecks backlinks on activation before offering Add Note. Keep that behavior and one primary destination. |
| Meeting capture | `src/noteMeetings.ts`, `src-tauri/src/note_meetings.rs`, and `src-tauri/src/db/note_entities.rs` insert the daily meeting line transactionally, preserve Markdown, and refuse unsafe/stale writes. A title-based reusable note is an explicit existing choice, not proof that unrelated events belong to one series. |
| Unsaved notes | `src/noteMutations.ts` coordinates editor flushes and local recovery drafts. `useNoteEditor` owns save/conflict handling. Preserve authored titles and never overwrite a pending title edit. |
| Tasks | Round `( )` items in saved Markdown project into Tasks; square `[ ]` checklists stay note-local. Note tasks have no mail account and do not support every standalone task field. No second synchronized task record should represent a note task. |
| Drafts | `src/useCorrespondence.tsx` owns composition, autosave, provider drafts, and safe composer replacement. Its `replyWithText` currently uses the visible thread; it is not a general event-to-thread drafting API. |
| Navigation | `useWorkspaces`, acknowledged Notes requests, and App navigation preserve targets. Sidebar transitions save a draft before replacing its composer. Failed saves retain the current work. |
| AI | `src-tauri/src/commands/ai.rs` loads native data; `src-tauri/src/ai/` owns provider requests, structured output, and usage recording. AI results are proposals. Notes AI and per-note exclusion are not implemented yet. |

## User experience

### Prepare during daily planning

Plan Today's meeting step lists the day's meetings, excluding declined events, in
time order. Its evening variant lists tomorrow's. Each row shows the meeting's
readiness from the batch summary below, without loading message or note bodies:

- whether this occurrence has a linked note, and whether earlier occurrences of
  the exact series have notes;
- the number of open commitments linked through those notes or the event's
  originating conversation, with candidate counts kept separate;
- the date of the most recent cached conversation with an attendee in the
  meeting's mail scope, or that the scope needs choosing.

Row actions are **Prepare** (opens the meeting view below), **Open Note / Add
Note**, **Reserve Prep Time**, and **No Prep Needed**. Readiness is a description,
not a score: a meeting with no notes is not marked as failing.

**Reserve Prep Time** adds a round task, "Prepare for <meeting title>", nested under
the meeting's line in today's daily note, using the existing meeting capture to
add the line first if it is missing. The user can then book the task before the
meeting with existing task time blocking. Both steps are explicit. Repeating the
action finds the existing prep task under that occurrence's line and offers it
instead of adding a second one. Prep tasks are ordinary note tasks with their
current completion semantics.

**No Prep Needed** hides the meeting from the step's outstanding count for that day
on this device, like a dismissed promise, and can be undone. It never changes the
event or its notes.

### Prepare from Calendar or Today

Add **Prepare** to timed and all-day event details in Calendar and to event actions
in Today's agenda. A command-palette action is available when a concrete event is selected.
Do not allocate a new single-letter shortcut in v1. Meetings without attendees can
still use their own notes; writable-calendar access is not required for reading
context or writing a local note.

Prepare opens a read-only meeting view within the current workspace, using its
existing detail/side-panel surface rather than another permanent panel or a new
primary workspace. Show the event title, date/time, account/calendar, and attendees,
then the following sections using shared `ContextSection` and `ContextRow` patterns:

1. **This meeting:** the occurrence's linked note and daily-note meeting excerpt.
2. **Previous meetings:** earlier occurrences from this exact series, newest first,
   with their linked excerpts. Separate other series context from prior occurrences;
   do not describe future meetings as history.
3. **Conversations:** the event's originating message, if cached, followed by recent
   attendee-related conversation candidates. Explain why each candidate appears.
4. **Open commitments:** tasks in the selected meeting material and tasks linked
   to selected conversations. Attendee-related tasks are clearly marked candidates,
   not asserted meeting commitments.

Open source records using existing navigation. Provide a return action to the
same event/date and origin (Calendar, Today, or the planning step it came from,
at the same meeting), without losing mail selection,
reader state, Notes search, or unsaved work. Closing preparation returns focus to
its trigger. Forms and source selection open explicit modals; merely opening the
view must not autofocus an input, create a note, or call an AI provider.

Keep **Open Note / Add Note** alongside Prepare. Recheck the occurrence's backlinks
on activation and use the existing primary-note rule. Additional backlinks remain
available; do not add a competing meeting notebook or silently merge notes.

### Review after the meeting

Offer **Review Follow-through** in the same meeting view, including for past events.
Shutdown lists the day's meetings that have ended, excluding declined events, and
offers the same action for each one that has not been reviewed, plus **No
Follow-through** to set it aside for that day on this device.

A meeting counts as reviewed when a note contains a follow-through section linked
to that occurrence. Derive this from saved note content, which syncs, so the state
is the same on every device. Do not derive it from capture receipts, which are
local. A review started but not saved leaves the meeting unreviewed and offers to
resume the local recovery record on that device.
It opens a modal with a chosen source excerpt and editable proposed decisions and
actions. Without AI, the user enters these directly or selects existing note items;
there is no claim that local code has inferred the meeting's decisions.

The user chooses an existing occurrence-linked destination note or runs the existing
Add Note flow first. Display the exact destination and insertion preview. Saving
appends an occurrence-linked dated section to that note. It never rewrites the
original notes or changes an event. When a reusable note holds several meetings,
the new section explicitly identifies this occurrence.

Decisions become ordinary Markdown. Each action can be saved as a round **Task** or
square **Checklist**, or linked to an existing task instead. Default new user-owned
actions to round Tasks. A commitment attributed to someone else remains information
unless the user explicitly chooses to track their own follow-up. Unclear ownership
stays unspecified; do not invent assignees or deadlines.

The first version creates note-derived actions, with their current completion
semantics. It does not add due dates, recurrence, account ownership, or waiting-for
metadata to note tasks. Existing mail/standalone tasks can be linked without
copying them. Extending those task types or promoting note tasks is separate work.

### Prepare a follow-up draft

**Draft Follow-up** opens a review form for sending account, recipients, subject,
body, and optionally an explicit existing conversation to reply to. A local
starting body can be assembled from the reviewed decisions/actions; an AI version
is optional. The source notes remain unchanged by drafting.

- Default recipients to a visible selection of current external attendees,
  excluding the user's known mail identities and calendar addresses. Include no
  Bcc recipients automatically and never reuse a past attendee list silently.
- Use a matching configured mail account where possible; otherwise require an
  explicit choice. Calendar-only setups can review/save notes and copy follow-up
  text but cannot open a send-capable draft without a mail account.
- Default to a new message. Replying requires a user-selected conversation and a
  resolved message/account; do not infer a reply thread from an event title or UID.
  Resolve the thread explicitly before invoking compose and preserve ordinary
  reply headers, aliases, signatures, and recipient behavior.
- Once confirmed, save the current composer first and open or resume a durable
  draft with the reviewed values. If saving fails, leave the old composer and
  review intact. Do not insert into an unrelated open draft.
- Reopening the same handoff resumes its draft. If that draft was deleted or
  sent, show its state and offer an explicit new draft instead of silently
  recreating one. Sending still uses the standard composer and outbox.

## Context selection, identity, and freshness

The native layer owns bounded retrieval. Return source IDs, revisions, provenance,
excerpts, and section-specific errors, not one giant text blob. Preparation is
read-only and local; opening it performs no server mail search, remote attachment
fetch, web request, or provider AI call.

Use these rules in order:

- Fetch current occurrence context from its full account/calendar/event reference.
  Keep the event's start/end instants, all-day semantics, and display timezone.
  A reschedule of the same event keeps identity; a new provider ID does not become
  the old event merely because its UID or title matches.
- Include direct occurrence backlinks separately from exact-series backlinks.
  A reference to another occurrence or the series does not count as this
  occurrence's primary note. Deduplicate sources without losing those categories.
- For a daily note, extract only the linked meeting list item and its nested
  content using a shared Markdown parser with source ranges. Neighboring entries,
  other headings, and unrelated tasks are excluded. A standalone reference outside
  a meeting block supplies a navigable link, not automatic permission to include
  the whole daily note. Ambiguous boundaries require explicit excerpt selection.
- A dedicated/reusable note is visible locally, but sending its whole body to AI
  requires explicit selection and preview. Never assume every line belongs to
  this occurrence. Shared TS/Rust fixtures cover extraction rather than parallel
  ad-hoc regular expressions.
- Resolve `sourceMessageId` when available. Other mail candidates come from exact
  normalized attendee addresses, with direct origin first and recent relevant
  correspondence next. Exclude the user's own addresses. Apply limits before
  loading full bodies; no organization-wide or domain-only inference.
- Default mail scope to the configured mail account matching the calendar account.
  If none matches, require a scope selection before attendee-mail retrieval.
  Expanding to other accounts is explicit, with the account shown on every source.
  Notes remain global user-owned records; linking a note is not an account filter.
- Prefer tasks explicitly linked through selected notes/conversations. Completed
  tasks remain available as history but are not open commitments. Current heuristics
  for promises are evidence candidates, not proof that a promise is unfinished.

The batch readiness summary applies the same identity, scope, and categorization
rules as full context, but returns only flags, counts, and dates. It never loads
message bodies or note text beyond what is needed to find the linked meeting
block, and it bounds the number of events per request. A meeting whose mail scope
needs choosing reports that instead of guessing an account. Readiness must agree
with the full view it opens: a count in the plan is the number of rows in that
category when Prepare opens, given unchanged sources.

Show cache/retrieval limits, source timestamps, and a retry on partial errors. Empty
results differ from unavailable data. Event deletion, cancellation, account removal,
or a missing cache entry disables unsafe actions while keeping readable saved notes
available. Do not fetch an arbitrary remote calendar event just to repair a missing
link in v1.

Use a keyed controller for the selected event, scope, and source revisions. Ignore
late results after a switch, close, or unmount. Refresh affected context after note,
task, mail, or calendar updates; preserve explicit selections and user edits where
still valid, and mark changed/deleted sources for re-review rather than replacing
edited results. Native writes and AI calls must revalidate, not trust a UI snapshot.

## Optional AI and privacy

Add a separate **Meeting Assistance** switch, off by default for new and existing
settings, using the user's existing provider/key configuration. Add a per-note
**Exclude from AI** flag before enabling any note-bearing AI request. It excludes
the note from preparation synthesis and follow-through generation, not from local
reading or ordinary search. Do not reuse Trash or a display-only toggle as privacy.

The flag is persistent note metadata carried through sync and Markdown interchange;
it is not a Markdown task marker or a convention the model interprets. The native
request builder enforces it at request time, including direct note IDs and restored
review sessions. Do not attach the flag to settings transfer, which excludes notes.
Metadata compatibility is a stage-4 deliverable below, not an assumption that current
protocols already support this field.

**Summarize Preparation** and **Suggest Follow-through** open a source-selection
modal. Nothing is sent until the user confirms the exact material. Notes and mail
bodies start unselected. Show included excerpts, dates/accounts, text amounts, and
omissions; enable explicit excerpt selection where a full note is too broad. Include
only reviewed event fields, not the entire calendar, contact profile, or contact's
private notes. Attachments and linked web pages are excluded in v1. Selection is
request-specific; opening the meeting again does not authorize a new request.

On confirmation, resolve the selected references natively, check source revisions,
privacy exclusions and limits, and build a bounded manifest. Reject stale selections
for re-review. Treat all source content as untrusted evidence; instructions inside
mail, notes, and event descriptions cannot authorize tools or writes. The model has
no sending, browsing, database-write, calendar, or attachment tools.

Return structured proposals for summary, decisions, open questions, and actions,
with source references per claim. Validate reference membership and cited excerpts
against the actual manifest. Exact source validation does not establish that an
interpretation is correct; display the evidence and retain user review. Withhold
invalid claims individually with a count/reason; do not present an invalid response
as an empty successful analysis. Preserve the last good review on request failure.

Use existing structured-output/provider support and `ai::with_purpose` usage logging,
with distinct preparation and follow-through purposes. Enforce request/source/output
limits in one native policy module, with a typed UI disclosure of those limits and
below/exact/above-limit tests. Choose concrete budgets using existing provider context
allowances during stage 1; freeze them before AI implementation. Do not silently
truncate selected text or automatically batch/retry costly synthesis. No new AI SDK
or agent runner is needed.

Provider output is transient until explicitly kept. Saving accepted material uses
the same guarded note mutation as manual follow-through. Changes after selection,
including exclusions, invalidate the result for applying until re-reviewed. Requests
already sent cannot be recalled; changing an exclusion prevents subsequent requests.

## Proposed implementation and persistence

Names below are proposed contracts to add, not existing APIs.

| Layer | Work |
| --- | --- |
| UI/controller | `MeetingPreparation` and `useMeetingPreparation` share the event view between Calendar, Today, and the planning steps; `MeetingFollowThroughDialog` owns explicit editing and source selection. `MeetingReadinessList` and `useMeetingReadiness` render a day's meetings for the plan's meeting step and Shutdown, so the routine reuses them instead of querying meeting data itself. App only wires navigation and compose handoff. |
| Typed client/demo | Extend `src/domain.ts`, `src/data/client.ts`, and `src/data/demoClient.ts` with context, source-selection, review, and mutation results. Demo fixtures exercise the same shape without real mail or AI. |
| Native retrieval | `commands/meetings.rs` and a meeting-context service query existing calendar, note-link, contact-history, mail, and task data. Add narrowly scoped DB helpers and indexes only where query evidence requires them. Register commands in `src-tauri/src/lib.rs`. |
| Markdown | Extend the shared note parsing/capture layer with meeting block extraction and guarded follow-through insertion. Reuse note indexing, task markers, sync budget checks, and change notifications. |
| AI | Add a meeting module under `src-tauri/src/ai/` and native command wrappers; extend existing feature settings, activity purposes, structured output, and exclusion checks. |
| Draft handoff | Extend the correspondence controller with an explicit target/account and durable draft seed; return a promise/result so callers can wait for save/open failures. Reuse existing composer/outbox authority. |

Suggested command boundaries:

- `meeting_context(reference, mailScope, cursor)` returns the event snapshot,
  categorized source candidates, revision tokens, and per-section availability.
- `meeting_readiness(references, mailScopes)` returns, per event, note presence,
  commitment and candidate counts, latest correspondence date, follow-through
  state, and per-event availability or errors, without bodies. One failing event
  does not fail the batch.
- `reserve_meeting_prep(reference, dailyNote, expectedRevision, operationId)`
  adds or returns the occurrence's prep task under its meeting line, using the same
  guarded note mutation and receipts as follow-through capture.
- `meeting_source_preview(reference, selections)` resolves bounded local excerpts
  and produces the exact source manifest without a provider request.
- `analyze_meeting(reference, selectionRevision, selections, purpose)` performs
  native validation and returns validated proposals with evidence references.
- `save_meeting_follow_through(reference, destination, expectedRevision,
  operationId, acceptedItems)` validates and atomically appends accepted decisions,
  new task/checklist markers, and links to existing tasks.

Keep context and generated text device-local until the user saves it into a note
or draft. Save in-progress follow-through edits in a bounded local recovery record,
keyed by the full event reference and review ID, with user-controlled discard.
Restore edited text after restart without replaying mutations or AI requests.
Do not persist copies of selected source bodies merely for recovery; resolve them
again and invalidate missing or changed selections.

Add local operation receipts for committed note capture and draft handoff. A capture
receipt and note write share a transaction, so a lost IPC response can be retried
with the same operation ID and return the original result. The native transaction
checks the destination revision, conflict/Trash state, source revisions, item count,
Markdown structure, and note/sync size budgets before any write. All accepted items
save together or none do; unrelated text and authored title remain intact.

For capture, run `prepareNoteMutation` first and reconcile only a version advance
caused by that editor's own flush. Broadcast `notesChanged` after success. Return
created marker IDs and the inserted range for guarded undo. Undo refuses to replace
a newer edited note or remove an action whose state changed afterward; it never
restores an entire old body over new writing.

A durable draft handoff ID maps to one ordinary draft through the draft-creation
transaction or equivalent native idempotency guard. Do not claim exactly-once
behavior from a React ref or from a mapping written after draft creation. When a
handoff fails after creation, query that ID before retrying. A local receipt is not
synchronized sending authority.

Review cards identify existing task markers and linked tasks before offering a new
one. Reopening an applied review shows its receipts and links. A new generated
review can suggest similar text; show existing actions for explicit reuse rather
than claiming semantic deduplication. Across devices, committed note content and
stable markers are authoritative; stale saves conflict instead of silently appending.
Receipts remain local and do not promise global exactly-once handling of separately
created reviews.

## Compatibility and retention

- **No Prep Needed** and **No Follow-through** choices are device-local, kept per
  occurrence and day like dismissed promises, and expire with ordinary retention.
- Local context indexes, review recovery, and operation receipts do not become new
  replicated entities or settings-export fields. Apply normal account/note deletion
  cleanup. Bound recovery retention and offer discard; ordinary saved notes/drafts
  retain their existing lifecycle.
- New AI feature settings are optional with a backward-compatible false default.
  Add import regression fixtures from the preceding encrypted settings schema;
  preserve all existing meanings and export security exclusions.
- For note exclusion metadata, update native models, DB migration, TS/demo models,
  sync projection/protocol, conflict handling, and Markdown import/export together.
  Old records/files default to not excluded, but enabling Meeting Assistance still
  requires fresh explicit opt-in and source selection. Never export an excluded
  note as eligible silently. If an older sync reader cannot preserve the field,
  use the existing protocol/version gate and require device updates rather than
  silently stripping the exclusion. Test the exact preceding snapshot format.
- Saved decisions and round tasks use current Markdown and existing task projection;
  no independent meeting-task entity or synthetic mail account is introduced.
- Opening preparation remains read-only. Nothing migrates historical meeting lines,
  merges same-title notes, or rewrites calendar events as a side effect.

## Delivery stages and completion gates

### 1. Freeze contracts and extraction

Implement typed event/source/revision contracts, bounded retrieval policy, and
shared fixtures for meeting excerpt extraction. Add native context queries, the
batch readiness summary, and matching demo fixtures. Cover identity, scope,
partial cache, and query bounds. Gate: known event context is deterministic,
local, bounded, and contains no unrelated daily-note text; readiness for a day's
meetings agrees with full context and loads no bodies. Record concrete policy
budgets, the per-request event limit, and recovery retention before stages that
depend on them.

### 2. Ship local preparation

Add the shared read-only meeting view, Calendar/Today/palette wiring, source
navigation/return, refresh handling, and existing Open Note/Add Note actions. Add
the readiness list, **Reserve Prep Time**, and **No Prep Needed** for the plan's
meeting step; until the routine ships, the list can appear above Today's agenda.
Gate: a recurring meeting can be prepared from local sources with AI disabled,
without duplicate notes or prep tasks, focus theft, or lost draft/editor state.
Missing mail accounts or one failing section or event do not prevent access to
available notes.

The first Plan Today routine and Today's reshaped layout follow this stage under
their own design, using the readiness list as the meeting step.

### 3. Ship manual follow-through and durable drafts

Add review editing/recovery, guarded note insertion, round-task/checklist capture,
existing-task linking, capture receipts, guarded undo, idempotent composer
handoff, and the follow-through state Shutdown uses. Ship together with the first
Shutdown. Gate: a user can record decisions, track actions, and open/resume a
follow-up draft without AI; retries and restart preserve work without replaying
it; Shutdown lists the same unreviewed meetings on every synced device. This is
the first complete local preparation-to-follow-up workflow.

### 4. Ship privacy and compatibility prerequisites

Implement Meeting Assistance settings and persistent per-note exclusion across
all storage/interchange paths. Add the source-preview manifest and native deny
checks. Gate: prior settings exports/snapshots import safely, excluded notes
cannot enter a request by any supported selection path, and no source-selection
UI can bypass native validation. No note-bearing AI ships before this gate.

### 5. Ship optional synthesis

Add preparation and follow-through structured requests, source-backed review cards,
usage reporting, failure recovery, and apply-time revision checks. Gate: a user can
select evidence, request help, inspect sources, edit/reject proposals, and explicitly
keep them through stage 3. Disabling AI preserves the whole local workflow.

### 6. Validate and document the complete feature

Run required automated checks for the touched areas, update Calendar/Today/Notes,
Tasks and AI help as appropriate, and add user-facing `changes/<branch>.md` entries
for each shipped increment. Keep PLAN.MD and this document's stage statuses aligned.
Do not change the application version. The developer validates the installed app;
agents do not start a dev server or use browser screenshots for appearance review.

## Regression and acceptance coverage

| Area | Required cases |
| --- | --- |
| Identity/history | Same-title unrelated events; exact occurrence versus series-only links; same UID on different calendars/accounts; reschedule retaining ID; changed ID; changed attendees; cancelled/missing events; future series notes not presented as prior meetings. |
| Extraction | Multiple meetings in one daily note; nested lists and tasks; neighboring headings; fenced code and unclosed fences; escaped links; duplicate references; general notes outside meeting blocks; reusable note with several occurrences. Never include neighboring personal text. |
| Scope/retrieval | Own aliases excluded; calendar-only account; explicit cross-account expansion; direct originating message missing; empty versus error versus limit; attendee count extremes; completed tasks; partial failures; late A result after switching to B; offline access. |
| Notes/tasks | Pending body/title edits; local recovery draft; concurrent sync conflict; Trash/delete during review; exact size limits; repeat apply and lost IPC response; transactional rollback; stable round-task IDs and two-way completion; checklist remains note-local; guarded undo after later edits/completion. |
| Drafts | Existing unsaved composer; failed save; explicit reply thread/account; aliases/signatures/reply headers; recipient changes; calendar-only account; duplicate click, restart, lost handoff response; resume existing draft; deleted/sent draft; no send or outbox entry before user sends. |
| AI/privacy | Off/no key; setting migration defaults; excluded note selected directly or restored from recovery; exclusion toggled before request/apply; full daily-note leakage negative; unselected mail/calendar fields absent; oversized request; stale sources; source prompt injection; invalid JSON/citations; partial invalid proposals; failure retains edited review; activity and cost reporting. |
| Readiness/planning | Readiness counts match full context; no bodies loaded; per-request event limit below/exact/above; one failing event in a batch; declined and all-day events; scope needing a choice; Reserve Prep Time adds the meeting line when missing, reuses an existing prep task on repeat and after a lost response; No Prep Needed and No Follow-through undo and stay device-local; follow-through state from a note saved on another device; unsaved review stays unreviewed. |
| Navigation/accessibility | Prepare from the plan's meeting step, Shutdown, Calendar, and Today, returning to the same step and meeting; palette enablement; source/return wiring; note-request acknowledgement; Escape closes topmost surface; read mode keeps shortcuts; explicit form typing scope; restored trigger focus; accessible labels/descriptions; compact shared rows and wrapping under style-token rules. |

Use Rust unit/DB/command tests for retrieval, parsing, transaction, privacy, and
idempotency contracts. Use pure TS tests for ranking/presentation, `renderHook` for
the controller, and direct component tests for views/dialogs. Keep App tests slim:
entry points, return navigation, and compose handoff only. Extend existing suites
for behavior they own and keep every pre-existing assertion unless the intended
contract is explicitly changed.

For implementation changes run `pnpm test`, TypeScript, changed-file ESLint,
style-token tests when styling changes, `node --test scripts/changelog.test.mjs`,
and `git diff --check`. Run `pnpm test:e2e` for compose/triage/email-rendering changes.
For Rust run both required format commands and the Rust suite; in this Tart VM use
`cargo test --manifest-path src-tauri/Cargo.toml` from the root if the test runner
requires unavailable signing. If `crates/` changes, also run `cargo test --workspace`.
No new dependency is expected; any dependency change must follow dependency safety
and vet/deny checks without exemptions.

Developer acceptance: review the day's meetings from the planning step, reserve
prep time for one and book it, prepare a recurring meeting, follow a source and return,
record decisions and actions, complete a note task from Tasks, draft and resume a
follow-up, repeat the actions, and exercise offline/reconnect and a note changed
on another device. With AI configured, verify the reviewed sharing boundary and
an excluded note. Measure preparation time and duplicate/unwanted-action incidents
through explicit evaluation; this plan adds no telemetry or delivery-date promise.
