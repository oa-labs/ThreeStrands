# Today view design

Today (key `0`) is the day at a glance: the agenda, unread mail, tasks, and
the promises waiting on the user. It's a live dashboard, not a snapshot. It
updates as mail syncs and the clock moves, and every row opens the thing it
describes.

This document records what Today does without AI (phase 1) and the plan for
phase 2, which adds an AI layer on top. Phase 2 takes lessons from a
comparison with Dia's "Monday Brief", an AI-written morning brief built from
Gmail and Google Calendar.

## Principles

- **Complete before curated.** Today never hides mail, meetings, or tasks to
  look tidy. An AI layer can rank and summarize, but the full lists stay
  underneath it, so nothing the model skipped is lost.
- **Every claim is traceable.** A row that tells the user something ("Taylor
  is also in your 2 PM") links to the event, message, or task it came from.
  Guesses are labelled as guesses or left out.
- **Heuristics first, AI on request.** Anything that can be worked out from
  headers, calendar data, or the user's own sent mail runs locally and always.
  AI features follow the AI affordance rules: a sparkles button means data
  goes to the provider, and turning AI off hides the feature.
- **Say what to do and why.** A row's headline is better as the action ("Send
  Jill the report") than as metadata (a sender and a time), and its second line
  is better as the reason than as the subject.

## Phase 1: built without AI

- Agenda for today and tomorrow, with a Now line, declined and finished events
  dimmed, and Next meeting with Join in the header.
- Unread mail in four groups worked out from headers (`src/todayMail.ts`):
  Invitations, Needs a reply, From people, Updates.
- Tasks (overdue, due today, open) and due Keep in Touch reminders.

Added after the Dia comparison:

- **Day summary line.** Under the counts, one line describes the rest of the
  day: "Free until 2:00 pm · 4:00 pm double-booked (CS Updates is recorded) ·
  Last meeting ends 5:00 pm" (`dayShape` in `src/todayInsights.ts`). A gap
  counts as free from `FREE_STRETCH_MINUTES` up.
- **Conflicts on the agenda.** Today's agenda marks each meeting that overlaps
  another ("Overlaps Ramp AI deep dive"). When the event's own text says it is
  recorded, the row adds "Recorded", which tells the user which meeting is
  safer to skip. Declined and all-day events never conflict.
- **People across columns.** A mail row whose sender is invited to a meeting
  still ahead today says so ("Also in your 2:00 pm and 3:00 pm meetings").
  This compares the sender's address with the event's attendees and makes no
  model call.
- **You said you would.** The Tasks column lists promises from the user's own
  sent mail in the last `PROMISE_LOOKBACK_DAYS` (14) days: a first-person
  promise ("I'll send…", "Let me check…") in the user's own text, not in quoted
  history, where nothing the user has sent since went to anyone on its To
  line. Detection is in `crates/mail-text/src/promise_text.rs`; the query is
  `Database::list_open_promises`. A promise whose conversation already has an
  open task is left out. Each row can become a follow-up task, or be dismissed
  on this device.

## Phase 2: the AI layer

Phase 2 was already planned as AI grouping plus a "Plan my day" button. The
Dia comparison sharpens it into two features.

### Sort mail by meaning

Phase 1 sorts mail by how it's addressed: whether the user is on the To line,
how many people are, and whether it's list mail. That reliably separates bulk
mail from personal mail. It can't tell "Paylocity needs you to approve
Taylor's time off today" from "Paylocity: your pay stub is ready", because
both are automated mail from the same sender.

What Dia does well, and what to copy:

- **Group by what the user has to do.** Dia had two groups: *to-dos*
  (something to send, decide, or approve) and *new updates* (something worth
  knowing, with nothing to do). It left everything else out. Ours should be:
  **Needs you** (a reply, a decision, an approval, or a deadline), **Worth
  knowing** (news on a project or account the user follows), and **The rest**.
  The rest stays visible, collapsed, so nothing is hidden.
- **Write the row as an action.** For mail in Needs you, the model writes a
  short headline in the imperative ("Approve Taylor's time-off request for
  today"). The sender and subject move to the second line. For Worth knowing,
  the headline is the news itself ("Synergy pilot starts on 4 injection
  molding presses").
- **Give the reason in one line.** Dia's best rows explain themselves ("On Oct
  22 you told Jill you'd… Nothing has gone to her since"). Each AI headline
  gets a one-sentence reason drawn only from the cited messages.
- **Label by project, not sender.** Dia tagged updates with the account or
  project ("Synergy Industrial", "Hanel"), not the person who wrote. The model
  can propose a project label from the conversation. It's a label for display,
  never a filter rule, so a wrong label costs nothing.

What not to copy:

- **Facts the mail doesn't state.** Dia wrote "likely Coverings, Apr 6–9 in
  Orlando" for a conference the email never named. Our reasons may only
  restate what the cited messages say. A detail the model inferred is either
  marked as a guess or left out. The prompt should forbid outside knowledge,
  and the output should cite message IDs that the code checks.
- **Silent omission.** Dia's brief doesn't say what it left out. Ours always
  shows "The rest (N)".

Mechanics:

- Phase 1 groups stay the fallback and the default when AI is off, so this
  is a layer, not a replacement.
- Send only new mail: the unread conversations since the last sort, each
  conversation's newest inbound message trimmed to its own text, the way
  Briefs focus on new messages. Cache the result per thread revision so
  reopening Today costs nothing.
- Wrap provider calls in `ai::with_purpose` so they appear in **Settings → AI
  Provider → Activity**.
- One sparkles button, **Sort by Meaning**, until the user turns on an
  automatic option under **Settings → AI → Show AI In**, matching Automatic
  Briefs.

### Top of mind strip

Dia's whole brief is, in effect, a strip of five or six items chosen from
everything. For Today, that's a strip above the columns, never a replacement
for them.

What to copy:

- **A small fixed budget.** Three to five items. The strip is worth having
  because it's short. Mail, calendar, tasks, and promises all compete for the
  same few slots.
- **One focus item.** Dia's "Push your work forward" picks the single piece
  of deep work (write the tile-opportunity brief before tomorrow's meeting
  with Mike) and ties it to the meeting it's for. The strip leads with one
  focus item when there's a good candidate: a task or promise linked to an
  upcoming meeting or deadline.
- **Combine sources.** The best items join two or more facts: a time-off
  request plus "Taylor is also in your 2 PM and 3 PM"; a double-booking plus
  "the CS session is recorded and Mike shared the deck". Phase 1 already
  works out the joins (conflicts, recordings, shared meetings, promises). The
  model's job is to choose among them and phrase them, not to discover them.
  That keeps the facts deterministic and cheap.
- **An offer, not an action.** Dia ended its focus item with "I can put
  together a one-page brief". Items may offer a next step (Draft Reply, Make
  a Task, Find Times) that the user starts. Nothing is sent or created
  without the user.
- **One sentence of tone.** "Your morning is wide open, and 4 PM somehow got
  booked twice" reads better than the counts. The deterministic day summary
  line already covers the facts. A model-written line is optional polish and
  must not add facts.

What not to copy:

- A once-a-day snapshot. The strip recalculates when its inputs change, like
  the rest of Today, and an item disappears once it's done: the task checked
  off, the reply sent, the conflict resolved.
- The editorial look (painting, jokes, "Made for you"). Today stays a working
  view.

Mechanics:

- Input is the phase 1 data Today already holds, plus the meaning sort when it
  has run: agenda with conflicts, promises, tasks due, and Needs you mail.
  Each candidate goes in with its source ID, and each strip item must cite at
  least one ID the code can resolve to a row; uncited items are dropped.
- Button-only to start (**Plan My Day**, with sparkles), matching how Find
  Times started.
- Dismissing a strip item hides it for the day on this device, like a
  dismissed promise.

## Not built

- "From mail" event suggestions in Today.
- A "Coming back" section, which needs mail snooze.
- Promise detection in languages other than English, and promises made in
  the Cc'd part of a reply-all (detection reads the To line only).
