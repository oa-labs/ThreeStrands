---
title: Plan Your Day with Today
nav: Today
summary: Plan the day beside a view of today's calendar, with tasks, promises, check-ins, and a line about mail.
reviewed: 0.179.0
---

## Open Today {#open-today}

Click **Today** in the sidebar or press <kbd>0</kbd>. To start there every time, choose **Today** under **Settings → Appearance → Start With**. Otherwise the app remembers the view you last used.

<ts-shot scene="today" alt="Today with the day's plan and its sources on the left and a time grid of today on the right."></ts-shot>

Today has two columns: your plan for the day on the left, and a view of today's calendar on the right. A line in the header sums up your mail. In the evening, Today turns into a [review of the day](#review-your-day) with tomorrow on the right.

## Read Your Day {#read-your-day}

A line under the date sums up the day's meetings: "2 meetings · next at 2:00 pm", the meeting you're in and when it ends, or "2 meetings, both finished · last ended 3:30 pm", along with any double-booking still ahead. Once your plan has items, the next line says how many are done, such as "3 of 5 planned done". The day view needs a [Google Calendar connection](calendar.html#connect-google-calendar).

- The day view shows today as a time grid with the **Now** line. It opens on your working hours (the **Default** profile in **Settings → Availability → Profiles**), starting a little before now once the working day has begun, and shades the time outside them; evening hours scroll inside it. Without working hours, it opens on now.
- Finished and declined meetings are dimmed. A short meeting shows its start time after its title. A highlighted outline means the meeting you're preparing.
- **Tomorrow** below it shows the first few events the next day.
- **Join** opens your next online meeting.
- **Add Note**, beside your next meeting, writes a line about the meeting in that day's note. See [Meeting Notes](notes.html#meeting-notes).
- Click a meeting to [prepare for it](#get-ready-for-meetings). Click anything else, such as a working location, to open it in Calendar.
- In a narrow window, the day view gives way to the list of today's events, which marks overlapping meetings and says when a session will be **Recorded**. Dragging onto the day isn't available there; **Book** still works.

## Review Your Day {#review-your-day}

In the evening, Today shows what the day came to and what is left over, with tomorrow beside it. It opens by itself once your last meeting has ended and your working hours are over (the **Default** profile in **Settings → Availability → Profiles**, or 6 pm without working hours). Earlier, click **Review Today** in the header, or choose **Review Today** in the [command palette](keyboard-shortcuts.html), <kbd>⌘/Ctrl + k</kbd>, which switches between the two. **Back to Today's Plan** returns to the day. The view you choose lasts for the rest of the day on this computer. Nothing reminds you to use it.

**Today in review**, on the left:

- **Done today**: the plan items you finished, the meetings you attended, and other tasks you finished today. Click one to open it.
- **Loose ends**: **Unfinished planned items**, each with **Carry to Tomorrow**, **Defer…** (pick a date from tomorrow on in a small window), and **Drop**. Each choice is saved to today's note at once, and **Undo** in the notice takes it back. Anything you leave alone comes back in tomorrow morning's review. Below them, the day's finished meetings show whether you reviewed their follow-through (**Review Follow-through**, or **No Follow-through** to set one aside on this computer), the promises you made in today's sent mail offer **Make a Task** or **Dismiss**, and one line counts the mail that needs a reply and opens your inbox.
- **Notes on the day**: **Add Notes on the Day…** opens a small window; what you write is added as a paragraph under **Shutdown** at the end of the Plan section of today's note, with **Undo**.

The header shows **Day closed** when every finished meeting has been reviewed or set aside and no planned item is left undecided. It follows what is saved in your notes, so it agrees on your other computers once they sync; only **No Follow-through** stays on one computer.

**Tomorrow**, on the right, lists what you carried to tomorrow, then tomorrow's calendar as a day view with your working hours shaded and the same preparation marks as today's meetings. Click a meeting to [prepare for it](#get-ready-for-meetings); preparation takes the left column in place of the review, and <kbd>Escape</kbd> returns to it. **Plan Tomorrow** opens [Plan Today](#plan-today) for tomorrow with what you carried already chosen.

## Plan Your Day {#plan-your-day}

**Today's plan** lists what you chose to do today, in order. The first item not yet done is your focus and is highlighted. Each card shows where it comes from (a mail account, a note, or **Keep in Touch**), its goal when it has one, the time you booked for it, and how many times it has been carried over from an earlier day ("Carried 3 times"). Pointing at or focusing a card highlights its booked time in the day view, and the other way around.

- Check a task off on its card. Click a card to open the task, its note, or the person.
- **Book** reserves the next free half hour today, on the quarter hour, within your working hours (the **Default** profile in **Settings → Availability → Profiles**) and around the meetings you're attending. If no half hour is left, Today says so; if your calendar couldn't be read, it says free time is unknown rather than guessing. **Undo** removes the booked time.
- **Remove from Today's Plan** takes the item off the plan without changing the task. **Undo** puts it back.

The plan is the **Plan** section of today's daily note in [Notes](notes.html), so you can also read and edit it there. Planning something never changes its due date.

### Plan Today {#plan-today}

**Plan Today** in Today's header (or in the [command palette](keyboard-shortcuts.html), <kbd>⌘/Ctrl + k</kbd>) walks you through the day. Once your plan has items, the header shows its progress and **Edit Plan**, which starts from the saved plan. Late in the day, after your last meeting or in the last hour of your working hours, the header also offers **Plan Tomorrow**, which plans tomorrow's note the same way (the [evening view](#review-your-day) has it beside tomorrow); the palette has **Plan Tomorrow** at any time. Items you carried to tomorrow start out chosen as **Carry** in tomorrow's review.

Move between the steps with **Next** and **Back**, or jump to one in the step list; every step can be skipped. Nothing is written to your note until you save.

1. **Review**: what you got done from your last plan (finished items, the meetings you attended, and other tasks you finished that day), and what you **Didn't Get To**. For each, choose **Carry** (into this plan), **Defer…** (until a later date), or **Drop**, or **Carry All**. Items you leave undecided come back next time. An item carried three times or more asks whether to defer or drop it, but nothing is decided for you. Without anything to review, the step is skipped.
2. **Choose**: the same sources as Today, a search of every open task, and **New Task** to type a task that lives in the plan. Your choices form an ordered list; move items with the arrows or <kbd>Option/Alt + ↑</kbd> and <kbd>Option/Alt + ↓</kbd>. Removing an item that was already in the plan takes its line out of the note. Past five items, Today notes that most people finish three to five.
3. **Meetings**: the day's meetings with what is ready for them, **Reserve Prep Time**, and **No Prep Needed**. **Prepare** opens the meeting's preparation inside Plan Today; **Back to Meetings** returns with your choices as they were.
4. **Fit**: your choices beside the day's calendar, with the free time left in your working hours. **Book** or drag a card onto the day to reserve half an hour. Booking happens right away, with **Undo**, and stays even if you don't save the plan.
5. **Save**: exactly what will be written to the note, and any time already booked. **Save Plan** (or <kbd>⌘/Ctrl + Enter</kbd> from any step) writes it all at once; **Undo** in the notice reverses it if the note hasn't changed since. **Copy Plan** copies the plan as plain text.

Carried items are marked with the day they came from, and deferred or dropped ones are listed under **Not today** in the note, so the next review knows they are settled. Press <kbd>Escape</kbd> to close: Plan Today asks whether to keep your choices for later or discard them. Kept choices, and choices left when the app closes, stay on this computer for a week and return the next time you plan that day, with **Discard** to start over.

### Add to the Plan {#add-to-the-plan}

Below the plan, choose a source. Today opens on **Suggested**; **Starred Mail**, **Goals**, and **Notes** are under **More Sources** (⋯).

- **Suggested**: what is pressing, each with its reason: overdue and due-today tasks, tasks in progress, tasks waiting past their date, promises, and people due for a check-in.
- **Also Due**: tasks overdue or due today that aren't in the plan yet.
- **Tasks**: all your open tasks.
- **You Said You Would**: [promises](#you-said-you-would) from your sent mail.
- **Keep in Touch**: people due for a check-in.
- **Starred Mail**: conversations you starred, newest first. One that already has an open task offers that task. Otherwise **Add to Today's Plan** opens a new task from the conversation for you to save; it's added only when you save, and **Cancel** changes nothing. Adding never unstars or archives the conversation.
- **Goals**: your goals for the current period, each followed by its open tasks and the days they were planned ("Planned Mon, Tue"). **New Task for Goal** opens a new task with the goal chosen and adds it once you save.
- **Notes**: open tasks in your other notes, with the note's name. Click one to open its note.

Suggested, Starred Mail, Goals, and Notes show 50 at a time; **Show More** reads the next 50. When the source you're on has nothing, Today says so in one line and shows the first few cards of the next source that has some, with a link to switch to it. Use the arrow keys to move between sources, including those under **More Sources**; Today remembers the one you chose. **Open Tasks** is at the foot of **Tasks**. **Add to Today's Plan** adds a card to the plan. A promise becomes a task first, which opens for you to adjust. Cards already in the plan say **In plan**.

Drag a card from the plan or a source onto the day view to book half an hour at that time, even outside your working hours (Today mentions it when it is). Dragging a source card also adds it to the plan; a starred conversation or a goal opens its new task first and books only once you save it. A **Keep in Touch** card books "Check in with" and the person's name. If the plan can't be changed, for example because the note changed on another device, the time stays booked and **Open Note** lets you fix the note. Dragging is never required: **Add to Today's Plan** and **Book** do the same from the keyboard.

## Get Ready for Meetings {#get-ready-for-meetings}

Meetings still ahead in the day view say what is ready for them when you focus them: whether they have notes, earlier meetings in their series with notes, open commitments, and how many unread conversations their attendees sent you. Small marks before a meeting's title show at a glance that it has notes, how many open commitments it has, and how much unread mail its attendees sent.

- Click a meeting in the day view, click **Prepare** beside your next meeting, or press <kbd>p</kbd> to [prepare for it](calendar.html#prepare-for-a-meeting). Preparation replaces the plan in the left column, and the meeting stays selected in the day view. Press <kbd>↓</kbd> or <kbd>↑</kbd> for the next or previous meeting. Press <kbd>Escape</kbd> or <kbd>p</kbd>, or click the meeting again, to return to the plan.
- Above preparation, **Add Note** (or **Open Note**), **Reserve Prep Time**, **Join**, and **Open in Calendar** offer the next steps. While you prepare, the next-meeting link in the header keeps only its link.
- **Reserve Prep Time** adds a task, "Prepare for" and the meeting's name, under the meeting's line in that day's note, adding the line if the note doesn't have one. It appears in **Tasks**. Doing it again finds the task already there.
- Working locations, such as **Home** or **Office**, aren't meetings, so they offer no **Prepare** or **Add Note**; clicking one opens it in Calendar.

## Keep an Eye on Mail {#keep-an-eye-on-mail}

The **Mail** line in the header lists calendar invitations by name, since they need an answer in Calendar, and counts what **needs a reply** (messages written to you and at most two others by people you write to) and the rest of your unread mail. Click an invitation to open it, or **Open Inbox** for everything else. The arrow before **Mail** folds the line to a short summary; Today remembers that on this computer.

The mail line and the mail and task sources (**Suggested**, **Also Due**, **Tasks**, **You Said You Would**, **Starred Mail**, and **Goals**) follow the selected mail account, which they name beside them; **Keep in Touch** and **Notes** don't. The plan and the day view cover every account: the day view includes every calendar you show in **Calendar**. If it looks wrong, check [calendar selection](troubleshooting.html#calendar-availability-looks-wrong). **Open Calendar** leads to the full workspace.

## You Said You Would {#you-said-you-would}

**You Said You Would** finds promises in your own sent mail from the last two weeks, such as “I'll send the report over,” when you have not written to that person since. Click one to open its conversation, turn it into a task, add it to your plan, or dismiss it. Detection runs on your computer without AI. Review the suggestion; it may not know you already finished the work elsewhere.
