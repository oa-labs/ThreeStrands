# Today view design

Today (key `0`) is the day at a glance: a Planning Inbox and Done Today record
beside the day's calendar agenda or inline Daily Note. It is a live dashboard,
not a snapshot—it updates as mail syncs, notes change, and the clock moves, and
every row opens the thing it describes.

*(History note: A modal **Plan Today / Plan Tomorrow** routine and a **Shut
Down** evening view were built and removed before release on 2026-11-18, along
with the old unread-mail column and `dayShape` summary. Today now focuses
directly on the Planning Inbox beside the daily schedule and Daily Note.)*

## Principles

- **Complete before curated.** Today never hides mail, meetings, or tasks to
  look tidy. A plan or an optional AI layer can highlight priorities, while the
  full lists stay underneath so nothing is lost.
- **Every claim is traceable.** A row that surfaces an overlap, promise, or
  attendee unread count links to the event, message, or task it came from.
  Guesses are labeled or left out.
- **Heuristics first, AI on request.** Everything that can be derived from
  headers, calendar data, daily notes, or the user's own sent mail runs locally
  and always. Any AI layer follows the AI affordance rules: a sparkles button
  sends data only on explicit action, and turning AI off hides it.
- **A plan links work; it does not copy it.** Planning an item for a day never
  duplicates a task or alters its due date, and checking a link never replaces
  the underlying source state.

## Current layout and behavior (Phase 1)

`TodayWorkspace` (`src/TodayWorkspace.tsx`) renders a two-column workspace:
the **Planning Inbox** and **Done Today** on the left, and the **Agenda** /
**Daily Note** pane on the right (toggled via `Tab`, with `Enter` to edit the
Daily Note). An optional three-phase SVG/CSS header backdrop (`TodayBackdrop`
in `src/TodayBackdrop.tsx`, controlled under **Settings → Appearance → Today
Backdrop**, stored locally only) follows the local clock across `day` (6am–5:59pm),
`dusk` (5am–5:59am and 6pm–8:59pm), and `night` (9pm–4:59am).

### Planning Inbox and Commitments (`src/PlanningInbox.tsx`)

- **Five filters:** `All`, `Tasks`, `Emails`, `Contacts`, and `Commitments`.
  - **Tasks:** Open and in-progress mail, standalone, and note tasks (overdue
    and due first), plus active `## Plan` items leading the list.
  - **Emails:** Starred conversations (opening the prefilled task form on
    creation/booking without unstarring or archiving).
  - **Contacts:** Due **Keep in Touch** check-ins.
  - **Commitments:** Combines due follow-up/waiting-for tasks, tracked mail
    tasks, open **You Said You Would** promises, and possible replies
    (`list_commitment_replies`, open while the newest message is inbound).
- **Open promises (`crates/mail-text/src/promise_text.rs`, `Database::list_open_promises`):**
  Detects first-person promises ("I'll send…", "Let me check…") in the user's
  own sent text over the last `PROMISE_LOOKBACK_DAYS` (14) days where no later
  message went to anyone on the `To` line and no open task already exists on the
  conversation. Each promise can become an action task via **Make a Task** or
  be dismissed locally.
- **Quick add and keyboard triage:** Inline quick-add task input (`onAddTask`)
  in the inbox plus keyboard navigation (`j`/`k`, `[`/`]`, `e`, `s`, `a`, `#`,
  `o`, `b`, `Tab`, `Enter`, `-`/`=`/`t` defined in `src/commands.ts`).

### Done Today (`src/DoneCard.tsx`)

Lists work finished on the selected day:
- Completed tasks (including **Done by email** attribution), completed note
  tasks, and checked plan items;
- Sent emails (`list_sent_between`), created calendar meetings, and created or
  edited notes.

### Agenda, Daily Note, and Meeting Preparation

- **Independent day browsing (`src/todayDayChoice.ts`):** `-` / `=` / `t` (Today)
  browses dates for the right pane, driving both the Agenda and the Daily Note
  for that date.
- **One-day Agenda (`src/TodayDayView.tsx`):** Reuses Calendar's shared day-grid
  and drag hook (`src/useTaskBlockDrag.ts`), showing timed events, all-day
  events, working locations, and the **Now** line (with a narrow-width agenda
  list fallback). Declined and finished events are dimmed.
  - `agendaConflicts` & `mentionsRecording` (`src/todayInsights.ts`) flag
    overlapping events and recorded meetings.
  - Attendee-unread badges count unread conversations in Today's mail list from
    each meeting's attendees.
  - Dragging a Planning Inbox card onto the grid—or pressing **Book** (`b`,
    finding the next `TASK_BLOCK_MINUTES` (30) slot in `planCapacity`)—creates
    a calendar block (`src/todayBooking.ts`). Both mail/standalone tasks and
    note tasks (`note:<noteId>:<markerId>` via `src/taskBooking.ts`) can be
    booked. Check-ins book unlinked events; Starred Mail opens the task form
    before booking.
  - Clicking a meeting title opens Calendar; clicking its note icon or pressing
    `p` opens or creates the occurrence note in Notes, where [meeting preparation](meeting-preparation-design.md)
    lives in the right rail.
