# Conversation snooze

Implemented in 0.190.0 for Gmail and IMAP/SMTP. The time belongs to ThreeStrands;
archiving and restoring belong to the mail provider. Gmail removes/adds INBOX;
IMAP uses its existing mapped Archive/Inbox move contract. There is no new
provider folder and this does not control Gmail's native Snoozed state.

## Behavior

The keyboard bindings follow Superhuman: **h** snoozes (Remind Me), and **b**
creates a calendar event from Mail or Calendar. Calendar also retains Shift+N.

Snooze applies to a whole conversation or checked batch, preserves read state,
and supports undo, rescheduling, a Snoozed view, and Return to Inbox. A new
incoming message ends snooze; sent mail does not. Baseline message IDs distinguish
new arrivals even with skewed Date headers. Explicit archive/restore, trash, and
spam actions in ThreeStrands cancel the schedule. Provider trash/spam evidence
also cancels it. Another client's Archive action on already archived mail is
indistinguishable from no action, so it cannot cancel a ThreeStrands snooze.
Tasks and follow-ups are independent.

A native clock checks accounts with due or unmaterialized schedules every mail
poll interval. The serialized account sync fetches fresh provider evidence before
returning mail, so overdue startup/resume work does not resurrect provider trash.
Offline work queues its archive immediately and catches up when connected. No
cloud service runs when all devices are closed; returns need a running, online,
authorized installation. Returning to Inbox does not change the email's Date.

## Persistence and replication

Schema 91 stores schedules independently of mail-cache rows, including terminal
returned/cancelled state. Creating/rescheduling, cancellation, and return commit
with the provider mutation queue and, when enabled, the replica write in one
transaction. A whole schedule is one atomic replicated field, preventing hybrid
concurrent updates. Existing replica conflict resolution handles concurrent edits.
Local edits made while replication is off remain dirty until sync resumes.

Optional encrypted sync carries account/thread/message references, Message-ID
reference tokens for IMAP recovery, return time, and disposition. It contains no
message bodies, subjects, attachments, or credentials. Settings exports do not
include snoozes. Update every device before sharing this new entity type; older
apps cannot read snapshots containing it.

An imported schedule can precede cached mail. Pending application is persisted
and waits for provider hydration; it is not discarded because the cache is empty.
Active snoozes pin cached mail against retention and hold IMAP locations outside
the normal archive window, independently of expiring search pins. Server expunges
still remove locations. IMAP recovery uses bounded existing search acquisition,
UIDVALIDITY checks, quoted Message-ID searches, and the normal server-search
deadline. Unlocatable mail (including missing references) remains pending with a
sync error, rather than treating a cache miss as proof of deletion. Provider
thread aliases transfer an active schedule to the canonical conversation.

## Verification

Database tests cover exact/overdue boundaries, provider mutations, read state,
new replies versus own mail, cancellation, undo/rescheduling, atomic batches,
retention, aliasing, replication and disabled-sync edits, and hydration before
mail arrives. IMAP protocol fixtures cover location retention and reference
recovery. UI tests cover time selection, errors, undo state, keyboard and batch
wiring; existing provider mutation contracts continue to cover Gmail labels and
IMAP moves. Live-account verification remains a developer release check.
