# Planning items while composing

**Status:** Proposed. **Planned:** 2026-11-25 against 0.211.0.

## Outcome

When an email is how a planning item gets done, sending it should finish the
item and leave a record. Today the user writes the email, sends it, then goes to
**Today** to check the item off, and nothing ties the item to the message.

While writing, the compose side panel shows open items from the planning inbox.
The user links the email to one of them. When the message actually sends, the
item is done, and the item shows which email finished it.

## What exists today

- **Compose side panel** (`ComposeContext`) shows **Before you send**, the
  first outside recipient, **Find Times**, a **Tasks** section, meetings, recent
  emails, and files. The Tasks section (`ThreadTasks`, labelled "Tasks with this
  person") lists only open tasks tied to a conversation with that recipient, and
  is hidden when there are none. A standalone task such as "Email Dana the
  revised quote" never appears there.
- **Drafts already carry a task link.** `Draft.followUpTaskId` is saved with the
  draft, kept across server draft pulls, and acted on when the message sends,
  both immediately (`useCorrespondence.tsx`) and from the outbox
  (`db/scheduled_send.rs`). It only moves a repeating follow-up to its next due
  date; `record_follow_up` refuses every other task.
- **Planning inbox** (`PlanningInbox` in **Today**) has the filters All, Tasks,
  Emails, Contacts, and Commitments. Commitments are:
  - tracked tasks from mail and due follow-ups (real tasks);
  - **You Said You Would** promises (`OpenPromise`), not tasks until **Make a
    Task**, cleared on their own once any later mail goes to the same recipient,
    or dismissed on this device only;
  - possible replies (`CommitmentReply`), dismissible on this device only.
- **Keep in Touch** due dates already count sent mail: `keep_in_touch_due_at`
  takes the later of the last email and the last **Mark Contacted**. But a
  check-in added to **today's plan** is marked done only by **Mark Contacted**
  (`daily_plan.rs`, `kit_last_touch_at`). Emailing the person clears the Keep in
  Touch reminder and still leaves the plan item open.

## Design

### The Planning section

Replace the compose panel's Tasks section with a **Planning** section, placed
after the recipient summary. It lists open planning-inbox items in three groups,
named as the planning inbox names them. Starred mail is left out.

| Group | Items | What sending a linked email does |
| --- | --- | --- |
| Tasks | Open and in-progress mail, standalone, and note tasks. | Completes the task. A repeating follow-up moves to its next due date instead, as now. |
| Contacts | Due and overdue Keep in Touch check-ins. | Nothing extra. Mail to the contact already counts (see Phase 1). |
| Commitments | Tracked tasks and due follow-ups, promises, possible replies. | A task completes as above. A promise or possible reply becomes a completed task (see Phase 3). |

Order within the section:

1. Items about this email's recipients: tasks with the recipient, check-ins for a
   recipient, promises made to a recipient, possible replies from a recipient.
2. Items on today's plan.
3. Overdue and due today, then the rest in planning-inbox order.

Show the first five, with **Show All** and a filter field. With no recipients
yet, the section still shows today's plan and due items, since the user may start
from the item ("I need to email someone about X").

Each task or commitment row has a **Done When Sent** checkbox. A check-in row for
a recipient says that this email counts as the check-in and has no control. A
check-in for someone not on the draft offers **Add to To**.

Rules:

- **Never link automatically.** Recipient matches only rank. Sometimes an email is
  one step of a task, not the end of it.
- **One link per draft** in the first version. The checkbox works like a radio:
  checking a second row moves the link. Revisit if users ask for more.
- The link is saved with the draft, so it survives closing the draft, switching
  devices, and scheduled send.
- The panel row stays linked and shows "Done when sent" until the message leaves.
  If the draft is discarded, nothing changes.

### Finishing at send time

Do the work in one native place, on the outbox's **sent** transition, so
scheduled sends, undo-send windows, and failed sends behave the same. A message
that never leaves finishes nothing. Move the existing `recordFollowUp` call in
`useCorrespondence.tsx` there if the immediate path also goes through that
transition (check this first).

Store the link in a **new** optional draft field, for example
`planLink: { kind: "task" | "promise" | "reply", id, threadId?, messageId? }`.
Don't widen `followUpTaskId`: an older version that sends a synced draft would
call `record_follow_up` on a one-off task, show an error, and leave the task
open. Keep `followUpTaskId` for **Draft Follow-up**, which keeps working as now.

When the message is sent:

- **Task:** complete it with `completion_source = 'user'` (the user chose the
  link), or record the follow-up if it is a repeating follow-up. A task that is
  already done is left alone.
- **Promise or possible reply:** create an action task from it, as **Make a
  Task** does, already completed, and record the link on it. That moves it out of
  Commitments on every device, which a per-device dismissal doesn't.

Don't add a new `completion_source` value. The column's `CHECK` constraint
rejects unknown values, so an older device would fail to apply the synced write.

### The record

Add an optional task field for the message that finished it: account, RFC
`Message-ID`, and the local message and conversation once the Sent copy syncs.
Fill it at send time from the outgoing message, and resolve the local ids when
the Sent folder copy arrives. Leave the task's own `threadId` as it is, so a
standalone task doesn't silently become a mail task.

Show it as **Done by email** with the subject, opening the message:

- in the task editor;
- on the row in Today's **Completed** card;
- in that conversation's Tasks section, as a checked task.

Before adding the field, confirm in `replicated-sync-design.md` and the task sync
code that an older device keeps or ignores an unknown task field instead of
rejecting the write. Tasks aren't part of the settings export; confirm that too.

### Starting from the item

The user's flow usually starts in **Today**, not in compose. Add **Write Email**
to planning-inbox items, which opens a draft already linked:

- a task without a conversation: a new message, subject from the task title;
- a task with a conversation: the existing **Draft Follow-up** path, plus the
  link;
- a check-in: a new message to the contact;
- a promise or possible reply: a reply in its source conversation, linked.

This uses the same draft field and send-time finish, so it's small once
Phase 2 lands.

## Phases

Each phase ships on its own and adds a `changes/` entry.

1. **Check-ins count email on today's plan.** In `daily_plan.rs`, a contact plan
   item is done when the contact was emailed on or after the plan's day, as well
   as after **Mark Contacted**. Use `contact_interactions` (`direction = 'sent'`)
   across the contact's addresses, as Keep in Touch does. A Rust fix with no new
   UI. It fixes the contact half of the problem by itself.
   Changelog: **Fixed**.
2. **Planning section and Done When Sent for tasks.** The compose panel section,
   the draft field, the native finish on sent, and the **Done by email** record.
   Changelog: **New**.
3. **Promises and possible replies.** Make the completed task at send time.
   Changelog: added to the Phase 2 entry if unreleased, else **Improved**.
4. **Write Email from Today.** Changelog: added to the Phase 2 entry if
   unreleased, else **Improved**.
5. **Replies in an open conversation** (optional). A reply written inside the
   reader uses the conversation's panel, not `ComposeContext`. Add **Done When
   Sent** to that panel's Tasks rows. Decide after Phase 2 is in use.

## Tests

- Phase 1: Rust tests in `daily_plan.rs`. A check-in is done after mail sent that
  day; still open after mail sent the day before; done for mail to any of the
  contact's addresses; **Mark Contacted** still works; received mail doesn't
  count.
- Phase 2:
  - Render `ComposeContext` directly: grouping and order, recipient ranking, the
    five-row limit with **Show All**, one link at a time, check-in rows without a
    control, and help text attached with `toHaveAccessibleDescription()`.
  - Rust: sent finishes a one-off task and records a repeating follow-up, and
    does nothing for a failed send, a cancelled scheduled send, or an already
    done task. Also test the record's later resolution once the Sent copy syncs.
  - Sync: an older device applying the task write keeps working.
  - A slim `App` test only if the panel's wiring to the draft isn't otherwise
    covered. Compose changes also need `pnpm test:e2e`.
- Phase 3: Rust tests that a linked promise and a linked possible reply each
  become one completed task with the record, and leave Commitments.

## Open questions

- Should **Done When Sent** default on when an item matches the draft's only
  recipient and is on today's plan? This plan says no and only ranks; reconsider
  if users find the extra click annoying.
- Should a linked email also add the task's goal or notes anywhere? Out of scope
  here.
