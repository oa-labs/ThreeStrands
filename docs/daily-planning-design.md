# Daily planning and shutdown

**Status:** Proposed design; no feature code implemented.
**Baseline:** 0.210.0 repository, reviewed 2026-11-16.
**Priority:** Part of the daily loop in the [product plan](../PLAN.MD#connected-workflow-additions),
interleaved with [meeting preparation](meeting-preparation-design.md).

## Outcome and scope

Give the user a short, deliberate start and end to the day in Today. **Plan
Today** settles what is left from the last plan, chooses what today is for, gets
ready for the day's meetings, and checks the plan against free time. **Shut
Down** reviews the day's meetings for follow-through and decides what happens to
unfinished planned work. Today itself is reshaped around the agenda and the plan,
because its unread-mail column has proved low value.

The first version is deliberately rudimentary. It reuses tasks, daily notes, the
day summary, task time blocking, and meeting readiness. It adds no task fields,
no new synchronized entity, no estimates, no workload warnings, no automatic
scheduling, no notifications, and no AI. Those are later work, listed at the end.

Inspiration: Sunsama's guided [daily planning](https://www.sunsama.com/daily-planning)
and [shutdown](https://www.sunsama.com/features/daily-planning-and-shutdown)
routines, its workload warnings, and its explicit choice about which unfinished
tasks carry over. Publishing a plan to a chat service and importing tasks from
other tools are not adopted.

## Principles

- **The plan is the user's choice, not a ranking.** The routine offers candidates
  with reasons; the user picks. Nothing enters the plan on its own.
- **A plan links work; it does not copy it.** A planned task stays one task with
  one completion state. Planning never creates a second record of the same work.
- **Planned is not due.** Planning something for today never changes its due date,
  and a due date never puts something in the plan.
- **Complete before curated.** Today stays complete underneath the plan: overdue
  and due tasks, promises, and check-ins remain visible outside it.
- **Nothing nags.** No plan, a skipped step, or an unfinished shutdown is a quiet
  state, not a warning. There is no badge, notification, or repeated prompt.
- **Unknown is not free.** If the calendar cannot be read, free time is shown as
  unavailable, never as an open day.

## Where the plan lives

The plan for a date is a `## Plan` section in that date's daily note. It syncs,
exports, imports, and conflicts exactly as the daily note does, so it needs no new
synchronized entity, settings-transfer field, or sync-protocol change. A user can
read and edit it in Notes like any other text.

```markdown
## Plan

- [Send the Q3 proposal to Jill](threestrands:task/…)
- [Review the vendor contract](threestrands:task/…) <!-- threestrands-plan:carried 2026-11-15 -->
- ( ) Draft the hiring memo <!-- threestrands-task:… -->
- [Check in with Sam Ortiz](threestrands:contact/…)

### Not today

- [Update the board deck](threestrands:task/…) <!-- threestrands-plan:deferred 2026-11-20 -->
- [Reply to the survey](threestrands:task/…) <!-- threestrands-plan:dropped -->

## Meetings

- [9:30am](threestrands:event/…) met with … for CS Leadership Meeting
  - ( ) Prepare for CS Leadership Meeting <!-- threestrands-task:… -->
```

### Plan items

A plan item is a top-level list item in the `## Plan` section, found with the
shared Markdown parser and source ranges, outside code and escaped text. The
section is the first `## Plan` heading by normalized text, up to the next heading
of the same or a higher level, and excludes its `### Not today` subsection. An
item is one of:

| Item | Markdown | State comes from |
| --- | --- | --- |
| Existing task (mail or standalone) | A link to `threestrands:task/<id>` | The task's status |
| Note task elsewhere | A link to the note task reference (owning note id plus marker id) | The note task's marker |
| New task typed while planning | A round task `( )` with its persisted marker | Its own marker, as an ordinary note task |
| Check-in | A link to `threestrands:contact/…` | Whether the contact was marked contacted on or after the plan date |
| Free text | Any other list item | Not tracked; shown as text |

Linking follows the existing reference rules. A link to a task is a reference, not
a second task row, and checking a list item that contains a link never completes
the linked task. A note task linked from the plan must have a persisted marker;
linking a provisional note task first saves its marker through the guarded
mutation path, as Notes already requires. New items typed during planning are
round tasks, so they appear in Tasks with current note-task semantics: open or
completed, no due date, no account.

Order is priority. The first open item is the day's focus, shown first in Today.
There is no separate priority field.

### Decision markers

Carry, defer, and drop decisions use an inert trailing metadata comment of the
same family as the note task marker. It is metadata, never live HTML, hidden by
the editor in every syntax mode and preserved byte for byte:

- `<!-- threestrands-plan:carried YYYY-MM-DD -->` on a plan item: carried here
  from the plan of that date.
- `<!-- threestrands-plan:deferred YYYY-MM-DD -->` on a `### Not today` item: set
  aside until that date.
- `<!-- threestrands-plan:dropped -->` on a `### Not today` item: removed from
  planning; the task itself is unchanged.

Malformed markers are ignored as metadata. Markers never change what a link
points to or whether a task is complete.

### Write rules

- **The routine writes only the note for the date it is planning or closing.** It
  never rewrites an earlier day's note to record a decision. A decision about
  yesterday's unfinished work is recorded in today's note.
- **Writes are item-level.** Insert item lines, remove an exact item line the user
  chose to remove from this plan, and add `### Not today` lines. Never rewrite,
  reorder, or remove other text in the section. Reordering in the routine moves
  only plan item lines and keeps each item's nested content with it.
- Every write uses the existing guarded note mutation: flush any open or closed
  unsaved draft of that note first, a version-checked native transaction, refusal
  on stale versions, conflicts, Trash, permanent deletion, an unclosed fence at the
  insertion point, or size limits, with **Open Note** to resolve. All of one save
  applies or none does. A deleted note is never recreated silently.
- Without a `## Plan` heading, the section is inserted before `## Meetings` when
  that exists, else appended at the end, separated by a blank line.
- A save carries an operation ID with a local receipt in the same transaction, so a
  lost response can be retried and return the original result.
- Undo after a save removes exactly what that save inserted, under the same version
  check, and offers **Open Note** if the note changed since.
- Two offline devices editing the same day's note still produce the existing
  whole-body conflict. This design adds no text merging.

### What counts as planned and unfinished

An item is **planned for date D** when it is a plan item in D's note. It is
**unfinished** when its source is still open: an open or in-progress task, an
unchecked note task, or a check-in not yet marked contacted. A completed,
cancelled, deleted, or unavailable source is not unfinished; unavailable items
show **Unavailable on this device** and can be dropped but not carried.

An unfinished item is **settled** once a later plan, up to and including today,
mentions the same canonical target as a plan item or a `### Not today` item. A
deferral is **due** on its date. Comparison uses canonical references, never link
labels.

## Plan Today

### Entry points

- **Plan Today** in Today's header while today's note has no plan items; once it
  does, the header shows the plan's progress and **Edit Plan**.
- The palette command **Plan Today**, and **Plan Tomorrow** (the evening variant).
- **Add to Today's Plan** on task rows in Today and Tasks, and on promise and
  Keep in Touch rows in Today: a one-step reversible action that adds a single
  item without opening the routine. A promise first becomes a follow-up task
  through the existing **Make a Task** flow.

No new single-letter shortcut in this version.

### The routine

The routine is a modal form: the `modal` shortcut scope, background inert, focus
placed on the step's first control, Tab trapped, a visible step list, **Back**,
**Next**, and **Cancel**, and `Cmd/Ctrl+Enter` to finish. Escape asks to keep or
discard the routine's unsaved choices and restores focus to the trigger. The plan
date is resolved in local time once when the routine opens; crossing midnight does
not change it.

Steps can be skipped. Nothing is written until **Save Plan**.

1. **Review.** Unfinished, unsettled items from earlier plans in the last
   `PLAN_LOOKBACK_DAYS` (7), newest plan first, plus deferrals due today or
   earlier. Each offers **Carry**, **Defer…** (choose a date after the plan date),
   or **Drop**, with **Carry All**. Items finished since are listed as done, with
   no choice needed. Items left undecided stay unsettled and appear again next
   time. An empty review is skipped automatically.
2. **Choose.** Candidates, each with its reason: overdue and due-today tasks,
   in-progress tasks, waiting-for tasks whose wait date has passed, open promises,
   and due Keep in Touch check-ins. A search adds any open task. **New Task** types
   a plan-only round task. Selected items form an ordered list; move up and down
   with buttons and `Alt+Up/Down`. Show a quiet note above five items ("Most
   people finish three to five planned items"), never a limit.
3. **Meetings.** The day's meetings, from the meeting design's
   [readiness list](meeting-preparation-design.md#prepare-during-daily-planning):
   readiness, **Prepare**, **Open Note / Add Note**, **Reserve Prep Time**, and
   **No Prep Needed**. Opening preparation from here returns to this step with the
   routine's choices intact. Prep tasks are ordinary note tasks under the meeting
   line, not plan items, but they count toward the plan's capacity.
4. **Fit.** The same arrangement as Today's layout, inside the dialog: the chosen
   items as cards on the left and the plan date's [day view](#day-view) on the
   right, with the free time left above it. Drag a card onto the day view to book
   a `TASK_BLOCK_MINUTES` block, or use the card's **Book** action to open the
   event editor at the next free stretch. Booking is optional. A booking made here
   is a calendar write that happens at the drop, with the usual Undo notice; it
   does not wait for **Save Plan**. If the calendar is unavailable, the step says
   so and the plan can still be saved.
5. **Save.** A preview of the lines that will be written to the note. **Save Plan**
   writes them in one guarded transaction; **Copy Plan** copies a plain-text
   version. Calendar bookings made in step 4 are already separate confirmed events
   and are not undone by cancelling the routine. A card booked in step 4 whose item
   is not saved stays booked; the step 5 preview names those bookings.

Reopening the routine with **Edit Plan** starts from the saved plan: current items
selected in order, with the same steps. Removing an item from today's plan removes
its line; it does not mark the item dropped, since dropping is a decision about
unfinished work from an earlier day.

### Capacity in the first version

Capacity is a pure function, `planCapacity(events, bookedBlocks, workingHours,
now)`, beside `dayShape` in `src/todayInsights.ts`:

- Working hours come from the default availability profile, the profile Today
  already uses because it has no conversation or event. The plan date's working
  windows bound the day; outside them nothing counts as free.
- Busy time is attending, timed events on calendars counted as busy, including
  booked task blocks. Declined and all-day events do not count, as in the day
  summary.
- Free time is the remainder from now (or the start of working hours) to the end
  of working hours, in stretches. The stretch threshold reuses
  `FREE_STRETCH_MINUTES` for naming free time and `TASK_BLOCK_MINUTES` for what can
  hold a block.
- The step reports facts only: free time left, items booked, items not booked.
  Without estimates, it does not claim the plan fits or overflows.
- Every booked block is `TASK_BLOCK_MINUTES` (30) long when created. The user can
  change its length in the event editor; capacity then reads the event's actual
  times. Cards carry no duration of their own.

### Booking note tasks

Task time blocking currently offers only tasks of the selected mail accounts, so
note tasks, including plan-only tasks and meeting prep tasks, cannot be booked.
Extend booking to note tasks: the event's source task property holds the note task
reference (owning note id plus marker id) instead of a replicated task id, and
`bookedBlock` and the Calendar task list resolve both forms. The block's calendar
follows the existing default calendar choice. Completing the note task keeps the
event and deleting the event keeps the task, as for other tasks. Old events with a
replicated task id keep their meaning.

## Today's new layout

Today keeps its key (`0`), its role as a read surface, and its live updates. It
has no text inputs; the routine and every form open as modals. Today stays one
day: there is no multi-day board, and tomorrow is planned through **Plan
Tomorrow**.

Two columns, with work on the left and time on the right:

- **Header:** the date, the day summary line, the mail signal, and the plan status:
  **Plan Today**, or "3 of 5 planned done" with **Edit Plan**, and **Shut Down**.
- **Plan column (left):** the plan items as cards, in order, the focus item first.
  A card shows the item's title, its source (account, note, or contact), its
  state, and its booked time when it has one ("10:30 am"). Below the plan,
  **Also due** (overdue and due-today tasks not in the plan), **You said you
  would**, and Keep in Touch use the same cards, each with **Add to Today's Plan**.
  Without a plan, the column shows those sections with a single line offering
  **Plan Today**.
- **Day view (right):** today's schedule as a time grid; see [Day view](#day-view).
  Tomorrow's first events show below it, as the agenda shows them now.
- **Mail signal:** the unread-mail column is removed. A one-line, collapsible
  signal in the header shows **Invitations** (listed, since they are calendar
  decisions) and counts of **Needs a reply** and other unread mail, with
  **Open Inbox**. The phase 1 grouping in `src/todayMail.ts` still computes the
  counts. Collapsed state is a per-device convenience.

Account scope: the plan, day view, and meeting readiness are account-independent;
every task card shows its account. Today's account selector continues to filter
**Also due**, promises, and the mail signal, as it filters them today. The day view
shows the calendars selected in Calendar, as the agenda does now.

Below a width breakpoint the day view is replaced by the current agenda list, with
the same readiness and actions, and dragging is not offered; **Book** still works.
The breakpoint is a layout token, not a number in component code.

Pull-to-refresh, the minute clock, and the existing keyboard navigation of rows
are kept. Cards are `ContextRow`s with card styling, so they keep its keyboard and
accessibility behavior.

### Day view

The day view is the Calendar workspace's time grid for one day, not a second
calendar implementation. Extract from `CalendarWeekView` a shared day-grid
component and a task-block drag hook, used by Calendar's week, workweek, three-day,
and day views and by Today and the Fit step. Calendar's behavior does not change.

- **Contents.** Timed events laid out as in Calendar, all-day events and working
  locations above the grid, and the **Now** line, as in Calendar. Declined and
  finished events are dimmed. Today does not shade working hours, since Calendar
  doesn't; if Calendar adds it, Today inherits it through the shared grid. The grid scrolls to the
  current time on open.
- **Meetings.** Each meeting shows a small readiness indicator from the meeting
  design: a note mark when the occurrence has a note and a count of open
  commitments. A meeting with unread conversations from its attendees shows that
  count; this replaces the "Also in your meeting" note that the mail rows carried.
  Selecting a meeting shows its details in Today, using Calendar's event details
  component, with **Prepare**, **Open Note / Add Note**, **Reserve Prep Time**,
  **Join**, and the existing event actions. Escape closes the details and returns
  focus to the meeting.
- **Booked blocks.** A task block names its task and shows the task's state. A
  card and its block are linked: focusing or hovering one highlights the other,
  and selecting the block offers **Open Task** and **Mark Done**, as Calendar's
  booked-task details do.
- **Dragging.** Pressing and dragging a card past the existing drag threshold
  shows the drop preview, scrolls the grid near its edges, and books a
  `TASK_BLOCK_MINUTES` block at the drop, exactly as Calendar does today: the
  default calendar choice, the selected-calendar fix-up, and a **Booked … for
  "Title"** notice with **Undo**. Escape cancels a drag. A drop outside the grid
  does nothing. A press without a drag activates the card.
- **Dragging an unplanned item.** Dropping a card from **Also due**, **You said
  you would**, or Keep in Touch books the block and adds the item to today's plan
  in one action. A promise becomes a follow-up task first through **Make a Task**;
  a Keep in Touch check-in is booked as an event titled "Check in with <name>"
  with no task link. The plan write uses the guarded note mutation; if it is
  refused, the booking stays and the notice says the item could not be added to
  the plan, with **Open Note**.
- **Keyboard.** Every card has **Book**, which opens the event editor for its task
  at the next free stretch that holds a block, so booking never requires a
  pointer. Dragging is never the only way to do anything.
- **Moving and resizing.** Existing blocks are moved, resized, or deleted through
  Calendar's event editor. Direct dragging of existing events in the day view is
  not part of this design; if Calendar gains it, Today inherits it through the
  shared grid.
- **Unavailable calendar.** Without a calendar connection, or when the schedule
  fails to load, the day view shows that state with **Try Again** and the cards
  stay usable; free time is shown as unavailable, never as an open day.

## Shut Down

### Entry points

**Shut Down** in Today's header, available at any time, and the palette command
**Shut Down**. It is never started automatically, and nothing reminds the user.

### The routine

The same modal form pattern as Plan Today, for the current date:

1. **Meetings.** Today's meetings that have ended, from the meeting design's
   [follow-through state](meeting-preparation-design.md#review-after-the-meeting):
   unreviewed meetings offer **Review Follow-through** and **No Follow-through**.
   Reviewing opens the follow-through dialog over the routine and returns to this
   step.
2. **Plan.** Planned items, done ones first. Each unfinished item offers **Carry to
   Tomorrow**, **Defer…**, or **Drop**, or can be left undecided for the next
   morning's review. Unplanned work completed today is listed as "Also done",
   read-only.
3. **Close.** An optional **Notes on the day** text field, a summary of choices,
   and **Finish**. **Plan Tomorrow** finishes and opens Plan Today for tomorrow's
   date with the carried items already selected.

Shut Down writes to today's note only: decisions in step 2 become `### Not today`
lines with markers (a carry to tomorrow is recorded as a deferral to tomorrow's
date, which tomorrow's review pre-selects as **Carry**), and the optional text
becomes a `### Shutdown` paragraph at the end of the Plan section. It uses the same
guarded write, receipt, and Undo rules. Shutdown is complete for the day when all
ended meetings are reviewed or set aside and no planned item is undecided; the
header then shows "Day closed". That state is derived from note content and
meeting follow-through, so it is the same on every synced device.

## Local state

- An in-progress routine (choices, order, typed items) is a bounded local recovery
  record keyed by routine and date, restored after a restart, with **Discard**.
  It holds no copies of task or note bodies; sources are resolved again and
  missing ones are marked.
- Operation receipts for plan saves are local and follow the meeting design's
  receipt rules.
- **No Prep Needed** and **No Follow-through** are device-local, as the meeting
  design specifies. The mail signal's collapsed state is a per-device preference.
- None of these become replicated entities or settings-export fields.

## Native and client contracts

Names are proposed, not existing APIs.

| Layer | Work |
| --- | --- |
| Markdown | Plan section parsing, plan item kinds, decision markers, and item-level insertion/removal in the shared note parser, with shared TS/Rust fixtures. Extend the editor to keep plan markers hidden and preserved. |
| Native | `daily_plan(date)` returns parsed items with resolved state, settledness, and per-item availability. `plan_review(date)` returns unsettled items from the lookback window and due deferrals. `save_daily_plan(date, expectedRevision, operationId, changes)` applies item-level changes in one guarded transaction. Index plan items in the existing note entity link rows so settledness queries do not reparse every note. |
| Booking | Note task references as event source tasks, in `taskBooking.ts`, the calendar model, and `CalendarTasks`. |
| Shared day grid | Extract the one-day time grid and the pointer task-block drag (threshold, preview, edge scrolling, Escape, booking with Undo) from `CalendarWeekView` into a shared component and hook. Calendar's views use them unchanged; Today and the Fit step render one day. Drop targets accept a plan card source as well as a Calendar task row. |
| UI/controller | `PlanTodayDialog` and `ShutDownDialog` with `useDailyPlan`; `TodayPlanColumn` (cards), `TodayDayView`, and `TodayMailSignal` in a reshaped `TodayWorkspace`; `planCapacity` in `todayInsights.ts`. The meeting step reuses `MeetingReadinessList`; the day view's readiness indicators use the same `meeting_readiness` results. |
| Typed client/demo | Extend `src/domain.ts`, `src/data/client.ts`, and `src/data/demoClient.ts` with plan and review shapes and fixtures. |

## Compatibility

- Plan sections are ordinary Markdown. Older app versions show them as text and
  links; their editors already preserve unknown inert metadata comments, which must
  be confirmed by a regression fixture against the preceding release's editor
  serialization before shipping. If an older editor could strip plan markers, the
  feature needs the existing sync protocol/version gate rather than silent loss.
- Export and import keep the section and markers verbatim. Imported notes with plan
  sections are read like any other.
- Booking events created for note tasks are read by older versions as events with
  an unknown source task, which they already treat as unlinked.
- No settings, settings-transfer, or replicated schema change.

## Delivery stages

1. **Contracts.** Plan section format, parser, markers, shared fixtures, native
   read/review/save, `planCapacity`, and note task booking. Gate: a plan round-trips
   through save, export, import, and an older editor's serialization without losing
   items or markers, and repeated saves create no duplicate lines.
2. **Reshaped Today.** Extract the shared day grid with Calendar's behavior
   unchanged, then the plan cards reading the saved plan, **Add to Today's Plan**,
   the day view with drag-to-book and **Book**, the narrow-width agenda fallback,
   the mail signal replacing the mail column, and meeting readiness on the day view
   (after meeting preparation stage 2). Gate: Today is useful without ever running
   the routine; every Calendar drag and booking test still passes against the
   shared grid; no mail-derived information that the old column provided is lost
   without a path to it.
3. **Plan Today.** The five-step routine, Edit Plan, and Plan Tomorrow. Gate: a user
   can review, choose, prepare, fit, and save a plan with the calendar available or
   not, and restart mid-routine without losing choices.
4. **Shut Down**, shipped with meeting preparation stage 3. Gate: meetings and
   unfinished items can be closed out, decisions appear in the next morning's
   review, and the day's closed state agrees across synced devices.
5. **Document and validate.** Update the Today, Tasks and Goals, Notes, and Calendar
   help pages, add `changes/<branch>.md` entries for each shipped increment, and
   keep PLAN.MD and this document's status aligned.

## Regression and acceptance coverage

| Area | Required cases |
| --- | --- |
| Parsing | Plan section bounds; `### Not today` exclusion; nested content under items; links inside code or escaped; malformed and duplicate markers; free-text items; plan heading inside a fenced block; a note with Meetings but no Plan. |
| Writes | Draft flush; stale version; conflict; Trash; size limit; unclosed fence; lost response with the same operation ID; Undo after later edits; reorder keeps nested content; removal of an exact item only; text written by hand in the section kept. |
| Review | Lookback boundary below/exact/above seven days; items completed since; carried, deferred, dropped settledness; deferral due today, past, and future; unavailable targets; the same task planned on two earlier days; plan made on another device. |
| Choose/fit | Candidate reasons; promise becoming a task first; provisional note task marker saved before linking; working hours split windows; outside working hours; calendar unavailable; declined and all-day events; booking a note task and a mail task; old events with replicated task ids. |
| Today | Layout without calendar, without mail accounts, with no plan, with a finished plan; mail signal counts and Invitations; unread-from-attendees count on day-view meetings; account selector scope; pull-to-refresh; narrow-width agenda fallback with **Book** and no dragging. |
| Day view | Shared grid renders Calendar's existing views unchanged; scroll to now; dimmed declined and finished events; readiness indicators; event details with Prepare and Escape back to the meeting; card and block linked highlight; booked-time label on cards; schedule load failure and Try Again. |
| Drag and book | Drag below the threshold activates the card; drop preview and edge scrolling; Escape cancels; drop outside the grid; drop of a plan card, an Also due task, a note task, a promise (task made first), and a Keep in Touch check-in (unlinked event); refused plan write keeps the booking and says so; Undo removes only the event; no writable calendar; **Book** opens the editor at the next free stretch; two quick drops of the same card. |
| Shut Down | Ended versus upcoming meetings; follow-through state from another device; carry to tomorrow pre-selected next morning; optional notes text; day-closed state derivation. |
| Navigation/accessibility | Modal scope, focus placement, Tab trap, Escape keep/discard, restored focus; step list semantics; reorder by keyboard; Prepare and return to the routine; palette enablement. |

Use Rust tests for parsing, settledness, and guarded writes; pure TS tests for
`planCapacity` and candidate ordering; `renderHook` for `useDailyPlan`; direct
component tests for the dialogs and Today columns; and slim App tests only for
entry points and palette wiring. Extend the existing Today, Notes, and task booking
suites rather than adding parallel ones.

Developer acceptance: plan a day with carried items, a new task, and a block
booked by dragging a card onto the day view and another by **Book**; prepare a meeting from the routine; complete items from Tasks and Notes;
shut down with one deferral and one carry; plan tomorrow; open the same day on a
second device; repeat with the calendar offline.

## Proposed measures

Lightweight, explicit evaluation during development, with no telemetry: how often
a plan is made, how many planned items finish or are deliberately carried, deferred,
or dropped, and whether users still open Inbox from Today for mail the signal did
not show.

## Later, not in this design

- Duration estimates, a workday end the user can change per day, and workload
  warnings when planned time approaches or exceeds free time.
- Reviewed proposals that place several planned items into free time at once.
- An optional AI step in Choose ("Suggest priorities"), taking over the Today
  design's Plan My Day and Top of mind ideas, with cited candidates.
- A weekly plan and review built on the same section format.
- A reminder to plan or shut down, which needs a decision about notifications.
- Plan items with times of day apart from booked blocks.
- A multi-day board of plans. Today stays one day; the next day is planned with
  **Plan Tomorrow**.
- Dragging existing events within Today's day view, unless Calendar gains it first.
- Durations other than the fixed `TASK_BLOCK_MINUTES` block at booking time.
