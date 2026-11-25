# Finishing planning items by email

**Status:** Phases 1, 2 and 3 implemented. **Planned:** 2026-11-25
against 0.211.0. Revised the same day to a smaller scope.

## Outcome

When an email is how a planning item gets done, sending it should finish the
item and leave a record. Today the user writes the email, sends it, then goes to
**Today** to check the item off, and nothing ties the item to the message.

Keep the new interface small. Most emails finish nothing, so compose gains no
standing panel section. The user either starts from the item, or gets one
suggestion after sending.

## What exists today

- **Commitments that aren't tasks clear themselves.** A **You Said You Would**
  promise drops out once any later mail goes to that recipient
  (`list_open_promises`). A possible reply is listed only while the newest
  message in the conversation is the other person's, so replying clears it
  (`list_commitment_replies`). Neither needs a link.
- **Keep in Touch due dates count mail** in either direction
  (`keep_in_touch_due_at`). But a check-in on **today's plan** was marked done
  only by **Mark Contacted**, so emailing the person cleared the reminder and left
  the plan item open. Phase 1 fixes that.
- **Today's Done card** already lists the conversations the user sent mail in
  that day (`list_sent_between`), next to completed tasks, but doesn't connect
  the two.
- **Drafts already carry a task link.** `Draft.followUpTaskId` is saved with the
  draft, kept across server draft pulls, and acted on when the message sends,
  both immediately (`useCorrespondence.tsx`) and from the outbox
  (`db/scheduled_send.rs`). It only moves a repeating follow-up to its next due
  date; `record_follow_up` refuses every other task.
- **Notices** (`useNotice`) can carry one action button.

## Phase 1: check-ins on today's plan count email

A check-in plan item is done when the user emailed the contact on or after the
plan's day, as well as after **Mark Contacted**. Only mail the user sent counts:
the plan item is something the user does. Mail to any of a saved contact's
addresses counts, from any account. A check-in for an address without a saved
contact counts mail to that address.

"The plan's day" is the user's local day. The app passes the instant local
midnight falls (`dayStart`) with the plan request. Before this, the check
compared the UTC date of **Mark Contacted** with the local plan date, so for
users east of UTC a morning **Mark Contacted** didn't count for that day. Without
`dayStart`, the backend falls back to UTC midnight.

Changelog: **Fixed**.

## Phase 2: Write Email from the planning inbox

Add **Write Email** to planning-inbox tasks, including commitments that are tasks
(tracked items and due follow-ups). It opens a draft linked to the task:

- a task without a conversation: a new message, subject from the task title;
- a task with a conversation: a plain reply to the conversation's newest message,
  with no drafting (**Draft Follow-up** keeps its AI instruction).

The draft shows one line under its header, "Completes: *task title*", with a
button to unlink. That's the only compose interface. A check-in gets **Write
Email** too, opening a new message to the contact, with no link: Phase 1 already
counts the mail.

**Finishing at send time.** One native function, `Database::finish_linked_tasks_on`,
runs inside `change_outbox` in the transaction that moves an outbox row to
`sent`, guarded so a row that is already `sent` doesn't run it again. Immediate
and scheduled sends, and an uncertain send later found in sent mail, all pass
through it; a failed, undone, or cancelled send never reaches `sent`, so it
finishes nothing. It reads the outbox row's draft:

- `followUpTaskId` records a follow-up, as before. This replaces both the
  scheduled-only block in `record_schedule_report_on` and the frontend
  `recordFollowUp` call at queue time (which advanced a follow-up even when the
  send was undone).
- `completesTaskId` records a follow-up when the task is a repeating
  follow-up, and otherwise completes the task with `completion_source = 'user'`.
  A task that is missing, finished, or can't take the change is left alone, and
  the send never fails because of it (the work sits in a savepoint).
- The changed task fields are recorded for sync in the same transaction.
- The draft link is its own field, `completesTaskId`, not `followUpTaskId`: an
  older version that sends a synced draft would call `record_follow_up` on a
  one-off task, show an error, and leave the task open. **Draft Follow-up**
  keeps `followUpTaskId` as now. Unlinking is a saved edit that clears it.
- No new `completion_source` value; the column's `CHECK` constraint would make
  an older device fail to apply the synced write.
- After the message leaves, `useCorrespondence` bumps `linkedSendRevision` when
  it sees a linked outbox row go to `sent`, and `App` refreshes the task lists.

**The record.** The task gains an optional `completedByEmail` (a nullable JSON
column and a replicated field): the account, the draft's subject, the send time
and the message key. The key is the outbox row's `provider_id`, the same value
a conversation matches against a synced message's id to replace its optimistic
copy of a send (`queuedReplies.ts`) and that **Edit as New** reads, for Gmail
and IMAP alike. A new local lookup, `find_completed_message`, finds it on this
device or says it isn't there; nothing is resolved in the background. When the id
misses (an IMAP server that assigns its own ids can give the synced copy a
different id than the send receipt), it falls back, with no provider branching, to
the one message in the account that its own addresses sent with the same subject
(reply and forward prefixes ignored), to the recorded To and Cc addresses, dated
from 60 seconds before to 10 minutes after the send, and inside the reply's
conversation when there is one. The record keeps the reply's provider thread id and
up to twenty normalized recipients for this. None or several matches say the
message isn't on this device yet rather than guessing. Reopening the
task clears the record, and a record on a task that isn't completed is ignored,
because a device without the field can reopen a task. It shows as **Done by
email** with the subject in the task editor and on the task's row in **Today**'s
**Completed** card. The task's `threadId` is unchanged.