- **Inline Daily Note (`src/TodayDailyNote.tsx`):** Pressing `Tab` switches the
  right pane to an embedded `NoteEditor` for that date's daily note (`daily:YYYY-MM-DD`).

### Daily-Note Plan Format

Plans live in a `## Plan` section inside each date's daily note so they sync,
export, and import automatically through Notes without a separate replicated
entity:

- **Plan items (`src-tauri/src/daily_plan.rs`, `src/dailyPlan.ts`):** Top-level
  list items linking `threestrands:task/<id>`, note-task markers, round tasks
  `( ) <!-- threestrands-task:… -->`, `threestrands:contact/…` check-ins, or
  free text. A `### Not today` subsection records deferred
  (`<!-- threestrands-plan:deferred YYYY-MM-DD -->`) or dropped
  (`<!-- threestrands-plan:dropped -->`) items; carried items carry
  `<!-- threestrands-plan:carried YYYY-MM-DD -->`.
- **Editor marker hiding (`src/noteEntities.ts`):** `toEditorMarkdown` /
  `fromEditorMarkdown` hide trailing markers as indented comments while editing
  and preserve untouched markdown byte-for-byte on save.
- **Guarded writes & local receipts:** `src-tauri/src/db/daily_plan.rs`,
  `db/plan_save.rs`, `commands/plans.rs`, and `plan_save_receipts` (schema
  v110, local-only) keep item-level additions/removals transactional and
  idempotent.
- **Account scope:** The plan, Daily Note, Agenda calendars, and Notes are
  account-independent; Today's account selector filters account-scoped Tasks,
  Starred Mail, promises, and Goals.

### Finishing Planning Items by Email

Sending mail connects directly to Today's commitments and tasks:

1. **Check-ins count sent mail:** `daily_plan` passes local midnight (`dayStart`).
   A `threestrands:contact/…` check-in on a day's plan counts as done when mail
   was sent to any of that contact's addresses (or the unlinked address) on or
   after `dayStart`, as well as after **Mark Contacted**.
2. **Write Email & `completesTaskId`:** Planning-inbox tasks offer **Write Email**
   (`TodayWorkspace.tsx`), opening a draft with a `"Completes: <task title>"`
   header line and an unlink action (`Composer.tsx`). When the outbox row moves
   to `sent`, `Database::finish_linked_tasks_on` (`src-tauri/src/db/tasks.rs`
   inside `db/scheduled_send.rs::change_outbox`) advances repeating follow-ups
   or completes one-off tasks (`completion_source = 'user'`) and records
   `completedByEmail` (replicated JSON metadata) so the task displays **Done by
   email** (`DoneByEmail.tsx`, `find_completed_message`). Failed, undone, or
   canceled sends finish nothing.
3. **Post-send task suggestions (`db/task_suggestions.rs`, `src/useSendTaskSuggestion.ts`):**
   After sending an unlinked draft, `suggest_task_for_outbox` checks for a single
   strong match—either an open task linked to the thread or a planned/due task
   whose conversation or title matches a To/Cc recipient (`MIN_NAME_MATCH_CHARS = 3`).
   When unambiguous, a notice offers **Mark Done**, calling `link_task_to_outbox`
   so the task completes immediately (if already sent) or on send.

## Phase 2: deferred AI layer

Planned optional AI enhancements layered strictly over the local Phase 1 lists
(none built yet):

- **Sort mail by meaning:** Group unread inbound conversations into **Needs you**
  (reply/decision/approval/deadline), **Worth knowing**, and **The rest**
  (never hidden), with imperative action headlines, cited one-sentence reasons,
  and display-only project labels. Runs only via an explicit sparkles action or
  an opt-in setting, sending trimmed newest inbound text wrapped in
  `ai::with_purpose` and cached by thread revision.
- **Top of mind strip & Plan My Day:** A compact 3–5 item strip above Today
  triggered by **Plan My Day** (with sparkles), leading with one focus item and
  joining Phase 1 facts (conflicts, recordings, attendee overlap, promises) via
  resolved source IDs (uncited items are dropped). Items offer next steps without
  acting automatically and can be dismissed for the day.

## Not built

- "From mail" event suggestions in Today.
- A "Coming back" section of snoozed mail in Today.
- Promise detection in languages other than English, or promises made to Cc-only
  recipients.
- AI matching for post-send task completion if deterministic matching misses.
- Duration estimates, workload warnings, or multi-day planning boards.
