# Daily planning and shutdown

**Status:** Superseded in part (2026-11-18). The **Plan Today** routine (and
its **Plan Tomorrow** variant), the **Shut Down** evening view with **Review
Today**, **Day closed**, and **Notes on the Day**, and the Day Details / Edit
Plan header controls were built and then removed before release, when Today was
simplified to the Planning Inbox beside the schedule (see
[Today view design](today-design.md)). Their sections below are kept as design
history and are marked as removed; they do not describe the app. What remains
in use: the plan section in the daily note and its parser and writes (stage 1),
the Planning Inbox and day view (stage 2, as since simplified), and the optional
header backdrop, which now follows the clock rather than the view.

**Original status:** Stages 1 to 4 are built. Stage 1 (contracts): the plan section format,
parser, item-level writes, and plan reading (`src-tauri/src/daily_plan.rs`,
`src-tauri/src/db/daily_plan.rs`, `src/dailyPlan.ts`, `fixtures/daily-plan.json`);
plan history and review (`src-tauri/src/db/plan_review.rs`); candidate sources
(`src-tauri/src/db/plan_sources.rs`); `planCapacity` in `src/todayInsights.ts`;
and note-task booking in `src/taskBooking.ts`. The demo client follows the same
rules (`src/data/demoPlans.ts`). Stage 2's reshaped Today is listed under
[As built (stage 2)](#as-built-stage-2), stage 3's Plan Today routine, with
`save_daily_plan` and its receipts and Undo, under
[As built (stage 3)](#as-built-stage-3), and stage 4's evening view and optional
backdrop under [As built (stage 4)](#as-built-stage-4). The editor's marker
handling is built too, as noted under stage 4. See also [As built (stage 1)](#as-built-stage-1).
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

### Plan history

Plan sections are also the record of what was planned when. Two facts are derived
from them, through the indexed plan item rows, never by reparsing every note:

- **Carry count.** The number of plan days, within the last `PLAN_HISTORY_DAYS`
  (90), on which the same canonical target appears as a plan item with a
  `carried` marker. A card shows it as "Carried 3 times" when it is at least one.
  A count is a fact about the notes, not a judgment; it never changes an item's
  order or hides it.
- **Planned days.** The dates on which a target appeared as a plan item, used to
  show "Planned Mon, Tue" for a goal's tasks (see [Goals](#goals-on-cards)).

Both are computed for whatever notes are on this device. A plan section written on
another device counts once its note has synced. Edits to old notes change history
accordingly; nothing is stored separately.

## Plan Today

> **Removed (2026-11-18).** The routine below was built (stage 3) and removed
> before release; nothing in Today or the command palette opens it. Kept as
> design history.

### Entry points

- **Plan Today** in Today's header while today's note has no plan items; once it
  does, the header shows the plan's progress and **Edit Plan**.
- The palette command **Plan Today**, and **Plan Tomorrow** (the evening variant).
- **Add to Today's Plan** on task rows in Tasks and on every source card in Today:
  a one-step reversible action that adds a single item without opening the
  routine. A promise first becomes a follow-up task through the existing **Make a
  Task** flow; a starred conversation without an open task opens the prefilled
  task form first, as in [Candidate sources](#candidate-sources).

No new single-letter shortcut in this version.

### The routine

The routine is a modal form: the `modal` shortcut scope, background inert, focus
placed on the step's first control, Tab trapped, a visible step list, **Back**,
**Next**, and **Cancel**, and `Cmd/Ctrl+Enter` to finish. Escape asks to keep or
discard the routine's unsaved choices and restores focus to the trigger. The plan
date is resolved in local time once when the routine opens; crossing midnight does
not change it.

Steps can be skipped. Nothing is written until **Save Plan**.

1. **Review.** Two columns about the most recent earlier plan in the last
   `PLAN_LOOKBACK_DAYS` (7), titled with its day ("Yesterday", or "Friday"):
   - **Done:** that plan's items finished since, the meetings attended that day
     (ended, not declined), and **Also done**: tasks completed that day that were
     not in the plan. Read-only; each opens its source.
   - **Didn't get to:** that plan's unfinished, unsettled items, then unsettled
     items from older plans in the window grouped by date, then deferrals due on
     or before the plan date. Each offers **Carry**, **Defer…** (choose a date after
     the plan date), or **Drop**, with **Carry All**. Each shows its carry count.
     From `PLAN_CARRY_HINT` (3) carries, the row adds a quiet hint, "Carried 3
     times. Defer or drop it?", without changing the default choice.

   Items left undecided stay unsettled and appear again next time. With nothing in
   either column, the step is skipped automatically. There are no time totals or
   charts: without time tracking or estimates, they would be invented.
2. **Choose.** A [source switcher](#candidate-sources) beside the selected list:
   **Suggested** (the default: overdue and due-today tasks, in-progress tasks,
   waiting-for tasks whose wait date has passed, open promises, and due Keep in
   Touch check-ins, each with its reason), **Tasks**, **Starred Mail**, **You Said
   You Would**, **Keep in Touch**, **Goals**, and **Notes**. A search adds any open
   task. **New Task** types a plan-only round task. Selected items form an ordered
   list; move up and down with buttons and `Alt+Up/Down`. Show a quiet note above
   five items ("Most people finish three to five planned items"), never a limit.
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

### Candidate sources

The Choose step offers individual sources through a tab list with arrow-key
navigation. Every candidate is a card, and chosen items are marked. Today's
Planning Inbox combines these sources into All, Tasks, Emails, and Contacts;
planned items lead the list, and scheduled or finished items leave the active
list. Finished tasks appear in a separate Completed card, with checked boxes
to reopen them; its history comes from saved task and note-task completions.

| Source | Candidates | Adding to the plan |
| --- | --- | --- |
| Suggested (Choose only) | As in step 2, each with its reason. | As for its own source. |
| Tasks | Open and in-progress mail and standalone tasks, overdue and due first, then the rest in task display order. | Links the task. |
| Starred Mail | Starred conversations, newest first, with account and sender. A conversation that already has an open task shows that task instead. | Opens the existing task form prefilled from the conversation (title from the subject, linked to the conversation and its account); saving creates the task and adds it. Cancel writes nothing. A conversation with an open task adds that task. Adding never unstars or archives the conversation. |
| You Said You Would | Open promises, as in Today. | Makes an action task through **Make a Task**, then links it. |
| Keep in Touch | Due check-ins. | Links the contact. |
| Goals | Active goals of the current periods, each with its open tasks and its planned days. | Links a goal's task. **New Task for Goal** opens the existing task form with the goal set. |
| Notes | Open round tasks in every note, newest note first, with the note's title. | Links the note task, saving its marker first if it is provisional. |

Starred mail is the user's own choice of what matters, so it is the mail source
for planning; the old addressing-based groups are not offered as candidates. The
source lists are bounded and paged (`PLAN_SOURCE_PAGE`, 50), use existing local
queries, and never search the server. Sources with mail-account scope (Tasks,
Starred Mail, You Said You Would, Goals) follow Today's account selector and label
each card's account; Keep in Touch and Notes are account-independent.

### Goals on cards

A card for a task with a goal shows the goal's title, which opens the goal. In the
Goals source, each goal lists its tasks with the days they were planned in the
goal's current period ("Planned Mon, Tue"), from [plan history](#plan-history),
and its open task count. This connects daily plans to the existing goals and
period reviews without adding weekly objectives: goals keep their horizons,
periods, and reviews. Goals belong to a mail account and only mail and standalone
tasks carry a goal; note tasks and check-ins show none.

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

> **Partly superseded (2026-11-18).** The two-column layout below stands, but
> Today no longer offers Plan Today, Plan Tomorrow, Review Today, Day Details, or
> Edit Plan. The schedule's arrows browse other days, including tomorrow, and
> tasks and events are added inline. See [Today view design](today-design.md).

Today keeps its key (`0`), its role as a read surface, and its live updates. It
has no text inputs; the routine and every form open as modals. Today stays one
day: there is no multi-day board, and tomorrow is planned through **Plan
Tomorrow**.

Two columns: a planning inbox on the left and today’s schedule on the right.
The inbox takes about one-third of the width and the schedule about two-thirds,
with minimum column widths and the responsive agenda fallback described below.
The inbox uses All, Tasks, Emails, and Contacts filters over one list. Active,
unscheduled plan items lead, followed by an interleaved set of tasks, emails, and
contacts. Plan and candidate identities merge, including tasks linked from starred
conversations. Completed tasks have a separate, reopenable Completed card and
remain available in Review Today; scheduled items live on the calendar.
Note and goal tasks belong under Tasks, including tasks in today’s note;
square checklists remain note-local. Secondary actions are in each card’s menu.
Contact bookings carry a private `sourcePlanKey` alongside task bookings’ existing
source identity, so calendar refresh, Undo, and restart preserve inbox membership.
The new event field is optional when older cached events are read.

Meeting titles open Calendar. A separate note icon opens the occurrence’s existing
note or adds the event to its daily note and opens it at the event reference.
<kbd>p</kbd> does the same for the focused meeting or the next one. Preparation is
available from the event’s link in the Notes context rail while the editor stays
visible. Closing it returns to note details, and Back to Meeting restores the same
note and preparation after opening a source. Today’s inbox is never replaced.

- **Header:** the date, the planning title, plan progress, Plan Today / Edit Plan,
  Day Details, and Review Today. Day Details reveals the meeting summary and mail.
- **Planning inbox (left):** four filters and a matching count, with one card per
  active unscheduled item. Schedule is the primary action; plan edits, promise
  conversion, and dismissal remain in the card’s secondary menu. Pagination and
  refresh retain the current filter and expanded list. Task and email suggestions
  have Done checkboxes. Starred mail first uses the task form; promises create an
  action task linked to the exact sent message. Completion preserves email flags
  and settles the current message, while a later message can offer new work.
- **Schedule (right):** today’s time grid. Tomorrow appears in Review Today.
- **Mail signal:** under Day Details, invitations by name and counts of reply-needed
  and other unread mail, with Open Inbox. Its collapse state remains device-local.

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
  Selecting a meeting shows its preparation in the left column, as above; Escape
  returns to the plan and focus to the meeting. Event details, such as answering
  an invitation, stay in Calendar, reached through **Open in Calendar**. Working
  locations are shown above the grid as in Calendar and are never prepared.
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
- **Dragging an unplanned item.** Dropping a card from any source books the block
  and adds the item to today's plan in one action. A promise becomes a follow-up
  task first through **Make a Task**. A starred conversation without an open task
  opens the prefilled task form with the dropped time shown; saving creates the
  task, books the block, and adds it, and Cancel writes nothing. A Keep in Touch
  check-in is booked as an event titled "Check in with <name>" with no task link. The plan write uses the guarded note mutation; if it is
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

### As built (stage 2)

- `TodayWorkspace` lays out the plan column (`PlanCards`, `PlanSourceSwitcher` in
  `src/TodayPlan.tsx`) beside `TodayDayView`, with `TodayMailSignal` in the header.
  The ratio is `minmax(0, 2fr) minmax(var(--today-day-view-min), 1fr)`. The
  narrow fallback is a CSS container query on `.today-page` (container `today`):
  both the day view and the agenda list are rendered, and the query shows one.
  Container conditions can't read custom properties, so the breakpoint number
  lives in that one CSS rule rather than a token.
- The plan is read with `useDailyPlan` (`mailClient.dailyPlan`), again on any note
  change and on task changes, and written with `addToDailyPlan` /
  `removeFromDailyPlan` from `src/noteCapture.ts`, with Undo and **Open Note** on
  refusal. Note tasks are checked off with `changeNoteItem`.
- The guided planner registers sources as `PlanSource` entries (`id`, `label`,
  groups of `PlanCandidate` cards). A candidate names its plan key and a
  `resolve()` that returns what to add and what to book, creating a promise's
  task first through **Make a Task**. Suggested belongs to the routine's Choose
  step. The inbox converts candidates into one deduplicated list, with planned
  entries first and secondary actions in each card's menu.
- **Starred Mail**, **Goals**, and **Notes** in the guided planner are fed by
  `planSources(date, source, accountId, cursor)` through `usePlanSourcePage`, read
  only while the tab shows, a page at a time with **Show More** on `next`, and again
  when the plan, tasks, or mail change. Starred Mail and Goals pass Today's account;
  Notes passes none. Each candidate's `action` decides adding: `link` saves its
  addition; `makeTask` uses **Make a Task**; `taskForm` and `newTaskForGoal` call
  Today's `onCreateTask`, which App answers with the existing task editor
  (`createTaskInEditor` in `useTaskActions`: prefilled from the conversation and
  linked to it, or with the goal chosen) and which resolves with the task once it
  saves, or null on Cancel, so Cancel writes nothing. Dropping such a card waits
  for the save before booking. The inbox reads Starred Mail and Notes together,
  retaining expanded pages across refresh. Tasks, promises, and due contacts
  continue using their existing local reads; all contribute to the matching
  filter count. Goal tasks appear under Tasks, while creating tasks from a goal
  stays in the guided planner.
- Booking lives in `src/todayBooking.ts`: `bookBlock` uses Calendar's default
  calendar choice and selected-calendar fix-up; `blockRequest` tags the block with
  the item's `taskId` (a note task's `note:<noteId>:<markerId>`) or none for a
  check-in. Drops use `useTaskBlockDrag`; a source drop books first and then adds,
  and a refused plan write keeps the booking and says so with **Open Note**.
- **Book** books directly, without the event editor: the first quarter-hour start
  inside a free stretch of `planCapacity` (today's working windows of the default
  availability profile, less attending timed events) that still holds
  `TASK_BLOCK_MINUTES` (`nextFreeSlot`), with the same notice and Undo, or "No free
  half hour is left in today's working hours". It is simpler than opening the
  editor at the next free stretch: one keystroke has the same effect as a drop, the
  result is visible in the day view and reversible with Undo, and the editor is one
  click away on the event. When the schedule is unread or failed, or Today has no
  working hours, Book says free time is unknown. App passes its
  `availabilityPreferences` (the default profile) as `workingHours`.
- A drop is an explicit time, so it books where it lands even outside working
  hours, as Calendar does; the notice adds "(outside working hours)"
  (`withinWorkingHours`).
- Booked note tasks use `taskBooking.ts`'s `bookedSource` / `sourceTaskId`, so a
  block's source task is written and matched as Calendar writes and matches it.
- Plan cards show "Carried N times" from `carryCount` when it is at least one,
  without reordering; the hint from `PLAN_CARRY_HINT` belongs to Review.
- A plan card and the block booked for its task highlight each other on hover or
  focus: the card wrapper sets the linked task, and `DayTimeGrid`'s decoration
  gained `linked` and `onLinkHover` (still unused by Calendar).
- Day-view meetings carry their readiness summary and a count of unread mail from
  attendees as the event's accessible description, with a small mark for a note
  or unread mail, through an optional `eventDecoration` on `DayTimeGrid` that
  Calendar doesn't pass.
- After the developer's review of the installed app:
  - **Suggested** (`planSources(date, "suggested", account)`) is the first tab and
    the default, each card naming its reason. An empty source still shows, in one
    line, with the first `SOURCE_FALLBACK_CARDS` (3) cards of the next source in
    tab order that has some beneath it and a link to switch, so the first view is
    never two empty states. The tabs use Mail's light split-tab style; Starred
    Mail, Goals, and Notes sit behind **More Sources** (`overflow`) and join the
    row while chosen; arrow keys still move through every source.
  - The day view opens on the default profile's working hours (their start, or
    half an hour before now inside them), shades time outside them
    (`DayTimeGrid`'s `shade`), and is as tall as the working day so Tomorrow stays
    in view (`dayViewLayout`). Unknown hours keep the old opening and height.
  - Finished meetings carry `data-past` and dim; only the meeting being prepared
    carries `data-selected`. The "selected" look the developer saw was Calendar's
    unanswered-invitation style (white fill, 2px outline), which Today now draws
    filled with a dashed edge. Short blocks show their start time (`shortTimes`),
    and readiness marks (notes, open commitments, unread from attendees) are
    visible in the block. Calendar passes none of these props.
  - The summary line is `daySummary` ("2 meetings · next at 2:00 pm", "2 meetings,
    both finished · last ended 3:30 pm", "No meetings today"). The account in view
    is named beside the mail line and the mail-scoped sources, not in the page
    eyebrow; the mail line is text with a chevron to fold it; **Open Tasks** is at
    the foot of the Tasks tab.
- Not yet: a free-time line from `planCapacity` on
  Today itself (the Fit step has one; Shut Down's header entry is stage 4's
  **Review Today**), and Open Task / Mark Done on a selected
  block (choosing a block still prepares or opens it like any event). Plan
  Today and Edit Plan are in the header since stage 3.

### As built (stage 3)

- **Entry points.** Today's header shows **Plan Today** while the plan has no
  items, else the progress line and **Edit Plan**. **Plan Tomorrow** joins them
  once today's last attended, timed meeting has ended or from
  `EVENING_HINT_MINUTES` (60) before the end of today's working hours
  (`offersPlanTomorrow` in `todayInsights.ts`). The palette commands
  `today.plan` (**Plan Today**) and `today.planTomorrow` (**Plan Tomorrow**)
  have no keys; they open Today through `useWorkspaces`' `planDay`, and Today
  consumes the `PlanRequest` and opens the routine. The plan date is fixed when
  the routine opens.
- **Dialog.** `PlanTodayDialog` is one `Modal` (modal scope, inert background,
  Tab trap, focus restored to the trigger) with an ordered step list whose
  buttons jump to any step, so every step can be skipped; **Back**, **Next**,
  **Cancel**, and **Save Plan** on the last step, `Cmd/Ctrl+Enter` saving from
  any step. Each step focuses its first control. Review is left out of the step
  list when the review has nothing in either column. Escape (or Cancel) closes
  at once without choices; with choices it shows **Keep for Later**, **Discard
  Choices**, and **Go Back** inside the dialog, and a second Escape goes back.
- **Model and controller.** `src/planRoutine.ts` holds the routine as data
  (items in order, plan items taken off, Review decisions, bookings made) and
  derives the `PlanChanges` a save sends, the exact Markdown lines it adds, and
  the plain text for **Copy Plan**. `usePlanRoutine` reads the plan, the review,
  and the open tasks for search once, after `prepareNoteMutation`, and starts at
  Choose when there is nothing to review.
- **Review.** Done lists the reviewed plan's finished items, attended meetings,
  and Also done; rows don't open their sources, since that would leave the
  dialog. Didn't get to rows are grouped by the plan they came from, with
  **Carry**, **Defer…** (a date input whose minimum is the day after the plan
  date), **Drop**, **Carry All** for the undecided, carry counts, and the hint
  from `PLAN_CARRY_HINT`. Pressing a chosen decision again clears it.
  Unavailable rows can be dropped but not carried. Carrying adds the item to
  the chosen list with `carried` set to the date it was planned.
- **Choose.** Today builds the switcher's tabs with `sourcesFor(selected, page,
  choose)` for both Today and the routine; the routine's set leaves out Also
  Due (Suggested covers it), reads its own `usePlanSourcePage` for the plan date
  (tomorrow's ranks by tomorrow's dates), and its cards add to the chosen list
  through the same `resolve()` (so a promise or a starred conversation still
  makes its task first) instead of writing. Opening a card's source is turned
  off inside the dialog. A search over open mail, standalone, and note tasks
  lists `PLAN_SEARCH_RESULTS` (8) matches; **New Task** adds a plan-only round
  task. Items move with buttons or `Alt+Up/Down`; above `PLAN_ITEMS_NOTE` (5)
  the quiet note shows.
- **Meetings.** The plan date's timed, not-declined meetings in
  `MeetingReadinessList`. **Prepare** shows the same preparation view App
  renders for Today *inside the dialog*, with **Back to Meetings**: the routine
  never unmounts, so its choices stay as they were, and nothing is handed back
  and forth. If preparation leads elsewhere (opening a note or a conversation
  switches workspace and closes Today), the choices are already kept for
  recovery and return when the routine opens again.
- **Fit.** The chosen items as cards beside `TodayDayView` for the plan date
  (it gained `day` and `label`), with the free time, booked, and not booked
  counts from `planCapacity`. **Book** uses `nextFreeSlot` for the plan date
  (it gained a `day` argument) and dragging uses `useTaskBlockDrag`; both book
  through `bookBlock` at once with Undo, as on Today, and are listed at Save. A
  typed new task has no task id until it is saved, so its block is booked
  unlinked, titled with the task.
- **Save.** `save_daily_plan(date, expected, operationId, changes)` (`src-tauri/src/db/plan_save.rs`)
  takes `items` (the new order: `existing` keys and `add` entries with an
  optional `carried` date), `remove` (plan item keys whose lines go), and
  `notToday` (`deferred` after the plan date, or `dropped`). It refuses stale
  versions, conflicts, Trash, an unclosed fence, unknown items, carried dates on
  or after the plan date, more than `MAX_PLAN_ADDITIONS` new lines, and note size
  limits, all before writing. Rearranging is item level
  (`arrange_plan_items` in `daily_plan.rs`, mirrored by `arrangePlanItems` and
  held to the `arrange` fixtures): named items fill the places those items held,
  so text between items and unnamed items stay put; links already in the plan
  are reported in `already`; a Not today decision already recorded isn't
  repeated. The result (`added`, `removed`, `notToday`, `already`, `changed`) and
  the body it replaced are kept in a local `plan_save_receipts` row (schema
  v110; unsynced, not exported, pruned after `PLAN_RECEIPT_DAYS` (7) and beyond
  `MAX_PLAN_RECEIPTS` (200)) in the same transaction. Repeating an
  `operationId` returns the stored result; a refused save leaves no receipt.
  `undo_daily_plan_save(date, operationId)` restores the replaced body only while
  the note is still at the version the save left, else refuses with Open Note.
  The routine keeps one operation id, so retrying after a lost response doesn't
  repeat the save.
- **Re-basing.** Reserve Prep Time in the Meetings step writes the same note,
  so `saveDailyPlan` in `noteCapture.ts` saves on the note's current version
  when every plan item the changes name is still there and none is free text;
  otherwise the save refuses as stale and the dialog keeps the choices with
  **Open Note**.
- **Keys.** Plan item keys are the canonical target as natively
  (`planItemKey` in `dailyPlan.ts`: the link's href without contact or series).
  Today's local cards had used `noteReferenceKey`, which never matched native
  keys, so "In plan" and Also Due's hiding only worked in the demo; they and the
  demo now use `planItemKey`.
- **Recovery.** `writeRecovery` keeps the routine in `localStorage`
  (`threestrands.planRoutine`) as choices change: one per date, at most
  `MAX_RECOVERED_ROUTINES` (2), none older than `ROUTINE_RECOVERY_DAYS` (7). It
  holds keys, link labels, titles, and dates, never note or task bodies.
  Reopening the routine for that date restores it, re-based on the plan as it
  is now (plan items gone since, and added tasks no longer open, are marked and
  not saved), with **Discard**. Saving or discarding forgets it.

### Day and evening backdrop (optional)

An optional, purely decorative backdrop behind Today's header: a daytime scene
from 6 am and an evening scene from 6 pm, following the clock. (It first followed
the evening view; since that view was removed it follows the time of day.) It is off by default and turned on under
**Settings → Appearance → Today Backdrop**, stored on this device only (not synced
or exported).

- The art is original and drawn in code (SVG and CSS gradients), not photographs
  or stock images: small to ship, crisp at any size, and free of licensing
  questions. Day: a soft sky with the sun's height following the time of day.
  Evening: a dusk-to-night sky with a moon and a few stars.
- It sits only behind the header band, never behind lists, cards, or the day
  view, and keeps text contrast within the style-token rules in both light and
  dark themes. It is hidden from assistive technology and has no motion beyond an
  optional slow change when the view switches; reduced-motion turns that off.

## Shut Down: Today's evening view

> **Removed (2026-11-18).** The evening view, **Review Today**, **Plan
> Tomorrow**, **Day closed**, and **Notes on the Day** were built (stage 4) and
> removed before release. Kept as design history.

**Decision (2026-11-17, after review of the installed app):** Shut Down is not a
modal routine. In the evening the user's questions are what they accomplished,
what loose ends remain, and how to get ready for tomorrow, and they expect Today
to answer them when it opens. So Shut Down is the **evening view** of Today: the
same two columns, showing the day in review on the left and tomorrow on the right.
Its decisions, note format, and derived "Day closed" state are unchanged from the
earlier design; only the presentation moved from a dialog into Today. Plan Today
stays a routine; **Plan Tomorrow** is how the evening view hands off to it.

### When Today shows it

- Automatically once the day's last attending timed meeting has ended **and** now
  is past the end of the default profile's working hours (or, without working
  hours, after 6 pm local time). Before that, **Review Today** in the header shows
  it on demand; **Back to Today's Plan** returns to the daytime view. The choice
  lasts for the rest of the day on this device.
- The palette command **Review Today** toggles it. Nothing reminds the user, and
  skipping it is a quiet state.
- Meeting preparation (`p`) still works: preparing a meeting shows it in the left
  column in place of the review, as in the daytime view.

### Left column: Today in review

From `plan_review(today)` and today's plan, read only from local data:

- **Done today:** finished planned items, tasks completed today outside the plan
  ("Also done"), meetings attended (ended, not declined), and check-ins marked
  today. Read-only rows that open their source. No time totals or charts.
- **Loose ends**, each row with its own decision:
  - Unfinished planned items, with carry counts and the carry hint:
    **Carry to Tomorrow**, **Defer…** (date in a small modal), or **Drop**.
    Leaving one undecided keeps it for tomorrow morning's review.
  - Today's ended meetings without follow-through:
    **Review Follow-through** (the meeting design's dialog) or **No
    Follow-through** (this device). Until follow-through ships, these rows offer
    **Prepare** and **Open Note / Add Note** instead.
  - Promises made in today's sent mail (**You said you would**, today only):
    **Make a Task** or **Dismiss**.
  - Mail that needs a reply: one summary row with the count, opening Inbox.
- **Notes on the day:** **Add Notes on the Day…** opens a small modal form (Today
  has no inline inputs); saving appends a `### Shutdown` paragraph to the Plan
  section of today's note through the guarded write.

Decisions write to today's note only, as before: a carry to tomorrow is a
`### Not today` line deferred to tomorrow's date, which tomorrow's review and
Plan Tomorrow pre-select as **Carry**; defer and drop use their markers. Each
decision is one guarded item-level write with Undo.

### Right column: tomorrow

Tomorrow's day view (the same shared grid, working hours shaded, readiness marks
on meetings, **Prepare** on choosing a meeting), with the items carried to
tomorrow listed above it as the start of tomorrow's plan, and **Plan Tomorrow**,
which opens the Plan Today routine for tomorrow with those items selected. The
"Tomorrow" agenda list below the daytime view is not repeated.

### Day closed

The header shows **Day closed** when every ended meeting is reviewed or set aside
and no planned item is undecided. It is derived from note content and meeting
follow-through, so it agrees on every synced device; No Follow-through is the one
device-local input.

### As built (stage 4)

- **Switching.** `TodayWorkspace` shows one of two views, `day` or `evening`
  (`src/eveningView.ts`). `eveningOpens` (`todayInsights.ts`) opens the evening
  view by itself once every attending, timed, non-working-location meeting has
  ended and now is at or past `workingDayEnd` (the last working window's end
  today, in the default profile's zone), else, when the profile has no window
  today or there is no profile, from `EVENING_FALLBACK_HOUR` (18) local. A
  calendar that is connected but still loading, or failed, never opens it, since
  a meeting may still be ahead; without a connected calendar there are no
  meetings to wait for. **Review Today** and **Back to Today's Plan** in the
  header write the choice to `threestrands.today.view` as `{ day, view }` and
  win over the clock for that local date only (a new date reads as no choice).
  The palette's **Review Today** (`today.review`) toggles through the same
  `planDay` request that Plan Today uses (`PlanRequest.day` gained `review`).
  The header's **Plan Tomorrow** moves to the tomorrow column in the evening
  view.
- **Left column (`EveningReview`, `src/TodayEvening.tsx`).** It reads
  `planReview(tomorrow)`, which reviews the most recent plan before tomorrow:
  today's, when today has one. Its Done and Also done are used only when
  `reviewed` is today; otherwise Also done comes from the tasks completed today
  that Today already lists, and meetings come from the schedule (declined,
  all-day, and working-location events left out), falling back to the review's
  cached meetings. The review is read again on any note change. Done rows open
  their source. The loose ends are the review's Didn't get to rows less
  the keys under today's own `### Not today`: the review reads a carry to
  tomorrow back as a deferral that is due tomorrow, and today's own line says it
  is decided. Rows use `ReviewDecisionRow`, and the Done list `ReviewDone`
  (`src/PlanReviewColumns.tsx`), both extracted from Plan Today's Review step,
  which now uses them too. **Carry to Tomorrow**, **Defer…**, and **Drop** each
  save one `saveDailyPlan` change (`decisionChanges`: no items, no removals, one
  `notToday` entry, a fresh operation id), a carry being a deferral to tomorrow's
  date and Defer… asking for a date from tomorrow on in `DeferDialog`. The
  notice carries Undo (`undoDailyPlanSave`) and, on a refusal, **Open Note**.
  Meetings use `MeetingFollowThroughList` (it gained `onOutstanding`), promises
  are today's `OpenPromise` rows with **Make a Task** (Today's existing flow) and
  **Dismiss** (the existing per-device dismissal), and mail that needs a reply is
  one row from `groupTodayMail` that opens Inbox.
- **Notes on the day.** `PlanChanges` gained `shutdown`, and `PlanSaveResult`
  `shutdown`: `append_shutdown_note` (`src-tauri/src/daily_plan.rs`, mirrored by
  `appendShutdownNote` and held to the `shutdown` cases of
  `fixtures/daily-plan.json`) appends one paragraph under `### Shutdown` at the
  end of the Plan section, or under an existing `### Shutdown` subsection, and
  creates the section when the note has none. The text has its whitespace
  collapsed, its inline syntax escaped as other inserted text is, and a leading
  list, quote, or ordered-list marker escaped so it can't become a plan item; it
  is limited to `MAX_SHUTDOWN_NOTE_CHARS` (2000, in both languages) and refused,
  like any plan write, on a stale version, conflict, Trash, or unclosed fence.
  It is written in the same transaction as the other changes and covered by the
  same receipt and Undo. A request without the field behaves as before, and a
  receipt saved before it existed reads as `shutdown: false`.
  `ShutdownNotesDialog` is the small modal (**Add Notes on the Day…**); a refused
  save keeps the text and offers **Open Note**.
- **Day closed.** `dayClosed(outstandingMeetings, undecidedItems)` is true only
  when both counts are known and zero: the ended meetings that are neither
  reviewed (`meeting_readiness.followThrough`, from the notes, so it agrees on
  every synced device) nor set aside with the device-local **No Follow-through**,
  and the loose-end planned items. A count still loading, or a follow-through
  state that couldn't be checked, is unknown, never closed. Without a connected
  calendar the meeting count is zero. The header shows it only in the evening
  view.
- **Right column (`EveningTomorrow`).** The items carried to tomorrow
  (`carriedToTomorrow`: today's Not today lines deferred to tomorrow's date),
  **Plan Tomorrow**, and tomorrow's day view (`TodayDayView` with `day`), with
  working hours shaded and readiness marks, since the readiness read and the
  unread-from-attendees counts include tomorrow's meetings in the evening view.
  Choosing a meeting prepares it in the left column in place of the review, and
  `p` with no meeting in focus prepares tomorrow's first when today's are over.
  There is no drag (the grid has no plan card to drop), and tomorrow's agenda
  list appears only where the day view gives way to it in a narrow window.
- **Plan Tomorrow's pre-selection.** `usePlanRoutine` starts a fresh routine
  with each Review row whose deferral falls on the routine's own date chosen as
  **Carry**, unless it is unavailable; routines restored from this device keep
  their own choices. This is also what the next morning's Plan Today does for
  what was carried to it, and as it chooses before anything is clicked, closing
  such a routine with Escape asks whether to keep or discard those choices.
- **Backdrop.** `TodayBackdrop` (`src/TodayBackdrop.tsx`) is drawn in SVG from
  `--backdrop-*` tokens, behind `.today-header` only (`z-index: 0`, the header's
  own children positioned above it), `aria-hidden`, with no pointer events. Day:
  gradient sky, clouds, and a sun whose height is `sunHeight(now)`, a half sine
  from 6 am to 6 pm. Evening: dusk gradient, a crescent moon, and seven fixed
  stars. Every token is a flat color checked in `styleTokens.test.ts` for 4.5:1
  against `--text`, `--secondary`, `--muted`, and `--link` in both themes. The
  one motion is a `--duration-slow` fade when it mounts, which the global
  reduced-motion rule collapses. **Settings → Appearance → Today Backdrop**
  (`TodayBackdropSettings`) stores `on` or `off` in
  `threestrands.today.backdrop`, like Start With: it is not part of
  `ExportablePreferences`, and `src-tauri/src/transfer.rs` is unchanged.
- Differences from the design above: Today's evening view keeps **Edit Plan**
  and the progress line in the header; the carried list is read from today's
  `dailyPlan` rather than from the review; no "Planned" breakdown by earlier plan
  day appears in the evening Loose ends (the Review step keeps it); and the
  reviewed day's meetings come from the live schedule rather than only from the
  review's cache.

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

- **Marker handling in the note editor.** `toEditorMarkdown` and
  `fromEditorMarkdown` (`src/noteEntities.ts`) hide the trailing inert comments
  of any list item (plan decisions, task ids, and the follow-through marker) by
  moving them onto an indented block comment, which the editor draws as nothing
  in every syntax mode, and folding them back on save. Only a validated run of
  markers, each after a single space at the end of a live list item (outside
  code, quotes, and open comments), moves; anything else stays authored
  content. Saving also takes back the blank lines the editor adds between the
  items of a list holding a hidden block, so an untouched note saves byte for
  byte, and a note saved by the previous release's editor, which showed the
  markers inline, loads and saves unchanged.

## Native and client contracts

Names are proposed, not existing APIs.

| Layer | Work |
| --- | --- |
| Markdown | Plan section parsing, plan item kinds, decision markers, and item-level insertion/removal in the shared note parser, with shared TS/Rust fixtures. Extend the editor to keep plan markers hidden and preserved. |
| Native | `daily_plan(date)` returns parsed items with resolved state, settledness, and per-item availability. `plan_review(date)` returns the Done and Didn't get to columns: the reviewed plan's finished items, attended meetings, and tasks completed that day outside the plan, then unsettled items from the lookback window and due deferrals, each with its carry count. `plan_history(targets, from, to)` returns carry counts and planned days for the given targets. `plan_sources(source, scope, cursor)` returns one page of candidates for a source, using existing local queries for tasks, starred conversations, promises, Keep in Touch, goals, and note tasks. `save_daily_plan(date, expectedRevision, operationId, changes)` applies item-level changes in one guarded transaction. Index plan items in the existing note entity link rows so settledness queries do not reparse every note. |
| Booking | Note task references as event source tasks, in `taskBooking.ts`, the calendar model, and `CalendarTasks`. |
| Shared day grid | Extract the one-day time grid and the pointer task-block drag (threshold, preview, edge scrolling, Escape, booking with Undo) from `CalendarWeekView` into a shared component and hook. Calendar's views use them unchanged; Today and the Fit step render one day. Drop targets accept a plan card source as well as a Calendar task row. |
| UI/controller | `PlanTodayDialog` and `ShutDownDialog` with `useDailyPlan`; `TodayPlanColumn` (cards), `PlanSourceSwitcher` (shared by the Choose step and Today), `PlanReviewColumns` (shared by Review and Shut Down), `TodayDayView`, and `TodayMailSignal` in a reshaped `TodayWorkspace`; `planCapacity` in `todayInsights.ts`. The meeting step reuses `MeetingReadinessList`; the day view's readiness indicators use the same `meeting_readiness` results. |
| Typed client/demo | Extend `src/domain.ts`, `src/data/client.ts`, and `src/data/demoClient.ts` with plan and review shapes and fixtures. |

## As built (stage 1)

Where the stage 1 contracts differ from the text above, and why:

- **No plan item index table.** History, settledness, and due deferrals read
  plan sections from daily notes on this device, but only the bounded set that
  can matter: notes linking a target come from the existing `note_entity_links`
  target index, and daily notes in a date range are a primary-key range read,
  since daily note ids are `daily:YYYY-MM-DD`. A round task typed in a plan is
  found through its own note, which its target names. So there is no schema
  change, and nothing to rebuild or keep in step with note edits.
- **`plan_history(targets, from, to)`** accepts at most 200 targets
  (`MAX_PLAN_HISTORY_TARGETS`) and a range of at most `PLAN_HISTORY_DAYS`
  dates. A day counts once per target; a carry counts when any of that day's
  plan items for the target has a `carried` marker. `daily_plan(date)` returns
  `carryCount` on each linked item over the 90 days ending on its date.
- **Settled also by the same day's Not today.** Shut Down records decisions
  about a day's unfinished items in that day's own `### Not today`, so a Not
  today mention in the item's own plan settles it, as a later plan's mention
  does. Otherwise a deferral to tomorrow would also show the item as unsettled.
- **Review.** `plan_review(date)` returns `reviewed` (the latest earlier plan
  with plan items within `PLAN_LOOKBACK_DAYS`), `done`, `alsoDone` (at most
  50), `meetings`, and `didntGetTo`. Unavailable items stay in Didn't get to
  so they can be dropped; cancelled items appear in neither column. Due
  deferrals are found in plans as old as `PLAN_HISTORY_DAYS`, not only the
  seven-day window, by reading only daily notes whose text has a deferral
  marker. A note task's completion time is when this device first saw it
  checked, so one finished on another device counts on the day it synced.
  `meetings` comes from the calendar cache and is `null` when nothing of that
  day is cached for a connected calendar: unknown, never an empty day. Copies
  of one meeting on two calendars count once.
- **Sources.** `plan_sources(date, source, accountId, cursor)` takes the plan
  date too, so Plan Tomorrow ranks by tomorrow's dates ("due" means due on the
  plan date). The cursor is an opaque string. Each candidate carries `key`,
  `title`, `reason`, `detail`, `accountId`, `due`, `taskId`, `threadId`,
  `contactId`, `goal`, `plannedDays`, `openTasks`, `planned` (already a plan
  item that day), and an `action`: `link` with the `PlanAddition` to save,
  `makeTask` for a promise, `taskForm` for a starred conversation without an
  open task, or `newTaskForGoal`. A promise or a starred conversation without a
  task is keyed by its conversation target until the task exists. Waiting-for
  tasks count as past their wait date when their due date is on or before the
  plan date. The Goals source lists each current goal (key `goal:<id>`, with
  its open task count) followed by its open tasks; planned days are limited to
  the goal's period and the last 90 days. Dismissed promises are a per-device
  list, so the client filters them as Today does.
- **Capacity.** `planCapacity(events, workingHours, now, day?)` takes the
  booked blocks as part of `events` (they are events with a source task),
  returns `null` for an unknown schedule or working hours, and reports working
  minutes, free minutes, free stretches (each marked `holdsBlock` and `named`),
  and booked blocks in working hours. Working-location entries are not busy.
- **Note-task booking.** A note task's block stores its Tasks id,
  `note:<noteId>:<markerId>`, as the source task. `bookedSource`,
  `bookedTaskFor`, and `bookedBlock` resolve both forms, and
  `openTasksFor(..., { includeNoteTasks: true })` offers note tasks. Calendar's
  Open tasks list includes open note tasks beside account tasks, with the same
  drag-to-book, Book, check-off with Undo, and booked-event Task section. It
  loads `listNoteTasks` with the account tasks and reloads on note changes. Note
  tasks belong to no account, so they show whichever accounts are shown, as in
  Today, and they book on the default writable calendar. Checking one off goes
  through `changeNoteItem`; a conflicted note task's checkbox is disabled. If
  only one source fails to load, the other's tasks stay listed with Try Again.

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
   read/review/save, plan history and candidate source queries, `planCapacity`, and
   note task booking. Gate: a plan round-trips through save, export, import, and an
   older editor's serialization without losing items or markers; repeated saves
   create no duplicate lines; history and source queries are bounded and indexed.
2. **Reshaped Today.** Extract the shared day grid with Calendar's behavior
   unchanged, then the plan cards reading the saved plan with goals and carry
   counts, the source switcher with Starred Mail and Goals, **Add to Today's Plan**,
   the day view with drag-to-book and **Book**, the narrow-width agenda fallback,
   the mail signal replacing the mail column, and meeting readiness on the day view
   (after meeting preparation stage 2). Gate: Today is useful without ever running
   the routine; every Calendar drag and booking test still passes against the
   shared grid; no mail-derived information that the old column provided is lost
   without a path to it.
3. **Plan Today** (built, then removed 2026-11-18). The five-step routine with the Done / Didn't get to review and
   the source switcher, Edit Plan, and Plan Tomorrow. Gate: a user can review,
   choose from every source, prepare, fit, and save a plan with the calendar
   available or not, and restart mid-routine without losing choices.
4. **Shut Down as Today's evening view** (built, then removed 2026-11-18; see [Shut Down](#shut-down-todays-evening-view)),
   with meeting rows completed once meeting preparation stage 3 lands. Gate: the
   day's done items and loose ends show without a routine, decisions appear in the
   next morning's review, Plan Tomorrow starts from the carried items, and the
   day's closed state agrees across synced devices.
5. **Document and validate.** Update the Today, Tasks and Goals, Notes, and Calendar
   help pages, add `changes/<branch>.md` entries for each shipped increment, and
   keep PLAN.MD and this document's status aligned.

## Regression and acceptance coverage

| Area | Required cases |
| --- | --- |
| Parsing | Plan section bounds; `### Not today` exclusion; nested content under items; links inside code or escaped; malformed and duplicate markers; free-text items; plan heading inside a fenced block; a note with Meetings but no Plan. |
| Writes | Draft flush; stale version; conflict; Trash; size limit; unclosed fence; lost response with the same operation ID; Undo after later edits; reorder keeps nested content; removal of an exact item only; text written by hand in the section kept. |
| Review | Lookback boundary below/exact/above seven days; Done includes finished planned items, attended meetings (not declined or upcoming), and unplanned tasks completed that day; note tasks completed on another device without local completion history; Didn't get to ordering; carried, deferred, dropped settledness; deferral due today, past, and future; unavailable targets; the same task planned on two earlier days; plan made on another device; both columns empty skips the step. |
| History | Carry count below/exact/above `PLAN_CARRY_HINT`; history window boundary; counts from a note synced from another device; an old note edited by hand; planned days within a goal's period only; counts never reorder or hide items. |
| Sources | Each source's candidates, paging at `PLAN_SOURCE_PAGE`, and account scope; items already planned marked, not repeated; starred conversation with and without an open task; task form Cancel writes nothing; adding never unstars or archives; dropping a starred card on the day view books only after the form saves; New Task for Goal sets the goal; provisional note task marker saved before linking; selected source restored per device; arrow-key tab navigation. |
| Goals | Goal title on cards for tasks with goals; none on note tasks and check-ins; open task counts; goal from another account labelled. |
| Choose/fit | Suggested candidate reasons; promise becoming a task first; working hours split windows; outside working hours; calendar unavailable; declined and all-day events; booking a note task and a mail task; old events with replicated task ids. |
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

Developer acceptance: plan a day with carried items (one carried three times), a
new task, a starred conversation, and a goal's task, with a block booked by
dragging a card onto the day view and another by **Book**; prepare a meeting from
the routine; complete items from Tasks and Notes; shut down with one deferral and
one carry, checking the Done column; plan tomorrow; open the same day on a
second device; repeat with the calendar offline.

## Proposed measures

Lightweight, explicit evaluation during development, with no telemetry: how often
a plan is made, how many planned items finish or are deliberately carried, deferred,
or dropped, how often items reach the carry hint and what is then decided, which
sources plans draw from (including starred mail), and whether users still open
Inbox from Today for mail the signal did not show.

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