Older versions ignore both new fields: neither `Draft` nor `ThreadTask` rejects
unknown fields, and task sync payloads are only checked for the fields they
know. Drafts no longer replicate (their records are ignored; drafts reach other
devices as server drafts, which don't carry the link), so the link stays on the
device that wrote it, like `followUpTaskId`. Tasks are not in the encrypted
settings export.

Changelog: **New**, plus **Fixed** for undo-send no longer advancing a follow-up.

## Phase 3: suggest after sending

For when the user didn't start from Today. After **Send**, **Schedule** or
**Send & Archive** queues a draft that has neither `completesTaskId` nor
`followUpTaskId`, one notice reads "Mark “*task title*” done?" with **Mark
Done**. It shows only for one strong match, and nothing otherwise. The match is
`suggest_task_for_outbox(outboxId, date, dayStart)` (`db/task_suggestions.rs`),
which reads the queued row's draft. Candidates are open or in-progress mail and
standalone tasks (note tasks are not in the table, so a planned note task never
matches):

- a task linked to the draft's conversation (the draft's provider thread id,
  mapped to the local thread of that account); or
- a task that is on the day's plan (`daily_plan_from`), or due that day or
  overdue (a date on or before the day; a datetime before the end of the local
  day, taken as 24 hours after `dayStart`), **and** that relates to a
  recipient: its conversation includes the recipient (the `list_contact_tasks`
  rule), or its title names one.

A title names a recipient with a case-insensitive whole-word match of the
address, the full display name, or the first name (at least
`MIN_NAME_MATCH_CHARS` = 3 letters). Names come from the draft's To and Cc and
the saved contact. Recipients are To and Cc without the account's own
addresses; Bcc is ignored. Two or more candidates, or an error (logged, never a
failed send), show nothing.

**Mark Done links; it doesn't complete in place.** `link_task_to_outbox(outboxId,
taskId)` runs in one transaction on the one connection:

- the row is queued (`undo_pending`, `ready`, `scheduled`, `sending`,
  `uncertain`) and the payload has no link: it sets `completesTaskId` in the
  payload, and the Phase 2 hook finishes the task when the row becomes `sent`;
- the row is already `sent`: it sets the link and calls
  `finish_linked_tasks_on` in the same transaction, so the task completes with
  its record;
- a failed, canceled or missing row, a finished task, or a draft that already
  has a link is refused ("That message wasn't sent", and so on).

`change_outbox` takes the same connection lock, so a send becomes `sent` wholly
before or wholly after a link: the hook either reads the link or the link call
finishes the task itself. Only the stored `payload` changes; the raw message was
built at queue time and isn't touched. The notice then says "Marked “*title*”
done" or "“*title*” will be marked done when it’s sent". Undoing the send later
finishes nothing (Phase 2), and the restored draft keeps its **Completes** link.

The notice is set from `onQueued`. It is skipped when the send's own notice
carries Undo or a newer notice has appeared meanwhile (`useSendTaskSuggestion`).
A completion refreshes the task lists through `linkedSendRevision`, or directly
when the task finished at once.

**AI later, only if matching misses too often.** Conversation analysis already
produces task proposals with confidence and evidence (`ActionProposal`); "this
sent email finishes task X" has the same shape. It would only run with AI on, it
sends the message body and open task titles to the provider, and it adds a delay
before the notice. Start without it.

Changelog: added to the Phase 2 entry (unreleased).

## Not doing

- A planning section in the compose side panel. It would sit in every compose for
  an action most emails don't take.
- Turning promises and possible replies into tasks at send time. They clear
  themselves.
- Linking more than one item per email, or linking from a reply inside the
  reader, until there's evidence they're needed.
- Linking by default. Linking is always the user's choice.

## Tests

- Phase 1 (`db/tests/daily_plan.rs`): mail sent on the plan's local day counts,
  including mail sent before UTC midnight for a user east of UTC; mail sent the
  local day before doesn't; mail to a second address of the contact counts; mail
  to an address with no saved contact counts; received mail doesn't; **Mark
  Contacted** still counts. Client test that `dailyPlan` sends `dayStart`.
- Phase 2: Rust tests that sent completes a linked one-off task and records a
  repeating follow-up, and that a failed send, a cancelled scheduled send, or an
  already done task changes nothing. Render the compose header line directly.
  Test the record's lookup when the message is and isn't on this device. Compose
  changes also need `pnpm test:e2e`.
- Phase 3 (`db/tests/task_suggestions.rs`): the matcher for a reply, a planned
  task naming a first name, a due task in a conversation with the recipient,
  two candidates, a task that is neither planned nor due, finished and note
  tasks, own addresses, Bcc, the minimum name length, whole words, and the local
  day east of UTC; the link for each send state, an already-sent message, a
  refused second link, and an undone send. `useSendTaskSuggestion.test.ts` covers
  the notices (match, none, already linked, **Mark Done** both ways, an Undo
  notice, a stale answer).
