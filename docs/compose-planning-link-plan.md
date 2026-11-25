# Finishing planning items by email

**Status:** Phase 1 implemented; Phases 2–3 proposed. **Planned:** 2026-11-25
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
- a task with a conversation: a reply, as **Draft Follow-up** does now.

The draft shows one line under its header, "Completes: *task title*", with a
button to unlink. That's the only compose interface. A check-in gets **Write
Email** too, opening a new message to the contact, with no link: Phase 1 already
counts the mail.

**Finishing at send time.** Do the work in one native place, on the outbox's
**sent** transition, so scheduled sends, undo-send windows, and failed sends
behave the same. A message that never leaves finishes nothing. First check
whether the immediate send path also goes through that transition; if so, move
the existing `recordFollowUp` call in `useCorrespondence.tsx` there.

- Store the link in a **new** optional draft field (for example
  `completesTaskId`). Don't widen `followUpTaskId`: an older version that sends a
  synced draft would call `record_follow_up` on a one-off task, show an error,
  and leave the task open. **Draft Follow-up** keeps `followUpTaskId` as now.
- On sent, complete the task with `completion_source = 'user'` (the user chose
  the link), or record the follow-up if it's a repeating follow-up. Leave a task
  that's already done alone. Don't add a `completion_source` value; the column's
  `CHECK` constraint would make an older device fail to apply the synced write.

**The record.** Add an optional task field holding the account and RFC
`Message-ID` of the message that finished it, filled at send time. Don't resolve
it in the background: when the user opens it, look the message up by
`Message-ID`, and say so if it isn't on this device yet. Show it as **Done by
email**, with the subject from the draft, in the task editor and on the task's
row in Today's **Completed** card. Leave the task's `threadId` as it is, so a
standalone task doesn't become a mail task.

Before adding either field, confirm in `replicated-sync-design.md` and the task
and draft sync code that an older device keeps or ignores an unknown field
rather than rejecting the write.

Changelog: **New**.

## Phase 3: suggest after sending

For when the user didn't start from Today. After **Send** (or **Schedule**) on a
draft with no link, show one notice: "Mark '*task title*' done?" with **Mark
Done**. Show it only for one strong match, and nothing otherwise:

- an open task linked to the draft's conversation; or
- an open task on today's plan, or due today or overdue, that is linked to a
  recipient, or whose title names a recipient (display name or address).

With two or more candidates, show nothing. **Mark Done** completes the task now
for a sent message, or sets the Phase 2 link for a scheduled one, so it finishes
when the message leaves. The record is the same as Phase 2. Never complete a task
without the click: a wrong completion quietly hides a task, which is worse than
the extra step.

**AI later, only if matching misses too often.** Conversation analysis already
produces task proposals with confidence and evidence (`ActionProposal`); "this
sent email finishes task X" has the same shape. It would only run with AI on, it
sends the message body and open task titles to the provider, and it adds a delay
before the notice. Start without it.

Changelog: added to the Phase 2 entry if unreleased, else **Improved**.

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
- Phase 3: unit tests for the matcher: one strong match, none, two, a scheduled
  send. One test that the notice's **Mark Done** completes the task.
